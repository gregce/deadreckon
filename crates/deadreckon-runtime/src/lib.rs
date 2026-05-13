#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

//! Runtime orchestration for provider turns, sandboxed tools, and run docs.

pub mod polish;
pub mod turn_loop;

mod error {
    use std::path::PathBuf;

    use deadreckon_core::error::{DeadreckonError, Result};

    pub(crate) trait IoContext<T> {
        fn with_path(self, path: impl Into<PathBuf>) -> Result<T>;
    }

    impl<T> IoContext<T> for std::io::Result<T> {
        fn with_path(self, path: impl Into<PathBuf>) -> Result<T> {
            let path = path.into();
            self.map_err(|source| DeadreckonError::Io { path, source })
        }
    }
}

pub use polish::{
    PolishConfig, PolishRecord, PolishedDocs, ResolvedSkill, SkillSource,
    default_polished_json_for_tests, inputs_hash, polish_run_docs, read_polish_record,
    resolve_skill, substitute_placeholders, templated_docs_json,
};
pub use turn_loop::{RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, run_turn_loop};
