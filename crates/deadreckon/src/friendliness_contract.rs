#![allow(dead_code)]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FriendlinessClause {
    AutoDetectDontAsk,
    PreviewBeforeMutate,
    RefuseWithTry,
    OneCommandRollback,
    OneVerdictOnePrimaryAction,
    LifecycleHint,
}

impl FriendlinessClause {
    pub const fn label(self) -> &'static str {
        match self {
            Self::AutoDetectDontAsk => "Auto-detect, don't ask",
            Self::PreviewBeforeMutate => "Preview before mutate",
            Self::RefuseWithTry => "Refuse with try:",
            Self::OneCommandRollback => "One-command rollback",
            Self::OneVerdictOnePrimaryAction => "One verdict + ONE primary action",
            Self::LifecycleHint => "Lifecycle hint",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FriendlinessMark {
    Pass,
    Fail,
    NotApplicable,
}

impl FriendlinessMark {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NotApplicable => "n-a",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerbFriendliness {
    pub verb: &'static str,
    pub marks: [FriendlinessMark; FRIENDLINESS_CLAUSES.len()],
}

pub const FRIENDLINESS_CLAUSES: [FriendlinessClause; 6] = [
    FriendlinessClause::AutoDetectDontAsk,
    FriendlinessClause::PreviewBeforeMutate,
    FriendlinessClause::RefuseWithTry,
    FriendlinessClause::OneCommandRollback,
    FriendlinessClause::OneVerdictOnePrimaryAction,
    FriendlinessClause::LifecycleHint,
];

use FriendlinessMark::{Fail as F, NotApplicable as N, Pass as P};

pub const FRIENDLINESS_CONTRACT: &[VerbFriendliness] = &[
    VerbFriendliness {
        verb: "init",
        marks: [P, P, P, N, P, P],
    },
    VerbFriendliness {
        verb: "config",
        marks: [P, P, F, P, P, P],
    },
    VerbFriendliness {
        verb: "help-all",
        marks: [N, N, N, N, P, P],
    },
    VerbFriendliness {
        verb: "completion",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "acceptance",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "def-done",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "try",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "start",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "run",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "seams",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "orchestrate",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "campaign",
        marks: [F, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "plan",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "fork",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "merge",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "chain",
        marks: [F, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "doctor",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "detect",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "providers",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "models",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "update",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "list",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "library",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "finish",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "materialize",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "apply",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "abandon",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "cleanup",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "extend",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "doc",
        marks: [P, P, P, N, P, P],
    },
    VerbFriendliness {
        verb: "attach",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "kill",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "reshape",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "resume",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "undo",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "rewind",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "show",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "history",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "status",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "import",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "verdict",
        marks: [P, N, P, N, P, P],
    },
    VerbFriendliness {
        verb: "learn",
        marks: [P, P, P, P, P, P],
    },
    VerbFriendliness {
        verb: "improve",
        marks: [P, P, P, P, P, P],
    },
];
