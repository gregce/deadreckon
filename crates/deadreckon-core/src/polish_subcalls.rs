//! Contracts for provider-backed documentation polish subcalls.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DEFAULT_DOC_SUBSKILLS: &[&str] = &[
    "narrator-overview",
    "narrator-phases",
    "narrator-as-built",
    "narrator-decisions",
];

pub const DEFAULT_DOC_POLISH_TOKEN_BUDGET: u32 = 16_384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocProviderSelection {
    pub provider: Option<String>,
    pub source: DocProviderSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocProviderSource {
    Flag,
    Config,
    AutoSubscription,
    RunProvider,
    None,
}

impl DocProviderSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Config => "config",
            Self::AutoSubscription => "auto_subscription",
            Self::RunProvider => "run_provider",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolishSubcallRecord {
    pub skill: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_source: Option<String>,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub retries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolishDiffCoverage {
    #[serde(default)]
    pub changed_files: usize,
    #[serde(default)]
    pub missing_files: Vec<String>,
    #[serde(default)]
    pub retries: u32,
}
