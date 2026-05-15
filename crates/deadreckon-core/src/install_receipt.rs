use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{IoContext, JsonContext};
use crate::{DeadreckonPaths, Result};

pub const INSTALL_RECEIPT_FILE: &str = "install-receipt.json";
pub const INSTALL_RECEIPT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Npm,
    Brew,
    Shell,
    Cargo,
    Source,
}

impl Channel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Brew => "brew",
            Self::Shell => "shell",
            Self::Cargo => "cargo",
            Self::Source => "source",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub channel: Channel,
    pub channel_version: String,
    pub binary_path: PathBuf,
    pub installed_at: DateTime<Utc>,
    pub install_source: Option<String>,
    pub platform_package: Option<String>,
    pub receipt_version: u32,
}

#[must_use]
pub fn receipt_path(paths: &DeadreckonPaths) -> PathBuf {
    paths.home().join(INSTALL_RECEIPT_FILE)
}

pub fn read_receipt(paths: &DeadreckonPaths) -> Result<Option<Receipt>> {
    let path = receipt_path(paths);
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(crate::DeadreckonError::Io { path, source }),
    };
    serde_json::from_slice(&raw).with_json_path(path).map(Some)
}

pub fn write_receipt(paths: &DeadreckonPaths, receipt: &Receipt) -> Result<()> {
    let path = receipt_path(paths);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    let raw = serde_json::to_vec_pretty(receipt).with_json_path(&path)?;
    fs::write(&path, raw).with_path(path)
}

#[must_use]
pub fn detect_channel(binary_path: &Path) -> Channel {
    let normalized = binary_path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();

    if normalized.contains("/node_modules/deadreckon/")
        || normalized.contains("/node_modules/deadreckon-")
    {
        return Channel::Npm;
    }
    if normalized.contains("/cellar/deadreckon/") {
        return Channel::Brew;
    }
    if normalized.contains("/.cargo/bin/deadreckon") {
        return Channel::Cargo;
    }
    if normalized.contains("/.local/share/deadreckon/")
        || normalized.contains("/appdata/local/deadreckon/")
    {
        return Channel::Shell;
    }

    Channel::Source
}

#[must_use]
pub fn detect_receipt(binary_path: &Path) -> Receipt {
    let channel = detect_channel(binary_path);
    Receipt {
        channel,
        channel_version: env!("CARGO_PKG_VERSION").to_string(),
        binary_path: binary_path.to_path_buf(),
        installed_at: Utc::now(),
        install_source: Some("detected".to_string()),
        platform_package: (channel == Channel::Npm).then(platform_package_for_current),
        receipt_version: INSTALL_RECEIPT_VERSION,
    }
}

#[must_use]
pub fn platform_package_for_current() -> String {
    platform_package_for(std::env::consts::OS, std::env::consts::ARCH)
        .unwrap_or_else(|| "deadreckon-unknown-unknown".to_string())
}

#[must_use]
pub fn platform_package_for(os: &str, arch: &str) -> Option<String> {
    let os = match os {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "win32",
        _ => return None,
    };
    let arch = match arch {
        "x86_64" | "x64" => "x64",
        "aarch64" | "arm64" => "arm64",
        _ => return None,
    };
    Some(format!("deadreckon-{os}-{arch}"))
}
