use super::super::*;

#[derive(Debug, Clone, Serialize)]
struct SalvageReport {
    schema_version: u32,
    source_job_id: String,
    source_job_phase: deadreckon_protocol::JobPhase,
    source_job_outcome: Option<deadreckon_protocol::JobOutcome>,
    source_job_stop_reason: Option<deadreckon_protocol::StopReason>,
    source_job_unchanged: bool,
    candidate_initial_revision: String,
    candidate_head_revision: String,
    candidate_head_tree: String,
    completed_applications: usize,
    validated_child_receipts: usize,
    source_artifacts_sha256: std::collections::BTreeMap<String, String>,
    acceptance_spec_sha256: String,
    job_history_sha256: String,
    plan_sha256: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence: Option<PathBuf>,
    next_action: String,
}

pub(crate) fn salvage_command(
    job_ref: &str,
    output: Option<&Path>,
    dry_run: bool,
    json_output: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let resolved = super::reference::resolve_ref(
        &paths,
        super::reference::RefQuery {
            reference: Some(job_ref),
            all_scopes: true,
            verb: "salvage",
        },
    )?;
    let super::reference::ResolvedRef::Job(view) = resolved else {
        return Err(CliError::Core(deadreckon_core::user_error(
            "salvage requires a durable failed Graph Job",
            "pass the Job id shown by `deadreckon status --json`",
        )));
    };
    if view.job.shape != deadreckon_protocol::JobShape::Graph
        || !view.projection.is_terminal()
        || view.projection.outcome != Some(deadreckon_protocol::JobOutcome::Failed)
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "Job {} is not a terminal failed Graph Job",
                run_prefix(view.job.job_id.as_ref())
            ),
            "inspect it with `deadreckon status <job-id> --json`; salvage never bypasses an active or successful lifecycle",
        )));
    }

    let authority_path = paths.job_authority(view.job.job_id.as_ref());
    let authority_sha256 = deadreckon_core::flight::sha256_file(&authority_path)?;
    if authority_sha256 != view.job.authority_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Job {} authority changed after approval",
            view.job.job_id
        ))));
    }
    let authority: deadreckon_protocol::JobAuthority =
        serde_json::from_slice(&fs::read(&authority_path)?).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: authority_path,
                source,
            })
        })?;
    if authority.job_id != view.job.job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Job {} authority names another Job",
            view.job.job_id
        ))));
    }

    let plan = load_plan(&paths, view.job.job_id.as_ref())?;
    if plan.status == PlanStatus::Merged || plan.merged_run_id.is_some() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("Job {} already has a merged Plan result", view.job.job_id),
            "use `deadreckon finish <job-id>` for a completed result",
        )));
    }
    if let Some(task) = plan
        .tasks
        .iter()
        .find(|task| !task.status.is_successful_terminal())
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "Job {} is not salvageable because {} is {:?}",
                view.job.job_id, task.task_id, task.status
            ),
            "inspect the failed child; salvage never omits an unfinished Plan node",
        )));
    }

    let inspection =
        super::graph_job::inspect_ordered_candidate_for_salvage(&paths, &view.job, &authority)?;
    let job_dir = paths.job_dir(view.job.job_id.as_ref());
    let acceptance_path = job_dir.join("acceptance.yaml");
    if !acceptance_path.is_file() {
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "frozen acceptance contract for Job {}",
            view.job.job_id
        ))));
    }
    let source_artifacts_sha256 = salvage_source_artifact_hashes(&paths, view.job.job_id.as_ref())?;
    let acceptance_spec_sha256 = source_artifacts_sha256["acceptance.yaml"].clone();
    let job_history_sha256 = source_artifacts_sha256["events.jsonl"].clone();
    let plan_sha256 = source_artifacts_sha256["plan.json"].clone();
    let validated_child_receipts = inspection.completed_applications.len();

    if dry_run {
        let next_action = format!(
            "deadreckon salvage {} --output <new-directory>",
            run_prefix(view.job.job_id.as_ref())
        );
        let report = SalvageReport {
            schema_version: 1,
            source_job_id: view.job.job_id.to_string(),
            source_job_phase: view.projection.phase,
            source_job_outcome: view.projection.outcome,
            source_job_stop_reason: view.projection.stop_reason,
            source_job_unchanged: true,
            candidate_initial_revision: inspection.initial_revision,
            candidate_head_revision: inspection.head_revision,
            candidate_head_tree: inspection.head_tree,
            completed_applications: inspection.completed_application_count,
            validated_child_receipts,
            source_artifacts_sha256,
            acceptance_spec_sha256,
            job_history_sha256,
            plan_sha256,
            status: "salvageable",
            output: None,
            evidence: None,
            next_action,
        };
        return print_salvage_report(&report, json_output);
    }

    let output = output.ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "salvage needs a new output directory",
            "pass `--output <new-directory>` or inspect first with `--dry-run`",
        ))
    })?;
    let output = absolute_new_output(output)?;
    if output.exists() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("salvage output already exists: {}", output.display()),
            "choose a new directory; salvage never overwrites existing work",
        )));
    }
    let output_parent = output.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "salvage output has no parent: {}",
            output.display()
        )))
    })?;
    fs::create_dir_all(output_parent)?;
    let output_parent = fs::canonicalize(output_parent)?;
    let output_name = output.file_name().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "salvage output has no directory name: {}",
            output.display()
        )))
    })?;
    let output = output_parent.join(output_name);
    let candidate = fs::canonicalize(&inspection.workspace)?;
    let job_dir_canonical = fs::canonicalize(&job_dir)?;
    if output_parent.starts_with(&candidate) || output_parent.starts_with(&job_dir_canonical) {
        return Err(CliError::Core(deadreckon_core::user_error(
            "salvage output cannot be placed inside its evidence source",
            "choose a new directory outside the Job and ordered-candidate directories",
        )));
    }

    let staging = tempfile::Builder::new()
        .prefix(".deadreckon-salvage-")
        .tempdir_in(&output_parent)?;
    deadreckon_core::copy_deliverable_tree(&inspection.workspace, staging.path())?;
    let control_dir = staging.path().join(".deadreckon");
    fs::create_dir_all(&control_dir)?;
    copy_artifact_path(&acceptance_path, &control_dir.join("acceptance.yaml"))?;
    for relative in ["acceptance.md", "acceptance"] {
        let source = job_dir.join(relative);
        if source.exists() {
            copy_salvage_control_artifact(&source, &control_dir.join(relative))?;
        }
    }
    assert_same_deliverable_tree(&inspection.workspace, staging.path())?;
    let final_inspection =
        super::graph_job::inspect_ordered_candidate_for_salvage(&paths, &view.job, &authority)?;
    if final_inspection != inspection
        || salvage_source_artifact_hashes(&paths, view.job.job_id.as_ref())?
            != source_artifacts_sha256
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Job {} salvage evidence changed during export",
            view.job.job_id
        ))));
    }

    let evidence_relative = PathBuf::from(".deadreckon/salvage.json");
    let evidence_path = output.join(&evidence_relative);
    let next_action = format!(
        "deadreckon acceptance check --spec {} --against {}",
        output.join(".deadreckon/acceptance.yaml").display(),
        output.display()
    );
    let report = SalvageReport {
        schema_version: 1,
        source_job_id: view.job.job_id.to_string(),
        source_job_phase: view.projection.phase,
        source_job_outcome: view.projection.outcome,
        source_job_stop_reason: view.projection.stop_reason,
        source_job_unchanged: true,
        candidate_initial_revision: inspection.initial_revision,
        candidate_head_revision: inspection.head_revision,
        candidate_head_tree: inspection.head_tree,
        completed_applications: inspection.completed_application_count,
        validated_child_receipts,
        source_artifacts_sha256,
        acceptance_spec_sha256,
        job_history_sha256,
        plan_sha256,
        status: "exported_unverified",
        output: Some(output.clone()),
        evidence: Some(evidence_path),
        next_action,
    };
    fs::write(
        staging.path().join(&evidence_relative),
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    let staging = staging.keep();
    fs::rename(&staging, &output).map_err(|source| {
        let _ = fs::remove_dir_all(&staging);
        CliError::Core(DeadreckonError::Io {
            path: output.clone(),
            source,
        })
    })?;
    assert_same_deliverable_tree(&inspection.workspace, &output)?;
    print_salvage_report(&report, json_output)
}

fn salvage_source_artifact_hashes(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<std::collections::BTreeMap<String, String>> {
    let artifacts = [
        ("job.json", paths.job_json(job_id)),
        ("projection.json", paths.job_projection(job_id)),
        ("events.jsonl", paths.job_events(job_id)),
        ("authority.json", paths.job_authority(job_id)),
        ("launch-plan.json", paths.job_launch_plan(job_id)),
        (
            "acceptance.yaml",
            paths.job_dir(job_id).join("acceptance.yaml"),
        ),
        ("plan.json", paths.plan_json(job_id)),
        (
            "ordered-candidate.json",
            paths.job_dir(job_id).join("ordered-candidate.json"),
        ),
        (
            "ordered-candidate-applications.jsonl",
            deadreckon_core::plan::ordered_candidate_application_events_path(paths, job_id),
        ),
    ];
    artifacts
        .into_iter()
        .map(|(name, path)| {
            deadreckon_core::flight::sha256_file(&path)
                .map(|digest| (name.to_string(), digest))
                .map_err(CliError::from)
        })
        .collect()
}

fn copy_salvage_control_artifact(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_dir() {
        deadreckon_core::artifacts::copy_tree(source, target)?;
    } else {
        copy_artifact_path(source, target)?;
    }
    Ok(())
}

fn absolute_new_output(output: &Path) -> Result<PathBuf> {
    if output.as_os_str().is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "salvage output cannot be empty".to_string(),
        )));
    }
    if output.is_absolute() {
        Ok(output.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(output))
    }
}

fn assert_same_deliverable_tree(source: &Path, recovered: &Path) -> Result<()> {
    let source_index = build_deliverable_file_index(source)?;
    let recovered_index = build_deliverable_file_index(recovered)?;
    if source_index.files != recovered_index.files {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "salvage copy changed the ordered candidate deliverable tree".to_string(),
        )));
    }
    Ok(())
}

fn print_salvage_report(report: &SalvageReport, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!("salvage {}", report.status);
        println!("job {}", report.source_job_id);
        println!("candidate {}", report.candidate_head_revision);
        println!("tree {}", report.candidate_head_tree);
        println!(
            "children {}/{} receipts validated",
            report.validated_child_receipts, report.completed_applications
        );
        if let Some(output) = report.output.as_ref() {
            println!("output {}", output.display());
        }
        println!("source job unchanged true");
        println!("next {}", report.next_action);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deliverable_comparison_ignores_recovery_control_pack_but_detects_code_changes() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let recovered = temp.path().join("recovered");
        fs::create_dir_all(source.join("src")).expect("source dirs");
        fs::create_dir_all(recovered.join("src")).expect("recovered dirs");
        fs::write(source.join("src/app.js"), "same\n").expect("source");
        fs::write(recovered.join("src/app.js"), "same\n").expect("recovered");
        fs::create_dir_all(recovered.join(".deadreckon/acceptance")).expect("control dir");
        fs::write(
            recovered.join(".deadreckon/acceptance/helper.sh"),
            "#!/bin/sh\n",
        )
        .expect("helper");
        assert_same_deliverable_tree(&source, &recovered).expect("same deliverables");

        fs::write(recovered.join("src/app.js"), "changed\n").expect("tamper");
        assert!(assert_same_deliverable_tree(&source, &recovered).is_err());
    }

    #[test]
    fn salvage_control_pack_copies_a_helper_directory() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let source = temp.path().join("acceptance");
        let target = temp.path().join("control/acceptance");
        fs::create_dir_all(source.join("nested")).expect("source dirs");
        fs::write(source.join("nested/check.sh"), "#!/bin/sh\n").expect("helper");

        copy_salvage_control_artifact(&source, &target).expect("control pack");

        assert_eq!(
            fs::read_to_string(target.join("nested/check.sh")).expect("copied helper"),
            "#!/bin/sh\n"
        );
    }
}
