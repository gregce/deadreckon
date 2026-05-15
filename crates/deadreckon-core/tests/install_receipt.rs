#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use chrono::Utc;
use deadreckon_core::DeadreckonPaths;
use deadreckon_core::install_receipt::{
    Channel, Receipt, detect_channel, read_receipt, write_receipt,
};
use tempfile::TempDir;

#[test]
fn install_receipt_roundtrips_every_channel_variant() {
    for channel in [
        Channel::Npm,
        Channel::Brew,
        Channel::Shell,
        Channel::Cargo,
        Channel::Source,
    ] {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let receipt = Receipt {
            channel,
            channel_version: "0.2.0".to_string(),
            binary_path: PathBuf::from("/usr/local/bin/deadreckon"),
            installed_at: Utc::now(),
            install_source: Some("test".to_string()),
            platform_package: (channel == Channel::Npm)
                .then(|| "deadreckon-darwin-arm64".to_string()),
            receipt_version: 1,
        };

        write_receipt(&paths, &receipt).expect("write receipt");
        let reloaded = read_receipt(&paths)
            .expect("read receipt")
            .expect("receipt present");

        assert_eq!(reloaded, receipt);
    }
}

#[test]
fn install_receipt_detects_npm_path_layout() {
    assert_eq!(
        detect_channel(Path::new(
            "/Users/me/.bun/install/global/node_modules/deadreckon/bin/deadreckon"
        )),
        Channel::Npm
    );
}

#[test]
fn install_receipt_detects_brew_cellar_layout() {
    assert_eq!(
        detect_channel(Path::new(
            "/opt/homebrew/Cellar/deadreckon/0.2.0/bin/deadreckon"
        )),
        Channel::Brew
    );
}

#[test]
fn install_receipt_detects_cargo_bin_layout() {
    assert_eq!(
        detect_channel(Path::new("/Users/me/.cargo/bin/deadreckon")),
        Channel::Cargo
    );
}

#[test]
fn install_receipt_detects_shell_install_layout() {
    assert_eq!(
        detect_channel(Path::new(
            "/Users/me/.local/share/deadreckon/bin/deadreckon"
        )),
        Channel::Shell
    );
}

#[test]
fn install_receipt_falls_back_to_source_on_unknown_path() {
    assert_eq!(
        detect_channel(Path::new(
            "/Users/me/src/deadreckon/target/debug/deadreckon"
        )),
        Channel::Source
    );
}
