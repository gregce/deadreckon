#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

//! Runtime orchestration for provider turns, sandboxed tools, and run docs.

mod error;

pub mod polish;
pub mod turn_loop;

pub use polish::{
    PolishConfig, PolishRecord, PolishedDocs, ResolvedSkill, SkillSource,
    default_polished_json_for_tests, inputs_hash, polish_run_docs, read_polish_record,
    resolve_skill, substitute_placeholders, templated_docs_json,
};
pub use turn_loop::{RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, run_turn_loop};
