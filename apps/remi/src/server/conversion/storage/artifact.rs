// apps/remi/src/server/conversion/storage/artifact.rs
//! Verified converted-archive staging and append-only digest publication.

use anyhow::{Context, Result, ensure};
use conary_core::ccs::convert::PendingConversionResult;
use conary_core::ccs::verify::TrustPolicy;
use conary_core::filesystem::CasStore;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PRIVATE_STAGING_WRITE_MODE: u32 = 0o600;
const SEALED_ARCHIVE_MODE: u32 = 0o400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn matches(self, metadata: &fs::Metadata) -> bool {
        metadata.file_type().is_file()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
    }
}

#[derive(Debug, Default)]
pub(in crate::server::conversion) struct ConversionArchiveWork {
    pub(in crate::server::conversion) complete_archive_copy: Duration,
    pub(in crate::server::conversion) complete_archive_hash: Duration,
    pub(in crate::server::conversion) complete_archive_copy_bytes: u64,
    pub(in crate::server::conversion) complete_archive_hash_bytes: u64,
}

/// Holds the private staging name and exact read-only canonical inode through
/// conversion metadata commit. Published digest names are append-only runtime
/// cache material and are reclaimed only by exclusive stopped-runtime GC.
#[derive(Debug)]
pub(in crate::server::conversion) struct PublishedConversionArtifact {
    staging_path: PathBuf,
    staging_inode: FileIdentity,
    final_path: PathBuf,
    /// Exact read-only canonical inode used by the independent hash. Retained
    /// through conversion-row commit as the runtime identity handle.
    final_file: File,
    final_inode: FileIdentity,
    final_mode: u32,
    archive_sha256: String,
    archive_bytes: u64,
}

impl PublishedConversionArtifact {
    pub(in crate::server::conversion) fn path(&self) -> &Path {
        &self.final_path
    }

    pub(in crate::server::conversion) fn archive_sha256(&self) -> &str {
        &self.archive_sha256
    }

    pub(in crate::server::conversion) fn archive_bytes(&self) -> u64 {
        self.archive_bytes
    }

    /// Require both names to retain the exact regular inodes admitted before
    /// any conversion row may refer to the published artifact.
    pub(in crate::server::conversion) fn require_publication_binding(&self) -> Result<()> {
        require_named_inode(
            &self.staging_path,
            self.staging_inode,
            self.archive_bytes,
            Some(SEALED_ARCHIVE_MODE),
            "verified conversion staging artifact",
        )?;
        require_named_inode(
            &self.final_path,
            self.final_inode,
            self.archive_bytes,
            Some(self.final_mode),
            "published conversion artifact",
        )?;
        let opened = self.final_file.metadata()?;
        ensure!(
            self.final_inode.matches(&opened)
                && opened.len() == self.archive_bytes
                && opened.mode() & 0o7777 == self.final_mode,
            "held published conversion artifact no longer matches its admitted regular inode"
        );
        Ok(())
    }

    /// Best-effort cleanup after the authoritative conversion transaction has
    /// committed. Cleanup can never turn that successful commit into a request
    /// failure, and it never removes the canonical digest name.
    pub(in crate::server::conversion) fn retire_staging_after_commit(&mut self) {
        if remove_if_same_inode(&self.staging_path, self.staging_inode).unwrap_or(false)
            && let Some(parent) = self.final_path.parent()
        {
            let _ = sync_directory(parent);
        }
    }
}

impl Drop for PublishedConversionArtifact {
    fn drop(&mut self) {
        let changed = remove_if_same_inode(&self.staging_path, self.staging_inode).unwrap_or(false);
        if changed && let Some(parent) = self.final_path.parent() {
            let _ = sync_directory(parent);
        }
    }
}

pub(super) struct FinalizedConversionArtifact {
    pub(super) conversion: conary_core::ccs::convert::ConversionResult,
    pub(super) verification: conary_core::ccs::VerifiedCcsArchive,
    pub(super) artifact: PublishedConversionArtifact,
    pub(super) work: ConversionArchiveWork,
    pub(super) verification_and_cas_duration: Duration,
}

pub(super) fn stage_finalize_and_publish_then<F>(
    pending: PendingConversionResult,
    policy: &TrustPolicy,
    cas: &CasStore,
    packages_dir: &Path,
    after_publication: F,
) -> Result<FinalizedConversionArtifact>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let authored_bytes = pending.metrics().ccs_write.ccs_output_bytes;
    let (staged, complete_archive_copy) = copy_to_private_staging(
        pending.unverified_package_path(),
        authored_bytes,
        packages_dir,
    )?;

    let verification_started = Instant::now();
    let finalized = pending.verify_staged_copy_into_cas(staged.path(), policy, cas)?;
    let verification_and_cas_duration = verification_started.elapsed();
    let verified_identity = finalized.archive_identity().clone();
    staged.require_binding("verified conversion staging artifact")?;

    let publication = publish_staged_archive(
        staged,
        verified_identity.sha256(),
        verified_identity.bytes(),
    )?;
    after_publication(publication.path())?;
    let mut publication = publication;
    let complete_archive_hash = require_published_digest(
        &mut publication,
        verified_identity.sha256(),
        verified_identity.bytes(),
    )?;
    let (conversion, verification, finalized_identity) = finalized.into_parts();
    ensure!(
        finalized_identity == verified_identity,
        "native conversion archive identity changed while publishing"
    );

    Ok(FinalizedConversionArtifact {
        conversion,
        verification,
        artifact: publication,
        work: ConversionArchiveWork {
            complete_archive_copy: complete_archive_copy.duration,
            complete_archive_hash,
            complete_archive_copy_bytes: complete_archive_copy.bytes,
            complete_archive_hash_bytes: verified_identity.bytes(),
        },
        verification_and_cas_duration,
    })
}

struct CopyWork {
    duration: Duration,
    bytes: u64,
}

struct StagedArchive {
    path: PathBuf,
    inode: FileIdentity,
    bytes: u64,
    armed: bool,
}

impl StagedArchive {
    fn path(&self) -> &Path {
        &self.path
    }

    fn require_binding(&self, label: &str) -> Result<()> {
        require_named_inode(
            &self.path,
            self.inode,
            self.bytes,
            Some(SEALED_ARCHIVE_MODE),
            label,
        )
    }
}

impl Drop for StagedArchive {
    fn drop(&mut self) {
        if self.armed {
            let _ = remove_if_same_inode(&self.path, self.inode);
            if let Some(parent) = self.path.parent() {
                let _ = sync_directory(parent);
            }
        }
    }
}

fn copy_to_private_staging(
    source_path: &Path,
    expected_bytes: u64,
    packages_dir: &Path,
) -> Result<(StagedArchive, CopyWork)> {
    fs::create_dir_all(packages_dir).with_context(|| {
        format!(
            "create durable converted-package directory {}",
            packages_dir.display()
        )
    })?;
    let packages_metadata = fs::symlink_metadata(packages_dir).with_context(|| {
        format!(
            "inspect durable converted-package directory {}",
            packages_dir.display()
        )
    })?;
    ensure!(
        packages_metadata.file_type().is_dir() && !packages_metadata.file_type().is_symlink(),
        "durable converted-package path {} must be a real directory",
        packages_dir.display()
    );

    let copy_started = Instant::now();
    let source_named = fs::symlink_metadata(source_path)
        .with_context(|| format!("inspect pending CCS archive {}", source_path.display()))?;
    ensure!(
        source_named.file_type().is_file() && !source_named.file_type().is_symlink(),
        "pending CCS archive {} must be a regular non-symlink file",
        source_path.display()
    );
    let source_inode = FileIdentity::from_metadata(&source_named);
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(source_path)
        .with_context(|| format!("open pending CCS archive {}", source_path.display()))?;
    let opened_source = source.metadata()?;
    ensure!(
        source_inode.matches(&opened_source) && opened_source.len() == expected_bytes,
        "pending CCS archive changed before durable staging"
    );

    let staging_path = packages_dir.join(format!(".conversion-{}.ccs.tmp", uuid::Uuid::new_v4()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_STAGING_WRITE_MODE)
        .custom_flags(libc::O_CLOEXEC)
        .open(&staging_path)
        .with_context(|| {
            format!(
                "create private converted-package staging file {}",
                staging_path.display()
            )
        })?;
    output.set_permissions(fs::Permissions::from_mode(PRIVATE_STAGING_WRITE_MODE))?;
    let staged_metadata = output.metadata()?;
    let staged = StagedArchive {
        path: staging_path,
        inode: FileIdentity::from_metadata(&staged_metadata),
        bytes: expected_bytes,
        armed: true,
    };

    let copied = io::copy(&mut source, &mut output)
        .context("copy pending CCS archive into durable private staging")?;
    ensure!(
        copied == expected_bytes,
        "pending CCS archive changed size while entering durable staging"
    );
    ensure!(
        source_inode.matches(&fs::symlink_metadata(source_path)?)
            && source.metadata()?.len() == expected_bytes,
        "pending CCS archive changed while entering durable staging"
    );
    output.sync_all().with_context(|| {
        format!(
            "synchronize private converted-package staging file {}",
            staged.path.display()
        )
    })?;
    output.set_permissions(fs::Permissions::from_mode(SEALED_ARCHIVE_MODE))?;
    output.sync_all().with_context(|| {
        format!(
            "synchronize sealed converted-package staging file {}",
            staged.path.display()
        )
    })?;
    staged.require_binding("private converted-package staging file")?;
    drop(output);
    drop(source);
    sync_directory(packages_dir)?;
    let duration = copy_started.elapsed();

    Ok((
        staged,
        CopyWork {
            duration,
            bytes: copied,
        },
    ))
}

fn publish_staged_archive(
    mut staged: StagedArchive,
    archive_sha256: &str,
    archive_bytes: u64,
) -> Result<PublishedConversionArtifact> {
    ensure!(
        archive_sha256.len() == 64
            && archive_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            && archive_sha256 == archive_sha256.to_ascii_lowercase(),
        "verified conversion archive identity is not one lowercase SHA-256 digest"
    );
    ensure!(
        archive_bytes == staged.bytes,
        "verified conversion archive size differs from durable staging"
    );
    staged.require_binding("verified conversion staging artifact")?;
    let parent = staged
        .path
        .parent()
        .context("verified conversion staging artifact has no parent")?;
    let final_path = parent.join(format!("{archive_sha256}.ccs"));

    let (final_inode, final_mode) = match fs::hard_link(&staged.path, &final_path) {
        Ok(()) => (staged.inode, SEALED_ARCHIVE_MODE),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let final_metadata = fs::symlink_metadata(&final_path).with_context(|| {
                format!(
                    "inspect preexisting converted-package artifact {}",
                    final_path.display()
                )
            })?;
            ensure!(
                final_metadata.file_type().is_file()
                    && !final_metadata.file_type().is_symlink()
                    && final_metadata.mode() & 0o7777 == SEALED_ARCHIVE_MODE,
                "preexisting converted-package artifact {} is not one sealed private regular file",
                final_path.display()
            );
            let final_inode = FileIdentity::from_metadata(&final_metadata);
            (final_inode, final_metadata.mode() & 0o7777)
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "atomically publish verified conversion staging artifact {} as {}",
                    staged.path.display(),
                    final_path.display()
                )
            });
        }
    };
    let final_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&final_path)
        .with_context(|| {
            format!(
                "open published conversion artifact {}",
                final_path.display()
            )
        })?;
    let opened_final = final_file.metadata()?;
    ensure!(
        final_inode.matches(&opened_final)
            && opened_final.len() == archive_bytes
            && opened_final.mode() & 0o7777 == final_mode,
        "published conversion artifact changed while acquiring its read-only identity handle"
    );

    let artifact = PublishedConversionArtifact {
        staging_path: staged.path.clone(),
        staging_inode: staged.inode,
        final_path,
        final_file,
        final_inode,
        final_mode,
        archive_sha256: archive_sha256.to_string(),
        archive_bytes,
    };
    staged.armed = false;
    artifact.require_publication_binding()?;
    sync_directory(parent)?;

    Ok(artifact)
}

fn require_published_digest(
    artifact: &mut PublishedConversionArtifact,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<Duration> {
    artifact.require_publication_binding()?;
    let hash_started = Instant::now();
    let final_path = artifact.final_path.clone();
    let final_inode = artifact.final_inode;
    let published_sha256 = hash_opened_named_regular_file(
        &final_path,
        &mut artifact.final_file,
        final_inode,
        expected_bytes,
        "published conversion artifact",
    )?;
    let duration = hash_started.elapsed();
    ensure!(
        published_sha256 == expected_sha256,
        "published conversion artifact changed after finalization"
    );
    Ok(duration)
}

fn hash_opened_named_regular_file(
    path: &Path,
    input: &mut File,
    expected_inode: FileIdentity,
    expected_bytes: u64,
    label: &str,
) -> Result<String> {
    let named_before = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        expected_inode.matches(&named_before) && named_before.len() == expected_bytes,
        "{label} changed before independent hash"
    );
    let opened_before = input.metadata()?;
    ensure!(
        expected_inode.matches(&opened_before) && opened_before.len() == expected_bytes,
        "held {label} identity differs before independent hash"
    );
    input.rewind()?;
    let sha256 = conary_core::hash::hash_reader(conary_core::hash::HashAlgorithm::Sha256, input)?;
    let opened_after = input.metadata()?;
    let named_after = fs::symlink_metadata(path)?;
    ensure!(
        expected_inode.matches(&opened_after)
            && opened_after.len() == expected_bytes
            && expected_inode.matches(&named_after)
            && named_after.len() == expected_bytes,
        "{label} changed during independent hash"
    );
    Ok(sha256.value)
}

fn require_named_inode(
    path: &Path,
    identity: FileIdentity,
    expected_bytes: u64,
    expected_mode: Option<u32>,
    label: &str,
) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        identity.matches(&metadata)
            && metadata.len() == expected_bytes
            && expected_mode.is_none_or(|mode| metadata.mode() & 0o7777 == mode),
        "{label} no longer names the admitted private regular inode"
    );
    Ok(())
}

fn remove_if_same_inode(path: &Path, identity: FileIdentity) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if identity.matches(&metadata) => {
            fs::remove_file(path)
                .with_context(|| format!("remove owned conversion artifact {}", path.display()))?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("open directory {} for synchronization", path.display()))?
        .sync_all()
        .with_context(|| format!("synchronize directory {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged_bytes(root: &Path, packages_dir: &Path, bytes: &[u8]) -> StagedArchive {
        let source = root.join(format!("source-{}", uuid::Uuid::new_v4()));
        fs::write(&source, bytes).unwrap();
        copy_to_private_staging(&source, bytes.len() as u64, packages_dir)
            .unwrap()
            .0
    }

    fn digest(bytes: &[u8]) -> String {
        conary_core::hash::sha256(bytes)
    }

    #[test]
    fn preexisting_exact_artifact_is_verified_and_preserved() {
        let root = tempfile::tempdir().unwrap();
        let packages = root.path().join("packages");
        fs::create_dir(&packages).unwrap();
        let bytes = b"exact verified archive";
        let digest = digest(bytes);
        let final_path = packages.join(format!("{digest}.ccs"));
        fs::write(&final_path, bytes).unwrap();
        fs::set_permissions(&final_path, fs::Permissions::from_mode(SEALED_ARCHIVE_MODE)).unwrap();
        let staged = staged_bytes(root.path(), &packages, bytes);
        let staging_path = staged.path.clone();

        let mut published = publish_staged_archive(staged, &digest, bytes.len() as u64).unwrap();
        require_published_digest(&mut published, &digest, bytes.len() as u64).unwrap();
        drop(published);

        assert_eq!(fs::read(&final_path).unwrap(), bytes);
        assert!(!staging_path.exists());
    }

    #[test]
    fn drop_retires_staging_without_touching_a_replacement_digest() {
        let root = tempfile::tempdir().unwrap();
        let packages = root.path().join("packages");
        fs::create_dir(&packages).unwrap();
        let bytes = b"new verified archive";
        let digest = digest(bytes);
        let staged = staged_bytes(root.path(), &packages, bytes);
        let staging_path = staged.path.clone();
        let published = publish_staged_archive(staged, &digest, bytes.len() as u64).unwrap();
        let final_path = published.path().to_path_buf();

        fs::remove_file(&final_path).unwrap();
        fs::write(&final_path, b"concurrent unrelated artifact").unwrap();
        drop(published);

        assert_eq!(
            fs::read(&final_path).unwrap(),
            b"concurrent unrelated artifact"
        );
        assert!(!staging_path.exists());
    }

    #[test]
    fn persistence_failure_retires_only_staging_and_keeps_verified_digest() {
        let root = tempfile::tempdir().unwrap();
        let packages = root.path().join("packages");
        fs::create_dir(&packages).unwrap();
        let bytes = b"verified archive awaiting persistence";
        let digest = digest(bytes);
        let staged = staged_bytes(root.path(), &packages, bytes);
        let staging_path = staged.path.clone();
        let mut published = publish_staged_archive(staged, &digest, bytes.len() as u64).unwrap();
        let final_path = published.path().to_path_buf();

        require_published_digest(&mut published, &digest, bytes.len() as u64).unwrap();
        assert!(staging_path.exists());
        assert!(final_path.exists());

        // Model a later persistence failure by dropping the publication guard.
        drop(published);

        assert!(!staging_path.exists());
        assert_eq!(fs::read(&final_path).unwrap(), bytes);
    }

    #[test]
    fn committed_new_digest_link_survives_staging_retirement() {
        let root = tempfile::tempdir().unwrap();
        let packages = root.path().join("packages");
        fs::create_dir(&packages).unwrap();
        let bytes = b"committed verified archive";
        let digest = digest(bytes);
        let staged = staged_bytes(root.path(), &packages, bytes);
        let staging_path = staged.path.clone();
        let mut published = publish_staged_archive(staged, &digest, bytes.len() as u64).unwrap();
        let final_path = published.path().to_path_buf();
        require_published_digest(&mut published, &digest, bytes.len() as u64).unwrap();

        published.retire_staging_after_commit();
        drop(published);

        assert_eq!(fs::read(&final_path).unwrap(), bytes);
        assert!(!staging_path.exists());
    }

    #[test]
    fn corrupt_preexisting_digest_target_fails_without_removal() {
        let root = tempfile::tempdir().unwrap();
        let packages = root.path().join("packages");
        fs::create_dir(&packages).unwrap();
        let bytes = b"verified-archive";
        let corrupt = b"corrupt--archive";
        assert_eq!(bytes.len(), corrupt.len());
        let digest = digest(bytes);
        let final_path = packages.join(format!("{digest}.ccs"));
        fs::write(&final_path, corrupt).unwrap();
        fs::set_permissions(&final_path, fs::Permissions::from_mode(SEALED_ARCHIVE_MODE)).unwrap();
        let staged = staged_bytes(root.path(), &packages, bytes);
        let staging_path = staged.path.clone();

        let mut published = publish_staged_archive(staged, &digest, bytes.len() as u64).unwrap();
        let error = require_published_digest(&mut published, &digest, bytes.len() as u64)
            .expect_err("corrupt digest target must fail closed");
        assert!(error.to_string().contains("changed after finalization"));
        drop(published);
        assert_eq!(fs::read(&final_path).unwrap(), corrupt);
        assert!(!staging_path.exists());
    }

    #[test]
    fn canonical_path_swap_fails_and_drop_preserves_the_replacement() {
        let root = tempfile::tempdir().unwrap();
        let packages = root.path().join("packages");
        fs::create_dir(&packages).unwrap();
        let bytes = b"verified-archive";
        let replacement = b"replaced-archive";
        assert_eq!(bytes.len(), replacement.len());
        let digest = digest(bytes);
        let staged = staged_bytes(root.path(), &packages, bytes);
        let staging_path = staged.path.clone();
        let mut published = publish_staged_archive(staged, &digest, bytes.len() as u64).unwrap();
        let final_path = published.path().to_path_buf();

        fs::remove_file(&final_path).unwrap();
        fs::write(&final_path, replacement).unwrap();
        fs::set_permissions(&final_path, fs::Permissions::from_mode(SEALED_ARCHIVE_MODE)).unwrap();
        let error = require_published_digest(&mut published, &digest, bytes.len() as u64)
            .expect_err("canonical path replacement must fail closed");
        assert!(error.to_string().contains("admitted private regular inode"));
        drop(published);

        assert_eq!(fs::read(&final_path).unwrap(), replacement);
        assert!(!staging_path.exists());
    }
}
