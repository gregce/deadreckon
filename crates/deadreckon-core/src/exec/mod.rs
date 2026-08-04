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
pub use pid_file::{
    SUPERVISED_PROCESS_RECORD_SCHEMA_VERSION, SupervisedProcess, SupervisedProcessIdentity,
    SupervisedProcessPhase, SupervisedProcessRecord, boot_identities_match, boot_identity,
    normalize_boot_identity, process_start_identity, read_supervised_process,
    read_supervised_process_record, remove_supervised_process_record_if_matches,
    remove_supervised_process_record_if_same, write_supervised_process,
    write_supervised_process_record,
};
#[cfg(unix)]
pub use termination::ProcessGroupTerminator;
pub use termination::{ChildTerminator, RawPidTerminator, TerminationOutcome, spawn_grouped};
pub use truncation::TruncationPolicy;
