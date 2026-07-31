use std::env;
use std::path::{Path, PathBuf};

use crate::error::{DeadreckonError, Result};

/// The durable-state home: `$DEADRECKON_HOME`, else `~/.deadreckon`.
pub fn default_deadreckon_home() -> PathBuf {
    std::env::home_dir()
        .map(|home| home.join(".deadreckon"))
        .unwrap_or_else(|| PathBuf::from(".deadreckon"))
}

/// The deadreckon source checkout, when one is reachable. `$DEADRECKON_SOURCE_ROOT`
/// wins; otherwise the workspace this binary was compiled from is probed. Installed
/// release binaries return a path that does not exist on the user's machine, so
/// callers must treat this as an optional last-tier fallback, never a requirement.
pub fn source_root() -> Option<PathBuf> {
    if let Some(root) = env::var_os("DEADRECKON_SOURCE_ROOT") {
        return Some(PathBuf::from(root));
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadreckonPaths {
    home: PathBuf,
}

impl DeadreckonPaths {
    pub fn discover() -> Self {
        // An empty env value (a shell mishap like DEADRECKON_HOME="$unset")
        // must not yield a relative home that scatters state into the cwd.
        let home = env::var_os("DEADRECKON_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(default_deadreckon_home);
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

    pub fn jobs_dir(&self) -> PathBuf {
        self.home.join("jobs")
    }

    pub fn job_dir(&self, job_id: &str) -> PathBuf {
        self.jobs_dir().join(job_id)
    }

    pub fn job_json(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("job.json")
    }

    pub fn job_events(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("job-events.jsonl")
    }

    pub fn job_projection(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("projection.json")
    }

    pub fn job_lease(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("lease.json")
    }

    pub fn job_launch_plan(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("launch-plan.json")
    }

    pub fn job_authority(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("authority.json")
    }

    pub fn job_receipt(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("receipt.json")
    }

    pub fn job_sandbox_boundary_observation(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id)
            .join("sandbox-boundary-observation.json")
    }

    /// Trusted operator captures live under DeadReckon home, never in an
    /// agent-visible run workspace.
    pub fn operator_captures_dir(&self) -> PathBuf {
        self.home.join("operator-captures")
    }

    pub fn operator_capture_dir(&self, job_id: &str, session_id: &str) -> PathBuf {
        self.operator_captures_dir()
            .join(operator_capture_component(job_id))
            .join(operator_capture_component(session_id))
    }

    pub fn operator_capture_binding(&self, job_id: &str, session_id: &str) -> PathBuf {
        self.operator_capture_dir(job_id, session_id)
            .join("binding.json")
    }

    pub fn operator_capture_events(&self, job_id: &str, session_id: &str) -> PathBuf {
        self.operator_capture_dir(job_id, session_id)
            .join("capture-events.jsonl")
    }

    pub fn operator_capture_receipt(&self, job_id: &str, session_id: &str) -> PathBuf {
        self.operator_capture_dir(job_id, session_id)
            .join("capture-receipt.json")
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

fn operator_capture_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len().max(1));
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_') {
            output.push(char::from(*byte));
        } else {
            output.push('%');
            output.push_str(&format!("{byte:02x}"));
        }
    }
    if output.is_empty() {
        "%00".to_string()
    } else {
        output
    }
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

    #[test]
    fn job_paths_stay_under_one_durable_identity() {
        let paths = DeadreckonPaths::from_home("/tmp/deadreckon-home");
        let root = paths.job_dir("job-1");

        for path in [
            paths.job_json("job-1"),
            paths.job_events("job-1"),
            paths.job_projection("job-1"),
            paths.job_lease("job-1"),
            paths.job_launch_plan("job-1"),
            paths.job_authority("job-1"),
            paths.job_receipt("job-1"),
        ] {
            assert!(
                path.starts_with(&root),
                "{} escaped job root",
                path.display()
            );
        }
    }
}
