//! Bounded child-output capture and whole-process-tree supervision.
//!
//! These primitives deliberately do not decide where output is persisted or
//! when a run should be cancelled. Callers retain those policy decisions while
//! sharing one byte-bounding and termination implementation.

mod head_tail_buffer;
mod pid_file;
mod termination;
mod truncation;

pub use head_tail_buffer::HeadTailBuffer;
pub use pid_file::{SupervisedProcess, read_supervised_process, write_supervised_process};
#[cfg(unix)]
pub use termination::ProcessGroupTerminator;
pub use termination::{ChildTerminator, RawPidTerminator, TerminationOutcome, spawn_grouped};
pub use truncation::TruncationPolicy;
