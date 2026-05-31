use super::super::*;
use std::path::Path;

pub(crate) async fn doctor_command(json_output: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let source = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if json_output {
        let sandbox_checks = deadreckon_sandbox::doctor()
            .into_iter()
            .map(|backend| {
                json!({
                    "backend": backend.backend,
                    "available": backend.available,
                    "path": backend.path,
                    "note": backend.note,
                })
            })
            .collect::<Vec<_>>();
        let seams = match doctor_seam_resolution(&paths, false) {
            Ok(lines) => json!({
                "status": "ok",
                "resolution": lines,
            }),
            Err(err) => json!({
                "status": "invalid",
                "error": err.to_string(),
            }),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "doctor",
                "id": &source,
                "status": "ok",
                "next_actions": ["deadreckon detect", "deadreckon providers list"],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "home": paths.home(),
                    "config": paths.config_path(),
                },
                "source": &source,
                "home": paths.home(),
                "config_path": paths.config_path(),
                "config_present": paths.config_path().exists(),
                "sandboxes": sandbox_checks,
                "seams": seams,
            }))?
        );
        return Ok(());
    }
    println!("{}", ui_heading("deadreckon doctor"));
    println!(
        "{} source {} | {} cd {}",
        ui_ok("✓"),
        source.display(),
        ui_command("try:"),
        source.display()
    );
    println!(
        "{} home {} | {} DEADRECKON_HOME={}",
        ui_ok("✓"),
        paths.home().display(),
        ui_command("try:"),
        paths.home().display()
    );
    for backend in deadreckon_sandbox::doctor() {
        if backend.available {
            let path = backend
                .path
                .as_ref()
                .map(|path| format!(" at {}", path.display()))
                .unwrap_or_default();
            let version = backend
                .path
                .as_ref()
                .and_then(|path| command_version(path))
                .map(|version| format!(" ({version})"))
                .unwrap_or_default();
            println!(
                "{} sandbox {} found{}{} | {} deadreckon run \"goal\" --sandbox {} --preview",
                ui_ok("✓"),
                backend.backend,
                path,
                version,
                ui_command("try:"),
                backend.backend
            );
        } else {
            println!("{} sandbox {} missing", ui_warn("✗"), backend.backend);
            println!("    {} {}", ui_command("fix:"), backend.note);
        }
    }
    if paths.config_path().exists() {
        match load_config_value(&paths) {
            Ok(root) => {
                println!(
                    "{} {} present and parseable | {} deadreckon config provider",
                    ui_ok("✓"),
                    paths.config_path().display(),
                    ui_command("try:")
                );
                doctor_providers(&paths, &root).await?;
            }
            Err(err) => {
                println!(
                    "{} {} is not parseable",
                    ui_warn("✗"),
                    paths.config_path().display()
                );
                println!(
                    "    {} check TOML syntax or rerun `deadreckon init` ({err})",
                    ui_command("fix:")
                );
            }
        }
    } else {
        println!("{} {} missing", ui_warn("✗"), paths.config_path().display());
        println!("    {} deadreckon init", ui_command("fix:"));
    }
    let defaults = config_defaults(&paths).unwrap_or_default();
    if defaults.provider.is_some() || paths.config_path().exists() {
        println!(
            "{} provider defaults configured | {} deadreckon config provider",
            ui_ok("✓"),
            ui_command("try:")
        );
    } else if command_exists("claude") || command_exists("codex") {
        println!(
            "{} cli subscription provider available | {} deadreckon init --no-confirm",
            ui_ok("✓"),
            ui_command("try:")
        );
    } else {
        println!("{} no provider configured", ui_warn("✗"));
        println!(
            "    {} deadreckon init or deadreckon config set providers.anthropic.api_key <KEY>",
            ui_command("fix:")
        );
    }
    doctor_seams(&paths);
    doctor_disk_and_permissions(&paths);
    doctor_os();
    doctor_sleep_prevention();
    doctor_subscription_binary("claude");
    doctor_subscription_binary("codex");
    Ok(())
}

fn doctor_seams(paths: &DeadreckonPaths) {
    println!("seams");
    match doctor_seam_resolution(paths, false) {
        Ok(lines) => {
            println!(
                "{} seams resolved | {} deadreckon run \"goal\" --no-seams",
                ui_ok("✓"),
                ui_command("try:")
            );
            for line in lines {
                println!("    {line}");
            }
        }
        Err(err) => {
            println!("{} seams config invalid", ui_warn("✗"));
            println!("    {} {err}", ui_command("fix:"));
            println!("    {} deadreckon doctor", ui_command("try:"));
        }
    }
}

fn doctor_seam_resolution(paths: &DeadreckonPaths, no_seams: bool) -> Result<Vec<String>> {
    let seams = read_seams_config(&paths.config_path(), no_seams)?;
    Ok(SeamKind::all()
        .into_iter()
        .map(|kind| {
            let fail = seam_fail_policy_label(kind);
            match seams.command_for(kind) {
                Some(command) => format!(
                    "{}: external command={} timeout_ms={} fail={fail}",
                    kind.config_key(),
                    seam_command_basename(&command.command),
                    command.timeout_ms
                ),
                None => format!("{}: builtin fail={fail}", kind.config_key()),
            }
        })
        .collect())
}

fn seam_command_basename(command: &[String]) -> String {
    command
        .first()
        .and_then(|argv0| Path::new(argv0).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("-")
        .to_string()
}

fn doctor_sleep_prevention() {
    let preview = sleep::preview(SleepPrefs::On, true);
    match preview.mode {
        sleep::SleepMode::Caffeinate | sleep::SleepMode::SystemdInhibit => {
            println!(
                "{} sleep prevention {} | {} deadreckon run \"goal\" --prevent-sleep auto",
                ui_ok("✓"),
                preview.label(),
                ui_command("try:")
            );
        }
        sleep::SleepMode::None => {
            println!(
                "{} sleep prevention disabled | {} deadreckon run \"goal\" --prevent-sleep on",
                ui_warn("✗"),
                ui_command("try:")
            );
        }
        sleep::SleepMode::Unsupported => {
            let fix = if cfg!(target_os = "linux") {
                "sudo apt install systemd"
            } else if cfg!(target_os = "macos") {
                "check /usr/bin/caffeinate"
            } else {
                "--prevent-sleep off (Windows native prevention is a V1 candidate)"
            };
            println!("{} sleep prevention unsupported", ui_warn("✗"));
            println!("    {} {fix}", ui_command("fix:"));
        }
    }
}

async fn doctor_providers(paths: &DeadreckonPaths, root: &toml::Value) -> Result<()> {
    let Some(providers) = root.get("providers").and_then(toml::Value::as_table) else {
        println!("{} providers table missing", ui_warn("✗"));
        println!("    {} deadreckon init", ui_command("fix:"));
        return Ok(());
    };
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    for (name, entry) in providers {
        let kind = entry
            .get("kind")
            .and_then(toml::Value::as_str)
            .unwrap_or(name);
        let kind_label = registry
            .get(name)
            .map(|descriptor| descriptor_kind_label(&descriptor.kind).to_string())
            .unwrap_or_else(|| config_provider_kind_label(kind).to_string());
        if kind.contains("cli") || name.starts_with("cli:") {
            let binary = entry
                .get("binary")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| {
                    if name.contains("claude") {
                        "claude"
                    } else {
                        "codex"
                    }
                });
            if command_exists(binary) || PathBuf::from(binary).exists() {
                println!(
                    "{} provider {name} kind={kind_label} CLI binary {binary} found | {} deadreckon run \"goal\" --provider {name} --preview",
                    ui_ok("✓"),
                    ui_command("try:")
                );
            } else {
                println!(
                    "{} provider {name} kind={kind_label} CLI binary {binary} missing",
                    ui_warn("✗")
                );
                println!(
                    "    {} install {binary} or set providers.\"{name}\".binary",
                    ui_command("fix:")
                );
            }
        } else if provider_has_key(entry) {
            if std::env::var_os("DEADRECKON_DOCTOR_PING").is_some() {
                doctor_provider_ping(paths, name, &kind_label).await?;
            } else {
                println!(
                    "{} provider {name} kind={kind_label} credential present; ping skipped | {} DEADRECKON_DOCTOR_PING=1 deadreckon doctor",
                    ui_ok("✓"),
                    ui_command("try:")
                );
            }
        } else {
            println!(
                "{} provider {name} kind={kind_label} credential missing",
                ui_warn("✗")
            );
            println!(
                "    {} deadreckon config set providers.{name}.api_key <KEY>",
                ui_command("fix:")
            );
        }
    }
    Ok(())
}

fn config_provider_kind_label(kind: &str) -> &'static str {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("cli") {
        "cli"
    } else if kind.contains("compatible") || kind.contains("local") {
        "local-http"
    } else if kind.contains("smoke") || kind.contains("script") {
        "scripted"
    } else if kind.contains("anthropic") || kind.contains("open-ai") || kind.contains("openai") {
        "http"
    } else {
        "custom"
    }
}

async fn doctor_provider_ping(paths: &DeadreckonPaths, name: &str, kind_label: &str) -> Result<()> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), Some(name))?;
    let request = ProviderRequest {
        prompt: "Reply with OK only.".to_string(),
        max_output_tokens: 8,
        cwd: None,
        output_path: None,
        sandbox_backend: None,
        pid_file: None,
        cancellation_token: None,
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        router.complete(&request),
    )
    .await
    {
        Ok(Ok(response)) => println!(
            "{} provider {name} kind={kind_label} ping ok model {} | {} deadreckon run \"goal\" --provider {name} --preview",
            ui_ok("✓"),
            response.model,
            ui_command("try:")
        ),
        Ok(Err(err)) => {
            println!(
                "{} provider {name} kind={kind_label} ping failed",
                ui_warn("✗")
            );
            println!(
                "    {} check credentials or set a fallback provider ({err})",
                ui_command("fix:")
            );
        }
        Err(_) => {
            println!(
                "{} provider {name} kind={kind_label} ping timed out",
                ui_warn("✗")
            );
            println!(
                "    {} check network/provider status or unset DEADRECKON_DOCTOR_PING",
                ui_command("fix:")
            );
        }
    }
    Ok(())
}

fn provider_has_key(entry: &toml::Value) -> bool {
    entry
        .get("api_key")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || entry
            .get("api_key_env")
            .and_then(toml::Value::as_str)
            .and_then(std::env::var_os)
            .is_some()
}

fn doctor_disk_and_permissions(paths: &DeadreckonPaths) {
    if let Err(err) = fs::create_dir_all(paths.runstate_dir()) {
        println!(
            "{} runstate dir {} not writable",
            ui_warn("✗"),
            paths.runstate_dir().display()
        );
        println!(
            "    {} mkdir -p {} && chmod u+w {}",
            ui_command("fix:"),
            paths.runstate_dir().display(),
            paths.runstate_dir().display()
        );
        println!("    detail: {err}");
        return;
    }
    let probe = paths.runstate_dir().join(".doctor-write-test");
    match fs::write(&probe, b"ok").and_then(|_| fs::remove_file(&probe)) {
        Ok(()) => println!(
            "{} runstate dir {} writable | {} deadreckon run \"goal\" --preview",
            ui_ok("✓"),
            paths.runstate_dir().display(),
            ui_command("try:")
        ),
        Err(err) => {
            println!(
                "{} runstate dir {} not writable",
                ui_warn("✗"),
                paths.runstate_dir().display()
            );
            println!(
                "    {} chmod u+w {}",
                ui_command("fix:"),
                paths.runstate_dir().display()
            );
            println!("    detail: {err}");
        }
    }
    match free_kb(paths.home()) {
        Some(kb) if kb < 1_048_576 => {
            println!(
                "{} disk space low: {} MB free in {}",
                ui_warn("✗"),
                kb / 1024,
                paths.home().display()
            );
            println!(
                "    {} free at least 1 GB or set DEADRECKON_HOME to a larger disk",
                ui_command("fix:")
            );
        }
        Some(kb) => println!(
            "{} disk space {} MB free in {} | {} deadreckon status",
            ui_ok("✓"),
            kb / 1024,
            paths.home().display(),
            ui_command("try:")
        ),
        None => {
            println!(
                "{} disk space check unavailable for {}",
                ui_warn("✗"),
                paths.home().display()
            );
            println!(
                "    {} run `df -Pk {}` manually",
                ui_command("fix:"),
                paths.home().display()
            );
        }
    }
}

fn doctor_os() {
    #[cfg(target_os = "macos")]
    {
        let version = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "{} os macOS {version} | {} sw_vers -productVersion",
            ui_ok("✓"),
            ui_command("try:")
        );
    }
    #[cfg(target_os = "linux")]
    {
        let version = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "{} os Linux kernel {version} | {} uname -r",
            ui_ok("✓"),
            ui_command("try:")
        );
    }
}

fn doctor_subscription_binary(binary: &str) {
    if command_exists(binary) {
        let provider = if binary == "claude" {
            "cli:claude-code"
        } else {
            "cli:codex"
        };
        println!(
            "{} subscription binary {binary} {} | {} deadreckon config provider {provider}",
            ui_ok("✓"),
            command_version(std::path::Path::new(binary))
                .unwrap_or_else(|| "version unknown".to_string()),
            ui_command("try:")
        );
    } else {
        println!("{} subscription binary {binary} missing", ui_warn("✗"));
        println!(
            "    {} install {binary} or choose another provider with `deadreckon config set defaults.provider <name>`",
            ui_command("fix:")
        );
    }
}

fn command_version(path: &std::path::Path) -> Option<String> {
    std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).to_string()
            } else {
                String::from_utf8_lossy(&output.stdout).to_string()
            };
            text.lines().next().unwrap_or_default().trim().to_string()
        })
        .filter(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_lists_seam_resolution() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        std::fs::create_dir_all(paths.home()).expect("home");
        std::fs::write(
            paths.config_path(),
            r#"
[seams.policy]
command = ["/bin/sh", "-c", "cat >/dev/null; echo '{\"decision\":\"allow\"}'"]
timeout_ms = 1234
"#,
        )
        .expect("config");

        let lines = doctor_seam_resolution(&paths, false).expect("seams");

        assert!(lines.iter().any(|line| {
            line.contains("policy: external")
                && line.contains("command=sh")
                && line.contains("timeout_ms=1234")
                && line.contains("fail=closed")
        }));
        assert!(
            lines
                .iter()
                .any(|line| line == "catalog: builtin fail=open")
        );
        assert!(lines.iter().any(|line| line == "hooks: builtin fail=safe"));
        assert!(
            lines
                .iter()
                .any(|line| line == "event_sink: builtin fail=safe")
        );
    }
}
