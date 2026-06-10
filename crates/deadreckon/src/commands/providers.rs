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
                println!(
                    "{}",
                    update_native_surface(receipt.channel, &current)
                        .render_plain(!completion_hints_enabled(false))
                );
            }
            Ok(())
        }
        Channel::Source => Err(CliError::Surface {
            code: 1,
            surface: update_source_surface(&current).render_plain(!completion_hints_enabled(false)),
        }),
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
    // When the newest release is itself a prerelease (the RC era has no
    // stable), the updater must be allowed to install it without requiring
    // --pre — otherwise update reports nothing to do while the check above
    // names a newer version.
    let effective_prerelease = allow_prerelease || latest.version.contains('-');
    let backup_dir = unique_shell_backup_dir(&shell_backup_root(paths));
    confirm_shell_update(yes, &current, &latest, receipt)?;
    let backup_dir = create_shell_update_backup(paths, receipt, backup_dir)?;
    match run_shell_swap(receipt, force, effective_prerelease, quiet).await {
        Ok(()) => {
            prune_shell_backups(paths)?;
            if !quiet {
                println!(
                    "{}",
                    update_shell_completed_surface(&current, &latest, &backup_dir, receipt)
                        .render_plain(!completion_hints_enabled(false))
                );
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

fn update_native_surface(channel: Channel, current: &str) -> VerdictSurface {
    let primary = channel_native_update_command(channel);
    VerdictSurface::try_new(
        VerdictKind::Blocked,
        "update",
        Some(channel.as_str()),
        ExplanationPanel::new(
            format!(
                "DeadReckon detected this binary is managed by the {} channel.",
                channel.as_str()
            ),
            "DeadReckon cannot safely replace a package-manager-owned binary directly; run the native package-manager update command.",
            vec![
                ("channel", channel.as_str().to_string()),
                ("current", current.to_string()),
                ("managed by", channel.as_str().to_string()),
            ],
        ),
        vec![("Recommended", primary)],
        vec![("Secondary", "deadreckon update --check")],
    )
    .expect("native update verdict surface must be valid")
}

fn update_shell_completed_surface(
    current: &str,
    latest: &LatestUpdate,
    backup_dir: &Path,
    receipt: &deadreckon_core::install_receipt::Receipt,
) -> VerdictSurface {
    VerdictSurface::try_new(
        VerdictKind::Completed,
        "update",
        Some("shell"),
        ExplanationPanel::new(
            "DeadReckon replaced the shell-installed binary and preserved a rollback backup.",
            "The update completed; doctor is the safest next command to verify the new binary and provider setup.",
            vec![
                ("channel", "shell".to_string()),
                ("current", current.to_string()),
                ("target", latest.version.clone()),
                ("archive", latest.archive_url()),
                ("sha256", latest.sha256().to_string()),
                ("backup", backup_dir.display().to_string()),
                ("updated", receipt.binary_path.display().to_string()),
            ],
        ),
        vec![("Recommended", "deadreckon doctor")],
        vec![("Secondary", "deadreckon update --check")],
    )
    .expect("shell update verdict surface must be valid")
}

fn confirm_shell_update(
    yes: bool,
    current: &str,
    latest: &LatestUpdate,
    receipt: &deadreckon_core::install_receipt::Receipt,
) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(CliError::Surface {
            code: 1,
            surface: update_shell_requires_yes_surface(current, latest, receipt)
                .render_plain(!completion_hints_enabled(false)),
        });
    }
    if prompt::confirm("apply this shell update?", false)? {
        Ok(())
    } else {
        Err(CliError::Core(DeadreckonError::InvalidInput(
            "update cancelled by user".to_string(),
        )))
    }
}

fn update_shell_requires_yes_surface(
    current: &str,
    latest: &LatestUpdate,
    receipt: &deadreckon_core::install_receipt::Receipt,
) -> VerdictSurface {
    VerdictSurface::try_new(
        VerdictKind::Blocked,
        "update",
        Some("shell"),
        ExplanationPanel::new(
            "DeadReckon did not replace the shell-installed binary because confirmation is required.",
            "This shell update can mutate the active binary, so non-interactive execution must pass --yes after reviewing the target release.",
            vec![
                ("channel".to_string(), "shell".to_string()),
                ("current".to_string(), current.to_string()),
                ("target".to_string(), latest.version.clone()),
                ("archive".to_string(), latest.archive_url()),
                ("updated".to_string(), receipt.binary_path.display().to_string()),
            ],
        ),
        vec![("Recommended", "deadreckon update --yes")],
        vec![("Secondary", "deadreckon update --check")],
    )
    .expect("shell update --yes refusal verdict surface must be valid")
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
        owner: "gregce".to_string(),
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
    let primary = format!(
        "cp {} {}",
        backup_dir.join("deadreckon").display(),
        receipt.binary_path.display()
    );
    CliError::Surface {
        code: 2,
        surface: VerdictSurface::try_new(
            VerdictKind::Failed,
            "update",
            Some("shell"),
            ExplanationPanel::new(
                "DeadReckon could not replace the shell-installed binary.",
                "The update failed after creating a rollback backup, so restoring the preserved binary is the safest next step.",
                vec![
                    ("channel".to_string(), "shell".to_string()),
                    ("source".to_string(), source.to_string()),
                    ("backup".to_string(), backup_dir.display().to_string()),
                    ("updated".to_string(), receipt.binary_path.display().to_string()),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", "deadreckon update --check")],
        )
        .expect("shell update failure verdict surface must be valid")
        .render_plain(!completion_hints_enabled(false)),
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
    println!(
        "{}",
        update_check_surface(channel, current, latest)
            .render_plain(!completion_hints_enabled(false))
    );
}

fn update_check_surface(channel: Channel, current: &str, latest: &LatestUpdate) -> VerdictSurface {
    let evidence = vec![
        ("channel", channel.as_str().to_string()),
        ("current", current.to_string()),
        ("latest", latest.version.clone()),
        ("release", latest.release_url.clone()),
    ];
    if latest.update_available {
        return VerdictSurface::try_new(
            VerdictKind::Preview,
            "update",
            Some(channel.as_str()),
            ExplanationPanel::new(
                "DeadReckon found a newer release for this install channel.",
                "The check is read-only; run the channel update command to apply the available release.",
                evidence,
            ),
            vec![("Recommended", channel_native_update_command(channel))],
            vec![("Secondary", "deadreckon doctor")],
        )
        .expect("update-check preview verdict surface must be valid");
    }

    VerdictSurface::try_new(
        VerdictKind::Noop,
        "update",
        Some(channel.as_str()),
        ExplanationPanel::new(
            "DeadReckon did not find a newer release for this install channel.",
            "The installed version is already current, or the check fell back to cached/current release metadata.",
            evidence,
        ),
        vec![("Recommended", "deadreckon doctor")],
        Vec::<(&str, &str)>::new(),
    )
    .expect("update-check no-op verdict surface must be valid")
}

fn update_source_surface(current: &str) -> VerdictSurface {
    VerdictSurface::try_new(
        VerdictKind::Blocked,
        "update",
        Some("source"),
        ExplanationPanel::new(
            "DeadReckon cannot replace a source-built binary in place.",
            "Source installs do not have a managed package channel for self-update; reinstall from the checkout so Cargo rebuilds the binary.",
            vec![
                ("channel".to_string(), "source".to_string()),
                ("current".to_string(), current.to_string()),
                ("managed swap".to_string(), "unsupported".to_string()),
            ],
        ),
        vec![("Recommended", "cargo install --path crates/deadreckon")],
        vec![("Secondary", "deadreckon update --check")],
    )
    .expect("source update verdict surface must be valid")
}

fn channel_native_update_command(channel: Channel) -> &'static str {
    match channel {
        Channel::Npm => "bun update -g deadreckon",
        Channel::Brew => "brew upgrade gregce/tap/deadreckon",
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
            // release_url is the human tag page
            // (…/releases/tag/vX); assets live under
            // …/releases/download/vX/<asset>.
            let download_base = self
                .release_url
                .trim_end_matches('/')
                .replace("/releases/tag/", "/releases/download/");
            format!("{download_base}/deadreckon-installer.sh")
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
                release_url: "https://github.com/gregce/deadreckon/releases".to_string(),
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
                format!("https://github.com/gregce/deadreckon/releases/tag/v{version}")
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
        return fetch_newest_any_release(&client).await;
    }

    let url = std::env::var("DEADRECKON_UPDATE_RELEASES_URL").unwrap_or_else(|_| {
        "https://api.github.com/repos/gregce/deadreckon/releases/latest".to_string()
    });
    let stable = client
        .get(url)
        .header(reqwest::header::USER_AGENT, "deadreckon-update")
        .send()
        .await
        .map_err(|err| err.to_string())
        .and_then(|response| response.error_for_status().map_err(|err| err.to_string()));
    match stable {
        Ok(response) => response
            .json::<GithubRelease>()
            .await
            .map(GithubRelease::into_latest_update)
            .map_err(|err| err.to_string()),
        // The release-candidate era has no stable release, so releases/latest
        // 404s. Mirror the installer: fall back to the newest release of any
        // kind instead of silently reporting "up to date".
        Err(_) => fetch_newest_any_release(&client).await,
    }
}

async fn fetch_newest_any_release(
    client: &reqwest::Client,
) -> std::result::Result<LatestUpdate, String> {
    let url = std::env::var("DEADRECKON_UPDATE_RELEASES_URL")
        .unwrap_or_else(|_| "https://api.github.com/repos/gregce/deadreckon/releases".to_string());
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
    Ok(release.into_latest_update())
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
    let mut results = Vec::new();
    let mut missing = Vec::new();
    for id in ids {
        if let Some(descriptor) = registry.get(&id) {
            results.push(descriptor.probe(ProviderProbeOptions { ping: false }).await);
        } else {
            missing.push(id);
        }
    }
    let surface = providers_list_verdict_surface(&results, &missing, all);
    if json_output {
        let primary_action = surface.primary_action.command.clone();
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(json!({
                "kind": "providers",
                "id": if all { "all" } else { "configured" },
                "status": "ok",
                "next_actions": [primary_action],
                "try_lines": Vec::<String>::new(),
                "paths": {
                    "home": paths.home(),
                    "config": paths.config_path(),
                },
                "providers": results,
                "missing_providers": missing,
                "active": active,
            })))?
        );
        return Ok(());
    }
    println!("{}", ui_heading("provider registry"));
    if results.is_empty() && missing.is_empty() {
        println!("{} no configured providers", ui_muted("-"));
        println!("{}", surface.render_plain(!completion_hints_enabled(false)));
        return Ok(());
    }
    for id in &missing {
        let marker = if active.as_deref() == Some(id.as_str()) {
            "*"
        } else {
            " "
        };
        println!(
            "{marker} {} {} {} | {}",
            ui_warn("✗"),
            ui_id(id),
            ui_warn("not registered"),
            ui_command("deadreckon providers list --all")
        );
    }
    for result in &results {
        let Some(descriptor) = registry.get(&result.id) else {
            continue;
        };
        print_provider_list_row(&result, descriptor, active.as_deref(), full);
        if models {
            print_provider_models(descriptor);
        }
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn providers_list_verdict_surface(
    results: &[ProviderProbeResult],
    missing: &[String],
    all: bool,
) -> VerdictSurface {
    let failed = results
        .iter()
        .filter(|result| result.status == ProbeStatus::Failed)
        .collect::<Vec<_>>();
    let ok = results
        .iter()
        .filter(|result| result.status == ProbeStatus::Ok)
        .count();
    let skipped = results
        .iter()
        .filter(|result| result.status == ProbeStatus::Skipped)
        .count();
    let subject = if all { "all" } else { "configured" };
    let kind = if results.is_empty() && missing.is_empty() {
        VerdictKind::Noop
    } else if missing.is_empty() && failed.is_empty() {
        VerdictKind::Verified
    } else {
        VerdictKind::Blocked
    };
    let primary = if let Some(first) = failed.first() {
        format!("deadreckon detect {}", first.id)
    } else if !missing.is_empty() || results.is_empty() {
        "deadreckon providers list --all".to_string()
    } else {
        "deadreckon detect".to_string()
    };
    let what = if results.is_empty() && missing.is_empty() {
        "DeadReckon found no configured provider routes.".to_string()
    } else if missing.is_empty() && failed.is_empty() {
        format!("DeadReckon listed {ok} usable provider route(s).")
    } else {
        format!(
            "DeadReckon listed provider routes and found {} issue(s).",
            failed.len() + missing.len()
        )
    };
    let why = if let Some(first) = failed.first() {
        format!(
            "{} is not ready; inspect that provider before using it for a run.",
            first.id
        )
    } else if !missing.is_empty() {
        "At least one configured provider is not registered; list all routes or change the configured provider."
            .to_string()
    } else if results.is_empty() {
        "There is no configured provider inventory yet; listing all routes is the safest discovery step."
            .to_string()
    } else {
        "The configured provider inventory is readable; a detect pass can refresh setup diagnostics."
            .to_string()
    };
    let mut evidence = vec![
        ("scope".to_string(), subject.to_string()),
        ("providers".to_string(), results.len().to_string()),
        ("ok".to_string(), ok.to_string()),
        ("failed".to_string(), failed.len().to_string()),
        ("skipped".to_string(), skipped.to_string()),
        ("missing".to_string(), missing.len().to_string()),
    ];
    if let Some(first) = failed.first() {
        evidence.push(("first failed".to_string(), first.id.clone()));
        if let Some(message) = &first.message {
            evidence.push(("reason".to_string(), message.clone()));
        }
    }
    if let Some(first) = missing.first() {
        evidence.push(("first missing".to_string(), first.clone()));
    }
    VerdictSurface::try_new(
        kind,
        "providers",
        Some(subject),
        ExplanationPanel::new(what, why, evidence),
        vec![("Recommended", primary.as_str())],
        vec![
            ("Secondary", "deadreckon detect"),
            ("Secondary", "deadreckon providers list --all"),
            ("Secondary", "deadreckon config provider cli:codex"),
        ],
    )
    .expect("providers list verdict surface must be valid")
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
            ui_id(&result.id),
            symbol,
            descriptor_kind_label(&descriptor.kind),
            result.credential,
            model,
            result.metering,
            location,
            version
        );
    } else {
        println!(
            "{marker} {} {}  kind={:<10} credential={:<8} model={} metering={} location={} version={}",
            crate::ui::pad_visible(&ui_id(&result.id), 20),
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
            "{} {}  kind={:<10} credential={:<14} location={:<36} version={:<18} metering={}",
            crate::ui::pad_visible(&ui_id(&result.id), 20),
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;
    use deadreckon_core::install_receipt::{Channel, INSTALL_RECEIPT_VERSION, Receipt};

    use super::*;

    #[test]
    fn shell_update_failure_surface_uses_single_recommended_restore_command() {
        let receipt = Receipt {
            channel: Channel::Shell,
            channel_version: "0.1.0".to_string(),
            binary_path: PathBuf::from("/usr/local/bin/deadreckon"),
            installed_at: Utc::now(),
            install_source: Some("https://example.test/deadreckon".to_string()),
            platform_package: None,
            receipt_version: INSTALL_RECEIPT_VERSION,
        };

        match shell_update_failure(&receipt, PathBuf::from("/tmp/dr-backup").as_path(), "boom") {
            CliError::Surface { code, surface } => {
                assert_eq!(code, 2);
                assert!(
                    !surface.contains("try:"),
                    "verdict surface should not embed try footers: {surface}"
                );
                assert_eq!(surface.matches("\nRecommended\n").count(), 1, "{surface}");
                assert!(
                    surface.contains(
                        "Recommended\ncp /tmp/dr-backup/deadreckon /usr/local/bin/deadreckon"
                    ),
                    "{surface}"
                );
                assert_eq!(surface.matches("\nSecondary\n").count(), 1, "{surface}");
            }
            other => panic!("expected verdict surface error, got {other:?}"),
        }
    }
}

/// `deadreckon models [PROVIDER]` — the catalog surface for choosing a model
/// before any launch. Lists each route's selectable model ids with context,
/// cost, the descriptor's recommended entry, and the configured default.
pub(crate) fn models_command(provider: Option<&str>, all: bool, json: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let defaults = config_defaults(&paths)?;
    let configured_model = configured_default_model(&paths)?;
    let descriptors: Vec<&deadreckon_providers::registry::ProviderDescriptor> = match provider {
        Some(name) => {
            let Some(descriptor) = registry.get(name) else {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("no provider '{name}' in registry"),
                    "deadreckon providers list --all",
                )));
            };
            vec![descriptor]
        }
        None => registry
            .iter()
            .filter(|descriptor| descriptor.id != "smoke")
            .filter(|descriptor| all || descriptor_route_has_credential(descriptor))
            .collect(),
    };
    if descriptors.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "no providers configured",
            "deadreckon init",
        )));
    }

    if json {
        let providers = descriptors
            .iter()
            .map(|descriptor| models_provider_json(descriptor, &defaults, &configured_model))
            .collect::<Vec<_>>();
        let value = match provider {
            Some(_) => providers
                .into_iter()
                .next()
                .unwrap_or(serde_json::json!({})),
            None => serde_json::json!({ "providers": providers }),
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    for descriptor in &descriptors {
        println!(
            "{} {}",
            ui_heading(&descriptor.id),
            ui_muted(&descriptor.display_name)
        );
        let rows = descriptor
            .model_catalog
            .iter()
            .map(|entry| {
                vec![
                    entry.id.clone(),
                    entry
                        .context_window
                        .map(|window| format!("{}k", window / 1000))
                        .unwrap_or_else(|| "-".to_string()),
                    model_cost_label(entry),
                    model_notes_label(entry, descriptor, &defaults, &configured_model),
                ]
            })
            .collect::<Vec<_>>();
        println!("{}", columns(&["model", "context", "cost", "notes"], &rows));
        println!();
    }
    println!(
        "  {} {}",
        ui_muted("set a default:"),
        ui_command("deadreckon config model <id>")
    );
    println!(
        "  {} {}",
        ui_muted("one run only:  "),
        ui_command("deadreckon run \"goal\" --model <id>")
    );
    Ok(())
}

fn configured_default_model(paths: &DeadreckonPaths) -> Result<Option<String>> {
    let root = load_config_value(paths)?;
    Ok(get_toml_path(&root, "defaults.model")
        .and_then(toml::Value::as_str)
        .map(ToString::to_string))
}

fn descriptor_route_has_credential(
    descriptor: &deadreckon_providers::registry::ProviderDescriptor,
) -> bool {
    use deadreckon_providers::registry::DescriptorKind;
    match descriptor.kind {
        DescriptorKind::Cli => descriptor
            .default_binary
            .as_deref()
            .is_some_and(command_exists),
        DescriptorKind::Http => descriptor
            .auth
            .env_var
            .as_deref()
            .is_some_and(|var| std::env::var_os(var).is_some()),
        DescriptorKind::LocalHttp | DescriptorKind::Scripted => true,
    }
}

fn model_is_configured_default(
    entry: &deadreckon_providers::ModelEntry,
    descriptor: &deadreckon_providers::registry::ProviderDescriptor,
    defaults: &ConfigDefaults,
    configured_model: &Option<String>,
) -> bool {
    let provider_is_default = defaults.provider.as_deref() == Some(descriptor.id.as_str());
    provider_is_default && configured_model.as_deref() == Some(entry.id.as_str())
}

fn model_cost_label(entry: &deadreckon_providers::ModelEntry) -> String {
    match (entry.input_per_million, entry.output_per_million) {
        (Some(input), Some(output)) if input > 0.0 || output > 0.0 => {
            format!("${input}/{output} per M")
        }
        _ => "-".to_string(),
    }
}

fn model_notes_label(
    entry: &deadreckon_providers::ModelEntry,
    descriptor: &deadreckon_providers::registry::ProviderDescriptor,
    defaults: &ConfigDefaults,
    configured_model: &Option<String>,
) -> String {
    let mut notes = Vec::new();
    if model_is_configured_default(entry, descriptor, defaults, configured_model) {
        notes.push("default".to_string());
    }
    if entry.recommended {
        notes.push("recommended".to_string());
    }
    if !entry.aliases.is_empty() {
        notes.push(format!("aka {}", entry.aliases.join(", ")));
    }
    if notes.is_empty() {
        "-".to_string()
    } else {
        notes.join(" · ")
    }
}

fn models_provider_json(
    descriptor: &deadreckon_providers::registry::ProviderDescriptor,
    defaults: &ConfigDefaults,
    configured_model: &Option<String>,
) -> serde_json::Value {
    serde_json::json!({
        "provider": descriptor.id,
        "display_name": descriptor.display_name,
        "configured_default": configured_model
            .as_deref()
            .filter(|_| defaults.provider.as_deref() == Some(descriptor.id.as_str())),
        "models": descriptor.model_catalog.iter().map(|entry| serde_json::json!({
            "id": entry.id,
            "recommended": entry.recommended,
            "context_window": entry.context_window,
            "aliases": entry.aliases,
            "default": model_is_configured_default(entry, descriptor, defaults, configured_model),
        })).collect::<Vec<_>>(),
    })
}
