use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempDir};
use thiserror::Error;

const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum ArtifactStoreError {
    #[error("artifact I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("artifact `{0}` has an invalid digest")]
    InvalidDigest(String),
    #[error("artifact `{path}` exceeds the configured limit of {limit} bytes")]
    SizeLimit { path: String, limit: u64 },
    #[error("artifact blob `{digest}` is missing at {path}")]
    Missing { digest: String, path: String },
    #[error(
        "artifact blob `{digest}` is corrupt: expected {expected_size} bytes and `{digest}`, found {actual_size} bytes and `{actual_digest}`"
    )]
    Corrupt {
        digest: String,
        expected_size: u64,
        actual_size: u64,
        actual_digest: String,
    },
    #[error("artifact export target already exists: {0}")]
    ExportExists(String),
    #[error("artifact path is not a regular file: {0}")]
    NotRegularFile(String),
    #[error("artifact temporary-file persistence failed: {0}")]
    Persist(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactBlob {
    pub digest: String,
    pub size_bytes: u64,
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredArtifactBlob {
    pub digest: String,
    pub size_bytes: u64,
    pub modified_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactVerification {
    pub digest: String,
    pub size_bytes: u64,
    pub path: String,
    pub valid: bool,
}

pub trait ArtifactStore: Send + Sync {
    fn root(&self) -> &Path;

    fn ingest(&self, source: &Path, max_bytes: u64) -> Result<ArtifactBlob, ArtifactStoreError>;

    fn verify(
        &self,
        digest: &str,
        expected_size: u64,
    ) -> Result<ArtifactVerification, ArtifactStoreError>;

    fn export(
        &self,
        digest: &str,
        expected_size: u64,
        destination: &Path,
        overwrite: bool,
    ) -> Result<(), ArtifactStoreError>;
}

#[derive(Debug, Clone)]
pub struct LocalArtifactStore {
    root: PathBuf,
    _temporary_root: Option<Arc<TempDir>>,
}

impl LocalArtifactStore {
    pub fn open(root: PathBuf) -> Result<Self, ArtifactStoreError> {
        prepare_root(&root)?;
        Ok(Self {
            root,
            _temporary_root: None,
        })
    }

    pub(crate) fn temporary() -> Result<Self, ArtifactStoreError> {
        let temporary_root = Arc::new(tempfile::tempdir()?);
        let root = temporary_root.path().join("artifacts");
        prepare_root(&root)?;
        Ok(Self {
            root,
            _temporary_root: Some(temporary_root),
        })
    }

    fn blob_path(&self, digest: &str) -> Result<PathBuf, ArtifactStoreError> {
        let hex = validate_digest(digest)?;
        Ok(self.root.join("sha256").join(&hex[..2]).join(hex))
    }

    pub(crate) fn lock_exclusive(&self) -> Result<File, ArtifactStoreError> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(".lock"))?;
        FileExt::lock_exclusive(&lock)?;
        Ok(lock)
    }

    pub(crate) fn stored_blobs(&self) -> Result<Vec<StoredArtifactBlob>, ArtifactStoreError> {
        let mut blobs = Vec::new();
        for prefix in std::fs::read_dir(self.root.join("sha256"))? {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(prefix.path())? {
                let entry = entry?;
                let metadata = entry.metadata()?;
                if !metadata.is_file() {
                    continue;
                }
                let Some(hex) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                    continue;
                };
                let digest = format!("sha256:{hex}");
                if validate_digest(&digest).is_err() {
                    continue;
                }
                blobs.push(StoredArtifactBlob {
                    digest,
                    size_bytes: metadata.len(),
                    modified_at: metadata.modified()?,
                });
            }
        }
        blobs.sort_by(|left, right| left.digest.cmp(&right.digest));
        Ok(blobs)
    }

    pub(crate) fn stale_temporary_files(
        &self,
        before: SystemTime,
    ) -> Result<Vec<(PathBuf, u64)>, ArtifactStoreError> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(self.root.join("tmp"))? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() && metadata.modified()? < before {
                files.push((entry.path(), metadata.len()));
            }
        }
        files.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(files)
    }

    pub(crate) fn quarantined(&self) -> Result<Vec<(String, PathBuf)>, ArtifactStoreError> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(self.root.join("trash"))? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(hex) = name.to_str() else {
                continue;
            };
            let digest = format!("sha256:{hex}");
            validate_digest(&digest)?;
            entries.push((digest, entry.path()));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(entries)
    }

    pub(crate) fn stage_remove(&self, digest: &str) -> Result<Option<PathBuf>, ArtifactStoreError> {
        let source = self.blob_path(digest)?;
        if !source.exists() {
            return Ok(None);
        }
        let hex = validate_digest(digest)?;
        let destination = self.root.join("trash").join(hex);
        if destination.exists() {
            return Err(ArtifactStoreError::Persist(format!(
                "artifact quarantine target already exists: {}",
                destination.display()
            )));
        }
        let mut permissions = std::fs::metadata(&source)?.permissions();
        make_owner_writable(&mut permissions);
        std::fs::set_permissions(&source, permissions)?;
        std::fs::rename(&source, &destination)?;
        sync_directory(source.parent().ok_or_else(|| {
            ArtifactStoreError::Persist(format!("no parent for {}", source.display()))
        })?)?;
        sync_directory(destination.parent().ok_or_else(|| {
            ArtifactStoreError::Persist(format!("no parent for {}", destination.display()))
        })?)?;
        Ok(Some(destination))
    }

    pub(crate) fn restore_staged(
        &self,
        digest: &str,
        staged: &Path,
    ) -> Result<(), ArtifactStoreError> {
        if !staged.exists() {
            return Ok(());
        }
        let destination = self.blob_path(digest)?;
        let parent = destination.parent().ok_or_else(|| {
            ArtifactStoreError::Persist(format!("no parent for {}", destination.display()))
        })?;
        std::fs::create_dir_all(parent)?;
        set_private_directory(parent)?;
        if destination.exists() {
            std::fs::remove_file(staged)?;
        } else {
            std::fs::rename(staged, &destination)?;
            set_blob_read_only(&destination)?;
            sync_directory(parent)?;
        }
        Ok(())
    }

    pub(crate) fn finish_staged(&self, staged: &Path) -> Result<(), ArtifactStoreError> {
        if staged.exists() {
            std::fs::remove_file(staged)?;
            if let Some(parent) = staged.parent() {
                sync_directory(parent)?;
            }
        }
        Ok(())
    }
}

impl ArtifactStore for LocalArtifactStore {
    fn root(&self) -> &Path {
        &self.root
    }

    fn ingest(&self, source: &Path, max_bytes: u64) -> Result<ArtifactBlob, ArtifactStoreError> {
        let source_metadata = std::fs::symlink_metadata(source)?;
        if !source_metadata.file_type().is_file() {
            return Err(ArtifactStoreError::NotRegularFile(
                source.display().to_string(),
            ));
        }
        if source_metadata.len() > max_bytes {
            return Err(ArtifactStoreError::SizeLimit {
                path: source.display().to_string(),
                limit: max_bytes,
            });
        }

        let temporary_directory = self.root.join("tmp");
        let mut temporary = NamedTempFile::new_in(&temporary_directory)?;
        let mut input = File::open(source)?;
        let mut hasher = Sha256::new();
        let mut size_bytes = 0_u64;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            size_bytes = size_bytes
                .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
                .ok_or_else(|| ArtifactStoreError::SizeLimit {
                    path: source.display().to_string(),
                    limit: max_bytes,
                })?;
            if size_bytes > max_bytes {
                return Err(ArtifactStoreError::SizeLimit {
                    path: source.display().to_string(),
                    limit: max_bytes,
                });
            }
            hasher.update(&buffer[..read]);
            temporary.write_all(&buffer[..read])?;
        }
        temporary.as_file_mut().sync_all()?;

        let digest = format!("sha256:{}", hex::encode(hasher.finalize()));
        let destination = self.blob_path(&digest)?;
        let parent = destination.parent().ok_or_else(|| {
            ArtifactStoreError::Persist(format!("no parent for {}", destination.display()))
        })?;
        std::fs::create_dir_all(parent)?;
        set_private_directory(parent)?;

        if destination.exists() {
            self.verify(&digest, size_bytes)?;
        } else {
            match temporary.persist_noclobber(&destination) {
                Ok(file) => {
                    file.sync_all()?;
                    set_blob_read_only(&destination)?;
                    sync_directory(parent)?;
                }
                Err(error) if destination.exists() => {
                    drop(error.file);
                    self.verify(&digest, size_bytes)?;
                }
                Err(error) => {
                    return Err(ArtifactStoreError::Persist(error.error.to_string()));
                }
            }
        }

        let relative_path = destination
            .strip_prefix(&self.root)
            .map_err(|error| ArtifactStoreError::Persist(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        Ok(ArtifactBlob {
            digest,
            size_bytes,
            relative_path,
        })
    }

    fn verify(
        &self,
        digest: &str,
        expected_size: u64,
    ) -> Result<ArtifactVerification, ArtifactStoreError> {
        let path = self.blob_path(digest)?;
        if !path.exists() {
            return Err(ArtifactStoreError::Missing {
                digest: digest.to_owned(),
                path: path.display().to_string(),
            });
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() {
            return Err(ArtifactStoreError::NotRegularFile(
                path.display().to_string(),
            ));
        }
        let mut input = File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut actual_size = 0_u64;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            actual_size = actual_size.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            hasher.update(&buffer[..read]);
        }
        let actual_digest = format!("sha256:{}", hex::encode(hasher.finalize()));
        if actual_size != expected_size || actual_digest != digest {
            return Err(ArtifactStoreError::Corrupt {
                digest: digest.to_owned(),
                expected_size,
                actual_size,
                actual_digest,
            });
        }
        Ok(ArtifactVerification {
            digest: digest.to_owned(),
            size_bytes: actual_size,
            path: path.display().to_string(),
            valid: true,
        })
    }

    fn export(
        &self,
        digest: &str,
        expected_size: u64,
        destination: &Path,
        overwrite: bool,
    ) -> Result<(), ArtifactStoreError> {
        let source = self.blob_path(digest)?;
        self.verify(digest, expected_size)?;
        if destination.exists() && !overwrite {
            return Err(ArtifactStoreError::ExportExists(
                destination.display().to_string(),
            ));
        }
        if destination
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(ArtifactStoreError::NotRegularFile(
                destination.display().to_string(),
            ));
        }
        let parent = destination.parent().ok_or_else(|| {
            ArtifactStoreError::Persist(format!(
                "export target {} has no parent",
                destination.display()
            ))
        })?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        let mut input = File::open(source)?;
        std::io::copy(&mut input, temporary.as_file_mut())?;
        temporary.as_file_mut().sync_all()?;
        if overwrite {
            temporary
                .persist(destination)
                .map_err(|error| ArtifactStoreError::Persist(error.error.to_string()))?;
        } else {
            temporary
                .persist_noclobber(destination)
                .map_err(|error| ArtifactStoreError::Persist(error.error.to_string()))?;
        }
        sync_directory(parent)?;
        Ok(())
    }
}

fn prepare_root(root: &Path) -> Result<(), ArtifactStoreError> {
    for directory in [
        root.to_path_buf(),
        root.join("sha256"),
        root.join("tmp"),
        root.join("trash"),
    ] {
        std::fs::create_dir_all(&directory)?;
        set_private_directory(&directory)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".lock"))?;
    set_private_file(&lock)?;
    lock.sync_all()?;
    sync_directory(root)?;
    Ok(())
}

fn validate_digest(digest: &str) -> Result<&str, ArtifactStoreError> {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err(ArtifactStoreError::InvalidDigest(digest.to_owned()));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ArtifactStoreError::InvalidDigest(digest.to_owned()));
    }
    Ok(hex)
}

#[cfg(unix)]
fn set_blob_read_only(path: &Path) -> Result<(), ArtifactStoreError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_blob_read_only(path: &Path) -> Result<(), ArtifactStoreError> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), ArtifactStoreError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), ArtifactStoreError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(file: &File) -> Result<(), ArtifactStoreError> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file(_file: &File) -> Result<(), ArtifactStoreError> {
    Ok(())
}

#[cfg(unix)]
fn make_owner_writable(permissions: &mut std::fs::Permissions) {
    use std::os::unix::fs::PermissionsExt;

    permissions.set_mode(0o600);
}

#[cfg(not(unix))]
fn make_owner_writable(permissions: &mut std::fs::Permissions) {
    permissions.set_readonly(false);
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), ArtifactStoreError> {
    OpenOptions::new().read(true).open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), ArtifactStoreError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_deduplicates_verifies_exports_and_detects_corruption() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("report.txt");
        std::fs::write(&source, b"durable report").expect("source");
        let store =
            LocalArtifactStore::open(root.path().join("cas")).expect("artifact store opens");

        let first = store.ingest(&source, 1024).expect("first ingest");
        let second = store.ingest(&source, 1024).expect("deduplicated ingest");
        assert_eq!(first, second);
        assert!(
            store
                .verify(&first.digest, first.size_bytes)
                .expect("verify")
                .valid
        );

        let export = root.path().join("export").join("report.txt");
        store
            .export(&first.digest, first.size_bytes, &export, false)
            .expect("export");
        assert_eq!(
            std::fs::read(&export).expect("export bytes"),
            b"durable report"
        );

        let blob = store.blob_path(&first.digest).expect("blob path");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(store.root())
                    .expect("root metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(store.root().join(".lock"))
                    .expect("lock metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&blob)
                    .expect("blob metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o400
            );
        }
        let mut permissions = std::fs::metadata(&blob).expect("metadata").permissions();
        make_owner_writable(&mut permissions);
        std::fs::set_permissions(&blob, permissions).expect("make writable");
        std::fs::write(&blob, b"corrupt").expect("corrupt blob");
        assert!(matches!(
            store.verify(&first.digest, first.size_bytes),
            Err(ArtifactStoreError::Corrupt { .. })
        ));
    }

    #[test]
    fn ingest_rejects_limits_and_export_symlinks() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("large.bin");
        std::fs::write(&source, [7_u8; 8]).expect("source");
        let store =
            LocalArtifactStore::open(root.path().join("cas")).expect("artifact store opens");
        assert!(matches!(
            store.ingest(&source, 7),
            Err(ArtifactStoreError::SizeLimit { .. })
        ));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let blob = store.ingest(&source, 8).expect("ingest");
            let destination = root.path().join("destination");
            let target = root.path().join("target");
            std::fs::write(&target, b"unchanged").expect("target");
            symlink(&target, &destination).expect("symlink");
            assert!(matches!(
                store.export(&blob.digest, blob.size_bytes, &destination, true),
                Err(ArtifactStoreError::NotRegularFile(_))
            ));
            assert_eq!(std::fs::read(&target).expect("target"), b"unchanged");
        }
    }
}
