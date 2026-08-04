use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr as _;

use deadreckon_core::gate::{
    AcceptanceContainment, GATE_CONTAINED_ENV, GATE_KEY_ENV, GATE_SANDBOX_BACKEND_ENV,
    GateEvaluation, decode_gate_key, evaluate_gate_with_identity, sign_gate_evaluation_with_key,
};
use deadreckon_core::{
    SupervisedProcessIdentity, SupervisedProcessPhase, read_supervised_process_record,
    write_supervised_process_record,
};
use deadreckon_sandbox::SandboxBackend;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw = std::env::args_os().skip(1);
    let mode = raw
        .next()
        .ok_or("usage: dr-gate <protocol|evaluate|sign> [arguments]")?;
    if mode == OsStr::new("protocol") {
        if raw.next().is_some() {
            return Err("protocol accepts no arguments".into());
        }
        return print_protocol();
    }
    if mode == OsStr::new("guarded-exec") {
        let args = parse_guarded_exec(raw)?;
        return guarded_exec(&args);
    }
    if mode == OsStr::new("probe-boundary") {
        if raw.next().is_some() {
            return Err("probe-boundary accepts no arguments".into());
        }
        return probe_boundary();
    }
    let mode = mode
        .into_string()
        .map_err(|_| "dr-gate command must be valid UTF-8")?;
    let rest = raw
        .map(|value| {
            value
                .into_string()
                .map_err(|_| "dr-gate arguments must be valid UTF-8")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let command = parse_command(std::iter::once(mode).chain(rest))?;
    match command {
        GateCommand::Evaluate(args) => evaluate(&args),
        GateCommand::Sign(args) => sign(args),
    }
}

fn print_protocol() -> Result<(), Box<dyn std::error::Error>> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(
        &mut stdout,
        &serde_json::json!({
            "schema_version": deadreckon_protocol::GATE_EVALUATOR_IDENTITY_SCHEMA_VERSION,
            "protocol_version": deadreckon_protocol::GATE_EVALUATOR_PROTOCOL_VERSION,
            "protocol_marker": deadreckon_protocol::GATE_EVALUATOR_PROTOCOL_MARKER,
            "package_version": env!("CARGO_PKG_VERSION"),
            "bundle_build_id": env!("DEADRECKON_BUNDLE_BUILD_ID"),
        }),
    )?;
    stdout.write_all(b"\n")?;
    Ok(())
}

fn probe_boundary() -> Result<(), Box<dyn std::error::Error>> {
    const SUCCESS: &str = "deadreckon-sandbox-boundary-v1";
    reject_evaluator_gate_environment(|name| std::env::var_os(name).is_some()).map_err(
        |message| format!("dr-gate probe-boundary refused unsafe environment: {message}"),
    )?;
    let gate_key = required_boundary_path("DR_BOUNDARY_GATE_KEY")?;
    let proof = required_boundary_path("DR_BOUNDARY_PROOF")?;
    let control = required_boundary_path("DR_BOUNDARY_CONTROL")?;
    let operator_capture = required_boundary_path("DR_BOUNDARY_OPERATOR_CAPTURE")?;
    if fs::File::open(&gate_key).is_ok() {
        return Err("probe-boundary could read the protected gate key".into());
    }
    for (label, path) in [
        ("proof", &proof),
        ("control", &control),
        ("operator capture", &operator_capture),
    ] {
        if fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
            .is_ok()
        {
            return Err(format!("probe-boundary could write protected {label}").into());
        }
    }
    if fs::File::open(&operator_capture).is_ok() {
        return Err("probe-boundary could read protected operator capture".into());
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(SUCCESS.as_bytes())?;
    Ok(())
}

fn required_boundary_path(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let value = std::env::var_os(name).ok_or_else(|| format!("{name} is required"))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{name} must be absolute").into());
    }
    Ok(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommonArgs {
    run_id: String,
    run_root: PathBuf,
    working_dir: PathBuf,
    gate_evaluator_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SignArgs {
    common: CommonArgs,
    evaluation: String,
    containment: AcceptanceContainment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GateCommand {
    Evaluate(CommonArgs),
    Sign(SignArgs),
}

#[derive(Debug)]
struct GuardedExecArgs {
    metadata: PathBuf,
    launch_id: String,
    attempt: u32,
    release_token_sha256: String,
    program: OsString,
    args: Vec<OsString>,
}

fn evaluate(args: &CommonArgs) -> Result<(), Box<dyn std::error::Error>> {
    reject_evaluator_gate_environment(|name| std::env::var_os(name).is_some())
        .map_err(|message| format!("dr-gate evaluate refused unsafe environment: {message}"))?;
    let evaluation = evaluate_gate_with_identity(
        &args.run_id,
        &args.run_root,
        &args.working_dir,
        args.gate_evaluator_sha256.clone(),
    )?;
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, &evaluation)?;
    stdout.write_all(b"\n")?;
    Ok(())
}

fn sign(args: SignArgs) -> Result<(), Box<dyn std::error::Error>> {
    let raw = read_evaluation(&args.evaluation)?;
    let evaluation: GateEvaluation = serde_json::from_str(&raw)
        .map_err(|error| format!("invalid gate evaluation JSON: {error}"))?;
    if evaluation.gate_evaluator_sha256 != args.common.gate_evaluator_sha256 {
        return Err(format!(
            "gate evaluation identity {:?} does not match approved identity {:?}",
            evaluation.gate_evaluator_sha256, args.common.gate_evaluator_sha256
        )
        .into());
    }
    let environment = SigningEnvironment::from_lookup(
        |name| std::env::var(name).ok(),
        |name| std::env::var_os(name).is_some(),
    )
    .map_err(|message| format!("dr-gate sign refused unsafe supervisor inputs: {message}"))?;
    sign_gate_evaluation_with_key(
        &args.common.run_root,
        &args.common.run_id,
        &args.common.working_dir,
        evaluation,
        &environment.key,
        args.containment,
    )?;
    Ok(())
}

fn read_evaluation(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    if source == "-" {
        let mut raw = String::new();
        io::stdin().read_to_string(&mut raw)?;
        Ok(raw)
    } else {
        Ok(fs::read_to_string(source)?)
    }
}

fn parse_guarded_exec(
    mut args: impl Iterator<Item = OsString>,
) -> Result<GuardedExecArgs, Box<dyn std::error::Error>> {
    let mut metadata = None;
    let mut launch_id = None;
    let mut attempt = None;
    let mut release_token_sha256 = None;
    loop {
        let argument = args
            .next()
            .ok_or("guarded-exec requires -- followed by a command")?;
        if argument == OsStr::new("--") {
            break;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{} requires a value", argument.to_string_lossy()))?;
        match argument.to_str() {
            Some("--metadata") => set_once(&mut metadata, PathBuf::from(value), "--metadata")?,
            Some("--launch-id") => set_once(
                &mut launch_id,
                value
                    .into_string()
                    .map_err(|_| "--launch-id must be valid UTF-8")?,
                "--launch-id",
            )?,
            Some("--attempt") => {
                let value = value
                    .into_string()
                    .map_err(|_| "--attempt must be valid UTF-8")?;
                set_once(
                    &mut attempt,
                    value.parse::<u32>().map_err(|_| {
                        format!("--attempt must be a positive integer, got {value}")
                    })?,
                    "--attempt",
                )?;
            }
            Some("--release-token-sha256") => set_once(
                &mut release_token_sha256,
                value
                    .into_string()
                    .map_err(|_| "--release-token-sha256 must be valid UTF-8")?,
                "--release-token-sha256",
            )?,
            _ => {
                return Err(format!(
                    "unknown guarded-exec argument {}",
                    argument.to_string_lossy()
                )
                .into());
            }
        }
    }
    let program = args.next().ok_or("guarded-exec command is required")?;
    let command_args = args.collect();
    let attempt = attempt
        .filter(|attempt| *attempt > 0)
        .ok_or("--attempt is required and must be positive")?;
    Ok(GuardedExecArgs {
        metadata: metadata.ok_or("--metadata is required")?,
        launch_id: launch_id
            .filter(|value| !value.trim().is_empty())
            .ok_or("--launch-id is required")?,
        attempt,
        release_token_sha256: release_token_sha256
            .filter(|value| !value.trim().is_empty())
            .ok_or("--release-token-sha256 is required")?,
        program,
        args: command_args,
    })
}

fn guarded_exec(args: &GuardedExecArgs) -> Result<(), Box<dyn std::error::Error>> {
    const MAX_RELEASE_TOKEN_BYTES: u64 = 512;
    let mut release_bytes = Vec::new();
    io::stdin()
        .take(MAX_RELEASE_TOKEN_BYTES + 1)
        .read_to_end(&mut release_bytes)?;
    if release_bytes.len() as u64 > MAX_RELEASE_TOKEN_BYTES {
        return Err("guarded-exec release token exceeded its bounded size".into());
    }
    let release_token = std::str::from_utf8(&release_bytes)
        .map_err(|_| "guarded-exec release token was not UTF-8")?
        .trim();
    if release_token.is_empty()
        || deadreckon_core::flight::sha256_text(release_token) != args.release_token_sha256
    {
        return Err("guarded-exec release token did not match its durable launch".into());
    }

    let mut record = read_supervised_process_record(&args.metadata)?;
    let current_pid = std::process::id();
    let observed_identity = record.identity();
    let pid_matches = record.process.pid == current_pid;
    let launch_matches = record.launch_id == args.launch_id;
    let attempt_matches = record.attempt == args.attempt;
    let release_matches = record.release_token_sha256 == args.release_token_sha256;
    let phase_matches = record.phase == SupervisedProcessPhase::Prepared;
    let process_identity_matches = observed_identity == SupervisedProcessIdentity::Current;
    if !pid_matches
        || !launch_matches
        || !attempt_matches
        || !release_matches
        || !phase_matches
        || !process_identity_matches
    {
        return Err(format!(
            "guarded-exec identity did not match its durable prepared record \
             (pid={pid_matches}, launch={launch_matches}, attempt={attempt_matches}, \
             release={release_matches}, phase={phase_matches}, \
             process_identity={process_identity_matches}, observed={observed_identity:?})"
        )
        .into());
    }

    #[cfg(unix)]
    {
        nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0))
            .map_err(|error| format!("guarded-exec could not create its process group: {error}"))?;
        record.process.pgid = Some(record.process.pid);
    }
    record.phase = SupervisedProcessPhase::Running;
    write_supervised_process_record(&args.metadata, &record)?;

    let mut command = Command::new(&args.program);
    command
        .args(&args.args)
        // This internal override is needed only while checking the durable
        // guarded-process identity in restart tests. Never expose it to the
        // approved repository command or let it impersonate a host reboot.
        .env_remove("DEADRECKON_BOOT_ID");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;

        let error = command.exec();
        Err(Box::new(error))
    }
    #[cfg(not(unix))]
    {
        let status = command.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("guarded command exited with status {status}").into())
        }
    }
}

fn parse_command(
    mut args: impl Iterator<Item = String>,
) -> Result<GateCommand, Box<dyn std::error::Error>> {
    let mode = args
        .next()
        .ok_or("usage: dr-gate <evaluate|sign> [arguments]")?;
    let mut run_id = None;
    let mut run_root = None;
    let mut working_dir = None;
    let mut evaluation = None;
    let mut contained = None;
    let mut sandbox_backend = None;
    let mut gate_evaluator_sha256 = None;
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{arg} requires a value"))?;
        match arg.as_str() {
            "--run" => set_once(&mut run_id, value, "--run")?,
            "--run-root" => set_once(&mut run_root, PathBuf::from(value), "--run-root")?,
            "--working-dir" => {
                set_once(&mut working_dir, PathBuf::from(value), "--working-dir")?;
            }
            "--evaluation" => set_once(&mut evaluation, value, "--evaluation")?,
            "--contained" => set_once(&mut contained, parse_bool(&value)?, "--contained")?,
            "--sandbox-backend" => {
                set_once(&mut sandbox_backend, value, "--sandbox-backend")?;
            }
            "--gate-evaluator-sha256" => {
                validate_sha256(&value, "--gate-evaluator-sha256")?;
                set_once(&mut gate_evaluator_sha256, value, "--gate-evaluator-sha256")?;
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let common = CommonArgs {
        run_id: run_id.ok_or("--run is required")?,
        run_root: run_root.ok_or("--run-root is required")?,
        working_dir: working_dir.ok_or("--working-dir is required")?,
        gate_evaluator_sha256,
    };
    match mode.as_str() {
        "evaluate" => {
            reject_sign_only_args(&evaluation, &contained, &sandbox_backend)?;
            Ok(GateCommand::Evaluate(common))
        }
        "sign" => Ok(GateCommand::Sign(SignArgs {
            common,
            evaluation: evaluation.ok_or("--evaluation is required")?,
            containment: explicit_containment(
                contained.ok_or("--contained is required")?,
                &sandbox_backend.ok_or("--sandbox-backend is required")?,
            )?,
        })),
        other => Err(format!("unknown dr-gate command {other}; expected evaluate or sign").into()),
    }
}

fn validate_sha256(value: &str, argument: &str) -> Result<(), Box<dyn std::error::Error>> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(format!("{argument} must use sha256:<64 lowercase hex>").into());
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{argument} must use sha256:<64 lowercase hex>").into());
    }
    Ok(())
}

fn set_once<T>(
    slot: &mut Option<T>,
    value: T,
    argument: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if slot.replace(value).is_some() {
        return Err(format!("{argument} may only be supplied once").into());
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool, Box<dyn std::error::Error>> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("--contained must be true or false, got {value}").into()),
    }
}

fn reject_sign_only_args(
    evaluation: &Option<String>,
    contained: &Option<bool>,
    sandbox_backend: &Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if evaluation.is_some() || contained.is_some() || sandbox_backend.is_some() {
        return Err(
            "--evaluation, --contained and --sandbox-backend are only valid for sign".into(),
        );
    }
    Ok(())
}

fn explicit_containment(
    contained: bool,
    backend: &str,
) -> Result<AcceptanceContainment, Box<dyn std::error::Error>> {
    let backend = SandboxBackend::from_str(backend)?;
    if backend == SandboxBackend::Auto {
        return Err("signing requires the observed backend; auto is not an observation".into());
    }
    if contained != (backend != SandboxBackend::None) {
        return Err(format!(
            "--contained {contained} is inconsistent with observed backend {backend}"
        )
        .into());
    }
    if contained {
        Ok(AcceptanceContainment::contained(backend.to_string()))
    } else {
        Ok(AcceptanceContainment::uncontained(backend.to_string()))
    }
}

fn reject_evaluator_gate_environment(
    mut is_present: impl FnMut(&str) -> bool,
) -> Result<(), String> {
    let present = [GATE_KEY_ENV, GATE_CONTAINED_ENV, GATE_SANDBOX_BACKEND_ENV]
        .into_iter()
        .filter(|name| is_present(name))
        .collect::<Vec<_>>();
    if present.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "keyless evaluation must not receive {}",
            present.join(", ")
        ))
    }
}

struct SigningEnvironment {
    key: Vec<u8>,
}

impl SigningEnvironment {
    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<String>,
        mut is_present: impl FnMut(&str) -> bool,
    ) -> Result<Self, String> {
        let legacy = [GATE_CONTAINED_ENV, GATE_SANDBOX_BACKEND_ENV]
            .into_iter()
            .filter(|name| is_present(name))
            .collect::<Vec<_>>();
        if !legacy.is_empty() {
            return Err(format!(
                "legacy containment environment is forbidden: {}",
                legacy.join(", ")
            ));
        }
        let encoded_key = lookup(GATE_KEY_ENV)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("{GATE_KEY_ENV} is required"))?;
        let key = decode_gate_key(&encoded_key).map_err(|err| err.to_string())?;
        Ok(Self { key })
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use deadreckon_core::gate::{GATE_CONTAINED_ENV, GATE_KEY_ENV, GATE_SANDBOX_BACKEND_ENV};
    use tempfile::TempDir;

    use super::{
        CommonArgs, GateCommand, SignArgs, SigningEnvironment, explicit_containment, parse_command,
        parse_guarded_exec, read_evaluation, reject_evaluator_gate_environment, sign,
    };

    #[test]
    fn compiled_protocol_marker_matches_the_controller_contract() {
        assert_eq!(
            deadreckon_protocol::GATE_EVALUATOR_PROTOCOL_MARKER,
            format!(
                "deadreckon-gate-evaluator-protocol-v{}",
                deadreckon_protocol::GATE_EVALUATOR_PROTOCOL_VERSION
            )
        );
        assert!(
            env!("DEADRECKON_BUNDLE_BUILD_ID").starts_with("deadreckon-bundle-build-id-sha256:")
        );
    }

    fn common_args(command: &str) -> Vec<String> {
        vec![
            command.to_string(),
            "--run".to_string(),
            "run-1".to_string(),
            "--run-root".to_string(),
            "/tmp/run-1".to_string(),
            "--working-dir".to_string(),
            "/tmp/run-1/working".to_string(),
        ]
    }

    #[test]
    fn evaluate_cli_has_no_signing_inputs() {
        let parsed = parse_command(common_args("evaluate").into_iter()).expect("evaluate args");
        let GateCommand::Evaluate(args) = parsed else {
            panic!("expected evaluate");
        };
        assert_eq!(args.run_id, "run-1");
        assert_eq!(args.run_root, PathBuf::from("/tmp/run-1"));
        assert_eq!(args.gate_evaluator_sha256, None);
    }

    #[test]
    fn sign_cli_requires_explicit_coherent_containment() {
        let mut args = common_args("sign");
        args.extend([
            "--gate-evaluator-sha256".to_string(),
            format!("sha256:{}", "a".repeat(64)),
            "--evaluation".to_string(),
            "-".to_string(),
            "--contained".to_string(),
            "true".to_string(),
            "--sandbox-backend".to_string(),
            "sandbox-exec".to_string(),
        ]);
        let parsed = parse_command(args.into_iter()).expect("sign args");
        let GateCommand::Sign(args) = parsed else {
            panic!("expected sign");
        };
        assert!(args.containment.contained);
        assert_eq!(args.containment.sandbox_backend, "sandbox-exec");
        let expected_identity = format!("sha256:{}", "a".repeat(64));
        assert_eq!(
            args.common.gate_evaluator_sha256.as_deref(),
            Some(expected_identity.as_str())
        );

        let err = explicit_containment(true, "none").expect_err("incoherent containment");
        assert!(err.to_string().contains("inconsistent"), "{err}");
        let err = explicit_containment(true, "auto").expect_err("unobserved backend");
        assert!(err.to_string().contains("observed backend"), "{err}");
    }

    #[test]
    fn evaluator_refuses_every_gate_environment_variable() {
        for forbidden in [GATE_KEY_ENV, GATE_CONTAINED_ENV, GATE_SANDBOX_BACKEND_ENV] {
            let err = reject_evaluator_gate_environment(|name| name == forbidden)
                .expect_err("gate environment refused");
            assert!(err.contains(forbidden), "{err}");
        }
    }

    #[test]
    fn signer_reads_only_the_key_and_rejects_legacy_containment_env() {
        let environment = SigningEnvironment::from_lookup(
            |name| (name == GATE_KEY_ENV).then(|| "07".repeat(32)),
            |_| false,
        )
        .expect("key");
        assert_eq!(environment.key, vec![7; 32]);

        let err = SigningEnvironment::from_lookup(
            |name| (name == GATE_KEY_ENV).then(|| "07".repeat(32)),
            |name| name == GATE_CONTAINED_ENV,
        )
        .err()
        .expect("legacy env refused");
        assert!(err.contains(GATE_CONTAINED_ENV), "{err}");
    }

    #[test]
    fn signer_without_key_fails_loudly() {
        let err = SigningEnvironment::from_lookup(|_| None, |_| false)
            .err()
            .expect("missing key refused");

        assert!(err.contains(GATE_KEY_ENV), "{err}");
        assert!(err.contains("required"), "{err}");
    }

    #[test]
    fn signer_rejects_evaluator_identity_before_reading_the_gate_key() {
        let temp = TempDir::new().expect("tempdir");
        let evaluation_path = temp.path().join("evaluation.json");
        std::fs::write(
            &evaluation_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "run_id": "run-1",
                "working_dir": temp.path(),
                "contract_sha256": format!("sha256:{}", "c".repeat(64)),
                "gate_evaluator_sha256": format!("sha256:{}", "a".repeat(64)),
                "results": [],
                "tamper": {
                    "schema_version": 1,
                    "run_id": "run-1",
                    "evaluated_at": "2026-07-31T00:00:00Z",
                    "verdict": "clean",
                    "spec_modified": false,
                    "lint_findings": [],
                    "covered_files_touched": [],
                    "caveats": [],
                    "refusal_reasons": []
                }
            }))
            .expect("evaluation JSON"),
        )
        .expect("evaluation fixture");
        let approved_identity = format!("sha256:{}", "b".repeat(64));
        let error = sign(SignArgs {
            common: CommonArgs {
                run_id: "run-1".to_string(),
                run_root: temp.path().join("run"),
                working_dir: temp.path().to_path_buf(),
                gate_evaluator_sha256: Some(approved_identity),
            },
            evaluation: evaluation_path.to_string_lossy().into_owned(),
            containment: deadreckon_core::gate::AcceptanceContainment::contained("sandbox-exec"),
        })
        .expect_err("identity mismatch must fail before the signing environment is read");

        assert!(
            error
                .to_string()
                .contains("does not match approved identity"),
            "{error}"
        );
        assert!(
            !error.to_string().contains(GATE_KEY_ENV),
            "the mismatch must be decided before gate-key lookup: {error}"
        );
    }

    #[test]
    fn signer_can_read_an_explicit_evaluation_file() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("evaluation.json");
        std::fs::write(&path, "{\"schema_version\":1}\n").expect("evaluation fixture");

        let raw = read_evaluation(path.to_str().expect("utf-8 path")).expect("read evaluation");

        assert_eq!(raw, "{\"schema_version\":1}\n");
    }

    #[test]
    fn guarded_exec_preserves_non_utf8_command_arguments_and_requires_identity() {
        let parsed = parse_guarded_exec(
            [
                "--metadata",
                "/tmp/gate.json",
                "--launch-id",
                "launch-1",
                "--attempt",
                "2",
                "--release-token-sha256",
                "digest",
                "--",
                "/bin/sh",
                "-c",
                "printf ok",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .expect("guarded args");

        assert_eq!(parsed.metadata, PathBuf::from("/tmp/gate.json"));
        assert_eq!(parsed.launch_id, "launch-1");
        assert_eq!(parsed.attempt, 2);
        assert_eq!(parsed.program, OsString::from("/bin/sh"));
        assert_eq!(parsed.args.len(), 2);
    }
}
