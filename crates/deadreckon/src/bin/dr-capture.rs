//! Private trusted helper for operator-gated Watchkeeper captures.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use clap::{Args, Parser, Subcommand, ValueEnum};
use deadreckon_core::git::run_git;
use deadreckon_core::{
    DeadreckonPaths, JobView, OperatorCaptureEventDraft, RUN_EVENTS_JSONL,
    append_operator_capture_event, boot_identity, load_job, load_job_lease,
    load_operator_capture_binding, load_plan, load_run, operator_capture_binding_sha256,
    pid_is_alive, read_job_history, read_operator_capture_history, read_plan_events,
    reduce_job_history, seal_operator_capture_receipt, validate_completion_receipt,
    validate_operator_capture_receipt, write_operator_capture_binding,
};
use deadreckon_protocol::{
    CompletionReceipt, Job, JobAuthority, JobEvent, JobEventKind, JobLease, OperatorCaptureBinding,
    OperatorCaptureCompletionLineage, OperatorCaptureEventKind, OperatorCapturePhase,
    OperatorCaptureProvenance, OperatorCaptureRequirement, OperatorCaptureSchemaVersion,
    OperatorCaptureSource, OperatorCaptureStatus, SandboxBoundaryObservation, SemanticJudgment,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::{Builder as TempFileBuilder, tempdir};

type AnyResult<T> = Result<T, Box<dyn Error>>;

#[derive(Debug, Parser)]
#[command(name = "dr-capture", about = "Private trusted capture helper")]
struct Cli {
    #[command(subcommand)]
    command: CaptureCommand,
}

#[derive(Debug, Subcommand)]
enum CaptureCommand {
    Prepare(PrepareArgs),
    Observe(ObserveArgs),
    Attest(AttestArgs),
    Seal(SealArgs),
    Inspect(IdentityArgs),
    Verify(VerifyArgs),
}

#[derive(Debug, Clone, Args)]
struct IdentityArgs {
    #[arg(long)]
    job_id: String,
    #[arg(long)]
    session_id: String,
}

#[derive(Debug, Args)]
struct PrepareArgs {
    #[command(flatten)]
    identity: IdentityArgs,
    #[arg(long)]
    trial_id: String,
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    result_schema: PathBuf,
    #[arg(long)]
    recorder: PathBuf,
    #[arg(long)]
    recorder_interpreter: PathBuf,
    #[arg(long)]
    deadreckon_binary: PathBuf,
    #[arg(long)]
    replay: PathBuf,
    #[arg(long)]
    backend: String,
    #[arg(long = "provider-route")]
    provider_routes: Vec<String>,
    #[arg(long)]
    inconclusive_only: bool,
}

#[derive(Debug, Args)]
struct ObserveArgs {
    #[command(flatten)]
    identity: IdentityArgs,
    #[arg(long)]
    source: CanonicalSource,
    #[arg(long)]
    subject: String,
    #[arg(long)]
    event_id: String,
    #[arg(long)]
    causation_id: String,
    #[arg(long)]
    phase: CapturePhaseArg,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct AttestArgs {
    #[command(flatten)]
    identity: IdentityArgs,
    #[arg(long)]
    file: PathBuf,
    #[arg(long)]
    subject: String,
    #[arg(long)]
    event_id: String,
    #[arg(long)]
    causation_id: String,
    #[arg(long)]
    phase: CapturePhaseArg,
}

#[derive(Debug, Args)]
struct SealArgs {
    #[command(flatten)]
    identity: IdentityArgs,
    #[arg(long)]
    result: PathBuf,
    #[arg(long)]
    status: CaptureStatusArg,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    #[command(flatten)]
    identity: IdentityArgs,
    #[arg(long)]
    result: PathBuf,
    #[arg(long)]
    envelope: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum CapturePhaseArg {
    Prepared,
    Before,
    Intervention,
    After,
    Cleanup,
    Finalized,
}

impl From<CapturePhaseArg> for OperatorCapturePhase {
    fn from(value: CapturePhaseArg) -> Self {
        match value {
            CapturePhaseArg::Prepared => Self::Prepared,
            CapturePhaseArg::Before => Self::Before,
            CapturePhaseArg::Intervention => Self::Intervention,
            CapturePhaseArg::After => Self::After,
            CapturePhaseArg::Cleanup => Self::Cleanup,
            CapturePhaseArg::Finalized => Self::Finalized,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum CaptureStatusArg {
    Passed,
    Failed,
    Inconclusive,
    NotRun,
}

impl From<CaptureStatusArg> for OperatorCaptureStatus {
    fn from(value: CaptureStatusArg) -> Self {
        match value {
            CaptureStatusArg::Passed => Self::Passed,
            CaptureStatusArg::Failed => Self::Failed,
            CaptureStatusArg::Inconclusive => Self::Inconclusive,
            CaptureStatusArg::NotRun => Self::NotRun,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum CanonicalSource {
    JobView,
    JobEvents,
    JobIntervention,
    JobCleanup,
    Job,
    Authority,
    LaunchPlan,
    Lease,
    JobReport,
    Receipt,
    SupervisedChild,
    HostBootId,
    SemanticJudgment,
    ParentRepairManifest,
    ParentRepairCandidate,
    Doctor,
    SupervisorServiceStatus,
    ParentArtifact,
    ParentEvents,
    Campaign,
    CampaignEvents,
    ActivePlan,
    ActivePlanEvents,
    SandboxBoundaryObservation,
    CampaignIntervention,
}

impl CanonicalSource {
    fn protocol(self) -> OperatorCaptureSource {
        match self {
            Self::JobView => OperatorCaptureSource::JobView,
            Self::JobEvents => OperatorCaptureSource::JobEvents,
            Self::JobIntervention => OperatorCaptureSource::JobIntervention,
            Self::JobCleanup => OperatorCaptureSource::JobCleanup,
            Self::Job => OperatorCaptureSource::Job,
            Self::Authority => OperatorCaptureSource::Authority,
            Self::LaunchPlan => OperatorCaptureSource::LaunchPlan,
            Self::Lease => OperatorCaptureSource::Lease,
            Self::JobReport => OperatorCaptureSource::JobReport,
            Self::Receipt => OperatorCaptureSource::Receipt,
            Self::SupervisedChild => OperatorCaptureSource::SupervisedChild,
            Self::HostBootId => OperatorCaptureSource::HostBootId,
            Self::SemanticJudgment => OperatorCaptureSource::SemanticJudgment,
            Self::ParentRepairManifest => OperatorCaptureSource::ParentRepairManifest,
            Self::ParentRepairCandidate => OperatorCaptureSource::ParentRepairCandidate,
            Self::Doctor => OperatorCaptureSource::Doctor,
            Self::SupervisorServiceStatus => OperatorCaptureSource::SupervisorServiceStatus,
            Self::ParentArtifact => OperatorCaptureSource::ParentArtifact,
            Self::ParentEvents => OperatorCaptureSource::ParentEvents,
            Self::Campaign => OperatorCaptureSource::Campaign,
            Self::CampaignEvents => OperatorCaptureSource::CampaignEvents,
            Self::ActivePlan => OperatorCaptureSource::ActivePlan,
            Self::ActivePlanEvents => OperatorCaptureSource::ActivePlanEvents,
            Self::SandboxBoundaryObservation => OperatorCaptureSource::SandboxBoundaryObservation,
            Self::CampaignIntervention => OperatorCaptureSource::CampaignIntervention,
        }
    }

    fn provenance(self) -> OperatorCaptureProvenance {
        match self {
            Self::JobView | Self::JobReport | Self::Receipt | Self::Doctor => {
                OperatorCaptureProvenance::PublicDeadreckon
            }
            Self::HostBootId | Self::SupervisorServiceStatus => {
                OperatorCaptureProvenance::AuthoritativeHost
            }
            _ => OperatorCaptureProvenance::TrustedSupervisor,
        }
    }

    fn media_type(self) -> &'static str {
        match self {
            Self::JobEvents
            | Self::ParentEvents
            | Self::CampaignEvents
            | Self::ActivePlanEvents => "application/x-ndjson",
            Self::HostBootId => "text/plain; charset=utf-8",
            _ => "application/json",
        }
    }
}

#[derive(Debug, Clone)]
struct HelperContext {
    paths: DeadreckonPaths,
    current_exe: PathBuf,
    deadreckon_source_root: PathBuf,
}

impl HelperContext {
    fn discover() -> AnyResult<Self> {
        let current_exe = std::env::current_exe()?;
        let deadreckon_source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .ok_or_else(|| refused("compiled DeadReckon source root is unavailable"))?
            .to_path_buf();
        Ok(Self {
            paths: DeadreckonPaths::discover(),
            current_exe,
            deadreckon_source_root,
        })
    }
}

#[derive(Debug)]
struct CanonicalObservation {
    bytes: Vec<u8>,
    source: OperatorCaptureSource,
    provenance: OperatorCaptureProvenance,
    media_type: &'static str,
}

#[derive(Debug, Serialize)]
struct VerifyVerdict {
    schema_version: u32,
    job_id: String,
    session_id: String,
    trial_id: String,
    verified: bool,
    status: OperatorCaptureStatus,
    event_count: u64,
    receipt_sha256: String,
    publication_proof: String,
    binding_sha256: String,
    binding_coverage: BindingCoverage,
    capture_coverage: CaptureCoverage,
    subject_coverage: Vec<SubjectCoverage>,
}

#[derive(Debug, Serialize)]
struct InspectVerdict {
    schema_version: u32,
    job_id: String,
    session_id: String,
    trial_id: String,
    verified: bool,
    event_count: u64,
    binding_sha256: String,
    capture_coverage: CaptureCoverage,
    subject_coverage: Vec<SubjectCoverage>,
}

#[derive(Debug, Serialize)]
struct CaptureCoverage {
    required_total: usize,
    required_covered: usize,
    intervention_covered: bool,
    cleanup_covered: bool,
    pass_ready: bool,
    missing: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BindingCoverage {
    job_source: bool,
    deadreckon_source: bool,
    manifest: bool,
    result_schema: bool,
    recorder: bool,
    capture_binary: bool,
    deadreckon_binary: bool,
    execution_declaration: bool,
    replay: bool,
}

#[derive(Debug, Serialize)]
struct SubjectCoverage {
    subject: String,
    phase: OperatorCapturePhase,
    kind: OperatorCaptureEventKind,
    source: OperatorCaptureSource,
    provenance: OperatorCaptureProvenance,
    content_sha256: String,
    content_bytes: u64,
}

fn main() -> AnyResult<()> {
    let cli = Cli::parse();
    let context = HelperContext::discover()?;
    let value = execute(&context, cli.command)?;
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, &value)?;
    stdout.write_all(b"\n")?;
    Ok(())
}

fn execute(context: &HelperContext, command: CaptureCommand) -> AnyResult<Value> {
    match command {
        CaptureCommand::Prepare(args) => {
            Ok(serde_json::to_value(prepare_binding(context, &args)?)?)
        }
        CaptureCommand::Observe(args) => Ok(serde_json::to_value(observe(context, &args)?)?),
        CaptureCommand::Attest(args) => Ok(serde_json::to_value(attest(context, &args)?)?),
        CaptureCommand::Seal(args) => Ok(serde_json::to_value(seal(context, &args)?)?),
        CaptureCommand::Inspect(args) => Ok(serde_json::to_value(inspect(context, &args)?)?),
        CaptureCommand::Verify(args) => Ok(serde_json::to_value(verify(context, &args)?)?),
    }
}

fn prepare_binding(
    context: &HelperContext,
    args: &PrepareArgs,
) -> AnyResult<OperatorCaptureBinding> {
    let job = load_job(&context.paths, &args.identity.job_id)?;
    let authority = load_authority(&context.paths, &args.identity.job_id)?;
    if authority.job_id != job.job_id {
        return Err(refused("Job authority identity does not match job.json").into());
    }
    let pass_capable = !args.inconclusive_only;
    if pass_capable
        && (!matches!(args.backend.as_str(), "sandbox-exec" | "bwrap" | "docker")
            || args.provider_routes.is_empty())
    {
        return Err(refused(
            "pass-capable prepare requires sandbox-exec, bwrap, or docker and at least one provider route",
        )
        .into());
    }
    let manifest = stable_regular_bytes(&args.manifest, "manifest")?;
    let provider_routes = provider_route_map(
        &manifest,
        &args.trial_id,
        &args.provider_routes,
        pass_capable,
    )?;
    let result_schema = stable_regular_bytes(&args.result_schema, "result schema")?;
    let recorder = stable_regular_bytes(&args.recorder, "recorder")?;
    let recorder_interpreter =
        stable_regular_bytes(&args.recorder_interpreter, "recorder interpreter")?;
    let recorder_interpreter_path = fs::canonicalize(&args.recorder_interpreter)?;
    let replay = stable_regular_bytes(&args.replay, "replay")?;
    let current_exe = stable_regular_bytes(&context.current_exe, "dr-capture executable")?;
    let current_exe_path = fs::canonicalize(&context.current_exe)?;
    let deadreckon_binary = validate_deadreckon_binary(
        &context.current_exe,
        &args.deadreckon_binary,
        env!("CARGO_PKG_VERSION"),
    )?;
    let deadreckon_binary_path = fs::canonicalize(&args.deadreckon_binary)?;
    let deadreckon_source_revision = clean_git_revision(&context.deadreckon_source_root)?;
    if pass_capable {
        validate_pass_capture_locations(context, &job, args)?;
    }
    let required_captures = manifest_requirements(&manifest, &args.trial_id)?;
    if pass_capable && manifest_trial_is_structurally_inconclusive(&manifest, &args.trial_id)? {
        return Err(refused(
            "a trial with a structurally_inconclusive oracle cannot be prepared as pass-capable",
        )
        .into());
    }
    if pass_capable && required_captures.is_empty() {
        return Err(refused("pass-capable trial has no manifest-required captures").into());
    }
    if pass_capable
        && required_captures
            .iter()
            .any(|requirement| requirement.source == OperatorCaptureSource::UnavailableObjective)
    {
        return Err(refused(
            "pass-capable trial has required evidence without a canonical objective producer; use --inconclusive-only",
        )
        .into());
    }
    let source_revision = authority.source_revision.clone().ok_or_else(|| {
        refused("capture requires a concrete Job source revision in authority.json")
    })?;
    let mut unsigned = OperatorCaptureBinding {
        schema_version: OperatorCaptureSchemaVersion::CURRENT,
        job_id: job.job_id,
        session_id: required_text(&args.identity.session_id, "session ID")?,
        trial_id: required_text(&args.trial_id, "trial ID")?,
        created_at: Utc::now(),
        source_revision,
        source_tree_sha256: authority.source_tree_sha256,
        deadreckon_source_revision,
        manifest_sha256: sha256_bytes(&manifest),
        result_schema_sha256: sha256_bytes(&result_schema),
        recorder_sha256: sha256_bytes(&recorder),
        recorder_interpreter: recorder_interpreter_path.to_string_lossy().into_owned(),
        recorder_interpreter_sha256: sha256_bytes(&recorder_interpreter),
        capture_binary: current_exe_path.to_string_lossy().into_owned(),
        capture_binary_sha256: sha256_bytes(&current_exe),
        deadreckon_binary: deadreckon_binary_path.to_string_lossy().into_owned(),
        deadreckon_binary_sha256: sha256_bytes(&deadreckon_binary),
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
        declared_shape: job.shape,
        declared_backend: required_text(&args.backend, "backend")?,
        provider_routes,
        replay_sha256: sha256_bytes(&replay),
        pass_capable,
        required_captures,
        signature: String::new(),
    };
    let binding_path = context
        .paths
        .operator_capture_binding(unsigned.job_id.as_ref(), &unsigned.session_id);
    if binding_path.exists() {
        let existing = load_operator_capture_binding(
            &context.paths,
            unsigned.job_id.as_ref(),
            &unsigned.session_id,
        )?;
        unsigned.created_at = existing.created_at;
        unsigned.signature = existing.signature.clone();
        if unsigned != existing {
            return Err(refused(
                "an existing capture binding does not match the requested prepare inputs",
            )
            .into());
        }
    }
    let binding = write_operator_capture_binding(&context.paths, &unsigned)?;
    persist_bound_bytes(
        &protected_input_path(&context.paths, &binding, "manifest.json"),
        &manifest,
        "bound manifest",
    )?;
    persist_bound_bytes(
        &protected_input_path(&context.paths, &binding, "result-schema.json"),
        &result_schema,
        "bound result schema",
    )?;
    persist_bound_bytes(
        &protected_input_path(&context.paths, &binding, "recorder.bin"),
        &recorder,
        "bound recorder",
    )?;
    persist_bound_bytes(
        &protected_input_path(&context.paths, &binding, "replay.json"),
        &replay,
        "bound replay",
    )?;
    let binding_bytes = serde_json::to_vec(&binding)?;
    append_operator_capture_event(
        &context.paths,
        &binding,
        &OperatorCaptureEventDraft {
            event_id: format!(
                "prepare:{}",
                operator_capture_binding_sha256(&binding)?.trim_start_matches("sha256:")
            ),
            causation_id: "prepare".to_string(),
            timestamp: binding.created_at,
            phase: OperatorCapturePhase::Prepared,
            kind: OperatorCaptureEventKind::SessionPrepared,
            provenance: OperatorCaptureProvenance::TrustedSupervisor,
            source: OperatorCaptureSource::Binding,
            subject: "binding".to_string(),
            content_sha256: sha256_bytes(&binding_bytes),
            content_bytes: u64::try_from(binding_bytes.len())
                .map_err(|_| refused("binding is too large"))?,
        },
    )?;
    Ok(binding)
}

fn validate_pass_capture_locations(
    context: &HelperContext,
    job: &Job,
    args: &PrepareArgs,
) -> AnyResult<()> {
    if job
        .policy
        .execution
        .as_ref()
        .is_none_or(|execution| !execution.require_containment)
    {
        return Err(
            refused("pass-capable capture requires a contained Job execution policy").into(),
        );
    }
    let mut forbidden = vec![
        fs::canonicalize(&job.source_cwd)?,
        fs::canonicalize(context.paths.job_dir(job.job_id.as_ref()))?,
    ];
    if let Ok(plan_dir) = fs::canonicalize(context.paths.plan_dir(job.job_id.as_ref())) {
        forbidden.push(plan_dir);
    }
    if let Ok(view) = JobView::load(&context.paths, job.job_id.as_ref()) {
        for attempt in view.attempts {
            if let Ok(run_root) = fs::canonicalize(
                context
                    .paths
                    .run_root(&attempt.id.scope, &attempt.id.run_id),
            ) {
                forbidden.push(run_root);
            }
        }
    }
    let replay_parent = args
        .replay
        .parent()
        .ok_or_else(|| refused("trial replay has no parent directory"))?;
    let trusted_paths = [
        context.current_exe.as_path(),
        args.deadreckon_binary.as_path(),
        args.recorder.as_path(),
        args.recorder_interpreter.as_path(),
        args.manifest.as_path(),
        args.result_schema.as_path(),
        replay_parent,
    ];
    for path in trusted_paths {
        let canonical = fs::canonicalize(path)?;
        if forbidden
            .iter()
            .any(|root| canonical == *root || canonical.starts_with(root))
        {
            return Err(refused(
                "pass-capable capture authority and trial files must be outside every Job source and run workspace",
            )
            .into());
        }
    }
    Ok(())
}

fn observe(
    context: &HelperContext,
    args: &ObserveArgs,
) -> AnyResult<deadreckon_protocol::OperatorCaptureEvent> {
    let binding = load_operator_capture_binding(
        &context.paths,
        &args.identity.job_id,
        &args.identity.session_id,
    )?;
    validate_runtime_binding(context, &binding)?;
    let phase = OperatorCapturePhase::from(args.phase);
    let subject = required_text(&args.subject, "subject")?;
    let event_id = required_text(&args.event_id, "event ID")?;
    let existing_history = read_operator_capture_history(
        &context.paths,
        binding.job_id.as_ref(),
        &binding.session_id,
    )?;
    if let Some(existing) = existing_history
        .events()
        .iter()
        .find(|event| event.event_id == event_id)
    {
        if existing.subject != subject
            || existing.phase != phase
            || existing.source != args.source.protocol()
        {
            return Err(refused("capture event ID already belongs to another observation").into());
        }
        let bytes = stable_regular_bytes(
            &protected_evidence_path(&context.paths, &binding, &subject),
            "protected canonical evidence",
        )?;
        if sha256_bytes(&bytes) != existing.content_sha256
            || u64::try_from(bytes.len()).ok() != Some(existing.content_bytes)
        {
            return Err(refused("existing capture event bytes no longer match").into());
        }
        persist_observation_no_clobber(&args.output, &bytes)?;
        return Ok(existing.clone());
    }
    let protected_path = protected_evidence_path(&context.paths, &binding, &subject);
    let observed = if protected_path.exists() {
        CanonicalObservation {
            bytes: stable_regular_bytes(&protected_path, "protected canonical evidence")?,
            source: args.source.protocol(),
            provenance: args.source.provenance(),
            media_type: args.source.media_type(),
        }
    } else {
        canonical_observation(context, &binding, args.source, phase)?
    };
    if let Some(requirement) = binding
        .required_captures
        .iter()
        .find(|requirement| requirement.subject == subject)
        && (requirement.phase != phase
            || requirement.source != observed.source
            || requirement.media_type != observed.media_type)
    {
        return Err(refused(
            "canonical source, phase, or media type does not match the manifest-bound subject requirement",
        )
        .into());
    }
    persist_bound_bytes(
        &protected_evidence_path(&context.paths, &binding, &subject),
        &observed.bytes,
        "protected canonical evidence",
    )?;
    persist_observation_no_clobber(&args.output, &observed.bytes)?;
    let draft = OperatorCaptureEventDraft {
        event_id,
        causation_id: required_text(&args.causation_id, "causation ID")?,
        timestamp: Utc::now(),
        phase,
        kind: objective_kind(observed.source),
        provenance: observed.provenance,
        source: observed.source,
        subject,
        content_sha256: sha256_bytes(&observed.bytes),
        content_bytes: u64::try_from(observed.bytes.len())
            .map_err(|_| refused("observed content is too large"))?,
    };
    Ok(append_operator_capture_event(
        &context.paths,
        &binding,
        &draft,
    )?)
}

fn attest(
    context: &HelperContext,
    args: &AttestArgs,
) -> AnyResult<deadreckon_protocol::OperatorCaptureEvent> {
    let binding = load_operator_capture_binding(
        &context.paths,
        &args.identity.job_id,
        &args.identity.session_id,
    )?;
    validate_runtime_binding(context, &binding)?;
    let bytes = stable_regular_bytes(&args.file, "manual attestation")?;
    let draft = OperatorCaptureEventDraft {
        event_id: required_text(&args.event_id, "event ID")?,
        causation_id: required_text(&args.causation_id, "causation ID")?,
        timestamp: Utc::now(),
        phase: OperatorCapturePhase::from(args.phase),
        kind: OperatorCaptureEventKind::OperatorAttestation,
        provenance: OperatorCaptureProvenance::OperatorAttested,
        source: OperatorCaptureSource::ManualFile,
        subject: required_text(&args.subject, "subject")?,
        content_sha256: sha256_bytes(&bytes),
        content_bytes: u64::try_from(bytes.len())
            .map_err(|_| refused("manual attestation is too large"))?,
    };
    Ok(append_operator_capture_event(
        &context.paths,
        &binding,
        &draft,
    )?)
}

fn seal(
    context: &HelperContext,
    args: &SealArgs,
) -> AnyResult<deadreckon_protocol::OperatorCaptureReceipt> {
    let binding = load_operator_capture_binding(
        &context.paths,
        &args.identity.job_id,
        &args.identity.session_id,
    )?;
    validate_runtime_binding(context, &binding)?;
    validate_bound_inputs(context, &binding)?;
    let result = stable_regular_bytes(&args.result, "capture result")?;
    let history = read_operator_capture_history(
        &context.paths,
        binding.job_id.as_ref(),
        &binding.session_id,
    )?;
    validate_protected_history(&context.paths, &binding, &history)?;
    validate_result_evaluation(
        context,
        &binding,
        &history,
        &result,
        OperatorCaptureStatus::from(args.status),
    )?;
    persist_bound_bytes(
        &protected_input_path(&context.paths, &binding, "sealed-evaluation.json"),
        &result,
        "protected sealed evaluation",
    )?;
    let completion_lineage =
        if OperatorCaptureStatus::from(args.status) == OperatorCaptureStatus::Passed {
            Some(validated_completion_lineage(context, &binding)?)
        } else {
            None
        };
    Ok(seal_operator_capture_receipt(
        &context.paths,
        &binding,
        Utc::now(),
        &sha256_bytes(&result),
        u64::try_from(result.len()).map_err(|_| refused("capture result is too large"))?,
        OperatorCaptureStatus::from(args.status),
        completion_lineage,
    )?)
}

fn inspect(context: &HelperContext, args: &IdentityArgs) -> AnyResult<InspectVerdict> {
    let binding = load_operator_capture_binding(&context.paths, &args.job_id, &args.session_id)?;
    validate_runtime_binding(context, &binding)?;
    validate_bound_inputs(context, &binding)?;
    let history = read_operator_capture_history(&context.paths, &args.job_id, &args.session_id)?;
    validate_protected_history(&context.paths, &binding, &history)?;
    let subject_coverage = subject_coverage(&history);
    let capture_coverage = capture_coverage(&binding, &history);
    Ok(InspectVerdict {
        schema_version: 1,
        job_id: binding.job_id.to_string(),
        session_id: binding.session_id.clone(),
        trial_id: binding.trial_id.clone(),
        verified: true,
        event_count: u64::try_from(history.events().len())
            .map_err(|_| refused("capture event count overflowed"))?,
        binding_sha256: operator_capture_binding_sha256(&binding)?,
        capture_coverage,
        subject_coverage,
    })
}

fn verify(context: &HelperContext, args: &VerifyArgs) -> AnyResult<VerifyVerdict> {
    let binding = load_operator_capture_binding(
        &context.paths,
        &args.identity.job_id,
        &args.identity.session_id,
    )?;
    validate_runtime_binding(context, &binding)?;
    validate_bound_inputs(context, &binding)?;
    let receipt = validate_operator_capture_receipt(&context.paths, &binding)?;
    let expected_lineage = if receipt.status == OperatorCaptureStatus::Passed {
        Some(validated_completion_lineage(context, &binding)?)
    } else {
        None
    };
    if receipt.completion_lineage != expected_lineage {
        return Err(
            refused("validated CompletionReceipt lineage changed after capture sealing").into(),
        );
    }
    let result = stable_regular_bytes(&args.result, "published capture result")?;
    let protected_result = stable_regular_bytes(
        &protected_input_path(&context.paths, &binding, "sealed-evaluation.json"),
        "protected sealed evaluation",
    )?;
    if result != protected_result || u64::try_from(result.len()).ok() != Some(receipt.result_bytes)
    {
        return Err(refused(
            "published result bytes do not match protected sealed evaluation bytes",
        )
        .into());
    }
    if sha256_bytes(&result) != receipt.result_sha256 {
        return Err(refused(
            "published result bytes do not match the digest authenticated by the receipt",
        )
        .into());
    }
    let history = read_operator_capture_history(
        &context.paths,
        &args.identity.job_id,
        &args.identity.session_id,
    )?;
    validate_protected_history(&context.paths, &binding, &history)?;
    validate_result_evaluation(context, &binding, &history, &result, receipt.status)?;
    let receipt_path = context
        .paths
        .operator_capture_receipt(&args.identity.job_id, &args.identity.session_id);
    let receipt_raw = stable_regular_bytes(&receipt_path, "capture receipt")?;
    let receipt_sha256 = sha256_bytes(&receipt_raw);
    if let Some(envelope_path) = &args.envelope {
        validate_published_envelope(envelope_path, &result, &receipt, &receipt_sha256)?;
    }
    let subject_coverage = subject_coverage(&history);
    let capture_coverage = capture_coverage(&binding, &history);
    Ok(VerifyVerdict {
        schema_version: 1,
        job_id: binding.job_id.to_string(),
        session_id: binding.session_id.clone(),
        trial_id: binding.trial_id.clone(),
        verified: true,
        status: receipt.status,
        event_count: receipt.event_count,
        receipt_sha256,
        publication_proof: receipt.signature,
        binding_sha256: operator_capture_binding_sha256(&binding)?,
        binding_coverage: BindingCoverage {
            job_source: true,
            deadreckon_source: true,
            manifest: true,
            result_schema: true,
            recorder: true,
            capture_binary: true,
            deadreckon_binary: true,
            execution_declaration: true,
            replay: true,
        },
        capture_coverage,
        subject_coverage,
    })
}

fn validate_published_envelope(
    path: &Path,
    result: &[u8],
    receipt: &deadreckon_protocol::OperatorCaptureReceipt,
    receipt_sha256: &str,
) -> AnyResult<()> {
    let bytes = stable_regular_bytes(path, "published result envelope")?;
    let envelope: PublishedEnvelope = serde_json::from_slice(&bytes)?;
    let evaluation: Value = serde_json::from_slice(result)?;
    let mut embedded_evaluation = serde_json::to_vec_pretty(&envelope.evaluation)?;
    embedded_evaluation.push(b'\n');
    if envelope.schema_version != 2
        || !envelope.sanitized
        || envelope.evaluation != evaluation
        || embedded_evaluation != result
        || envelope.evaluation_sha256 != receipt.result_sha256
        || envelope.capture_provenance.status != "verified"
        || envelope.capture_provenance.receipt_sha256 != receipt_sha256
        || envelope.capture_provenance.publication_proof != receipt.signature
        || evaluation.get("status").and_then(Value::as_str)
            != Some(capture_status_name(receipt.status))
    {
        return Err(refused(
            "published envelope is not bound to the authenticated evaluation and receipt",
        )
        .into());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedEnvelope {
    schema_version: u64,
    sanitized: bool,
    evaluation: Value,
    evaluation_sha256: String,
    capture_provenance: PublishedCaptureProvenance,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedCaptureProvenance {
    status: String,
    receipt_sha256: String,
    publication_proof: String,
}

fn subject_coverage(history: &deadreckon_core::OperatorCaptureHistory) -> Vec<SubjectCoverage> {
    history
        .events()
        .iter()
        .map(|event| SubjectCoverage {
            subject: event.subject.clone(),
            phase: event.phase,
            kind: event.kind,
            source: event.source,
            provenance: event.provenance,
            content_sha256: event.content_sha256.clone(),
            content_bytes: event.content_bytes,
        })
        .collect()
}

fn capture_coverage(
    binding: &OperatorCaptureBinding,
    history: &deadreckon_core::OperatorCaptureHistory,
) -> CaptureCoverage {
    let mut required_covered = 0;
    let mut missing = Vec::new();
    for requirement in &binding.required_captures {
        let covered = history.events().iter().any(|event| {
            event.subject == requirement.subject
                && event.phase == requirement.phase
                && event.source == requirement.source
                && event.provenance != OperatorCaptureProvenance::OperatorAttested
                && event.source != OperatorCaptureSource::ManualFile
        });
        if covered {
            required_covered += 1;
        } else {
            missing.push(format!(
                "{}:{:?}:{:?}",
                requirement.subject, requirement.phase, requirement.source
            ));
        }
    }
    let expected_intervention = expected_operator_intervention_source(&binding.trial_id);
    let intervention_covered = history.events().iter().any(|event| {
        event.kind == OperatorCaptureEventKind::InterventionRecorded
            && Some(event.source) == expected_intervention
            && event.provenance == OperatorCaptureProvenance::TrustedSupervisor
    });
    let cleanup_covered = history.events().iter().any(|event| {
        event.kind == OperatorCaptureEventKind::CleanupRecorded
            && event.source == OperatorCaptureSource::JobCleanup
            && event.provenance == OperatorCaptureProvenance::TrustedSupervisor
    });
    if !intervention_covered {
        missing.push("authoritative-intervention".to_string());
    }
    if !cleanup_covered {
        missing.push("authoritative-cleanup".to_string());
    }
    CaptureCoverage {
        required_total: binding.required_captures.len(),
        required_covered,
        intervention_covered,
        cleanup_covered,
        pass_ready: binding.pass_capable
            && required_covered == binding.required_captures.len()
            && intervention_covered
            && cleanup_covered,
        missing,
    }
}

fn expected_operator_intervention_source(trial_id: &str) -> Option<OperatorCaptureSource> {
    match trial_id {
        "live_provider_worker_kill"
        | "live_provider_supervisor_restart"
        | "live_provider_network_loss"
        | "machine_reboot"
        | "live_provider_parent_repair" => Some(OperatorCaptureSource::JobIntervention),
        "live_campaign_interruption_recovery" => Some(OperatorCaptureSource::CampaignIntervention),
        "cross_provider_gate_attack"
        | "linux_bubblewrap_gate_boundary"
        | "docker_gate_boundary" => Some(OperatorCaptureSource::SandboxBoundaryObservation),
        _ => None,
    }
}

fn validate_protected_history(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
    history: &deadreckon_core::OperatorCaptureHistory,
) -> AnyResult<()> {
    for event in history.events().iter().filter(|event| {
        !matches!(
            event.kind,
            OperatorCaptureEventKind::SessionPrepared
                | OperatorCaptureEventKind::OperatorAttestation
                | OperatorCaptureEventKind::ResultFinalized
        )
    }) {
        let bytes = stable_regular_bytes(
            &protected_evidence_path(paths, binding, &event.subject),
            "protected canonical evidence",
        )?;
        if sha256_bytes(&bytes) != event.content_sha256
            || u64::try_from(bytes.len()).ok() != Some(event.content_bytes)
        {
            return Err(refused(
                "protected canonical evidence does not match its authenticated capture event",
            )
            .into());
        }
    }
    Ok(())
}

fn validate_result_evaluation(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
    history: &deadreckon_core::OperatorCaptureHistory,
    result: &[u8],
    status: OperatorCaptureStatus,
) -> AnyResult<()> {
    let paths = &context.paths;
    let manifest = stable_regular_bytes(
        &protected_input_path(paths, binding, "manifest.json"),
        "bound manifest",
    )?;
    let result_schema = stable_regular_bytes(
        &protected_input_path(paths, binding, "result-schema.json"),
        "bound result schema",
    )?;
    if sha256_bytes(&manifest) != binding.manifest_sha256
        || sha256_bytes(&result_schema) != binding.result_schema_sha256
    {
        return Err(
            refused("protected manifest or result schema does not match the binding").into(),
        );
    }
    let _: Value = serde_json::from_slice(&manifest)?;
    let result_schema: Value = serde_json::from_slice(&result_schema)?;
    let value: Value = serde_json::from_slice(result)?;
    let object = value
        .as_object()
        .ok_or_else(|| refused("capture result evaluation must be one JSON object"))?;
    validate_root_schema_contract(&value, &result_schema)?;
    if object.get("trial_id").and_then(Value::as_str) != Some(binding.trial_id.as_str())
        || object.get("source_revision").and_then(Value::as_str)
            != Some(binding.source_revision.as_str())
        || object.get("status").and_then(Value::as_str) != Some(capture_status_name(status))
    {
        return Err(refused(
            "capture result evaluation does not match the bound trial, source, or seal status",
        )
        .into());
    }
    validate_trusted_recorder_evaluation(context, binding, history, result)?;
    if let Some(receipt_digest) = value
        .pointer("/capture_provenance/receipt_sha256")
        .filter(|value| !value.is_null())
    {
        return Err(refused(&format!(
            "pre-seal evaluation must not claim a receipt digest ({receipt_digest})"
        ))
        .into());
    }
    if status != OperatorCaptureStatus::Passed {
        return Ok(());
    }
    if manifest_trial_is_structurally_inconclusive(&manifest, &binding.trial_id)? {
        return Err(
            refused("a structurally inconclusive trial cannot seal a passed evaluation").into(),
        );
    }
    validate_final_intervention_freshness(context, binding, history)?;
    if object.get("sanitized").and_then(Value::as_bool) != Some(true) {
        return Err(refused("passed evaluation must be explicitly sanitized").into());
    }
    let coverage = capture_coverage(binding, history);
    if !coverage.pass_ready {
        return Err(refused("passed evaluation lacks authenticated capture coverage").into());
    }
    let completion_bytes = canonical_completion_receipt(context, binding)?;
    let completion: CompletionReceipt = serde_json::from_slice(&completion_bytes)?;
    let backend = object
        .get("backend")
        .and_then(Value::as_str)
        .ok_or_else(|| refused("passed evaluation has no backend"))?;
    if !completion.contained
        || backend != binding.declared_backend
        || backend != completion.sandbox_backend
    {
        return Err(refused(
            "passed evaluation backend is not bound to the validated contained completion receipt",
        )
        .into());
    }
    let actual_routes = validated_actual_provider_routes(context, binding)?;
    if actual_routes != binding.provider_routes {
        return Err(refused(
            "role-bound provider routes do not match persisted attempt, planner, and semantic evidence",
        )
        .into());
    }
    let evidence = object
        .get("evidence")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("passed evaluation has no evidence array"))?;
    let mut seen = BTreeSet::new();
    for record in evidence {
        let record = record
            .as_object()
            .ok_or_else(|| refused("evaluation evidence record must be an object"))?;
        let subject = record
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("evaluation evidence record has no name"))?;
        if !seen.insert(subject) {
            return Err(refused("evaluation evidence repeats a subject").into());
        }
        let requirement = binding
            .required_captures
            .iter()
            .find(|requirement| requirement.subject == subject)
            .ok_or_else(|| refused("evaluation evidence is not manifest-required"))?;
        let event = history
            .events()
            .iter()
            .find(|event| {
                event.subject == requirement.subject
                    && event.phase == requirement.phase
                    && event.source == requirement.source
                    && event.provenance != OperatorCaptureProvenance::OperatorAttested
            })
            .ok_or_else(|| refused("evaluation evidence has no authenticated capture event"))?;
        if record.get("declared_source").and_then(Value::as_str)
            != Some(capture_source_name(requirement.source))
            || record.get("sha256").and_then(Value::as_str) != Some(event.content_sha256.as_str())
            || record.get("bytes").and_then(Value::as_u64) != Some(event.content_bytes)
            || record.get("media_type").and_then(Value::as_str)
                != Some(requirement.media_type.as_str())
        {
            return Err(refused(
                "evaluation evidence metadata does not match authenticated exact bytes",
            )
            .into());
        }
    }
    if seen.len() != binding.required_captures.len() {
        return Err(refused("passed evaluation omits manifest-required evidence").into());
    }
    let intervention = history
        .events()
        .iter()
        .find(|event| event.kind == OperatorCaptureEventKind::InterventionRecorded)
        .ok_or_else(|| refused("passed evaluation has no intervention event"))?;
    let cleanup = history
        .events()
        .iter()
        .find(|event| event.kind == OperatorCaptureEventKind::CleanupRecorded)
        .ok_or_else(|| refused("passed evaluation has no cleanup event"))?;
    if value
        .pointer("/intervention/status")
        .and_then(Value::as_str)
        != Some("performed")
        || value
            .pointer("/intervention/detail_sha256")
            .and_then(Value::as_str)
            != Some(intervention.content_sha256.as_str())
        || value.pointer("/cleanup/status").and_then(Value::as_str) != Some("completed")
        || value
            .pointer("/cleanup/detail_sha256")
            .and_then(Value::as_str)
            != Some(cleanup.content_sha256.as_str())
    {
        return Err(refused(
            "passed evaluation lifecycle details do not match authenticated intervention and cleanup bytes",
        )
        .into());
    }
    let assertions = object
        .get("oracle_assertions")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("passed evaluation has no oracle assertions"))?;
    if assertions.is_empty()
        || assertions
            .iter()
            .any(|assertion| assertion.get("status").and_then(Value::as_str) != Some("passed"))
    {
        return Err(refused("passed evaluation contains a non-passing oracle assertion").into());
    }
    Ok(())
}

fn validate_trusted_recorder_evaluation(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
    history: &deadreckon_core::OperatorCaptureHistory,
    candidate: &[u8],
) -> AnyResult<()> {
    let interpreter_path = Path::new(&binding.recorder_interpreter);
    let interpreter = stable_regular_bytes(interpreter_path, "bound recorder interpreter")?;
    if sha256_bytes(&interpreter) != binding.recorder_interpreter_sha256 {
        return Err(refused("recorder interpreter does not match the signed binding").into());
    }
    let recorder = stable_regular_bytes(
        &protected_input_path(&context.paths, binding, "recorder.bin"),
        "bound recorder",
    )?;
    if sha256_bytes(&recorder) != binding.recorder_sha256 {
        return Err(refused("recorder bytes do not match the signed binding").into());
    }
    let manifest = stable_regular_bytes(
        &protected_input_path(&context.paths, binding, "manifest.json"),
        "bound manifest",
    )?;
    let work = tempdir()?;
    let trial_dir = work.path().join("trial");
    let raw_dir = trial_dir.join("raw");
    fs::create_dir_all(&raw_dir)?;
    let manifest_path = work.path().join("manifest.json");
    let recorder_path = work.path().join("recorder.py");
    let template_path = work.path().join("candidate.json");
    let expected_path = work.path().join("expected.json");
    fs::write(&manifest_path, &manifest)?;
    fs::write(&recorder_path, &recorder)?;
    fs::write(&template_path, candidate)?;

    let mut captures = serde_json::Map::new();
    for requirement in &binding.required_captures {
        if !safe_capture_subject(&requirement.subject) {
            return Err(refused("manifest capture subject is unsafe for replay").into());
        }
        let event = history
            .events()
            .iter()
            .find(|event| {
                event.subject == requirement.subject
                    && event.phase == requirement.phase
                    && event.source == requirement.source
                    && event.provenance != OperatorCaptureProvenance::OperatorAttested
            })
            .ok_or_else(|| refused("trusted recorder replay lacks a required capture event"))?;
        let (format_name, suffix) = match requirement.media_type.as_str() {
            "application/json" => ("json", "json"),
            "application/x-ndjson" => ("jsonl", "jsonl"),
            "text/plain; charset=utf-8" => ("text", "txt"),
            _ => return Err(refused("trusted recorder replay has an unknown media type").into()),
        };
        let filename = format!("{}.{}", requirement.subject, suffix);
        let bytes = stable_regular_bytes(
            &protected_evidence_path(&context.paths, binding, &requirement.subject),
            "protected canonical evidence",
        )?;
        if sha256_bytes(&bytes) != event.content_sha256
            || u64::try_from(bytes.len()).ok() != Some(event.content_bytes)
        {
            return Err(refused("trusted recorder replay evidence changed").into());
        }
        fs::write(raw_dir.join(&filename), &bytes)?;
        captures.insert(
            requirement.subject.clone(),
            serde_json::json!({
                "file": filename,
                "format": format_name,
                "captured_at": event.timestamp,
                "bytes": event.content_bytes,
                "sha256": event.content_sha256,
                "provenance": "trusted_canonical",
                "source": capture_source_name(event.source),
                "phase": capture_phase_name(event.phase),
            }),
        );
    }
    let intervention = history
        .events()
        .iter()
        .find(|event| event.kind == OperatorCaptureEventKind::InterventionRecorded);
    let cleanup = history
        .events()
        .iter()
        .find(|event| event.kind == OperatorCaptureEventKind::CleanupRecorded);
    if let Some(event) = intervention {
        let expected_source = expected_operator_intervention_source(&binding.trial_id)
            .ok_or_else(|| refused("trusted recorder replay has no intervention policy"))?;
        if event.subject != "intervention"
            || event.phase != OperatorCapturePhase::Intervention
            || event.source != expected_source
            || event.provenance != OperatorCaptureProvenance::TrustedSupervisor
        {
            return Err(refused(
                "trusted recorder replay intervention has mismatched identity or provenance",
            )
            .into());
        }
        let bytes = stable_regular_bytes(
            &protected_evidence_path(&context.paths, binding, "intervention"),
            "protected canonical intervention",
        )?;
        if sha256_bytes(&bytes) != event.content_sha256
            || u64::try_from(bytes.len()).ok() != Some(event.content_bytes)
        {
            return Err(refused("trusted recorder replay intervention changed").into());
        }
        fs::write(raw_dir.join("intervention.json"), bytes)?;
    }
    let lifecycle = |event: Option<&deadreckon_protocol::OperatorCaptureEvent>,
                     completed: &str,
                     absent: &str| {
        event.map_or_else(
            || {
                serde_json::json!({
                    "status": absent,
                    "recorded_at": null,
                    "detail_sha256": null,
                })
            },
            |event| {
                serde_json::json!({
                    "status": completed,
                    "recorded_at": event.timestamp,
                    "detail_sha256": event.content_sha256,
                })
            },
        )
    };
    let state = serde_json::json!({
        "schema_version": 2,
        "trial_id": binding.trial_id,
        "session_id": binding.session_id,
        "source_revision": binding.source_revision,
        "created_at": binding.created_at,
        "capture_mode": "trusted",
        "capture_provenance": {
            "status": "trusted_prepared",
            "receipt_sha256": null,
            "reason": "trusted deterministic replay",
        },
        "trusted_capture": {
            "backend": binding.declared_backend,
        },
        "captures": captures,
        "intervention": lifecycle(intervention, "performed", "not_performed"),
        "cleanup": lifecycle(cleanup, "completed", "not_run"),
    });
    fs::write(
        trial_dir.join("trial-state.json"),
        serde_json::to_vec_pretty(&state)?,
    )?;
    let output = trusted_recorder_command(interpreter_path, &recorder_path)
        .arg("--manifest")
        .arg(&manifest_path)
        .arg("evaluate-bundle")
        .arg("--trial-dir")
        .arg(&trial_dir)
        .arg("--template")
        .arg(&template_path)
        .arg("--output")
        .arg(&expected_path)
        .output()?;
    if !output.status.success() {
        return Err(
            refused("binding-hashed recorder rejected the trusted evaluation bundle").into(),
        );
    }
    let expected = stable_regular_bytes(&expected_path, "trusted recorder evaluation")?;
    if expected != candidate {
        return Err(refused(
            "submitted evaluation does not byte-match trusted deterministic recorder output",
        )
        .into());
    }
    Ok(())
}

fn trusted_recorder_command(interpreter_path: &Path, recorder_path: &Path) -> Command {
    let mut command = Command::new(interpreter_path);
    command
        .arg("-I")
        .arg("-s")
        .arg("-B")
        .arg(recorder_path)
        .env_clear()
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0");
    command
}

fn validate_final_intervention_freshness(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
    history: &deadreckon_core::OperatorCaptureHistory,
) -> AnyResult<()> {
    let before_boundary = history
        .events()
        .iter()
        .filter(|event| event.phase == OperatorCapturePhase::Before)
        .map(|event| event.timestamp)
        .max()
        .unwrap_or(binding.created_at);
    let event = history
        .events()
        .iter()
        .find(|event| event.kind == OperatorCaptureEventKind::InterventionRecorded)
        .ok_or_else(|| refused("passed evaluation has no intervention event"))?;
    let bytes = stable_regular_bytes(
        &protected_evidence_path(&context.paths, binding, &event.subject),
        "protected intervention evidence",
    )?;
    let observed_at = match event.source {
        OperatorCaptureSource::JobIntervention => {
            let job_event: JobEvent = serde_json::from_slice(&bytes)?;
            if job_event.kind != expected_job_intervention_kind(&binding.trial_id)? {
                return Err(refused("intervention kind changed before final seal").into());
            }
            job_event.timestamp
        }
        OperatorCaptureSource::SandboxBoundaryObservation => {
            serde_json::from_slice::<SandboxBoundaryObservation>(&bytes)?.observed_at
        }
        OperatorCaptureSource::CampaignIntervention => {
            serde_json::from_slice::<deadreckon_core::campaign::CampaignEvent>(&bytes)?.ts
        }
        _ => return Err(refused("passed trial has an invalid intervention source").into()),
    };
    if observed_at <= before_boundary {
        return Err(refused(
            "authoritative intervention does not follow the final authenticated before boundary",
        )
        .into());
    }
    Ok(())
}

fn safe_capture_subject(subject: &str) -> bool {
    !subject.is_empty()
        && subject.bytes().enumerate().all(|(index, byte)| {
            (index == 0 && byte.is_ascii_lowercase())
                || (index > 0
                    && (byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
        })
}

fn capture_phase_name(phase: OperatorCapturePhase) -> &'static str {
    match phase {
        OperatorCapturePhase::Prepared => "prepared",
        OperatorCapturePhase::Before => "before",
        OperatorCapturePhase::Intervention => "intervention",
        OperatorCapturePhase::After => "after",
        OperatorCapturePhase::Cleanup => "cleanup",
        OperatorCapturePhase::Finalized => "finalized",
    }
}

fn capture_status_name(status: OperatorCaptureStatus) -> &'static str {
    match status {
        OperatorCaptureStatus::Passed => "passed",
        OperatorCaptureStatus::Failed => "failed",
        OperatorCaptureStatus::Inconclusive => "inconclusive",
        OperatorCaptureStatus::NotRun => "not_run",
    }
}

fn capture_source_name(source: OperatorCaptureSource) -> &'static str {
    match source {
        OperatorCaptureSource::Binding => "binding",
        OperatorCaptureSource::JobView => "job-view",
        OperatorCaptureSource::JobEvents => "job-events",
        OperatorCaptureSource::JobIntervention => "job-intervention",
        OperatorCaptureSource::JobCleanup => "job-cleanup",
        OperatorCaptureSource::Job => "job",
        OperatorCaptureSource::Authority => "authority",
        OperatorCaptureSource::LaunchPlan => "launch-plan",
        OperatorCaptureSource::Lease => "lease",
        OperatorCaptureSource::JobReport => "job-report",
        OperatorCaptureSource::Receipt => "receipt",
        OperatorCaptureSource::SupervisedChild => "supervised-child",
        OperatorCaptureSource::HostBootId => "host-boot-id",
        OperatorCaptureSource::SemanticJudgment => "semantic-judgment",
        OperatorCaptureSource::ParentRepairManifest => "parent-repair-manifest",
        OperatorCaptureSource::ParentRepairCandidate => "parent-repair-candidate",
        OperatorCaptureSource::Doctor => "doctor",
        OperatorCaptureSource::SupervisorServiceStatus => "supervisor-service-status",
        OperatorCaptureSource::ParentArtifact => "parent-artifact",
        OperatorCaptureSource::ParentEvents => "parent-events",
        OperatorCaptureSource::Campaign => "campaign",
        OperatorCaptureSource::CampaignEvents => "campaign-events",
        OperatorCaptureSource::ActivePlan => "active-plan",
        OperatorCaptureSource::ActivePlanEvents => "active-plan-events",
        OperatorCaptureSource::SandboxBoundaryObservation => "sandbox-boundary-observation",
        OperatorCaptureSource::CampaignIntervention => "campaign-intervention",
        OperatorCaptureSource::ResultEnvelope => "result-envelope",
        OperatorCaptureSource::ManualFile => "manual-file",
        OperatorCaptureSource::UnavailableObjective => "unavailable-objective",
    }
}

fn validate_root_schema_contract(value: &Value, schema: &Value) -> AnyResult<()> {
    let object = value
        .as_object()
        .ok_or_else(|| refused("result schema requires a JSON object"))?;
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("bound result schema has no root required array"))?;
    for field in required {
        let field = field
            .as_str()
            .ok_or_else(|| refused("bound result schema has a non-string required field"))?;
        if !object.contains_key(field) {
            return Err(refused(&format!(
                "capture result omits schema-required field {field}"
            ))
            .into());
        }
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| refused("bound result schema has no root properties object"))?;
    if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
        && object.keys().any(|key| !properties.contains_key(key))
    {
        return Err(refused("capture result contains a root field forbidden by its schema").into());
    }
    for (name, property) in properties {
        let Some(found) = object.get(name) else {
            continue;
        };
        if let Some(expected) = property.get("const")
            && found != expected
        {
            return Err(refused(&format!(
                "capture result field {name} violates its schema const"
            ))
            .into());
        }
        if let Some(allowed) = property.get("enum").and_then(Value::as_array)
            && !allowed.contains(found)
        {
            return Err(refused(&format!(
                "capture result field {name} is outside its schema enum"
            ))
            .into());
        }
        if let Some(expected_type) = property.get("type").and_then(Value::as_str) {
            let valid = match expected_type {
                "object" => found.is_object(),
                "array" => found.is_array(),
                "string" => found.is_string(),
                "integer" => found.as_i64().is_some() || found.as_u64().is_some(),
                "number" => found.is_number(),
                "boolean" => found.is_boolean(),
                "null" => found.is_null(),
                _ => false,
            };
            if !valid {
                return Err(refused(&format!(
                    "capture result field {name} violates its schema type"
                ))
                .into());
            }
        }
    }
    Ok(())
}

fn validated_actual_provider_routes(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<BTreeMap<String, Vec<String>>> {
    let worker_role = if binding.provider_routes.contains_key("hostile_worker") {
        "hostile_worker"
    } else {
        "worker"
    };
    let mut routes = BTreeMap::new();
    let mut worker_routes = Vec::new();
    let view = JobView::load(&context.paths, binding.job_id.as_ref())?;
    for attempt in &view.attempts {
        if attempt.provider.trim().is_empty() {
            return Err(refused("persisted Job attempt has no provider route").into());
        }
        if !worker_routes.contains(&attempt.provider) {
            worker_routes.push(attempt.provider.clone());
        }
    }
    routes.insert(worker_role.to_string(), worker_routes);
    let judgment_bytes =
        canonical_same_id_proof(context, binding, "proofs/semantic-judgment.json", true)?;
    let judgment: SemanticJudgment = serde_json::from_slice(&judgment_bytes)?;
    if judgment.provider.trim().is_empty() {
        return Err(refused("semantic judgment has no provider route").into());
    }
    routes.insert("independent_judge".to_string(), vec![judgment.provider]);
    if binding.provider_routes.contains_key("planner") {
        let planner = if let Ok(campaign) = deadreckon_core::campaign::read_campaign(
            &context.paths.plan_dir(binding.job_id.as_ref()),
        ) {
            campaign.providers.planner
        } else {
            load_plan(&context.paths, binding.job_id.as_ref())?
                .providers
                .planner
        }
        .filter(|route| !route.trim().is_empty())
        .ok_or_else(|| refused("persisted Plan has no planner provider route"))?;
        routes.insert("planner".to_string(), vec![planner]);
    }
    if routes.values().any(Vec::is_empty) {
        return Err(refused("no persisted provider route evidence is available").into());
    }
    Ok(routes)
}

fn canonical_observation(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
    source: CanonicalSource,
    phase: OperatorCapturePhase,
) -> AnyResult<CanonicalObservation> {
    let job_id = binding.job_id.as_ref();
    let bytes = match source {
        CanonicalSource::JobView => serde_json::to_vec(&JobView::load(&context.paths, job_id)?)?,
        CanonicalSource::JobEvents => canonical_job_events(&context.paths, job_id)?,
        CanonicalSource::JobIntervention => canonical_job_intervention(&context.paths, binding)?,
        CanonicalSource::JobCleanup => canonical_job_cleanup(&context.paths, binding)?,
        CanonicalSource::Job => {
            let path = context.paths.job_json(job_id);
            let bytes = stable_regular_bytes(&path, "job.json")?;
            let parsed: Job = serde_json::from_slice(&bytes)?;
            if parsed != load_job(&context.paths, job_id)? {
                return Err(refused("job.json changed across validation").into());
            }
            bytes
        }
        CanonicalSource::Authority => {
            let path = context.paths.job_authority(job_id);
            let bytes = stable_regular_bytes(&path, "authority.json")?;
            let authority: JobAuthority = serde_json::from_slice(&bytes)?;
            if authority.job_id != binding.job_id
                || authority.source_revision.as_deref() != Some(&binding.source_revision)
                || authority.source_tree_sha256 != binding.source_tree_sha256
            {
                return Err(refused("authority.json does not match the signed binding").into());
            }
            bytes
        }
        CanonicalSource::LaunchPlan => {
            let path = context.paths.job_launch_plan(job_id);
            let bytes = stable_regular_bytes(&path, "launch-plan.json")?;
            let _: Value = serde_json::from_slice(&bytes)?;
            if sha256_bytes(&bytes) != load_job(&context.paths, job_id)?.launch_plan_sha256 {
                return Err(refused("launch-plan.json digest does not match job.json").into());
            }
            bytes
        }
        CanonicalSource::Lease => {
            let path = context.paths.job_lease(job_id);
            let bytes = stable_regular_bytes(&path, "lease.json")?;
            let parsed: JobLease = serde_json::from_slice(&bytes)?;
            if parsed != load_job_lease(&context.paths, &binding.job_id)? {
                return Err(refused("lease.json changed across validation").into());
            }
            bytes
        }
        CanonicalSource::JobReport => canonical_job_report(context, binding)?,
        CanonicalSource::Receipt => canonical_completion_receipt(context, binding)?,
        CanonicalSource::SupervisedChild => canonical_supervised_child(&context.paths, binding)?,
        CanonicalSource::HostBootId => canonical_boot_identity()?,
        CanonicalSource::SemanticJudgment => {
            canonical_same_id_proof(context, binding, "proofs/semantic-judgment.json", true)?
        }
        CanonicalSource::ParentRepairManifest => {
            canonical_same_id_proof(context, binding, "proofs/parent-repair.json", false)?
        }
        CanonicalSource::ParentRepairCandidate => canonical_same_id_proof(
            context,
            binding,
            "proofs/parent-repair-candidate.json",
            false,
        )?,
        CanonicalSource::Doctor => {
            canonical_deadreckon_json(context, binding, &["doctor", "--json"])?
        }
        CanonicalSource::SupervisorServiceStatus => {
            canonical_supervisor_service_status(context, binding)?
        }
        CanonicalSource::ParentArtifact => canonical_parent_artifact(context, binding)?,
        CanonicalSource::ParentEvents => canonical_parent_events(context, binding)?,
        CanonicalSource::Campaign => canonical_campaign(&context.paths, binding)?,
        CanonicalSource::CampaignEvents => canonical_campaign_events(&context.paths, binding)?,
        CanonicalSource::ActivePlan => canonical_active_plan(&context.paths, binding, phase)?,
        CanonicalSource::ActivePlanEvents => {
            canonical_active_plan_events(&context.paths, binding, phase)?
        }
        CanonicalSource::SandboxBoundaryObservation => {
            canonical_sandbox_boundary_observation(context, binding)?
        }
        CanonicalSource::CampaignIntervention => {
            canonical_campaign_intervention(&context.paths, binding)?
        }
    };
    Ok(CanonicalObservation {
        bytes,
        source: source.protocol(),
        provenance: source.provenance(),
        media_type: source.media_type(),
    })
}

fn canonical_job_events(paths: &DeadreckonPaths, job_id: &str) -> AnyResult<Vec<u8>> {
    let job = load_job(paths, job_id)?;
    let path = paths.job_events(job_id);
    let before = read_job_history(&path)?;
    let before_projection = reduce_job_history(&job.job_id, &before)?;
    let exact = stable_regular_bytes(&path, "job-events.jsonl")?;
    let after = read_job_history(&path)?;
    let after_projection = reduce_job_history(&job.job_id, &after)?;
    if before.events() != after.events() || before_projection != after_projection {
        return Err(refused("Job event history changed across stable capture").into());
    }
    Ok(exact)
}

fn canonical_job_intervention(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    let job = load_job(paths, binding.job_id.as_ref())?;
    let history = read_job_history(&paths.job_events(binding.job_id.as_ref()))?;
    let projection = reduce_job_history(&job.job_id, &history)?;
    let expected_kind = expected_job_intervention_kind(&binding.trial_id)?;
    let capture_history =
        read_operator_capture_history(paths, binding.job_id.as_ref(), &binding.session_id)?;
    let before_boundary = capture_history
        .events()
        .iter()
        .filter(|event| event.phase == OperatorCapturePhase::Before)
        .map(|event| event.timestamp)
        .max()
        .unwrap_or(binding.created_at);
    let event = history
        .events()
        .iter()
        .rev()
        .find(|event| {
            job_intervention_is_fresh(event.kind, event.timestamp, expected_kind, before_boundary)
        })
        .ok_or_else(|| {
            refused("Job history has no trial-specific intervention after the before boundary")
        })?;
    if event.job_id != binding.job_id || event.sequence.get() > projection.last_sequence {
        return Err(refused("intervention event is not part of the bound Job projection").into());
    }
    Ok(serde_json::to_vec(event)?)
}

fn expected_job_intervention_kind(trial_id: &str) -> AnyResult<JobEventKind> {
    match trial_id {
        "live_provider_worker_kill" | "live_provider_network_loss" => {
            Ok(JobEventKind::AttemptStopped)
        }
        "live_provider_supervisor_restart" | "machine_reboot" => Ok(JobEventKind::LeaseReclaimed),
        "live_provider_parent_repair" => Ok(JobEventKind::SemanticJudgeRevise),
        _ => {
            Err(refused("this trial requires a different authoritative intervention source").into())
        }
    }
}

fn job_intervention_is_fresh(
    found_kind: JobEventKind,
    found_at: chrono::DateTime<Utc>,
    expected_kind: JobEventKind,
    before_boundary: chrono::DateTime<Utc>,
) -> bool {
    found_kind == expected_kind && found_at > before_boundary
}

fn canonical_sandbox_boundary_observation(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    if !matches!(
        binding.trial_id.as_str(),
        "cross_provider_gate_attack" | "linux_bubblewrap_gate_boundary" | "docker_gate_boundary"
    ) {
        return Err(refused(
            "sandbox boundary observations are not an intervention source for this trial",
        )
        .into());
    }
    let authority = load_authority(&context.paths, binding.job_id.as_ref())?;
    let state = load_run(&context.paths, authority.run_id.as_ref())?;
    let validated = deadreckon_core::validate_sandbox_boundary_observation(
        &context.paths,
        &state,
        &authority,
        &binding.declared_backend,
    )?;
    let capture_history = read_operator_capture_history(
        &context.paths,
        binding.job_id.as_ref(),
        &binding.session_id,
    )?;
    let before_boundary = capture_history
        .events()
        .iter()
        .filter(|event| event.phase == OperatorCapturePhase::Before)
        .map(|event| event.timestamp)
        .max()
        .unwrap_or(binding.created_at);
    if validated.observed_at <= before_boundary {
        return Err(refused("sandbox boundary observation predates the capture boundary").into());
    }
    let path = context
        .paths
        .job_sandbox_boundary_observation(binding.job_id.as_ref());
    let bytes = stable_regular_bytes(&path, "sandbox-boundary-observation.json")?;
    let parsed: deadreckon_protocol::SandboxBoundaryObservation = serde_json::from_slice(&bytes)?;
    if parsed != validated {
        return Err(refused("sandbox boundary observation changed across validation").into());
    }
    Ok(bytes)
}

fn canonical_campaign_intervention(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    if binding.trial_id != "live_campaign_interruption_recovery" {
        return Err(refused("Campaign intervention source is not valid for this trial").into());
    }
    canonical_campaign(paths, binding)?;
    let capture_history =
        read_operator_capture_history(paths, binding.job_id.as_ref(), &binding.session_id)?;
    let before_boundary = capture_history
        .events()
        .iter()
        .filter(|event| event.phase == OperatorCapturePhase::Before)
        .map(|event| event.timestamp)
        .max()
        .unwrap_or(binding.created_at);
    let events =
        deadreckon_core::campaign::read_campaign_events(&paths.plan_dir(binding.job_id.as_ref()))?;
    let plan_id = resolve_active_plan_id(paths, binding, OperatorCapturePhase::After)?;
    let event = events
        .iter()
        .rev()
        .find(|event| {
            event.ts > before_boundary
                && matches!(
                    event.kind.as_str(),
                    "sub_process_relaunch_safe" | "sub_process_adopted" | "sub_recovered"
                )
                && event.detail.get("plan_id").and_then(Value::as_str) == Some(plan_id.as_str())
        })
        .ok_or_else(|| {
            refused("Campaign has no guarded interruption recovery event after the before boundary")
        })?;
    Ok(serde_json::to_vec(event)?)
}

fn canonical_job_cleanup(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    let view = JobView::load(paths, binding.job_id.as_ref())?;
    if !view.projection.is_terminal() {
        return Err(refused("cleanup capture requires a terminal Job projection").into());
    }
    let child_path = paths
        .job_dir(binding.job_id.as_ref())
        .join("supervised-child.json");
    if child_path.exists() {
        return Err(
            refused("cleanup capture found residual supervised-child control state").into(),
        );
    }
    let history = read_job_history(&paths.job_events(binding.job_id.as_ref()))?;
    for event in history
        .events()
        .iter()
        .filter(|event| event.kind == JobEventKind::ChildLinked)
    {
        let pid = event.detail.get("pid").and_then(Value::as_u64).unwrap_or(0);
        if pid > 0 && u32::try_from(pid).ok().is_some_and(pid_is_alive) {
            return Err(refused("cleanup capture found a live supervised child leader").into());
        }
        let pgid = event
            .detail
            .get("process_group")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if pgid > 0 && u32::try_from(pgid).ok().is_some_and(process_group_is_alive) {
            return Err(
                refused("cleanup capture found a residual supervised process group").into(),
            );
        }
    }
    for attempt in &view.attempts {
        let child_pids = paths
            .run_root(&attempt.id.scope, &attempt.id.run_id)
            .join("child-pids");
        if child_pids.is_dir()
            && fs::read_dir(&child_pids)?
                .filter_map(Result::ok)
                .any(|entry| entry.path().is_file())
        {
            return Err(refused(
                "cleanup capture found residual guarded gate-evaluator control state",
            )
            .into());
        }
    }
    Ok(serde_json::to_vec(&view)?)
}

#[cfg(unix)]
fn process_group_is_alive(pgid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let Ok(pgid) = i32::try_from(pgid) else {
        return true;
    };
    match kill(Pid::from_raw(-pgid), None) {
        Ok(()) | Err(Errno::EPERM) => true,
        Err(Errno::ESRCH) => false,
        Err(_) => true,
    }
}

#[cfg(not(unix))]
fn process_group_is_alive(_pgid: u32) -> bool {
    false
}

fn canonical_completion_receipt(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    let job_id = binding.job_id.as_ref();
    let authority = load_authority(&context.paths, job_id)?;
    let state = load_run(&context.paths, authority.run_id.as_ref())?;
    let validated = validate_completion_receipt(&context.paths, &state)?;
    let path = context.paths.job_receipt(job_id);
    let bytes = stable_regular_bytes(&path, "receipt.json")?;
    let parsed: CompletionReceipt = serde_json::from_slice(&bytes)?;
    if parsed != validated || parsed.job_id != binding.job_id {
        return Err(refused("completion receipt does not validate for the bound Job").into());
    }
    Ok(bytes)
}

fn validated_completion_lineage(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<OperatorCaptureCompletionLineage> {
    let bytes = canonical_completion_receipt(context, binding)?;
    let receipt: CompletionReceipt = serde_json::from_slice(&bytes)?;
    Ok(OperatorCaptureCompletionLineage {
        completion_receipt_sha256: sha256_bytes(&bytes),
        authority_sha256: receipt.authority_sha256,
        contract_sha256: receipt.contract_sha256,
        effective_policy_sha256: receipt.effective_policy_sha256,
        launch_plan_sha256: receipt.launch_plan_sha256,
        source_tree_sha256: receipt.source_tree_sha256,
        source_revision: receipt.source_revision,
        result_tree_sha256: receipt.result_tree_sha256,
        result_revision: receipt.result_revision,
    })
}

fn canonical_supervised_child(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    let path = paths
        .job_dir(binding.job_id.as_ref())
        .join("supervised-child.json");
    let bytes = stable_regular_bytes(&path, "supervised-child.json")?;
    let value: Value = serde_json::from_slice(&bytes)?;
    let object = value
        .as_object()
        .ok_or_else(|| refused("supervised-child.json must contain an object"))?;
    let pid = object.get("pid").and_then(Value::as_u64).unwrap_or(0);
    let attempt = object.get("attempt").and_then(Value::as_u64).unwrap_or(0);
    let launch_id = object
        .get("launch_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let release = object
        .get("release_token_sha256")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if pid == 0 || attempt == 0 || launch_id.is_empty() || !valid_sha256(release) {
        return Err(refused("supervised-child.json has incomplete durable identity").into());
    }
    let history = read_job_history(&paths.job_events(binding.job_id.as_ref()))?;
    let linked = history.events().iter().any(|event| {
        event.kind == JobEventKind::ChildLinked
            && event.detail.get("launch_id").and_then(Value::as_str) == Some(launch_id)
            && event.detail.get("attempt").and_then(Value::as_u64) == Some(attempt)
            && event.detail.get("pid").and_then(Value::as_u64) == Some(pid)
    });
    if !linked {
        return Err(refused(
            "supervised-child.json is not bound to a durable ChildLinked Job event",
        )
        .into());
    }
    Ok(bytes)
}

fn canonical_boot_identity() -> AnyResult<Vec<u8>> {
    if std::env::var_os("DEADRECKON_BOOT_ID").is_some() {
        return Err(refused("host boot observation refuses DEADRECKON_BOOT_ID overrides").into());
    }
    let identity = boot_identity();
    if identity == "unknown-boot" {
        return Err(refused("host boot identity is not authoritative on this platform").into());
    }
    Ok(format!("{identity}\n").into_bytes())
}

fn canonical_job_report(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    let bytes = canonical_deadreckon_json(
        context,
        binding,
        &["report", binding.job_id.as_ref(), "--json", "--plain"],
    )?;
    let report: Value = serde_json::from_slice(&bytes)?;
    let view = JobView::load(&context.paths, binding.job_id.as_ref())?;
    if report.get("id").and_then(Value::as_str) != Some(binding.job_id.as_ref())
        || report
            .pointer("/lifecycle/last_event_sequence")
            .and_then(Value::as_u64)
            != Some(view.projection.last_sequence)
        || report.get("phase") != Some(&serde_json::to_value(view.projection.phase)?)
        || report.get("outcome") != Some(&serde_json::to_value(view.projection.outcome)?)
    {
        return Err(refused("DeadReckon Job report does not match the current JobView").into());
    }
    Ok(bytes)
}

fn canonical_deadreckon_json(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
    args: &[&str],
) -> AnyResult<Vec<u8>> {
    let binary = trusted_deadreckon_binary(context, binding)?;
    let output = Command::new(binary).args(args).output()?;
    if !output.status.success() {
        return Err(refused("trusted DeadReckon observation command failed").into());
    }
    let _: Value = serde_json::from_slice(&output.stdout)?;
    Ok(output.stdout)
}

fn canonical_supervisor_service_status(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    if std::env::var_os("DEADRECKON_BOOT_ID").is_some() {
        return Err(
            refused("supervisor service observation refuses DEADRECKON_BOOT_ID overrides").into(),
        );
    }
    let bytes = canonical_deadreckon_json(context, binding, &["supervisor", "status", "--json"])?;
    let value: Value = serde_json::from_slice(&bytes)?;
    let source = value
        .get("boot_identity_source")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let current_boot = value
        .get("current_boot_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if value.get("test_override").and_then(Value::as_bool) != Some(false)
        || matches!(source, "" | "unknown" | "test_override")
        || current_boot.is_empty()
        || current_boot == "unknown-boot"
    {
        return Err(
            refused("supervisor service status has no authoritative host boot identity").into(),
        );
    }
    Ok(bytes)
}

fn canonical_parent_artifact(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    let job = load_job(&context.paths, binding.job_id.as_ref())?;
    let state = load_run(&context.paths, binding.job_id.as_ref())?;
    if state.run_id.as_str() != binding.job_id.as_ref()
        || state.scope != job.scope
        || state.goal != job.goal
    {
        return Err(refused("same-ID parent run does not match the bound Job").into());
    }
    let bytes = stable_regular_bytes(&state.state_path(), "parent state.json")?;
    let parsed: deadreckon_core::PipelineState = serde_json::from_slice(&bytes)?;
    if parsed.run_id != state.run_id
        || parsed.scope != state.scope
        || parsed.goal != state.goal
        || parsed.run_root != state.run_root
    {
        return Err(refused("parent state.json changed across validation").into());
    }
    Ok(bytes)
}

fn canonical_parent_events(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    let job = load_job(&context.paths, binding.job_id.as_ref())?;
    let state = load_run(&context.paths, binding.job_id.as_ref())?;
    if state.run_id.as_str() != binding.job_id.as_ref() || state.scope != job.scope {
        return Err(refused("same-ID parent run does not match the bound Job").into());
    }
    let path = state.run_root.join(RUN_EVENTS_JSONL);
    canonical_jsonl(&path, "parent events", |value| {
        value.get("run_id").and_then(Value::as_str) == Some(binding.job_id.as_ref())
    })
}

fn canonical_campaign(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    let dir = paths.plan_dir(binding.job_id.as_ref());
    let campaign = deadreckon_core::campaign::read_campaign(&dir)?;
    if campaign.campaign_id != binding.job_id.as_ref() {
        return Err(refused("campaign artifact contains a foreign Job identity").into());
    }
    let path = deadreckon_core::campaign::campaign_path_for_plan_dir(&dir);
    let bytes = stable_regular_bytes(&path, "campaign.json")?;
    let parsed: deadreckon_core::campaign::Campaign = serde_json::from_slice(&bytes)?;
    if parsed != campaign {
        return Err(refused("campaign.json changed across validation").into());
    }
    Ok(bytes)
}

fn canonical_campaign_events(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> AnyResult<Vec<u8>> {
    canonical_campaign(paths, binding)?;
    let path =
        deadreckon_core::campaign::campaign_events_path(&paths.plan_dir(binding.job_id.as_ref()));
    let before =
        deadreckon_core::campaign::read_campaign_events(&paths.plan_dir(binding.job_id.as_ref()))?;
    let exact = stable_regular_bytes(&path, "campaign-events.jsonl")?;
    let after =
        deadreckon_core::campaign::read_campaign_events(&paths.plan_dir(binding.job_id.as_ref()))?;
    if before != after {
        return Err(refused("campaign events changed across validation").into());
    }
    Ok(exact)
}

fn canonical_active_plan(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
    phase: OperatorCapturePhase,
) -> AnyResult<Vec<u8>> {
    let plan_id = resolve_active_plan_id(paths, binding, phase)?;
    let plan = load_plan(paths, &plan_id)?;
    if plan.plan_id != plan_id {
        return Err(refused("active Plan artifact contains a foreign identity").into());
    }
    let path = paths.plan_json(&plan_id);
    let bytes = stable_regular_bytes(&path, "plan.json")?;
    let parsed: deadreckon_core::Plan = serde_json::from_slice(&bytes)?;
    if parsed != plan {
        return Err(refused("plan.json changed across validation").into());
    }
    Ok(bytes)
}

fn canonical_active_plan_events(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
    phase: OperatorCapturePhase,
) -> AnyResult<Vec<u8>> {
    let plan_id = resolve_active_plan_id(paths, binding, phase)?;
    canonical_active_plan(paths, binding, phase)?;
    let before = read_plan_events(paths, &plan_id)?;
    let path = paths.plan_events(&plan_id);
    let exact = stable_regular_bytes(&path, "plan-events.jsonl")?;
    let after = read_plan_events(paths, &plan_id)?;
    if before != after || after.iter().any(|event| event.plan_id != plan_id) {
        return Err(refused("active Plan events are foreign or changed across validation").into());
    }
    Ok(exact)
}

fn resolve_active_plan_id(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
    phase: OperatorCapturePhase,
) -> AnyResult<String> {
    let campaign =
        deadreckon_core::campaign::read_campaign(&paths.plan_dir(binding.job_id.as_ref()))?;
    if campaign.campaign_id != binding.job_id.as_ref() {
        return Err(refused("Campaign does not match the bound Job").into());
    }
    if phase == OperatorCapturePhase::After
        && let Some(requirement) = binding.required_captures.iter().find(|requirement| {
            requirement.phase == OperatorCapturePhase::Before
                && requirement.source == OperatorCaptureSource::ActivePlan
        })
    {
        let path = protected_evidence_path(paths, binding, &requirement.subject);
        if path.is_file() {
            let bytes = stable_regular_bytes(&path, "protected before active Plan")?;
            let plan: deadreckon_core::Plan = serde_json::from_slice(&bytes)?;
            if campaign
                .sub_goals
                .iter()
                .any(|sub| sub.sub_plan_id.as_deref() == Some(plan.plan_id.as_str()))
            {
                return Ok(plan.plan_id);
            }
            return Err(refused(
                "protected before active Plan is not linked by the bound Campaign",
            )
            .into());
        }
    }
    let active = campaign
        .sub_goals
        .iter()
        .filter(|sub| sub.status == deadreckon_core::campaign::SubGoalStatus::Running)
        .filter_map(|sub| sub.sub_plan_id.as_deref())
        .collect::<BTreeSet<_>>();
    if active.len() != 1 {
        return Err(refused("Campaign must have exactly one persisted active sub-Plan").into());
    }
    Ok(active.into_iter().next().unwrap_or_default().to_string())
}

fn canonical_jsonl(
    path: &Path,
    label: &str,
    validate: impl Fn(&Value) -> bool,
) -> AnyResult<Vec<u8>> {
    let exact = stable_regular_bytes(path, label)?;
    if !exact.is_empty() && !exact.ends_with(b"\n") {
        return Err(refused(&format!("{label} has a torn final row")).into());
    }
    for line in exact
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let value: Value = serde_json::from_slice(line)?;
        if !validate(&value) {
            return Err(refused(&format!("{label} contains a foreign identity")).into());
        }
    }
    Ok(exact)
}

fn canonical_same_id_proof(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
    relative: &str,
    semantic: bool,
) -> AnyResult<Vec<u8>> {
    let job = load_job(&context.paths, binding.job_id.as_ref())?;
    let authority = load_authority(&context.paths, binding.job_id.as_ref())?;
    if authority.run_id.as_ref() != binding.job_id.as_ref() {
        return Err(refused("same-ID parent run is not resolvable for this Job").into());
    }
    let run_root = context
        .paths
        .run_root(&job.scope, authority.run_id.as_ref());
    let path = run_root.join(relative);
    let bytes = stable_regular_bytes(&path, relative)?;
    if semantic {
        let judgment: SemanticJudgment = serde_json::from_slice(&bytes)?;
        if judgment.job_id != binding.job_id || judgment.run_id != authority.run_id {
            return Err(refused("semantic judgment contains a foreign Job or run").into());
        }
    } else {
        let value: Value = serde_json::from_slice(&bytes)?;
        let object = value
            .as_object()
            .ok_or_else(|| refused("parent repair proof must contain a JSON object"))?;
        if let Some(found) = object.get("job_id").and_then(Value::as_str)
            && found != binding.job_id.as_ref()
        {
            return Err(refused("parent repair proof contains a foreign Job").into());
        }
    }
    Ok(bytes)
}

fn load_authority(paths: &DeadreckonPaths, job_id: &str) -> AnyResult<JobAuthority> {
    let path = paths.job_authority(job_id);
    let bytes = stable_regular_bytes(&path, "authority.json")?;
    let authority: JobAuthority = serde_json::from_slice(&bytes)?;
    if authority.job_id.as_ref() != job_id {
        return Err(refused("authority.json contains a foreign Job").into());
    }
    Ok(authority)
}

fn manifest_requirements(
    manifest: &[u8],
    trial_id: &str,
) -> AnyResult<Vec<OperatorCaptureRequirement>> {
    let value: Value = serde_json::from_slice(manifest)?;
    let trials = value
        .get("trials")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("manifest has no trials array"))?;
    let trial = trials
        .iter()
        .find(|trial| trial.get("id").and_then(Value::as_str) == Some(trial_id))
        .ok_or_else(|| refused("manifest does not contain the requested trial ID"))?;
    let evidence = trial
        .get("evidence")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("trial has no evidence declarations"))?;
    let phases = trial
        .get("capture_phases")
        .and_then(Value::as_object)
        .ok_or_else(|| refused("trial has no explicit capture_phases"))?;
    let mut phase_by_subject = BTreeMap::new();
    for (name, phase) in [
        ("before", OperatorCapturePhase::Before),
        ("after", OperatorCapturePhase::After),
    ] {
        let subjects = phases
            .get(name)
            .and_then(Value::as_array)
            .ok_or_else(|| refused("capture_phases must contain before and after arrays"))?;
        for subject in subjects {
            let subject = subject
                .as_str()
                .ok_or_else(|| refused("capture phase subject must be a string"))?;
            if phase_by_subject.insert(subject, phase).is_some() {
                return Err(refused("capture subject appears in more than one phase").into());
            }
        }
    }
    let mut requirements = Vec::new();
    for declaration in evidence {
        if declaration.get("required").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let subject = declaration
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("required evidence declaration has no name"))?;
        let phase = phase_by_subject
            .get(subject)
            .copied()
            .ok_or_else(|| refused("required evidence has no explicit capture phase"))?;
        let format = declaration
            .get("format")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("required evidence declaration has no format"))?;
        let declared_source = declaration
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("required evidence declaration has no source"))?;
        let source = parse_manifest_source(declared_source)?;
        let expected = expected_source_for_subject(subject);
        if source != expected {
            return Err(refused(&format!(
                "required evidence {subject} declares source {declared_source}, which disagrees with its closed subject mapping"
            ))
            .into());
        }
        requirements.push(OperatorCaptureRequirement {
            subject: subject.to_string(),
            phase,
            source,
            media_type: media_type_for_format(format)?.to_string(),
        });
    }
    requirements.sort_by(|left, right| {
        left.phase
            .cmp(&right.phase)
            .then_with(|| left.subject.cmp(&right.subject))
    });
    Ok(requirements)
}

fn manifest_trial_is_structurally_inconclusive(manifest: &[u8], trial_id: &str) -> AnyResult<bool> {
    let value: Value = serde_json::from_slice(manifest)?;
    let trial = value
        .get("trials")
        .and_then(Value::as_array)
        .and_then(|trials| {
            trials
                .iter()
                .find(|trial| trial.get("id").and_then(Value::as_str) == Some(trial_id))
        })
        .ok_or_else(|| refused("manifest does not contain the requested trial ID"))?;
    let oracles = trial
        .get("oracles")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("trial has no oracle declarations"))?;
    Ok(oracles.iter().any(|oracle| {
        oracle.get("type").and_then(Value::as_str) == Some("structurally_inconclusive")
    }))
}

fn provider_route_map(
    manifest: &[u8],
    trial_id: &str,
    declarations: &[String],
    pass_capable: bool,
) -> AnyResult<BTreeMap<String, Vec<String>>> {
    let value: Value = serde_json::from_slice(manifest)?;
    let trial = value
        .get("trials")
        .and_then(Value::as_array)
        .and_then(|trials| {
            trials
                .iter()
                .find(|trial| trial.get("id").and_then(Value::as_str) == Some(trial_id))
        })
        .ok_or_else(|| refused("manifest does not contain the requested trial ID"))?;
    let expected_roles = trial
        .pointer("/job/provider_slots")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("trial has no provider role declarations"))?
        .iter()
        .map(|role| {
            role.as_str()
                .map(str::to_string)
                .ok_or_else(|| refused("provider role must be a string"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut routes = BTreeMap::<String, Vec<String>>::new();
    for declaration in declarations {
        let (role, route) = declaration
            .split_once('=')
            .ok_or_else(|| refused("--provider-route must use ROLE=ROUTE"))?;
        if !expected_roles.contains(role) || route.trim().is_empty() {
            return Err(refused("provider route uses an unknown role or empty route").into());
        }
        let role_routes = routes.entry(role.to_string()).or_default();
        if !role_routes.iter().any(|existing| existing == route) {
            role_routes.push(route.to_string());
        }
    }
    if pass_capable && routes.keys().collect::<BTreeSet<_>>() != expected_roles.iter().collect() {
        return Err(refused(
            "pass-capable provider routes must cover exactly the manifest provider roles",
        )
        .into());
    }
    let judge = routes
        .get("independent_judge")
        .into_iter()
        .flatten()
        .collect::<BTreeSet<_>>();
    let workers = routes
        .get("worker")
        .into_iter()
        .chain(routes.get("hostile_worker"))
        .flatten()
        .collect::<BTreeSet<_>>();
    if !judge.is_disjoint(&workers) {
        return Err(refused("independent judge routes must be disjoint from worker routes").into());
    }
    Ok(routes)
}

fn parse_manifest_source(value: &str) -> AnyResult<OperatorCaptureSource> {
    let source = match value {
        "job-view" => OperatorCaptureSource::JobView,
        "job-events" => OperatorCaptureSource::JobEvents,
        "job-intervention" => OperatorCaptureSource::JobIntervention,
        "job-cleanup" => OperatorCaptureSource::JobCleanup,
        "job" => OperatorCaptureSource::Job,
        "authority" => OperatorCaptureSource::Authority,
        "launch-plan" => OperatorCaptureSource::LaunchPlan,
        "lease" => OperatorCaptureSource::Lease,
        "job-report" => OperatorCaptureSource::JobReport,
        "receipt" => OperatorCaptureSource::Receipt,
        "supervised-child" => OperatorCaptureSource::SupervisedChild,
        "host-boot-id" => OperatorCaptureSource::HostBootId,
        "semantic-judgment" => OperatorCaptureSource::SemanticJudgment,
        "parent-repair-manifest" => OperatorCaptureSource::ParentRepairManifest,
        "parent-repair-candidate" => OperatorCaptureSource::ParentRepairCandidate,
        "doctor" => OperatorCaptureSource::Doctor,
        "supervisor-service-status" => OperatorCaptureSource::SupervisorServiceStatus,
        "parent-artifact" => OperatorCaptureSource::ParentArtifact,
        "parent-events" => OperatorCaptureSource::ParentEvents,
        "campaign" => OperatorCaptureSource::Campaign,
        "campaign-events" => OperatorCaptureSource::CampaignEvents,
        "active-plan" => OperatorCaptureSource::ActivePlan,
        "active-plan-events" => OperatorCaptureSource::ActivePlanEvents,
        "unavailable-objective" => OperatorCaptureSource::UnavailableObjective,
        _ => {
            return Err(refused(&format!("unknown canonical evidence source {value}")).into());
        }
    };
    Ok(source)
}

fn expected_source_for_subject(subject: &str) -> OperatorCaptureSource {
    if subject.starts_with("job-view") {
        OperatorCaptureSource::JobView
    } else if subject == "job-report" || subject.starts_with("job-report-") {
        OperatorCaptureSource::JobReport
    } else if subject.starts_with("events-") || subject.starts_with("job-events") {
        OperatorCaptureSource::JobEvents
    } else if subject.starts_with("job-intervention") || subject == "intervention" {
        OperatorCaptureSource::JobIntervention
    } else if subject.starts_with("job-cleanup") || subject == "cleanup" {
        OperatorCaptureSource::JobCleanup
    } else if subject.starts_with("lease-") || subject == "lease" {
        OperatorCaptureSource::Lease
    } else if subject.starts_with("supervised-child") {
        OperatorCaptureSource::SupervisedChild
    } else if subject.starts_with("host-boot") {
        OperatorCaptureSource::HostBootId
    } else if subject == "semantic-judgment" || subject.starts_with("semantic-judgment-") {
        OperatorCaptureSource::SemanticJudgment
    } else if subject.starts_with("parent-repair-candidate") {
        OperatorCaptureSource::ParentRepairCandidate
    } else if subject.starts_with("parent-repair") {
        OperatorCaptureSource::ParentRepairManifest
    } else if subject.starts_with("parent-artifact") {
        OperatorCaptureSource::ParentArtifact
    } else if subject.starts_with("parent-events") {
        OperatorCaptureSource::ParentEvents
    } else if subject.starts_with("campaign-events") {
        OperatorCaptureSource::CampaignEvents
    } else if subject.starts_with("campaign-") || subject == "campaign" {
        OperatorCaptureSource::Campaign
    } else if subject.starts_with("active-plan-events") {
        OperatorCaptureSource::ActivePlanEvents
    } else if subject.starts_with("active-plan") {
        OperatorCaptureSource::ActivePlan
    } else if subject == "doctor" || subject.starts_with("doctor-") {
        OperatorCaptureSource::Doctor
    } else if subject.starts_with("service-") || subject.starts_with("supervisor-service-status") {
        OperatorCaptureSource::SupervisorServiceStatus
    } else if subject.starts_with("authority") {
        OperatorCaptureSource::Authority
    } else if subject.starts_with("launch-plan") {
        OperatorCaptureSource::LaunchPlan
    } else if subject == "receipt" || subject.starts_with("receipt-") {
        OperatorCaptureSource::Receipt
    } else if subject == "job" || subject.starts_with("job-") {
        OperatorCaptureSource::Job
    } else {
        OperatorCaptureSource::UnavailableObjective
    }
}

fn media_type_for_format(format: &str) -> AnyResult<&'static str> {
    match format {
        "json" => Ok("application/json"),
        "jsonl" => Ok("application/x-ndjson"),
        "text" => Ok("text/plain; charset=utf-8"),
        _ => Err(refused(&format!("unsupported required evidence format {format}")).into()),
    }
}

fn validate_deadreckon_binary(
    current_exe: &Path,
    deadreckon_binary: &Path,
    expected_version: &str,
) -> AnyResult<Vec<u8>> {
    let current = canonical_regular_path(current_exe, "dr-capture executable")?;
    let deadreckon = canonical_regular_path(deadreckon_binary, "DeadReckon binary")?;
    if current.parent() != deadreckon.parent()
        || deadreckon.file_name().and_then(|name| name.to_str())
            != Some(if cfg!(windows) {
                "deadreckon.exe"
            } else {
                "deadreckon"
            })
    {
        return Err(refused("DeadReckon binary must be the sibling of dr-capture").into());
    }
    let output = Command::new(&deadreckon).arg("--version").output()?;
    let version = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !version.contains(expected_version) {
        return Err(refused("DeadReckon binary version does not match dr-capture").into());
    }
    stable_regular_bytes(&deadreckon, "DeadReckon binary")
}

fn trusted_deadreckon_binary(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<PathBuf> {
    validate_runtime_binding(context, binding)?;
    let name = if cfg!(windows) {
        "deadreckon.exe"
    } else {
        "deadreckon"
    };
    let candidate = context
        .current_exe
        .parent()
        .ok_or_else(|| refused("dr-capture executable has no sibling directory"))?
        .join(name);
    if fs::canonicalize(&candidate)?.to_string_lossy() != binding.deadreckon_binary {
        return Err(
            refused("DeadReckon binary path does not match the signed canonical binding").into(),
        );
    }
    let bytes = validate_deadreckon_binary(
        &context.current_exe,
        &candidate,
        &binding.deadreckon_version,
    )?;
    if sha256_bytes(&bytes) != binding.deadreckon_binary_sha256 {
        return Err(refused("DeadReckon binary digest does not match the signed binding").into());
    }
    Ok(candidate)
}

fn validate_runtime_binding(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<()> {
    let bytes = stable_regular_bytes(&context.current_exe, "dr-capture executable")?;
    if fs::canonicalize(&context.current_exe)?.to_string_lossy() != binding.capture_binary
        || sha256_bytes(&bytes) != binding.capture_binary_sha256
    {
        return Err(
            refused("dr-capture executable digest does not match the signed binding").into(),
        );
    }
    Ok(())
}

fn validate_bound_inputs(
    context: &HelperContext,
    binding: &OperatorCaptureBinding,
) -> AnyResult<()> {
    let inputs = [
        ("manifest.json", "bound manifest", &binding.manifest_sha256),
        (
            "result-schema.json",
            "bound result schema",
            &binding.result_schema_sha256,
        ),
        ("recorder.bin", "bound recorder", &binding.recorder_sha256),
        ("replay.json", "bound replay", &binding.replay_sha256),
    ];
    for (name, label, expected) in inputs {
        let bytes =
            stable_regular_bytes(&protected_input_path(&context.paths, binding, name), label)?;
        if sha256_bytes(&bytes) != *expected {
            return Err(refused(&format!("{label} does not match the signed binding")).into());
        }
    }
    let _ = trusted_deadreckon_binary(context, binding)?;
    let revision = clean_git_revision(&context.deadreckon_source_root)?;
    if revision != binding.deadreckon_source_revision {
        return Err(refused(
            "compiled DeadReckon source revision does not match the signed binding",
        )
        .into());
    }
    Ok(())
}

fn clean_git_revision(root: &Path) -> AnyResult<String> {
    let status = run_git(root, &["status", "--porcelain", "--untracked-files=all"])?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err(refused("compiled DeadReckon source checkout is not clean").into());
    }
    let revision = run_git(root, &["rev-parse", "--verify", "HEAD"])?;
    if !revision.status.success() {
        return Err(refused("could not resolve compiled DeadReckon source HEAD").into());
    }
    let revision = String::from_utf8(revision.stdout)?.trim().to_string();
    if revision.len() != 40
        || !revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(refused("DeadReckon source HEAD is not a full 40-hex revision").into());
    }
    Ok(revision)
}

fn stable_regular_bytes(path: &Path, label: &str) -> AnyResult<Vec<u8>> {
    const MAX_TRUSTED_FILE_BYTES: u64 = 256 * 1024 * 1024;
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(refused(&format!("{label} must be a regular non-symlink file")).into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if opened.len() > MAX_TRUSTED_FILE_BYTES {
        return Err(refused(&format!("{label} exceeds the trusted read size bound")).into());
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(MAX_TRUSTED_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_TRUSTED_FILE_BYTES {
        return Err(refused(&format!("{label} exceeds the trusted read size bound")).into());
    }
    let after = file.metadata()?;
    let post = fs::symlink_metadata(path)?;
    if !metadata_matches(&before, &opened)
        || !metadata_matches(&opened, &after)
        || !metadata_matches(&after, &post)
        || u64::try_from(bytes.len()).ok() != Some(after.len())
    {
        return Err(refused(&format!("{label} changed during trusted capture")).into());
    }
    Ok(bytes)
}

fn protected_input_path(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
    name: &str,
) -> PathBuf {
    paths
        .operator_capture_dir(binding.job_id.as_ref(), &binding.session_id)
        .join("inputs")
        .join(name)
}

fn protected_evidence_path(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
    subject: &str,
) -> PathBuf {
    let digest = sha256_bytes(subject.as_bytes());
    paths
        .operator_capture_dir(binding.job_id.as_ref(), &binding.session_id)
        .join("evidence")
        .join(format!("{}.bin", digest.trim_start_matches("sha256:")))
}

fn persist_bound_bytes(path: &Path, bytes: &[u8], label: &str) -> AnyResult<()> {
    persist_bytes_atomically_no_clobber(path, bytes, label)
}

fn persist_observation_no_clobber(path: &Path, bytes: &[u8]) -> AnyResult<()> {
    persist_bytes_atomically_no_clobber(path, bytes, "canonical observation output")
}

fn persist_bytes_atomically_no_clobber(path: &Path, bytes: &[u8], label: &str) -> AnyResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| refused(&format!("{label} path has no parent")))?;
    fs::create_dir_all(parent)?;
    let prefix = format!(
        ".{}.deadreckon-capture-",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact")
    );
    let mut temp = TempFileBuilder::new()
        .prefix(&prefix)
        .suffix(".tmp")
        .tempfile_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file_mut().sync_all()?;
    match temp.persist_noclobber(path) {
        Ok(file) => {
            file.sync_all()?;
            sync_capture_parent(parent)?;
            Ok(())
        }
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = stable_regular_bytes(path, label)?;
            if existing == bytes {
                Ok(())
            } else {
                Err(refused(&format!("{label} already exists with different bytes")).into())
            }
        }
        Err(error) => Err(error.error.into()),
    }
}

#[cfg(unix)]
fn sync_capture_parent(parent: &Path) -> AnyResult<()> {
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_capture_parent(_parent: &Path) -> AnyResult<()> {
    Ok(())
}

fn canonical_regular_path(path: &Path, label: &str) -> AnyResult<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(refused(&format!("{label} must be a regular non-symlink file")).into());
    }
    Ok(fs::canonicalize(path)?)
}

#[cfg(unix)]
fn metadata_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.file_type().is_file()
        && right.file_type().is_file()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn metadata_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.file_type().is_file()
        && right.file_type().is_file()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

fn required_text(value: &str, label: &str) -> AnyResult<String> {
    if value.trim().is_empty() {
        Err(refused(&format!("{label} must not be empty")).into())
    } else {
        Ok(value.to_string())
    }
}

fn objective_kind(source: OperatorCaptureSource) -> OperatorCaptureEventKind {
    match source {
        OperatorCaptureSource::JobIntervention
        | OperatorCaptureSource::SandboxBoundaryObservation
        | OperatorCaptureSource::CampaignIntervention => {
            OperatorCaptureEventKind::InterventionRecorded
        }
        OperatorCaptureSource::JobCleanup => OperatorCaptureEventKind::CleanupRecorded,
        _ => OperatorCaptureEventKind::EvidenceCaptured,
    }
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

fn refused(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::{TimeZone as _, Utc};
    use clap::Parser as _;
    use tempfile::TempDir;

    use deadreckon_protocol::{
        JobEventKind, JobId, OperatorCaptureReceipt, OperatorCaptureSchemaVersion,
        OperatorCaptureStatus,
    };

    use super::{
        Cli, expected_job_intervention_kind, job_intervention_is_fresh, manifest_requirements,
        persist_bound_bytes, persist_observation_no_clobber, provider_route_map, sha256_bytes,
        stable_regular_bytes, trusted_recorder_command, validate_published_envelope,
        validate_root_schema_contract,
    };

    #[test]
    fn observe_parser_has_no_arbitrary_path_argument() {
        let error = Cli::try_parse_from([
            "dr-capture",
            "observe",
            "--job-id",
            "job-1",
            "--session-id",
            "session-1",
            "--source",
            "job-view",
            "--subject",
            "job-view-before",
            "--event-id",
            "event-1",
            "--causation-id",
            "step-1",
            "--phase",
            "before",
            "--output",
            "/tmp/observed",
            "--file",
            "/tmp/forged",
        ])
        .expect_err("observe path refused");
        assert!(error.to_string().contains("unexpected argument '--file'"));
    }

    #[test]
    fn manifest_requirements_bind_trial_subjects_and_phases() {
        let manifest = br#"{
          "trials": [{
            "id": "trial-1",
            "evidence": [
              {"name":"job-view-before","source":"job-view","format":"json","required":true},
              {"name":"job-view-after","source":"job-view","format":"json","required":true},
              {"name":"optional","required":false}
            ],
            "capture_phases": {
              "before":["job-view-before"],
              "after":["job-view-after"]
            }
          }]
        }"#;
        let requirements = manifest_requirements(manifest, "trial-1").expect("requirements");
        assert_eq!(requirements.len(), 2);
        assert_eq!(requirements[0].subject, "job-view-before");
        assert_eq!(requirements[1].subject, "job-view-after");
    }

    #[cfg(unix)]
    #[test]
    fn trusted_file_reader_refuses_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        std::fs::write(&target, b"evidence").expect("target");
        symlink(&target, &link).expect("link");
        let error = stable_regular_bytes(&link, "evidence").expect_err("symlink refused");
        assert!(error.to_string().contains("non-symlink"));
    }

    #[test]
    fn intervention_kind_is_closed_per_trial() {
        assert_eq!(
            expected_job_intervention_kind("live_provider_worker_kill").expect("worker kind"),
            JobEventKind::AttemptStopped
        );
        assert_eq!(
            expected_job_intervention_kind("machine_reboot").expect("reboot kind"),
            JobEventKind::LeaseReclaimed
        );
        assert_eq!(
            expected_job_intervention_kind("live_provider_parent_repair").expect("repair kind"),
            JobEventKind::SemanticJudgeRevise
        );
        assert!(
            expected_job_intervention_kind("cross_provider_gate_attack")
                .expect_err("different source required")
                .to_string()
                .contains("different authoritative intervention source")
        );
    }

    #[test]
    fn stale_intervention_cannot_cross_a_new_before_boundary() {
        let boundary = Utc
            .with_ymd_and_hms(2026, 7, 30, 12, 0, 0)
            .single()
            .expect("boundary");
        assert!(!job_intervention_is_fresh(
            JobEventKind::AttemptStopped,
            boundary,
            JobEventKind::AttemptStopped,
            boundary,
        ));
        assert!(!job_intervention_is_fresh(
            JobEventKind::AttemptStopped,
            boundary - chrono::Duration::seconds(1),
            JobEventKind::AttemptStopped,
            boundary,
        ));
        assert!(job_intervention_is_fresh(
            JobEventKind::AttemptStopped,
            boundary + chrono::Duration::seconds(1),
            JobEventKind::AttemptStopped,
            boundary,
        ));
    }

    #[test]
    fn observation_output_is_no_clobber() {
        let temp = TempDir::new().expect("temp");
        let path = temp.path().join("evidence.json");
        persist_observation_no_clobber(&path, b"first").expect("first write");
        persist_observation_no_clobber(&path, b"first").expect("idempotent retry");
        let error = persist_observation_no_clobber(&path, b"second").expect_err("no replacement");
        assert!(error.to_string().contains("different bytes"));
        assert_eq!(std::fs::read(&path).expect("evidence"), b"first");
    }

    #[test]
    fn observation_copy_boundaries_converge_after_interruption() {
        let temp = TempDir::new().expect("temp");
        let protected = temp.path().join("protected.bin");
        let public = temp.path().join("public.json");
        let stranded_temp = temp
            .path()
            .join(".protected.bin.deadreckon-capture-dead.tmp");
        std::fs::write(&stranded_temp, b"partial").expect("stranded temp");
        persist_bound_bytes(&protected, b"exact", "protected").expect("protected first");
        persist_bound_bytes(&protected, b"exact", "protected").expect("protected retry");
        std::fs::write(&public, b"exact").expect("simulate crash after public copy");
        persist_observation_no_clobber(&public, b"exact").expect("public retry");
        assert_eq!(std::fs::read(&protected).expect("protected"), b"exact");
        assert_eq!(std::fs::read(&public).expect("public"), b"exact");
        assert!(
            persist_bound_bytes(&protected, b"mutated", "protected")
                .expect_err("protected mutation")
                .to_string()
                .contains("different bytes")
        );
        assert!(
            persist_observation_no_clobber(&public, b"mutated")
                .expect_err("public mutation")
                .to_string()
                .contains("different bytes")
        );
    }

    #[test]
    fn verify_envelope_rejects_extra_nested_capture_provenance() {
        let temp = TempDir::new().expect("temp");
        let path = temp.path().join("result.json");
        let evaluation = serde_json::json!({"status": "inconclusive"});
        let mut result = serde_json::to_vec_pretty(&evaluation).expect("evaluation");
        result.push(b'\n');
        let receipt = OperatorCaptureReceipt {
            schema_version: OperatorCaptureSchemaVersion::CURRENT,
            job_id: JobId::from("job-1"),
            session_id: "session-1".to_string(),
            binding_sha256: format!("sha256:{}", "a".repeat(64)),
            issued_at: Utc
                .with_ymd_and_hms(2026, 7, 30, 12, 0, 0)
                .single()
                .expect("timestamp"),
            event_count: 1,
            final_event_sha256: format!("sha256:{}", "b".repeat(64)),
            result_sha256: sha256_bytes(&result),
            result_bytes: u64::try_from(result.len()).expect("length"),
            completion_lineage: None,
            status: OperatorCaptureStatus::Inconclusive,
            signature: "c".repeat(64),
        };
        let receipt_sha256 = format!("sha256:{}", "d".repeat(64));
        let envelope = serde_json::json!({
            "schema_version": 2,
            "sanitized": true,
            "evaluation": evaluation,
            "evaluation_sha256": receipt.result_sha256,
            "capture_provenance": {
                "status": "verified",
                "receipt_sha256": receipt_sha256,
                "publication_proof": receipt.signature,
            },
        });
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&envelope).expect("envelope"),
        )
        .expect("write valid envelope");
        validate_published_envelope(&path, &result, &receipt, &receipt_sha256)
            .expect("valid verify envelope");

        let mut forged = envelope;
        forged
            .pointer_mut("/capture_provenance")
            .and_then(serde_json::Value::as_object_mut)
            .expect("provenance")
            .insert("forged".to_string(), serde_json::Value::Bool(true));
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&forged).expect("forged envelope"),
        )
        .expect("write forged envelope");
        let error = validate_published_envelope(&path, &result, &receipt, &receipt_sha256)
            .expect_err("nested extra field must fail verify");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn trusted_recorder_ignores_python_startup_injection() {
        let temp = TempDir::new().expect("temp");
        let sentinel = temp.path().join("sitecustomize-ran");
        let sitecustomize = temp.path().join("sitecustomize.py");
        let recorder = temp.path().join("recorder.py");
        let output = temp.path().join("recorder-output");
        std::fs::write(
            &sitecustomize,
            format!(
                "from pathlib import Path\nPath({:?}).write_text('injected')\n",
                sentinel
            ),
        )
        .expect("sitecustomize");
        std::fs::write(
            &recorder,
            "from pathlib import Path\nimport sys\nPath(sys.argv[1]).write_text('clean')\n",
        )
        .expect("recorder");
        let status = trusted_recorder_command(Path::new("python3"), &recorder)
            .arg(&output)
            .env("PYTHONPATH", temp.path())
            .env("PYTHONUSERBASE", temp.path())
            .status()
            .expect("python3");
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(output).expect("recorder output"),
            "clean"
        );
        assert!(!sentinel.exists(), "sitecustomize must not execute");
    }

    #[test]
    fn provider_routes_are_role_bound_complete_and_disjoint() {
        let manifest = br#"{
          "trials": [{
            "id": "trial-1",
            "job": {"provider_slots": ["worker", "independent_judge"]}
          }]
        }"#;
        let routes = provider_route_map(
            manifest,
            "trial-1",
            &[
                "worker=cli:worker-a".to_string(),
                "worker=cli:worker-b".to_string(),
                "independent_judge=cli:judge".to_string(),
            ],
            true,
        )
        .expect("role map");
        assert_eq!(
            routes["worker"],
            vec!["cli:worker-a".to_string(), "cli:worker-b".to_string()]
        );
        assert!(
            provider_route_map(
                manifest,
                "trial-1",
                &[
                    "worker=cli:same".to_string(),
                    "independent_judge=cli:same".to_string(),
                ],
                true,
            )
            .expect_err("same route across roles")
            .to_string()
            .contains("disjoint")
        );
        assert!(
            provider_route_map(
                manifest,
                "trial-1",
                &[
                    "worker=cli:worker".to_string(),
                    "independent_judge=cli:judge".to_string(),
                    "planner=cli:unused".to_string(),
                ],
                true,
            )
            .expect_err("unused role")
            .to_string()
            .contains("unknown role")
        );
    }

    #[test]
    fn bound_root_schema_rejects_omitted_and_extra_fields() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["trial_id", "backend"],
            "properties": {
                "trial_id": {"type": "string"},
                "backend": {"enum": ["sandbox-exec"]}
            },
            "additionalProperties": false
        });
        let missing = serde_json::json!({"trial_id": "trial"});
        assert!(
            validate_root_schema_contract(&missing, &schema)
                .expect_err("backend is required")
                .to_string()
                .contains("schema-required field backend")
        );
        let extra = serde_json::json!({
            "trial_id": "trial",
            "backend": "sandbox-exec",
            "forged": true
        });
        assert!(
            validate_root_schema_contract(&extra, &schema)
                .expect_err("extra field refused")
                .to_string()
                .contains("forbidden")
        );
        validate_root_schema_contract(
            &serde_json::json!({"trial_id": "trial", "backend": "sandbox-exec"}),
            &schema,
        )
        .expect("closed valid root");
    }
}
