use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::{Result, SandboxBackend, SandboxError, backend_executable};

pub const DOCKER_SIDECAR_CONTAINER_PROGRAM: &str = "/usr/local/bin/dr-gate-evaluate";

const MANAGED_LABEL: &str = "io.deadreckon.managed";
const MANAGED_VALUE: &str = "gate-evaluator";
const JOB_ID_LABEL: &str = "io.deadreckon.job-id";
const LAUNCH_ID_LABEL: &str = "io.deadreckon.launch-id";
const ATTEMPT_LABEL: &str = "io.deadreckon.attempt";
const OWNER_LAUNCH_ID_LABEL: &str = "io.deadreckon.owner-launch-id";

pub const DOCKER_EXECUTION_RECORD_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DockerPlatform {
    LinuxAmd64,
    LinuxArm64,
}

impl DockerPlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LinuxAmd64 => "linux/amd64",
            Self::LinuxArm64 => "linux/arm64",
        }
    }

    pub fn architecture(self) -> &'static str {
        match self {
            Self::LinuxAmd64 => "amd64",
            Self::LinuxArm64 => "arm64",
        }
    }

    fn from_inspection(os: &str, architecture: &str) -> Result<Self> {
        if os != "linux" {
            return Err(invalid_docker(format!(
                "Docker evaluator image must be Linux, observed {os}/{architecture}"
            )));
        }
        match architecture {
            "amd64" | "x86_64" => Ok(Self::LinuxAmd64),
            "arm64" | "aarch64" => Ok(Self::LinuxArm64),
            other => Err(invalid_docker(format!(
                "unsupported Docker evaluator architecture {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerImage {
    id: String,
    platform: DockerPlatform,
}

impl DockerImage {
    pub fn new(id: impl Into<String>, platform: DockerPlatform) -> Result<Self> {
        let id = id.into().to_ascii_lowercase();
        validate_image_id(&id)?;
        Ok(Self { id, platform })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn platform(&self) -> DockerPlatform {
        self.platform
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerExecution {
    image: DockerImage,
    sidecar_host_path: PathBuf,
    container_name: String,
    cid_file: PathBuf,
    job_id: String,
    launch_id: String,
    attempt: u32,
    owner_launch_id: Option<String>,
}

impl DockerExecution {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        image: DockerImage,
        sidecar_host_path: impl Into<PathBuf>,
        container_name: impl Into<String>,
        cid_file: impl Into<PathBuf>,
        job_id: impl Into<String>,
        launch_id: impl Into<String>,
        attempt: u32,
        owner_launch_id: Option<String>,
    ) -> Result<Self> {
        let sidecar_host_path = sidecar_host_path.into();
        let container_name = container_name.into();
        let cid_file = cid_file.into();
        let job_id = job_id.into();
        let launch_id = launch_id.into();
        validate_sidecar(&sidecar_host_path)?;
        validate_container_name(&container_name)?;
        validate_cid_file(&cid_file)?;
        validate_label_value("Job ID", &job_id)?;
        validate_label_value("launch ID", &launch_id)?;
        if attempt == 0 {
            return Err(invalid_docker(
                "Docker evaluator attempt must be positive".to_string(),
            ));
        }
        if let Some(owner) = owner_launch_id.as_deref() {
            validate_label_value("owner launch ID", owner)?;
        }
        let sidecar_host_path = sidecar_host_path.canonicalize().map_err(SandboxError::Io)?;
        validate_sidecar_mount_path(&sidecar_host_path)?;
        Ok(Self {
            image,
            sidecar_host_path,
            container_name,
            cid_file,
            job_id,
            launch_id,
            attempt,
            owner_launch_id,
        })
    }

    pub fn image(&self) -> &DockerImage {
        &self.image
    }

    pub fn sidecar_host_path(&self) -> &Path {
        &self.sidecar_host_path
    }

    pub fn container_program(&self) -> &'static str {
        DOCKER_SIDECAR_CONTAINER_PROGRAM
    }

    pub fn container_name(&self) -> &str {
        &self.container_name
    }

    pub fn cid_file(&self) -> &Path {
        &self.cid_file
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn launch_id(&self) -> &str {
        &self.launch_id
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn owner_launch_id(&self) -> Option<&str> {
        self.owner_launch_id.as_deref()
    }

    pub(crate) fn labels(&self) -> BTreeMap<String, String> {
        self.record().labels()
    }

    pub fn record(&self) -> DockerExecutionRecord {
        DockerExecutionRecord {
            schema_version: DOCKER_EXECUTION_RECORD_SCHEMA_VERSION,
            image_id: self.image.id.clone(),
            platform: self.image.platform,
            sidecar_host_path: self.sidecar_host_path.clone(),
            container_name: self.container_name.clone(),
            cid_file: self.cid_file.clone(),
            job_id: self.job_id.clone(),
            launch_id: self.launch_id.clone(),
            attempt: self.attempt,
            owner_launch_id: self.owner_launch_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerExecutionRecord {
    schema_version: u32,
    image_id: String,
    platform: DockerPlatform,
    sidecar_host_path: PathBuf,
    container_name: String,
    cid_file: PathBuf,
    job_id: String,
    launch_id: String,
    attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_launch_id: Option<String>,
}

impl DockerExecutionRecord {
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn image_id(&self) -> &str {
        &self.image_id
    }

    pub fn platform(&self) -> DockerPlatform {
        self.platform
    }

    pub fn sidecar_host_path(&self) -> &Path {
        &self.sidecar_host_path
    }

    pub fn container_name(&self) -> &str {
        &self.container_name
    }

    pub fn cid_file(&self) -> &Path {
        &self.cid_file
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn launch_id(&self) -> &str {
        &self.launch_id
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn owner_launch_id(&self) -> Option<&str> {
        self.owner_launch_id.as_deref()
    }

    /// Durably create this immutable recovery record without replacing a
    /// different launch identity already present at `path`.
    pub fn write_to(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_execution_record(path, self)
    }

    fn labels(&self) -> BTreeMap<String, String> {
        let mut labels = BTreeMap::from([
            (MANAGED_LABEL.to_string(), MANAGED_VALUE.to_string()),
            (JOB_ID_LABEL.to_string(), self.job_id.clone()),
            (LAUNCH_ID_LABEL.to_string(), self.launch_id.clone()),
            (ATTEMPT_LABEL.to_string(), self.attempt.to_string()),
        ]);
        if let Some(owner) = self.owner_launch_id.as_ref() {
            labels.insert(OWNER_LAUNCH_ID_LABEL.to_string(), owner.clone());
        }
        labels
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != DOCKER_EXECUTION_RECORD_SCHEMA_VERSION {
            return Err(invalid_docker(format!(
                "unsupported Docker execution record schema {}",
                self.schema_version
            )));
        }
        validate_image_id(&self.image_id)?;
        validate_container_name(&self.container_name)?;
        validate_cid_file(&self.cid_file)?;
        if !self.sidecar_host_path.is_absolute() {
            return Err(invalid_docker(format!(
                "Docker evaluator sidecar record must use an absolute path: {}",
                self.sidecar_host_path.display()
            )));
        }
        validate_sidecar_mount_path(&self.sidecar_host_path)?;
        validate_label_value("Job ID", &self.job_id)?;
        validate_label_value("launch ID", &self.launch_id)?;
        if self.attempt == 0 {
            return Err(invalid_docker(
                "Docker evaluator attempt must be positive".to_string(),
            ));
        }
        if let Some(owner) = self.owner_launch_id.as_deref() {
            validate_label_value("owner launch ID", owner)?;
        }
        Ok(())
    }
}

pub fn inspect_docker_image(image: &OsStr) -> Result<DockerImage> {
    let wrapper = backend_executable(SandboxBackend::Docker)?;
    inspect_docker_image_with(&wrapper, image)
}

pub fn reconcile_docker_execution(execution: &DockerExecution) -> Result<()> {
    let wrapper = backend_executable(SandboxBackend::Docker)?;
    reconcile_docker_execution_with(&wrapper, &execution.record())
}

pub fn write_docker_execution_record(path: &Path, execution: &DockerExecution) -> Result<()> {
    let record = execution.record();
    record.validate()?;
    write_execution_record(path, &record)
}

pub fn read_docker_execution_record(path: &Path) -> Result<DockerExecutionRecord> {
    validate_absolute_record_path(path)?;
    let raw = read_bounded_regular(path, 64 * 1024, "Docker execution record")?;
    let record: DockerExecutionRecord = serde_json::from_slice(&raw)
        .map_err(|error| invalid_docker(format!("invalid Docker execution record: {error}")))?;
    record.validate()?;
    Ok(record)
}

pub fn reconcile_docker_execution_record(path: &Path) -> Result<()> {
    let Some(record) = read_docker_execution_record_for_reconciliation(path)? else {
        return Ok(());
    };
    let wrapper = backend_executable(SandboxBackend::Docker)?;
    reconcile_docker_execution_record_value_with(&wrapper, path, &record)
}

#[cfg(test)]
fn reconcile_docker_execution_record_with(wrapper: &Path, path: &Path) -> Result<()> {
    let Some(record) = read_docker_execution_record_for_reconciliation(path)? else {
        return Ok(());
    };
    reconcile_docker_execution_record_value_with(wrapper, path, &record)
}

fn read_docker_execution_record_for_reconciliation(
    path: &Path,
) -> Result<Option<DockerExecutionRecord>> {
    match read_docker_execution_record(path) {
        Ok(record) => Ok(Some(record)),
        Err(SandboxError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            // An absent recovery record is the terminal state of successful
            // reconciliation. Confirm that the path itself is absent so a
            // dangling symlink cannot masquerade as completed cleanup.
            match fs::symlink_metadata(path) {
                Err(metadata_error) if metadata_error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(None)
                }
                Ok(_) => Err(invalid_docker(format!(
                    "Docker execution record path exists but cannot be read: {}",
                    path.display()
                ))),
                Err(metadata_error) => Err(SandboxError::Io(metadata_error)),
            }
        }
        Err(error) => Err(error),
    }
}

fn reconcile_docker_execution_record_value_with(
    wrapper: &Path,
    path: &Path,
    record: &DockerExecutionRecord,
) -> Result<()> {
    reconcile_docker_execution_with(wrapper, record)?;
    let Some(current) = read_docker_execution_record_for_reconciliation(path)? else {
        return Ok(());
    };
    if current != *record {
        return Err(invalid_docker(format!(
            "Docker execution record changed during reconciliation: {}",
            path.display()
        )));
    }
    remove_synced(path)
}

fn inspect_docker_image_with(wrapper: &Path, image: &OsStr) -> Result<DockerImage> {
    if image.is_empty() {
        return Err(invalid_docker(
            "Docker image reference must not be empty".to_string(),
        ));
    }
    let output = trusted_docker(wrapper)
        .args([
            OsStr::new("image"),
            OsStr::new("inspect"),
            OsStr::new("--format"),
            OsStr::new("{{.Id}}\t{{.Os}}\t{{.Architecture}}"),
        ])
        .arg(image)
        .output()
        .map_err(SandboxError::Io)?;
    if !output.status.success() {
        return Err(docker_command_error("image inspect", &output));
    }
    if output.stdout.len() > 4096 {
        return Err(invalid_docker(
            "Docker image inspection output exceeded 4096 bytes".to_string(),
        ));
    }
    let rendered = std::str::from_utf8(&output.stdout)
        .map_err(|_| invalid_docker("Docker image inspection was not UTF-8".to_string()))?
        .trim();
    let mut fields = rendered.split('\t');
    let id = fields.next().unwrap_or_default();
    let os = fields.next().unwrap_or_default();
    let architecture = fields.next().unwrap_or_default();
    if fields.next().is_some() || id.is_empty() || os.is_empty() || architecture.is_empty() {
        return Err(invalid_docker(format!(
            "Docker image inspection returned malformed identity: {rendered}"
        )));
    }
    DockerImage::new(id, DockerPlatform::from_inspection(os, architecture)?)
}

fn reconcile_docker_execution_with(
    wrapper: &Path,
    execution: &DockerExecutionRecord,
) -> Result<()> {
    execution.validate()?;
    let cid = read_cid_file(execution.cid_file())?;
    let inspected = if let Some(cid) = cid.as_deref() {
        match inspect_container(wrapper, cid)? {
            Some(container) => Some(container),
            None => inspect_container(wrapper, execution.container_name())?,
        }
    } else {
        inspect_container(wrapper, execution.container_name())?
    };

    let Some(container) = inspected else {
        remove_cid_file(execution.cid_file())?;
        return Ok(());
    };
    validate_container(&container, execution, cid.as_deref())?;

    let output = trusted_docker(wrapper)
        .args(["container", "rm", "-f", &container.id])
        .output()
        .map_err(SandboxError::Io)?;
    if !output.status.success() && inspect_container(wrapper, &container.id)?.is_some() {
        return Err(docker_command_error("container rm -f", &output));
    }
    if inspect_container(wrapper, &container.id)?.is_some()
        || inspect_container(wrapper, execution.container_name())?.is_some()
    {
        return Err(invalid_docker(format!(
            "Docker evaluator container {} remained after forced removal",
            container.id
        )));
    }
    remove_cid_file(execution.cid_file())
}

#[derive(Debug, Deserialize)]
struct ContainerInspect {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Image")]
    image: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Config")]
    config: ContainerConfig,
}

#[derive(Debug, Deserialize)]
struct ContainerConfig {
    #[serde(rename = "Labels", default)]
    labels: BTreeMap<String, String>,
}

fn inspect_container(wrapper: &Path, identity: &str) -> Result<Option<ContainerInspect>> {
    let output = trusted_docker(wrapper)
        .args(["container", "inspect", identity])
        .output()
        .map_err(SandboxError::Io)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such object") || stderr.contains("No such container") {
            return Ok(None);
        }
        return Err(docker_command_error("container inspect", &output));
    }
    if output.stdout.len() > 1024 * 1024 {
        return Err(invalid_docker(
            "Docker container inspection exceeded 1 MiB".to_string(),
        ));
    }
    let mut containers: Vec<ContainerInspect> = serde_json::from_slice(&output.stdout)
        .map_err(|error| invalid_docker(format!("invalid Docker container inspection: {error}")))?;
    if containers.len() != 1 {
        return Err(invalid_docker(format!(
            "Docker container inspection returned {} entries",
            containers.len()
        )));
    }
    Ok(containers.pop())
}

fn validate_container(
    container: &ContainerInspect,
    execution: &DockerExecutionRecord,
    cid: Option<&str>,
) -> Result<()> {
    if let Some(cid) = cid
        && container.id != cid
    {
        return Err(invalid_docker(format!(
            "Docker cidfile identity {cid} does not match inspected container {}",
            container.id
        )));
    }
    if container.image != execution.image_id() {
        return Err(invalid_docker(format!(
            "Docker evaluator image changed: expected {}, observed {}",
            execution.image_id(),
            container.image
        )));
    }
    let observed_name = container.name.strip_prefix('/').unwrap_or(&container.name);
    if observed_name != execution.container_name() {
        return Err(invalid_docker(format!(
            "Docker evaluator name changed: expected {}, observed {observed_name}",
            execution.container_name()
        )));
    }
    for (key, expected) in execution.labels() {
        let observed = container.config.labels.get(&key);
        if observed != Some(&expected) {
            return Err(invalid_docker(format!(
                "Docker evaluator label {key} changed: expected {expected:?}, observed {observed:?}"
            )));
        }
    }
    Ok(())
}

fn read_cid_file(path: &Path) -> Result<Option<String>> {
    let raw = match read_bounded_regular(path, 128, "Docker cidfile") {
        Ok(raw) => raw,
        Err(SandboxError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let cid = std::str::from_utf8(&raw)
        .map_err(|_| invalid_docker("Docker cidfile was not UTF-8".to_string()))?
        .trim();
    if cid.is_empty() {
        return Ok(None);
    }
    if cid.len() != 64 || !cid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_docker(format!(
            "Docker cidfile contains an invalid container ID: {cid}"
        )));
    }
    Ok(Some(cid.to_ascii_lowercase()))
}

fn remove_cid_file(path: &Path) -> Result<()> {
    remove_synced(path)
}

fn remove_synced(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SandboxError::Io(error)),
    }
}

fn write_execution_record(path: &Path, record: &DockerExecutionRecord) -> Result<()> {
    validate_absolute_record_path(path)?;
    if let Ok(existing) = read_docker_execution_record(path) {
        if existing == *record {
            return Ok(());
        }
        return Err(invalid_docker(format!(
            "refused to replace conflicting Docker execution record {}",
            path.display()
        )));
    } else if path.exists() {
        return Err(invalid_docker(format!(
            "refused to replace unreadable Docker execution record {}",
            path.display()
        )));
    }
    let parent = path.parent().ok_or_else(|| {
        invalid_docker("Docker execution record must have a parent directory".to_string())
    })?;
    if !parent.is_dir() {
        return Err(invalid_docker(format!(
            "Docker execution record parent does not exist: {}",
            parent.display()
        )));
    }
    let temporary = parent.join(format!(".docker-execution-{}.tmp", Uuid::new_v4()));
    let mut encoded = serde_json::to_vec(record)
        .map_err(|error| invalid_docker(format!("encode Docker execution record: {error}")))?;
    encoded.push(b'\n');
    let write_result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;

            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(SandboxError::Io)?;
        file.write_all(&encoded).map_err(SandboxError::Io)?;
        file.sync_all().map_err(SandboxError::Io)?;
        match fs::hard_link(&temporary, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = read_docker_execution_record(path)?;
                if existing != *record {
                    return Err(invalid_docker(format!(
                        "conflicting Docker execution record won creation race: {}",
                        path.display()
                    )));
                }
            }
            Err(error) => return Err(SandboxError::Io(error)),
        }
        fs::remove_file(&temporary).map_err(SandboxError::Io)?;
        sync_directory(parent)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn trusted_docker(wrapper: &Path) -> Command {
    let mut command = Command::new(wrapper);
    command.env_clear();
    command
}

fn validate_image_id(id: &str) -> Result<()> {
    let Some(digest) = id.strip_prefix("sha256:") else {
        return Err(invalid_docker(format!(
            "Docker image identity must be an immutable sha256 ID, got {id}"
        )));
    };
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_docker(format!(
            "Docker image identity is not a 64-digit sha256 ID: {id}"
        )));
    }
    Ok(())
}

fn validate_sidecar(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(invalid_docker(format!(
            "Docker evaluator sidecar must use an absolute host path: {}",
            path.display()
        )));
    }
    validate_sidecar_mount_path(path)?;
    let metadata = fs::symlink_metadata(path).map_err(SandboxError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid_docker(format!(
            "Docker evaluator sidecar must be a regular non-symlink file: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(invalid_docker(format!(
                "Docker evaluator sidecar is not executable: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn validate_sidecar_mount_path(path: &Path) -> Result<()> {
    let Some(rendered) = path.to_str() else {
        return Err(invalid_docker(
            "Docker evaluator sidecar path must be valid UTF-8".to_string(),
        ));
    };
    if rendered.contains(',') || rendered.chars().any(char::is_control) {
        return Err(invalid_docker(format!(
            "Docker evaluator sidecar path cannot be safely serialized as a mount: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_container_name(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid_docker(
            "Docker evaluator container name must not be empty".to_string(),
        ));
    };
    if name.len() > 128
        || !first.is_ascii_alphanumeric()
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        return Err(invalid_docker(format!(
            "invalid Docker evaluator container name {name:?}"
        )));
    }
    Ok(())
}

fn validate_cid_file(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(invalid_docker(format!(
            "Docker cidfile must use an absolute host path: {}",
            path.display()
        )));
    }
    let Some(parent) = path.parent() else {
        return Err(invalid_docker(
            "Docker cidfile must have a parent directory".to_string(),
        ));
    };
    if !parent.is_dir() {
        return Err(invalid_docker(format!(
            "Docker cidfile parent does not exist: {}",
            parent.display()
        )));
    }
    if path.to_str().is_none() {
        return Err(invalid_docker(
            "Docker cidfile path must be valid UTF-8".to_string(),
        ));
    }
    Ok(())
}

fn validate_absolute_record_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(invalid_docker(format!(
            "Docker execution record must use an absolute path: {}",
            path.display()
        )));
    }
    Ok(())
}

fn read_bounded_regular(path: &Path, limit: usize, kind: &str) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(SandboxError::Io)?;
    let metadata = file.metadata().map_err(SandboxError::Io)?;
    let limit_u64 = u64::try_from(limit).unwrap_or(u64::MAX);
    if !metadata.is_file() || metadata.len() > limit_u64 {
        return Err(invalid_docker(format!(
            "{kind} is not a bounded regular file: {}",
            path.display()
        )));
    }
    let mut raw = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(limit_u64.saturating_add(1))
        .read_to_end(&mut raw)
        .map_err(SandboxError::Io)?;
    if raw.len() > limit {
        return Err(invalid_docker(format!(
            "{kind} exceeded {limit} bytes: {}",
            path.display()
        )));
    }
    Ok(raw)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(SandboxError::Io)
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn validate_label_value(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 256
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'=')
    {
        return Err(invalid_docker(format!(
            "Docker evaluator {name} is not a bounded label value"
        )));
    }
    Ok(())
}

fn docker_command_error(operation: &str, output: &Output) -> SandboxError {
    invalid_docker(format!(
        "Docker {operation} failed with status {:?}: {}{}",
        output.status.code(),
        bounded_output(&output.stdout),
        bounded_output(&output.stderr)
    ))
}

fn bounded_output(bytes: &[u8]) -> String {
    const MAX: usize = 4096;
    let bytes = &bytes[..bytes.len().min(MAX)];
    String::from_utf8_lossy(bytes).trim().to_string()
}

fn invalid_docker(message: String) -> SandboxError {
    SandboxError::InvalidDockerExecution(message)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn image_identity_requires_linux_and_an_immutable_digest() {
        let id = format!("sha256:{}", "a".repeat(64));
        let image = DockerImage::new(&id, DockerPlatform::LinuxArm64).expect("image");
        assert_eq!(image.id(), id);
        assert_eq!(image.platform().as_str(), "linux/arm64");
        assert!(DockerImage::new("rust:1", DockerPlatform::LinuxArm64).is_err());
        assert!(DockerPlatform::from_inspection("darwin", "arm64").is_err());
        assert!(DockerPlatform::from_inspection("linux", "s390x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn execution_identity_rejects_untrusted_sidecar_and_unbounded_labels() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let sidecar = executable_script(&temp, "sidecar", "#!/bin/sh\nexit 0\n");
        let linked_sidecar = temp.path().join("linked-sidecar");
        symlink(&sidecar, &linked_sidecar).expect("sidecar symlink");
        let image = DockerImage::new(
            format!("sha256:{}", "a".repeat(64)),
            DockerPlatform::LinuxAmd64,
        )
        .expect("image");
        let cid = temp.path().join("container.cid");

        assert!(
            DockerExecution::new(
                image.clone(),
                &linked_sidecar,
                "deadreckon-gate-launch-1",
                &cid,
                "job-1",
                "launch-1",
                1,
                None,
            )
            .is_err()
        );
        assert!(
            DockerExecution::new(
                image.clone(),
                &sidecar,
                "deadreckon-gate-launch-1",
                &cid,
                "job\n1",
                "launch-1",
                1,
                None,
            )
            .is_err()
        );
        assert!(
            DockerExecution::new(
                image,
                &sidecar,
                "deadreckon-gate-launch-1",
                &cid,
                "job-1",
                "launch-1",
                0,
                None,
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn inspect_uses_bounded_trusted_output() {
        let temp = TempDir::new().expect("tempdir");
        let docker = executable_script(
            &temp,
            "docker-inspect",
            &format!(
                "#!/bin/sh\nprintf '%s\\tlinux\\tarm64\\n' '{}'\n",
                format!("sha256:{}", "b".repeat(64))
            ),
        );
        let image = inspect_docker_image_with(&docker, OsStr::new("rust:1")).expect("inspect");
        assert_eq!(image.platform(), DockerPlatform::LinuxArm64);
        assert_eq!(image.id(), format!("sha256:{}", "b".repeat(64)));
    }

    #[cfg(unix)]
    #[test]
    fn reconciliation_validates_labels_then_forces_removal_and_cleans_cidfile() {
        let fixture = docker_reconciliation_fixture(false);
        let record = fixture.execution.record();

        reconcile_docker_execution_with(&fixture.docker, &record).expect("reconciliation");

        assert!(!fixture.state.exists(), "container state survived");
        assert!(!fixture.execution.cid_file().exists(), "cidfile survived");
        let log = fs::read_to_string(&fixture.log).expect("Docker log");
        assert!(log.contains("container rm -f"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn reconciliation_refuses_mismatched_labels_without_removing_container_or_cidfile() {
        let fixture = docker_reconciliation_fixture(true);
        let record = fixture.execution.record();

        let error = reconcile_docker_execution_with(&fixture.docker, &record)
            .expect_err("mismatched label");

        assert!(error.to_string().contains(LAUNCH_ID_LABEL), "{error}");
        assert!(fixture.state.exists(), "container state was removed");
        assert!(fixture.execution.cid_file().exists(), "cidfile was removed");
        let log = fs::read_to_string(&fixture.log).expect("Docker log");
        assert!(!log.contains("container rm -f"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn reconciliation_refuses_mismatched_job_identity() {
        let fixture = docker_reconciliation_fixture(false);
        let mut record = fixture.execution.record();
        record.job_id = "job-2".to_string();

        let error = reconcile_docker_execution_with(&fixture.docker, &record)
            .expect_err("mismatched Job label");

        assert!(error.to_string().contains(JOB_ID_LABEL), "{error}");
        assert!(fixture.state.exists(), "container state was removed");
        assert!(fixture.execution.cid_file().exists(), "cidfile was removed");
        let log = fs::read_to_string(&fixture.log).expect("Docker log");
        assert!(!log.contains("container rm -f"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn reconciliation_cleans_stale_cidfile_only_after_container_absence_is_proven() {
        let fixture = docker_reconciliation_fixture(false);
        let record = fixture.execution.record();
        fs::remove_file(&fixture.state).expect("remove container state");

        reconcile_docker_execution_with(&fixture.docker, &record).expect("absent reconciliation");

        assert!(!fixture.execution.cid_file().exists());
        let log = fs::read_to_string(&fixture.log).expect("Docker log");
        assert!(!log.contains("container rm -f"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn reconciliation_retains_cid_when_forced_removal_does_not_make_container_absent() {
        let fixture = docker_reconciliation_fixture(false);
        let record = fixture.execution.record();
        fs::write(
            &fixture.docker,
            format!(
                "#!/bin/sh\n\
                 state='{}'\n\
                 log='{}'\n\
                 printf '%s\\n' \"$*\" >>\"$log\"\n\
                 if test \"$1 $2\" = 'container inspect'; then\n\
                   if test -f \"$state\"; then /bin/cat \"$state\"; exit 0; fi\n\
                   printf 'Error: No such object\\n' >&2; exit 1\n\
                 fi\n\
                 if test \"$1 $2 $3\" = 'container rm -f'; then exit 0; fi\n\
                 exit 90\n",
                fixture.state.display(),
                fixture.log.display()
            ),
        )
        .expect("replacement Docker script");

        let error = reconcile_docker_execution_with(&fixture.docker, &record)
            .expect_err("container remained");

        assert!(error.to_string().contains("remained"), "{error}");
        assert!(fixture.state.exists(), "container state was removed");
        assert!(fixture.execution.cid_file().exists(), "cidfile was removed");
    }

    #[cfg(unix)]
    #[test]
    fn durable_record_round_trips_and_recovers_after_sidecar_disappears() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = docker_reconciliation_fixture(false);
        let record_path = fixture
            .execution
            .cid_file()
            .parent()
            .expect("cid parent")
            .join("docker-execution.json");
        write_docker_execution_record(&record_path, &fixture.execution).expect("write record");
        write_docker_execution_record(&record_path, &fixture.execution)
            .expect("idempotent record write");

        let observed = read_docker_execution_record(&record_path).expect("read record");
        assert_eq!(observed, fixture.execution.record());
        assert_eq!(
            observed.schema_version(),
            DOCKER_EXECUTION_RECORD_SCHEMA_VERSION
        );
        assert_eq!(observed.job_id(), "job-1");
        assert_eq!(
            observed.labels().get(JOB_ID_LABEL).map(String::as_str),
            Some("job-1")
        );
        assert_eq!(
            fs::metadata(&record_path)
                .expect("record metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        fs::remove_file(fixture.execution.sidecar_host_path()).expect("remove old sidecar");
        reconcile_docker_execution_record_with(&fixture.docker, &record_path)
            .expect("record recovery");

        assert!(!fixture.state.exists(), "container state survived");
        assert!(!fixture.execution.cid_file().exists(), "cidfile survived");
        assert!(!record_path.exists(), "execution record survived");
    }

    #[cfg(unix)]
    #[test]
    fn durable_record_refuses_conflicting_identity_and_survives_failed_reconciliation() {
        let fixture = docker_reconciliation_fixture(true);
        let record_path = fixture
            .execution
            .cid_file()
            .parent()
            .expect("cid parent")
            .join("docker-execution.json");
        let original = fixture.execution.record();
        original.write_to(&record_path).expect("write record");

        let mut conflicting = original.clone();
        conflicting.job_id = "job-2".to_string();
        let error = conflicting
            .write_to(&record_path)
            .expect_err("conflicting record");
        assert!(error.to_string().contains("conflicting"), "{error}");
        assert_eq!(
            read_docker_execution_record(&record_path).expect("original record"),
            original
        );

        let error = reconcile_docker_execution_record_with(&fixture.docker, &record_path)
            .expect_err("label mismatch");
        assert!(error.to_string().contains(LAUNCH_ID_LABEL), "{error}");
        assert!(fixture.state.exists(), "container state was removed");
        assert!(fixture.execution.cid_file().exists(), "cidfile was removed");
        assert!(record_path.exists(), "execution record was removed");
    }

    #[cfg(unix)]
    #[test]
    fn malformed_or_symlinked_execution_records_are_rejected() {
        use std::os::unix::fs::symlink;

        let fixture = docker_reconciliation_fixture(false);
        let record_path = fixture
            .execution
            .cid_file()
            .parent()
            .expect("cid parent")
            .join("docker-execution.json");
        let mut encoded = serde_json::to_value(fixture.execution.record()).expect("record JSON");
        encoded["schema_version"] = serde_json::json!(99);
        fs::write(
            &record_path,
            serde_json::to_vec(&encoded).expect("encoded record"),
        )
        .expect("record");
        let error = read_docker_execution_record(&record_path).expect_err("invalid schema");
        assert!(error.to_string().contains("unsupported"), "{error}");

        fs::remove_file(&record_path).expect("remove invalid record");
        let target = record_path.with_extension("target");
        fs::write(&target, b"{}\n").expect("target");
        symlink(&target, &record_path).expect("record symlink");
        assert!(read_docker_execution_record(&record_path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn successful_exit_finalization_removes_stale_cid_and_record() {
        let fixture = docker_reconciliation_fixture(false);
        let record_path = fixture
            .execution
            .cid_file()
            .parent()
            .expect("cid parent")
            .join("successful-exit.json");
        write_docker_execution_record(&record_path, &fixture.execution).expect("write record");
        // Docker --rm has already removed the successfully exited container,
        // while Docker's cidfile remains for trusted finalization.
        fs::remove_file(&fixture.state).expect("successful container exit");

        reconcile_docker_execution_record_with(&fixture.docker, &record_path)
            .expect("successful exit finalization");

        assert!(!fixture.execution.cid_file().exists(), "cidfile survived");
        assert!(!record_path.exists(), "execution record survived");
        let log = fs::read_to_string(&fixture.log).expect("Docker log");
        assert!(!log.contains("container rm -f"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn launch_error_finalization_removes_record_when_nothing_reached_daemon() {
        let fixture = docker_reconciliation_fixture(false);
        let record_path = fixture
            .execution
            .cid_file()
            .parent()
            .expect("cid parent")
            .join("launch-error.json");
        write_docker_execution_record(&record_path, &fixture.execution).expect("write record");
        // The guarded host launch failed before Docker created either durable
        // identity. Recovery must still prove name absence before retiring the
        // pre-release execution record.
        fs::remove_file(&fixture.state).expect("no container");
        fs::remove_file(fixture.execution.cid_file()).expect("no cid");

        reconcile_docker_execution_record_with(&fixture.docker, &record_path)
            .expect("launch error finalization");

        assert!(!record_path.exists(), "execution record survived");
        let log = fs::read_to_string(&fixture.log).expect("Docker log");
        assert!(
            log.contains("container inspect deadreckon-gate-launch-1"),
            "{log}"
        );
        assert!(!log.contains("container rm -f"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn launch_error_retains_record_when_daemon_absence_is_uncertain() {
        let fixture = docker_reconciliation_fixture(false);
        let record_path = fixture
            .execution
            .cid_file()
            .parent()
            .expect("cid parent")
            .join("ambiguous-launch-error.json");
        write_docker_execution_record(&record_path, &fixture.execution).expect("write record");
        fs::remove_file(&fixture.state).expect("no visible container");
        fs::remove_file(fixture.execution.cid_file()).expect("no cid");
        fs::write(
            &fixture.docker,
            "#!/bin/sh\nprintf 'daemon unavailable\\n' >&2\nexit 73\n",
        )
        .expect("unavailable Docker script");

        let error = reconcile_docker_execution_record_with(&fixture.docker, &record_path)
            .expect_err("uncertain daemon state");

        assert!(error.to_string().contains("container inspect"), "{error}");
        assert!(
            record_path.exists(),
            "uncertain launch retired its recovery record"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_finalization_forces_verified_container_removal() {
        let fixture = docker_reconciliation_fixture(false);
        let record_path = fixture
            .execution
            .cid_file()
            .parent()
            .expect("cid parent")
            .join("cancelled.json");
        write_docker_execution_record(&record_path, &fixture.execution).expect("write record");

        reconcile_docker_execution_record_with(&fixture.docker, &record_path)
            .expect("cancel finalization");

        assert!(!fixture.state.exists(), "cancelled container survived");
        assert!(!fixture.execution.cid_file().exists(), "cidfile survived");
        assert!(!record_path.exists(), "execution record survived");
        let log = fs::read_to_string(&fixture.log).expect("Docker log");
        assert!(log.contains("container rm -f"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn stale_container_recovery_falls_back_to_name_when_cid_is_missing() {
        let fixture = docker_reconciliation_fixture(false);
        let record_path = fixture
            .execution
            .cid_file()
            .parent()
            .expect("cid parent")
            .join("stale-container.json");
        write_docker_execution_record(&record_path, &fixture.execution).expect("write record");
        fs::remove_file(fixture.execution.cid_file()).expect("lost cidfile");

        reconcile_docker_execution_record_with(&fixture.docker, &record_path)
            .expect("stale container recovery");

        assert!(!fixture.state.exists(), "stale container survived");
        assert!(!record_path.exists(), "execution record survived");
        let log = fs::read_to_string(&fixture.log).expect("Docker log");
        assert!(
            log.contains("container inspect deadreckon-gate-launch-1"),
            "{log}"
        );
        assert!(log.contains("container rm -f"), "{log}");
    }

    #[test]
    fn completed_record_reconciliation_is_idempotent_without_docker() {
        let temp = TempDir::new().expect("tempdir");
        let absent_record = temp.path().join("already-reconciled.json");

        reconcile_docker_execution_record(&absent_record).expect("first absent reconciliation");
        reconcile_docker_execution_record(&absent_record).expect("repeated absent reconciliation");

        assert!(!absent_record.exists());
    }

    #[cfg(unix)]
    #[test]
    fn idempotent_reconciliation_does_not_accept_a_dangling_record_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let record_path = temp.path().join("docker-execution.json");
        symlink(temp.path().join("missing-target"), &record_path).expect("dangling symlink");

        let error = reconcile_docker_execution_record_with(
            &temp.path().join("unused-docker"),
            &record_path,
        )
        .expect_err("dangling record symlink");

        assert!(error.is_fatal(), "{error}");
        assert!(
            fs::symlink_metadata(&record_path).is_ok(),
            "dangling record marker was removed"
        );
    }

    #[cfg(unix)]
    struct ReconciliationFixture {
        _temp: TempDir,
        docker: PathBuf,
        state: PathBuf,
        log: PathBuf,
        execution: DockerExecution,
    }

    #[cfg(unix)]
    fn docker_reconciliation_fixture(mismatched_launch: bool) -> ReconciliationFixture {
        let temp = TempDir::new().expect("tempdir");
        let sidecar = executable_script(&temp, "sidecar", "#!/bin/sh\nexit 0\n");
        let cid_file = temp.path().join("container.cid");
        let cid = "c".repeat(64);
        fs::write(&cid_file, format!("{cid}\n")).expect("cidfile");
        let state = temp.path().join("container.json");
        let log = temp.path().join("docker.log");
        let image_id = format!("sha256:{}", "d".repeat(64));
        let launch = if mismatched_launch {
            "different-launch"
        } else {
            "launch-1"
        };
        let container = serde_json::json!([{
            "Id": cid,
            "Image": image_id.clone(),
            "Name": "/deadreckon-gate-launch-1",
            "Config": {
                "Labels": {
                    (MANAGED_LABEL): MANAGED_VALUE,
                    (JOB_ID_LABEL): "job-1",
                    (LAUNCH_ID_LABEL): launch,
                    (ATTEMPT_LABEL): "1",
                    (OWNER_LAUNCH_ID_LABEL): "owner-1"
                }
            }
        }]);
        fs::write(
            &state,
            serde_json::to_vec(&container).expect("container JSON"),
        )
        .expect("container state");
        let docker = executable_script(
            &temp,
            "docker",
            &format!(
                "#!/bin/sh\n\
                 state='{}'\n\
                 log='{}'\n\
                 printf '%s\\n' \"$*\" >>\"$log\"\n\
                 if test \"$1 $2\" = 'container inspect'; then\n\
                   if test -f \"$state\"; then /bin/cat \"$state\"; exit 0; fi\n\
                   printf 'Error: No such object\\n' >&2; exit 1\n\
                 fi\n\
                 if test \"$1 $2 $3\" = 'container rm -f'; then\n\
                   /bin/rm -f \"$state\"; printf '%s\\n' \"$4\"; exit 0\n\
                 fi\n\
                 exit 90\n",
                state.display(),
                log.display()
            ),
        );
        let image = DockerImage::new(image_id, DockerPlatform::LinuxArm64).expect("Docker image");
        let execution = DockerExecution::new(
            image,
            sidecar,
            "deadreckon-gate-launch-1",
            cid_file,
            "job-1",
            "launch-1",
            1,
            Some("owner-1".to_string()),
        )
        .expect("Docker execution");
        ReconciliationFixture {
            _temp: temp,
            docker,
            state,
            log,
            execution,
        }
    }

    #[cfg(unix)]
    fn executable_script(temp: &TempDir, name: &str, content: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let path = temp.path().join(name);
        let mut file = fs::File::create(&path).expect("script");
        file.write_all(content.as_bytes()).expect("script body");
        let mut permissions = file.metadata().expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("permissions");
        path
    }
}
