use super::super::*;

#[derive(Debug)]
pub(crate) struct ImportCommandOptions {
    pub(crate) source: String,
    pub(crate) preview: bool,
    pub(crate) list: bool,
    pub(crate) session: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) all: bool,
    pub(crate) since: Option<String>,
    pub(crate) replace: bool,
    pub(crate) json: bool,
}

#[derive(Debug, Clone)]
enum ImportSourceKind {
    Provider,
    Cursor,
}

#[derive(Debug, Clone)]
struct ResolvedImportSource {
    alias: String,
    source: String,
    schema: String,
    storage: IngestStorage,
    roots: Vec<PathBuf>,
    cwd_match: IngestCwdMatch,
    cwd_match_path: Option<String>,
    file_glob: Option<String>,
    id_prefix: Option<String>,
    kind: ImportSourceKind,
}

#[derive(Debug, Clone, Serialize)]
struct ImportCandidate {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    paths: Vec<PathBuf>,
    root: PathBuf,
    updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    row_count_hint: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ImportMode {
    Session,
    All,
}

#[derive(Debug, Serialize, Deserialize)]
struct ImportManifest {
    version: u32,
    source: String,
    source_alias: String,
    schema: String,
    storage: String,
    cwd: PathBuf,
    mode: String,
    session_id: Option<String>,
    session_paths: Vec<PathBuf>,
    content_hash: String,
    imported_at: DateTime<Utc>,
    source_started_at: Option<DateTime<Utc>>,
    source_updated_at: Option<DateTime<Utc>>,
    rows_seen: usize,
    events_imported: usize,
    provenance_records: usize,
    raw_rows_stored: bool,
    reimport_command: String,
}

#[derive(Debug, Clone)]
struct ImportParseResult {
    rows_seen: usize,
    events: Vec<ImportedEvent>,
    source_started_at: Option<DateTime<Utc>>,
    source_updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct ImportedEvent {
    timestamp: Option<DateTime<Utc>>,
    source_event: String,
    role: Option<String>,
    summary: String,
    tool_name: Option<String>,
    tool_category: Option<String>,
    tool_call_id: Option<String>,
    files: Vec<PathBuf>,
    usage: Option<ImportedUsage>,
    source_path: PathBuf,
    source_line: Option<usize>,
    raw_hash: String,
}

#[derive(Debug, Clone, Serialize)]
struct ImportedUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_window: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ImportedTraceDetail<'a> {
    import_version: u32,
    source: &'a str,
    schema: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
    source_path: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_line: Option<usize>,
    source_event: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'a str>,
    summary: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_category: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
    files: &'a [PathBuf],
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<&'a ImportedUsage>,
    raw_hash: &'a str,
}

#[derive(Debug, Serialize)]
struct ImportJsonOutput<'a> {
    kind: &'a str,
    source: &'a str,
    schema: &'a str,
    roots: &'a [PathBuf],
    candidates: &'a [ImportCandidate],
    next_actions: Vec<String>,
    try_lines: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ImportCompletedJson<'a> {
    kind: &'a str,
    run_id: &'a str,
    manifest_path: PathBuf,
    manifest: &'a ImportManifest,
    next_actions: Vec<String>,
    try_lines: Vec<String>,
}

// SAFETY: Import options are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn import_command(options: ImportCommandOptions) -> Result<()> {
    // Import is a read-only recovery bridge. It reads provider transcript roots
    // through descriptor [ingest] metadata and writes only deadreckon run state.
    let paths = DeadreckonPaths::discover();
    let cwd = options
        .cwd
        .as_deref()
        .map(|path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
        .unwrap_or(std::env::current_dir()?);
    let since = import_since(&options)?;
    let resolved = resolve_import_source(&paths, &options, &cwd)?;
    let candidates = discover_import_candidates(&resolved, &options, &cwd, since);

    if options.list {
        print_import_candidates(&resolved, &candidates, options.json, "import_candidates")?;
        return Ok(());
    }

    if candidates.is_empty() {
        let stale = stale_import_candidates(&resolved, &options, &cwd, since);
        if !stale.is_empty() {
            return Err(CliError::Exit {
                code: 1,
                message: format!(
                    "no fresh import candidates for {}; stale candidates were found\n{}",
                    resolved.alias,
                    import_candidate_table(&stale)
                ),
                hint: format!("deadreckon import {} --since 1d --preview", resolved.alias),
            });
        }
    }

    let (selected, mode) = select_import_candidates(&resolved, &options, &candidates)?;
    if options.preview {
        print_import_selection(&resolved, &selected, mode, options.json)?;
        return Ok(());
    }

    let (run_id, manifest) = normalize_import(&paths, &resolved, &selected, mode, &options, &cwd)?;
    let manifest_path = paths
        .run_root(&manifest_scope(&cwd)?, &run_id)
        .join("import.json");
    let surface = import_completed_surface(&resolved, &run_id, &manifest, &manifest_path, mode);
    if options.json {
        let primary = surface.primary_action.command.clone();
        let secondary = manifest.reimport_command.clone();
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(serde_json::to_value(
                ImportCompletedJson {
                    kind: "import_completed",
                    run_id: &run_id,
                    manifest_path,
                    manifest: &manifest,
                    next_actions: vec![primary.clone(), secondary.clone()],
                    try_lines: vec![primary, secondary],
                },
            )?))?
        );
        return Ok(());
    }

    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn manifest_scope(cwd: &Path) -> Result<String> {
    workspace_scope(cwd).map_err(CliError::from)
}

fn normalize_import(
    paths: &DeadreckonPaths,
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
    mode: ImportMode,
    options: &ImportCommandOptions,
    cwd: &Path,
) -> Result<(String, ImportManifest)> {
    let source_paths = import_source_paths(resolved, selected)?;
    let content_hash = sha256_for_paths(&source_paths)?;
    let imported_id = import_run_id(resolved, selected, mode);
    let scope = workspace_scope(cwd).map_err(CliError::from)?;
    let existing_root = paths.run_root(&scope, &imported_id);
    if existing_root.exists() {
        if let Some(previous) = read_import_manifest(&existing_root)?
            && previous.content_hash != content_hash
            && !options.replace
        {
            return Err(import_invalid(format!(
                "existing import run {} has changed content\nold {}\nnew {}\ntry: deadreckon import {} --session {} --replace",
                imported_id,
                previous.content_hash,
                content_hash,
                resolved.alias,
                shell_arg(&selected_session_arg(selected))
            )));
        }
        fs::remove_dir_all(&existing_root)?;
    }

    let imported_at = Utc::now();
    let parsed = parse_import_candidates(resolved, selected)?;
    let mut state = create_run(
        paths,
        RunOptions {
            goal: format!(
                "imported {} {}",
                import_display_source(&resolved.source),
                import_mode_label(mode)
            ),
            cwd: cwd.to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some(format!("import:{}", resolved.source)),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: Some(imported_id.clone()),
            codebase: None,
        },
    )?;

    let session_id = import_session_id(selected, mode);
    let mut provenance_records = 0usize;
    for (idx, event) in parsed.events.iter().enumerate() {
        let turn = (idx + 1) as u32;
        append_trace(
            &state,
            &TraceRecord {
                timestamp: event.timestamp.unwrap_or(imported_at),
                run_id: state.run_id.clone(),
                turn,
                event: format!("import.{}", import_display_source(&resolved.source)),
                latency_ms: None,
                detail: serde_json::to_value(ImportedTraceDetail {
                    import_version: 1,
                    source: &resolved.source,
                    schema: &resolved.schema,
                    session_id: session_id.as_deref(),
                    source_path: &event.source_path,
                    source_line: event.source_line,
                    source_event: &event.source_event,
                    role: event.role.as_deref(),
                    summary: &event.summary,
                    tool_name: event.tool_name.as_deref(),
                    tool_category: event.tool_category.as_deref(),
                    tool_call_id: event.tool_call_id.as_deref(),
                    files: &event.files,
                    usage: event.usage.as_ref(),
                    raw_hash: &event.raw_hash,
                })?,
            },
        )?;
        if !event.files.is_empty() {
            provenance_records += 1;
            append_provenance(
                &state,
                &ProvenanceRecord {
                    timestamp: event.timestamp.unwrap_or(imported_at),
                    prompt_id: format!("turn-{turn}"),
                    model: format!("import:{}", resolved.source),
                    tool_call_id: event
                        .tool_call_id
                        .clone()
                        .unwrap_or_else(|| format!("imported-{turn}")),
                    session_id: state.run_id.clone(),
                    files: event.files.clone(),
                },
            )?;
        }
    }

    state.turn = parsed.events.len() as u32;
    state.status = RunStatus::Completed;
    state.updated_at = Utc::now();
    save_state(&state)?;

    let manifest = ImportManifest {
        version: 1,
        source: resolved.source.clone(),
        source_alias: resolved.alias.clone(),
        schema: resolved.schema.clone(),
        storage: ingest_storage_label(&resolved.storage).to_string(),
        cwd: cwd.to_path_buf(),
        mode: import_mode_label(mode).to_string(),
        session_id,
        session_paths: source_paths,
        content_hash,
        imported_at,
        source_started_at: parsed.source_started_at,
        source_updated_at: parsed.source_updated_at,
        rows_seen: parsed.rows_seen,
        events_imported: parsed.events.len(),
        provenance_records,
        raw_rows_stored: false,
        reimport_command: reimport_command(resolved, selected, mode),
    };
    fs::write(
        state.run_root.join("import.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    Ok((imported_id, manifest))
}

fn resolve_import_source(
    paths: &DeadreckonPaths,
    options: &ImportCommandOptions,
    cwd: &Path,
) -> Result<ResolvedImportSource> {
    let source = options.source.trim();
    if source == "cursor" {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| paths.home().to_path_buf());
        let root = std::env::var_os("DEADRECKON_IMPORT_CURSOR_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cursor/chats"));
        return Ok(ResolvedImportSource {
            alias: source.to_string(),
            source: "cursor".to_string(),
            schema: "cursor-sqlite".to_string(),
            storage: IngestStorage::Json,
            roots: vec![root],
            cwd_match: IngestCwdMatch::None,
            cwd_match_path: None,
            file_glob: Some("*.db".to_string()),
            id_prefix: Some("cursor:".to_string()),
            kind: ImportSourceKind::Cursor,
        });
    }

    let descriptor_id = import_descriptor_id(source).ok_or_else(|| {
        import_invalid(format!(
            "unknown import source {source}; accepted sources: {}\ntry: deadreckon import codex --list",
            accepted_import_sources().join(", ")
        ))
    })?;
    let registry = ProviderRegistry::with_overrides(paths.home())?;
    let descriptor = registry.get(&descriptor_id).ok_or_else(|| {
        import_invalid(format!(
            "unknown import source {source}; accepted sources: {}\ntry: deadreckon import codex --list",
            accepted_import_sources().join(", ")
        ))
    })?;
    let ingest = descriptor.ingest.clone().ok_or_else(|| {
        import_invalid(format!(
            "{} has no importable descriptor [ingest]\ntry: deadreckon providers list --all",
            descriptor.id
        ))
    })?;
    let schema = ingest.schema.trim();
    if schema.is_empty() {
        return Err(import_invalid(format!(
            "{} has an empty descriptor ingest schema\ntry: deadreckon providers list --all",
            descriptor.id
        )));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.home().to_path_buf());
    let working_dirs = vec![cwd.to_string_lossy().to_string()];
    let roots = provider_ingest_roots_for_working_dirs(
        &ingest,
        &home,
        &working_dirs,
        options.all || options.session.is_some(),
    );
    let storage = ingest.storage.clone().unwrap_or(IngestStorage::Jsonl);
    Ok(ResolvedImportSource {
        alias: source.to_string(),
        source: descriptor.id.clone(),
        schema: schema.to_string(),
        storage,
        roots,
        cwd_match: ingest.cwd_match.clone(),
        cwd_match_path: ingest.cwd_match_path.clone(),
        file_glob: ingest.file_glob.clone(),
        id_prefix: ingest.id_prefix.clone(),
        kind: ImportSourceKind::Provider,
    })
}

fn discover_import_candidates(
    resolved: &ResolvedImportSource,
    options: &ImportCommandOptions,
    cwd: &Path,
    since: DateTime<Utc>,
) -> Vec<ImportCandidate> {
    let effective_since = if options.all || options.session.is_some() {
        import_long_ago()
    } else {
        since
    };
    let mut candidates = match &resolved.kind {
        ImportSourceKind::Provider => {
            discover_provider_import_candidates(resolved, cwd, effective_since)
        }
        ImportSourceKind::Cursor => discover_cursor_import_candidates(resolved, effective_since),
    };
    if let Some(session) = options.session.as_deref()
        && !candidates
            .iter()
            .any(|candidate| import_candidate_matches(candidate, session))
        && Path::new(session).exists()
    {
        candidates.push(candidate_from_explicit_path(resolved, Path::new(session)));
    }
    candidates.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates
}

fn stale_import_candidates(
    resolved: &ResolvedImportSource,
    options: &ImportCommandOptions,
    cwd: &Path,
    since: DateTime<Utc>,
) -> Vec<ImportCandidate> {
    if options.all || options.session.is_some() {
        return Vec::new();
    }
    let mut candidates = match &resolved.kind {
        ImportSourceKind::Provider => {
            discover_provider_import_candidates(resolved, cwd, import_long_ago())
        }
        ImportSourceKind::Cursor => discover_cursor_import_candidates(resolved, import_long_ago()),
    };
    candidates.retain(|candidate| candidate.updated_at < since);
    candidates.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates
}

fn discover_provider_import_candidates(
    resolved: &ResolvedImportSource,
    cwd: &Path,
    since: DateTime<Utc>,
) -> Vec<ImportCandidate> {
    let spec = ProviderJsonlLogSpec {
        schema: resolved.schema.clone(),
        roots: resolved.roots.clone(),
        since,
        cwd_match: resolved.cwd_match.clone(),
        cwd_match_path: resolved.cwd_match_path.clone(),
        storage: resolved.storage.clone(),
        file_glob: resolved.file_glob.clone(),
    };
    let working_dirs = vec![cwd.to_string_lossy().to_string()];
    let mut candidates = Vec::new();
    for root in &resolved.roots {
        let mut files = Vec::new();
        collect_recent_provider_files(root, &spec, &mut files, 0);
        for (path, updated_at) in files {
            let matched_cwd = provider_jsonl_session_matches_run(&spec, &path, &working_dirs)
                .then(|| cwd.to_path_buf());
            let session_id = provider_import_session_id(resolved, &path);
            let id = import_candidate_id(resolved, session_id.as_deref(), &path);
            candidates.push(ImportCandidate {
                id,
                session_id,
                paths: vec![path.clone()],
                root: root.clone(),
                updated_at,
                matched_cwd,
                row_count_hint: import_row_count_hint(resolved, &path),
            });
        }
    }
    candidates
}

fn discover_cursor_import_candidates(
    resolved: &ResolvedImportSource,
    since: DateTime<Utc>,
) -> Vec<ImportCandidate> {
    let mut candidates = Vec::new();
    for root in &resolved.roots {
        let Ok(files) = inventory_files(root) else {
            continue;
        };
        for path in files {
            let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            if !matches!(extension, "sqlite" | "sqlite3" | "db") {
                continue;
            }
            let Some(updated_at) = fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .map(DateTime::<Utc>::from)
            else {
                continue;
            };
            if updated_at < since {
                continue;
            }
            let session_id = path.file_stem().and_then(|stem| stem.to_str()).map(|stem| {
                let prefix = resolved.id_prefix.as_deref().unwrap_or("cursor:");
                format!("{prefix}{stem}")
            });
            let id = import_candidate_id(resolved, session_id.as_deref(), &path);
            candidates.push(ImportCandidate {
                id,
                session_id,
                paths: vec![path],
                root: root.clone(),
                updated_at,
                matched_cwd: None,
                row_count_hint: None,
            });
        }
    }
    candidates
}

fn candidate_from_explicit_path(resolved: &ResolvedImportSource, path: &Path) -> ImportCandidate {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let updated_at = fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now);
    let root = resolved
        .roots
        .iter()
        .find(|root| path.starts_with(root))
        .cloned()
        .unwrap_or_else(|| explicit_import_root(resolved, &path));
    let session_id = provider_import_session_id(resolved, &path).or_else(|| {
        path.file_stem().and_then(|stem| stem.to_str()).map(|stem| {
            let prefix = resolved.id_prefix.as_deref().unwrap_or("");
            format!("{prefix}{stem}")
        })
    });
    ImportCandidate {
        id: import_candidate_id(resolved, session_id.as_deref(), &path),
        session_id,
        paths: vec![path.clone()],
        root,
        updated_at,
        matched_cwd: None,
        row_count_hint: import_row_count_hint(resolved, &path),
    }
}

fn explicit_import_root(resolved: &ResolvedImportSource, path: &Path) -> PathBuf {
    if resolved.storage == IngestStorage::OpenCodeStorage
        && let Some(root) = path.ancestors().nth(4)
    {
        return root.to_path_buf();
    }
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn select_import_candidates(
    resolved: &ResolvedImportSource,
    options: &ImportCommandOptions,
    candidates: &[ImportCandidate],
) -> Result<(Vec<ImportCandidate>, ImportMode)> {
    if candidates.is_empty() {
        return Err(import_invalid(format!(
            "no import candidates for {}\nresolved roots:\n{}\ntry: deadreckon import {} --all --preview",
            resolved.alias,
            resolved_roots_lines(&resolved.roots),
            resolved.alias
        )));
    }
    if options.all {
        return Ok((candidates.to_vec(), ImportMode::All));
    }
    if let Some(session) = options.session.as_deref() {
        let selected = candidates
            .iter()
            .filter(|candidate| import_candidate_matches(candidate, session))
            .cloned()
            .collect::<Vec<_>>();
        return match selected.len() {
            0 => Err(import_invalid(format!(
                "no import candidate matched session {session}\n{}\ntry: deadreckon import {} --list",
                import_candidate_table(candidates),
                resolved.alias
            ))),
            1 => Ok((selected, ImportMode::Session)),
            _ => Err(import_invalid(format!(
                "session {session} matched multiple import candidates\n{}\ntry: deadreckon import {} --session {}",
                import_candidate_table(&selected),
                resolved.alias,
                shell_arg(session)
            ))),
        };
    }

    let cwd_matched = candidates
        .iter()
        .filter(|candidate| {
            resolved.cwd_match == IngestCwdMatch::None || candidate.matched_cwd.is_some()
        })
        .cloned()
        .collect::<Vec<_>>();
    if cwd_matched.len() == 1 {
        return Ok((cwd_matched, ImportMode::Session));
    }
    if cwd_matched.len() > 1 {
        return Err(import_invalid(format!(
            "ambiguous import candidates for {}\n{}\ntry: deadreckon import {} --session <id-or-path>",
            resolved.alias,
            import_candidate_table(&cwd_matched),
            resolved.alias
        )));
    }
    if candidates.len() == 1 {
        return Ok((vec![candidates[0].clone()], ImportMode::Session));
    }
    Err(import_invalid(format!(
        "no cwd-matched import session for {}; {} recent candidates need an explicit session\n{}\ntry: deadreckon import {} --session <id-or-path>",
        resolved.alias,
        candidates.len(),
        import_candidate_table(candidates),
        resolved.alias
    )))
}

fn parse_import_candidates(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
) -> Result<ImportParseResult> {
    let mut rows_seen = 0usize;
    let mut events = Vec::new();
    let mut source_started_at: Option<DateTime<Utc>> = None;
    let mut source_updated_at: Option<DateTime<Utc>> = None;
    for candidate in selected {
        let parsed = match &resolved.kind {
            ImportSourceKind::Provider => parse_provider_import_candidate(resolved, candidate)?,
            ImportSourceKind::Cursor => parse_cursor_import_candidate(candidate)?,
        };
        rows_seen += parsed.rows_seen;
        for timestamp in parsed
            .source_started_at
            .into_iter()
            .chain(parsed.source_updated_at)
        {
            source_started_at = Some(
                source_started_at
                    .map(|existing| existing.min(timestamp))
                    .unwrap_or(timestamp),
            );
            source_updated_at = Some(
                source_updated_at
                    .map(|existing| existing.max(timestamp))
                    .unwrap_or(timestamp),
            );
        }
        events.extend(parsed.events);
    }
    events.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.source_path.cmp(&right.source_path))
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    Ok(ImportParseResult {
        rows_seen,
        events,
        source_started_at,
        source_updated_at,
    })
}

fn parse_provider_import_candidate(
    resolved: &ResolvedImportSource,
    candidate: &ImportCandidate,
) -> Result<ImportParseResult> {
    if resolved.storage == IngestStorage::OpenCodeStorage {
        return parse_opencode_import_candidate(resolved, candidate);
    }
    let mut rows_seen = 0usize;
    let mut events = Vec::new();
    for path in &candidate.paths {
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            let raw = fs::read_to_string(path)?;
            let value = serde_json::from_str::<Value>(&raw).map_err(|err| {
                import_invalid(format!(
                    "malformed JSON at {}: {err}\ntry: fix or exclude {}; then rerun deadreckon import {} --session {}",
                    path.display(),
                    path.display(),
                    resolved.alias,
                    shell_arg(&candidate.id)
                ))
            })?;
            rows_seen += import_json_value_row_count(&value);
            events.extend(import_events_from_json_value(
                resolved, candidate, path, None, &value, &raw,
            ));
            continue;
        }
        for (line_idx, line) in fs::read_to_string(path)?.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            rows_seen += 1;
            let value = serde_json::from_str::<Value>(line).map_err(|err| {
                import_invalid(format!(
                    "malformed JSONL at {}:{}: {err}\ntry: fix or exclude {}; then rerun deadreckon import {} --session {}",
                    path.display(),
                    line_idx + 1,
                    path.display(),
                    resolved.alias,
                    shell_arg(&candidate.id)
                ))
            })?;
            events.extend(import_events_from_json_value(
                resolved,
                candidate,
                path,
                Some(line_idx + 1),
                &value,
                line,
            ));
        }
    }
    let (source_started_at, source_updated_at) = source_time_bounds(&events, candidate.updated_at);
    Ok(ImportParseResult {
        rows_seen,
        events,
        source_started_at,
        source_updated_at,
    })
}

fn parse_cursor_import_candidate(candidate: &ImportCandidate) -> Result<ImportParseResult> {
    let Some(path) = candidate.paths.first() else {
        return Ok(empty_import_parse_result(candidate.updated_at));
    };
    let output = std::process::Command::new("sqlite3")
        .arg("-json")
        .arg(path)
        .arg("select rowid as source_rowid, * from messages order by rowid")
        .output();
    let output = output.map_err(|err| {
        import_invalid(format!(
            "sqlite3 is required to import Cursor history from {}: {err}\ntry: install sqlite3 or pass a JSONL-capable provider source",
            path.display()
        ))
    })?;
    if !output.status.success() {
        return Err(import_invalid(format!(
            "failed to query Cursor database {}: {}\ntry: install sqlite3 or pass a JSONL-capable provider source",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let values: Vec<Value> = serde_json::from_slice(&output.stdout).map_err(|err| {
        import_invalid(format!(
            "sqlite3 returned invalid JSON for {}: {err}",
            path.display()
        ))
    })?;
    let mut events = Vec::new();
    for value in &values {
        let raw = serde_json::to_string(value)?;
        let mut event = generic_import_event(
            "cursor-sqlite",
            candidate,
            path,
            value
                .get("source_rowid")
                .and_then(Value::as_u64)
                .and_then(|row| usize::try_from(row).ok()),
            value,
            &raw,
        );
        event.source_event = "message".to_string();
        events.push(event);
    }
    let (source_started_at, source_updated_at) = source_time_bounds(&events, candidate.updated_at);
    Ok(ImportParseResult {
        rows_seen: values.len(),
        events,
        source_started_at,
        source_updated_at,
    })
}

fn parse_opencode_import_candidate(
    resolved: &ResolvedImportSource,
    candidate: &ImportCandidate,
) -> Result<ImportParseResult> {
    let Some(session_path) = candidate.paths.first() else {
        return Ok(empty_import_parse_result(candidate.updated_at));
    };
    let session_raw = fs::read_to_string(session_path)?;
    let session = serde_json::from_str::<Value>(&session_raw).map_err(|err| {
        import_invalid(format!(
            "malformed JSON at {}: {err}\ntry: fix or exclude {}; then rerun deadreckon import {} --session {}",
            session_path.display(),
            session_path.display(),
            resolved.alias,
            shell_arg(&candidate.id)
        ))
    })?;
    let Some(session_id) = session.get("id").and_then(Value::as_str) else {
        return Ok(empty_import_parse_result(candidate.updated_at));
    };
    let root = session_path
        .ancestors()
        .nth(4)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| candidate.root.clone());
    let messages = read_json_entries_sorted(&root.join("storage/message").join(session_id));
    let mut rows_seen = 1usize;
    let mut events = Vec::new();
    events.extend(import_events_from_json_value(
        resolved,
        candidate,
        session_path,
        None,
        &session,
        &session_raw,
    ));
    for (message_path, message, message_raw) in messages {
        rows_seen += 1;
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let timestamp = import_timestamp(&message);
        let message_id = message.get("id").and_then(Value::as_str).unwrap_or("");
        let parts = read_json_entries_sorted(&root.join("storage/part").join(message_id));
        if parts.is_empty() {
            let mut event = generic_import_event(
                &resolved.schema,
                candidate,
                &message_path,
                None,
                &message,
                &message_raw,
            );
            event.role = role.clone();
            event.timestamp = timestamp;
            events.push(event);
            continue;
        }
        for (part_path, part, part_raw) in parts {
            rows_seen += 1;
            let mut event =
                opencode_import_event(candidate, &part_path, &part, &part_raw, role.as_deref());
            event.timestamp = import_timestamp(&part).or(timestamp);
            events.push(event);
        }
    }
    let (source_started_at, source_updated_at) = source_time_bounds(&events, candidate.updated_at);
    Ok(ImportParseResult {
        rows_seen,
        events,
        source_started_at,
        source_updated_at,
    })
}

fn import_events_from_json_value(
    resolved: &ResolvedImportSource,
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    match resolved.schema.as_str() {
        "codex-cli" => codex_import_events(candidate, path, line, value, raw),
        "claude-code" => claude_import_events(candidate, path, line, value, raw),
        "gemini" => gemini_import_events(candidate, path, line, value, raw),
        "copilot-cli" => copilot_import_events(candidate, path, line, value, raw),
        "pi" => pi_import_events(candidate, path, line, value, raw),
        "opencode" => vec![generic_import_event(
            &resolved.schema,
            candidate,
            path,
            line,
            value,
            raw,
        )],
        _ => vec![generic_import_event(
            &resolved.schema,
            candidate,
            path,
            line,
            value,
            raw,
        )],
    }
}

fn codex_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let mut event = generic_import_event("codex-cli", candidate, path, line, value, raw);
    match (
        value.get("type").and_then(Value::as_str),
        payload.get("type").and_then(Value::as_str),
    ) {
        (Some("session_meta"), _) => {
            event.source_event = "session_meta".to_string();
            event.summary = payload
                .get("cwd")
                .and_then(Value::as_str)
                .map(|cwd| format!("session cwd {cwd}"))
                .unwrap_or_else(|| "session metadata".to_string());
        }
        (Some("event_msg"), Some("agent_message")) => {
            event.source_event = "agent_message".to_string();
            event.role = Some("assistant".to_string());
            if let Some(message) = payload.get("message").and_then(Value::as_str) {
                event.summary = one_line(message, 180);
            }
        }
        (Some("event_msg"), Some("token_count")) => {
            event.source_event = "usage".to_string();
            event.summary = "token count".to_string();
            event.usage = codex_usage(payload);
        }
        (Some("response_item"), Some("function_call")) => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let args = payload
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args_value =
                serde_json::from_str::<Value>(args).unwrap_or(Value::String(args.to_string()));
            event.source_event = "tool_call".to_string();
            event.tool_name = Some(name.to_string());
            event.tool_category = Some(provider_tool_label(name).to_string());
            event.tool_call_id = payload
                .get("call_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = format!(
                "tool {} {}",
                provider_tool_label(name),
                one_line(&json_tool_summary(name, &args_value), 160)
            );
            event.files = collect_import_paths(&args_value);
        }
        (Some("response_item"), Some("function_call_output")) => {
            event.source_event = "tool_result".to_string();
            event.role = Some("tool".to_string());
            event.tool_call_id = payload
                .get("call_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = payload
                .get("output")
                .and_then(Value::as_str)
                .map(|output| format!("result {}", one_line(output, 160)))
                .unwrap_or_else(|| event.summary.clone());
        }
        _ => {}
    }
    vec![event]
}

fn claude_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    let row_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let Some(message) = value.get("message") else {
        return vec![generic_import_event(
            "claude-code",
            candidate,
            path,
            line,
            value,
            raw,
        )];
    };
    let usage = message.get("usage").and_then(import_usage_from_value);
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        let mut event = generic_import_event("claude-code", candidate, path, line, value, raw);
        event.source_event = row_type.to_string();
        event.usage = usage;
        return vec![event];
    };
    let mut events = Vec::new();
    for part in content {
        let mut event = import_base_event(candidate, path, line, raw);
        event.timestamp = import_timestamp(value);
        event.role = Some(row_type.to_string());
        event.usage = usage.clone();
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                event.source_event = "message".to_string();
                event.summary = part
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| one_line(text, 180))
                    .unwrap_or_else(|| "message".to_string());
            }
            Some("thinking") => {
                event.source_event = "thinking".to_string();
                event.summary = part
                    .get("thinking")
                    .and_then(Value::as_str)
                    .map(|text| format!("thinking {}", one_line(text, 160)))
                    .unwrap_or_else(|| "thinking".to_string());
            }
            Some("tool_use") => {
                let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
                let input = part.get("input").unwrap_or(&Value::Null);
                event.source_event = "tool_call".to_string();
                event.tool_name = Some(name.to_string());
                event.tool_category = Some(provider_tool_label(name).to_string());
                event.tool_call_id = part
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                event.summary = format!(
                    "tool {} {}",
                    provider_tool_label(name),
                    one_line(&claude_tool_summary(name, input), 160)
                );
                event.files = collect_import_paths(input);
            }
            Some("tool_result") => {
                event.source_event = "tool_result".to_string();
                event.tool_call_id = part
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                event.summary = format!(
                    "result {}",
                    one_line(
                        &claude_content_text(part.get("content").unwrap_or(&Value::Null)),
                        160
                    )
                );
                event.files = collect_import_paths(part);
            }
            Some(other) => {
                event.source_event = other.to_string();
                event.summary = one_line(&json_value_text(part), 180);
                event.files = collect_import_paths(part);
            }
            None => {
                event.source_event = row_type.to_string();
                event.summary = one_line(&json_value_text(part), 180);
            }
        }
        events.push(event);
    }
    if events.is_empty() {
        vec![generic_import_event(
            "claude-code",
            candidate,
            path,
            line,
            value,
            raw,
        )]
    } else {
        events
    }
}

fn gemini_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    if let Some(messages) = value.get("messages").and_then(Value::as_array) {
        let mut events = Vec::new();
        for message in messages {
            let raw = serde_json::to_string(message).unwrap_or_else(|_| raw.to_string());
            events.extend(gemini_import_events(candidate, path, line, message, &raw));
        }
        return events;
    }
    let mut events = Vec::new();
    let usage = gemini_usage(value);
    for thought in value
        .get("thoughts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut event = import_base_event(candidate, path, line, raw);
        event.timestamp = import_timestamp(value);
        event.source_event = "thinking".to_string();
        event.role = Some("assistant".to_string());
        event.usage = usage.clone();
        event.summary = format!("thinking {}", one_line(&json_value_text(thought), 160));
        events.push(event);
    }
    for text in gemini_content_texts(value.get("content").unwrap_or(&Value::Null)) {
        let mut event = import_base_event(candidate, path, line, raw);
        event.timestamp = import_timestamp(value);
        event.source_event = "message".to_string();
        event.role = Some("assistant".to_string());
        event.usage = usage.clone();
        event.summary = one_line(&text, 180);
        events.push(event);
    }
    if let Some(tool_calls) = value.get("toolCalls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let name = tool_call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let args = tool_call.get("args").unwrap_or(&Value::Null);
            let mut event = import_base_event(candidate, path, line, raw);
            event.timestamp = import_timestamp(value);
            event.source_event = "tool_call".to_string();
            event.role = Some("assistant".to_string());
            event.tool_name = Some(name.to_string());
            event.tool_category = Some(provider_tool_label(name).to_string());
            event.tool_call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = format!(
                "tool {} {}",
                provider_tool_label(name),
                one_line(&json_tool_summary(name, args), 160)
            );
            event.files = collect_import_paths(args);
            events.push(event);
        }
    }
    if events.is_empty() {
        vec![generic_import_event(
            "gemini", candidate, path, line, value, raw,
        )]
    } else {
        events
    }
}

fn copilot_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    let data = value.get("data").unwrap_or(&Value::Null);
    let usage = value.get("usage").and_then(import_usage_from_value);
    let mut events = Vec::new();
    match value.get("type").and_then(Value::as_str) {
        Some("assistant.message") => {
            if let Some(reasoning) = data.get("reasoningText").and_then(Value::as_str)
                && !reasoning.trim().is_empty()
            {
                let mut event = import_base_event(candidate, path, line, raw);
                event.timestamp = import_timestamp(value);
                event.source_event = "thinking".to_string();
                event.role = Some("assistant".to_string());
                event.usage = usage.clone();
                event.summary = format!("thinking {}", one_line(reasoning, 160));
                events.push(event);
            }
            if let Some(content) = data.get("content").and_then(Value::as_str)
                && !content.trim().is_empty()
            {
                let mut event = import_base_event(candidate, path, line, raw);
                event.timestamp = import_timestamp(value);
                event.source_event = "message".to_string();
                event.role = Some("assistant".to_string());
                event.usage = usage.clone();
                event.summary = one_line(content, 180);
                events.push(event);
            }
            if let Some(tool_requests) = data.get("toolRequests").and_then(Value::as_array) {
                for request in tool_requests {
                    let name = request
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    let input = provider_arguments_value(request.get("arguments"));
                    let mut event = import_base_event(candidate, path, line, raw);
                    event.timestamp = import_timestamp(value);
                    event.source_event = "tool_call".to_string();
                    event.role = Some("assistant".to_string());
                    event.tool_name = Some(name.to_string());
                    event.tool_category = Some(provider_tool_label(name).to_string());
                    event.tool_call_id = request
                        .get("id")
                        .or_else(|| request.get("toolCallId"))
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    event.summary = format!(
                        "tool {} {}",
                        provider_tool_label(name),
                        one_line(&json_tool_summary(name, &input), 160)
                    );
                    event.files = collect_import_paths(&input);
                    event.usage = usage.clone();
                    events.push(event);
                }
            }
        }
        Some("assistant.reasoning") => {
            let mut event = import_base_event(candidate, path, line, raw);
            event.timestamp = import_timestamp(value);
            event.source_event = "thinking".to_string();
            event.role = Some("assistant".to_string());
            event.usage = usage;
            event.summary = data
                .get("text")
                .or_else(|| data.get("content"))
                .and_then(Value::as_str)
                .map(|text| format!("thinking {}", one_line(text, 160)))
                .unwrap_or_else(|| "thinking".to_string());
            events.push(event);
        }
        Some("tool.execution_complete") => {
            let result = data.get("result").unwrap_or(&Value::Null);
            let mut event = import_base_event(candidate, path, line, raw);
            event.timestamp = import_timestamp(value);
            event.source_event = "tool_result".to_string();
            event.role = Some("tool".to_string());
            event.tool_call_id = data
                .get("toolCallId")
                .or_else(|| data.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = format!("result {}", one_line(&json_value_text(result), 160));
            event.files = collect_import_paths(result);
            event.usage = usage;
            events.push(event);
        }
        Some("session.model_change") => {
            let mut event = generic_import_event("copilot-cli", candidate, path, line, value, raw);
            event.source_event = "model_change".to_string();
            events.push(event);
        }
        _ => {}
    }
    if events.is_empty() {
        vec![generic_import_event(
            "copilot-cli",
            candidate,
            path,
            line,
            value,
            raw,
        )]
    } else {
        events
    }
}

fn pi_import_events(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> Vec<ImportedEvent> {
    if value.get("type").and_then(Value::as_str) == Some("session") {
        let mut event = generic_import_event("pi", candidate, path, line, value, raw);
        event.source_event = "session".to_string();
        event.summary = "session header".to_string();
        return vec![event];
    }
    let message = value.get("message").unwrap_or(value);
    let usage = message.get("usage").and_then(import_usage_from_value);
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let mut events = Vec::new();
    match role.as_deref() {
        Some("assistant") => {
            let content = message.get("content").unwrap_or(&Value::Null);
            if let Some(text) = content.as_str()
                && !text.trim().is_empty()
            {
                let mut event = import_base_event(candidate, path, line, raw);
                event.timestamp = import_timestamp(value);
                event.source_event = "message".to_string();
                event.role = role.clone();
                event.usage = usage.clone();
                event.summary = one_line(text, 180);
                events.push(event);
            }
            if let Some(blocks) = content.as_array() {
                for block in blocks {
                    let mut event = import_base_event(candidate, path, line, raw);
                    event.timestamp = import_timestamp(value);
                    event.role = role.clone();
                    event.usage = usage.clone();
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            event.source_event = "message".to_string();
                            event.summary = block
                                .get("text")
                                .and_then(Value::as_str)
                                .map(|text| one_line(text, 180))
                                .unwrap_or_else(|| "message".to_string());
                        }
                        Some("thinking") => {
                            event.source_event = "thinking".to_string();
                            event.summary = block
                                .get("thinking")
                                .and_then(Value::as_str)
                                .map(|text| format!("thinking {}", one_line(text, 160)))
                                .unwrap_or_else(|| "thinking".to_string());
                        }
                        Some("toolCall") => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = normalize_pi_tool_arguments(provider_arguments_value(
                                block.get("arguments"),
                            ));
                            event.source_event = "tool_call".to_string();
                            event.tool_name = Some(name.to_string());
                            event.tool_category = Some(provider_tool_label(name).to_string());
                            event.tool_call_id = block
                                .get("id")
                                .or_else(|| block.get("toolCallId"))
                                .and_then(Value::as_str)
                                .map(ToString::to_string);
                            event.summary = format!(
                                "tool {} {}",
                                provider_tool_label(name),
                                one_line(&json_tool_summary(name, &input), 160)
                            );
                            event.files = collect_import_paths(&input);
                        }
                        Some(other) => {
                            event.source_event = other.to_string();
                            event.summary = one_line(&json_value_text(block), 180);
                            event.files = collect_import_paths(block);
                        }
                        None => {
                            event.source_event = "message".to_string();
                            event.summary = one_line(&json_value_text(block), 180);
                        }
                    }
                    events.push(event);
                }
            }
        }
        Some("toolResult") => {
            let mut event = import_base_event(candidate, path, line, raw);
            event.timestamp = import_timestamp(value);
            event.source_event = "tool_result".to_string();
            event.role = role;
            event.usage = usage;
            event.summary = format!(
                "result {}",
                one_line(
                    &json_value_text(message.get("content").unwrap_or(&Value::Null)),
                    160
                )
            );
            event.files = collect_import_paths(message);
            events.push(event);
        }
        _ => {}
    }
    if events.is_empty() {
        vec![generic_import_event(
            "pi", candidate, path, line, value, raw,
        )]
    } else {
        events
    }
}

fn opencode_import_event(
    candidate: &ImportCandidate,
    path: &Path,
    value: &Value,
    raw: &str,
    role: Option<&str>,
) -> ImportedEvent {
    let mut event = generic_import_event("opencode", candidate, path, None, value, raw);
    event.role = role.map(ToString::to_string);
    event.source_event = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("part")
        .to_string();
    match value.get("type").and_then(Value::as_str) {
        Some("text") => {
            event.source_event = "message".to_string();
            event.summary = value
                .get("content")
                .or_else(|| value.get("text"))
                .and_then(Value::as_str)
                .map(|text| one_line(text, 180))
                .unwrap_or_else(|| "message".to_string());
        }
        Some("reasoning") => {
            event.source_event = "thinking".to_string();
            event.summary = value
                .get("content")
                .or_else(|| value.get("text"))
                .and_then(Value::as_str)
                .map(|text| format!("thinking {}", one_line(text, 160)))
                .unwrap_or_else(|| "thinking".to_string());
        }
        Some("tool") => {
            let name = value.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let input = value
                .pointer("/state/input")
                .or_else(|| value.get("input"))
                .unwrap_or(&Value::Null);
            event.source_event = "tool_call".to_string();
            event.tool_name = Some(name.to_string());
            event.tool_category = Some(provider_tool_label(name).to_string());
            event.tool_call_id = value
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            event.summary = format!(
                "tool {} {}",
                provider_tool_label(name),
                one_line(&json_tool_summary(name, input), 160)
            );
            event.files = collect_import_paths(input);
        }
        _ => {}
    }
    event
}

fn generic_import_event(
    schema: &str,
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    value: &Value,
    raw: &str,
) -> ImportedEvent {
    let mut event = import_base_event(candidate, path, line, raw);
    event.timestamp = import_timestamp(value);
    event.source_event = import_source_event(value);
    event.role = value
        .get("role")
        .or_else(|| value.pointer("/message/role"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    event.summary = import_summary(schema, value);
    event.tool_name = import_tool_name(value);
    event.tool_category = event
        .tool_name
        .as_deref()
        .map(provider_tool_label)
        .map(ToString::to_string);
    event.tool_call_id = import_tool_call_id(value);
    event.files = collect_import_paths(value);
    event.usage = import_usage_for_schema(schema, value);
    event
}

fn import_base_event(
    candidate: &ImportCandidate,
    path: &Path,
    line: Option<usize>,
    raw: &str,
) -> ImportedEvent {
    ImportedEvent {
        timestamp: None,
        source_event: "row".to_string(),
        role: None,
        summary: "imported row".to_string(),
        tool_name: None,
        tool_category: None,
        tool_call_id: None,
        files: Vec::new(),
        usage: None,
        source_path: path.to_path_buf(),
        source_line: line,
        raw_hash: sha256_for_str(raw),
    }
    .with_candidate_tool_id(candidate)
}

trait ImportedEventExt {
    fn with_candidate_tool_id(self, candidate: &ImportCandidate) -> Self;
}

impl ImportedEventExt for ImportedEvent {
    fn with_candidate_tool_id(mut self, candidate: &ImportCandidate) -> Self {
        if self.tool_call_id.is_none() {
            self.tool_call_id = candidate.session_id.clone();
        }
        self
    }
}

fn import_since(options: &ImportCommandOptions) -> Result<DateTime<Utc>> {
    if options.all || options.session.is_some() {
        return Ok(import_long_ago());
    }
    let Some(raw) = options.since.as_deref() else {
        return Ok(Utc::now() - ChronoDuration::minutes(2));
    };
    let duration = parse_import_duration(raw).ok_or_else(|| {
        import_invalid(format!(
            "invalid import --since duration {raw}; use values like 10m, 2h, or 1d\ntry: deadreckon import {} --since 10m --list",
            options.source
        ))
    })?;
    Ok(Utc::now() - duration)
}

fn parse_import_duration(raw: &str) -> Option<ChronoDuration> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (number, unit) = trimmed.split_at(
        trimmed
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(trimmed.len()),
    );
    let value = number.parse::<i64>().ok()?;
    match unit {
        "" | "m" | "min" | "mins" | "minute" | "minutes" => Some(ChronoDuration::minutes(value)),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(ChronoDuration::hours(value)),
        "d" | "day" | "days" => Some(ChronoDuration::days(value)),
        "s" | "sec" | "secs" | "second" | "seconds" => Some(ChronoDuration::seconds(value)),
        _ => None,
    }
}

fn import_long_ago() -> DateTime<Utc> {
    Utc::now() - ChronoDuration::days(36_500)
}

fn accepted_import_sources() -> Vec<&'static str> {
    vec![
        "codex",
        "claude-code",
        "gemini",
        "opencode",
        "copilot",
        "pi",
        "cursor",
        "cli:claude-code",
        "cli:codex",
        "cli:gemini",
        "cli:opencode",
        "cli:copilot",
        "cli:pi",
    ]
}

fn import_descriptor_id(source: &str) -> Option<String> {
    Some(
        match source {
            "codex" | "cli:codex" => "cli:codex",
            "claude-code" | "cli:claude-code" => "cli:claude-code",
            "gemini" | "cli:gemini" => "cli:gemini",
            "opencode" | "cli:opencode" => "cli:opencode",
            "copilot" | "cli:copilot" => "cli:copilot",
            "pi" | "cli:pi" => "cli:pi",
            _ if source.starts_with("cli:") => source,
            _ => return None,
        }
        .to_string(),
    )
}

fn import_invalid(message: String) -> CliError {
    CliError::Core(DeadreckonError::InvalidInput(message))
}

fn print_import_candidates(
    resolved: &ResolvedImportSource,
    candidates: &[ImportCandidate],
    json_output: bool,
    kind: &str,
) -> Result<()> {
    let surface = import_candidates_surface(resolved, candidates, kind);
    let primary = surface.primary_action.command.clone();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(serde_json::to_value(
                ImportJsonOutput {
                    kind,
                    source: &resolved.source,
                    schema: &resolved.schema,
                    roots: &resolved.roots,
                    candidates,
                    next_actions: vec![primary.clone()],
                    try_lines: vec![primary],
                },
            )?))?
        );
        return Ok(());
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn print_import_selection(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
    mode: ImportMode,
    json_output: bool,
) -> Result<()> {
    let surface = import_selection_surface(resolved, selected, mode);
    let primary = surface.primary_action.command.clone();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&surface.add_to_json(serde_json::to_value(
                ImportJsonOutput {
                    kind: "import_preview",
                    source: &resolved.source,
                    schema: &resolved.schema,
                    roots: &resolved.roots,
                    candidates: selected,
                    next_actions: vec![primary.clone()],
                    try_lines: vec![primary],
                },
            )?))?
        );
        return Ok(());
    }
    println!("{}", surface.render_plain(!completion_hints_enabled(false)));
    Ok(())
}

fn import_candidates_surface(
    resolved: &ResolvedImportSource,
    candidates: &[ImportCandidate],
    kind: &str,
) -> VerdictSurface {
    let candidate_count = candidates.len();
    let primary = if candidates.is_empty() {
        format!("deadreckon import {} --all --preview", resolved.alias)
    } else {
        format!(
            "deadreckon import {} --session <id-or-path>",
            resolved.alias
        )
    };
    let mut evidence = vec![
        ("source", resolved.source.clone()),
        ("schema", resolved.schema.clone()),
        ("candidates", candidate_count.to_string()),
        (
            "roots",
            resolved_roots_lines(&resolved.roots).replace('\n', "; "),
        ),
        ("surface", kind.to_string()),
    ];
    if !candidates.is_empty() {
        evidence.push(("candidate table", compact_candidate_table(candidates)));
    }
    let kind = if candidates.is_empty() {
        VerdictKind::Noop
    } else {
        VerdictKind::Preview
    };
    let why = if candidates.is_empty() {
        "No fresh candidate matched the current import filters, so DeadReckon did not select a session."
    } else {
        "The command is read-only; choose a concrete session before importing transcript state."
    };
    VerdictSurface::try_new(
        kind,
        "import",
        None,
        ExplanationPanel::new(
            format!(
                "DeadReckon inspected {} import roots and found {candidate_count} candidate session{}.",
                resolved.alias,
                if candidate_count == 1 { "" } else { "s" }
            ),
            why,
            evidence,
        ),
        vec![("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
    .expect("import candidates verdict surface must be valid")
}

fn import_selection_surface(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
    mode: ImportMode,
) -> VerdictSurface {
    let primary = reimport_command(resolved, selected, mode);
    let mut evidence = vec![
        ("source", resolved.source.clone()),
        ("schema", resolved.schema.clone()),
        ("mode", import_mode_label(mode).to_string()),
        ("selected", selected.len().to_string()),
    ];
    if !selected.is_empty() {
        evidence.push(("candidate table", compact_candidate_table(selected)));
    }
    VerdictSurface::try_new(
        VerdictKind::Preview,
        "import",
        None,
        ExplanationPanel::new(
            format!(
                "DeadReckon previewed {} selected import candidate{}.",
                selected.len(),
                if selected.len() == 1 { "" } else { "s" }
            ),
            "Preview mode writes no run state; rerun without --preview or with --replace to create the import run.",
            evidence,
        ),
        vec![("Recommended", primary.as_str())],
        Vec::<(&str, &str)>::new(),
    )
    .expect("import preview verdict surface must be valid")
}

fn import_completed_surface(
    resolved: &ResolvedImportSource,
    run_id: &str,
    manifest: &ImportManifest,
    manifest_path: &Path,
    mode: ImportMode,
) -> VerdictSurface {
    let primary = format!("deadreckon show {run_id}");
    let mut evidence = vec![
        ("run", run_id.to_string()),
        ("source", resolved.source.clone()),
        ("schema", resolved.schema.clone()),
        ("mode", import_mode_label(mode).to_string()),
        ("events", manifest.events_imported.to_string()),
        ("provenance", manifest.provenance_records.to_string()),
        ("manifest", manifest_path.display().to_string()),
    ];
    if let Some(session_id) = manifest.session_id.as_deref() {
        evidence.push(("session", session_id.to_string()));
    }
    if !manifest.session_paths.is_empty() {
        evidence.push((
            "paths",
            manifest
                .session_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    VerdictSurface::try_new(
        VerdictKind::Completed,
        "import",
        Some(run_id),
        ExplanationPanel::new(
            format!(
                "DeadReckon imported {} event{} into run {run_id}.",
                manifest.events_imported,
                if manifest.events_imported == 1 { "" } else { "s" }
            ),
            "The transcript was normalized into completed run state and its import manifest was written for repeatable reimport.",
            evidence,
        ),
        vec![("Recommended", primary.as_str())],
        vec![("Secondary", manifest.reimport_command.as_str())],
    )
    .expect("import completed verdict surface must be valid")
}

fn compact_candidate_table(candidates: &[ImportCandidate]) -> String {
    import_candidate_table(candidates)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn import_mode_label(mode: ImportMode) -> &'static str {
    match mode {
        ImportMode::Session => "session",
        ImportMode::All => "all",
    }
}

fn ingest_storage_label(storage: &IngestStorage) -> &'static str {
    match storage {
        IngestStorage::Jsonl => "jsonl",
        IngestStorage::Json => "json",
        IngestStorage::JsonOrJsonl => "json-or-jsonl",
        IngestStorage::OpenCodeStorage => "opencode-storage",
    }
}

fn import_display_source(source: &str) -> &str {
    source.strip_prefix("cli:").unwrap_or(source)
}

fn import_source_paths(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for candidate in selected {
        for path in &candidate.paths {
            if resolved.storage == IngestStorage::OpenCodeStorage {
                for related in opencode_related_paths(path)? {
                    paths.insert(related);
                }
            } else {
                paths.insert(path.clone());
            }
        }
    }
    Ok(paths.into_iter().collect())
}

fn opencode_related_paths(session_path: &Path) -> Result<Vec<PathBuf>> {
    let raw = fs::read_to_string(session_path)?;
    let session = serde_json::from_str::<Value>(&raw)?;
    let Some(session_id) = session.get("id").and_then(Value::as_str) else {
        return Ok(vec![session_path.to_path_buf()]);
    };
    let root = session_path
        .ancestors()
        .nth(4)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            session_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        });
    let mut paths = BTreeSet::new();
    paths.insert(session_path.to_path_buf());
    for (message_path, message, _) in
        read_json_entries_sorted(&root.join("storage/message").join(session_id))
    {
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        paths.insert(message_path);
        if let Some(message_id) = message_id {
            for (part_path, _, _) in
                read_json_entries_sorted(&root.join("storage/part").join(message_id))
            {
                paths.insert(part_path);
            }
        }
    }
    Ok(paths.into_iter().collect())
}

fn sha256_for_paths(paths: &[PathBuf]) -> Result<String> {
    let mut hasher = Sha256::new();
    for path in paths {
        hasher.update(fs::read(path)?);
        hasher.update([0xff]);
    }
    Ok(format!(
        "sha256:{}",
        hex_digest(hasher.finalize().as_slice())
    ))
}

fn sha256_for_str(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    format!("sha256:{}", hex_digest(digest.as_slice()))
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn import_run_id(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
    mode: ImportMode,
) -> String {
    let identity = match mode {
        ImportMode::All => {
            let roots = resolved
                .roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
                .join("|");
            format!("{}:all:{roots}", resolved.source)
        }
        ImportMode::Session => {
            let selected_identity = selected_session_identity(selected);
            format!("{}:session:{selected_identity}", resolved.source)
        }
    };
    format!("imported-{:016x}", stable_hash(&identity))
}

fn selected_session_identity(selected: &[ImportCandidate]) -> String {
    selected
        .iter()
        .flat_map(|candidate| candidate.paths.iter())
        .map(|path| {
            path.canonicalize()
                .unwrap_or_else(|_| path.clone())
                .display()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn read_import_manifest(run_root: &Path) -> Result<Option<ImportManifest>> {
    let path = run_root.join("import.json");
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn selected_session_arg(selected: &[ImportCandidate]) -> String {
    selected
        .first()
        .and_then(|candidate| candidate.session_id.as_deref())
        .map(ToString::to_string)
        .or_else(|| {
            selected
                .first()
                .and_then(|candidate| candidate.paths.first())
                .map(|path| path.display().to_string())
        })
        .unwrap_or_else(|| "<id-or-path>".to_string())
}

fn import_session_id(selected: &[ImportCandidate], mode: ImportMode) -> Option<String> {
    match mode {
        ImportMode::All => None,
        ImportMode::Session => selected
            .first()
            .and_then(|candidate| candidate.session_id.clone())
            .or_else(|| selected.first().map(|candidate| candidate.id.clone())),
    }
}

fn reimport_command(
    resolved: &ResolvedImportSource,
    selected: &[ImportCandidate],
    mode: ImportMode,
) -> String {
    match mode {
        ImportMode::All => format!("deadreckon import {} --all --replace", resolved.alias),
        ImportMode::Session => format!(
            "deadreckon import {} --session {} --replace",
            resolved.alias,
            shell_arg(&selected_session_arg(selected))
        ),
    }
}

fn shell_arg(raw: &str) -> String {
    if raw
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-' | '/' | '.' | '='))
    {
        return raw.to_string();
    }
    format!("'{}'", raw.replace('\'', "'\\''"))
}

fn provider_import_session_id(resolved: &ResolvedImportSource, path: &Path) -> Option<String> {
    let raw_id = match resolved.storage {
        IngestStorage::OpenCodeStorage | IngestStorage::Json => fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| session_id_from_value(&resolved.schema, &value)),
        IngestStorage::Jsonl | IngestStorage::JsonOrJsonl => {
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                fs::read_to_string(path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                    .and_then(|value| session_id_from_value(&resolved.schema, &value))
            } else {
                jsonl_session_id(path, &resolved.schema)
            }
        }
    }
    .or_else(|| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToString::to_string)
    })?;
    let prefix = resolved.id_prefix.as_deref().unwrap_or("");
    if prefix.is_empty() || raw_id.starts_with(prefix) {
        Some(raw_id)
    } else {
        Some(format!("{prefix}{raw_id}"))
    }
}

fn jsonl_session_id(path: &Path, schema: &str) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = io::BufReader::new(file);
    for line in reader.lines().map_while(std::result::Result::ok).take(80) {
        let value = serde_json::from_str::<Value>(&line).ok()?;
        if let Some(id) = session_id_from_value(schema, &value) {
            return Some(id);
        }
    }
    None
}

fn session_id_from_value(schema: &str, value: &Value) -> Option<String> {
    for pointer in [
        "/session_id",
        "/sessionId",
        "/conversation_id",
        "/conversationId",
        "/id",
        "/payload/session_id",
        "/payload/sessionId",
        "/payload/id",
        "/data/session_id",
        "/data/sessionId",
        "/data/conversationId",
        "/message/session_id",
    ] {
        if let Some(id) = value.pointer(pointer).and_then(Value::as_str)
            && !id.trim().is_empty()
        {
            return Some(id.to_string());
        }
    }
    if schema == "codex-cli"
        && value.get("type").and_then(Value::as_str) == Some("session_meta")
        && let Some(cwd) = value.pointer("/payload/cwd").and_then(Value::as_str)
    {
        return Some(format!("cwd-{:016x}", stable_hash(cwd)));
    }
    None
}

fn import_candidate_id(
    resolved: &ResolvedImportSource,
    session_id: Option<&str>,
    path: &Path,
) -> String {
    session_id.map(ToString::to_string).unwrap_or_else(|| {
        let prefix = resolved.id_prefix.as_deref().unwrap_or("");
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("session");
        format!("{prefix}{stem}")
    })
}

fn import_row_count_hint(resolved: &ResolvedImportSource, path: &Path) -> Option<usize> {
    if resolved.storage == IngestStorage::OpenCodeStorage {
        return opencode_related_paths(path).ok().map(|paths| paths.len());
    }
    if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
        return fs::read_to_string(path)
            .ok()
            .map(|raw| raw.lines().filter(|line| !line.trim().is_empty()).count());
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .map(|value| import_json_value_row_count(&value))
}

fn import_json_value_row_count(value: &Value) -> usize {
    value
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(1)
}

fn import_candidate_matches(candidate: &ImportCandidate, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return false;
    }
    let query_path = Path::new(query);
    let canonical_query = query_path.canonicalize().ok();
    if candidate.id == query
        || candidate.session_id.as_deref().is_some_and(|session_id| {
            session_id == query || strip_import_id_prefix(session_id) == query
        })
        || strip_import_id_prefix(&candidate.id) == query
    {
        return true;
    }
    candidate.paths.iter().any(|path| {
        path.display().to_string() == query
            || path.file_name().and_then(|name| name.to_str()) == Some(query)
            || path.file_stem().and_then(|stem| stem.to_str()) == Some(query)
            || canonical_query
                .as_ref()
                .is_some_and(|canonical| path.canonicalize().ok().as_ref() == Some(canonical))
    })
}

fn strip_import_id_prefix(id: &str) -> &str {
    id.split_once(':').map(|(_, rest)| rest).unwrap_or(id)
}

fn import_candidate_table(candidates: &[ImportCandidate]) -> String {
    let mut out = String::new();
    for candidate in candidates {
        let rows = candidate
            .row_count_hint
            .map(|count| count.to_string())
            .unwrap_or_else(|| "-".to_string());
        let cwd = candidate
            .matched_cwd
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        let first_path = candidate
            .paths
            .first()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "  {}  updated={}  rows={}  cwd={}  path={}\n",
            candidate.id,
            candidate.updated_at.to_rfc3339(),
            rows,
            cwd,
            first_path
        ));
    }
    out
}

fn resolved_roots_lines(roots: &[PathBuf]) -> String {
    if roots.is_empty() {
        return "  -".to_string();
    }
    roots
        .iter()
        .map(|root| format!("  {}", root.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_json_entries_sorted(dir: &Path) -> Vec<(PathBuf, Value, String)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut values = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return None;
            }
            let raw = fs::read_to_string(&path).ok()?;
            let value = serde_json::from_str::<Value>(&raw).ok()?;
            Some((path, value, raw))
        })
        .collect::<Vec<_>>();
    values.sort_by_key(|(_, value, _)| opencode_time_value(value));
    values
}

fn empty_import_parse_result(updated_at: DateTime<Utc>) -> ImportParseResult {
    ImportParseResult {
        rows_seen: 0,
        events: Vec::new(),
        source_started_at: None,
        source_updated_at: Some(updated_at),
    }
}

fn source_time_bounds(
    events: &[ImportedEvent],
    fallback_updated_at: DateTime<Utc>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let mut timestamps = events.iter().filter_map(|event| event.timestamp);
    let Some(first) = timestamps.next() else {
        return (None, Some(fallback_updated_at));
    };
    let mut min = first;
    let mut max = first;
    for timestamp in timestamps {
        min = min.min(timestamp);
        max = max.max(timestamp);
    }
    (Some(min), Some(max.max(fallback_updated_at)))
}

fn import_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    for pointer in [
        "/timestamp",
        "/created_at",
        "/createdAt",
        "/updated_at",
        "/updatedAt",
        "/message/created_at",
        "/message/timestamp",
    ] {
        if let Some(timestamp) = value.pointer(pointer)
            && let Some(parsed) = timestamp_from_value(timestamp)
        {
            return Some(parsed);
        }
    }
    for pointer in [
        "/time/created",
        "/time/start",
        "/time/end",
        "/time/updated",
        "/created",
        "/start",
        "/end",
    ] {
        if let Some(timestamp) = value
            .pointer(pointer)
            .and_then(Value::as_i64)
            .and_then(DateTime::<Utc>::from_timestamp_millis)
        {
            return Some(timestamp);
        }
    }
    None
}

fn timestamp_from_value(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(raw) = value.as_str() {
        return DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|timestamp| timestamp.with_timezone(&Utc));
    }
    value
        .as_i64()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
}

fn import_usage_for_schema(schema: &str, value: &Value) -> Option<ImportedUsage> {
    match schema {
        "codex-cli" => value.get("payload").and_then(codex_usage),
        "gemini" => gemini_usage(value),
        _ => value
            .get("usage")
            .or_else(|| value.pointer("/message/usage"))
            .or_else(|| value.get("tokens"))
            .and_then(import_usage_from_value),
    }
}

fn codex_usage(payload: &Value) -> Option<ImportedUsage> {
    let usage = payload
        .pointer("/info/total_token_usage")
        .or_else(|| payload.get("usage"))
        .unwrap_or(payload);
    let mut imported = import_usage_from_value(usage)?;
    imported.context_window = payload
        .pointer("/info/model_context_window")
        .and_then(Value::as_u64)
        .or(imported.context_window);
    Some(imported)
}

fn gemini_usage(value: &Value) -> Option<ImportedUsage> {
    let tokens = value.get("tokens")?;
    let input = tokens.get("input").and_then(Value::as_u64);
    let output = tokens.get("output").and_then(Value::as_u64);
    let cache = tokens.get("cached").and_then(Value::as_u64);
    if input.is_none() && output.is_none() && cache.is_none() {
        return None;
    }
    Some(ImportedUsage {
        input_tokens: input,
        output_tokens: output,
        cache_tokens: cache,
        context_window: Some(1_000_000),
    })
}

fn import_usage_from_value(value: &Value) -> Option<ImportedUsage> {
    let input = number_field_any(
        value,
        &[
            "inputTokens",
            "input_tokens",
            "input",
            "prompt_tokens",
            "promptTokens",
        ],
    );
    let output = number_field_any(
        value,
        &[
            "outputTokens",
            "output_tokens",
            "output",
            "completion_tokens",
            "completionTokens",
        ],
    );
    let cache_read = number_field_any(
        value,
        &[
            "cacheReadTokens",
            "cache_read_tokens",
            "cache_read_input_tokens",
            "cacheRead",
            "cache.read",
        ],
    )
    .unwrap_or(0);
    let cache_write = number_field_any(
        value,
        &[
            "cacheCreationTokens",
            "cacheWriteTokens",
            "cache_creation_tokens",
            "cache_creation_input_tokens",
            "cache_write_tokens",
            "cacheCreation",
            "cacheWrite",
            "cache.write",
        ],
    )
    .unwrap_or(0);
    let cache = (cache_read + cache_write > 0).then_some(cache_read + cache_write);
    if input.is_none() && output.is_none() && cache.is_none() {
        return None;
    }
    Some(ImportedUsage {
        input_tokens: input,
        output_tokens: output,
        cache_tokens: cache,
        context_window: number_field_any(value, &["context_window", "contextWindow"]),
    })
}

fn import_source_event(value: &Value) -> String {
    value
        .get("type")
        .or_else(|| value.pointer("/payload/type"))
        .or_else(|| value.pointer("/message/role"))
        .and_then(Value::as_str)
        .unwrap_or("row")
        .to_string()
}

fn import_summary(schema: &str, value: &Value) -> String {
    for pointer in [
        "/content",
        "/message/content",
        "/payload/message",
        "/payload/output",
        "/data/content",
        "/data/text",
        "/data/result",
        "/text",
        "/summary",
    ] {
        if let Some(text) = value.pointer(pointer).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return one_line(text, 180);
        }
    }
    if let Some(tool) = import_tool_name(value) {
        return format!("tool {}", provider_tool_label(&tool));
    }
    if let Some(usage) = import_usage_for_schema(schema, value) {
        return format!(
            "tokens input {} output {} cache {}",
            usage.input_tokens.unwrap_or(0),
            usage.output_tokens.unwrap_or(0),
            usage.cache_tokens.unwrap_or(0)
        );
    }
    one_line(&json_value_text(value), 180)
}

fn import_tool_name(value: &Value) -> Option<String> {
    for pointer in [
        "/tool_name",
        "/toolName",
        "/name",
        "/payload/name",
        "/data/name",
        "/tool",
    ] {
        if let Some(name) = value.pointer(pointer).and_then(Value::as_str)
            && !name.trim().is_empty()
        {
            return Some(name.to_string());
        }
    }
    None
}

fn import_tool_call_id(value: &Value) -> Option<String> {
    for pointer in [
        "/tool_call_id",
        "/toolCallId",
        "/call_id",
        "/callId",
        "/id",
        "/payload/call_id",
        "/payload/id",
        "/data/toolCallId",
        "/data/id",
    ] {
        if let Some(id) = value.pointer(pointer).and_then(Value::as_str)
            && !id.trim().is_empty()
        {
            return Some(id.to_string());
        }
    }
    value
        .get("source_rowid")
        .and_then(Value::as_u64)
        .map(|row| format!("cursor-row-{row}"))
}

fn collect_import_paths(value: &Value) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    collect_import_paths_inner(value, None, &mut paths);
    paths.into_iter().collect()
}

fn collect_import_paths_inner(value: &Value, key: Option<&str>, paths: &mut BTreeSet<PathBuf>) {
    match value {
        Value::String(text) => {
            if key.is_some_and(import_path_key) && looks_like_import_path(text) {
                paths.insert(PathBuf::from(text));
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_import_paths_inner(item, key, paths);
            }
        }
        Value::Object(map) => {
            for (child_key, child_value) in map {
                collect_import_paths_inner(child_value, Some(child_key), paths);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn import_path_key(key: &str) -> bool {
    matches!(
        key,
        "path"
            | "file"
            | "files"
            | "file_path"
            | "filePath"
            | "notebook_path"
            | "notebookPath"
            | "target_file"
            | "targetFile"
            | "source_file"
            | "sourceFile"
            | "destination"
            | "dest"
            | "uri"
            | "paths"
    )
}

fn looks_like_import_path(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 512
        && !trimmed.contains('\n')
        && !trimmed.starts_with("http://")
        && !trimmed.starts_with("https://")
        && (trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed.contains('.')
            || trimmed.starts_with('~'))
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
