use super::super::*;

pub(crate) async fn init_command(
    provider: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    max_spend: f64,
    sandbox: String,
    no_confirm: bool,
    no_completion: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    fs::create_dir_all(paths.home())?;
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let provider = match provider {
        Some(provider) => provider,
        None if no_confirm => preferred_init_subscription_cli_provider(&registry)
            .unwrap_or_else(|| "anthropic".to_string()),
        None => prompt_provider()?,
    };
    provider_setup_selection(
        &paths,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::ConfigDefault,
            explicit_provider: Some(&provider),
            explicit_model: None,
            config_default_provider: None,
            config_doc_provider: None,
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: Some("anthropic"),
            use_router_default: false,
            allow_auto_subscription: true,
            require_usable_route: false,
        },
    )?;
    let api_key = api_key.or_else(|| {
        if provider.starts_with("cli:") {
            None
        } else {
            prompt::open("provider API key (leave blank to use env var): ", None).ok()
        }
    });
    let config = init_config_text(
        &provider,
        api_key.as_deref(),
        base_url.as_deref(),
        max_spend,
        &sandbox,
    );
    fs::write(paths.config_path(), config)?;
    let provider_setup = provider_setup_selection(
        &paths,
        setup::ProviderSetupRequest {
            role: setup::SetupProviderRoleRef::ConfigDefault,
            explicit_provider: Some(&provider),
            explicit_model: None,
            config_default_provider: None,
            config_doc_provider: None,
            run_provider: None,
            auto_subscription_provider: None,
            built_in_default_provider: Some("anthropic"),
            use_router_default: false,
            allow_auto_subscription: true,
            require_usable_route: false,
        },
    )?;
    println!("{} {}", ui_ok("wrote"), paths.config_path().display());
    print_provider_setup_rows(&[provider_setup]);
    let doctor_summary = super::doctor::doctor_setup_summary().await?;
    let completion_status = if no_completion {
        "skipped by --no-completion".to_string()
    } else {
        super::completion::try_install_completion_after_init()
    };
    print!(
        "{}",
        init_completion_surface(
            &paths,
            &provider,
            &sandbox,
            &completion_status,
            &doctor_summary,
        )
        .render_plain(!completion_hints_enabled(false))
    );
    Ok(())
}

fn init_completion_surface(
    paths: &DeadreckonPaths,
    provider: &str,
    sandbox: &str,
    completion_status: &str,
    doctor_summary: &super::doctor::DoctorSetupSummary,
) -> VerdictSurface {
    let (kind, why, primary, secondary) = if doctor_summary.has_blocking_issues() {
        (
            VerdictKind::Blocked,
            "Initialization wrote the config, but setup verification found blocking issues; run doctor before starting managed work.",
            "deadreckon doctor",
            vec![("Secondary", "deadreckon run \"describe the coding goal\"")],
        )
    } else {
        (
            VerdictKind::Completed,
            "Initialization completed; the next step is to start a managed run from this workspace.",
            "deadreckon run \"describe the coding goal\"",
            vec![("Secondary", "deadreckon doctor")],
        )
    };
    VerdictSurface::try_new(
        kind,
        "init",
        None,
        ExplanationPanel::new(
            "DeadReckon wrote the local configuration and checked the provider setup.",
            why,
            vec![
                ("config", paths.config_path().display().to_string()),
                ("provider", provider.to_string()),
                ("sandbox", sandbox.to_string()),
                ("doctor", doctor_summary.evidence_detail()),
                ("completion", completion_status.to_string()),
            ],
        ),
        vec![("Recommended", primary)],
        secondary,
    )
    .expect("init verdict surface must be valid")
}

fn preferred_init_subscription_cli_provider(registry: &ProviderRegistry) -> Option<String> {
    registry
        .iter()
        .filter(|descriptor| {
            descriptor.kind == DescriptorKind::Cli
                && descriptor.subscription
                && descriptor
                    .default_binary
                    .as_deref()
                    .is_some_and(command_exists)
        })
        .map(|descriptor| descriptor.id.clone())
        .next()
}
