use super::super::*;

pub(crate) fn steer_command(run_id: &str, text: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = super::reference::resolve_run_like(&paths, Some(run_id), "steer")?;
    let prefix = run_prefix(&state.run_id);
    let entry = queue_steer_for_state(
        &paths,
        &state,
        deadreckon_core::steer_inbox::SteerSource::Cli,
        text,
    )?;

    let mid_turn =
        state.provider.as_deref() == Some(deadreckon_core::MID_TURN_STEER_PROVIDER_ROUTE);
    if let Some(verb) = machine_json::active() {
        let inbox_seq = steer_inbox_seq(&state.run_root, &entry);
        machine_json::emit_success(
            verb,
            &state.run_id,
            &steer_queued_surface(&prefix, &entry, mid_turn),
            json!({
                "queued_at": entry.ts,
                "inbox_seq": inbox_seq,
                "source": "cli",
                "delivery": if mid_turn {
                    "active or next provider turn"
                } else {
                    "next turn boundary"
                },
            }),
        )?;
        return Ok(());
    }
    print!("{}", steer_queued_text(&prefix, mid_turn));
    Ok(())
}

/// The `queued steer` prose, factored so the characterization test can pin
/// it while `--json` rides the same command. The `cli:codex-server` line is
/// the exact historical prose (mid-turn delivery is unchanged); every other
/// provider gets the honest between-turn wording: the run consumes the note
/// at its next turn boundary.
fn steer_queued_text(prefix: &str, mid_turn: bool) -> String {
    let delivery = if mid_turn {
        "delivery will begin on the active or next Codex turn"
    } else {
        "queued; the run consumes it at the start of its next turn"
    };
    format!(
        "{} {}\n  {} {delivery}\n  {} {}\n",
        ui_ok("queued steer for"),
        ui_id(prefix),
        ui_muted("delivery:"),
        ui_muted("watch:"),
        ui_command(format!("deadreckon attach {prefix}"))
    )
}

fn steer_queued_surface(
    prefix: &str,
    entry: &deadreckon_core::steer_inbox::SteerInboxEntry,
    mid_turn: bool,
) -> VerdictSurface {
    let mechanics = if mid_turn {
        "Delivery begins on the active or next Codex turn; the run stays the authority on when it consumes the inbox."
    } else {
        "The run consumes the inbox at its next turn boundary and injects the note into that turn's prompt as advisory operator guidance."
    };
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "steer",
        Some(prefix),
        ExplanationPanel::new(
            "DeadReckon queued the steering instruction in the run's steer inbox.",
            mechanics,
            vec![
                ("run".to_string(), prefix.to_string()),
                ("queued at".to_string(), entry.ts.to_rfc3339()),
            ],
        ),
        vec![("Recommended", format!("deadreckon attach {prefix}"))],
        vec![("Secondary", format!("deadreckon status {prefix}"))],
    )
}

/// The queued entry's position in the deduplicated inbox order, so a machine
/// consumer can correlate later delivery records with this queue action.
fn steer_inbox_seq(run_root: &Path, entry: &deadreckon_core::steer_inbox::SteerInboxEntry) -> u64 {
    deadreckon_core::steer_inbox::read_steer_inbox(run_root)
        .ok()
        .and_then(|inbox| {
            inbox
                .iter()
                .position(|candidate| candidate.identity() == entry.identity())
        })
        .and_then(|position| u64::try_from(position).ok())
        .unwrap_or(0)
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
            unreachable!(
                "the predicate no longer produces provider_not_steerable; \
                 every provider is steerable while Executing (M1 widening)"
            )
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

    /// Characterizes the guard order: empty-text and not-executing refusals
    /// keep their historical prose, and — the M1 widening — an Executing run
    /// accepts steering on any provider (the historical provider-route
    /// refusal is intentionally gone).
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

        // M1 widening: an Executing run accepts steering on ANY provider —
        // the smoke route queues for between-turn consumption by the loop.
        state.status = deadreckon_core::RunStatus::Executing;
        let entry = queue(&state, "focus on tests")
            .expect("executing run accepts steering on any provider");
        assert_eq!(entry.text, "focus on tests");

        state.provider = Some(deadreckon_core::MID_TURN_STEER_PROVIDER_ROUTE.to_string());
        let entry =
            queue(&state, "ship the fix").expect("executing codex-server run accepts steering");
        assert_eq!(entry.text, "ship the fix");
    }

    #[test]
    fn steer_follow_up_hint_quotes_shell_metacharacters() {
        assert_eq!(
            super::quoted_shell_argument("use $HOME and `pwd` \"carefully\""),
            "\"use \\$HOME and \\`pwd\\` \\\"carefully\\\"\""
        );
    }

    /// Prose pins (colors are gated on a TTY and off here). For the
    /// mid-turn `cli:codex-server` route the three lines are the exact
    /// historical bytes — the M1 widening did not move its surface. Every
    /// other provider gets the honest between-turn wording: the note is
    /// queued and consumed at the run's next turn boundary.
    #[test]
    fn steer_queued_prose_states_the_delivery_mechanics_per_provider() {
        assert_eq!(
            super::steer_queued_text("abc12345", true),
            "queued steer for abc12345\n  delivery: delivery will begin on the active or next Codex turn\n  watch: deadreckon attach abc12345\n"
        );
        assert_eq!(
            super::steer_queued_text("abc12345", false),
            "queued steer for abc12345\n  delivery: queued; the run consumes it at the start of its next turn\n  watch: deadreckon attach abc12345\n"
        );
    }

    /// G1 success envelope: `steer --json` reports `{queued_at, inbox_seq}`
    /// inside the shared envelope, sourced from the durable inbox entry.
    #[test]
    fn steer_success_envelope_carries_queued_at_and_inbox_seq() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        std::fs::create_dir_all(&run_root).expect("run root");
        let entry = deadreckon_core::steer_inbox::append_steer(
            &run_root,
            deadreckon_core::steer_inbox::SteerSource::Cli,
            "focus on tests",
        )
        .expect("queued entry");

        let inbox_seq = super::steer_inbox_seq(&run_root, &entry);
        assert_eq!(inbox_seq, 0, "first queued entry sits at inbox position 0");

        let envelope = crate::machine_json::success_envelope(
            "steer",
            "steer-envelope-fixture",
            &super::steer_queued_surface("steer-en", &entry, false),
            serde_json::json!({
                "queued_at": entry.ts,
                "inbox_seq": inbox_seq,
            }),
        );
        assert_eq!(envelope["kind"], "steer");
        assert_eq!(envelope["id"], "steer-envelope-fixture");
        assert_eq!(envelope["status"], "completed");
        assert_eq!(envelope["inbox_seq"], 0);
        assert_eq!(
            envelope["queued_at"],
            serde_json::json!(entry.ts),
            "queued_at is the durable inbox timestamp"
        );
        assert_eq!(envelope["next_actions"][0], "deadreckon attach steer-en");
    }

    /// G1 refusal envelope: the exact typed refusals the steer guard already
    /// produces become machine refusal envelopes with the packed `try:` line
    /// lifted into `try_lines` and the exit code preserved.
    #[test]
    fn steer_refusals_convert_to_machine_error_envelopes() {
        let temp = TempDir::new().expect("tempdir");
        let paths = deadreckon_core::DeadreckonPaths::from_home(temp.path().join("home"));
        let state = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "steer envelope fixture".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("steer-envelope-refusal".to_string()),
                codebase: None,
            },
        )
        .expect("run");

        let refusal = super::queue_steer_for_state(
            &paths,
            &state,
            deadreckon_core::steer_inbox::SteerSource::Cli,
            "focus on tests".to_string(),
        )
        .expect_err("pending run must refuse steering");
        let code = refusal.exit_code();
        let envelope = crate::machine_json::error_envelope("steer", code, &refusal.to_string(), "");

        assert_eq!(envelope["kind"], "error");
        assert_eq!(envelope["verb"], "steer");
        assert_eq!(envelope["code"], 1, "refusal exit code is preserved");
        let message = envelope["message"].as_str().expect("message");
        assert!(message.contains("cannot accept steering"), "{message}");
        assert!(
            !message.contains("try: "),
            "packed try line must be lifted out of the message: {message}"
        );
        let try_lines = envelope["try_lines"].as_array().expect("try_lines");
        assert_eq!(try_lines.len(), 1);
        assert!(
            try_lines[0]
                .as_str()
                .expect("try line")
                .starts_with("deadreckon extend "),
            "{try_lines:?}"
        );
    }
}
