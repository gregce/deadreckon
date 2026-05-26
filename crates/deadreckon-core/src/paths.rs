use std::env;
use std::path::{Path, PathBuf};

use crate::error::{DeadreckonError, Result};

pub const DEFAULT_DEADRECKON_HOME: &str = "/Users/gdc/.deadreckon";
pub const SOURCE_ROOT: &str = "/Users/gdc/deadreckon";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadreckonPaths {
    home: PathBuf,
}

impl DeadreckonPaths {
    pub fn discover() -> Self {
        let home = env::var_os("DEADRECKON_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DEADRECKON_HOME));
        Self { home }
    }

    pub fn from_home(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn config_path(&self) -> PathBuf {
        self.home.join("config.toml")
    }

    pub fn runstate_dir(&self) -> PathBuf {
        self.home.join("runstate")
    }

    pub fn scope_root(&self, scope: &str) -> PathBuf {
        self.runstate_dir().join(scope)
    }

    pub fn current_dir(&self, scope: &str) -> PathBuf {
        self.scope_root(scope).join("current")
    }

    pub fn current_pointer_path(&self, scope: &str, task_key: &str) -> PathBuf {
        self.current_dir(scope).join(format!("{task_key}.json"))
    }

    pub fn runs_dir(&self, scope: &str) -> PathBuf {
        self.scope_root(scope).join("runs")
    }

    pub fn run_root(&self, scope: &str, run_id: &str) -> PathBuf {
        self.runs_dir(scope).join(run_id)
    }

    pub fn locks_dir(&self) -> PathBuf {
        self.home.join("locks")
    }

    pub fn chains_dir(&self) -> PathBuf {
        self.home.join("chains")
    }

    pub fn plans_dir(&self) -> PathBuf {
        self.home.join("plans")
    }

    pub fn plan_dir(&self, plan_id: &str) -> PathBuf {
        self.plans_dir().join(plan_id)
    }

    pub fn plan_json(&self, plan_id: &str) -> PathBuf {
        self.plan_dir(plan_id).join("plan.json")
    }

    pub fn coordinator_json(&self, plan_id: &str) -> PathBuf {
        self.plan_dir(plan_id).join("coordinator.json")
    }

    pub fn plan_messages(&self, plan_id: &str) -> PathBuf {
        self.plan_dir(plan_id).join("messages.jsonl")
    }

    pub fn plan_events(&self, plan_id: &str) -> PathBuf {
        self.plan_dir(plan_id).join("plan-events.jsonl")
    }

    pub fn worker_spec(&self, plan_id: &str, task_id: &str) -> PathBuf {
        self.plan_dir(plan_id)
            .join("worker-specs")
            .join(format!("{}.md", sanitize_slug(task_id)))
    }

    pub fn child_summary(&self, plan_id: &str, task_id: &str) -> PathBuf {
        self.plan_dir(plan_id)
            .join("summaries")
            .join(format!("{}.md", sanitize_slug(task_id)))
    }

    pub fn merge_working(&self, plan_id: &str) -> PathBuf {
        self.plan_dir(plan_id).join("merge-working")
    }

    pub fn merge_proofs(&self, plan_id: &str) -> PathBuf {
        self.plan_dir(plan_id).join("merge-proofs")
    }

    pub fn chain_dir(&self, chain_id: &str) -> PathBuf {
        self.chains_dir().join(chain_id)
    }

    pub fn chain_json(&self, chain_id: &str) -> PathBuf {
        self.chain_dir(chain_id).join("chain.json")
    }

    pub fn chain_events(&self, chain_id: &str) -> PathBuf {
        self.chain_dir(chain_id).join("chain-events.jsonl")
    }

    pub fn conductor_json(&self, chain_id: &str) -> PathBuf {
        self.chain_dir(chain_id).join("conductor.json")
    }

    pub fn library_dir(&self, scope: &str, run_id: &str) -> PathBuf {
        self.home.join("library").join(scope).join(run_id)
    }

    pub fn learning_dir(&self) -> PathBuf {
        self.home.join("learning")
    }

    pub fn learning_episodes_dir(&self, scope: &str) -> PathBuf {
        self.learning_dir().join("episodes").join(scope)
    }

    pub fn learning_episode_path(&self, scope: &str, run_id: &str) -> PathBuf {
        self.learning_episodes_dir(scope)
            .join(format!("{run_id}.json"))
    }

    pub fn learning_signals_path(&self) -> PathBuf {
        self.learning_dir().join("signals.jsonl")
    }

    pub fn learning_insights_path(&self) -> PathBuf {
        self.learning_dir().join("insights.jsonl")
    }

    pub fn learning_proposals_dir(&self) -> PathBuf {
        self.learning_dir().join("proposals")
    }

    pub fn learning_proposal_path(&self, proposal_id: &str) -> PathBuf {
        self.learning_proposals_dir()
            .join(format!("{proposal_id}.json"))
    }

    pub fn learning_candidates_dir(&self) -> PathBuf {
        self.learning_dir().join("candidates")
    }

    pub fn learning_candidate_dir(&self, candidate_id: &str) -> PathBuf {
        self.learning_candidates_dir().join(candidate_id)
    }

    pub fn learning_candidate_path(&self, candidate_id: &str) -> PathBuf {
        self.learning_candidate_dir(candidate_id)
            .join("candidate.json")
    }

    pub fn learning_eval_path(&self, candidate_id: &str) -> PathBuf {
        self.learning_dir()
            .join("evals")
            .join(format!("{candidate_id}.json"))
    }

    pub fn learning_pr_events_path(&self) -> PathBuf {
        self.learning_dir().join("pr-events.jsonl")
    }

    pub fn learning_policy_path(&self) -> PathBuf {
        self.learning_dir().join("policy.toml")
    }

    pub fn learning_bundles_dir(&self) -> PathBuf {
        self.learning_dir().join("bundles")
    }

    pub fn learning_bundle_path(&self, bundle_id: &str) -> PathBuf {
        self.learning_bundles_dir()
            .join(format!("{bundle_id}.json"))
    }
}

pub fn workspace_scope(start: &Path) -> Result<String> {
    let root = env::var_os("DEADRECKON_SCOPE_ROOT")
        .map(PathBuf::from)
        .or_else(|| find_git_root(start))
        .unwrap_or_else(|| start.to_path_buf());
    let canonical = root.canonicalize().map_err(|source| DeadreckonError::Io {
        path: root.clone(),
        source,
    })?;
    let base = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_slug)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "workspace".to_string());
    Ok(format!(
        "{base}-{:08x}",
        fnv1a32(canonical.to_string_lossy().as_ref())
    ))
}

pub fn task_key(goal: &str) -> String {
    let trimmed = goal.trim();
    let mut stem = sanitize_slug(trimmed);
    if stem.len() > 48 {
        stem.truncate(48);
        stem = stem.trim_end_matches('-').to_string();
    }
    if stem.is_empty() {
        stem = "task".to_string();
    }
    format!("{stem}-{:08x}", fnv1a32(trimmed))
}

pub fn sanitize_slug(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = false;
    for ch in input.chars() {
        let next = if ch.is_ascii_alphanumeric() {
            last_dash = false;
            Some(ch.to_ascii_lowercase())
        } else if matches!(ch, '-' | '_' | ' ' | '.') {
            if last_dash {
                None
            } else {
                last_dash = true;
                Some('-')
            }
        } else {
            None
        };
        if let Some(ch) = next {
            out.push(ch);
        }
    }
    out.trim_matches('-').to_string()
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut cursor = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if cursor.join(".git").exists() {
            return Some(cursor);
        }
        if !cursor.pop() {
            return None;
        }
    }
}

fn fnv1a32(input: &str) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in input.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{DeadreckonPaths, sanitize_slug, task_key};

    #[test]
    fn task_key_is_slugged_and_stable() {
        let key = task_key("Tiny hello rust!");
        assert_eq!(key, "tiny-hello-rust-be7b910a");
    }

    #[test]
    fn slug_rejects_path_separators() {
        assert_eq!(sanitize_slug("../Hello World"), "hello-world");
    }

    #[test]
    fn learning_paths_stay_under_deadreckon_home() {
        let paths = DeadreckonPaths::from_home("/tmp/deadreckon-home");

        for path in [
            paths.learning_dir(),
            paths.learning_episode_path("scope", "run-id"),
            paths.learning_signals_path(),
            paths.learning_insights_path(),
            paths.learning_proposal_path("prop-1"),
            paths.learning_candidate_path("cand-1"),
            paths.learning_eval_path("cand-1"),
            paths.learning_pr_events_path(),
            paths.learning_policy_path(),
        ] {
            assert!(
                path.starts_with(paths.home()),
                "{} escaped {}",
                path.display(),
                paths.home().display()
            );
        }
    }
}
