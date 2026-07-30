use super::super::*;

const STEERABLE_PROVIDER_ROUTE: &str = "cli:codex-server";

pub(crate) fn steer_command(run_id: &str, text: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = super::reference::resolve_run_like(&paths, Some(run_id), "steer")?;
    super::graph_job::require_current_driver_for_job_owned_run(&paths, &state, "steer")?;
    let prefix = run_prefix(&state.run_id);
    queue_steer_for_state(&state, deadreckon_core::steer_inbox::SteerSource::Cli, text)?;

    println!("{} {}", ui_ok("queued steer for"), ui_id(&prefix));
    println!(
        "  {} delivery will begin on the active or next Codex turn",
        ui_muted("delivery:")
    );
    println!(
        "  {} {}",
        ui_muted("watch:"),
        ui_command(format!("deadreckon attach {prefix}"))
    );
    Ok(())
}

pub(crate) fn queue_steer_for_state(
    state: &deadreckon_core::PipelineState,
    source: deadreckon_core::steer_inbox::SteerSource,
    text: String,
) -> Result<deadreckon_core::steer_inbox::SteerInboxEntry> {
    let prefix = run_prefix(&state.run_id);
    if text.trim().is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "steer text must not be empty",
            "deadreckon steer <run-id> \"one concrete instruction\"",
        )));
    }

    if state.status != RunStatus::Executing {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "run {} is {} and cannot accept steering",
                state.run_id, state.status
            ),
            &format!(
                "deadreckon extend {prefix} {}",
                quoted_shell_argument(&text)
            ),
        )));
    }

    if state.provider.as_deref() != Some(STEERABLE_PROVIDER_ROUTE) {
        let provider = state.provider.as_deref().unwrap_or("no provider route");
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "run {} uses {provider}, which cannot accept live steering",
                state.run_id
            ),
            "deadreckon config provider cli:codex-server",
        )));
    }

    Ok(deadreckon_core::steer_inbox::append_steer(
        &state.run_root,
        source,
        text,
    )?)
}

fn quoted_shell_argument(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    #[test]
    fn steer_follow_up_hint_quotes_shell_metacharacters() {
        assert_eq!(
            super::quoted_shell_argument("use $HOME and `pwd` \"carefully\""),
            "\"use \\$HOME and \\`pwd\\` \\\"carefully\\\"\""
        );
    }
}
