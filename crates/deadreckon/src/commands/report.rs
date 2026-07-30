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
    // A report renders one run's evidence, so plans and chains redirect rather
    // than coming back as "run <id> not found" for an id that plainly exists.
    let resolved = super::reference::resolve_ref(
        &paths,
        super::reference::RefQuery {
            reference: Some(&args.run_id),
            all_scopes: false,
            verb: "report",
        },
    )?;
    let state = match resolved {
        super::reference::ResolvedRef::Job(view) => {
            return report_job_command(&paths, &view, args);
        }
        super::reference::ResolvedRef::Run(state) => *state,
        super::reference::ResolvedRef::PlanChild { state, .. } => *state,
        other => {
            return Err(super::reference::refusal_for(
                other.kind(),
                "report",
                &super::reference::resolved_id(&other),
            ));
        }
    };
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
        println!("{}", render_report_json(&view)?);
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

#[derive(Debug, Clone, Serialize)]
struct JobReport {
    id: String,
    goal: String,
    shape: deadreckon_protocol::JobShape,
    phase: deadreckon_protocol::JobPhase,
    outcome: Option<deadreckon_protocol::JobOutcome>,
    stop_reason: Option<deadreckon_protocol::StopReason>,
    lifecycle: JobLifecycleReport,
    contract: JobContractReport,
    deterministic_checks: Vec<deadreckon_core::AcceptanceCheckResult>,
    semantic: JobSemanticReport,
    attempts: Vec<JobAttemptReport>,
    resources: JobResourceReport,
    revisions: JobRevisionReport,
    receipt: JobReceiptReport,
    missing_evidence: MissingEvidenceReport,
}

#[derive(Debug, Clone, Serialize)]
struct JobLifecycleReport {
    attempt_count: u32,
    loaded_attempts: usize,
    lease_epoch: u64,
    last_event_sequence: u64,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize)]
struct JobContractReport {
    path: PathBuf,
    approved_sha256: Option<String>,
    current_sha256: Option<String>,
    matches_approved_digest: Option<bool>,
    accepted_by: Option<deadreckon_protocol::AuthorityAcceptedBy>,
    spec: Option<deadreckon_core::AcceptanceSpec>,
}

#[derive(Debug, Clone, Serialize)]
struct JobSemanticReport {
    lifecycle_decision: Option<deadreckon_protocol::SemanticDecision>,
    judgment: Option<deadreckon_protocol::SemanticJudgment>,
}

#[derive(Debug, Clone, Serialize)]
struct JobAttemptReport {
    run_id: String,
    status: RunStatus,
    provider: String,
    spend_usd: f64,
    wall_secs: Option<u64>,
    checks: Vec<deadreckon_core::AcceptanceCheckResult>,
}

#[derive(Debug, Clone, Serialize)]
struct JobResourceReport {
    recorded_spend_usd: f64,
    semantic_judge_spend_usd: Option<f64>,
    input_tokens: u64,
    output_tokens: u64,
    recorded_attempt_wall_secs: u64,
    lifecycle_elapsed_secs: Option<u64>,
    max_spend_usd: f64,
    max_wall_seconds: u64,
    max_attempts: u32,
}

#[derive(Debug, Clone, Serialize)]
struct JobRevisionReport {
    source_tree_sha256: Option<String>,
    source_revision: Option<String>,
    result_tree_sha256: Option<String>,
    result_revision: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct JobReceiptReport {
    path: PathBuf,
    status: &'static str,
    contained: Option<bool>,
    sandbox_backend: Option<String>,
    signature_validation_error: Option<String>,
    receipt: Option<deadreckon_protocol::CompletionReceipt>,
}

const MAX_MISSING_EVIDENCE_FACTS: usize = 20;

#[derive(Debug, Clone, Default, Serialize)]
struct MissingEvidenceReport {
    facts: Vec<String>,
    omitted: usize,
}

impl MissingEvidenceReport {
    fn push(&mut self, fact: impl Into<String>) {
        let fact = fact.into();
        if self.facts.iter().any(|existing| existing == &fact) {
            return;
        }
        if self.facts.len() < MAX_MISSING_EVIDENCE_FACTS {
            self.facts.push(fact);
        } else {
            self.omitted += 1;
        }
    }
}

fn report_job_command(
    paths: &DeadreckonPaths,
    view: &deadreckon_core::JobView,
    args: ReportCommandArgs,
) -> Result<()> {
    let report = build_job_report(paths, view);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let dest = args.dest.unwrap_or_else(|| {
        paths.job_dir(view.job.job_id.as_ref()).join(if args.html {
            "report.html"
        } else {
            "report.md"
        })
    });
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let rendered = if args.html {
        render_job_report_html(&report)
    } else {
        render_job_report_markdown(&report)
    };
    fs::write(&dest, rendered)?;
    if args.open {
        if !io::stdout().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "report --open requires an interactive terminal",
                &format!(
                    "deadreckon report {} --dest {}",
                    run_prefix(view.job.job_id.as_ref()),
                    dest.display()
                ),
            )));
        }
        open_path(&dest)?;
    }
    print!(
        "{}",
        job_report_surface(&report, &dest).render_plain(!crate::completion_hints_enabled(false))
    );
    Ok(())
}

fn build_job_report(paths: &DeadreckonPaths, view: &deadreckon_core::JobView) -> JobReport {
    let job_id = view.job.job_id.as_ref();
    let mut missing = MissingEvidenceReport::default();
    let authority_path = paths.job_authority(job_id);
    let authority =
        read_optional_json::<deadreckon_protocol::JobAuthority>(&authority_path, &mut missing);
    let contract_path = super::job::job_acceptance_path(paths, job_id);
    let contract = read_optional_contract(&contract_path, &mut missing);
    let current_contract_sha256 = deadreckon_core::flight::sha256_file(&contract_path).ok();
    let approved_contract_sha256 = authority
        .as_ref()
        .map(|authority| authority.contract_sha256.clone());
    let matches_approved_digest = approved_contract_sha256
        .as_ref()
        .zip(current_contract_sha256.as_ref())
        .map(|(approved, current)| approved == current);
    if matches_approved_digest == Some(false) {
        missing.push("frozen done contract no longer matches its approved digest");
    }

    let supporting = supporting_attempt_views(paths, view, &mut missing);
    let mut attempts = Vec::with_capacity(supporting.views.len());
    for attempt in &supporting.views {
        for artifact in attempt
            .missing
            .iter()
            .filter(|artifact| !matches!(artifact, deadreckon_core::Artifact::Doc(_)))
        {
            missing.push(format!(
                "attempt {} is missing essential artifact {:?}",
                attempt.id.run_id, artifact
            ));
        }
        attempts.push(JobAttemptReport {
            run_id: attempt.id.run_id.clone(),
            status: attempt.status,
            provider: attempt.provider.clone(),
            spend_usd: attempt.spend.total_usd,
            wall_secs: attempt.wall_secs,
            checks: attempt.proof.checks.clone(),
        });
    }
    if !view.missing_attempts.is_empty() {
        let examples = view
            .missing_attempts
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        missing.push(format!(
            "{} linked attempt(s) could not be loaded{}",
            view.missing_attempts.len(),
            if examples.is_empty() {
                String::new()
            } else {
                format!(": {examples}")
            }
        ));
    }
    let mut deterministic_checks = attempts
        .iter()
        .rev()
        .find(|attempt| !attempt.checks.is_empty())
        .map(|attempt| attempt.checks.clone())
        .unwrap_or_default();
    if deterministic_checks.is_empty()
        && let Ok(parent) = deadreckon_core::RunView::load(paths, &view.job.scope, job_id)
    {
        deterministic_checks = parent.proof.checks;
    }

    let semantic_lifecycle = semantic_lifecycle_decision(paths, job_id, &mut missing);
    let semantic_judgment = read_semantic_judgment(paths, view, &supporting.views, &mut missing);
    if semantic_judgment.is_none()
        && view.job.policy.semantic_judge == deadreckon_protocol::SemanticJudgeMode::Required
        && view.projection.is_terminal()
    {
        missing.push("required semantic judgment artifact is unavailable");
    }

    let receipt_path = paths.job_receipt(job_id);
    let receipt =
        read_optional_json::<deadreckon_protocol::CompletionReceipt>(&receipt_path, &mut missing);
    let (receipt_status, signature_validation_error) =
        validate_report_receipt(paths, view, receipt.as_ref(), &mut missing);
    let source_tree_sha256 = authority
        .as_ref()
        .map(|authority| authority.source_tree_sha256.clone());
    let source_revision = authority
        .as_ref()
        .and_then(|authority| authority.source_revision.clone());
    let result_tree_sha256 = receipt
        .as_ref()
        .map(|receipt| receipt.result_tree_sha256.clone());
    let result_revision = receipt
        .as_ref()
        .and_then(|receipt| receipt.result_revision.clone());

    let loaded_attempt_spend_usd = attempts.iter().map(|attempt| attempt.spend_usd).sum();
    let recorded_spend_usd = supporting
        .scheduler_spend_usd
        .unwrap_or(loaded_attempt_spend_usd);
    let input_tokens = supporting
        .views
        .iter()
        .map(|attempt| attempt.spend.input_tokens)
        .sum();
    let output_tokens = supporting
        .views
        .iter()
        .map(|attempt| attempt.spend.output_tokens)
        .sum();
    let recorded_attempt_wall_secs = supporting.scheduler_wall_secs.unwrap_or_else(|| {
        attempts
            .iter()
            .filter_map(|attempt| attempt.wall_secs)
            .sum()
    });
    let lifecycle_elapsed_secs = view.projection.updated_at.map(|updated_at| {
        u64::try_from(
            updated_at
                .signed_duration_since(view.job.created_at)
                .num_seconds()
                .max(0),
        )
        .unwrap_or(u64::MAX)
    });

    JobReport {
        id: job_id.to_string(),
        goal: view.job.goal.clone(),
        shape: view.job.shape,
        phase: view.projection.phase,
        outcome: view.projection.outcome,
        stop_reason: view.projection.stop_reason,
        lifecycle: JobLifecycleReport {
            attempt_count: view.projection.attempt_count,
            loaded_attempts: attempts.len(),
            lease_epoch: view.projection.current_lease_epoch,
            last_event_sequence: view.projection.last_sequence,
            created_at: view.job.created_at,
            updated_at: view.projection.updated_at,
        },
        contract: JobContractReport {
            path: contract_path,
            approved_sha256: approved_contract_sha256,
            current_sha256: current_contract_sha256,
            matches_approved_digest,
            accepted_by: authority.as_ref().map(|authority| authority.accepted_by),
            spec: contract,
        },
        deterministic_checks,
        semantic: JobSemanticReport {
            lifecycle_decision: semantic_lifecycle,
            judgment: semantic_judgment.clone(),
        },
        attempts,
        resources: JobResourceReport {
            recorded_spend_usd,
            semantic_judge_spend_usd: semantic_judgment
                .as_ref()
                .map(|judgment| judgment.spend_usd),
            input_tokens,
            output_tokens,
            recorded_attempt_wall_secs,
            lifecycle_elapsed_secs,
            max_spend_usd: view.job.policy.max_spend_usd,
            max_wall_seconds: view.job.policy.max_wall_seconds,
            max_attempts: view.job.policy.max_attempts,
        },
        revisions: JobRevisionReport {
            source_tree_sha256,
            source_revision,
            result_tree_sha256,
            result_revision,
        },
        receipt: JobReceiptReport {
            path: receipt_path,
            status: receipt_status,
            contained: receipt.as_ref().map(|receipt| receipt.contained),
            sandbox_backend: receipt
                .as_ref()
                .map(|receipt| receipt.sandbox_backend.clone()),
            signature_validation_error,
            receipt,
        },
        missing_evidence: missing,
    }
}

struct SupportingAttemptViews {
    views: Vec<deadreckon_core::RunView>,
    scheduler_spend_usd: Option<f64>,
    scheduler_wall_secs: Option<u64>,
}

fn supporting_attempt_views(
    paths: &DeadreckonPaths,
    view: &deadreckon_core::JobView,
    missing: &mut MissingEvidenceReport,
) -> SupportingAttemptViews {
    let mut views = view.attempts.clone();
    let mut seen = views
        .iter()
        .map(|attempt| attempt.id.run_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    match view.job.shape {
        deadreckon_protocol::JobShape::Graph => {
            let plan = match deadreckon_core::load_plan(paths, view.job.job_id.as_ref()) {
                Ok(plan) => plan,
                Err(error) => {
                    missing.push(format!(
                        "graph plan state is unavailable; child attempts and scheduler spend are incomplete: {error}"
                    ));
                    return SupportingAttemptViews {
                        views,
                        scheduler_spend_usd: None,
                        scheduler_wall_secs: None,
                    };
                }
            };
            let scheduler_spend_usd = Some(
                plan.tasks
                    .iter()
                    .map(|task| task.attempts_spend_usd())
                    .sum(),
            );
            let scheduler_wall_secs = Some(
                plan.tasks
                    .iter()
                    .map(|task| task.attempts_wall_seconds())
                    .sum::<f64>()
                    .ceil()
                    .clamp(0.0, u64::MAX as f64) as u64,
            );
            for task in &plan.tasks {
                let run_ids = task
                    .attempts
                    .iter()
                    .filter_map(|attempt| attempt.run_id.as_deref())
                    .chain(task.child_run_id.as_deref())
                    .collect::<Vec<_>>();
                let idless_attempts = task
                    .attempts
                    .iter()
                    .filter(|attempt| attempt.run_id.is_none())
                    .count();
                if idless_attempts > 0 {
                    missing.push(format!(
                        "graph task {} has {idless_attempts} recorded attempt(s) without a run ID",
                        task.task_id
                    ));
                }
                for run_id in run_ids {
                    if !seen.insert(run_id.to_string()) {
                        continue;
                    }
                    match load_run(paths, run_id)
                        .and_then(|state| deadreckon_core::RunView::from_state(&state))
                    {
                        Ok(attempt) => views.push(attempt),
                        Err(error) => missing.push(format!(
                            "graph task {} attempt {} could not be loaded: {error}",
                            task.task_id, run_id
                        )),
                    }
                }
            }
            SupportingAttemptViews {
                views,
                scheduler_spend_usd,
                scheduler_wall_secs,
            }
        }
        deadreckon_protocol::JobShape::LegacyCampaign => {
            missing.push(
                "campaign child attempt aggregation is unavailable; this report shows the durable parent lifecycle and any directly linked runs",
            );
            SupportingAttemptViews {
                views,
                scheduler_spend_usd: None,
                scheduler_wall_secs: None,
            }
        }
        deadreckon_protocol::JobShape::Single | deadreckon_protocol::JobShape::LegacyChain => {
            SupportingAttemptViews {
                views,
                scheduler_spend_usd: None,
                scheduler_wall_secs: None,
            }
        }
    }
}

fn read_optional_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    missing: &mut MissingEvidenceReport,
) -> Option<T> {
    match fs::read(path) {
        Ok(raw) => match serde_json::from_slice(&raw) {
            Ok(value) => Some(value),
            Err(error) => {
                missing.push(format!(
                    "{} is unreadable JSON: {error}",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("evidence")
                ));
                None
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            missing.push(format!("{} is absent", path.display()));
            None
        }
        Err(error) => {
            missing.push(format!("{} could not be read: {error}", path.display()));
            None
        }
    }
}

fn read_optional_contract(
    path: &Path,
    missing: &mut MissingEvidenceReport,
) -> Option<deadreckon_core::AcceptanceSpec> {
    match fs::read_to_string(path) {
        Ok(raw) => match serde_yaml::from_str(&raw) {
            Ok(spec) => Some(spec),
            Err(error) => {
                missing.push(format!("frozen done contract is unreadable YAML: {error}"));
                None
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            missing.push("frozen done contract is absent");
            None
        }
        Err(error) => {
            missing.push(format!("frozen done contract could not be read: {error}"));
            None
        }
    }
}

fn semantic_lifecycle_decision(
    paths: &DeadreckonPaths,
    job_id: &str,
    missing: &mut MissingEvidenceReport,
) -> Option<deadreckon_protocol::SemanticDecision> {
    let history = match deadreckon_core::read_job_history(&paths.job_events(job_id)) {
        Ok(history) => history,
        Err(error) => {
            missing.push(format!("job event history could not be read: {error}"));
            return None;
        }
    };
    history
        .events()
        .iter()
        .rev()
        .find_map(|event| match event.kind {
            deadreckon_protocol::JobEventKind::SemanticJudgeAchieved => {
                Some(deadreckon_protocol::SemanticDecision::Achieved)
            }
            deadreckon_protocol::JobEventKind::SemanticJudgeRevise => {
                Some(deadreckon_protocol::SemanticDecision::Revise)
            }
            deadreckon_protocol::JobEventKind::SemanticJudgeUncertain => {
                Some(deadreckon_protocol::SemanticDecision::Uncertain)
            }
            _ => None,
        })
}

fn read_semantic_judgment(
    paths: &DeadreckonPaths,
    view: &deadreckon_core::JobView,
    attempts: &[deadreckon_core::RunView],
    missing: &mut MissingEvidenceReport,
) -> Option<deadreckon_protocol::SemanticJudgment> {
    let job_id = view.job.job_id.as_ref();
    let mut seen = std::collections::BTreeSet::new();
    let candidate_run_ids = std::iter::once(job_id).chain(
        attempts
            .iter()
            .rev()
            .map(|attempt| attempt.id.run_id.as_str()),
    );
    for run_id in candidate_run_ids.filter(|run_id| seen.insert(*run_id)) {
        let Ok(state) = load_run(paths, run_id) else {
            continue;
        };
        let path = state.run_root.join(deadreckon_core::SEMANTIC_JUDGMENT_JSON);
        if !path.exists() {
            continue;
        }
        return read_optional_json(&path, missing);
    }
    None
}

fn validate_report_receipt(
    paths: &DeadreckonPaths,
    view: &deadreckon_core::JobView,
    receipt: Option<&deadreckon_protocol::CompletionReceipt>,
    missing: &mut MissingEvidenceReport,
) -> (&'static str, Option<String>) {
    let Some(receipt) = receipt else {
        if view.projection.outcome == Some(deadreckon_protocol::JobOutcome::Verified) {
            missing.push("verified lifecycle has no completion receipt");
        }
        return ("absent", None);
    };
    if receipt.job_id != view.job.job_id {
        let error = "receipt job identity does not match lifecycle job".to_string();
        missing.push(error.clone());
        return ("invalid", Some(error));
    }
    if receipt.run_id.as_ref() != view.job.job_id.as_ref() {
        let error =
            "receipt signature validation is unavailable without a same-ID result run".to_string();
        missing.push(error.clone());
        return ("unverified", Some(error));
    }
    let state = match load_run(paths, receipt.run_id.as_ref()) {
        Ok(state) => state,
        Err(error) => {
            let error = format!("same-ID leaf run state is unavailable: {error}");
            missing.push(error.clone());
            return ("unverified", Some(error));
        }
    };
    match deadreckon_core::validate_completion_receipt(paths, &state) {
        Ok(validated) if validated == *receipt => ("valid", None),
        Ok(_) => {
            let error = "validated receipt bytes differ from the reported receipt".to_string();
            missing.push(error.clone());
            ("invalid", Some(error))
        }
        Err(error) => {
            let error = error.to_string();
            missing.push(format!("completion receipt validation failed: {error}"));
            ("invalid", Some(error))
        }
    }
}

fn job_report_surface(report: &JobReport, dest: &Path) -> VerdictSurface {
    let short = run_prefix(&report.id);
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "report",
        Some(&short),
        ExplanationPanel::new(
            "DeadReckon wrote a static report from the durable Job lifecycle and its factual evidence.",
            "The report keeps lifecycle truth separate from supporting run attempts and records missing evidence explicitly.",
            vec![
                ("output", dest.display().to_string()),
                ("attempts", report.lifecycle.attempt_count.to_string()),
                ("checks", report.deterministic_checks.len().to_string()),
                ("receipt", report.receipt.status.to_string()),
            ],
        ),
        vec![("Recommended", format!("deadreckon show {short}"))],
        vec![("Secondary", format!("deadreckon report {short} --json"))],
    )
}

fn render_report_json(view: &deadreckon_core::RunView) -> serde_json::Result<String> {
    serde_json::to_string_pretty(view)
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

fn render_job_report_markdown(report: &JobReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# deadreckon job report: {}\n\n",
        run_prefix(&report.id)
    ));
    out.push_str("## Lifecycle\n\n");
    out.push_str(&format!(
        "- job id: {}\n- goal: {}\n- shape: {}\n- phase: {}\n- outcome: {}\n- stop reason: {}\n- attempts: {} recorded, {} loaded\n- lease epoch: {}\n- last event: {}\n",
        report.id,
        report.goal,
        serialized_report_label(report.shape),
        serialized_report_label(report.phase),
        report
            .outcome
            .map(serialized_report_label)
            .unwrap_or_else(|| "not terminal".to_string()),
        report
            .stop_reason
            .map(serialized_report_label)
            .unwrap_or_else(|| "none".to_string()),
        report.lifecycle.attempt_count,
        report.lifecycle.loaded_attempts,
        report.lifecycle.lease_epoch,
        report.lifecycle.last_event_sequence,
    ));

    out.push_str("\n## Approved definition of done\n\n");
    out.push_str(&format!(
        "- contract: {}\n- approved digest: {}\n- current digest: {}\n- matches approval: {}\n- accepted by: {}\n",
        report.contract.path.display(),
        optional_text(report.contract.approved_sha256.as_deref()),
        optional_text(report.contract.current_sha256.as_deref()),
        report
            .contract
            .matches_approved_digest
            .map(|matches| matches.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        report
            .contract
            .accepted_by
            .map(serialized_report_label)
            .unwrap_or_else(|| "unknown".to_string()),
    ));
    match report.contract.spec.as_ref() {
        Some(spec) => {
            out.push_str(&format!(
                "- name: {}\n",
                spec.name.as_deref().unwrap_or("unnamed")
            ));
            if spec.checks.is_empty() {
                out.push_str("- approved checks: none\n");
            }
            for (index, check) in spec.checks.iter().enumerate() {
                out.push_str(&format!(
                    "- approved check {}: {}\n",
                    index + 1,
                    serde_json::to_string(check)
                        .unwrap_or_else(|_| "\"unrenderable check\"".to_string())
                ));
            }
        }
        None => out.push_str("- approved checks: unavailable\n"),
    }

    out.push_str("\n## Deterministic check results\n\n");
    if report.deterministic_checks.is_empty() {
        out.push_str("- no deterministic check results recorded\n");
    }
    for check in &report.deterministic_checks {
        out.push_str(&format!(
            "- {} {}: {}\n",
            if check.passed { "pass" } else { "fail" },
            check.kind,
            check.detail
        ));
    }

    out.push_str("\n## Semantic judgment\n\n");
    out.push_str(&format!(
        "- lifecycle decision: {}\n",
        report
            .semantic
            .lifecycle_decision
            .map(serialized_report_label)
            .unwrap_or_else(|| "not recorded".to_string())
    ));
    if let Some(judgment) = report.semantic.judgment.as_ref() {
        out.push_str(&format!(
            "- artifact decision: {}\n- provider: {}\n- model: {}\n- summary: {}\n- judge spend: ${:.4}\n",
            serialized_report_label(judgment.decision),
            judgment.provider,
            judgment.model,
            judgment.summary,
            judgment.spend_usd,
        ));
        for coverage in &judgment.goal_coverage {
            out.push_str(&format!(
                "- goal coverage {}: {} ({})\n",
                serialized_report_label(coverage.status),
                coverage.claim,
                if coverage.evidence.is_empty() {
                    "no cited evidence".to_string()
                } else {
                    coverage.evidence.join("; ")
                }
            ));
        }
        for missing in &judgment.missing {
            out.push_str(&format!("- judge says missing: {missing}\n"));
        }
    } else {
        out.push_str("- judgment artifact: unavailable\n");
    }

    out.push_str("\n## Attempts and resources\n\n");
    if report.attempts.is_empty() {
        out.push_str("- no backing run attempt is available\n");
    }
    for attempt in &report.attempts {
        out.push_str(&format!(
            "- run {}: {}; provider {}; spend ${:.4}; wall {}\n",
            attempt.run_id,
            run_status_label(attempt.status),
            attempt.provider,
            attempt.spend_usd,
            attempt
                .wall_secs
                .map(|seconds| format!("{seconds}s"))
                .unwrap_or_else(|| "unknown".to_string())
        ));
    }
    out.push_str(&format!(
        "- recorded spend: ${:.4} (limit ${:.4})\n- recorded tokens: {} input, {} output\n- recorded attempt wall: {}s (limit {}s)\n- lifecycle elapsed: {}\n- attempt limit: {}\n",
        report.resources.recorded_spend_usd,
        report.resources.max_spend_usd,
        report.resources.input_tokens,
        report.resources.output_tokens,
        report.resources.recorded_attempt_wall_secs,
        report.resources.max_wall_seconds,
        report
            .resources
            .lifecycle_elapsed_secs
            .map(|seconds| format!("{seconds}s"))
            .unwrap_or_else(|| "unknown".to_string()),
        report.resources.max_attempts,
    ));

    out.push_str("\n## Revisions and receipt\n\n");
    out.push_str(&format!(
        "- source revision: {}\n- source tree: {}\n- result revision: {}\n- result tree: {}\n- receipt: {} ({})\n- receipt contained: {}\n- sandbox backend: {}\n",
        optional_text(report.revisions.source_revision.as_deref()),
        optional_text(report.revisions.source_tree_sha256.as_deref()),
        optional_text(report.revisions.result_revision.as_deref()),
        optional_text(report.revisions.result_tree_sha256.as_deref()),
        report.receipt.status,
        report.receipt.path.display(),
        report
            .receipt
            .contained
            .map(|contained| contained.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        optional_text(report.receipt.sandbox_backend.as_deref()),
    ));
    if let Some(error) = report.receipt.signature_validation_error.as_deref() {
        out.push_str(&format!("- receipt validation detail: {error}\n"));
    }

    out.push_str("\n## Missing evidence\n\n");
    if report.missing_evidence.facts.is_empty() && report.missing_evidence.omitted == 0 {
        out.push_str("- none\n");
    } else {
        for fact in &report.missing_evidence.facts {
            out.push_str(&format!("- {fact}\n"));
        }
        if report.missing_evidence.omitted > 0 {
            out.push_str(&format!(
                "- {} additional missing-evidence fact(s) omitted\n",
                report.missing_evidence.omitted
            ));
        }
    }
    out
}

fn optional_text(value: Option<&str>) -> &str {
    value.unwrap_or("unavailable")
}

fn serialized_report_label<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToString::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

pub(crate) fn render_report_html(view: &deadreckon_core::RunView) -> String {
    let markdown = render_report_markdown(view);
    render_markdown_as_html(&markdown)
}

fn render_job_report_html(report: &JobReport) -> String {
    let markdown = render_job_report_markdown(report);
    render_markdown_as_html(&markdown)
}

fn render_markdown_as_html(markdown: &str) -> String {
    let mut html = String::new();
    html.push_str(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>deadreckon report</title>",
    );
    html.push_str("<style>body{font-family:system-ui,-apple-system,BlinkMacSystemFont,sans-serif;max-width:960px;margin:40px auto;padding:0 24px;line-height:1.5;color:#17202a}pre{white-space:pre-wrap;background:#f6f8fa;padding:16px;border-radius:6px}h1,h2{line-height:1.2}</style>");
    html.push_str("</head><body><pre>");
    html.push_str(&escape_html(markdown));
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

    fn factual_job_report() -> JobReport {
        use deadreckon_protocol::{
            AuthorityAcceptedBy, CompletionProofKind, CompletionReceipt, CompletionReceiptIssuer,
            GoalCoverage, GoalCoverageStatus, JobId, JobOutcome, JobPhase, JobSchemaVersion,
            JobShape, RunId, SemanticDecision, SemanticJudgment, StopReason,
        };

        let job_id = JobId("job-report-123456".to_string());
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            run_id: RunId(job_id.as_ref().to_string()),
            judged_at: chrono::Utc::now(),
            provider: "judge-provider".to_string(),
            model: "judge-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the implementation satisfies the approved goal".to_string(),
            goal_coverage: vec![GoalCoverage {
                claim: "README exists".to_string(),
                status: GoalCoverageStatus::Met,
                evidence: vec!["deterministic check 1".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: "semantic-input".to_string(),
            spend_usd: 0.02,
        };
        let receipt = CompletionReceipt {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            run_id: RunId(job_id.as_ref().to_string()),
            issued_at: chrono::Utc::now(),
            issuer: CompletionReceiptIssuer::DeadreckonSupervisor,
            proof_kind: CompletionProofKind::TwoKeyCompletion,
            outcome: JobOutcome::Verified,
            stop_reason: StopReason::Verified,
            authority_sha256: "authority".to_string(),
            goal_sha256: "goal".to_string(),
            contract_sha256: "contract-approved".to_string(),
            effective_policy_sha256: "policy".to_string(),
            launch_plan_sha256: "launch".to_string(),
            source_tree_sha256: "source-tree".to_string(),
            source_revision: Some("source-revision".to_string()),
            result_tree_sha256: "result-tree".to_string(),
            result_revision: Some("result-revision".to_string()),
            deterministic_marker_sha256: "marker".to_string(),
            semantic_judgment_sha256: "semantic".to_string(),
            contained: true,
            sandbox_backend: "sandbox-exec".to_string(),
            signature: "signature".to_string(),
        };
        let check = deadreckon_core::AcceptanceCheckResult {
            kind: "file_exists".to_string(),
            passed: true,
            must_pass: true,
            detail: "README.md exists".to_string(),
            command: None,
            cwd: None,
            duration_ms: Some(3),
            stdout: None,
            stderr: None,
        };
        JobReport {
            id: job_id.as_ref().to_string(),
            goal: "ensure README exists".to_string(),
            shape: JobShape::Single,
            phase: JobPhase::Terminal,
            outcome: Some(JobOutcome::Verified),
            stop_reason: Some(StopReason::Verified),
            lifecycle: JobLifecycleReport {
                attempt_count: 1,
                loaded_attempts: 1,
                lease_epoch: 1,
                last_event_sequence: 10,
                created_at: chrono::Utc::now(),
                updated_at: Some(chrono::Utc::now()),
            },
            contract: JobContractReport {
                path: PathBuf::from("/evidence/acceptance.yaml"),
                approved_sha256: Some("contract-approved".to_string()),
                current_sha256: Some("contract-approved".to_string()),
                matches_approved_digest: Some(true),
                accepted_by: Some(AuthorityAcceptedBy::Operator),
                spec: Some(deadreckon_core::AcceptanceSpec {
                    name: Some("README contract".to_string()),
                    checks: vec![deadreckon_core::AcceptanceCheck::FileExists {
                        path: "README.md".to_string(),
                        must_pass: true,
                    }],
                }),
            },
            deterministic_checks: vec![check.clone()],
            semantic: JobSemanticReport {
                lifecycle_decision: Some(SemanticDecision::Achieved),
                judgment: Some(judgment),
            },
            attempts: vec![JobAttemptReport {
                run_id: job_id.as_ref().to_string(),
                status: RunStatus::Completed,
                provider: "smoke".to_string(),
                spend_usd: 0.12,
                wall_secs: Some(9),
                checks: vec![check],
            }],
            resources: JobResourceReport {
                recorded_spend_usd: 0.12,
                semantic_judge_spend_usd: Some(0.02),
                input_tokens: 120,
                output_tokens: 45,
                recorded_attempt_wall_secs: 9,
                lifecycle_elapsed_secs: Some(10),
                max_spend_usd: 2.0,
                max_wall_seconds: 300,
                max_attempts: 3,
            },
            revisions: JobRevisionReport {
                source_tree_sha256: Some("source-tree".to_string()),
                source_revision: Some("source-revision".to_string()),
                result_tree_sha256: Some("result-tree".to_string()),
                result_revision: Some("result-revision".to_string()),
            },
            receipt: JobReceiptReport {
                path: PathBuf::from("/evidence/receipt.json"),
                status: "valid",
                contained: Some(true),
                sandbox_backend: Some("sandbox-exec".to_string()),
                signature_validation_error: None,
                receipt: Some(receipt),
            },
            missing_evidence: MissingEvidenceReport::default(),
        }
    }

    #[test]
    fn report_cites_contract_checks_semantic_attempts_spend_and_revisions() {
        let report = render_job_report_markdown(&factual_job_report());

        for evidence in [
            "README contract",
            "approved check 1",
            "pass file_exists: README.md exists",
            "artifact decision: achieved",
            "run job-report-123456",
            "recorded spend: $0.1200",
            "source revision: source-revision",
            "result revision: result-revision",
            "receipt: valid",
            "receipt contained: true",
        ] {
            assert!(report.contains(evidence), "missing {evidence:?}\n{report}");
        }
    }

    #[test]
    fn missing_optional_narrative_does_not_break_factual_receipt() {
        let report = factual_job_report();

        let markdown = render_job_report_markdown(&report);
        let json = serde_json::to_value(&report).expect("job report JSON");

        assert!(markdown.contains("receipt: valid"), "{markdown}");
        assert!(
            markdown.contains("artifact decision: achieved"),
            "{markdown}"
        );
        assert!(!markdown.contains("narrative"), "{markdown}");
        assert_eq!(json.pointer("/receipt/status"), Some(&json!("valid")));
        assert_eq!(json.pointer("/receipt/contained"), Some(&json!(true)));
    }

    #[test]
    fn graph_report_loads_plan_task_attempts_and_scheduler_spend() {
        use deadreckon_core::plan::TaskAttempt;
        use deadreckon_core::{
            Plan, PlanMode, PlanProviders, PlanRole, PlanTask, RunOptions, create_run, save_plan,
        };
        use deadreckon_protocol::{
            Job, JobId, JobPolicy, JobSchemaVersion, JobShape, SemanticJudgeMode,
        };

        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let paths = DeadreckonPaths::from_home(home.path());
        let child = create_run(
            &paths,
            RunOptions {
                goal: "child task".to_string(),
                cwd: cwd.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("child-report-123456".to_string()),
                codebase: None,
            },
        )
        .expect("child run");
        let mut first = PlanTask::new(0, "first", "child task", PlanRole::Coder, None);
        let mut attempt = TaskAttempt::new(1, Some(child.run_id.clone()));
        attempt.status = deadreckon_core::PlanTaskStatus::Completed;
        attempt.finished_at = Some(attempt.started_at + chrono::Duration::seconds(7));
        attempt.spend_usd = 0.37;
        first.attempts.push(attempt);
        first.child_run_id = Some(child.run_id.clone());
        first.child_scope = Some(child.scope.clone());
        let second = PlanTask::new(1, "second", "later task", PlanRole::Reviewer, None);
        let mut plan = Plan::new(
            "graph goal",
            PlanMode::FullPlan,
            vec![first, second],
            PlanProviders::default(),
            None,
            "0.0.0",
        )
        .expect("plan");
        plan.plan_id = "graph-report-123456".to_string();
        save_plan(&paths, &plan).expect("save plan");

        let job_id = JobId(plan.plan_id.clone());
        let view = deadreckon_core::JobView {
            job: Job {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job_id.clone(),
                scope: child.scope,
                goal: "graph goal".to_string(),
                shape: JobShape::Graph,
                created_at: chrono::Utc::now(),
                source_cwd: cwd.path().to_path_buf(),
                launch_plan_sha256: "launch".to_string(),
                authority_sha256: "authority".to_string(),
                policy: JobPolicy {
                    max_spend_usd: 2.0,
                    max_wall_seconds: 120,
                    max_attempts: 2,
                    deadline: None,
                    semantic_judge: SemanticJudgeMode::Required,
                    execution: None,
                },
            },
            projection: deadreckon_core::JobProjection {
                schema_version: JobSchemaVersion::CURRENT,
                job_id,
                phase: deadreckon_protocol::JobPhase::Running,
                outcome: None,
                stop_reason: None,
                last_sequence: 5,
                current_lease_epoch: 1,
                attempt_count: 1,
                child_run_ids: Vec::new(),
                delivery: None,
                updated_at: Some(chrono::Utc::now()),
                caveats: Vec::new(),
            },
            attempts: Vec::new(),
            missing_attempts: Vec::new(),
        };
        let mut missing = MissingEvidenceReport::default();

        let supporting = supporting_attempt_views(&paths, &view, &mut missing);

        assert_eq!(supporting.views.len(), 1);
        assert_eq!(supporting.views[0].id.run_id, child.run_id);
        assert_eq!(supporting.scheduler_spend_usd, Some(0.37));
        assert_eq!(supporting.scheduler_wall_secs, Some(7));
    }

    #[test]
    fn report_json_validates_against_generated_schema() {
        let schema = schemars::schema_for!(deadreckon_core::RunView);
        let rendered = serde_json::to_value(&schema).expect("serialize generated RunView schema");
        let report =
            serde_json::from_str(&render_report_json(&minimal_view()).expect("render report JSON"))
                .expect("report renderer must emit JSON");

        assert_json_matches_schema(&report, &rendered, &rendered);
        let mut invalid_report = report.clone();
        invalid_report
            .as_object_mut()
            .expect("RunView JSON object")
            .remove("goal");
        assert!(
            json_matches_schema(&invalid_report, &rendered, &rendered, "$").is_err(),
            "schema validator must reject a report missing a required field"
        );

        let checked_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("docs/schemas/projections/run-view.schema.json");
        let update_command = "DEADRECKON_UPDATE_SCHEMAS=1 cargo test -p deadreckon report_json_validates_against_generated_schema";
        if std::env::var_os("DEADRECKON_UPDATE_SCHEMAS").as_deref() == Some("1".as_ref()) {
            fs::create_dir_all(checked_path.parent().unwrap()).unwrap();
            let mut bytes = serde_json::to_vec_pretty(&rendered).unwrap();
            bytes.push(b'\n');
            fs::write(&checked_path, bytes).unwrap();
        }
        let checked: serde_json::Value = serde_json::from_slice(
            &fs::read(&checked_path).unwrap_or_else(|error| {
                panic!(
                    "checked RunView schema must exist: {error}\n\nregenerate it with:\n  {update_command}"
                )
            }),
        )
        .expect("checked RunView schema must be JSON");
        assert_eq!(
            checked, rendered,
            "checked RunView schema drifted\n\nregenerate it with:\n  {update_command}"
        );
    }

    fn assert_json_matches_schema(
        instance: &serde_json::Value,
        schema: &serde_json::Value,
        root: &serde_json::Value,
    ) {
        if let Err(error) = json_matches_schema(instance, schema, root, "$") {
            panic!("report JSON does not match generated RunView schema: {error}");
        }
    }

    fn json_matches_schema(
        instance: &serde_json::Value,
        schema: &serde_json::Value,
        root: &serde_json::Value,
        path: &str,
    ) -> std::result::Result<(), String> {
        if let Some(allowed) = schema.as_bool() {
            return allowed
                .then_some(())
                .ok_or_else(|| format!("{path}: rejected by false schema"));
        }
        let object = schema
            .as_object()
            .ok_or_else(|| format!("{path}: schema is not an object"))?;

        if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str) {
            let pointer = reference.strip_prefix('#').ok_or_else(|| {
                format!("{path}: unsupported external schema reference {reference}")
            })?;
            let target = root
                .pointer(pointer)
                .ok_or_else(|| format!("{path}: unresolved schema reference {reference}"))?;
            return json_matches_schema(instance, target, root, path);
        }

        if let Some(values) = object.get("enum").and_then(serde_json::Value::as_array)
            && !values.contains(instance)
        {
            return Err(format!("{path}: value is outside schema enum"));
        }

        validate_subschemas(instance, object, root, path)?;

        if let Some(expected) = object.get("type")
            && !schema_type_matches(instance, expected)
        {
            return Err(format!(
                "{path}: expected schema type {expected}, got {instance}"
            ));
        }

        if let Some(minimum) = object.get("minimum").and_then(serde_json::Value::as_f64)
            && instance.as_f64().is_some_and(|number| number < minimum)
        {
            return Err(format!("{path}: number is below schema minimum {minimum}"));
        }

        if let Some(instance_object) = instance.as_object() {
            if let Some(required) = object.get("required").and_then(serde_json::Value::as_array) {
                for key in required.iter().filter_map(serde_json::Value::as_str) {
                    if !instance_object.contains_key(key) {
                        return Err(format!("{path}: missing required property {key:?}"));
                    }
                }
            }
            let properties = object
                .get("properties")
                .and_then(serde_json::Value::as_object);
            for (key, value) in instance_object {
                let child_path = format!("{path}.{key}");
                if let Some(child_schema) = properties.and_then(|values| values.get(key)) {
                    json_matches_schema(value, child_schema, root, &child_path)?;
                } else if let Some(additional) = object.get("additionalProperties") {
                    json_matches_schema(value, additional, root, &child_path)?;
                }
            }
        }

        if let Some(instance_array) = instance.as_array()
            && let Some(items) = object.get("items")
        {
            if let Some(tuple) = items.as_array() {
                for (index, (value, child_schema)) in
                    instance_array.iter().zip(tuple.iter()).enumerate()
                {
                    json_matches_schema(value, child_schema, root, &format!("{path}[{index}]"))?;
                }
            } else {
                for (index, value) in instance_array.iter().enumerate() {
                    json_matches_schema(value, items, root, &format!("{path}[{index}]"))?;
                }
            }
        }

        Ok(())
    }

    fn validate_subschemas(
        instance: &serde_json::Value,
        schema: &serde_json::Map<String, serde_json::Value>,
        root: &serde_json::Value,
        path: &str,
    ) -> std::result::Result<(), String> {
        if let Some(all_of) = schema.get("allOf").and_then(serde_json::Value::as_array) {
            for child in all_of {
                json_matches_schema(instance, child, root, path)?;
            }
        }
        for keyword in ["anyOf", "oneOf"] {
            let Some(children) = schema.get(keyword).and_then(serde_json::Value::as_array) else {
                continue;
            };
            let matches = children
                .iter()
                .filter(|child| json_matches_schema(instance, child, root, path).is_ok())
                .count();
            let valid = if keyword == "oneOf" {
                matches == 1
            } else {
                matches >= 1
            };
            if !valid {
                return Err(format!(
                    "{path}: matched {matches} branches of schema keyword {keyword}"
                ));
            }
        }
        if let Some(not_schema) = schema.get("not")
            && json_matches_schema(instance, not_schema, root, path).is_ok()
        {
            return Err(format!("{path}: matched forbidden schema"));
        }
        Ok(())
    }

    fn schema_type_matches(instance: &serde_json::Value, expected: &serde_json::Value) -> bool {
        match expected {
            serde_json::Value::String(kind) => instance_matches_type(instance, kind),
            serde_json::Value::Array(kinds) => kinds.iter().any(|kind| {
                kind.as_str()
                    .is_some_and(|kind| instance_matches_type(instance, kind))
            }),
            _ => false,
        }
    }

    fn instance_matches_type(instance: &serde_json::Value, kind: &str) -> bool {
        match kind {
            "null" => instance.is_null(),
            "boolean" => instance.is_boolean(),
            "object" => instance.is_object(),
            "array" => instance.is_array(),
            "number" => instance.is_number(),
            "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
            "string" => instance.is_string(),
            _ => false,
        }
    }
}
