#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

//! Runtime orchestration for provider turns, sandboxed tools, and run docs.

mod error;

pub mod compaction;
pub mod flight;
pub mod polish;
pub mod seam;
pub mod turn_loop;

pub use compaction::{
    COMPACTION_JSONL, CompactionConfig, CompactionRecord, append_compaction_record,
    compact_history, estimate_tokens, parse_compaction_config, read_compaction_config,
};
pub use polish::{
    PolishConfig, PolishRecord, PolishedDocs, ResolvedSkill, SkillSource,
    default_polished_json_for_tests, inputs_hash, polish_run_docs, read_polish_record,
    resolve_skill, substitute_placeholders, templated_docs_json,
};
pub use seam::{
    FailPolicy, SeamCommandConfig, SeamKind, SeamOutcome, SeamRunCtx, SeamsConfig, dispatch_seam,
    parse_seams_config, read_seams_config, resolve_catalog_override, write_seams_audit,
};
pub use turn_loop::{RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, run_turn_loop};
