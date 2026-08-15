//! Shared classification for paths inside an agent-visible workspace.

use std::path::{Component, Path, PathBuf};

/// Whether a workspace path belongs in the operator's result or only in
/// DeadReckon's private evidence/control state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePathClass {
    Deliverable,
    EvidenceOnly,
    LifecycleMetadata,
    RuntimeOnly,
}

const EVIDENCE_ONLY_ROOTS: [&str; 1] = [".specstory"];
const DISPOSABLE_RUNTIME_ROOTS: [&str; 4] = ["target", "node_modules", ".build", ".next"];
const DELIVERY_GIT_EXCLUDE_PATHSPECS: [&str; 12] = [
    ":(glob,exclude)**/.specstory",
    ":(glob,exclude)**/.specstory/**",
    ":(glob,exclude)**/.deadreckon",
    ":(glob,exclude)**/.deadreckon/**",
    ":(glob,exclude)**/target",
    ":(glob,exclude)**/target/**",
    ":(glob,exclude)**/node_modules",
    ":(glob,exclude)**/node_modules/**",
    ":(glob,exclude)**/.build",
    ":(glob,exclude)**/.build/**",
    ":(glob,exclude)**/.next",
    ":(glob,exclude)**/.next/**",
];

pub const fn evidence_only_roots() -> &'static [&'static str] {
    &EVIDENCE_ONLY_ROOTS
}

/// Negative Git pathspecs that make a broad delivery stage obey the same
/// boundary as [`classify_workspace_path`].
///
/// Callers must include a positive pathspec such as `.` before these entries.
pub const fn delivery_git_exclude_pathspecs() -> &'static [&'static str] {
    &DELIVERY_GIT_EXCLUDE_PATHSPECS
}

pub fn classify_workspace_path(path: &Path) -> WorkspacePathClass {
    let mut evidence_only = false;
    let mut lifecycle_metadata = false;
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        if name == ".git" || is_disposable_runtime_root(name) {
            return WorkspacePathClass::RuntimeOnly;
        }
        if name == ".deadreckon" {
            lifecycle_metadata = true;
        }
        if EVIDENCE_ONLY_ROOTS
            .iter()
            .any(|root| name == std::ffi::OsStr::new(root))
        {
            evidence_only = true;
        }
    }
    if evidence_only {
        WorkspacePathClass::EvidenceOnly
    } else if lifecycle_metadata {
        WorkspacePathClass::LifecycleMetadata
    } else {
        WorkspacePathClass::Deliverable
    }
}

/// Return the nearest disposable build/dependency root containing `path`.
///
/// `.git` is runtime-only for delivery purposes, but it is deliberately not a
/// disposable output root. Lifecycle cleanup must never remove Git control
/// state while trying to clear build residue.
pub fn runtime_output_root(path: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        root.push(name);
        if is_disposable_runtime_root(name) {
            return Some(root);
        }
    }
    None
}

pub fn is_deliverable_workspace_path(path: &Path) -> bool {
    classify_workspace_path(path) == WorkspacePathClass::Deliverable
}

/// Whether a path belongs in a recoverable workspace copy.
///
/// Git control is retained because restoring a linked worktree requires it;
/// disposable build/dependency roots are omitted because they can be rebuilt.
pub fn is_recoverable_workspace_path(path: &Path) -> bool {
    runtime_output_root(path).is_none()
}

/// Whether provider-flight checkpoints should track a workspace path.
///
/// Flight evidence intentionally retains provider-private evidence while
/// excluding lifecycle control and disposable runtime output.
pub fn is_checkpointable_workspace_path(path: &Path) -> bool {
    if path
        .components()
        .any(|component| component.as_os_str() == ".deadreckon")
    {
        return false;
    }
    matches!(
        classify_workspace_path(path),
        WorkspacePathClass::Deliverable | WorkspacePathClass::EvidenceOnly
    )
}

pub fn is_promotable_workspace_path(path: &Path) -> bool {
    match classify_workspace_path(path) {
        WorkspacePathClass::Deliverable => true,
        WorkspacePathClass::LifecycleMetadata => path
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == ".deadreckon"),
        WorkspacePathClass::EvidenceOnly | WorkspacePathClass::RuntimeOnly => false,
    }
}

fn is_disposable_runtime_root(name: &std::ffi::OsStr) -> bool {
    DISPOSABLE_RUNTIME_ROOTS
        .iter()
        .any(|root| name == std::ffi::OsStr::new(root))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::{
        WorkspacePathClass, classify_workspace_path, delivery_git_exclude_pathspecs,
        is_checkpointable_workspace_path, is_deliverable_workspace_path,
        is_promotable_workspace_path, is_recoverable_workspace_path, runtime_output_root,
    };

    #[test]
    fn specstory_is_evidence_only_while_run_docs_and_notes_are_deliverable() {
        assert_eq!(
            classify_workspace_path(Path::new(".specstory/history/session.md")),
            WorkspacePathClass::EvidenceOnly
        );
        assert_eq!(
            classify_workspace_path(Path::new("docs/RUN-AS-BUILT.md")),
            WorkspacePathClass::Deliverable
        );
        assert_eq!(
            classify_workspace_path(Path::new("implementation-notes.html")),
            WorkspacePathClass::Deliverable
        );
        assert_eq!(
            classify_workspace_path(Path::new(".deadreckon/codebase.json")),
            WorkspacePathClass::LifecycleMetadata
        );
        assert!(!is_deliverable_workspace_path(Path::new(
            ".deadreckon/docs/RUN-NARRATIVE.md"
        )));
        assert!(is_promotable_workspace_path(Path::new(
            ".deadreckon/docs/RUN-NARRATIVE.md"
        )));
        assert_eq!(
            classify_workspace_path(Path::new("target/debug/deadreckon")),
            WorkspacePathClass::RuntimeOnly
        );
        assert_eq!(
            classify_workspace_path(Path::new(".build/debug/App")),
            WorkspacePathClass::RuntimeOnly
        );
        assert_eq!(
            classify_workspace_path(Path::new(".next/server/app.js")),
            WorkspacePathClass::RuntimeOnly
        );
        assert_eq!(
            classify_workspace_path(Path::new(".deadreckon/.specstory/session.md")),
            WorkspacePathClass::EvidenceOnly
        );
        assert!(!is_promotable_workspace_path(Path::new(
            "nested/.deadreckon/codebase.json"
        )));
        assert!(
            delivery_git_exclude_pathspecs()
                .iter()
                .any(|pathspec| pathspec.contains(".specstory"))
        );
        assert!(is_recoverable_workspace_path(Path::new(".git/index")));
        assert!(!is_recoverable_workspace_path(Path::new(
            ".build/debug/App"
        )));
        assert!(!is_recoverable_workspace_path(Path::new(
            ".next/server/app.js"
        )));
        assert!(is_checkpointable_workspace_path(Path::new(
            ".specstory/history/session.md"
        )));
        assert!(!is_checkpointable_workspace_path(Path::new(
            ".deadreckon/codebase.json"
        )));
        assert!(!is_checkpointable_workspace_path(Path::new(
            ".deadreckon/.specstory/session.md"
        )));
    }

    #[test]
    fn delivery_git_pathspecs_stage_only_deliverables() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        let git = |args: &[&str]| {
            let output = crate::git::run_git(root, args).expect("run git");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        git(&["init", "-q"]);
        for directory in [
            "src",
            ".specstory/history",
            ".deadreckon",
            "target/debug",
            "web/node_modules/pkg",
            ".build/debug",
            ".next/server",
        ] {
            fs::create_dir_all(root.join(directory)).expect("directory");
        }
        for file in [
            "src/lib.rs",
            ".specstory/history/session.md",
            ".deadreckon/codebase.json",
            "target/debug/output",
            "web/node_modules/pkg/index.js",
            ".build/debug/app",
            ".next/server/app.js",
        ] {
            fs::write(root.join(file), "fixture\n").expect("file");
        }

        let mut args = vec!["add", "-A", "--", "."];
        args.extend_from_slice(delivery_git_exclude_pathspecs());
        git(&args);
        let staged = git(&["diff", "--cached", "--name-only"]);

        assert_eq!(staged, "src/lib.rs\n");
    }

    #[test]
    fn runtime_output_root_is_nearest_disposable_root_and_never_git_control() {
        assert_eq!(
            runtime_output_root(Path::new("target/debug/deadreckon")),
            Some(Path::new("target").to_path_buf())
        );
        assert_eq!(
            runtime_output_root(Path::new("web/node_modules/pkg/index.js")),
            Some(Path::new("web/node_modules").to_path_buf())
        );
        assert_eq!(
            runtime_output_root(Path::new("target/nested/node_modules/pkg")),
            Some(Path::new("target").to_path_buf())
        );
        assert_eq!(
            runtime_output_root(Path::new("apps/game/.build/debug/App")),
            Some(Path::new("apps/game/.build").to_path_buf())
        );
        assert_eq!(
            runtime_output_root(Path::new("apps/web/.next/server/app.js")),
            Some(Path::new("apps/web/.next").to_path_buf())
        );
        assert_eq!(runtime_output_root(Path::new(".git/index")), None);
        assert_eq!(runtime_output_root(Path::new("src/lib.rs")), None);
    }

    #[test]
    fn unknown_framework_output_has_no_builtin_name() {
        assert_eq!(runtime_output_root(Path::new("invented-cache/lock")), None);
    }
}
