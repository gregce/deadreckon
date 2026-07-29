use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Additive child-process metadata written beside a supervised run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisedProcess {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgid: Option<u32>,
}

pub fn write_supervised_process(path: &Path, process: SupervisedProcess) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut encoded = serde_json::to_vec(&process).map_err(io::Error::other)?;
    encoded.push(b'\n');
    fs::write(path, encoded)
}

pub fn read_supervised_process(path: &Path) -> io::Result<SupervisedProcess> {
    let bytes = fs::read(path)?;
    let trimmed = trim_ascii(&bytes);
    if trimmed.starts_with(b"{") {
        return serde_json::from_slice(trimmed)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    }

    // Provider pid files before Capstan contained only `<pid>\n`.
    let raw = std::str::from_utf8(trimmed)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let pid = raw
        .parse::<u32>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(SupervisedProcess { pid, pgid: None })
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::{SupervisedProcess, read_supervised_process, write_supervised_process};

    #[test]
    fn pid_file_gains_additive_pgid_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("child.pid");
        write_supervised_process(
            &path,
            SupervisedProcess {
                pid: 41,
                pgid: Some(41),
            },
        )
        .expect("write metadata");

        let raw = std::fs::read_to_string(&path).expect("read metadata");
        assert!(raw.contains("\"pid\":41"));
        assert!(raw.contains("\"pgid\":41"));
        assert_eq!(
            read_supervised_process(&path).expect("parse metadata"),
            SupervisedProcess {
                pid: 41,
                pgid: Some(41),
            }
        );
    }

    #[test]
    fn absent_pgid_key_reads_as_legacy() {
        let temp = tempfile::tempdir().expect("tempdir");
        let json_path = temp.path().join("legacy-json.pid");
        std::fs::write(&json_path, "{\"pid\":42}\n").expect("write legacy json");
        assert_eq!(
            read_supervised_process(&json_path).expect("parse legacy json"),
            SupervisedProcess {
                pid: 42,
                pgid: None,
            }
        );

        let text_path = temp.path().join("legacy-text.pid");
        std::fs::write(&text_path, "43\n").expect("write legacy text");
        assert_eq!(
            read_supervised_process(&text_path).expect("parse legacy text"),
            SupervisedProcess {
                pid: 43,
                pgid: None,
            }
        );
    }
}
