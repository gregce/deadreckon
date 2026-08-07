use super::super::*;

pub(crate) fn steer_command(run_id: &str, text: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = super::reference::resolve_run_like(&paths, Some(run_id), "steer")?;
    let prefix = run_prefix(&state.run_id);
    queue_steer_for_state(
        &paths,
        &state,
        deadreckon_core::steer_inbox::SteerSource::Cli,
        text,
    )?;

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
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    source: deadreckon_core::steer_inbox::SteerSource,
    text: String,
) -> Result<deadreckon_core::steer_inbox::SteerInboxEntry> {
    if paths.job_json(&state.run_id).is_file() {
        let job = deadreckon_core::load_job(paths, &state.run_id)?;
        super::graph_job::require_current_driver_for_job_artifact(
            paths,
            &state.run_id,
            job.shape,
            "steer",
        )?;
    }
    super::graph_job::require_current_driver_for_job_owned_run(paths, state, "steer")?;
    let prefix = run_prefix(&state.run_id);
    if text.trim().is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "steer text must not be empty",
            "deadreckon steer <run-id> \"one concrete instruction\"",
        )));
    }

    // The eligibility predicate lives in deadreckon-core (gap G6) so this
    // guard, `status --json`, `show --json`, and `RunView` can never
    // disagree. The driver fence above already resolved with full plan
    // lineage, so it is passed as settled here.
    let eligibility = deadreckon_core::steer_eligibility_with_driver_fence(state, false);
    match eligibility.reason {
        None => {}
        Some(deadreckon_core::SteerIneligibleReason::NotExecuting) => {
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
        Some(deadreckon_core::SteerIneligibleReason::ProviderNotSteerable) => {
            let provider = state.provider.as_deref().unwrap_or("no provider route");
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "run {} uses {provider}, which cannot accept live steering",
                    state.run_id
                ),
                "deadreckon config provider cli:codex-server",
            )));
        }
        Some(deadreckon_core::SteerIneligibleReason::DriverFenced) => {
            unreachable!("driver fence resolved above; eligibility was computed with it settled")
        }
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
    use tempfile::TempDir;

    /// Characterizes the guard order and refusals that predate the shared
    /// `deadreckon_core::steer_eligibility` predicate: routing the guard
    /// through the predicate must not change CLI behavior.
    #[test]
    fn queue_steer_for_state_keeps_the_historical_guard_behavior() {
        let temp = TempDir::new().expect("tempdir");
        let paths = deadreckon_core::DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "steer guard fixture".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("steer-guard-fixture".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let queue = |state: &deadreckon_core::PipelineState, text: &str| {
            super::queue_steer_for_state(
                &paths,
                state,
                deadreckon_core::steer_inbox::SteerSource::Cli,
                text.to_string(),
            )
        };

        let empty = queue(&state, "  ").expect_err("blank steer text must refuse");
        assert!(
            empty.to_string().contains("steer text must not be empty"),
            "{empty}"
        );

        let not_executing = queue(&state, "focus on tests").expect_err("pending run must refuse");
        assert!(
            not_executing.to_string().contains("cannot accept steering"),
            "{not_executing}"
        );

        state.status = deadreckon_core::RunStatus::Executing;
        let wrong_provider =
            queue(&state, "focus on tests").expect_err("non-codex-server provider must refuse");
        assert!(
            wrong_provider
                .to_string()
                .contains("uses smoke, which cannot accept live steering"),
            "{wrong_provider}"
        );

        state.provider = Some(deadreckon_core::STEERABLE_PROVIDER_ROUTE.to_string());
        let entry =
            queue(&state, "focus on tests").expect("executing codex-server run accepts steering");
        assert_eq!(entry.text, "focus on tests");
    }

    #[test]
    fn steer_follow_up_hint_quotes_shell_metacharacters() {
        assert_eq!(
            super::quoted_shell_argument("use $HOME and `pwd` \"carefully\""),
            "\"use \\$HOME and \\`pwd\\` \\\"carefully\\\"\""
        );
    }
}
