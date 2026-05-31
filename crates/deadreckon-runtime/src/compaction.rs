use std::path::Path;
use std::path::PathBuf;

use deadreckon_core::error::{DeadreckonError, Result};
use deadreckon_core::state::append_json_line;
use serde::Deserialize;
use serde::Serialize;

pub const COMPACTION_JSONL: &str = "compaction.jsonl";
const DEFAULT_FRACTION: f64 = 0.75;
const DEFAULT_KEEP_RECENT_TURNS: usize = 6;
const DEFAULT_FALLBACK_CONTEXT_WINDOW: u32 = 200_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactionConfig {
    pub fraction: f64,
    pub keep_recent_turns: usize,
    pub fallback_context_window: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            fraction: DEFAULT_FRACTION,
            keep_recent_turns: DEFAULT_KEEP_RECENT_TURNS,
            fallback_context_window: DEFAULT_FALLBACK_CONTEXT_WINDOW,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionRecord {
    pub schema_version: u32,
    pub turn: u32,
    pub context_window: u32,
    pub fraction: String,
    pub est_tokens_before: usize,
    pub est_tokens_after: usize,
    pub kept_recent_turns: usize,
    pub elided_turns: usize,
    pub context_window_source: String,
}

pub fn read_compaction_config(config_path: &Path) -> Result<CompactionConfig> {
    let raw = match std::fs::read_to_string(config_path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CompactionConfig::default());
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: config_path.to_path_buf(),
                source,
            });
        }
    };
    parse_compaction_config(&raw)
}

pub fn parse_compaction_config(raw: &str) -> Result<CompactionConfig> {
    let value = toml::from_str::<toml::Value>(raw).map_err(|err| {
        DeadreckonError::InvalidInput(format!("config error: invalid TOML: {err}"))
    })?;
    let Some(compaction) = value.get("compaction") else {
        return Ok(CompactionConfig::default());
    };
    let parsed: PartialCompactionConfig = compaction.clone().try_into().map_err(|err| {
        DeadreckonError::InvalidInput(format!("config error: [compaction]: {err}"))
    })?;
    let cfg = CompactionConfig {
        fraction: parsed.fraction.unwrap_or(DEFAULT_FRACTION),
        keep_recent_turns: parsed
            .keep_recent_turns
            .unwrap_or(DEFAULT_KEEP_RECENT_TURNS),
        fallback_context_window: parsed
            .fallback_context_window
            .unwrap_or(DEFAULT_FALLBACK_CONTEXT_WINDOW),
    };
    validate_compaction_config(cfg)?;
    Ok(cfg)
}

pub fn estimate_tokens(value: &str) -> usize {
    value.chars().count() / 4
}

pub fn compact_history(
    history: &[String],
    context_window: u32,
    cfg: CompactionConfig,
    turn: u32,
    context_window_source: &str,
) -> (Vec<String>, Option<CompactionRecord>) {
    let joined = join_history(history);
    let est_tokens_before = estimate_tokens(&joined);
    let threshold = (context_window as f64 * cfg.fraction) as usize;
    if est_tokens_before <= threshold {
        return (history.to_vec(), None);
    }
    let keep_recent = cfg.keep_recent_turns.min(history.len());
    let elided = history.len().saturating_sub(keep_recent);
    if elided == 0 {
        return (history.to_vec(), None);
    }
    let elided_tokens = estimate_tokens(&history[..elided].join("\n"));
    let mut compacted = Vec::with_capacity(keep_recent + 1);
    compacted.push(format!(
        "[seam:compaction] elided {elided} earlier turns (~{elided_tokens} tokens) to fit context window {context_window}; full history in history.json"
    ));
    compacted.extend_from_slice(&history[history.len() - keep_recent..]);
    let est_tokens_after = estimate_tokens(&join_history(&compacted));
    let record = CompactionRecord {
        schema_version: 1,
        turn,
        context_window,
        fraction: fraction_label(cfg.fraction),
        est_tokens_before,
        est_tokens_after,
        kept_recent_turns: keep_recent,
        elided_turns: elided,
        context_window_source: context_window_source.to_string(),
    };
    (compacted, Some(record))
}

pub fn append_compaction_record(run_root: &Path, record: &CompactionRecord) -> Result<PathBuf> {
    let path = run_root.join(COMPACTION_JSONL);
    append_json_line(&path, record)?;
    Ok(path)
}

fn join_history(history: &[String]) -> String {
    if history.is_empty() {
        "none".to_string()
    } else {
        history.join("\n")
    }
}

fn validate_compaction_config(cfg: CompactionConfig) -> Result<()> {
    if !(cfg.fraction > 0.0 && cfg.fraction <= 1.0) {
        return Err(DeadreckonError::InvalidInput(
            "config error: [compaction].fraction must be > 0 and <= 1".to_string(),
        ));
    }
    if cfg.keep_recent_turns == 0 {
        return Err(DeadreckonError::InvalidInput(
            "config error: [compaction].keep_recent_turns must be greater than 0".to_string(),
        ));
    }
    if cfg.fallback_context_window == 0 {
        return Err(DeadreckonError::InvalidInput(
            "config error: [compaction].fallback_context_window must be greater than 0".to_string(),
        ));
    }
    Ok(())
}

fn fraction_label(value: f64) -> String {
    let mut label = format!("{value:.6}");
    while label.contains('.') && label.ends_with('0') {
        label.pop();
    }
    if label.ends_with('.') {
        label.pop();
    }
    label
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialCompactionConfig {
    fraction: Option<f64>,
    keep_recent_turns: Option<usize>,
    fallback_context_window: Option<u32>,
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn history_over_window_is_compacted_deterministically() {
        let history = long_history(10, 160);
        let cfg = CompactionConfig {
            fraction: 0.5,
            keep_recent_turns: 2,
            fallback_context_window: 80,
        };

        let (first, first_record) = compact_history(&history, 80, cfg, 12, "catalog");
        let (second, second_record) = compact_history(&history, 80, cfg, 12, "catalog");

        assert_eq!(first, second);
        assert_eq!(first_record, second_record);
        assert!(first[0].starts_with("[seam:compaction] elided 8 earlier turns"));
        assert_eq!(first.len(), 3);
        assert_eq!(
            first_record.expect("record").context_window_source,
            "catalog"
        );
    }

    #[test]
    fn goal_and_acceptance_spec_always_retained() {
        let history = long_history(20, 200);
        let cfg = CompactionConfig {
            fraction: 0.5,
            keep_recent_turns: 1,
            fallback_context_window: 100,
        };
        let spec_prefix = "Goal:\nship composable seams\n\nAcceptance criteria:\nkeep the gate";

        let (compacted, record) = compact_history(&history, 100, cfg, 4, "catalog");
        let prompt = format!("{spec_prefix}\n\nHistory:\n{}", compacted.join("\n"));

        assert!(record.is_some());
        assert!(prompt.contains("ship composable seams"));
        assert!(prompt.contains("keep the gate"));
    }

    #[test]
    fn identical_inputs_produce_identical_compaction() {
        let history = long_history(8, 120);
        let cfg = CompactionConfig {
            fraction: 0.25,
            keep_recent_turns: 3,
            fallback_context_window: 120,
        };

        assert_eq!(
            compact_history(&history, 120, cfg, 9, "seam"),
            compact_history(&history, 120, cfg, 9, "seam")
        );
    }

    #[test]
    fn unknown_context_window_uses_recorded_fallback() {
        let history = long_history(6, 160);
        let cfg = CompactionConfig {
            fraction: 0.5,
            keep_recent_turns: 2,
            fallback_context_window: 64,
        };

        let (_, record) =
            compact_history(&history, cfg.fallback_context_window, cfg, 7, "fallback");

        let record = record.expect("record");
        assert_eq!(record.context_window, 64);
        assert_eq!(record.context_window_source, "fallback");
    }

    #[test]
    fn history_under_threshold_is_unchanged() {
        let history = vec!["small".to_string()];
        let cfg = CompactionConfig::default();

        let (compacted, record) = compact_history(&history, 200_000, cfg, 1, "catalog");

        assert_eq!(compacted, history);
        assert!(record.is_none());
    }

    #[test]
    fn compaction_record_appends_jsonl() {
        let temp = TempDir::new().expect("tempdir");
        let record = CompactionRecord {
            schema_version: 1,
            turn: 3,
            context_window: 100,
            fraction: "0.75".to_string(),
            est_tokens_before: 90,
            est_tokens_after: 30,
            kept_recent_turns: 2,
            elided_turns: 4,
            context_window_source: "catalog".to_string(),
        };

        append_compaction_record(temp.path(), &record).expect("append");

        let raw = std::fs::read_to_string(temp.path().join(COMPACTION_JSONL)).expect("jsonl");
        assert!(raw.contains(r#""context_window_source":"catalog""#));
    }

    fn long_history(entries: usize, chars_per_entry: usize) -> Vec<String> {
        (0..entries)
            .map(|index| format!("turn-{index}: {}", "x".repeat(chars_per_entry)))
            .collect()
    }
}
