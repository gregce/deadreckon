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
    doctor_command(false).await?;
    if !no_completion {
        try_install_completion_after_init();
    }
    println!(
        "{} {}",
        ui_command("next:"),
        ui_command("deadreckon run \"describe the coding goal\"")
    );
    Ok(())
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
                    .is_some_and(start_command_exists)
        })
        .map(|descriptor| descriptor.id.clone())
        .next()
}
