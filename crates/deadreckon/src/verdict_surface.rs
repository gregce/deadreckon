use serde_json::{Map, Value, json};

use crate::ui_card::{Card, CardOptions, HintLine, Section, TitleGlyph, TitleLine, render_card};

pub const TERMINAL_OUTCOME_SURFACE_VERBS: &[&str] = &[
    "init",
    "config",
    "completion",
    "acceptance",
    "def-done",
    "start",
    "run",
    "orchestrate",
    "campaign",
    "plan",
    "fork",
    "merge",
    "chain",
    "doctor",
    "detect",
    "providers",
    "update",
    "list",
    "library",
    "finish",
    "materialize",
    "apply",
    "abandon",
    "cleanup",
    "extend",
    "doc",
    "attach",
    "kill",
    "resume",
    "undo",
    "rewind",
    "show",
    "history",
    "status",
    "import",
    "learn",
    "improve",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerdictKind {
    Completed,
    Verified,
    Failed,
    Blocked,
    Killed,
    Paused,
    Preview,
    Noop,
}

impl VerdictKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Verified => "verified",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Killed => "killed",
            Self::Paused => "paused",
            Self::Preview => "preview",
            Self::Noop => "no-op",
        }
    }

    const fn glyph(self) -> TitleGlyph {
        match self {
            Self::Completed | Self::Verified => TitleGlyph::Success,
            Self::Failed | Self::Blocked => TitleGlyph::Failed,
            Self::Killed => TitleGlyph::Stopped,
            Self::Paused => TitleGlyph::Paused,
            Self::Preview | Self::Noop => TitleGlyph::Preview,
        }
    }

    const fn tone(self) -> crate::ui::Tone {
        use crate::ui::Tone;
        match self {
            Self::Completed | Self::Verified => Tone::Ok,
            Self::Failed | Self::Blocked | Self::Killed => Tone::Negative,
            Self::Paused => Tone::Paused,
            Self::Preview | Self::Noop => Tone::Heading,
        }
    }
}

/// Color the leading status word of an evidence value (passed / warning / failed /
/// missing / …) when it is a recognized status, leaving the rest plain. A value
/// that does not start with a status word is returned unchanged.
fn color_evidence_value(value: &str) -> String {
    use crate::ui::{self, Stream, Tone};
    let mut parts = value.splitn(2, ' ');
    let Some(first) = parts.next() else {
        return value.to_string();
    };
    let tone = ui::status_tone(first);
    if tone == Tone::Plain {
        return value.to_string();
    }
    match parts.next() {
        Some(rest) => format!("{} {}", ui::render(Stream::Stdout, tone, first), rest),
        None => ui::render(Stream::Stdout, tone, first),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExplanationPanel {
    pub what_happened: String,
    pub why_this_verdict: String,
    pub evidence: Vec<(String, String)>,
}

impl ExplanationPanel {
    pub fn new<K, V>(
        what_happened: impl Into<String>,
        why_this_verdict: impl Into<String>,
        evidence: impl IntoIterator<Item = (K, V)>,
    ) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            what_happened: what_happened.into(),
            why_this_verdict: why_this_verdict.into(),
            evidence: evidence
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    fn validate(&self) -> Result<(), VerdictSurfaceError> {
        if self.what_happened.trim().is_empty()
            || self.why_this_verdict.trim().is_empty()
            || self.evidence.is_empty()
        {
            return Err(VerdictSurfaceError::MissingExplanation);
        }
        Ok(())
    }

    pub fn sections(&self) -> Vec<Section> {
        vec![
            Section::Lines {
                lines: vec![
                    "Explanation".to_string(),
                    self.what_happened.clone(),
                    self.why_this_verdict.clone(),
                    String::new(),
                    "Evidence".to_string(),
                ],
            },
            Section::KeyValue {
                rows: self.evidence.clone(),
            },
        ]
    }

    fn one_line(&self) -> String {
        format!("{} {}", self.what_happened, self.why_this_verdict)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerdictSurface {
    pub kind: VerdictKind,
    pub subject_kind: String,
    pub subject: Option<String>,
    pub explanation: ExplanationPanel,
    pub primary_action: HintLine,
    pub secondary_actions: Vec<HintLine>,
}

impl VerdictSurface {
    pub fn try_new<L, C, SL, SC>(
        kind: VerdictKind,
        subject_kind: impl Into<String>,
        subject: Option<&str>,
        explanation: ExplanationPanel,
        primary_actions: impl IntoIterator<Item = (L, C)>,
        secondary_actions: impl IntoIterator<Item = (SL, SC)>,
    ) -> Result<Self, VerdictSurfaceError>
    where
        L: Into<String>,
        C: Into<String>,
        SL: Into<String>,
        SC: Into<String>,
    {
        let subject_kind = subject_kind.into();
        if subject_kind.trim().is_empty() {
            return Err(VerdictSurfaceError::MissingSubjectKind);
        }
        explanation.validate()?;

        let primary = primary_actions
            .into_iter()
            .map(|(label, command)| HintLine {
                label: label.into(),
                command: command.into(),
            })
            .collect::<Vec<_>>();
        if primary.len() != 1 {
            return Err(VerdictSurfaceError::ExpectedOnePrimaryAction {
                actual: primary.len(),
            });
        }
        let mut primary = primary;
        let primary_action = primary.remove(0);
        if primary_action.command.trim().is_empty() {
            return Err(VerdictSurfaceError::MissingPrimaryCommand);
        }

        let primary_command = primary_action.command.clone();
        let secondary_actions = secondary_actions
            .into_iter()
            .map(|(label, command)| HintLine {
                label: label.into(),
                command: command.into(),
            })
            .filter(|action| action.command != primary_command)
            .collect();

        Ok(Self {
            kind,
            subject_kind,
            subject: subject.map(str::to_string),
            explanation,
            primary_action,
            secondary_actions,
        })
    }

    pub fn label(&self) -> String {
        match self.subject.as_deref() {
            Some(subject) if !subject.trim().is_empty() => {
                format!("{} {} {}", self.kind.as_str(), self.subject_kind, subject)
            }
            _ => format!("{} {}", self.kind.as_str(), self.subject_kind),
        }
    }

    pub fn render_card(&self, options: &CardOptions) -> String {
        let mut sections = self.explanation.sections();
        if !self.secondary_actions.is_empty() {
            sections.push(Section::Blank);
            sections.push(Section::Lines {
                lines: vec!["Secondary".to_string()],
            });
        }
        render_card(
            &Card {
                title: TitleLine {
                    glyph: self.kind.glyph(),
                    label: self.label(),
                },
                subtitle: None,
                sections,
                primary_action: Some(self.primary_action.clone()),
                hints: self.secondary_actions.clone(),
            },
            options,
        )
    }

    pub fn render_plain(&self, no_hints: bool) -> String {
        use crate::ui::{self, Stream, Tone};
        // Color is gated on ui::enabled (TTY, not --plain/NO_COLOR), so this stays
        // byte-identical plain text on non-terminals and in the golden tests while
        // a real terminal gets a readable, tone-coded surface.
        let heading = |text: &str| ui::render(Stream::Stdout, Tone::Heading, text);
        let command = |text: &str| ui::render(Stream::Stdout, Tone::Command, text);
        let mut out = String::new();
        out.push_str(&ui::render(Stream::Stdout, self.kind.tone(), self.label()));
        out.push_str("\n\n");
        out.push_str(&heading("Explanation"));
        out.push('\n');
        out.push_str(self.explanation.what_happened.trim());
        out.push('\n');
        out.push_str(self.explanation.why_this_verdict.trim());
        out.push_str("\n\n");
        out.push_str(&heading("Evidence"));
        out.push('\n');
        for (key, value) in &self.explanation.evidence {
            out.push_str(&ui::render(Stream::Stdout, Tone::Muted, key));
            out.push_str(": ");
            out.push_str(&color_evidence_value(value));
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&heading("Recommended"));
        out.push('\n');
        out.push_str(&command(&self.primary_action.command));
        out.push('\n');
        if !no_hints && !self.secondary_actions.is_empty() {
            out.push('\n');
            out.push_str(&heading("Secondary"));
            out.push('\n');
            for action in &self.secondary_actions {
                out.push_str(&command(&action.command));
                out.push('\n');
            }
        }
        out
    }

    pub fn verdict_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "label": self.label(),
            "subject": self.subject.as_deref().unwrap_or(self.subject_kind.as_str()),
            "recommended_command": self.primary_action.command,
            "explanation": self.explanation.one_line(),
            "evidence": self.explanation.evidence.iter().map(|(key, value)| {
                json!([key, value])
            }).collect::<Vec<_>>(),
        })
    }

    pub fn add_to_json(&self, mut value: Value) -> Value {
        if !value.is_object() {
            value = Value::Object(Map::new());
        }
        if let Value::Object(object) = &mut value {
            object.insert(
                "primary_action".to_string(),
                Value::String(self.primary_action.command.clone()),
            );
            object.insert("verdict".to_string(), self.verdict_json());
        }
        value
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerdictSurfaceError {
    ExpectedOnePrimaryAction { actual: usize },
    MissingExplanation,
    MissingSubjectKind,
    MissingPrimaryCommand,
}
