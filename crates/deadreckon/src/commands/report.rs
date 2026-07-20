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
