use super::super::*;

pub(crate) struct ReportCommandArgs {
    pub(crate) run_id: String,
    pub(crate) html: bool,
    pub(crate) dest: Option<PathBuf>,
    pub(crate) open: bool,
    pub(crate) json: bool,
    pub(crate) plain: bool,
}

pub(crate) fn report_command(args: ReportCommandArgs) -> Result<()> {
    let _ = args.plain;
    let paths = DeadreckonPaths::discover();
    let state = load_cli_run(&paths, &args.run_id).map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("run {} not found ({err})", args.run_id),
            "deadreckon list",
        ))
    })?;
    if matches!(
        state.status,
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing
    ) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("run {} still running", run_prefix(&state.run_id)),
            &format!("deadreckon attach {}", run_prefix(&state.run_id)),
        )));
    }
    let view = deadreckon_core::RunView::from_state(&state)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&view)?);
        return Ok(());
    }
    let dest = args.dest.unwrap_or_else(|| {
        state.run_root.join(if args.html {
            "report.html"
        } else {
            "report.md"
        })
    });
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let rendered = if args.html {
        render_report_html(&view)
    } else {
        render_report_markdown(&view)
    };
    fs::write(&dest, rendered)?;
    if args.open {
        if !io::stdout().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "report --open requires an interactive terminal",
                &format!(
                    "deadreckon report {} --dest {}",
                    run_prefix(&state.run_id),
                    dest.display()
                ),
            )));
        }
        open_path(&dest)?;
    }
    print!(
        "{}",
        report_surface(&view, &dest).render_plain(!crate::completion_hints_enabled(false))
    );
    Ok(())
}

fn report_surface(view: &deadreckon_core::RunView, dest: &Path) -> VerdictSurface {
    let short = &view.id.short;
    VerdictSurface::must_new(
        VerdictKind::Verified,
        "report",
        Some(short),
        ExplanationPanel::new(
            "DeadReckon wrote a static report from the shared RunView model.",
            "The report is self-contained and can be archived or shared without reading the run directory directly.",
            vec![
                ("output", dest.display().to_string()),
                ("turns", view.turns.len().to_string()),
                ("changed files", view.changed.files_changed.to_string()),
                ("proof checks", view.proof.checks.len().to_string()),
            ],
        ),
        vec![("Recommended", format!("deadreckon show {short}"))],
        vec![("Secondary", format!("deadreckon report {short} --json"))],
    )
}

pub(crate) fn render_report_markdown(view: &deadreckon_core::RunView) -> String {
    let mut out = String::new();
    out.push_str(&format!("# deadreckon report: {}\n\n", view.id.short));
    out.push_str("## Verdict\n\n");
    out.push_str(&format!("- state: {}\n", view.verdict.state));
    out.push_str(&format!("- status: {}\n", run_status_label(view.status)));
    out.push_str(&format!("- signature: {:?}\n", view.signature.status));
    out.push_str(&format!("- summary: {}\n\n", view.verdict.summary));
    out.push_str("## Changed\n\n");
    out.push_str(&format!(
        "- files: {}\n- lines added: {}\n- lines removed: {}\n",
        view.changed.files_changed, view.changed.added, view.changed.removed
    ));
    for file in &view.changed.files {
        out.push_str(&format!(
            "- {:?} {} (+{} -{})\n",
            file.status,
            file.path.display(),
            file.added,
            file.removed
        ));
    }
    out.push_str("\n## Why\n\n");
    if let Some(excerpt) = view.why.narrative_excerpt.as_deref() {
        out.push_str(&format!("- narrative: {excerpt}\n"));
    }
    if let Some(path) = view.why.narrative_path.as_ref() {
        out.push_str(&format!("- narrative path: {}\n", path.display()));
    }
    if let Some(path) = view.why.decisions_path.as_ref() {
        out.push_str(&format!("- decisions path: {}\n", path.display()));
    }
    for decision in &view.why.decision_refs {
        out.push_str(&format!("- decision: {decision}\n"));
    }
    out.push_str("\n## Turns\n\n");
    if view.turns.is_empty() {
        out.push_str("- no turns recorded\n");
    }
    for turn in &view.turns {
        out.push_str(&format!(
            "- turn {}: {}; files {}, +{} -{}, spend ${:.4}\n",
            turn.n,
            turn.did,
            turn.diff.files_changed,
            turn.diff.added,
            turn.diff.removed,
            turn.spend_delta.usd
        ));
        if let Some(exchange) = turn.exchange_ref.as_ref() {
            out.push_str(&format!("  - exchange: {}\n", exchange.preview));
        }
        for event in &turn.sandbox_events {
            out.push_str(&format!("  - event: {}\n", event.summary));
        }
    }
    out.push_str("\n## Proof\n\n");
    out.push_str(&format!(
        "- marker valid: {}\n- checks: {}\n",
        view.proof.marker_valid,
        view.proof.checks.len()
    ));
    if let Some(path) = view.proof.marker_path.as_ref() {
        out.push_str(&format!("- marker: {}\n", path.display()));
    }
    if let Some(path) = view.proof.tamper_path.as_ref() {
        out.push_str(&format!("- tamper: {}\n", path.display()));
    }
    for check in &view.proof.checks {
        out.push_str(&format!(
            "- {} {}: {}\n",
            if check.passed { "pass" } else { "fail" },
            check.kind,
            check.detail
        ));
    }
    out
}

pub(crate) fn render_report_html(view: &deadreckon_core::RunView) -> String {
    let markdown = render_report_markdown(view);
    let mut html = String::new();
    html.push_str(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>deadreckon report</title>",
    );
    html.push_str("<style>body{font-family:system-ui,-apple-system,BlinkMacSystemFont,sans-serif;max-width:960px;margin:40px auto;padding:0 24px;line-height:1.5;color:#17202a}pre{white-space:pre-wrap;background:#f6f8fa;padding:16px;border-radius:6px}h1,h2{line-height:1.2}</style>");
    html.push_str("</head><body><pre>");
    html.push_str(&escape_html(&markdown));
    html.push_str("</pre></body></html>\n");
    html
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn open_path(path: &Path) -> Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let mut command = std::process::Command::new(program);
    if cfg!(target_os = "windows") {
        command.arg("/C").arg("start").arg(path);
    } else {
        command.arg(path);
    }
    let status = command.status()?;
    if !status.success() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("failed to open {}", path.display()),
            &format!("open {}", path.display()),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_view() -> deadreckon_core::RunView {
        deadreckon_core::RunView {
            id: deadreckon_core::RunIdentity {
                scope: "scope".to_string(),
                run_id: "abcdef123456".to_string(),
                short: "abcdef12".to_string(),
            },
            goal: "goal".to_string(),
            status: RunStatus::Completed,
            verdict: deadreckon_core::VerdictBand {
                state: "VERIFIED".to_string(),
                summary: "verified".to_string(),
            },
            signature: deadreckon_core::SignatureFact {
                status: deadreckon_core::SignatureStatus::Valid,
                marker_path: None,
                tamper_path: None,
                tamper_verdict: None,
            },
            sandbox: deadreckon_core::SandboxFact {
                backend: "none".to_string(),
                path: None,
                tools: Vec::new(),
                fallback_note: None,
            },
            spend: deadreckon_core::SpendBand::default(),
            wall_secs: Some(1),
            provider: "smoke".to_string(),
            changed: deadreckon_core::DiffSummary::default(),
            why: deadreckon_core::WhyBand::default(),
            turns: Vec::new(),
            proof: deadreckon_core::ProofBand::default(),
            missing: Vec::new(),
        }
    }

    #[test]
    fn report_markdown_contains_all_five_bands() {
        let report = render_report_markdown(&minimal_view());

        for heading in ["## Verdict", "## Changed", "## Why", "## Turns", "## Proof"] {
            assert!(report.contains(heading), "{report}");
        }
    }

    #[test]
    fn report_html_is_self_contained_no_external_refs() {
        let report = render_report_html(&minimal_view());

        assert!(report.contains("<style>"), "{report}");
        assert!(!report.contains("<script"), "{report}");
        assert!(!report.contains("http://"), "{report}");
        assert!(!report.contains("https://"), "{report}");
    }
}
