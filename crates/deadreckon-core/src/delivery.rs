//! Authenticated Git delivery authority for durable Jobs.
//!
//! The intent is sealed before Git is allowed to mutate the operator's
//! checkout. The applied receipt is sealed only after the resulting revision
//! has been independently re-proved. Both artifacts live in controller-owned
//! Job state and use the protected completion key for their Job run.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use deadreckon_protocol::{
    AppliedGitDeliveryReceipt, GitDeliveryIntent, GitDeliveryRepositoryIdentity,
};
use fs2::FileExt;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::gate::read_gate_key;
use crate::git::run_git;
use crate::paths::DeadreckonPaths;

const DELIVERY_INTENT_MAGIC: &[u8] = b"deadreckon.git-delivery-intent.v1\0";
const APPLIED_DELIVERY_MAGIC: &[u8] = b"deadreckon.applied-git-delivery.v1\0";

/// One stable, never-unlinked per-Job lock for finish, undo, abandon, and
/// cleanup. Dropping the guard releases the OS lock but intentionally leaves
/// the file in place; unlinking a lock file permits two processes to hold
/// locks on different inodes at the same pathname.
#[derive(Debug)]
pub struct JobOperationLock {
    file: File,
    job_id: String,
}

impl JobOperationLock {
    pub fn job_id(&self) -> &str {
        &self.job_id
    }
}

impl Drop for JobOperationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// A delivery intent validated and hashed from one immutable byte snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedGitDeliveryIntent {
    pub intent: GitDeliveryIntent,
    pub sha256: String,
}

/// An applied-delivery receipt validated and hashed from one immutable byte
/// snapshot. Its bound intent is also validated from one byte snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAppliedGitDeliveryReceipt {
    pub receipt: AppliedGitDeliveryReceipt,
    pub sha256: String,
    pub intent_sha256: String,
}

pub fn acquire_job_operation_lock(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<JobOperationLock> {
    require_nonempty(job_id, "Job ID")?;
    let job_dir = paths.job_dir(job_id);
    fs::create_dir_all(&job_dir).with_path(&job_dir)?;
    let path = paths.job_operation_lock(job_id);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_path(&path)?;
    let opened = file.metadata().with_path(&path)?;
    let metadata = fs::symlink_metadata(&path).with_path(&path)?;
    if !opened.is_file()
        || !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || !same_open_file(&opened, &metadata)
    {
        return Err(delivery_error(format!(
            "Job operation lock is not a regular file: {}",
            path.display()
        )));
    }
    match file.try_lock_exclusive() {
        Ok(()) => Ok(JobOperationLock {
            file,
            job_id: job_id.to_string(),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
            Err(DeadreckonError::InvalidInput(format!(
                "Job {job_id} already has an active finish, undo, abandon, or cleanup operation"
            )))
        }
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

/// Controller-observed identity and current attached revision of one Git
/// delivery target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDeliveryTarget {
    pub repository: GitDeliveryRepositoryIdentity,
    pub target_ref: String,
    pub head_revision: String,
}

impl GitDeliveryTarget {
    /// Inspect an exact worktree root. Aliases, subdirectories, detached HEADs,
    /// and a different linked worktree are deliberately distinct/refused.
    pub fn inspect(path: &Path) -> Result<Self> {
        let requested = fs::canonicalize(path).with_path(path)?;
        let worktree_root =
            PathBuf::from(git_stdout(&requested, &["rev-parse", "--show-toplevel"])?);
        let worktree_root = fs::canonicalize(&worktree_root).with_path(&worktree_root)?;
        if requested != worktree_root {
            return Err(delivery_error(format!(
                "delivery target {} is not the exact Git worktree root {}",
                requested.display(),
                worktree_root.display()
            )));
        }

        let git_common_dir = git_stdout(&worktree_root, &["rev-parse", "--git-common-dir"])?;
        let git_common_dir = PathBuf::from(git_common_dir);
        let git_common_dir = if git_common_dir.is_absolute() {
            git_common_dir
        } else {
            worktree_root.join(git_common_dir)
        };
        let git_common_dir = fs::canonicalize(&git_common_dir).with_path(&git_common_dir)?;

        let target_ref = git_stdout(&worktree_root, &["symbolic-ref", "-q", "HEAD"])
            .map_err(|_| delivery_error("delivery target has a detached HEAD"))?;
        if !target_ref.starts_with("refs/heads/") {
            return Err(delivery_error(format!(
                "delivery target ref {target_ref:?} is not a full local branch ref"
            )));
        }
        let head_revision =
            git_stdout(&worktree_root, &["rev-parse", "--verify", "HEAD^{commit}"])?;

        Ok(Self {
            repository: GitDeliveryRepositoryIdentity {
                worktree_root,
                git_common_dir,
            },
            target_ref,
            head_revision,
        })
    }
}

pub fn seal_git_delivery_intent(
    paths: &DeadreckonPaths,
    intent: &GitDeliveryIntent,
) -> Result<GitDeliveryIntent> {
    validate_intent_shape(intent)?;
    if !intent.signature.is_empty() {
        return Err(delivery_error(
            "a delivery intent must be submitted for sealing without a signature",
        ));
    }
    let mut sealed = intent.clone();
    let key = read_gate_key(paths, sealed.run_id.as_ref())?;
    sealed.signature = sign_intent(&sealed, &key)?;

    let path = paths.job_delivery_intent(sealed.job_id.as_ref());
    match persist_new_json(&path, &sealed)? {
        PersistNew::Created => Ok(sealed),
        PersistNew::AlreadyExists => {
            let existing = validate_git_delivery_intent_snapshot(paths, sealed.job_id.as_ref())?;
            if existing.intent == sealed {
                Ok(existing.intent)
            } else {
                Err(delivery_error(format!(
                    "signed delivery intent already exists for {} with different authority",
                    sealed.job_id
                )))
            }
        }
    }
}

pub fn validate_git_delivery_intent(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<GitDeliveryIntent> {
    Ok(validate_git_delivery_intent_snapshot(paths, job_id)?.intent)
}

pub fn validate_git_delivery_intent_snapshot(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<ValidatedGitDeliveryIntent> {
    let path = paths.job_delivery_intent(job_id);
    let bytes = read_regular_artifact(&path)?;
    let sha256 = sha256_bytes(&bytes);
    let intent: GitDeliveryIntent = serde_json::from_slice(&bytes).with_json_path(&path)?;
    if intent.job_id.as_ref() != job_id {
        return Err(delivery_error(format!(
            "delivery intent at {} names Job {} instead of {job_id}",
            path.display(),
            intent.job_id
        )));
    }
    validate_intent_shape(&intent)?;
    let key = read_gate_key(paths, intent.run_id.as_ref())?;
    verify_intent_signature(&intent, &key)?;
    Ok(ValidatedGitDeliveryIntent { intent, sha256 })
}

pub fn seal_applied_git_delivery_receipt(
    paths: &DeadreckonPaths,
    receipt: &AppliedGitDeliveryReceipt,
) -> Result<AppliedGitDeliveryReceipt> {
    validate_applied_shape(receipt)?;
    if !receipt.signature.is_empty() {
        return Err(delivery_error(
            "an applied delivery receipt must be submitted for sealing without a signature",
        ));
    }
    let intent = validate_git_delivery_intent_snapshot(paths, receipt.job_id.as_ref())?;
    validate_applied_binding(receipt, &intent.intent, &intent.sha256)?;

    let mut sealed = receipt.clone();
    let key = read_gate_key(paths, sealed.run_id.as_ref())?;
    sealed.signature = sign_applied(&sealed, &key)?;

    let path = paths.job_applied_delivery_receipt(sealed.job_id.as_ref());
    match persist_new_json(&path, &sealed)? {
        PersistNew::Created => Ok(sealed),
        PersistNew::AlreadyExists => {
            let existing =
                validate_applied_git_delivery_receipt_snapshot(paths, sealed.job_id.as_ref())?;
            if existing.receipt == sealed {
                Ok(existing.receipt)
            } else {
                Err(delivery_error(format!(
                    "signed applied delivery receipt already exists for {} with a different result",
                    sealed.job_id
                )))
            }
        }
    }
}

pub fn validate_applied_git_delivery_receipt(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<AppliedGitDeliveryReceipt> {
    Ok(validate_applied_git_delivery_receipt_snapshot(paths, job_id)?.receipt)
}

pub fn validate_applied_git_delivery_receipt_snapshot(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<ValidatedAppliedGitDeliveryReceipt> {
    let path = paths.job_applied_delivery_receipt(job_id);
    let bytes = read_regular_artifact(&path)?;
    let sha256 = sha256_bytes(&bytes);
    let receipt: AppliedGitDeliveryReceipt =
        serde_json::from_slice(&bytes).with_json_path(&path)?;
    if receipt.job_id.as_ref() != job_id {
        return Err(delivery_error(format!(
            "applied delivery receipt at {} names Job {} instead of {job_id}",
            path.display(),
            receipt.job_id
        )));
    }
    validate_applied_shape(&receipt)?;
    let key = read_gate_key(paths, receipt.run_id.as_ref())?;
    verify_applied_signature(&receipt, &key)?;
    let intent = validate_git_delivery_intent_snapshot(paths, job_id)?;
    validate_applied_binding(&receipt, &intent.intent, &intent.sha256)?;
    Ok(ValidatedAppliedGitDeliveryReceipt {
        receipt,
        sha256,
        intent_sha256: intent.sha256,
    })
}

fn validate_intent_shape(intent: &GitDeliveryIntent) -> Result<()> {
    require_nonempty(intent.job_id.as_ref(), "Job ID")?;
    require_nonempty(intent.run_id.as_ref(), "run ID")?;
    require_digest(&intent.completion_receipt_sha256, "completion receipt")?;
    validate_repository(&intent.repository)?;
    validate_target_ref(&intent.target_ref)?;
    require_revision(&intent.pre_revision, "pre-delivery revision")?;
    require_revision(&intent.signed_source_revision, "signed source revision")?;
    require_revision(&intent.signed_result_revision, "signed result revision")?;
    require_digest(&intent.effective_policy_sha256, "effective policy")
}

fn validate_applied_shape(receipt: &AppliedGitDeliveryReceipt) -> Result<()> {
    require_nonempty(receipt.job_id.as_ref(), "Job ID")?;
    require_nonempty(receipt.run_id.as_ref(), "run ID")?;
    require_digest(&receipt.delivery_intent_sha256, "delivery intent")?;
    require_digest(&receipt.completion_receipt_sha256, "completion receipt")?;
    validate_repository(&receipt.repository)?;
    validate_target_ref(&receipt.target_ref)?;
    require_revision(&receipt.pre_revision, "pre-delivery revision")?;
    require_revision(&receipt.applied_revision, "applied revision")?;
    require_revision(&receipt.signed_source_revision, "signed source revision")?;
    require_revision(&receipt.signed_result_revision, "signed result revision")?;
    require_digest(&receipt.effective_policy_sha256, "effective policy")?;
    if receipt.pre_revision == receipt.applied_revision {
        return Err(delivery_error(
            "applied delivery receipt cannot describe a no-op delivery",
        ));
    }
    Ok(())
}

fn validate_applied_binding(
    receipt: &AppliedGitDeliveryReceipt,
    intent: &GitDeliveryIntent,
    intent_sha256: &str,
) -> Result<()> {
    if receipt.delivery_intent_sha256 != intent_sha256
        || receipt.job_id != intent.job_id
        || receipt.run_id != intent.run_id
        || receipt.completion_receipt_sha256 != intent.completion_receipt_sha256
        || receipt.repository != intent.repository
        || receipt.target_ref != intent.target_ref
        || receipt.pre_revision != intent.pre_revision
        || receipt.signed_source_revision != intent.signed_source_revision
        || receipt.signed_result_revision != intent.signed_result_revision
        || receipt.effective_policy_sha256 != intent.effective_policy_sha256
        || receipt.strategy != intent.strategy
    {
        return Err(delivery_error(
            "applied delivery receipt does not exactly match its signed delivery intent",
        ));
    }
    Ok(())
}

fn validate_repository(repository: &GitDeliveryRepositoryIdentity) -> Result<()> {
    for (label, path) in [
        ("worktree root", &repository.worktree_root),
        ("Git common directory", &repository.git_common_dir),
    ] {
        if !path.is_absolute() {
            return Err(delivery_error(format!(
                "delivery repository {label} {} is not absolute",
                path.display()
            )));
        }
        let canonical = fs::canonicalize(path).with_path(path)?;
        if canonical.as_path() != path {
            return Err(delivery_error(format!(
                "delivery repository {label} {} is not its canonical path {}",
                path.display(),
                canonical.display()
            )));
        }
    }
    Ok(())
}

fn validate_target_ref(target_ref: &str) -> Result<()> {
    if target_ref.starts_with("refs/heads/") && target_ref.len() > "refs/heads/".len() {
        Ok(())
    } else {
        Err(delivery_error(format!(
            "delivery target ref {target_ref:?} is not a full local branch ref"
        )))
    }
}

fn require_nonempty(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(delivery_error(format!("{label} is empty")))
    } else {
        Ok(())
    }
}

fn require_digest(value: &str, label: &str) -> Result<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(delivery_error(format!("{label} digest is not SHA-256")));
    };
    if hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(delivery_error(format!("{label} digest is not SHA-256")))
    }
}

fn require_revision(value: &str, label: &str) -> Result<()> {
    if matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(delivery_error(format!(
            "{label} is not a full Git object ID"
        )))
    }
}

fn sign_intent(intent: &GitDeliveryIntent, key: &[u8]) -> Result<String> {
    sign_bytes(&canonical_intent_bytes(intent)?, key)
}

fn verify_intent_signature(intent: &GitDeliveryIntent, key: &[u8]) -> Result<()> {
    verify_bytes(
        &canonical_intent_bytes(intent)?,
        &intent.signature,
        key,
        "delivery intent",
    )
}

fn sign_applied(receipt: &AppliedGitDeliveryReceipt, key: &[u8]) -> Result<String> {
    sign_bytes(&canonical_applied_bytes(receipt)?, key)
}

fn verify_applied_signature(receipt: &AppliedGitDeliveryReceipt, key: &[u8]) -> Result<()> {
    verify_bytes(
        &canonical_applied_bytes(receipt)?,
        &receipt.signature,
        key,
        "applied delivery receipt",
    )
}

fn sign_bytes(bytes: &[u8], key: &[u8]) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| delivery_error("HMAC-SHA-256 refused the protected delivery key"))?;
    mac.update(bytes);
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

fn verify_bytes(bytes: &[u8], signature: &str, key: &[u8], label: &str) -> Result<()> {
    let signature = hex_decode(signature)
        .map_err(|detail| delivery_error(format!("{label} signature is invalid: {detail}")))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| delivery_error("HMAC-SHA-256 refused the protected delivery key"))?;
    mac.update(bytes);
    mac.verify_slice(&signature)
        .map_err(|_| delivery_error(format!("{label} signature verification failed")))
}

fn canonical_intent_bytes(intent: &GitDeliveryIntent) -> Result<Vec<u8>> {
    let mut unsigned = intent.clone();
    unsigned.signature.clear();
    canonical_bytes(
        DELIVERY_INTENT_MAGIC,
        &unsigned,
        Path::new("delivery/intent.json"),
    )
}

fn canonical_applied_bytes(receipt: &AppliedGitDeliveryReceipt) -> Result<Vec<u8>> {
    let mut unsigned = receipt.clone();
    unsigned.signature.clear();
    canonical_bytes(
        APPLIED_DELIVERY_MAGIC,
        &unsigned,
        Path::new("delivery/applied-receipt.json"),
    )
}

fn canonical_bytes<T: serde::Serialize>(magic: &[u8], value: &T, path: &Path) -> Result<Vec<u8>> {
    let encoded = serde_json::to_vec(value).map_err(|source| DeadreckonError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = magic.to_vec();
    let len = u64::try_from(encoded.len())
        .map_err(|_| delivery_error("delivery authority is too large to sign"))?;
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistNew {
    Created,
    AlreadyExists,
}

fn persist_new_json(path: &Path, value: &impl serde::Serialize) -> Result<PersistNew> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let mut temp = NamedTempFile::new_in(parent).with_path(parent)?;
    serde_json::to_writer_pretty(&mut temp, value).with_json_path(path)?;
    temp.write_all(b"\n").with_path(path)?;
    temp.as_file_mut().sync_all().with_path(path)?;
    match temp.persist_noclobber(path) {
        Ok(file) => {
            file.sync_all().with_path(path)?;
            sync_parent(parent)?;
            Ok(PersistNew::Created)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(PersistNew::AlreadyExists)
        }
        Err(error) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source: error.error,
        }),
    }
}

fn read_regular_artifact(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path).with_path(path)?;
    let opened = file.metadata().with_path(path)?;
    let before = fs::symlink_metadata(path).with_path(path)?;
    if !opened.is_file() || !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(delivery_error(format!(
            "signed delivery artifact is not a regular file: {}",
            path.display()
        )));
    }
    if !same_open_file(&opened, &before) {
        return Err(delivery_error(format!(
            "signed delivery artifact changed while opening: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).with_path(path)?;
    let after = fs::symlink_metadata(path).with_path(path)?;
    if !after.file_type().is_file()
        || after.file_type().is_symlink()
        || !same_open_file(&opened, &after)
        || opened.len() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
    {
        return Err(delivery_error(format!(
            "signed delivery artifact changed while reading: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn same_open_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_open_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.created().ok() == right.created().ok()
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<()> {
    File::open(parent)
        .with_path(parent)?
        .sync_all()
        .with_path(parent)
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<()> {
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_encode(&Sha256::digest(bytes)))
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = run_git(cwd, args)?;
    if !output.status.success() {
        return Err(delivery_error(format!(
            "git {} failed in {}: {}",
            args.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Err(delivery_error(format!(
            "git {} returned an empty value in {}",
            args.join(" "),
            cwd.display()
        )));
    }
    Ok(value)
}

fn delivery_error(detail: impl Into<String>) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "verified Git delivery authority is invalid: {}",
        detail.into()
    ))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn hex_decode(value: &str) -> std::result::Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("odd length".to_string());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect()
}

fn hex_nibble(value: u8) -> std::result::Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(format!("invalid hex byte {value:#x}")),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use deadreckon_protocol::{
        AppliedGitDeliveryReceipt, GitDeliveryIntent, GitDeliveryRepositoryIdentity,
        GitDeliveryStrategy, JobId, JobSchemaVersion, RunId,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::flight::{sha256_file, sha256_text};
    use crate::gate::write_gate_key;
    use crate::state::atomic_write_json;

    fn intent_fixture(temp: &TempDir) -> (DeadreckonPaths, String, GitDeliveryIntent) {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "abababababababababababababababab".to_string();
        write_gate_key(&paths, &job_id, &[17_u8; 32]).expect("protected key");
        let worktree = temp.path().join("worktree");
        let common = temp.path().join("git-common");
        fs::create_dir_all(&worktree).expect("worktree");
        fs::create_dir_all(&common).expect("common dir");
        let intent = GitDeliveryIntent {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(job_id.clone()),
            run_id: RunId(job_id.clone()),
            prepared_at: Utc::now(),
            completion_receipt_sha256: sha256_text("completion"),
            repository: GitDeliveryRepositoryIdentity {
                worktree_root: fs::canonicalize(worktree).expect("canonical worktree"),
                git_common_dir: fs::canonicalize(common).expect("canonical common dir"),
            },
            target_ref: "refs/heads/main".to_string(),
            pre_revision: "1".repeat(40),
            signed_source_revision: "2".repeat(40),
            signed_result_revision: "3".repeat(40),
            effective_policy_sha256: sha256_text("policy"),
            strategy: GitDeliveryStrategy::Squash,
            signature: String::new(),
        };
        (paths, job_id, intent)
    }

    #[test]
    fn delivery_authority_is_signed_immutable_and_idempotent() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job_id, intent) = intent_fixture(&temp);
        let sealed_intent = seal_git_delivery_intent(&paths, &intent).expect("seal intent");
        assert_eq!(
            seal_git_delivery_intent(&paths, &intent).expect("repeat intent"),
            sealed_intent
        );
        let applied = AppliedGitDeliveryReceipt {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: sealed_intent.job_id.clone(),
            run_id: sealed_intent.run_id.clone(),
            issued_at: Utc::now(),
            delivery_intent_sha256: sha256_file(&paths.job_delivery_intent(&job_id))
                .expect("intent digest"),
            completion_receipt_sha256: sealed_intent.completion_receipt_sha256.clone(),
            repository: sealed_intent.repository.clone(),
            target_ref: sealed_intent.target_ref.clone(),
            pre_revision: sealed_intent.pre_revision.clone(),
            applied_revision: "4".repeat(40),
            signed_source_revision: sealed_intent.signed_source_revision.clone(),
            signed_result_revision: sealed_intent.signed_result_revision.clone(),
            effective_policy_sha256: sealed_intent.effective_policy_sha256.clone(),
            strategy: sealed_intent.strategy,
            signature: String::new(),
        };
        let sealed_applied =
            seal_applied_git_delivery_receipt(&paths, &applied).expect("seal applied");
        assert_eq!(
            seal_applied_git_delivery_receipt(&paths, &applied).expect("repeat applied"),
            sealed_applied
        );
        assert_eq!(
            validate_applied_git_delivery_receipt(&paths, &job_id).expect("validate applied"),
            sealed_applied
        );
        let snapshot = validate_applied_git_delivery_receipt_snapshot(&paths, &job_id)
            .expect("validated applied snapshot");
        assert_eq!(
            snapshot.sha256,
            sha256_file(&paths.job_applied_delivery_receipt(&job_id)).expect("applied digest")
        );
        assert_eq!(
            snapshot.intent_sha256,
            sealed_applied.delivery_intent_sha256
        );
    }

    #[test]
    fn job_operation_lock_is_exclusive_and_never_unlinked() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        let first = acquire_job_operation_lock(&paths, job_id).expect("first operation lock");
        let lock_path = paths.job_operation_lock(job_id);
        assert!(
            lock_path.is_file(),
            "stable lock path must exist while held"
        );
        let initial_lock_file = fs::symlink_metadata(&lock_path).expect("initial lock metadata");

        let error = std::thread::scope(|scope| {
            scope
                .spawn(|| acquire_job_operation_lock(&paths, job_id))
                .join()
                .expect("lock contender")
                .expect_err("concurrent operation must be refused")
        });
        assert!(error.to_string().contains("active finish"), "{error}");

        drop(first);
        assert!(
            lock_path.is_file(),
            "unlock must retain the same lock inode at its stable path"
        );
        drop(acquire_job_operation_lock(&paths, job_id).expect("reacquire stable lock"));
        assert!(lock_path.is_file(), "repeated unlock must not unlink lock");
        assert!(
            same_open_file(
                &initial_lock_file,
                &fs::symlink_metadata(&lock_path).expect("retained lock metadata")
            ),
            "operation lock path must retain the same file identity"
        );
    }

    #[test]
    fn concurrent_identical_intent_seals_create_one_immutable_artifact() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job_id, intent) = intent_fixture(&temp);
        let barrier = std::sync::Barrier::new(4);
        let sealed = std::thread::scope(|scope| {
            let handles = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        seal_git_delivery_intent(&paths, &intent)
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("seal thread").expect("sealed intent"))
                .collect::<Vec<_>>()
        });
        assert!(sealed.iter().all(|candidate| candidate == &sealed[0]));
        assert_eq!(
            validate_git_delivery_intent_snapshot(&paths, &job_id)
                .expect("single immutable intent")
                .intent,
            sealed[0]
        );
    }

    #[test]
    fn tampered_delivery_intent_signature_fails_closed() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job_id, intent) = intent_fixture(&temp);
        seal_git_delivery_intent(&paths, &intent).expect("seal intent");
        let path = paths.job_delivery_intent(&job_id);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("intent bytes")).expect("intent json");
        value["target_ref"] = serde_json::json!("refs/heads/redirected");
        atomic_write_json(&path, &value).expect("tampered intent");

        let error = validate_git_delivery_intent(&paths, &job_id)
            .expect_err("tampered intent must fail closed");
        assert!(
            error.to_string().contains("signature verification failed"),
            "{error}"
        );
    }

    #[test]
    fn applied_delivery_receipt_refuses_no_op_revision() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job_id, intent) = intent_fixture(&temp);
        let sealed_intent = seal_git_delivery_intent(&paths, &intent).expect("seal intent");
        let error = seal_applied_git_delivery_receipt(
            &paths,
            &AppliedGitDeliveryReceipt {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: sealed_intent.job_id.clone(),
                run_id: sealed_intent.run_id.clone(),
                issued_at: Utc::now(),
                delivery_intent_sha256: sha256_file(&paths.job_delivery_intent(&job_id))
                    .expect("intent digest"),
                completion_receipt_sha256: sealed_intent.completion_receipt_sha256.clone(),
                repository: sealed_intent.repository.clone(),
                target_ref: sealed_intent.target_ref.clone(),
                pre_revision: sealed_intent.pre_revision.clone(),
                applied_revision: sealed_intent.pre_revision.clone(),
                signed_source_revision: sealed_intent.signed_source_revision.clone(),
                signed_result_revision: sealed_intent.signed_result_revision.clone(),
                effective_policy_sha256: sealed_intent.effective_policy_sha256.clone(),
                strategy: sealed_intent.strategy,
                signature: String::new(),
            },
        )
        .expect_err("no-op receipt must fail closed");
        assert!(error.to_string().contains("no-op delivery"), "{error}");
    }
}
