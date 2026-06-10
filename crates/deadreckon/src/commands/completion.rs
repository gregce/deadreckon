use super::super::*;

pub(crate) fn completion_command(command: Option<CompletionCommand>) -> Result<()> {
    match command.unwrap_or(CompletionCommand::Install {
        shell: None,
        path: None,
        no_rc: false,
    }) {
        CompletionCommand::Install { shell, path, no_rc } => {
            let outcome = install_completion(shell, path, !no_rc)?;
            println!(
                "{}",
                completion_install_surface(&outcome).render_plain(!completion_hints_enabled(false))
            );
        }
        CompletionCommand::Bash => write_completion_script(Shell::Bash, &mut io::stdout()),
        CompletionCommand::Elvish => write_completion_script(Shell::Elvish, &mut io::stdout()),
        CompletionCommand::Fish => write_completion_script(Shell::Fish, &mut io::stdout()),
        CompletionCommand::PowerShell => {
            write_completion_script(Shell::PowerShell, &mut io::stdout());
        }
        CompletionCommand::Zsh => write_completion_script(Shell::Zsh, &mut io::stdout()),
    }
    Ok(())
}

fn write_completion_script(shell: Shell, output: &mut dyn Write) {
    let mut command = completion_command_tree();
    let bin_name = command.get_name().to_string();
    generate(shell, &mut command, bin_name, output);
}

fn completion_command_tree() -> ClapCommand {
    unhide_completion_commands(Cli::command())
}

fn unhide_completion_commands(command: ClapCommand) -> ClapCommand {
    command
        .hide(false)
        .mut_subcommands(unhide_completion_commands)
}

fn install_completion(
    shell: Option<Shell>,
    path_override: Option<PathBuf>,
    update_rc: bool,
) -> Result<CompletionInstallOutcome> {
    let shell = shell.or_else(detect_completion_shell).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "could not detect your shell",
            "deadreckon completion install --shell zsh",
        ))
    })?;
    let path = path_override.unwrap_or_else(|| default_completion_path(shell));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut script = Vec::new();
    write_completion_script(shell, &mut script);
    fs::write(&path, script)?;
    let rc_status = match (shell, update_rc) {
        (Shell::Zsh, true) => ensure_zsh_completion_rc(&path)?,
        (Shell::Zsh, false) => CompletionRcStatus::Skipped,
        _ => CompletionRcStatus::NotApplicable,
    };
    Ok(CompletionInstallOutcome {
        shell,
        path,
        rc_status,
    })
}

pub(crate) fn try_install_completion_after_init() -> String {
    match install_completion(None, None, true) {
        Ok(outcome) => format!(
            "installed {} completion at {}",
            completion_shell_name(outcome.shell),
            outcome.path.display()
        ),
        Err(err) => format!("not installed: {err}"),
    }
}

#[derive(Clone, Debug)]
struct CompletionInstallOutcome {
    shell: Shell,
    path: PathBuf,
    rc_status: CompletionRcStatus,
}

#[derive(Clone, Debug)]
enum CompletionRcStatus {
    Updated(PathBuf),
    Ready(PathBuf),
    Skipped,
    NotApplicable,
}

fn completion_install_surface(outcome: &CompletionInstallOutcome) -> VerdictSurface {
    let shell = completion_shell_name(outcome.shell);
    let primary = "deadreckon doctor";
    let rc_evidence = outcome.rc_status.evidence_value();
    VerdictSurface::must_new(
        VerdictKind::Completed,
        "completion",
        Some(shell),
        ExplanationPanel::new(
            format!(
                "DeadReckon wrote the {shell} completion script to {}.",
                outcome.path.display()
            ),
            "The install command finished and the shell integration state was recorded; doctor is the safest next command to verify setup.",
            vec![
                ("shell", shell.to_string()),
                ("script", outcome.path.display().to_string()),
                ("shell rc", rc_evidence),
            ],
        ),
        vec![("Recommended", primary)],
        Vec::<(&str, &str)>::new(),
    )
}

fn completion_shell_name(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => "bash",
        Shell::Elvish => "elvish",
        Shell::Fish => "fish",
        Shell::PowerShell => "powershell",
        Shell::Zsh => "zsh",
        _ => "shell",
    }
}

impl CompletionRcStatus {
    fn evidence_value(&self) -> String {
        match self {
            Self::Updated(path) => format!("updated {}", path.display()),
            Self::Ready(path) => format!("already configured {}", path.display()),
            Self::Skipped => "not updated (--no-rc)".to_string(),
            Self::NotApplicable => "not required for this shell".to_string(),
        }
    }
}

fn detect_completion_shell() -> Option<Shell> {
    let shell = std::env::var("SHELL").ok()?;
    let name = Path::new(&shell).file_name()?.to_string_lossy();
    match name.as_ref() {
        "bash" => Some(Shell::Bash),
        "elvish" => Some(Shell::Elvish),
        "fish" => Some(Shell::Fish),
        "pwsh" | "powershell" => Some(Shell::PowerShell),
        "zsh" => Some(Shell::Zsh),
        _ => None,
    }
}

fn default_completion_path(shell: Shell) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    match shell {
        Shell::Bash => home
            .join(".local")
            .join("share")
            .join("bash-completion")
            .join("completions")
            .join("deadreckon"),
        Shell::Elvish => home
            .join(".config")
            .join("elvish")
            .join("lib")
            .join("deadreckon-completers.elv"),
        Shell::Fish => home
            .join(".config")
            .join("fish")
            .join("completions")
            .join("deadreckon.fish"),
        Shell::PowerShell => home
            .join(".config")
            .join("powershell")
            .join("deadreckon-completions.ps1"),
        Shell::Zsh => home.join(".zsh").join("completions").join("_deadreckon"),
        _ => home.join(".deadreckon").join("completion"),
    }
}

fn ensure_zsh_completion_rc(completion_path: &Path) -> Result<CompletionRcStatus> {
    let Some(completion_dir) = completion_path.parent() else {
        return Ok(CompletionRcStatus::Skipped);
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let zshrc = home.join(".zshrc");
    let existing = fs::read_to_string(&zshrc).unwrap_or_default();
    let completion_dir = completion_dir.display();
    let block = format!(
        "\n# >>> deadreckon completion >>>\nfpath=({completion_dir} $fpath)\nautoload -Uz compinit\ncompinit\n# <<< deadreckon completion <<<\n"
    );
    if existing.contains("# >>> deadreckon completion >>>")
        || existing.contains(&format!("fpath=({completion_dir} $fpath)"))
    {
        return Ok(CompletionRcStatus::Ready(zshrc));
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&zshrc)?;
    file.write_all(block.as_bytes())?;
    Ok(CompletionRcStatus::Updated(zshrc))
}
