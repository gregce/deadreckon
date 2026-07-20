use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use deadreckon_core::flight::{CheckpointChangeKind, list_checkpoint_manifests};
use deadreckon_core::{
    AcceptanceProgressEntry, PipelineState, acceptance_progress_path_for_run_root,
};
use deadreckon_protocol::{RunEvent, SpendRecord, TraceRecord};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::{Result, one_line, read_jsonl};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineModel {
    pub(crate) entries: Vec<TimelineEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineEntry {
    pub(crate) turn: u32,
    pub(crate) checkpoint_ids: Vec<String>,
    pub(crate) created_at: Option<DateTime<Utc>>,
    pub(crate) story: String,
    pub(crate) diff: TimelineDiff,
    pub(crate) spend_delta_usd: f64,
    pub(crate) marks: Vec<TimelineMark>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TimelineDiff {
    pub(crate) created: usize,
    pub(crate) modified: usize,
    pub(crate) deleted: usize,
    pub(crate) skipped: usize,
}

impl TimelineDiff {
    fn total(self) -> usize {
        self.created + self.modified + self.deleted + self.skipped
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TimelineMark {
    Checkpoint,
    GatePassed,
    GateFailed,
    Tamper,
    Reshape,
}

impl TimelineMark {
    fn label(self) -> &'static str {
        match self {
            Self::Checkpoint => "checkpoint",
            Self::GatePassed => "gate passed",
            Self::GateFailed => "gate failed",
            Self::Tamper => "tamper",
            Self::Reshape => "reshape",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Checkpoint => Style::default().fg(Color::Cyan),
            Self::GatePassed => Style::default().fg(Color::Green),
            Self::GateFailed => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Self::Tamper => Style::default().fg(Color::Yellow),
            Self::Reshape => Style::default().fg(Color::Magenta),
        }
    }
}

#[derive(Default)]
struct TimelineEntryBuilder {
    checkpoint_ids: Vec<String>,
    created_at: Option<DateTime<Utc>>,
    diff: TimelineDiff,
    spend_delta_usd: f64,
    marks: Vec<TimelineMark>,
}

impl TimelineEntryBuilder {
    fn push_mark(&mut self, mark: TimelineMark) {
        if !self.marks.contains(&mark) {
            self.marks.push(mark);
            self.marks.sort();
        }
    }
}

pub(crate) fn timeline_for_run(
    state: &PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    _events: &[RunEvent],
) -> Result<TimelineModel> {
    let mut builders: BTreeMap<u32, TimelineEntryBuilder> = BTreeMap::new();

    for checkpoint in list_checkpoint_manifests(state)? {
        let entry = builders.entry(checkpoint.deadreckon_turn).or_default();
        entry.checkpoint_ids.push(checkpoint.checkpoint_id);
        entry.created_at = Some(match entry.created_at {
            Some(existing) => existing.min(checkpoint.created_at),
            None => checkpoint.created_at,
        });
        entry.push_mark(TimelineMark::Checkpoint);
        for file in checkpoint.files {
            match file.change {
                CheckpointChangeKind::Created => entry.diff.created += 1,
                CheckpointChangeKind::Modified => entry.diff.modified += 1,
                CheckpointChangeKind::Deleted => entry.diff.deleted += 1,
                CheckpointChangeKind::SkippedOversize => entry.diff.skipped += 1,
            }
        }
    }

    for record in spend {
        builders.entry(record.turn).or_default().spend_delta_usd += record.cost_usd;
    }

    for trace in traces {
        if trace.event == "reshape.proposed" {
            builders
                .entry(trace.turn)
                .or_default()
                .push_mark(TimelineMark::Reshape);
        }
    }

    if let Some(mark) = gate_mark(state)? {
        push_latest_mark(&mut builders, mark);
    }
    if tamper_mark(state)? {
        push_latest_mark(&mut builders, TimelineMark::Tamper);
    }

    Ok(TimelineModel {
        entries: builders
            .into_iter()
            .map(|(turn, mut entry)| {
                entry.checkpoint_ids.sort();
                let story = timeline_story(turn, &entry.checkpoint_ids, entry.diff);
                TimelineEntry {
                    turn,
                    checkpoint_ids: entry.checkpoint_ids,
                    created_at: entry.created_at,
                    story,
                    diff: entry.diff,
                    spend_delta_usd: entry.spend_delta_usd,
                    marks: entry.marks,
                }
            })
            .collect(),
    })
}

pub(crate) fn timeline_for_run_lossy(
    state: &PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
) -> TimelineModel {
    timeline_for_run(state, spend, traces, events).unwrap_or_else(|_| TimelineModel {
        entries: Vec::new(),
    })
}

pub(crate) fn timeline_detail_lines(
    timeline: &TimelineModel,
    selected: usize,
) -> Vec<Line<'static>> {
    let Some(entry) = selected_entry(timeline, selected) else {
        return vec![
            Line::from("No timeline entries yet."),
            Line::from("Provider checkpoints, spend rows, and trace marks appear here."),
        ];
    };
    let checkpoint = if entry.checkpoint_ids.is_empty() {
        "checkpoint none".to_string()
    } else {
        format!("checkpoint {}", entry.checkpoint_ids.join(", "))
    };
    let marks = mark_labels(&entry.marks);
    vec![
        Line::from(format!("turn {}  {checkpoint}", entry.turn)),
        Line::from(format!(
            "diff +{} ~{} -{}  spend ${:.4}",
            entry.diff.created, entry.diff.modified, entry.diff.deleted, entry.spend_delta_usd
        )),
        Line::from(format!("story {}", one_line(&entry.story, 130))),
        Line::from(format!(
            "marks {}",
            if marks.is_empty() {
                "none".to_string()
            } else {
                marks
            }
        )),
    ]
}

pub(crate) fn render_timeline_detail(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    timeline: &TimelineModel,
    selected: usize,
) {
    frame.render_widget(
        Paragraph::new(timeline_detail_lines(timeline, selected))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("timeline detail"),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub(crate) fn render_timeline_band(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    timeline: &TimelineModel,
    selected: usize,
    focused: bool,
) {
    let selected = selected_index(timeline, selected);
    let title = if focused {
        "timeline [focused]"
    } else {
        "timeline"
    };
    frame.render_widget(
        Paragraph::new(timeline_band_lines(timeline, selected))
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn timeline_band_lines(timeline: &TimelineModel, selected: usize) -> Vec<Line<'static>> {
    if timeline.entries.is_empty() {
        return vec![Line::from("no checkpoints yet")];
    }
    let mut spans = Vec::new();
    for (index, entry) in timeline.entries.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let prefix = if index == selected { ">" } else { " " };
        spans.push(Span::styled(
            format!(
                "{prefix}t{} +{} ~{} -{}",
                entry.turn, entry.diff.created, entry.diff.modified, entry.diff.deleted
            ),
            if index == selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ));
        for mark in &entry.marks {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(mark.label(), mark.style()));
        }
    }
    vec![Line::from(spans)]
}

fn selected_entry(timeline: &TimelineModel, selected: usize) -> Option<&TimelineEntry> {
    let index = selected_index(timeline, selected);
    timeline.entries.get(index)
}

fn selected_index(timeline: &TimelineModel, selected: usize) -> usize {
    selected.min(timeline.entries.len().saturating_sub(1))
}

fn timeline_story(turn: u32, checkpoint_ids: &[String], diff: TimelineDiff) -> String {
    if checkpoint_ids.is_empty() {
        return format!("turn {turn}: no checkpoint changed {} files", diff.total());
    }
    format!(
        "turn {turn}: {} changed {} files (+{} ~{} -{})",
        checkpoint_ids.join(", "),
        diff.total(),
        diff.created,
        diff.modified,
        diff.deleted
    )
}

fn mark_labels(marks: &[TimelineMark]) -> String {
    marks
        .iter()
        .map(|mark| mark.label())
        .collect::<Vec<_>>()
        .join(", ")
}

fn gate_mark(state: &PipelineState) -> Result<Option<TimelineMark>> {
    let path = acceptance_progress_path_for_run_root(&state.run_root);
    if !path.exists() {
        return Ok(None);
    }
    let entries = read_jsonl::<AcceptanceProgressEntry>(&path)?;
    if entries.iter().any(|entry| {
        entry.status == "failed"
            || entry
                .result
                .as_ref()
                .is_some_and(|result| !result.passed && result.must_pass)
    }) {
        return Ok(Some(TimelineMark::GateFailed));
    }
    Ok(entries
        .iter()
        .any(|entry| {
            entry.status == "passed" || entry.result.as_ref().is_some_and(|result| result.passed)
        })
        .then_some(TimelineMark::GatePassed))
}

fn tamper_mark(state: &PipelineState) -> Result<bool> {
    let Some(tamper) =
        deadreckon_core::tamper::read_acceptance_tamper_for_run_root(&state.run_root)?
    else {
        return Ok(false);
    };
    Ok(tamper.verdict != deadreckon_core::tamper::AcceptanceTamperVerdict::Clean)
}

fn push_latest_mark(builders: &mut BTreeMap<u32, TimelineEntryBuilder>, mark: TimelineMark) {
    let turn = builders.keys().next_back().copied().unwrap_or(0);
    builders.entry(turn).or_default().push_mark(mark);
}
