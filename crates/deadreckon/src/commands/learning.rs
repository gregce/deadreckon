use super::super::*;
use deadreckon_core::learning::{
    LearningBundleExportReport, LearningBundleImportReport, LearningIndexSummary, LearningReport,
    PrDryRun, ProposalGenerationReport,
};

pub(crate) async fn learn_command(command: LearnCommand) -> Result<()> {
    match command {
        LearnCommand::Index {
            scope,
            all,
            since,
            json,
        } => learn_index_command(scope, all, since.as_deref(), json),
        LearnCommand::Report { scope, limit, json } => {
            learn_report_command(scope.as_deref(), limit, json)
        }
        LearnCommand::Export {
            source,
            output,
            redacted,
            json,
        } => learn_export_command(&source, output, redacted, json),
        LearnCommand::ImportBundle {
            path,
            preview,
            yes,
            json,
        } => learn_import_bundle_command(&path, preview, yes, json),
        LearnCommand::Propose {
            scope,
            all,
            from_local,
            bundle,
            limit,
            json,
        } => learn_propose_command(scope, all, from_local, bundle, limit, json).await,
    }
}

pub(crate) async fn improve_command(command: ImproveCommand) -> Result<()> {
    match command {
        ImproveCommand::SelfRun {
            target,
            preview,
            yes,
            pr_dry_run,
            open_pr,
            json,
        } => improve_self_command(target, preview, yes, pr_dry_run, open_pr, json).await,
    }
}

fn learn_index_command(
    scope: Option<String>,
    all: bool,
    since: Option<&str>,
    json_output: bool,
) -> Result<()> {
    if since.is_some() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--since is reserved for a later learning index slice",
            "deadreckon learn index --all",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let scope = resolve_learning_scope(scope, all)?;
    let summary = index_learning(&paths, &LearningIndexOptions { scope })?;
    let surface = learn_index_surface(&summary);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(serde_json::to_value(&summary)?))?
        );
        return Ok(());
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn learn_index_surface(summary: &LearningIndexSummary) -> VerdictSurface {
    let primary = if summary.signals_written == 0 {
        "deadreckon learn report"
    } else {
        "deadreckon learn propose"
    };
    let secondary = if summary.signals_written == 0 {
        vec![("Secondary", "deadreckon learn index --all")]
    } else {
        vec![("Secondary", "deadreckon learn report")]
    };
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "learn",
        Some("index"),
        ExplanationPanel::new(
            "DeadReckon indexed local run evidence into the learning store.",
            "Indexing completed; the recommended command advances to proposal generation when signals were written, otherwise it inspects the report.",
            vec![
                ("episodes", summary.indexed.to_string()),
                ("signals", summary.signals_written.to_string()),
                ("skipped live", summary.skipped_live.to_string()),
                ("skipped corrupt", summary.skipped_corrupt.to_string()),
            ],
        ),
        vec![("Recommended", primary)],
        secondary,
    )
}

fn learn_report_command(scope: Option<&str>, limit: usize, json_output: bool) -> Result<()> {
    if limit == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--limit must be at least 1",
            "deadreckon learn report --limit 10",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let mut report = learning_report(&paths, scope)?;
    report.top_signals.truncate(limit);
    let surface = learn_report_surface(&report);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(serde_json::to_value(&report)?))?
        );
        return Ok(());
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn learn_report_surface(report: &LearningReport) -> VerdictSurface {
    let has_signals = report.signals > 0;
    let (kind, why, primary, secondary) = if has_signals {
        (
            VerdictKind::Completed,
            "The report found learning signals, so proposal generation is the next useful command.",
            "deadreckon learn propose",
            vec![("Secondary", "deadreckon learn index --all")],
        )
    } else {
        (
            VerdictKind::Noop,
            "The report has no learning signals yet, so indexing all local evidence is the next useful command.",
            "deadreckon learn index --all",
            vec![("Secondary", "deadreckon learn report")],
        )
    };
    VerdictSurface::must_new(
        kind,
        "learn",
        Some("report"),
        ExplanationPanel::new(
            "DeadReckon summarized the local learning store.",
            why,
            vec![
                ("episodes", report.episodes.to_string()),
                ("signals", report.signals.to_string()),
                ("insights", report.insights.to_string()),
                ("proposals", report.proposals.to_string()),
                ("signal kinds", report.signals_by_kind.len().to_string()),
                ("top signals", report.top_signals.len().to_string()),
            ],
        ),
        vec![("Recommended", primary)],
        secondary,
    )
}

fn learn_export_command(
    source: &str,
    output: Option<PathBuf>,
    _redacted: bool,
    json_output: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let output = output.unwrap_or_else(|| {
        let source_slug = sanitize_slug(source);
        let bundle_id = format!("bundle-{}", source_slug);
        paths.learning_bundle_path(&bundle_id)
    });
    let report = export_learning_bundle(&paths, source, &output)?;
    let surface = learn_export_surface(&report);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(serde_json::to_value(&report)?))?
        );
        return Ok(());
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn learn_export_surface(report: &LearningBundleExportReport) -> VerdictSurface {
    let primary = format!(
        "deadreckon learn import-bundle {} --preview",
        report.output.display()
    );
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "learn",
        Some("export"),
        ExplanationPanel::new(
            "DeadReckon exported a redacted learning bundle.",
            "The bundle was written to disk; previewing the import is the safest next command before applying it.",
            vec![
                ("bundle", report.bundle_id.clone()),
                ("output", report.output.display().to_string()),
                ("episodes", report.episodes.to_string()),
                ("signals", report.signals.to_string()),
                ("insights", report.insights.to_string()),
                ("proposals", report.proposals.to_string()),
                ("redaction", report.redaction.profile.clone()),
                (
                    "redaction findings",
                    report.redaction.findings.len().to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon learn report")],
    )
}

fn learn_import_bundle_command(
    path: &Path,
    preview: bool,
    yes: bool,
    json_output: bool,
) -> Result<()> {
    if preview && yes {
        return Err(CliError::Core(deadreckon_core::user_error(
            "choose either --preview or --yes",
            "deadreckon learn import-bundle <path> --preview",
        )));
    }
    let apply = yes;
    let paths = DeadreckonPaths::discover();
    let report = import_learning_bundle(&paths, path, apply)?;
    let surface = learn_import_bundle_surface(path, &report);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(serde_json::to_value(&report)?))?
        );
        return Ok(());
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn learn_import_bundle_surface(path: &Path, report: &LearningBundleImportReport) -> VerdictSurface {
    let primary = if report.applied {
        "deadreckon learn report".to_string()
    } else {
        format!("deadreckon learn import-bundle {} --yes", path.display())
    };
    let (kind, what, why) = if report.applied {
        (
            VerdictKind::Completed,
            "DeadReckon imported the learning bundle into the local learning store.",
            "The import changed local learning evidence, so the next useful command is to inspect the report.",
        )
    } else {
        (
            VerdictKind::Preview,
            "DeadReckon previewed the learning bundle import without applying it.",
            "This is a preview because no learning records were written; the recommended command applies the same bundle.",
        )
    };
    VerdictSurface::must_new(
        kind,
        "learn",
        Some("import-bundle"),
        ExplanationPanel::new(
            what,
            why,
            vec![
                ("bundle", report.bundle_id.clone()),
                ("path", path.display().to_string()),
                ("episodes", report.episodes.to_string()),
                ("signals", report.signals.to_string()),
                ("insights", report.insights.to_string()),
                ("proposals", report.proposals.to_string()),
                (
                    "applied",
                    if report.applied { "yes" } else { "no" }.to_string(),
                ),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon learn report")],
    )
}

async fn learn_propose_command(
    scope: Option<String>,
    all: bool,
    from_local: bool,
    bundle: Option<PathBuf>,
    limit: usize,
    json_output: bool,
) -> Result<()> {
    if limit == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "--limit must be at least 1",
            "deadreckon learn propose --limit 1",
        )));
    }
    if from_local && (scope.is_some() || all) {
        return Err(CliError::Core(deadreckon_core::user_error(
            "`--from-local` is implicit; do not combine it with scope flags",
            "deadreckon learn propose --scope <scope>",
        )));
    }
    if bundle.is_some() && (scope.is_some() || all || from_local) {
        return Err(CliError::Core(deadreckon_core::user_error(
            "`--bundle` cannot be combined with local evidence flags",
            "deadreckon learn propose --bundle <path>",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let prompt = if let Some(bundle_path) = bundle.as_deref() {
        let bundle = read_learning_bundle(bundle_path)?;
        import_learning_bundle(&paths, bundle_path, true)?;
        build_reflection_prompt_from_bundle(&paths, &bundle, limit)?
    } else {
        let scope = resolve_learning_scope(scope, all)?;
        build_reflection_prompt(&paths, scope.as_deref(), limit)?
    };
    let router = ProviderRouter::from_config_path(&paths.config_path(), None)?;
    let route = router.selected_route_info().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "no provider route resolves for learning reflection",
            "deadreckon config provider",
        ))
    })?;
    let response = router
        .complete(&ProviderRequest {
            prompt,
            max_output_tokens: 8_000,
            cwd: Some(std::env::current_dir()?),
            output_path: Some(paths.learning_dir().join("reflection.out")),
            sandbox_backend: None,
            workspace_access: deadreckon_providers::WorkspaceAccess::ReadWrite,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        })
        .await?;
    let reflection_provider = LearningInsightProvider {
        route: response.provider,
        model: response.model,
    };
    let report = persist_reflection(&paths, &reflection_provider, &response.content, limit)?;
    let surface = learn_propose_surface(&route.name, &report);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(serde_json::to_value(&report)?))?
        );
        return Ok(());
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn learn_propose_surface(
    provider_route: &str,
    report: &ProposalGenerationReport,
) -> VerdictSurface {
    let (kind, primary, why) = if let Some(proposal) = report.proposals.first() {
        (
            VerdictKind::Completed,
            format!("deadreckon improve self {} --preview", proposal.proposal_id),
            "The provider returned valid reflection JSON and at least one proposal was persisted; previewing the first proposal is the safest next command."
                .to_string(),
        )
    } else {
        (
            VerdictKind::Noop,
            "deadreckon learn report".to_string(),
            "No proposals were persisted, so the next useful command is to inspect the learning report before trying reflection again."
                .to_string(),
        )
    };
    let first_proposal = report
        .proposals
        .first()
        .map(|proposal| proposal.proposal_id.clone())
        .unwrap_or_else(|| "none".to_string());
    let first_title = report
        .proposals
        .first()
        .map(|proposal| one_line(&proposal.title, 100))
        .unwrap_or_else(|| "none".to_string());
    VerdictSurface::must_new(
        kind,
        "learn",
        Some("propose"),
        ExplanationPanel::new(
            "DeadReckon converted learning signals into provider-backed proposals.",
            why,
            vec![
                ("provider route", provider_route.to_string()),
                ("insights written", report.insights_written.to_string()),
                ("proposals written", report.proposals_written.to_string()),
                ("first proposal", first_proposal),
                ("first title", first_title),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon learn report")],
    )
}

fn resolve_learning_scope(scope: Option<String>, all: bool) -> Result<Option<String>> {
    if all && scope.is_some() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "use either --all or --scope, not both",
            "deadreckon learn index --all",
        )));
    }
    if all {
        return Ok(None);
    }
    scope.map_or_else(|| current_scope().map(Some), |scope| Ok(Some(scope)))
}

async fn improve_self_command(
    target: String,
    preview: bool,
    yes: bool,
    pr_dry_run: bool,
    open_pr: bool,
    json_output: bool,
) -> Result<()> {
    if [preview, yes, pr_dry_run, open_pr]
        .iter()
        .filter(|value| **value)
        .count()
        != 1
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            "choose exactly one of --preview, --yes, --pr-dry-run, or --open-pr",
            "deadreckon improve self <proposal-id> --preview",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let proposal = load_self_improve_proposal(&paths, &target)?;
    if preview {
        return improve_self_preview(&proposal, json_output);
    }
    if pr_dry_run || open_pr {
        let candidate = latest_candidate_for_proposal(&paths, &proposal.proposal_id)?;
        let eval = read_candidate_eval(&paths, &candidate.candidate_id)?;
        let policy = load_learning_policy(&paths)?;
        let dry_run = prepare_pr_dry_run(
            &paths,
            &proposal,
            &candidate,
            &eval,
            &policy,
            pr_dry_run || open_pr,
        )?;
        if open_pr {
            let adapter = GhSelfImprovePrAdapter;
            let pr_url = open_self_improve_pr_if_eligible(
                &paths,
                &proposal.proposal_id,
                &candidate,
                &dry_run,
                &adapter,
            )?;
            let surface = improve_self_open_pr_surface(&proposal, &candidate, &dry_run, &pr_url);
            if json_output {
                let payload = json!({
                    "proposal_id": proposal.proposal_id,
                    "candidate_id": candidate.candidate_id,
                    "branch": candidate.branch,
                    "pr_url": pr_url,
                    "body_path": dry_run.body_path,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&surface.add_to_json(payload))?
                );
            } else {
                println!("{}", surface.render_plain(!completion_hints_enabled(false)));
            }
            return Ok(());
        }
        let surface = improve_self_pr_dry_run_surface(&proposal, &candidate, &dry_run);
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &surface.add_to_json(serde_json::to_value(&dry_run)?)
                )?
            );
        } else {
            println!("{}", surface.render_plain(!completion_hints_enabled(false)));
        }
        return Ok(());
    }
    run_self_improve_candidate(&paths, &proposal, json_output).await
}

fn improve_self_pr_dry_run_surface(
    proposal: &LearningProposal,
    candidate: &LearningCandidate,
    dry_run: &PrDryRun,
) -> VerdictSurface {
    let eligible = dry_run.decision.eligible;
    let kind = if eligible {
        VerdictKind::Completed
    } else {
        VerdictKind::Blocked
    };
    let primary = if eligible {
        format!("deadreckon improve self {} --open-pr", proposal.proposal_id)
    } else if candidate.status == "verified" {
        format!("deadreckon improve self {} --yes", proposal.proposal_id)
    } else {
        format!("deadreckon show {} --why-failed", candidate.run_id)
    };
    let why = if eligible {
        "The evidence gate passed in dry-run mode; opening the PR remains an explicit opt-in command."
    } else {
        "The PR gate blocked live opening; the recommended command is the safest recovery path for the current candidate state."
    };
    VerdictSurface::must_new(
        kind,
        "improve",
        Some("self"),
        ExplanationPanel::new(
            "DeadReckon prepared the self-improvement PR body without pushing or opening a PR.",
            why,
            vec![
                ("proposal", proposal.proposal_id.clone()),
                ("candidate", candidate.candidate_id.clone()),
                ("branch", dry_run.branch.clone()),
                ("body", dry_run.body_path.display().to_string()),
                ("eligible", if eligible { "yes" } else { "no" }.to_string()),
                ("gate reasons", dry_run.decision.reasons.len().to_string()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon status")],
    )
}

fn improve_self_open_pr_surface(
    proposal: &LearningProposal,
    candidate: &LearningCandidate,
    dry_run: &PrDryRun,
    pr_url: &str,
) -> VerdictSurface {
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "improve",
        Some("self"),
        ExplanationPanel::new(
            "DeadReckon opened the self-improvement pull request after the evidence gate passed.",
            "The PR URL is now the durable review target, so the safest next command is local status inspection.",
            vec![
                ("proposal", proposal.proposal_id.clone()),
                ("candidate", candidate.candidate_id.clone()),
                ("branch", candidate.branch.clone()),
                ("body", dry_run.body_path.display().to_string()),
                ("pr", pr_url.to_string()),
            ],
        ),
        vec![("Recommended", "deadreckon status")],
        vec![("Secondary", "deadreckon learn report")],
    )
}

fn improve_self_preview(proposal: &LearningProposal, json_output: bool) -> Result<()> {
    let surface = improve_self_preview_surface(proposal);
    let payload = json!({
        "proposal_id": proposal.proposal_id,
        "title": proposal.title,
        "target": proposal.target,
        "risk": proposal.expected_risk,
        "done_criteria": proposal.done_criteria,
        "mode": "isolated-worktree",
        "provider": "existing resolver",
        "pr": "dry-run by default; live open requires evidence gate"
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(payload))?
        );
        return Ok(());
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn improve_self_preview_surface(proposal: &LearningProposal) -> VerdictSurface {
    let primary = format!("deadreckon improve self {} --yes", proposal.proposal_id);
    let secondary = format!(
        "deadreckon improve self {} --pr-dry-run",
        proposal.proposal_id
    );
    VerdictSurface::must_new(
        VerdictKind::Preview,
        "improve",
        Some("self"),
        ExplanationPanel::new(
            "DeadReckon previewed a self-improvement proposal without creating a candidate worktree.",
            "This is a preview because no run, worktree, branch, or PR state was created; the recommended command starts the isolated candidate run.",
            vec![
                ("proposal", proposal.proposal_id.clone()),
                ("title", proposal.title.clone()),
                ("mode", "isolated worktree".to_string()),
                ("done criteria", proposal.done_criteria.len().to_string()),
                ("risk", proposal.expected_risk.clone()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", secondary.as_str())],
    )
}

async fn run_self_improve_candidate(
    paths: &DeadreckonPaths,
    proposal: &LearningProposal,
    json_output: bool,
) -> Result<()> {
    let source_root = git_stdout(&std::env::current_dir()?, &["rev-parse", "--show-toplevel"])?;
    let source_root = PathBuf::from(source_root);
    let status = git_stdout(&source_root, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "base worktree is dirty",
            "git status --short",
        )));
    }
    let policy = load_learning_policy(paths)?;
    let defaults = config_defaults(paths)?;
    if defaults.sandbox.as_deref() == Some("none") && !policy.self_run.allow_sandbox_none {
        return Err(CliError::Core(deadreckon_core::user_error(
            "self-improve refuses sandbox none",
            "deadreckon config sandbox auto",
        )));
    }
    let base_commit = git_stdout(&source_root, &["rev-parse", "HEAD"])?;
    let candidate_id = format!("cand-{}", Uuid::new_v4().simple());
    let branch = format!("deadreckon/self/{candidate_id}");
    let candidate_dir = paths.learning_candidate_dir(&candidate_id);
    let worktree = candidate_dir.join("worktree");
    fs::create_dir_all(&candidate_dir)?;
    run_git(
        &source_root,
        &[
            "worktree",
            "add",
            "-b",
            branch.as_str(),
            path_to_str(&worktree)?,
            "HEAD",
        ],
    )?;
    let goal_file = candidate_dir.join("goal.md");
    fs::write(&goal_file, &proposal.goal_text)?;
    let acceptance = candidate_dir.join("acceptance.yaml");
    fs::write(
        &acceptance,
        r#"
name: self-improvement-focused
checks:
  - kind: shell
    command: "cargo test -p deadreckon-core learning --lib"
"#,
    )?;

    let before_runs = list_runs(paths, None)?
        .into_iter()
        .map(|run| run.run_id)
        .collect::<BTreeSet<_>>();
    let exe = std::env::current_exe()?;
    let status = std::process::Command::new(exe)
        .current_dir(&worktree)
        .env("DEADRECKON_HOME", paths.home())
        .arg("run")
        .arg(&proposal.goal_text)
        .arg("--in-place")
        .arg("--i-know-its-a-lot")
        .arg("--yes")
        .arg("--acceptance")
        .arg(&acceptance)
        .status()?;
    let run_id = newest_created_run_id(paths, &before_runs)?;
    run_git(&worktree, &["add", "-A"])?;
    let staged = git_stdout(&worktree, &["diff", "--cached", "--name-only"])?;
    if !staged.trim().is_empty() {
        let message = format!("self-improve: {}", one_line(&proposal.title, 64));
        run_git(
            &worktree,
            &[
                "-c",
                "user.name=deadreckon",
                "-c",
                "user.email=deadreckon@example.invalid",
                "commit",
                "-m",
                message.as_str(),
            ],
        )?;
    }
    let head_commit = git_stdout(&worktree, &["rev-parse", "HEAD"])?;
    let changed_files = git_stdout(&worktree, &["diff", "--name-only", "HEAD~1..HEAD"])
        .unwrap_or_default()
        .lines()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let diff = diff_summary(&worktree, "HEAD~1..HEAD").unwrap_or_default();
    let diff_text = git_stdout(&worktree, &["diff", "HEAD~1..HEAD"]).unwrap_or_default();
    let risk = classify_candidate_risk(&changed_files);
    let mut candidate = LearningCandidate {
        version: 1,
        candidate_id: candidate_id.clone(),
        proposal_id: proposal.proposal_id.clone(),
        branch,
        base_commit,
        head_commit,
        run_id,
        worktree: worktree.clone(),
        diff: LearningCandidateDiff {
            files: changed_files.len() as u32,
            insertions: diff.0,
            deletions: diff.1,
            changed_files,
        },
        risk,
        status: if status.success() {
            "verified"
        } else {
            "rejected"
        }
        .to_string(),
        evidence_packet: "evidence.json".to_string(),
    };
    let verify_status = std::process::Command::new("cargo")
        .current_dir(&worktree)
        .args(["test", "-p", "deadreckon-core", "learning", "--lib"])
        .status()?;
    let mut eval = LearningEval {
        version: 1,
        candidate_id: candidate_id.clone(),
        evaluated_at: Utc::now(),
        accepted_run: status.success(),
        commands: vec![LearningEvalCommand {
            cmd: "cargo test -p deadreckon-core learning --lib".to_string(),
            status: verify_status.code().unwrap_or(1),
        }],
        docs_updated: candidate
            .diff
            .changed_files
            .iter()
            .any(|file| file.starts_with("docs/") || file == "CHANGELOG.md"),
        redaction_passed: !learning_text_has_sensitive(&diff_text, paths),
        evidence_score: 0.0,
        auto_pr: LearningAutoPrStatus {
            eligible: false,
            reasons: Vec::new(),
        },
    };
    eval.evidence_score = evidence_score(proposal, &candidate, &eval);
    let decision = evaluate_auto_pr(proposal, &candidate, &eval, &policy, false);
    eval.auto_pr.eligible = decision.eligible;
    eval.auto_pr.reasons = decision.reasons;
    if eval.evidence_score < policy.pr.min_evidence_score {
        candidate.status = "rejected".to_string();
    }
    write_candidate(paths, &candidate)?;
    write_eval(paths, &eval)?;
    let evidence_path = paths
        .learning_candidate_dir(&candidate_id)
        .join("evidence.json");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&json!({
            "proposal": proposal,
            "candidate": candidate,
            "eval": eval,
            "rollback": format!("git branch -D {} && git worktree remove {}", candidate.branch, candidate.worktree.display())
        }))?,
    )?;
    let surface = improve_self_candidate_surface(proposal, &candidate, &eval, &evidence_path);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(json!({
                "candidate_id": candidate_id,
                "candidate": candidate,
                "eval": eval,
                "evidence": evidence_path,
            })))?
        );
    } else {
        println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    }
    Ok(())
}

fn improve_self_candidate_surface(
    proposal: &LearningProposal,
    candidate: &LearningCandidate,
    eval: &LearningEval,
    evidence_path: &Path,
) -> VerdictSurface {
    let kind = if candidate.status == "verified" {
        VerdictKind::Verified
    } else {
        VerdictKind::Blocked
    };
    let primary = format!(
        "deadreckon improve self {} --pr-dry-run",
        proposal.proposal_id
    );
    let why = if candidate.status == "verified" {
        "The isolated candidate run and focused verification completed; the next safe command previews PR eligibility without opening anything."
    } else {
        "The isolated candidate did not satisfy the evidence gate; the PR dry-run explains the blocking reasons without pushing anything."
    };
    VerdictSurface::must_new(
        kind,
        "improve",
        Some("self"),
        ExplanationPanel::new(
            "DeadReckon created and evaluated a self-improvement candidate in an isolated worktree.",
            why,
            vec![
                ("proposal", proposal.proposal_id.clone()),
                ("candidate", candidate.candidate_id.clone()),
                ("run", candidate.run_id.clone()),
                ("branch", candidate.branch.clone()),
                ("status", candidate.status.clone()),
                ("evidence score", format!("{:.2}", eval.evidence_score)),
                ("evidence", evidence_path.display().to_string()),
            ],
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", "deadreckon status")],
    )
}

fn load_self_improve_proposal(paths: &DeadreckonPaths, target: &str) -> Result<LearningProposal> {
    let path = PathBuf::from(target);
    if path.exists() {
        let goal_text = fs::read_to_string(&path)?;
        return Ok(LearningProposal {
            version: 1,
            proposal_id: format!("prop-{}", Uuid::new_v4().simple()),
            created_at: Utc::now(),
            title: path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("self-improvement")
                .to_string(),
            insights: Vec::new(),
            stimulus: Vec::<LearningStimulus>::new(),
            hypothesis: "manual goal file".to_string(),
            target: LearningProposalTarget {
                repo: deadreckon_core::learning::proposal_target_repo(),
                scope: "manual".to_string(),
            },
            goal_text,
            done_criteria: vec!["focused verification passes".to_string()],
            expected_risk: "medium".to_string(),
            blocked_auto_pr_reasons: Vec::new(),
        });
    }
    read_proposal(paths, target).map_err(CliError::from)
}

fn latest_candidate_for_proposal(
    paths: &DeadreckonPaths,
    proposal_id: &str,
) -> Result<LearningCandidate> {
    let dir = paths.learning_candidates_dir();
    if !dir.exists() {
        return Err(missing_self_improve_candidate_error(
            paths,
            proposal_id,
            "candidate store missing",
        ));
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path().join("candidate.json");
        if !path.exists() {
            continue;
        }
        let candidate: LearningCandidate = serde_json::from_slice(&fs::read(&path)?)?;
        if candidate.proposal_id == proposal_id {
            let updated = fs::metadata(&path)?.modified().ok();
            candidates.push((updated, candidate));
        }
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    candidates
        .into_iter()
        .map(|(_, candidate)| candidate)
        .next()
        .ok_or_else(|| {
            missing_self_improve_candidate_error(
                paths,
                proposal_id,
                "no candidate matched this proposal",
            )
        })
}

fn missing_self_improve_candidate_error(
    paths: &DeadreckonPaths,
    proposal_id: &str,
    reason: &str,
) -> CliError {
    let primary = format!("deadreckon improve self {proposal_id} --yes");
    CliError::Surface {
        code: 1,
        surface: VerdictSurface::must_new(
            VerdictKind::Blocked,
            "improve",
            Some("self"),
            ExplanationPanel::new(
                "DeadReckon could not prepare the self-improvement PR dry run because no candidate evidence exists yet.",
                "The PR dry-run depends on a previously verified candidate, so running the isolated candidate is the safest next step.",
                vec![
                    ("proposal".to_string(), proposal_id.to_string()),
                    ("reason".to_string(), reason.to_string()),
                    (
                        "candidate evidence".to_string(),
                        paths.learning_candidates_dir().display().to_string(),
                    ),
                ],
            ),
            vec![("Recommended", primary.as_str())],
            vec![("Secondary", "deadreckon learn report")],
        )
        .render_plain(!completion_hints_enabled(false)),
    }
}

fn read_candidate_eval(paths: &DeadreckonPaths, candidate_id: &str) -> Result<LearningEval> {
    let path = paths.learning_eval_path(candidate_id);
    let data = fs::read(&path)?;
    serde_json::from_slice(&data).map_err(CliError::from)
}

fn newest_created_run_id(paths: &DeadreckonPaths, before: &BTreeSet<String>) -> Result<String> {
    list_runs(paths, None)?
        .into_iter()
        .find(|run| !before.contains(&run.run_id))
        .map(|run| run.run_id)
        .ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                "self-run did not create a discoverable run",
                "deadreckon list --all",
            ))
        })
}

fn diff_summary(worktree: &Path, range: &str) -> Result<(u32, u32)> {
    let raw = git_stdout(worktree, &["diff", "--numstat", range])?;
    let mut insertions = 0u32;
    let mut deletions = 0u32;
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let added = parts.next().and_then(|value| value.parse::<u32>().ok());
        let removed = parts.next().and_then(|value| value.parse::<u32>().ok());
        insertions = insertions.saturating_add(added.unwrap_or(0));
        deletions = deletions.saturating_add(removed.unwrap_or(0));
    }
    Ok((insertions, deletions))
}

fn learning_text_has_sensitive(value: &str, paths: &DeadreckonPaths) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("ghp_")
        || lower.contains("github_pat_")
        || lower.contains("sk-")
        || lower.contains("api_key")
        || lower.contains("api-key")
        || lower.contains("begin openssh private key")
        || lower.contains("begin private key")
        || value.contains(paths.home().to_string_lossy().as_ref())
        || std::env::var("HOME").is_ok_and(|home| !home.is_empty() && value.contains(&home))
}

pub(crate) trait SelfImprovePrAdapter {
    fn open_pr(&self, candidate: &LearningCandidate, dry_run: &PrDryRun) -> Result<String>;
}

struct GhSelfImprovePrAdapter;

impl SelfImprovePrAdapter for GhSelfImprovePrAdapter {
    fn open_pr(&self, candidate: &LearningCandidate, dry_run: &PrDryRun) -> Result<String> {
        run_git(
            &candidate.worktree,
            &["push", "-u", "origin", candidate.branch.as_str()],
        )?;
        let output = std::process::Command::new("gh")
            .current_dir(&candidate.worktree)
            .arg("pr")
            .arg("create")
            .arg("--title")
            .arg(&dry_run.title)
            .arg("--body-file")
            .arg(&dry_run.body_path)
            .arg("--head")
            .arg(&candidate.branch)
            .output()?;
        if !output.status.success() {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "gh pr create failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ))));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

pub(crate) fn open_self_improve_pr_if_eligible(
    paths: &DeadreckonPaths,
    proposal_id: &str,
    candidate: &LearningCandidate,
    dry_run: &PrDryRun,
    adapter: &dyn SelfImprovePrAdapter,
) -> Result<String> {
    if !dry_run.decision.eligible {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("PR gate failed: {}", dry_run.decision.reasons.join("; ")),
            &format!("deadreckon improve self {proposal_id} --pr-dry-run"),
        )));
    }
    let pr_url = adapter.open_pr(candidate, dry_run)?;
    record_pr_event(
        paths,
        &LearningPrEvent {
            version: 1,
            timestamp: Utc::now(),
            candidate_id: candidate.candidate_id.clone(),
            mode: "open".to_string(),
            status: "opened".to_string(),
            branch: candidate.branch.clone(),
            pr_url: Some(pr_url.clone()),
            body_path: dry_run.body_path.to_string_lossy().to_string(),
            reason: None,
        },
    )?;
    Ok(pr_url)
}
