use std::time::Duration;

use tachyonfx::{Effect, fx};

const GATE_PASS_DURATION_MS: u64 = 240;
const VERDICT_COMPLETION_DURATION_MS: u64 = 360;
const NODE_STATE_DURATION_MS: u64 = 120;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MotionPolicy {
    Full,
    #[default]
    Reduced,
    Off,
}

impl MotionPolicy {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::Full),
            "reduced" => Some(Self::Reduced),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub(crate) fn from_config_value(
        value: Option<&str>,
        stdout_is_tty: bool,
        replay_mode: bool,
    ) -> Self {
        value
            .and_then(Self::parse)
            .unwrap_or_else(|| Self::default_for_environment(stdout_is_tty, replay_mode))
    }

    pub(crate) fn default_for_environment(stdout_is_tty: bool, replay_mode: bool) -> Self {
        if stdout_is_tty && !replay_mode {
            Self::Full
        } else {
            Self::Reduced
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Reduced => "reduced",
            Self::Off => "off",
        }
    }

    fn permits(self, trigger: EffectTrigger) -> bool {
        match self {
            Self::Full => true,
            Self::Reduced => trigger == EffectTrigger::VerdictCompletion,
            Self::Off => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectTrigger {
    GatePass,
    VerdictCompletion,
    NodeStateChange,
}

pub(crate) fn registered_effect_triggers() -> [EffectTrigger; 3] {
    [
        EffectTrigger::GatePass,
        EffectTrigger::VerdictCompletion,
        EffectTrigger::NodeStateChange,
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiEffectEvent<'a> {
    Registered(EffectTrigger),
    Unregistered(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectKind {
    GatePassShimmer,
    VerdictCardFlash,
    NodeGlyphPulse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectFrame {
    pub(crate) trigger: EffectTrigger,
    pub(crate) kind: EffectKind,
    pub(crate) duration: Duration,
    pub(crate) input_preemptible: bool,
}

impl EffectFrame {
    pub(crate) fn tachyon_effect(self) -> Effect {
        let duration_ms = self.duration.as_millis().min(u128::from(u32::MAX)) as u32;
        match self.kind {
            EffectKind::GatePassShimmer => fx::coalesce(duration_ms),
            EffectKind::VerdictCardFlash => fx::dissolve(duration_ms),
            EffectKind::NodeGlyphPulse => fx::coalesce(duration_ms),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectFrameDecision {
    NoFrame,
    DrawFrame,
    PreemptedForInput,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EffectRegistry {
    motion: MotionPolicy,
}

impl EffectRegistry {
    pub(crate) fn new(motion: MotionPolicy) -> Self {
        Self { motion }
    }

    pub(crate) fn frames_for_event(self, event: UiEffectEvent<'_>) -> Vec<EffectFrame> {
        let trigger = match event {
            UiEffectEvent::Registered(trigger) => trigger,
            UiEffectEvent::Unregistered(name) => {
                let _ = name.len();
                return Vec::new();
            }
        };
        if !self.motion.permits(trigger) {
            return Vec::new();
        }
        vec![effect_for_trigger(trigger)]
    }

    pub(crate) fn next_frame_decision(
        self,
        event: UiEffectEvent<'_>,
        input_pending: bool,
    ) -> EffectFrameDecision {
        let frames = self.frames_for_event(event);
        if frames.is_empty() {
            EffectFrameDecision::NoFrame
        } else if input_pending && frames.iter().any(|frame| frame.input_preemptible) {
            EffectFrameDecision::PreemptedForInput
        } else {
            EffectFrameDecision::DrawFrame
        }
    }
}

fn effect_for_trigger(trigger: EffectTrigger) -> EffectFrame {
    let (kind, duration_ms) = match trigger {
        EffectTrigger::GatePass => (EffectKind::GatePassShimmer, GATE_PASS_DURATION_MS),
        EffectTrigger::VerdictCompletion => {
            (EffectKind::VerdictCardFlash, VERDICT_COMPLETION_DURATION_MS)
        }
        EffectTrigger::NodeStateChange => (EffectKind::NodeGlyphPulse, NODE_STATE_DURATION_MS),
    };
    EffectFrame {
        trigger,
        kind,
        duration: Duration::from_millis(duration_ms),
        input_preemptible: true,
    }
}
