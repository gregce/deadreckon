use std::path::PathBuf;

use crate::proof_block::ProofBlock;
use crate::ui_card::{Card, HintLine, MetricColumn, Section, TitleGlyph, TitleLine, Tone};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    Completed,
    Paused,
    Killed,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchDiffSummary {
    pub lines_added: u64,
    pub lines_deleted: u64,
    pub files_added: u64,
    pub files_updated: u64,
    pub files_deleted: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExitSummaryInput {
    pub run_id: String,
    pub goal: String,
    pub provider: String,
    pub branch: Option<String>,
    pub outcome: OutcomeKind,
    pub turns: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub spend_usd: f64,
    pub approximate_spend: bool,
    pub spend_label: String,
    pub wall_seconds: f64,
    pub diff: Option<BranchDiffSummary>,
    pub gate: String,
    pub gate_tone: Tone,
    pub tests_modified: Option<bool>,
    pub gate_caveats: Vec<String>,
    pub working_dir: PathBuf,
    pub proof_path: PathBuf,
    pub proof_block: Option<ProofBlock>,
    pub hints: Vec<(String, String)>,
}

pub fn build_exit_summary_card(input: &ExitSummaryInput) -> Card {
    let (glyph, label, tone) = match input.outcome {
        OutcomeKind::Completed => (TitleGlyph::Success, "completed run", Tone::Good),
        OutcomeKind::Paused => (TitleGlyph::Paused, "paused run", Tone::Warn),
        OutcomeKind::Killed => (TitleGlyph::Stopped, "killed run", Tone::Bad),
        OutcomeKind::Failed => (TitleGlyph::Failed, "failed run", Tone::Bad),
    };
    let mut sections = vec![
        Section::Metric {
            label: "turns".to_string(),
            columns: vec![MetricColumn {
                value: input.turns.to_string(),
                tone,
            }],
        },
        Section::Metric {
            label: "tokens".to_string(),
            columns: vec![
                MetricColumn {
                    value: format!("in {}", input.input_tokens),
                    tone: Tone::Neutral,
                },
                MetricColumn {
                    value: format!("out {}", input.output_tokens),
                    tone: Tone::Neutral,
                },
            ],
        },
        Section::Metric {
            label: "spend".to_string(),
            columns: vec![MetricColumn {
                value: input.spend_label.clone(),
                tone: Tone::Neutral,
            }],
        },
    ];
    if let Some(diff) = input.diff.as_ref() {
        sections.push(Section::Metric {
            label: "branch diff".to_string(),
            columns: vec![
                MetricColumn {
                    value: format!("+{}", diff.lines_added),
                    tone: Tone::Good,
                },
                MetricColumn {
                    value: format!("-{}", diff.lines_deleted),
                    tone: Tone::Bad,
                },
                MetricColumn {
                    value: format!(
                        "{} files",
                        diff.files_added + diff.files_updated + diff.files_deleted
                    ),
                    tone: Tone::Neutral,
                },
            ],
        });
        sections.push(Section::Metric {
            label: "files".to_string(),
            columns: vec![
                MetricColumn {
                    value: format!("added {}", diff.files_added),
                    tone: Tone::Good,
                },
                MetricColumn {
                    value: format!("updated {}", diff.files_updated),
                    tone: Tone::Neutral,
                },
                MetricColumn {
                    value: format!("deleted {}", diff.files_deleted),
                    tone: Tone::Bad,
                },
            ],
        });
    }
    sections.push(Section::Metric {
        label: "gate".to_string(),
        columns: vec![MetricColumn {
            value: input.gate.clone(),
            tone: input.gate_tone,
        }],
    });
    let mut rows = Vec::new();
    if let Some(tests_modified) = input.tests_modified {
        rows.push((
            "tests modified this run".to_string(),
            if tests_modified { "yes" } else { "no" }.to_string(),
        ));
    }
    rows.extend(
        input
            .gate_caveats
            .iter()
            .map(|caveat| ("caveat".to_string(), caveat.clone())),
    );
    rows.push((
        "working".to_string(),
        input.working_dir.display().to_string(),
    ));
    if input.proof_block.is_none() {
        rows.push(("proof".to_string(), input.proof_path.display().to_string()));
    }
    sections.push(Section::KeyValue { rows });
    if input.outcome == OutcomeKind::Completed
        && let Some(proof_block) = input.proof_block.as_ref()
    {
        sections.push(Section::Blank);
        sections.push(Section::Lines {
            lines: proof_block.render_lines(),
        });
    }

    let primary_action = exit_summary_primary_action(input);
    let primary_command = primary_action.as_ref().map(|hint| hint.command.clone());

    Card {
        title: TitleLine {
            glyph,
            label: label.to_string(),
        },
        subtitle: Some(format!(
            "{} worked on {}",
            input.provider,
            input
                .branch
                .as_deref()
                .map(|branch| format!("branch {branch}"))
                .unwrap_or_else(|| "this run".to_string())
        )),
        sections,
        primary_action,
        hints: input
            .hints
            .iter()
            .filter(|(_, command)| Some(command.as_str()) != primary_command.as_deref())
            .map(|(label, command)| HintLine {
                label: label.clone(),
                command: command.clone(),
            })
            .collect(),
    }
}

fn exit_summary_primary_action(input: &ExitSummaryInput) -> Option<HintLine> {
    let priorities: &[&str] = match input.outcome {
        OutcomeKind::Completed => &["apply", "export", "undo", "finish"],
        OutcomeKind::Paused => &["resume"],
        OutcomeKind::Failed | OutcomeKind::Killed => &["why", "logs"],
    };
    let selected = input
        .hints
        .iter()
        .find(|(label, command)| {
            priorities.iter().any(|priority| priority == label)
                || (matches!(input.outcome, OutcomeKind::Failed | OutcomeKind::Killed)
                    && command.contains("--why-failed"))
                || (matches!(input.outcome, OutcomeKind::Paused) && command.contains(" resume "))
        })
        .or_else(|| input.hints.first())?;
    Some(HintLine {
        label: "next".to_string(),
        command: selected.1.clone(),
    })
}
