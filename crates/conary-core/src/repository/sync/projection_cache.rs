// conary-core/src/repository/sync/projection_cache.rs

//! Strict durable cache for normalized authenticated native projections.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::repository::catalog::{
    CATALOG_CONTENT_SCHEMA_V1, CATALOG_FILE_NAME, CatalogBindingV1, CatalogCopyScratchV1,
    CatalogReader, CatalogScratchAdmission, CatalogSourceEvidenceV1,
};
use crate::repository::parsers::{
    AuthenticatedMetadataObject, AuthenticatedSnapshotIdentity,
    REPOSITORY_SNAPSHOT_PROJECTION_VERSION,
};

const CACHE_SCHEMA_VERSION: u32 = 1;
const MANIFEST_FILE_NAME: &str = "projection.json";
const MAX_MANIFEST_SIZE: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectionCacheKeyV1 {
    cache_schema_version: u32,
    parser_projection_version: u32,
    catalog_schema_version: u32,
    stream_binding_sha256: String,
    authenticated_root_sha256: String,
    authenticated_root_size: u64,
    authenticated_objects: Vec<AuthenticatedMetadataObject>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectionCacheManifestV1 {
    key: ProjectionCacheKeyV1,
    catalog: CatalogBindingV1,
}

pub(super) struct ProjectionCache {
    root: PathBuf,
    stream_binding_sha256: String,
    scratch_admission: Option<Arc<dyn CatalogScratchAdmission>>,
}

impl ProjectionCache {
    pub(super) fn open(root: &Path, stream_binding_sha256: &str) -> Result<Self> {
        Self::open_inner(root, stream_binding_sha256, None)
    }

    pub(super) fn open_with_scratch_admission(
        root: &Path,
        stream_binding_sha256: &str,
        scratch_admission: Arc<dyn CatalogScratchAdmission>,
    ) -> Result<Self> {
        Self::open_inner(root, stream_binding_sha256, Some(scratch_admission))
    }

    fn open_inner(
        root: &Path,
        stream_binding_sha256: &str,
        scratch_admission: Option<Arc<dyn CatalogScratchAdmission>>,
    ) -> Result<Self> {
        if stream_binding_sha256.len() != 64
            || !stream_binding_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::ConfigError(
                "native projection cache requires an exact stream binding SHA-256".to_string(),
            ));
        }
        fs::create_dir_all(root)?;
        require_real_directory(root, "native projection cache root")?;
        Ok(Self {
            root: root.to_path_buf(),
            stream_binding_sha256: stream_binding_sha256.to_string(),
            scratch_admission,
        })
    }

    pub(super) fn lookup(
        &self,
        snapshot: &AuthenticatedSnapshotIdentity,
        objects: &[AuthenticatedMetadataObject],
    ) -> Result<Option<CatalogReader>> {
        let key = self.key(snapshot, objects)?;
        let entry = self.entry_path(&key)?;
        if !entry.try_exists()? {
            return Ok(None);
        }
        match self.lookup_exact(&entry, &key) {
            Ok(reader) => Ok(Some(reader)),
            Err(error) => {
                tracing::warn!(path = %entry.display(), %error, "discarding invalid native projection cache entry");
                remove_exact_entry(&self.root, &entry)?;
                Ok(None)
            }
        }
    }

    pub(super) fn publish(
        &self,
        snapshot: &AuthenticatedSnapshotIdentity,
        objects: &[AuthenticatedMetadataObject],
        catalog: &CatalogBindingV1,
        candidate_path: &Path,
    ) -> Result<()> {
        let key = self.key(snapshot, objects)?;
        let entry = self.entry_path(&key)?;
        if entry.try_exists()? {
            self.lookup_exact(&entry, &key)?;
            return Ok(());
        }

        let candidate_metadata = fs::symlink_metadata(candidate_path)?;
        if candidate_metadata.file_type().is_symlink()
            || !candidate_metadata.file_type().is_file()
            || candidate_metadata.len() != catalog.artifact.size
        {
            return Err(Error::ConflictError(
                "native projection cache candidate does not match its exact catalog byte binding"
                    .to_string(),
            ));
        }
        let manifest = ProjectionCacheManifestV1 {
            key: key.clone(),
            catalog: catalog.clone(),
        };
        let manifest_bytes = crate::json::canonical_json(&manifest).map_err(Error::ConfigError)?;
        let manifest_size = u64::try_from(manifest_bytes.len()).map_err(|_| {
            Error::IoError("native projection cache manifest exceeds byte range".to_string())
        })?;
        if manifest_size > MAX_MANIFEST_SIZE {
            return Err(Error::ConfigError(format!(
                "native projection cache manifest requires {manifest_size} bytes, exceeding the \
                 {MAX_MANIFEST_SIZE}-byte bound"
            )));
        }
        let copy_requirement =
            CatalogCopyScratchV1::from_exact_bytes(catalog.artifact.size, manifest_size)?;
        let _scratch_lease = self
            .scratch_admission
            .as_ref()
            .map(|admission| admission.reserve_copy(&self.root, copy_requirement))
            .transpose()?;

        let stage = self
            .root
            .join(format!(".candidate-{}", uuid::Uuid::new_v4()));
        create_private_directory(&stage)?;
        let result = (|| {
            let target = stage.join(CATALOG_FILE_NAME);
            fs::copy(candidate_path, &target)?;
            set_private_file_permissions(&target)?;
            File::open(&target)?.sync_all()?;

            let manifest_path = stage.join(MANIFEST_FILE_NAME);
            let mut file = private_new_file(&manifest_path)?;
            file.write_all(&manifest_bytes)?;
            file.sync_all()?;
            File::open(&stage)?.sync_all()?;

            match fs::rename(&stage, &entry) {
                Ok(()) => {
                    File::open(&self.root)?.sync_all()?;
                    if let Err(error) = self.lookup_exact(&entry, &key) {
                        return Err(cleanup_failed_publication(&self.root, &entry, error));
                    }
                    Ok(())
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
                    ) =>
                {
                    remove_exact_entry(&self.root, &stage)?;
                    self.lookup_exact(&entry, &key).map(|_| ())
                }
                Err(error) => Err(error.into()),
            }
        })();
        if result.is_err() && stage.try_exists().unwrap_or(false) {
            let _ = remove_exact_entry(&self.root, &stage);
        }
        result
    }

    fn lookup_exact(&self, entry: &Path, key: &ProjectionCacheKeyV1) -> Result<CatalogReader> {
        require_direct_child(&self.root, entry)?;
        require_real_directory(entry, "native projection cache entry")?;
        let manifest_path = entry.join(MANIFEST_FILE_NAME);
        let metadata = fs::symlink_metadata(&manifest_path)?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_file()
            || metadata.len() > MAX_MANIFEST_SIZE
        {
            return Err(Error::InvalidPath(format!(
                "native projection cache manifest {} is not a bounded regular file",
                manifest_path.display()
            )));
        }
        let bytes = fs::read(&manifest_path)?;
        let manifest: ProjectionCacheManifestV1 = serde_json::from_slice(&bytes)?;
        if &manifest.key != key {
            return Err(Error::ConflictError(
                "native projection cache manifest key does not match its exact lookup key"
                    .to_string(),
            ));
        }
        let canonical = crate::json::canonical_json(&manifest).map_err(Error::ConfigError)?;
        if canonical != bytes {
            return Err(Error::ConflictError(
                "native projection cache manifest is not canonical JSON".to_string(),
            ));
        }
        let reader =
            CatalogReader::open_verified(entry.join(CATALOG_FILE_NAME), &manifest.catalog)?;
        let expected_evidence = key
            .authenticated_objects
            .iter()
            .map(|object| CatalogSourceEvidenceV1::AuthenticatedObject {
                role: object.role.clone(),
                source_path: object.source_path.clone(),
                sha256: object.sha256.clone(),
                size: object.size,
            })
            .collect::<Vec<_>>();
        if reader.source_evidence()? != expected_evidence {
            return Err(Error::ConflictError(
                "native projection cache catalog evidence does not match its authenticated child key"
                    .to_string(),
            ));
        }
        Ok(reader)
    }

    fn key(
        &self,
        snapshot: &AuthenticatedSnapshotIdentity,
        objects: &[AuthenticatedMetadataObject],
    ) -> Result<ProjectionCacheKeyV1> {
        let mut authenticated_objects = objects.to_vec();
        authenticated_objects.sort_by(|left, right| {
            left.role
                .cmp(&right.role)
                .then_with(|| left.source_path.cmp(&right.source_path))
        });
        for pair in authenticated_objects.windows(2) {
            if pair[0].role == pair[1].role {
                return Err(Error::ConflictError(
                    "native projection cache key repeats an authenticated child role".to_string(),
                ));
            }
        }
        Ok(ProjectionCacheKeyV1 {
            cache_schema_version: CACHE_SCHEMA_VERSION,
            parser_projection_version: REPOSITORY_SNAPSHOT_PROJECTION_VERSION,
            catalog_schema_version: CATALOG_CONTENT_SCHEMA_V1,
            stream_binding_sha256: self.stream_binding_sha256.clone(),
            authenticated_root_sha256: snapshot.sha256().to_string(),
            authenticated_root_size: snapshot.size().ok_or_else(|| {
                Error::ConfigError(
                    "native projection cache requires an exact authenticated root size".to_string(),
                )
            })?,
            authenticated_objects,
        })
    }

    fn entry_path(&self, key: &ProjectionCacheKeyV1) -> Result<PathBuf> {
        let bytes = crate::json::canonical_json(key).map_err(Error::ConfigError)?;
        Ok(self.root.join(crate::hash::sha256(&bytes)))
    }
}

fn cleanup_failed_publication(root: &Path, entry: &Path, error: Error) -> Error {
    match remove_exact_entry(root, entry) {
        Ok(()) => error,
        Err(cleanup_error) => Error::IoError(format!(
            "native projection cache publication failed after creating {}; cleanup also failed \
             ({cleanup_error}): {error}",
            entry.display()
        )),
    }
}

fn require_direct_child(root: &Path, entry: &Path) -> Result<()> {
    if entry.parent() != Some(root) {
        return Err(Error::InvalidPath(format!(
            "native projection cache entry {} is not a direct child of {}",
            entry.display(),
            root.display()
        )));
    }
    Ok(())
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(Error::InvalidPath(format!(
            "{label} {} must be a real directory",
            path.display()
        )));
    }
    Ok(())
}

fn remove_exact_entry(root: &Path, entry: &Path) -> Result<()> {
    require_direct_child(root, entry)?;
    let metadata = fs::symlink_metadata(entry)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(Error::InvalidPath(format!(
            "native projection cache entry {} must be a real directory",
            entry.display()
        )));
    }
    fs::remove_dir_all(entry)?;
    File::open(root)?.sync_all()?;
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(path)?;
    Ok(())
}

fn private_new_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn set_private_file_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::catalog::{
        CatalogCandidateWriter, CatalogFinalizationScratchV1, CatalogMetadataScratchV1,
        CatalogMetadataStreamAdmission, CatalogMetadataStreamScratchV1, CatalogScopeV1,
        CatalogScratchCapacityError, SourceMetadataObjectRoleV1,
    };
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RecordingAdmission {
        copies: Mutex<Vec<CatalogCopyScratchV1>>,
        lease_drops: Arc<AtomicUsize>,
        refuse: bool,
    }

    struct RecordingLease(Arc<AtomicUsize>);

    impl Drop for RecordingLease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl CatalogScratchAdmission for RecordingAdmission {
        fn reserve_profile_candidate(
            &self,
            _candidate_path: &Path,
            _requirement: crate::repository::catalog::CatalogProfileCandidateScratchV1,
        ) -> Result<Box<dyn Send>> {
            panic!("projection cache must not request profile growth admission")
        }

        fn reserve_metadata(
            &self,
            _work_directory: &Path,
            _requirement: CatalogMetadataScratchV1,
        ) -> Result<Box<dyn Send>> {
            panic!("projection cache must not request metadata admission")
        }

        fn stream_metadata(
            &self,
            _work_directory: &Path,
            _requirement: CatalogMetadataStreamScratchV1,
        ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
            panic!("projection cache must not request streamed metadata admission")
        }

        fn reserve_finalization(
            &self,
            _candidate_path: &Path,
            _requirement: CatalogFinalizationScratchV1,
        ) -> Result<Box<dyn Send>> {
            panic!("projection cache must not request finalization admission")
        }

        fn reserve_copy(
            &self,
            _destination_root: &Path,
            requirement: CatalogCopyScratchV1,
        ) -> Result<Box<dyn Send>> {
            self.copies.lock().unwrap().push(requirement);
            if self.refuse {
                return Err(CatalogScratchCapacityError {
                    required_bytes: requirement.required_additional_bytes,
                    available_bytes: requirement.required_additional_bytes - 1,
                    reserved_bytes: 0,
                }
                .into());
            }
            Ok(Box::new(RecordingLease(Arc::clone(&self.lease_drops))))
        }
    }

    fn recording_admission(refuse: bool) -> Arc<RecordingAdmission> {
        Arc::new(RecordingAdmission {
            copies: Mutex::new(Vec::new()),
            lease_drops: Arc::new(AtomicUsize::new(0)),
            refuse,
        })
    }

    struct Fixture {
        _root: tempfile::TempDir,
        cache: ProjectionCache,
        snapshot: AuthenticatedSnapshotIdentity,
        objects: Vec<AuthenticatedMetadataObject>,
        candidate: PathBuf,
        binding: CatalogBindingV1,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let candidate = root.path().join("candidate.sqlite");
        let object = AuthenticatedMetadataObject {
            role: SourceMetadataObjectRoleV1::DebianPackages,
            source_path: "main/binary-amd64/Packages.xz".to_string(),
            sha256: "b".repeat(64),
            size: 123,
        };
        let evidence = vec![CatalogSourceEvidenceV1::AuthenticatedObject {
            role: object.role.clone(),
            source_path: object.source_path.clone(),
            sha256: object.sha256.clone(),
            size: object.size,
        }];
        let writer = CatalogCandidateWriter::create(
            &candidate,
            CatalogScopeV1::Source {
                source_profile: "ubuntu-26.04".to_string(),
                source_identity: "ubuntu".to_string(),
                repository_identity: "ubuntu-main-amd64".to_string(),
            },
        )
        .unwrap();
        let binding = writer.finish(evidence).unwrap();
        let cache = ProjectionCache::open(&root.path().join("cache"), &"a".repeat(64)).unwrap();
        Fixture {
            _root: root,
            cache,
            snapshot: AuthenticatedSnapshotIdentity::for_bytes(b"authenticated release"),
            objects: vec![object],
            candidate,
            binding,
        }
    }

    #[test]
    fn exact_projection_key_replays_verified_catalog_without_native_input() {
        let fixture = fixture();
        fixture
            .cache
            .publish(
                &fixture.snapshot,
                &fixture.objects,
                &fixture.binding,
                &fixture.candidate,
            )
            .unwrap();

        let reader = fixture
            .cache
            .lookup(&fixture.snapshot, &fixture.objects)
            .unwrap()
            .expect("exact cache key should hit");
        assert_eq!(reader.binding(), &fixture.binding);
        assert!(reader.packages().unwrap().is_empty());
    }

    #[test]
    fn copy_admission_uses_exact_bytes_and_existing_hit_writes_nothing() {
        let fixture = fixture();
        let admission = recording_admission(false);
        let cache = ProjectionCache::open_with_scratch_admission(
            &fixture.cache.root,
            &"a".repeat(64),
            admission.clone(),
        )
        .unwrap();
        cache
            .publish(
                &fixture.snapshot,
                &fixture.objects,
                &fixture.binding,
                &fixture.candidate,
            )
            .unwrap();

        let copies = admission.copies.lock().unwrap();
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].catalog_bytes, fixture.binding.artifact.size);
        assert!(copies[0].manifest_bytes > 0);
        assert_eq!(
            copies[0].required_additional_bytes,
            copies[0].catalog_bytes + copies[0].manifest_bytes
        );
        drop(copies);
        assert_eq!(admission.lease_drops.load(Ordering::SeqCst), 1);

        cache
            .publish(
                &fixture.snapshot,
                &fixture.objects,
                &fixture.binding,
                &fixture.candidate,
            )
            .unwrap();
        assert_eq!(admission.copies.lock().unwrap().len(), 1);
        assert_eq!(admission.lease_drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn one_byte_short_refusal_precedes_projection_cache_mutation() {
        let fixture = fixture();
        let admission = recording_admission(true);
        let cache = ProjectionCache::open_with_scratch_admission(
            &fixture.cache.root,
            &"a".repeat(64),
            admission.clone(),
        )
        .unwrap();
        let error = cache
            .publish(
                &fixture.snapshot,
                &fixture.objects,
                &fixture.binding,
                &fixture.candidate,
            )
            .unwrap_err();
        let Error::CatalogScratchCapacity(error) = error else {
            panic!("expected typed catalog capacity refusal");
        };
        assert_eq!(error.available_bytes + 1, error.required_bytes);
        assert_eq!(admission.copies.lock().unwrap().len(), 1);
        assert_eq!(admission.lease_drops.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read_dir(&fixture.cache.root).unwrap().count(), 0);
    }

    #[test]
    fn altered_child_role_digest_or_source_binding_cannot_reuse_projection() {
        let fixture = fixture();
        fixture
            .cache
            .publish(
                &fixture.snapshot,
                &fixture.objects,
                &fixture.binding,
                &fixture.candidate,
            )
            .unwrap();

        let mut changed_digest = fixture.objects.clone();
        changed_digest[0].sha256 = "c".repeat(64);
        assert!(
            fixture
                .cache
                .lookup(&fixture.snapshot, &changed_digest)
                .unwrap()
                .is_none()
        );
        let mut changed_role = fixture.objects.clone();
        changed_role[0].role = SourceMetadataObjectRoleV1::RpmPrimary;
        assert!(
            fixture
                .cache
                .lookup(&fixture.snapshot, &changed_role)
                .unwrap()
                .is_none()
        );
        let other_binding = ProjectionCache::open(&fixture.cache.root, &"d".repeat(64)).unwrap();
        assert!(
            other_binding
                .lookup(&fixture.snapshot, &fixture.objects)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn tampered_or_version_mixed_manifest_is_discarded_before_reuse() {
        let fixture = fixture();
        fixture
            .cache
            .publish(
                &fixture.snapshot,
                &fixture.objects,
                &fixture.binding,
                &fixture.candidate,
            )
            .unwrap();
        let key = fixture
            .cache
            .key(&fixture.snapshot, &fixture.objects)
            .unwrap();
        let entry = fixture.cache.entry_path(&key).unwrap();
        let manifest_path = entry.join(MANIFEST_FILE_NAME);
        let mut manifest: ProjectionCacheManifestV1 =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest.key.parser_projection_version += 1;
        fs::write(
            &manifest_path,
            crate::json::canonical_json(&manifest).unwrap(),
        )
        .unwrap();

        assert!(
            fixture
                .cache
                .lookup(&fixture.snapshot, &fixture.objects)
                .unwrap()
                .is_none()
        );
        assert!(!entry.exists());
    }
}
