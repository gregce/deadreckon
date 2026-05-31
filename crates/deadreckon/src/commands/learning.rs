use super::super::*;

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
    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }
    println!("{}", ui_heading("learning indexed"));
    print_kv_block(&[
        ("episodes", &summary.indexed.to_string()),
        ("signals", &summary.signals_written.to_string()),
        ("skipped live", &summary.skipped_live.to_string()),
        ("skipped corrupt", &summary.skipped_corrupt.to_string()),
    ]);
    if summary.signals_written == 0 {
        println!(
            "{} try `{}`",
            ui_muted("next:"),
            ui_command("deadreckon learn report")
        );
    } else {
        println!(
            "{} try `{}`",
            ui_muted("next:"),
            ui_command("deadreckon learn propose")
        );
    }
    Ok(())
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
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("{}", ui_heading("learning report"));
    print_kv_block(&[
        ("episodes", &report.episodes.to_string()),
        ("signals", &report.signals.to_string()),
        ("insights", &report.insights.to_string()),
        ("proposals", &report.proposals.to_string()),
    ]);
    if report.signals_by_kind.is_empty() {
        println!(
            "{} try `{}`",
            ui_muted("hint:"),
            ui_command("deadreckon learn index --all")
        );
        return Ok(());
    }
    println!();
    println!("{}", ui_heading("signals"));
    for (kind, count) in &report.signals_by_kind {
        println!("  {kind}: {count}");
    }
    if !report.top_signals.is_empty() {
        println!();
        println!("{}", ui_heading("top signals"));
        for signal in &report.top_signals {
            println!(
                "  {} {} {}",
                ui_id(&signal.signal_id),
                ui_status(&signal.kind),
                one_line(&signal.summary, 120)
            );
        }
    }
    println!(
        "{} {}",
        ui_muted("next:"),
        ui_command("deadreckon learn propose")
    );
    Ok(())
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
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("{}", ui_heading("learning bundle exported"));
    print_kv_block(&[
        ("bundle", report.bundle_id.as_str()),
        ("output", &report.output.display().to_string()),
        ("episodes", &report.episodes.to_string()),
        ("signals", &report.signals.to_string()),
        ("insights", &report.insights.to_string()),
        ("proposals", &report.proposals.to_string()),
        ("redaction", report.redaction.profile.as_str()),
    ]);
    if !report.redaction.findings.is_empty() {
        println!("redacted:");
        for finding in &report.redaction.findings {
            println!("  - {finding}");
        }
    }
    println!(
        "{} {}",
        ui_muted("next:"),
        ui_command(format!(
            "deadreckon learn import-bundle {} --preview",
            report.output.display()
        ))
    );
    Ok(())
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
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{}",
        ui_heading(if apply {
            "learning bundle imported"
        } else {
            "learning bundle preview"
        })
    );
    print_kv_block(&[
        ("bundle", report.bundle_id.as_str()),
        ("episodes", &report.episodes.to_string()),
        ("signals", &report.signals.to_string()),
        ("insights", &report.insights.to_string()),
        ("proposals", &report.proposals.to_string()),
        ("applied", if report.applied { "yes" } else { "no" }),
    ]);
    if !apply {
        println!(
            "{} {}",
            ui_muted("next:"),
            ui_command(format!(
                "deadreckon learn import-bundle {} --yes",
                path.display()
            ))
        );
    }
    Ok(())
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
            pid_file: None,
            cancellation_token: None,
        })
        .await?;
    let reflection_provider = LearningInsightProvider {
        route: response.provider,
        model: response.model,
    };
    let report = persist_reflection(&paths, &reflection_provider, &response.content, limit)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("{}", ui_heading("learning proposals"));
    print_kv_block(&[
        ("provider route", route.name.as_str()),
        ("insights", &report.insights_written.to_string()),
        ("proposals", &report.proposals_written.to_string()),
    ]);
    for proposal in &report.proposals {
        println!(
            "  {} {}",
            ui_id(&proposal.proposal_id),
            one_line(&proposal.title, 100)
        );
    }
    if let Some(proposal) = report.proposals.first() {
        println!(
            "{} {}",
            ui_muted("next:"),
            ui_command(format!(
                "deadreckon improve self {} --preview",
                proposal.proposal_id
            ))
        );
    }
    Ok(())
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
            println!("{}", ui_ok(format!("opened PR {pr_url}")));
            return Ok(());
        }
        if json_output {
            println!("{}", serde_json::to_string_pretty(&dry_run)?);
        } else {
            println!("{}", ui_heading("self-improve PR dry-run"));
            print_kv_block(&[
                ("branch", dry_run.branch.as_str()),
                ("title", dry_run.title.as_str()),
                (
                    "eligible",
                    if dry_run.decision.eligible {
                        "yes"
                    } else {
                        "no"
                    },
                ),
                ("body", &dry_run.body_path.display().to_string()),
            ]);
            if !dry_run.decision.reasons.is_empty() {
                println!("reasons:");
                for reason in &dry_run.decision.reasons {
                    println!("  - {reason}");
                }
            }
        }
        return Ok(());
    }
    run_self_improve_candidate(&paths, &proposal, json_output).await
}

fn improve_self_preview(proposal: &LearningProposal, json_output: bool) -> Result<()> {
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
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    println!("{}", ui_heading("self-improve preview"));
    print_kv_block(&[
        ("proposal", proposal.proposal_id.as_str()),
        ("title", proposal.title.as_str()),
        ("mode", "isolated worktree"),
        ("provider", "existing resolver"),
        ("PR", "dry-run until evidence gate passes"),
    ]);
    println!();
    println!("{}", ui_heading("done criteria"));
    for criterion in &proposal.done_criteria {
        println!("  - {criterion}");
    }
    println!(
        "{} {}",
        ui_muted("next:"),
        ui_command(format!(
            "deadreckon improve self {} --yes",
            proposal.proposal_id
        ))
    );
    Ok(())
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
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "candidate_id": candidate_id,
                "candidate": candidate,
                "eval": eval,
                "evidence": evidence_path,
            }))?
        );
    } else {
        println!("{}", ui_heading("self-improve candidate"));
        print_kv_block(&[
            ("candidate", candidate_id.as_str()),
            ("run", candidate.run_id.as_str()),
            ("branch", candidate.branch.as_str()),
            ("status", candidate.status.as_str()),
            ("evidence score", &format!("{:.2}", eval.evidence_score)),
        ]);
        println!(
            "{} {}",
            ui_muted("next:"),
            ui_command(format!(
                "deadreckon improve self {} --pr-dry-run",
                proposal.proposal_id
            ))
        );
    }
    Ok(())
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
                repo: "/Users/gdc/deadreckon".to_string(),
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
        return Err(CliError::Core(deadreckon_core::user_error(
            "no self-improvement candidate evidence exists",
            &format!("deadreckon improve self {proposal_id} --yes"),
        )));
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
            CliError::Core(deadreckon_core::user_error(
                "no candidate evidence exists for this proposal",
                &format!("deadreckon improve self {proposal_id} --yes"),
            ))
        })
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
