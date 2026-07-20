use std::path::PathBuf;

use deadreckon_core::glossary::run_status_label;
use deadreckon_core::{
    AcceptanceProgressEntry, PipelineState, acceptance_progress_path_for_run_root,
};
use deadreckon_protocol::TraceRecord;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::spine::{PrimaryAction, spine_for_run};
use crate::{Result, one_line, read_jsonl, run_prefix};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WhyReport {
    pub(crate) verdict_line: String,
    pub(crate) causes: Vec<CitedCause>,
    pub(crate) next: PrimaryAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CitedCause {
    pub(crate) kind: WhyCauseKind,
    pub(crate) summary: String,
    pub(crate) evidence_path: PathBuf,
    pub(crate) excerpt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WhyCauseKind {
    GateFailure,
    PausedAtCap,
    Tamper,
    ProviderError,
    FailureReason,
}

impl WhyCauseKind {
    fn label(self) -> &'static str {
        match self {
            Self::GateFailure => "gate failed",
            Self::PausedAtCap => "paused at cap",
            Self::Tamper => "tamper",
            Self::ProviderError => "provider error",
            Self::FailureReason => "failure reason",
        }
    }
}

pub(crate) fn why_for_run(state: &PipelineState) -> Result<WhyReport> {
    let mut causes = Vec::new();
    causes.extend(gate_failure_causes(state)?);
    causes.extend(tamper_causes(state)?);
    causes.extend(provider_error_causes(state)?);
    if let Some(reason) = state.pause_reason.as_deref() {
        let kind = if reason.to_ascii_lowercase().contains("cap") {
            WhyCauseKind::PausedAtCap
        } else {
            WhyCauseKind::FailureReason
        };
        causes.push(CitedCause {
            kind,
            summary: format!("paused: {reason}"),
            evidence_path: state.state_path(),
            excerpt: bounded_excerpt(reason),
        });
    }
    if let Some(reason) = state.failure_reason.as_deref() {
        causes.push(CitedCause {
            kind: WhyCauseKind::FailureReason,
            summary: format!("failure reason: {reason}"),
            evidence_path: state.state_path(),
            excerpt: bounded_excerpt(reason),
        });
    }

    let id = run_prefix(&state.run_id);
    let verdict_line = if causes.is_empty() {
        format!("nothing wrong recorded for run {id}")
    } else if let Some(reason) = state
        .failure_reason
        .as_deref()
        .or(state.pause_reason.as_deref())
    {
        format!(
            "run {id} is {} because {reason}; {}",
            run_status_label(state.status),
            causes[0].summary
        )
    } else {
        let first = causes[0].summary.as_str();
        format!(
            "run {id} is {} because {first}",
            run_status_label(state.status)
        )
    };
    let next = if causes.is_empty() {
        PrimaryAction::new("n")
    } else {
        spine_for_run(state, chrono::Utc::now()).next
    };
    Ok(WhyReport {
        verdict_line,
        causes,
        next,
    })
}

pub(crate) fn why_for_run_lossy(state: &PipelineState) -> WhyReport {
    why_for_run(state).unwrap_or_else(|error| WhyReport {
        verdict_line: format!(
            "why report unavailable for run {}",
            run_prefix(&state.run_id)
        ),
        causes: vec![CitedCause {
            kind: WhyCauseKind::FailureReason,
            summary: "why report read failed".to_string(),
            evidence_path: state.state_path(),
            excerpt: bounded_excerpt(&error.to_string()),
        }],
        next: PrimaryAction::new(format!("deadreckon attach {}", state.run_id)),
    })
}

pub(crate) fn why_plain_lines(report: &WhyReport) -> Vec<String> {
    let mut lines = vec![
        format!("why: {}", report.verdict_line),
        format!("next: {}", report.next.command),
    ];
    if report.causes.is_empty() {
        lines.push("try: n".to_string());
        return lines;
    }
    for cause in &report.causes {
        lines.push(format!(
            "- {}: {}",
            cause.kind.label(),
            one_line(&cause.summary, 120)
        ));
        lines.push(format!("  evidence: {}", cause.evidence_path.display()));
        if !cause.excerpt.is_empty() {
            lines.push(format!("  excerpt: {}", cause.excerpt));
        }
    }
    lines
}

pub(crate) fn why_panel_lines(report: &WhyReport) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        report.verdict_line.clone(),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(format!("next {}", report.next.command)));
    if report.causes.is_empty() {
        lines.push(Line::from("nothing wrong recorded"));
        lines.push(Line::from("try: n"));
        return lines;
    }
    for cause in &report.causes {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                cause.kind.label().to_string(),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(format!("  {}", one_line(&cause.summary, 110))),
        ]));
        lines.push(Line::from(format!(
            "evidence {}",
            cause.evidence_path.display()
        )));
        if !cause.excerpt.is_empty() {
            lines.push(Line::from(format!("excerpt {}", cause.excerpt)));
        }
    }
    lines
}

pub(crate) fn render_why_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    report: &WhyReport,
    scroll: usize,
) {
    let lines = why_panel_lines(report);
    let height = area.height.saturating_sub(2) as usize;
    let visible = lines
        .into_iter()
        .skip(scroll.min(usize::MAX))
        .take(height.max(1))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(visible)
            .block(Block::default().borders(Borders::ALL).title("why"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn gate_failure_causes(state: &PipelineState) -> Result<Vec<CitedCause>> {
    let path = acceptance_progress_path_for_run_root(&state.run_root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let progress = read_jsonl::<AcceptanceProgressEntry>(&path)?;
    Ok(progress
        .into_iter()
        .filter_map(|entry| {
            let result = entry.result?;
            if result.passed && entry.status != "failed" {
                return None;
            }
            let summary = format!("{} {}", result.kind, result.detail);
            let mut excerpt = summary.clone();
            if let Some(command) = result.command.as_deref() {
                excerpt.push_str(&format!(" command {command}"));
            }
            if let Some(stderr) = result.stderr.as_deref() {
                excerpt.push_str(&format!(" stderr {stderr}"));
            }
            Some(CitedCause {
                kind: WhyCauseKind::GateFailure,
                summary,
                evidence_path: path.clone(),
                excerpt: bounded_excerpt(&excerpt),
            })
        })
        .collect())
}

fn tamper_causes(state: &PipelineState) -> Result<Vec<CitedCause>> {
    let path = deadreckon_core::tamper::acceptance_tamper_path_for_run_root(&state.run_root);
    let Some(tamper) =
        deadreckon_core::tamper::read_acceptance_tamper_for_run_root(&state.run_root)?
    else {
        return Ok(Vec::new());
    };
    if tamper.verdict == deadreckon_core::tamper::AcceptanceTamperVerdict::Clean {
        return Ok(Vec::new());
    }
    let mut fragments = Vec::new();
    fragments.extend(tamper.caveats.iter().cloned());
    fragments.extend(tamper.refusal_reasons.iter().cloned());
    fragments.extend(tamper.covered_files_touched.iter().map(|touch| {
        format!(
            "{} {:?} by {}",
            touch.path, touch.classification, touch.by_check
        )
    }));
    fragments.extend(tamper.lint_findings.iter().map(|finding| {
        format!(
            "{} contains {} in {}",
            finding.check_kind, finding.pattern, finding.command
        )
    }));
    let summary = match tamper.verdict {
        deadreckon_core::tamper::AcceptanceTamperVerdict::Caveat => "tamper caveat",
        deadreckon_core::tamper::AcceptanceTamperVerdict::Refuse => "acceptance refused",
        deadreckon_core::tamper::AcceptanceTamperVerdict::Clean => "tamper clean",
    };
    Ok(vec![CitedCause {
        kind: WhyCauseKind::Tamper,
        summary: format!("{}: {}", summary, fragments.join("; ")),
        evidence_path: path,
        excerpt: bounded_excerpt(&fragments.join("; ")),
    }])
}

fn provider_error_causes(state: &PipelineState) -> Result<Vec<CitedCause>> {
    let path = state.run_root.join("traces.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let traces = read_jsonl::<TraceRecord>(&path)?;
    Ok(traces
        .into_iter()
        .rev()
        .filter(|trace| {
            trace.event.contains("error")
                || trace.event.contains("failed")
                || trace.detail.get("stderr").is_some()
                || trace.detail.get("exit_code").is_some()
        })
        .take(3)
        .map(|trace| {
            let detail = one_line(&trace.detail.to_string(), 160);
            CitedCause {
                kind: WhyCauseKind::ProviderError,
                summary: format!("turn {} {}", trace.turn, trace.event),
                evidence_path: path.clone(),
                excerpt: bounded_excerpt(&detail),
            }
        })
        .collect())
}

fn bounded_excerpt(value: &str) -> String {
    one_line(value, 180)
}
