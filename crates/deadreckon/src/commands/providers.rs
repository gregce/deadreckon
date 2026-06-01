use super::super::*;

pub(crate) async fn detect_command(
    id: Option<String>,
    json_output: bool,
    ping: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let options = ProviderProbeOptions { ping };
    let requested_id = id.clone();
    let results = if let Some(id) = id {
        let Some(descriptor) = registry.get(&id) else {
            let message = format!("no provider '{id}' in registry");
            return Err(CliError::Core(deadreckon_core::user_error(
                &message,
                "deadreckon providers list",
            )));
        };
        vec![descriptor.probe(options).await]
    } else {
        registry.probe_all(options).await
    };
    let surface = detect_verdict_surface(&results, requested_id.as_deref());
    if json_output {
        let primary_action = surface.primary_action.command.clone();
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(json!({
                "kind": "provider_detect",
                "id": requested_id.as_deref().unwrap_or("all"),
                "status": "ok",
                "next_actions": [primary_action],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "home": paths.home(),
                    "config": paths.config_path(),
                },
                "providers": results,
            })))?
        );
    } else {
        print_detect_results(&results);
        println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    }
    Ok(())
}

pub(crate) async fn providers_command(command: ProvidersCommand) -> Result<()> {
    match command {
        ProvidersCommand::List {
            models,
            all,
            full,
            json,
        } => providers_list_command(models, all, full, json).await,
    }
}

pub(crate) async fn update_command(
    check: bool,
    force: bool,
    allow_prerelease: bool,
    yes: bool,
    quiet: bool,
    plain: bool,
) -> Result<()> {
    ui::set_plain_output(plain);
    let paths = DeadreckonPaths::discover();
    let receipt = update_receipt_for_current_binary(&paths, !check)?;
    let current = update_current_version(&receipt);
    if check {
        let latest = resolve_latest_update(&paths, &current, allow_prerelease).await?;
        if !quiet {
            print_update_check(receipt.channel, &current, &latest);
        }
        return Ok(());
    }

    match receipt.channel {
        Channel::Npm | Channel::Brew | Channel::Cargo => {
            if !quiet {
                println!("channel: {}", receipt.channel.as_str());
                println!("current: {current}");
                println!("try: {}", channel_native_update_command(receipt.channel));
            }
            Ok(())
        }
        Channel::Source => Err(CliError::Core(DeadreckonError::InvalidInput(
            "update: channel = source; in-place swap not supported".to_string(),
        ))),
        Channel::Shell => {
            update_shell_channel(&paths, &receipt, force, allow_prerelease, yes, quiet).await
        }
    }
}

async fn update_shell_channel(
    paths: &DeadreckonPaths,
    receipt: &deadreckon_core::install_receipt::Receipt,
    force: bool,
    allow_prerelease: bool,
    yes: bool,
    quiet: bool,
) -> Result<()> {
    if receipt.channel != Channel::Shell {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "update: shell updater requires shell receipt, got {}",
            receipt.channel.as_str()
        ))));
    }
    let current = update_current_version(receipt);
    let latest = resolve_latest_update(paths, &current, allow_prerelease).await?;
    let backup_dir = unique_shell_backup_dir(&shell_backup_root(paths));
    if !quiet {
        print_shell_update_preview(&current, &latest, &backup_dir);
    }
    confirm_shell_update(yes)?;
    let backup_dir = create_shell_update_backup(paths, receipt, backup_dir)?;
    match run_shell_swap(receipt, force, allow_prerelease, quiet).await {
        Ok(()) => {
            prune_shell_backups(paths)?;
            if !quiet {
                println!("channel: shell");
                println!("current: {current}");
                println!("backup: {}", backup_dir.display());
                println!("updated: {}", receipt.binary_path.display());
                println!("try: deadreckon doctor");
            }
            Ok(())
        }
        Err(source) => Err(shell_update_failure(receipt, &backup_dir, &source)),
    }
}

fn create_shell_update_backup(
    paths: &DeadreckonPaths,
    receipt: &deadreckon_core::install_receipt::Receipt,
    backup_dir: PathBuf,
) -> Result<PathBuf> {
    let root = shell_backup_root(paths);
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&backup_dir)?;
    fs::copy(&receipt.binary_path, backup_dir.join("deadreckon"))?;
    fs::write(
        backup_dir.join("receipt.json"),
        serde_json::to_vec_pretty(receipt)?,
    )?;
    Ok(backup_dir)
}

fn print_shell_update_preview(current: &str, latest: &LatestUpdate, backup_dir: &Path) {
    println!("channel: shell");
    println!("current: {current}");
    println!("target: {}", latest.version);
    println!("archive: {}", latest.archive_url());
    println!("sha256: {}", latest.sha256());
    println!("backup: {}", backup_dir.display());
}

fn confirm_shell_update(yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive shell update requires --yes after reviewing preview",
            "deadreckon update --yes",
        )));
    }
    if prompt::confirm("apply this shell update?", false)? {
        Ok(())
    } else {
        Err(CliError::Core(DeadreckonError::InvalidInput(
            "update cancelled by user".to_string(),
        )))
    }
}

fn shell_backup_root(paths: &DeadreckonPaths) -> PathBuf {
    paths.home().join("update-backups")
}

fn unique_shell_backup_dir(root: &Path) -> PathBuf {
    let stamp = Utc::now().format("%Y%m%d%H%M%S%3f").to_string();
    let mut candidate = root.join(&stamp);
    let mut suffix = 1_u32;
    while candidate.exists() {
        candidate = root.join(format!("{stamp}-{suffix}"));
        suffix += 1;
    }
    candidate
}

async fn run_shell_swap(
    receipt: &deadreckon_core::install_receipt::Receipt,
    force: bool,
    allow_prerelease: bool,
    quiet: bool,
) -> std::result::Result<(), String> {
    if std::env::var_os("DEADRECKON_UPDATE_TEST_SHELL_FAIL").is_some() {
        return Err("test requested swap failure".to_string());
    }
    if let Ok(replacement) = std::env::var("DEADRECKON_UPDATE_TEST_SHELL_REPLACEMENT") {
        fs::copy(replacement, &receipt.binary_path).map_err(|err| err.to_string())?;
        return Ok(());
    }
    run_axoupdater_shell_update(receipt, force, allow_prerelease, quiet).await
}

#[cfg(feature = "selfupdate")]
async fn run_axoupdater_shell_update(
    receipt: &deadreckon_core::install_receipt::Receipt,
    force: bool,
    allow_prerelease: bool,
    quiet: bool,
) -> std::result::Result<(), String> {
    let mut updater = axoupdater::AxoUpdater::new_for("deadreckon");
    updater.set_release_source(axoupdater::ReleaseSource {
        release_type: axoupdater::ReleaseSourceType::GitHub,
        owner: "gdc".to_string(),
        name: "deadreckon".to_string(),
        app_name: "deadreckon".to_string(),
    });
    let version = update_current_version(receipt)
        .parse::<axoupdater::Version>()
        .map_err(|err| err.to_string())?;
    updater
        .set_current_version(version)
        .map_err(|err| err.to_string())?;
    if let Some(parent) = receipt.binary_path.parent() {
        updater.set_install_dir(parent.to_string_lossy().to_string());
    }
    if allow_prerelease {
        updater.configure_version_specifier(axoupdater::UpdateRequest::LatestMaybePrerelease);
    }
    if force {
        updater.always_update(true);
    }
    if quiet {
        updater.disable_installer_output();
    }
    updater.run().await.map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(not(feature = "selfupdate"))]
async fn run_axoupdater_shell_update(
    _receipt: &deadreckon_core::install_receipt::Receipt,
    _force: bool,
    _allow_prerelease: bool,
    _quiet: bool,
) -> std::result::Result<(), String> {
    Err("selfupdate feature is disabled".to_string())
}

fn prune_shell_backups(paths: &DeadreckonPaths) -> Result<()> {
    let root = shell_backup_root(paths);
    let mut backups = fs::read_dir(&root)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.path().is_dir().then_some(entry.path())
        })
        .collect::<Vec<_>>();
    backups.sort();
    let remove_count = backups.len().saturating_sub(3);
    for backup in backups.into_iter().take(remove_count) {
        fs::remove_dir_all(backup)?;
    }
    Ok(())
}

fn shell_update_failure(
    receipt: &deadreckon_core::install_receipt::Receipt,
    backup_dir: &Path,
    source: &str,
) -> CliError {
    CliError::Exit {
        code: 2,
        message: format!(
            "update: swap failed; prior binary preserved: {source}; backup {}",
            backup_dir.display()
        ),
        hint: format!(
            "try: cp {} {}",
            backup_dir.join("deadreckon").display(),
            receipt.binary_path.display()
        ),
    }
}

pub(crate) fn update_receipt_for_current_binary(
    paths: &DeadreckonPaths,
    persist_detected: bool,
) -> Result<deadreckon_core::install_receipt::Receipt> {
    if let Some(receipt) = read_receipt(paths)? {
        return Ok(receipt);
    }
    let binary = std::env::current_exe()?;
    let receipt = detect_receipt(&binary);
    if persist_detected {
        write_receipt(paths, &receipt)?;
    }
    Ok(receipt)
}

pub(crate) fn update_current_version(
    receipt: &deadreckon_core::install_receipt::Receipt,
) -> String {
    if receipt.channel_version.trim().is_empty() {
        env!("CARGO_PKG_VERSION").to_string()
    } else {
        receipt.channel_version.clone()
    }
}

fn print_update_check(channel: Channel, current: &str, latest: &LatestUpdate) {
    println!("channel: {}", channel.as_str());
    println!("current: {current}");
    println!("latest: {}", latest.version);
    println!("release: {}", latest.release_url);
    if latest.update_available && matches!(channel, Channel::Npm | Channel::Brew | Channel::Cargo) {
        println!("try: {}", channel_native_update_command(channel));
    } else if latest.update_available && channel == Channel::Shell {
        println!("try: deadreckon update");
    }
}

fn channel_native_update_command(channel: Channel) -> &'static str {
    match channel {
        Channel::Npm => "bun update -g deadreckon",
        Channel::Brew => "brew upgrade gdc/tap/deadreckon",
        Channel::Cargo => "cargo binstall --force deadreckon",
        Channel::Shell => "deadreckon update",
        Channel::Source => "cargo install --path crates/deadreckon",
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LatestUpdate {
    version: String,
    release_url: String,
    archive_url: Option<String>,
    sha256: Option<String>,
    update_available: bool,
}

impl LatestUpdate {
    fn archive_url(&self) -> String {
        self.archive_url.clone().unwrap_or_else(|| {
            format!(
                "{}/download/deadreckon-installer.sh",
                self.release_url.trim_end_matches('/')
            )
        })
    }

    fn sha256(&self) -> &str {
        self.sha256.as_deref().unwrap_or("see release checksums")
    }
}

pub(crate) async fn resolve_latest_update(
    paths: &DeadreckonPaths,
    current: &str,
    allow_prerelease: bool,
) -> Result<LatestUpdate> {
    let now = Utc::now();
    let cache = read_cache(paths)?;
    if let Some(cache) = cache.as_ref()
        && !cache.is_stale(now)
    {
        return Ok(LatestUpdate {
            version: cache.latest_version.clone(),
            release_url: cache.release_url.clone(),
            archive_url: None,
            sha256: None,
            update_available: cache.update_available,
        });
    }

    match fetch_latest_update(allow_prerelease).await {
        Ok(mut latest) => {
            latest.update_available = version_is_newer(current, &latest.version);
            write_cache(
                paths,
                &deadreckon_core::update_cache::Cache {
                    checked_at: now,
                    latest_version: latest.version.clone(),
                    current_version: current.to_string(),
                    release_url: latest.release_url.clone(),
                    update_available: latest.update_available,
                },
            )?;
            Ok(latest)
        }
        Err(_) => Ok(cache.map_or_else(
            || LatestUpdate {
                version: current.to_string(),
                release_url: "https://github.com/gdc/deadreckon/releases".to_string(),
                archive_url: None,
                sha256: None,
                update_available: false,
            },
            |cache| LatestUpdate {
                version: cache.latest_version,
                release_url: cache.release_url,
                archive_url: None,
                sha256: None,
                update_available: cache.update_available,
            },
        )),
    }
}

async fn fetch_latest_update(allow_prerelease: bool) -> std::result::Result<LatestUpdate, String> {
    if let Ok(delay_ms) = std::env::var("DEADRECKON_UPDATE_TEST_FETCH_DELAY_MS")
        && let Ok(delay_ms) = delay_ms.parse::<u64>()
    {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    if std::env::var_os("DEADRECKON_UPDATE_TEST_OFFLINE").is_some() {
        return Err("offline test mode".to_string());
    }
    if let Ok(version) = std::env::var("DEADRECKON_UPDATE_TEST_LATEST_VERSION") {
        let release_url =
            std::env::var("DEADRECKON_UPDATE_TEST_RELEASE_URL").unwrap_or_else(|_| {
                format!("https://github.com/gdc/deadreckon/releases/tag/v{version}")
            });
        return Ok(LatestUpdate {
            version,
            release_url,
            archive_url: std::env::var("DEADRECKON_UPDATE_TEST_ARCHIVE_URL").ok(),
            sha256: std::env::var("DEADRECKON_UPDATE_TEST_SHA256").ok(),
            update_available: false,
        });
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|err| err.to_string())?;
    if allow_prerelease {
        let url = std::env::var("DEADRECKON_UPDATE_RELEASES_URL")
            .unwrap_or_else(|_| "https://api.github.com/repos/gdc/deadreckon/releases".to_string());
        let releases = client
            .get(url)
            .header(reqwest::header::USER_AGENT, "deadreckon-update")
            .send()
            .await
            .map_err(|err| err.to_string())?
            .error_for_status()
            .map_err(|err| err.to_string())?
            .json::<Vec<GithubRelease>>()
            .await
            .map_err(|err| err.to_string())?;
        let Some(release) = releases.into_iter().next() else {
            return Err("no releases found".to_string());
        };
        return Ok(release.into_latest_update());
    }

    let url = std::env::var("DEADRECKON_UPDATE_RELEASES_URL").unwrap_or_else(|_| {
        "https://api.github.com/repos/gdc/deadreckon/releases/latest".to_string()
    });
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, "deadreckon-update")
        .send()
        .await
        .map_err(|err| err.to_string())?
        .error_for_status()
        .map_err(|err| err.to_string())?
        .json::<GithubRelease>()
        .await
        .map(GithubRelease::into_latest_update)
        .map_err(|err| err.to_string())
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

impl GithubRelease {
    fn into_latest_update(self) -> LatestUpdate {
        LatestUpdate {
            version: self.tag_name.trim_start_matches('v').to_string(),
            release_url: self.html_url,
            archive_url: None,
            sha256: None,
            update_available: false,
        }
    }
}

fn version_is_newer(current: &str, latest: &str) -> bool {
    let current = current.trim_start_matches('v');
    let latest = latest.trim_start_matches('v');
    match (
        semver::Version::parse(current),
        semver::Version::parse(latest),
    ) {
        (Ok(current), Ok(latest)) => latest > current,
        _ => latest != current,
    }
}

async fn providers_list_command(
    models: bool,
    all: bool,
    full: bool,
    json_output: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let active = read_config(&paths.config_path())?.default_provider;
    let ids = if all {
        registry.ids()
    } else {
        configured_provider_ids(&paths)?
    };
    if json_output {
        let mut results = Vec::new();
        let mut missing = Vec::new();
        for id in ids {
            if let Some(descriptor) = registry.get(&id) {
                results.push(descriptor.probe(ProviderProbeOptions { ping: false }).await);
            } else {
                missing.push(id);
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "providers",
                "id": if all { "all" } else { "configured" },
                "status": "ok",
                "next_actions": ["deadreckon detect"],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "home": paths.home(),
                    "config": paths.config_path(),
                },
                "providers": results,
                "missing_providers": missing,
                "active": active,
            }))?
        );
        return Ok(());
    }
    println!("{}", ui_heading("provider registry"));
    if ids.is_empty() {
        println!("{} no configured providers", ui_muted("-"));
        println!(
            "{} {}",
            ui_command("try:"),
            ui_command("deadreckon providers list --all")
        );
        return Ok(());
    }
    for id in ids {
        let Some(descriptor) = registry.get(&id) else {
            let marker = if active.as_deref() == Some(id.as_str()) {
                "*"
            } else {
                " "
            };
            println!(
                "{marker} {} {} not registered | {}",
                ui_warn("✗"),
                ui_id(&id),
                ui_command("deadreckon detect")
            );
            continue;
        };
        let result = descriptor.probe(ProviderProbeOptions { ping: false }).await;
        print_provider_list_row(&result, descriptor, active.as_deref(), full);
        if models {
            print_provider_models(descriptor);
        }
    }
    if !all {
        println!(
            "{} {}",
            ui_muted("hint:"),
            ui_command("deadreckon providers list --all")
        );
    }
    Ok(())
}

fn print_provider_list_row(
    result: &ProviderProbeResult,
    descriptor: &deadreckon_providers::registry::ProviderDescriptor,
    active: Option<&str>,
    full: bool,
) {
    let symbol = match result.status {
        ProbeStatus::Ok => ui_ok("✓"),
        ProbeStatus::Failed => ui::render(ui::Stream::Stdout, ui::Tone::Negative, "✗"),
        ProbeStatus::Skipped => ui_muted("-"),
    };
    let marker = if active == Some(result.id.as_str()) {
        "*"
    } else {
        " "
    };
    let location = result.location.as_deref().unwrap_or("-");
    let version = result.version.as_deref().unwrap_or("-");
    let model = descriptor.default_model.as_deref().unwrap_or("-");
    if full {
        println!(
            "{marker} {} {} kind={} credential={} model={} metering={} location={} version={}",
            symbol,
            ui_id(&result.id),
            descriptor_kind_label(&descriptor.kind),
            result.credential,
            model,
            result.metering,
            location,
            version
        );
    } else {
        println!(
            "{marker} {:<20} {}  kind={:<10} credential={:<8} model={} metering={} location={} version={}",
            ui_id(&result.id),
            symbol,
            descriptor_kind_label(&descriptor.kind),
            result.credential,
            model,
            result.metering,
            location,
            version
        );
    }
}

fn print_provider_models(descriptor: &deadreckon_providers::registry::ProviderDescriptor) {
    if descriptor.model_catalog.is_empty() {
        println!("    {}", ui_muted("models: none"));
        return;
    }
    println!("    {}", ui_muted("models:"));
    for model in &descriptor.model_catalog {
        let aliases = if model.aliases.is_empty() {
            "-".to_string()
        } else {
            model.aliases.join(",")
        };
        let context = model
            .context_window
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let price = match (model.input_per_million, model.output_per_million) {
            (Some(input), Some(output)) => format!("${input:.3}/${output:.3} per 1M"),
            _ => "-".to_string(),
        };
        println!(
            "      {} aliases={} context={} price={}",
            ui_id(&model.id),
            aliases,
            context,
            price
        );
    }
}

fn print_detect_results(results: &[ProviderProbeResult]) {
    println!("{}", ui_heading("provider detection"));
    for result in results {
        let symbol = match result.status {
            ProbeStatus::Ok => ui_ok("✓"),
            ProbeStatus::Failed => ui::render(ui::Stream::Stdout, ui::Tone::Negative, "✗"),
            ProbeStatus::Skipped => ui_muted("-"),
        };
        let location = result.location.as_deref().unwrap_or("-");
        let version = result.version.as_deref().unwrap_or("-");
        let message = result.message.as_deref().unwrap_or("");
        println!(
            "{:<20} {}  kind={:<10} credential={:<14} location={:<36} version={:<18} metering={}",
            ui_id(&result.id),
            symbol,
            descriptor_kind_label(&result.kind),
            result.credential,
            location,
            version,
            result.metering
        );
        if !message.is_empty() {
            println!("    {}", ui_muted(message));
        }
    }
}

fn detect_verdict_surface(
    results: &[ProviderProbeResult],
    requested_id: Option<&str>,
) -> VerdictSurface {
    let failed = results
        .iter()
        .filter(|result| result.status == ProbeStatus::Failed)
        .collect::<Vec<_>>();
    let skipped = results
        .iter()
        .filter(|result| result.status == ProbeStatus::Skipped)
        .count();
    let ok = results
        .iter()
        .filter(|result| result.status == ProbeStatus::Ok)
        .count();
    let subject = requested_id.unwrap_or("all");
    let kind = if failed.is_empty() {
        VerdictKind::Verified
    } else {
        VerdictKind::Blocked
    };
    let primary = failed
        .first()
        .and_then(|result| result.try_lines.first())
        .cloned()
        .or_else(|| {
            requested_id.map(|id| {
                if failed.is_empty() {
                    format!("deadreckon config provider {id}")
                } else {
                    "deadreckon providers list".to_string()
                }
            })
        })
        .unwrap_or_else(|| {
            if failed.is_empty() {
                "deadreckon providers list".to_string()
            } else {
                "deadreckon providers list --all".to_string()
            }
        });
    let what = if failed.is_empty() {
        match requested_id {
            Some(id) => format!("DeadReckon verified provider probe {id}."),
            None => "DeadReckon completed provider detection without failed probes.".to_string(),
        }
    } else if failed.len() == 1 {
        format!("DeadReckon found provider {} is not ready.", failed[0].id)
    } else {
        format!(
            "DeadReckon found {} provider probes are not ready.",
            failed.len()
        )
    };
    let why = if let Some(first) = failed.first() {
        first.message.clone().unwrap_or_else(|| {
            "A provider probe failed, so setup must be repaired before it can run work.".to_string()
        })
    } else {
        "All failed-provider checks passed; the selected provider catalog is ready for the next setup or run command.".to_string()
    };
    let mut evidence = vec![
        ("providers".to_string(), results.len().to_string()),
        ("ok".to_string(), ok.to_string()),
        ("failed".to_string(), failed.len().to_string()),
        ("skipped".to_string(), skipped.to_string()),
    ];
    if let Some(first) = failed.first() {
        evidence.push(("first failed".to_string(), first.id.clone()));
        evidence.push(("credential".to_string(), first.credential.clone()));
        if let Some(kind) = &first.error_kind {
            evidence.push(("error kind".to_string(), format!("{kind:?}")));
        }
    }
    let mut secondary = vec![
        (
            "Secondary".to_string(),
            "deadreckon providers list".to_string(),
        ),
        ("Secondary".to_string(), "deadreckon doctor".to_string()),
    ];
    for line in failed
        .iter()
        .flat_map(|result| result.try_lines.iter())
        .filter(|line| line.as_str() != primary)
    {
        secondary.push(("Secondary".to_string(), line.clone()));
    }
    VerdictSurface::try_new(
        kind,
        "detect",
        Some(subject),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        secondary
            .iter()
            .map(|(label, command)| (label.as_str(), command.as_str()))
            .collect::<Vec<_>>(),
    )
    .expect("detect verdict surface must be valid")
}
