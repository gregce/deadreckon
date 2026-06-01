use super::super::*;

const PROJECT_ACCEPTANCE_DIR: &str = ".deadreckon";
const PROJECT_ACCEPTANCE_YAML: &str = "acceptance.yaml";
const PROJECT_ACCEPTANCE_MD: &str = "acceptance.md";
const PROJECT_ACCEPTANCE_HELPERS: &str = "acceptance";

#[derive(Clone, Debug)]
pub(crate) struct AcceptanceSource {
    pub(crate) path: PathBuf,
    source: setup::DoneCriteriaSource,
    companion_doc: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct AcceptanceDraft {
    pub(crate) yaml: String,
    pub(crate) markdown: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
}

pub(crate) async fn acceptance_command(command: AcceptanceCommand) -> Result<()> {
    match command {
        AcceptanceCommand::Setup {
            request,
            provider,
            model,
            force,
        } => {
            acceptance_agent_command(AcceptanceAgentMode::Draft, request, provider, model, force)
                .await
        }
        AcceptanceCommand::Add {
            request,
            provider,
            model,
            force,
        } => acceptance_add_command(request, provider, model, force).await,
        AcceptanceCommand::Init { preset, force } => acceptance_init_command(preset, force),
        AcceptanceCommand::Draft {
            request,
            provider,
            model,
            force,
        } => {
            acceptance_agent_command(AcceptanceAgentMode::Draft, request, provider, model, force)
                .await
        }
        AcceptanceCommand::Refine {
            request,
            provider,
            model,
            force,
        } => {
            acceptance_agent_command(AcceptanceAgentMode::Refine, request, provider, model, force)
                .await
        }
        AcceptanceCommand::Explain { spec } => acceptance_explain_command(spec),
        AcceptanceCommand::Check { spec, against } => acceptance_check_command(spec, against),
    }
}

pub(crate) async fn done_command(
    args: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
    spec: Option<PathBuf>,
    against: Option<PathBuf>,
) -> Result<()> {
    let Some(first) = args.first().map(String::as_str) else {
        return acceptance_explain_command(spec);
    };
    match first {
        "add" => {
            let request = args.iter().skip(1).cloned().collect::<Vec<_>>();
            if request.is_empty() {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "done add needs a criterion",
                    "deadreckon def-done add \"users can save drawings\"",
                )));
            }
            acceptance_add_command(request, provider, model, force).await
        }
        "check" => acceptance_check_command(spec, against),
        "show" | "explain" => acceptance_explain_command(spec),
        "edit" | "refine" => {
            let request = args.iter().skip(1).cloned().collect::<Vec<_>>();
            if request.is_empty() {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "done edit needs a requested change",
                    "deadreckon def-done edit \"also require the gallery to persist\"",
                )));
            }
            acceptance_agent_command(AcceptanceAgentMode::Refine, request, provider, model, force)
                .await
        }
        "help" => {
            print_done_help();
            Ok(())
        }
        _ => {
            acceptance_agent_command(AcceptanceAgentMode::Draft, args, provider, model, true).await
        }
    }
}

fn print_done_help() {
    println!("{}", ui_heading("deadreckon def-done"));
    println!("usage:");
    println!(
        "  {}",
        ui_command("deadreckon def-done \"builds, opens in a browser, and has no console errors\"")
    );
    println!(
        "  {}",
        ui_command("deadreckon def-done add \"users can save drawings\"")
    );
    println!("  {}", ui_command("deadreckon def-done check"));
    println!("  {}", ui_command("deadreckon def-done show"));
}

#[derive(Clone, Copy)]
pub(crate) enum AcceptanceAgentMode {
    Draft,
    Refine,
}

fn acceptance_init_command(preset: AcceptancePreset, force: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let preset = match preset {
        AcceptancePreset::Auto => detect_acceptance_preset(&cwd),
        other => other,
    };
    let draft = acceptance_template_for_preset(preset, &cwd);
    write_project_acceptance(&cwd, &draft, force, false)?;
    print_acceptance_written(&cwd, "template", acceptance_check_count(&draft.yaml)?);
    Ok(())
}

async fn acceptance_agent_command(
    mode: AcceptanceAgentMode,
    request: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    acceptance_agent_command_in_dir(&cwd, mode, request, provider, model, force).await
}

pub(crate) async fn acceptance_agent_command_in_dir(
    cwd: &Path,
    mode: AcceptanceAgentMode,
    request: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let yaml_path = project_acceptance_yaml(cwd);
    let md_path = project_acceptance_md(cwd);
    let existing_yaml = read_optional_text(&yaml_path)?;
    let existing_md = read_optional_text(&md_path)?;
    if matches!(mode, AcceptanceAgentMode::Refine) && existing_yaml.is_none() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "no project done criteria found",
            "deadreckon def-done \"what should count as done\"",
        )));
    }
    let request = acceptance_request_text(&request, mode)?;
    if !force && yaml_path.exists() && matches!(mode, AcceptanceAgentMode::Draft) {
        return Err(CliError::Core(deadreckon_core::user_error(
            ".deadreckon/acceptance.yaml already exists",
            "deadreckon def-done add \"one more criterion\" or rerun with --overwrite",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let selected_provider = provider.or(defaults.doc_provider).or(defaults.provider);
    let router = ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        selected_provider.as_deref(),
        model.as_deref(),
    )?;
    let route = router.selected_route_info();
    let prompt = acceptance_agent_prompt(
        mode,
        &request,
        cwd,
        existing_yaml.as_deref(),
        existing_md.as_deref(),
    )?;
    let response = with_cli_wait_status(
        match mode {
            AcceptanceAgentMode::Draft => "compiling done criteria",
            AcceptanceAgentMode::Refine => "refining done criteria",
        },
        router.complete(&ProviderRequest {
            prompt,
            max_output_tokens: 6_000,
            cwd: Some(cwd.to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            pid_file: None,
            cancellation_token: None,
        }),
    )
    .await
    .map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("done criteria provider failed: {err}"),
            "deadreckon def-done \"builds and passes tests\"",
        ))
    })?;
    let draft = parse_acceptance_agent_response(&response.content)?;
    acceptance_check_count(&draft.yaml)?;
    write_project_acceptance(cwd, &draft, true, true)?;
    let route_label = route
        .map(|route| format!("{} / {}", route.name, route.model))
        .unwrap_or_else(|| "configured provider".to_string());
    print_acceptance_written(
        cwd,
        &format!("agent draft via {route_label}"),
        acceptance_check_count(&draft.yaml)?,
    );
    Ok(())
}

async fn acceptance_add_command(
    request: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let joined = request.join(" ");
    if let Some(pack) = AcceptancePack::from_request(&joined) {
        return acceptance_add_pack_command(&cwd, pack, force);
    }
    let mode = if project_acceptance_yaml(&cwd).exists() {
        AcceptanceAgentMode::Refine
    } else {
        AcceptanceAgentMode::Draft
    };
    acceptance_agent_command_in_dir(&cwd, mode, request, provider, model, force).await
}

fn acceptance_add_pack_command(cwd: &Path, pack: AcceptancePack, force: bool) -> Result<()> {
    let mut draft = if project_acceptance_yaml(cwd).exists() {
        let yaml = fs::read_to_string(project_acceptance_yaml(cwd))?;
        let markdown = read_optional_text(&project_acceptance_md(cwd))?
            .unwrap_or_else(|| acceptance_markdown_from_yaml(&yaml));
        AcceptanceDraft {
            yaml,
            markdown,
            files: BTreeMap::new(),
        }
    } else {
        AcceptanceDraft {
            yaml: "name: project acceptance\nchecks: []\n".to_string(),
            markdown: "# Done Criteria\n\n".to_string(),
            files: BTreeMap::new(),
        }
    };
    let pack_draft = acceptance_pack_draft(pack, cwd);
    draft.yaml = append_acceptance_checks(&draft.yaml, &pack_draft.yaml)?;
    if !draft.markdown.ends_with('\n') {
        draft.markdown.push('\n');
    }
    draft.markdown.push('\n');
    draft.markdown.push_str(pack_draft.markdown.trim());
    draft.markdown.push('\n');
    draft.files.extend(pack_draft.files);
    write_project_acceptance(cwd, &draft, force, true)?;
    print_acceptance_written(
        cwd,
        &format!("{} pack", pack.name()),
        acceptance_check_count(&draft.yaml)?,
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum AcceptancePack {
    Auto,
    Basic,
    Build,
    Test,
    Rust,
    Node,
    StaticSite,
    Browser,
    Playwright,
    Vite,
    NextJs,
    Python,
}

impl AcceptancePack {
    fn from_request(request: &str) -> Option<Self> {
        let normalized = request.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "auto" => Some(Self::Auto),
            "basic" => Some(Self::Basic),
            "build" => Some(Self::Build),
            "test" | "tests" => Some(Self::Test),
            "rust" | "cargo" => Some(Self::Rust),
            "node" | "npm" | "javascript" | "typescript" => Some(Self::Node),
            "static" | "static-site" | "static site" | "html" => Some(Self::StaticSite),
            "browser" | "smoke" | "browser-smoke" => Some(Self::Browser),
            "playwright" | "e2e" => Some(Self::Playwright),
            "vite" => Some(Self::Vite),
            "next" | "nextjs" | "next.js" => Some(Self::NextJs),
            "python" | "py" => Some(Self::Python),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Basic => "basic",
            Self::Build => "build",
            Self::Test => "test",
            Self::Rust => "rust",
            Self::Node => "node",
            Self::StaticSite => "static-site",
            Self::Browser => "browser",
            Self::Playwright => "playwright",
            Self::Vite => "vite",
            Self::NextJs => "nextjs",
            Self::Python => "python",
        }
    }
}

// SAFETY: Acceptance paths are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn acceptance_explain_command(spec: Option<PathBuf>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let path = resolve_acceptance_path_for_command(&cwd, spec.as_deref())?;
    if let Some(path) = path {
        let raw = fs::read_to_string(&path)?;
        let count = acceptance_check_count(&raw)?;
        println!("{}", ui_heading("done criteria"));
        println!("  spec:   {}", path.display());
        println!("  checks: {count}");
        if path == project_acceptance_yaml(&cwd)
            && let Some(markdown) = read_optional_text(&project_acceptance_md(&cwd))?
        {
            println!();
            println!("{}", markdown.trim());
        }
        println!();
        print_acceptance_yaml_summary(&raw)?;
    } else {
        println!("{}", ui_heading("done criteria"));
        println!("  spec:   default dr-gate behavior");
        println!("  checks: working directory exists, or cargo test when Cargo.toml is present");
        println!();
        println!(
            "{}",
            ui_command("deadreckon def-done \"what should count as done\"")
        );
    }
    Ok(())
}

// SAFETY: Acceptance check paths are owned clap values at the command boundary.
#[allow(clippy::needless_pass_by_value)]
fn acceptance_check_command(spec: Option<PathBuf>, against: Option<PathBuf>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let working_dir = against.unwrap_or(cwd.clone());
    let spec_path = resolve_acceptance_path_for_command(&cwd, spec.as_deref())?;
    let temp_root = std::env::temp_dir().join(format!(
        "deadreckon-acceptance-check-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&temp_root)?;
    if let Some(spec_path) = spec_path.as_ref() {
        let raw = fs::read_to_string(spec_path)?;
        validate_acceptance_yaml_integrity(&raw)?;
        fs::copy(spec_path, acceptance_spec_path_for_run_root(&temp_root))?;
    }
    let result = evaluate_acceptance_checks(&temp_root, &working_dir);
    let _ = fs::remove_dir_all(&temp_root);
    match result {
        Ok(results) => {
            let failed_required = results
                .iter()
                .any(|result| result.must_pass && !result.passed);
            if failed_required {
                println!("{}", ui_status("done criteria failed"));
            } else {
                println!("{}", ui_ok("done criteria passed"));
            }
            println!("working {}", working_dir.display());
            if let Some(spec_path) = spec_path {
                println!("spec    {}", spec_path.display());
            } else {
                println!("spec    default dr-gate behavior");
            }
            print_acceptance_results(&results);
            if let Some(failed) = results
                .iter()
                .find(|result| result.must_pass && !result.passed)
            {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("required done criterion failed: {}", failed.detail),
                    "fix the project or run `deadreckon def-done edit \"tighten or correct the checks\"`",
                )));
            }
            Ok(())
        }
        Err(err) => Err(CliError::Core(deadreckon_core::user_error(
            &format!("done criteria check failed: {err}"),
            "fix the project or edit .deadreckon/acceptance.yaml, then rerun `deadreckon def-done check`",
        ))),
    }
}

pub(crate) fn print_acceptance_results(results: &[deadreckon_core::AcceptanceCheckResult]) {
    for result in results {
        let mark = if result.passed {
            ui_ok("✓")
        } else if result.must_pass {
            ui::render(ui::Stream::Stdout, ui::Tone::Negative, "✗")
        } else {
            ui_warn("!")
        };
        let requirement = if result.must_pass {
            "required"
        } else {
            "optional"
        };
        let elapsed = result
            .duration_ms
            .map(|ms| format!(" ({:.1}s)", ms as f64 / 1000.0))
            .unwrap_or_default();
        println!(
            "  {mark} {:<13} {:<8} {}{}",
            result.kind, requirement, result.detail, elapsed
        );
        if !result.passed {
            if let Some(command) = result.command.as_deref() {
                println!("      command: {}", ui_command(command));
            }
            if let Some(stderr) = result.stderr.as_deref() {
                println!("      stderr:  {}", one_line(stderr, 140));
            }
            if let Some(stdout) = result.stdout.as_deref() {
                println!("      stdout:  {}", one_line(stdout, 140));
            }
        }
    }
}

fn acceptance_request_text(request: &[String], mode: AcceptanceAgentMode) -> Result<String> {
    let joined = request.join(" ").trim().to_string();
    if !joined.is_empty() {
        return Ok(joined);
    }
    if io::stdin().is_terminal() {
        let prompt_text = match mode {
            AcceptanceAgentMode::Draft => "what should count as done? ",
            AcceptanceAgentMode::Refine => "how should done criteria change? ",
        };
        let answer = prompt::open(prompt_text, None)?;
        if !answer.trim().is_empty() {
            return Ok(answer.trim().to_string());
        }
    }
    match mode {
        AcceptanceAgentMode::Draft => Ok(
            "Draft practical acceptance criteria for this project and its likely build/test flow."
                .to_string(),
        ),
        AcceptanceAgentMode::Refine => Err(CliError::Core(deadreckon_core::user_error(
            "refine needs a requested change",
            "deadreckon def-done add \"also require tests for the gallery\"",
        ))),
    }
}

fn acceptance_agent_prompt(
    mode: AcceptanceAgentMode,
    request: &str,
    cwd: &Path,
    existing_yaml: Option<&str>,
    existing_md: Option<&str>,
) -> Result<String> {
    let mode_label = match mode {
        AcceptanceAgentMode::Draft => "draft",
        AcceptanceAgentMode::Refine => "refine",
    };
    let project = acceptance_project_summary(cwd)?;
    Ok(format!(
        "\
You are helping configure deadreckon acceptance criteria for an unattended coding run.
The user writes acceptance in plain English. Convert it into executable checks that dr-gate can run.

Return JSON only, with exactly these keys:
{{\"acceptance_yaml\":\"...\",\"acceptance_md\":\"...\",\"files\":{{}}}}

The YAML must be valid deadreckon acceptance.yaml. Use only these check kinds:
- file_exists with path
- content_match with path and pattern
- shell with command and optional cwd
- cargo_test

Use {{working_dir}} for paths inside the run. Prefer stable, automatable checks over subjective claims.
Do not include self-attestation checks, provider-output checks, or instructions that the agent can satisfy by writing a marker.
For Node projects, prefer `npm run build --if-present` and `npm test --if-present` shell checks.
For static apps, require the main HTML/CSS/JS files and one or two content_match checks for requested behavior.
If a criterion needs a helper script, include it in files under `.deadreckon/acceptance/` and call it from a shell check.
The acceptance_md must restate the user's criteria in readable English before listing the executable checks.
Keep the YAML concise and include at least one required check.

Mode: {mode_label}
User request: {request}

Project summary:
{project}

Existing acceptance.yaml:
{existing_yaml}

Existing acceptance.md:
{existing_md}
",
        existing_yaml = existing_yaml.unwrap_or("(none)"),
        existing_md = existing_md.unwrap_or("(none)")
    ))
}

fn acceptance_project_summary(cwd: &Path) -> Result<String> {
    let mut lines = vec![format!("root: {}", cwd.display())];
    if cwd.join("Cargo.toml").exists() {
        lines.push("stack: rust".to_string());
    }
    if cwd.join("package.json").exists() {
        lines.push("stack: node".to_string());
        if let Ok(package) = fs::read_to_string(cwd.join("package.json"))
            && let Ok(value) = serde_json::from_str::<Value>(&package)
            && let Some(scripts) = value.get("scripts").and_then(Value::as_object)
        {
            let mut script_names = scripts.keys().cloned().collect::<Vec<_>>();
            script_names.sort();
            lines.push(format!("package scripts: {}", script_names.join(", ")));
        }
    }
    let files = project_file_sample(cwd, 80)?;
    lines.push("files:".to_string());
    for file in files {
        lines.push(format!("  - {}", file.display()));
    }
    Ok(lines.join("\n"))
}

fn project_file_sample(cwd: &Path, limit: usize) -> Result<Vec<PathBuf>> {
    fn walk(
        root: &Path,
        current: &Path,
        depth: usize,
        out: &mut Vec<PathBuf>,
        limit: usize,
    ) -> io::Result<()> {
        if depth > 3 || out.len() >= limit {
            return Ok(());
        }
        let mut entries = fs::read_dir(current)?.collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if out.len() >= limit {
                break;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git" | "target" | "node_modules" | ".deadreckon" | "dist" | "build"
            ) {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, depth + 1, out, limit)?;
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_path_buf());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(cwd, cwd, 0, &mut files, limit)?;
    Ok(files)
}

fn parse_acceptance_agent_response(content: &str) -> Result<AcceptanceDraft> {
    let cleaned = strip_code_fence(content.trim());
    if let Ok(value) = serde_json::from_str::<Value>(&cleaned)
        && let Some(parsed) = acceptance_json_payload(&value)?
    {
        return Ok(parsed);
    }
    if let Some(json) = extract_json_object(&cleaned)
        && let Ok(value) = serde_json::from_str::<Value>(&json)
        && let Some(parsed) = acceptance_json_payload(&value)?
    {
        return Ok(parsed);
    }
    if let Some(yaml) = extract_fenced_block(&cleaned, &["yaml", "yml"]).or_else(|| {
        if cleaned.contains("checks:") {
            Some(cleaned.clone())
        } else {
            None
        }
    }) {
        let markdown = extract_fenced_block(&cleaned, &["markdown", "md"])
            .unwrap_or_else(|| acceptance_markdown_from_yaml(&yaml));
        acceptance_check_count(&yaml)?;
        return Ok(AcceptanceDraft {
            yaml,
            markdown,
            files: BTreeMap::new(),
        });
    }
    Err(CliError::Core(deadreckon_core::user_error(
        "provider did not return acceptance JSON or YAML",
        "rerun `deadreckon def-done ...` or use `deadreckon def-done check` after editing criteria",
    )))
}

fn acceptance_json_payload(value: &Value) -> Result<Option<AcceptanceDraft>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let yaml = object
        .get("acceptance_yaml")
        .or_else(|| object.get("yaml"))
        .and_then(Value::as_str);
    let Some(yaml) = yaml else {
        return Ok(None);
    };
    let markdown = object
        .get("acceptance_md")
        .or_else(|| object.get("markdown"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| acceptance_markdown_from_yaml(yaml));
    acceptance_check_count(yaml)?;
    let mut files = BTreeMap::new();
    if let Some(file_map) = object.get("files").and_then(Value::as_object) {
        for (path, body) in file_map {
            let Some(body) = body.as_str() else {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("acceptance helper {path} must be a string"),
                    "return files as {\".deadreckon/acceptance/name\": \"contents\"}",
                )));
            };
            let path = PathBuf::from(path);
            validate_acceptance_helper_path(&path)?;
            files.insert(path, body.to_string());
        }
    }
    Ok(Some(AcceptanceDraft {
        yaml: yaml.to_string(),
        markdown,
        files,
    }))
}

fn strip_code_fence(value: &str) -> String {
    let trimmed = value.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines = trimmed.lines().collect::<Vec<_>>();
    if lines.len() >= 2 && lines.last().is_some_and(|line| line.trim() == "```") {
        lines.remove(0);
        lines.pop();
        return lines.join("\n");
    }
    trimmed.to_string()
}

fn extract_json_object(value: &str) -> Option<String> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(value[start..=end].to_string())
}

fn extract_fenced_block(value: &str, languages: &[&str]) -> Option<String> {
    let mut in_block = false;
    let mut capture = false;
    let mut lines = Vec::new();
    for line in value.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_block {
                if capture {
                    return Some(lines.join("\n"));
                }
                in_block = false;
                continue;
            }
            in_block = true;
            capture = languages
                .iter()
                .any(|language| rest.trim().eq_ignore_ascii_case(language));
            lines.clear();
            continue;
        }
        if in_block && capture {
            lines.push(line);
        }
    }
    None
}

pub(crate) async fn ensure_acceptance_before_start(
    cwd: &Path,
    override_path: Option<&Path>,
    goal: &str,
    provider: Option<String>,
    model: Option<String>,
    skip_prompt: bool,
    noun: &str,
) -> Result<Option<AcceptanceSource>> {
    let existing = resolve_acceptance_source(cwd, override_path)?;
    if existing.is_some() || override_path.is_some() || skip_prompt || !io::stdin().is_terminal() {
        return Ok(existing);
    }
    println!("{}", ui_heading("done criteria"));
    println!("No done criteria found.");
    println!("Write the definition of done in English; deadreckon will compile it for dr-gate.");
    if !prompt::confirm(&format!("write done criteria before this {noun}?"), true)? {
        println!("using default gate: working directory exists, or cargo test for Rust projects");
        return Ok(existing);
    }
    let request = prompt::open("definition of done (Enter for a practical default): ", None)?;
    let request = if request.trim().is_empty() {
        format!("For this {noun}, define practical acceptance checks for: {goal}")
    } else {
        request.trim().to_string()
    };
    match acceptance_agent_command_in_dir(
        cwd,
        AcceptanceAgentMode::Draft,
        vec![request],
        provider,
        model,
        false,
    )
    .await
    {
        Ok(()) => resolve_acceptance_source(cwd, None).map(mark_generated_done_criteria),
        Err(err) => {
            println!("{}", ui_status("done criteria draft failed"));
            println!("  {err}");
            if !prompt::confirm("use a detected local check template instead?", true)? {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "done criteria were requested but not configured",
                    "rerun `deadreckon def-done \"what should count as done\"` or answer yes to the detected template fallback",
                )));
            }
            let preset = detect_acceptance_preset(cwd);
            let draft = acceptance_template_for_preset(preset, cwd);
            write_project_acceptance(cwd, &draft, false, false)?;
            print_acceptance_written(
                cwd,
                "detected template",
                acceptance_check_count(&draft.yaml)?,
            );
            resolve_acceptance_source(cwd, None).map(mark_generated_done_criteria)
        }
    }
}

pub(crate) fn resolve_acceptance_source(
    cwd: &Path,
    override_path: Option<&Path>,
) -> Result<Option<AcceptanceSource>> {
    if let Some(path) = override_path {
        let path = absolute_from(cwd, path);
        if !path.is_file() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("done criteria file not found: {}", path.display()),
                "deadreckon def-done \"what should count as done\"",
            )));
        }
        return Ok(Some(AcceptanceSource {
            path,
            source: setup::DoneCriteriaSource::ExplicitPath,
            companion_doc: None,
        }));
    }
    let project_yaml = project_acceptance_yaml(cwd);
    if project_yaml.is_file() {
        let project_md = project_acceptance_md(cwd);
        return Ok(Some(AcceptanceSource {
            path: project_yaml,
            source: setup::DoneCriteriaSource::ProjectFile,
            companion_doc: project_md.is_file().then_some(project_md),
        }));
    }
    Ok(None)
}

pub(crate) fn mark_generated_done_criteria(
    source: Option<AcceptanceSource>,
) -> Option<AcceptanceSource> {
    source.map(|mut source| {
        source.source = setup::DoneCriteriaSource::Generated;
        source
    })
}

pub(crate) fn done_criteria_selection(
    source: &Option<AcceptanceSource>,
) -> Result<setup::DoneCriteriaSelection> {
    match source {
        Some(source) => {
            let raw = fs::read_to_string(&source.path)?;
            let checks = Some(acceptance_check_count(&raw)?);
            Ok(match source.source {
                setup::DoneCriteriaSource::ExplicitPath => setup::DoneCriteriaSelection::explicit(
                    source.path.clone(),
                    source.companion_doc.clone(),
                    checks,
                ),
                setup::DoneCriteriaSource::ProjectFile => setup::DoneCriteriaSelection::project(
                    source.path.clone(),
                    source.companion_doc.clone(),
                    checks,
                ),
                setup::DoneCriteriaSource::Generated => setup::DoneCriteriaSelection::generated(
                    source.path.clone(),
                    source.companion_doc.clone(),
                    checks,
                ),
                setup::DoneCriteriaSource::DefaultGate => {
                    setup::DoneCriteriaSelection::default_gate()
                }
            })
        }
        None => Ok(setup::DoneCriteriaSelection::default_gate()),
    }
}

pub(crate) fn copy_acceptance_into_run(
    state: &deadreckon_core::PipelineState,
    source: &Option<AcceptanceSource>,
) -> Result<()> {
    let Some(source) = source else {
        return Ok(());
    };
    fs::copy(
        &source.path,
        acceptance_spec_path_for_run_root(&state.run_root),
    )?;
    if let Some(doc) = source.companion_doc.as_ref() {
        fs::copy(doc, state.run_root.join(PROJECT_ACCEPTANCE_MD))?;
    }
    let Some(project_dir) = source.path.parent() else {
        return Ok(());
    };
    let helper_source = project_dir.join(PROJECT_ACCEPTANCE_HELPERS);
    if helper_source.is_dir() {
        let helper_dest = state
            .working_dir
            .join(PROJECT_ACCEPTANCE_DIR)
            .join(PROJECT_ACCEPTANCE_HELPERS);
        if !same_path_best_effort(&helper_source, &helper_dest) {
            copy_tree(&helper_source, &helper_dest)?;
        }
    }
    Ok(())
}

pub(crate) fn copy_existing_acceptance_into_run(
    state: &deadreckon_core::PipelineState,
    candidate_roots: &[&Path],
) -> Result<()> {
    let source = resolve_existing_acceptance_source(candidate_roots)?;
    copy_acceptance_into_run(state, &source)
}

fn resolve_existing_acceptance_source(
    candidate_roots: &[&Path],
) -> Result<Option<AcceptanceSource>> {
    for root in candidate_roots {
        if let Some(source) = resolve_acceptance_source(root, None)? {
            return Ok(Some(source));
        }
    }
    Ok(None)
}

fn same_path_best_effort(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn resolve_acceptance_path_for_command(cwd: &Path, spec: Option<&Path>) -> Result<Option<PathBuf>> {
    if let Some(spec) = spec {
        let path = absolute_from(cwd, spec);
        if !path.is_file() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("done criteria file not found: {}", path.display()),
                "deadreckon def-done \"what should count as done\"",
            )));
        }
        return Ok(Some(path));
    }
    let project = project_acceptance_yaml(cwd);
    Ok(project.is_file().then_some(project))
}

fn absolute_from(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn project_acceptance_yaml(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_ACCEPTANCE_DIR)
        .join(PROJECT_ACCEPTANCE_YAML)
}

fn project_acceptance_md(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_ACCEPTANCE_DIR).join(PROJECT_ACCEPTANCE_MD)
}

pub(crate) fn write_project_acceptance(
    cwd: &Path,
    draft: &AcceptanceDraft,
    force: bool,
    allow_existing: bool,
) -> Result<()> {
    acceptance_check_count(&draft.yaml)?;
    validate_acceptance_yaml_integrity(&draft.yaml)?;
    let dir = cwd.join(PROJECT_ACCEPTANCE_DIR);
    let yaml_path = project_acceptance_yaml(cwd);
    let md_path = project_acceptance_md(cwd);
    if !allow_existing && !force && (yaml_path.exists() || md_path.exists()) {
        return Err(CliError::Core(deadreckon_core::user_error(
            ".deadreckon/acceptance files already exist",
            "deadreckon def-done add \"one more criterion\" or rerun with --overwrite",
        )));
    }
    fs::create_dir_all(&dir)?;
    fs::write(&yaml_path, ensure_trailing_newline(&draft.yaml))?;
    fs::write(&md_path, ensure_trailing_newline(&draft.markdown))?;
    for (relative_path, body) in &draft.files {
        let path = validate_acceptance_helper_path(relative_path)?;
        let absolute = cwd.join(path);
        if !force && absolute.exists() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("acceptance helper already exists: {}", absolute.display()),
                "rerun with --overwrite or edit the helper manually",
            )));
        }
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(absolute, ensure_trailing_newline(body))?;
    }
    Ok(())
}

fn validate_acceptance_helper_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("invalid acceptance helper path: {}", path.display()),
            "helper files must live under .deadreckon/acceptance/",
        )));
    }
    let required_prefix = Path::new(PROJECT_ACCEPTANCE_DIR).join(PROJECT_ACCEPTANCE_HELPERS);
    if !path.starts_with(&required_prefix) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("invalid acceptance helper path: {}", path.display()),
            "helper files must live under .deadreckon/acceptance/",
        )));
    }
    Ok(path.to_path_buf())
}

fn print_acceptance_written(cwd: &Path, source: &str, checks: usize) {
    let yaml_path = project_acceptance_yaml(cwd);
    let notes_path = project_acceptance_md(cwd);
    let primary = "deadreckon def-done check";
    print!(
        "{}",
        VerdictSurface::try_new(
            VerdictKind::Completed,
            "def-done",
            None,
            ExplanationPanel::new(
                "DeadReckon wrote project done criteria and companion notes.",
                "The criteria are configured; check them once before launching a long run.",
                vec![
                    ("source", source.to_string()),
                    ("checks", checks.to_string()),
                    ("yaml", yaml_path.display().to_string()),
                    ("notes", notes_path.display().to_string()),
                ],
            ),
            vec![("Recommended", primary)],
            vec![("Secondary", "deadreckon run \"goal\"")],
        )
        .expect("acceptance written verdict surface must be valid")
        .render_plain(!completion_hints_enabled(false))
    );
}

fn detect_acceptance_preset(cwd: &Path) -> AcceptancePreset {
    if cwd.join("Cargo.toml").exists() {
        AcceptancePreset::Rust
    } else if cwd.join("package.json").exists() {
        AcceptancePreset::Node
    } else if cwd.join("index.html").exists() || cwd.join("public/index.html").exists() {
        AcceptancePreset::StaticSite
    } else {
        AcceptancePreset::Basic
    }
}

pub(crate) fn acceptance_template_for_preset(
    preset: AcceptancePreset,
    cwd: &Path,
) -> AcceptanceDraft {
    let yaml = match preset {
        AcceptancePreset::Auto => unreachable!("auto is resolved before template generation"),
        AcceptancePreset::Rust => {
            "\
name: rust project acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/Cargo.toml\"
  - kind: cargo_test
"
        }
        AcceptancePreset::Node => {
            "\
name: node project acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/package.json\"
  - kind: shell
    command: \"npm run build --if-present\"
    cwd: \"{working_dir}\"
  - kind: shell
    command: \"npm test --if-present\"
    cwd: \"{working_dir}\"
    must_pass: false
"
        }
        AcceptancePreset::StaticSite => {
            if cwd.join("public/index.html").exists() && !cwd.join("index.html").exists() {
                "\
name: static site acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/public/index.html\"
  - kind: shell
    command: \"test -s public/index.html\"
    cwd: \"{working_dir}\"
"
            } else {
                "\
name: static site acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/index.html\"
  - kind: shell
    command: \"test -s index.html\"
    cwd: \"{working_dir}\"
"
            }
        }
        AcceptancePreset::Basic => {
            "\
name: basic project acceptance
checks:
  - kind: shell
    command: \"test -d .\"
    cwd: \"{working_dir}\"
"
        }
    }
    .to_string();
    let markdown = format!(
        "\
# Done Criteria

These checks define what `deadreckon` should verify before promoting a completed run.

{}
",
        acceptance_markdown_from_yaml(&yaml)
    );
    AcceptanceDraft {
        yaml,
        markdown,
        files: BTreeMap::new(),
    }
}

fn acceptance_pack_draft(pack: AcceptancePack, cwd: &Path) -> AcceptanceDraft {
    let pack = match pack {
        AcceptancePack::Auto => match detect_acceptance_preset(cwd) {
            AcceptancePreset::Rust => AcceptancePack::Rust,
            AcceptancePreset::Node => AcceptancePack::Node,
            AcceptancePreset::StaticSite => AcceptancePack::StaticSite,
            AcceptancePreset::Basic | AcceptancePreset::Auto => AcceptancePack::Basic,
        },
        other => other,
    };
    let yaml = match pack {
        AcceptancePack::Basic => {
            "name: basic acceptance pack\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Build => {
            if cwd.join("Cargo.toml").exists() {
                "name: build acceptance pack\nchecks:\n  - kind: build_success\n    cwd: \"{working_dir}\"\n"
            } else if cwd.join("package.json").exists() {
                "name: build acceptance pack\nchecks:\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n"
            } else if cwd.join("pyproject.toml").exists() || cwd.join("requirements.txt").exists() {
                "name: build acceptance pack\nchecks:\n  - kind: shell\n    command: \"python3 -m compileall .\"\n    cwd: \"{working_dir}\"\n"
            } else {
                "name: build acceptance pack\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n"
            }
        }
        AcceptancePack::Test => {
            if cwd.join("Cargo.toml").exists() {
                "name: test acceptance pack\nchecks:\n  - kind: cargo_test\n"
            } else if cwd.join("package.json").exists() {
                "name: test acceptance pack\nchecks:\n  - kind: shell\n    command: \"npm test --if-present\"\n    cwd: \"{working_dir}\"\n"
            } else {
                "name: test acceptance pack\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n"
            }
        }
        AcceptancePack::Rust => {
            "name: rust acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/Cargo.toml\"\n  - kind: cargo_test\n"
        }
        AcceptancePack::Node => {
            "name: node acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/package.json\"\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n  - kind: shell\n    command: \"npm test --if-present\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::StaticSite => {
            if cwd.join("public/index.html").exists() && !cwd.join("index.html").exists() {
                "name: static site acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/public/index.html\"\n  - kind: shell\n    command: \"test -s public/index.html\"\n    cwd: \"{working_dir}\"\n"
            } else {
                "name: static site acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/index.html\"\n  - kind: shell\n    command: \"test -s index.html\"\n    cwd: \"{working_dir}\"\n"
            }
        }
        AcceptancePack::Browser => {
            "name: browser acceptance pack\nchecks:\n  - kind: shell\n    command: \"node .deadreckon/acceptance/browser-smoke.mjs\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Playwright => {
            "name: playwright acceptance pack\nchecks:\n  - kind: shell\n    command: \"npm run build --if-present && (npm run preview --if-present -- --host 127.0.0.1 > .deadreckon/acceptance/preview.log 2>&1 & pid=$!; trap 'kill $pid 2>/dev/null || true' EXIT; sleep 3; DEADRECKON_BASE_URL=${DEADRECKON_BASE_URL:-http://127.0.0.1:4173} npx --yes playwright test .deadreckon/acceptance/playwright-smoke.spec.mjs --reporter=line)\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Vite => {
            "name: vite acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/package.json\"\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n  - kind: shell\n    command: \"node .deadreckon/acceptance/browser-smoke.mjs dist\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::NextJs => {
            "name: nextjs acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/package.json\"\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Python => {
            "name: python acceptance pack\nchecks:\n  - kind: shell\n    command: \"python3 -m compileall .\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Auto => unreachable!("auto pack resolved above"),
    }
    .to_string();

    let mut files = BTreeMap::new();
    if matches!(
        pack,
        AcceptancePack::Browser | AcceptancePack::Vite | AcceptancePack::Playwright
    ) {
        files.insert(
            PathBuf::from(".deadreckon/acceptance/browser-smoke.mjs"),
            browser_smoke_script().to_string(),
        );
    }
    if matches!(pack, AcceptancePack::Playwright) {
        files.insert(
            PathBuf::from(".deadreckon/acceptance/playwright-smoke.spec.mjs"),
            playwright_smoke_spec().to_string(),
        );
    }
    AcceptanceDraft {
        markdown: format!(
            "# Done Criteria\n\nAdded the `{}` pack.\n\n{}",
            pack.name(),
            acceptance_markdown_from_yaml(&yaml)
        ),
        yaml,
        files,
    }
}

fn append_acceptance_checks(existing_raw: &str, addition_raw: &str) -> Result<String> {
    let mut existing = acceptance_yaml_value(existing_raw)?;
    let addition = acceptance_yaml_value(addition_raw)?;
    let mut checks = yaml_mapping_get(&addition, "checks")
        .map(yaml_items)
        .unwrap_or_default()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "acceptance pack did not contain checks",
            "try `deadreckon def-done add browser` or `deadreckon def-done \"what should count as done\"`",
        )));
    }
    let mapping = existing.as_mapping_mut().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "acceptance.yaml must be a mapping",
            "run `deadreckon def-done \"what should count as done\" --overwrite`",
        ))
    })?;
    let key = serde_yaml::Value::String("checks".to_string());
    let entry = mapping
        .entry(key)
        .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new()));
    match entry {
        serde_yaml::Value::Sequence(existing_checks) => existing_checks.append(&mut checks),
        other => {
            let mut merged = yaml_items(other).into_iter().cloned().collect::<Vec<_>>();
            merged.append(&mut checks);
            *other = serde_yaml::Value::Sequence(merged);
        }
    }
    serde_yaml::to_string(&existing).map_err(|source| {
        CliError::Core(deadreckon_core::user_error(
            &format!("failed to write acceptance.yaml: {source}"),
            "run `deadreckon def-done \"what should count as done\" --overwrite`",
        ))
    })
}

fn browser_smoke_script() -> &'static str {
    r#"#!/usr/bin/env node
const fs = require('fs');
const http = require('http');
const path = require('path');

const requestedRoot = process.argv[2];
const candidates = [requestedRoot, 'dist', 'build', 'public', '.'].filter(Boolean);
const root = candidates.find((candidate) => fs.existsSync(path.join(candidate, 'index.html')));
if (!root) {
  console.error('No index.html found in dist, build, public, or project root.');
  process.exit(1);
}

const mime = new Map([
  ['.html', 'text/html'],
  ['.js', 'text/javascript'],
  ['.css', 'text/css'],
  ['.json', 'application/json'],
  ['.svg', 'image/svg+xml'],
]);

const server = http.createServer((req, res) => {
  const urlPath = decodeURIComponent((req.url || '/').split('?')[0]);
  const safePath = path.normalize(urlPath === '/' ? '/index.html' : urlPath).replace(/^(\.\.[/\\])+/, '');
  const file = path.join(root, safePath);
  if (!file.startsWith(path.resolve(root)) && path.isAbsolute(file)) {
    res.writeHead(403);
    res.end('forbidden');
    return;
  }
  fs.readFile(file, (err, body) => {
    if (err) {
      res.writeHead(404);
      res.end('missing');
      return;
    }
    res.writeHead(200, { 'content-type': mime.get(path.extname(file)) || 'application/octet-stream' });
    res.end(body);
  });
});

server.listen(0, '127.0.0.1', async () => {
  const { port } = server.address();
  try {
    const response = await fetch(`http://127.0.0.1:${port}/`);
    const body = await response.text();
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    if (!/<html[\s>]/i.test(body)) throw new Error('index response did not look like HTML');
    console.log(`loaded ${root}/index.html over HTTP`);
  } catch (error) {
    console.error(error.message || String(error));
    process.exitCode = 1;
  } finally {
    server.close();
  }
});
"#
}

fn playwright_smoke_spec() -> &'static str {
    r#"import { test, expect } from '@playwright/test';

test('app loads without browser console errors', async ({ page }) => {
  const errors = [];
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(process.env.DEADRECKON_BASE_URL || 'http://127.0.0.1:4173/', {
    waitUntil: 'networkidle',
  });
  await expect(page.locator('body')).toBeVisible();
  expect(errors).toEqual([]);
});
"#
}

fn acceptance_markdown_from_yaml(raw: &str) -> String {
    match acceptance_check_count(raw) {
        Ok(count) => format!(
            "Configured checks: {count}. Run `deadreckon def-done check` before starting long work."
        ),
        Err(_) => "Run `deadreckon def-done check` before starting long work.".to_string(),
    }
}

fn print_acceptance_yaml_summary(raw: &str) -> Result<()> {
    let root = acceptance_yaml_value(raw)?;
    println!("{}", ui_heading("checks"));
    for line in acceptance_summary_lines(&root) {
        println!("  {line}");
    }
    Ok(())
}

fn acceptance_summary_lines(root: &serde_yaml::Value) -> Vec<String> {
    let mut lines = Vec::new();
    for key in [
        "checks",
        "required",
        "optional",
        "tests",
        "file-exists",
        "content-match",
        "build-success",
    ] {
        if let Some(value) = yaml_mapping_get(root, key) {
            for item in yaml_items(value) {
                lines.push(describe_acceptance_item(key, item));
            }
        }
    }
    if lines.is_empty() {
        lines.push("no recognized checks".to_string());
    }
    lines
}

fn describe_acceptance_item(group: &str, item: &serde_yaml::Value) -> String {
    if let Some(command) = item.as_str() {
        return format!("{group}: shell {}", one_line(command, 96));
    }
    let Some(mapping) = item.as_mapping() else {
        return format!("{group}: {}", one_line(&format!("{item:?}"), 120));
    };
    let kind = yaml_mapping_get(item, "kind")
        .and_then(serde_yaml::Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            if mapping.len() == 1 {
                mapping
                    .keys()
                    .next()
                    .and_then(serde_yaml::Value::as_str)
                    .map(ToString::to_string)
            } else {
                None
            }
        })
        .unwrap_or_else(|| group.to_string());
    let path = yaml_mapping_get(item, "path")
        .and_then(serde_yaml::Value::as_str)
        .or_else(|| yaml_mapping_get(item, "cwd").and_then(serde_yaml::Value::as_str));
    let command = yaml_mapping_get(item, "command").and_then(serde_yaml::Value::as_str);
    let pattern = yaml_mapping_get(item, "pattern").and_then(serde_yaml::Value::as_str);
    let detail = command
        .or(path)
        .or(pattern)
        .map(|value| one_line(value, 96))
        .unwrap_or_else(|| one_line(&format!("{item:?}"), 96));
    format!("{group}: {kind} {detail}")
}

pub(crate) fn acceptance_check_count(raw: &str) -> Result<usize> {
    let root = acceptance_yaml_value(raw)?;
    let mut count = 0;
    for key in [
        "checks",
        "required",
        "optional",
        "tests",
        "file-exists",
        "content-match",
        "build-success",
    ] {
        if let Some(value) = yaml_mapping_get(&root, key) {
            count += yaml_items(value).len();
        }
    }
    if count == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "acceptance.yaml does not contain any recognized checks",
            "run `deadreckon def-done \"what should count as done\"`",
        )));
    }
    Ok(count)
}

fn validate_acceptance_yaml_integrity(raw: &str) -> Result<()> {
    let checks = deadreckon_core::gate::acceptance_checks_from_yaml(raw)?;
    let findings = deadreckon_core::tamper::lint_checks(&checks);
    if let Some(finding) = findings.first() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done criteria rejected: suppression pattern '{}' in {} check",
                finding.pattern, finding.check_kind
            ),
            "deadreckon def-done \"what should count as done\" and remove the suppression; checks must fail honestly",
        )));
    }
    Ok(())
}

fn acceptance_yaml_value(raw: &str) -> Result<serde_yaml::Value> {
    serde_yaml::from_str(raw).map_err(|source| {
        CliError::Core(deadreckon_core::user_error(
            &format!("invalid acceptance.yaml: {source}"),
            "deadreckon def-done \"what should count as done\"",
        ))
    })
}

fn yaml_mapping_get<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    value
        .as_mapping()?
        .get(serde_yaml::Value::String(key.to_string()))
}

fn yaml_items(value: &serde_yaml::Value) -> Vec<&serde_yaml::Value> {
    match value {
        serde_yaml::Value::Sequence(items) => items.iter().collect(),
        serde_yaml::Value::Null => Vec::new(),
        value => vec![value],
    }
}
