// apps/remi/src/server/catalog_authority.rs

//! Read-only authority for exact immutable Remi profile catalogs.
//!
//! Operational SQLite owns the fenced pointer and the exact resource metadata;
//! the package catalog itself is always read from the content-addressed bundle
//! below `ServerConfig::catalog_dir`.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use conary_core::db::models::{
    RemiActiveProfileRevision, RemiCatalogResource, RemiCatalogResourceKind,
    RemiProfileRevisionPin, RemiRevisionPinKind, RemiRuntimeSession,
};
use conary_core::repository::catalog::{
    CATALOG_FILE_NAME, CATALOG_MANIFEST_FILE_NAME, CatalogReader, ProfileRevisionV2,
    verify_profile_catalog_bundle,
};
use parking_lot::{Mutex, MutexGuard};
use rusqlite::Connection;
use rusqlite::{Transaction, TransactionBehavior};

use super::database_writer::DatabaseWriter;
use super::open_runtime_db;

mod source;
#[cfg(test)]
pub(crate) mod test_support;

/// The configured roots used to resolve one active immutable profile catalog.
#[derive(Clone)]
pub struct CatalogAuthority {
    db_path: PathBuf,
    catalog_dir: PathBuf,
    database_writer: DatabaseWriter,
    verified_readers: Arc<Mutex<BTreeMap<String, CachedProfileReader>>>,
}

struct CachedProfileReader {
    profile_revision_sha256: String,
    reader: Arc<Mutex<CatalogReader>>,
}

/// Minimal identity of one exact immutable profile revision.
///
/// Activation fencing and ownership decide whether a revision is public. They
/// are deliberately absent here so private validation can select a durable
/// registered candidate without inventing an active pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRevisionSelection {
    pub source_profile: String,
    pub profile_revision_sha256: String,
}

impl From<&RemiActiveProfileRevision> for ProfileRevisionSelection {
    fn from(active: &RemiActiveProfileRevision) -> Self {
        Self {
            source_profile: active.source_profile.clone(),
            profile_revision_sha256: active.profile_revision_sha256.clone(),
        }
    }
}

/// Bounded, read-only facts about one active immutable profile catalog.
///
/// This is deliberately not a serving reader. Health and deployment probes
/// use it to establish active identity and population without replaying a
/// multi-gigabyte catalog or claiming SQLite write authority.
#[derive(Debug, Clone)]
pub(crate) struct ActiveProfileInspection {
    pub(crate) pointer: RemiActiveProfileRevision,
    pub(crate) manifest: ProfileRevisionV2,
}

/// Bounded, read-only facts about one exact durable immutable profile catalog.
#[derive(Debug, Clone)]
pub(crate) struct SelectedProfileInspection {
    pub(crate) manifest: ProfileRevisionV2,
}

impl CatalogAuthority {
    /// Build an authority from explicit roots. This is useful for narrowly
    /// scoped owners and keeps path derivation in one place.
    #[must_use]
    pub(crate) fn from_paths(
        db_path: impl Into<PathBuf>,
        catalog_dir: impl Into<PathBuf>,
        database_writer: DatabaseWriter,
    ) -> Self {
        Self {
            db_path: db_path.into(),
            catalog_dir: catalog_dir.into(),
            database_writer,
            verified_readers: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Build an authority for a process-external read-only inspection.
    pub(crate) fn for_inspection(
        db_path: impl Into<PathBuf>,
        catalog_dir: impl Into<PathBuf>,
    ) -> Self {
        Self::from_paths(db_path, catalog_dir, DatabaseWriter::default())
    }

    /// Inspect the active pointer, strict manifest, and bounded filesystem
    /// identity without opening or hashing the catalog contents.
    pub(crate) fn inspect_active_profile(
        &self,
        source_profile: &str,
    ) -> Result<ActiveProfileInspection> {
        let flags =
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&self.db_path, flags).with_context(|| {
            format!("open Remi operational database to inspect profile '{source_profile}'")
        })?;
        let pointer = RemiActiveProfileRevision::find(&conn, source_profile)
            .context("resolve active Remi profile revision pointer")?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "profile '{source_profile}' has no active immutable catalog revision"
                )
            })?;
        let resolved = resolve_profile_selection(
            &conn,
            &self.catalog_dir,
            ProfileRevisionSelection::from(&pointer),
        )?;
        inspect_resolved_profile_files(&resolved)?;
        Ok(ActiveProfileInspection {
            pointer,
            manifest: resolved.manifest,
        })
    }

    /// Independently reopen one exact registered revision without granting it
    /// active authority.
    pub(crate) fn verify_selected_profile(
        &self,
        selection: &ProfileRevisionSelection,
    ) -> Result<SelectedProfileInspection> {
        let flags =
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&self.db_path, flags).with_context(|| {
            format!(
                "open Remi operational database to inspect profile '{}' revision {}",
                selection.source_profile, selection.profile_revision_sha256
            )
        })?;
        let resolved = resolve_profile_selection(&conn, &self.catalog_dir, selection.clone())?;
        let verified = open_resolved_profile(resolved)?;
        Ok(SelectedProfileInspection {
            manifest: verified.manifest().clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn database_writer_for_test(&self) -> DatabaseWriter {
        self.database_writer.clone()
    }

    /// Open the exact profile revision named by the current operational pointer.
    ///
    /// The returned handle owns its immutable SQLite connection. A later
    /// activation can therefore publish a new pointer without changing what
    /// this handle reads.
    pub fn open_active_profile(&self, source_profile: &str) -> Result<PinnedProfileCatalog> {
        let pin_id = uuid::Uuid::new_v4().to_string();
        let resolved = self.database_writer.execute(|| {
            let conn = open_runtime_db(&self.db_path).with_context(|| {
                format!("open Remi operational database for profile '{source_profile}'")
            })?;
            let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)
                .context("acquire immutable catalog reader-pin transaction")?;
            let resolved = resolve_active_profile(&tx, &self.catalog_dir, source_profile)?;
            insert_reader_pin(&tx, &pin_id, &resolved.selection)?;
            tx.commit().context("commit immutable catalog reader pin")?;
            Ok::<_, anyhow::Error>(resolved)
        })?;

        self.open_pinned_resolution(pin_id, resolved)
    }

    /// Reopen an exact registered revision without consulting the active pointer.
    ///
    /// Long-running work captures the typed active pointer during selection,
    /// then uses this method for each later read. The current pointer is not
    /// consulted, so refresh cannot silently substitute a different package
    /// universe between selection and conversion.
    pub(crate) fn open_selected_profile(
        &self,
        selection: &ProfileRevisionSelection,
    ) -> Result<PinnedProfileCatalog> {
        let pin_id = uuid::Uuid::new_v4().to_string();
        let selection = selection.clone();
        let resolved = self.database_writer.execute(|| {
            let conn = open_runtime_db(&self.db_path).with_context(|| {
                format!(
                    "open Remi operational database for selected profile '{}' revision {}",
                    selection.source_profile, selection.profile_revision_sha256
                )
            })?;
            let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)
                .context("acquire exact catalog reader-pin transaction")?;
            let resolved = resolve_profile_selection(&tx, &self.catalog_dir, selection.clone())?;
            insert_reader_pin(&tx, &pin_id, &resolved.selection)?;
            tx.commit().context("commit exact catalog reader pin")?;
            Ok::<_, anyhow::Error>(resolved)
        })?;

        self.open_pinned_resolution(pin_id, resolved)
    }

    /// Reopen an exact registered revision without a durable reader pin.
    ///
    /// The caller must hold the exclusive canonical runtime-root lock for the
    /// complete lifetime of the returned reader. This is the stopped-runtime
    /// proof path; background GC therefore cannot race this immutable bundle.
    pub(crate) fn open_selected_profile_exclusively(
        &self,
        selection: &ProfileRevisionSelection,
    ) -> Result<PinnedProfileCatalog> {
        let conn = open_runtime_db(&self.db_path).with_context(|| {
            format!(
                "open Remi operational database for exclusive profile '{}' revision {}",
                selection.source_profile, selection.profile_revision_sha256
            )
        })?;
        open_resolved_profile(resolve_profile_selection(
            &conn,
            &self.catalog_dir,
            selection.clone(),
        )?)
    }

    fn open_pinned_resolution(
        &self,
        pin_id: String,
        resolved: ResolvedProfileCatalog,
    ) -> Result<PinnedProfileCatalog> {
        match self.open_cached_resolution(resolved) {
            Ok(mut pinned) => {
                pinned.pin = Some(ReaderPin {
                    db_path: self.db_path.clone(),
                    pin_id,
                    database_writer: self.database_writer.clone(),
                });
                Ok(pinned)
            }
            Err(error) => {
                release_reader_pin(&self.db_path, &pin_id, &self.database_writer);
                Err(error)
            }
        }
    }

    fn open_cached_resolution(
        &self,
        resolved: ResolvedProfileCatalog,
    ) -> Result<PinnedProfileCatalog> {
        let profile = resolved.selection.source_profile.clone();
        let revision = resolved.selection.profile_revision_sha256.clone();
        let mut cache = self.verified_readers.lock();
        let reader = match cache.get(&profile) {
            Some(cached) if cached.profile_revision_sha256 == revision => {
                Arc::clone(&cached.reader)
            }
            _ => {
                let reader = Arc::new(Mutex::new(
                    verify_profile_catalog_bundle(&resolved.bundle_path, &resolved.manifest)
                        .with_context(|| {
                            format!(
                                "verify selected profile catalog bundle {}",
                                resolved.bundle_path.display()
                            )
                        })?,
                ));
                cache.insert(
                    profile,
                    CachedProfileReader {
                        profile_revision_sha256: revision,
                        reader: Arc::clone(&reader),
                    },
                );
                reader
            }
        };
        Ok(PinnedProfileCatalog {
            selection: resolved.selection,
            manifest: resolved.manifest,
            reader: Some(reader),
            pin: None,
        })
    }

    pub(crate) fn remember_verified_profile_reader(
        &self,
        source_profile: &str,
        profile_revision_sha256: &str,
        reader: CatalogReader,
    ) {
        self.verified_readers.lock().insert(
            source_profile.to_string(),
            CachedProfileReader {
                profile_revision_sha256: profile_revision_sha256.to_string(),
                reader: Arc::new(Mutex::new(reader)),
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn has_verified_profile_reader_for_test(
        &self,
        source_profile: &str,
        profile_revision_sha256: &str,
    ) -> bool {
        self.verified_readers
            .lock()
            .get(source_profile)
            .is_some_and(|cached| cached.profile_revision_sha256 == profile_revision_sha256)
    }

    /// Alias kept intentionally small for callers that treat the authority as
    /// the profile reader itself.
    pub fn open(&self, source_profile: &str) -> Result<PinnedProfileCatalog> {
        self.open_active_profile(source_profile)
    }
}

/// An owned, exact-revision profile catalog reader.
///
/// `CatalogReader` is opened with SQLite's immutable, read-only URI. The
/// pointer and manifest are copied into this handle so the profile revision
/// identity remains available after operational SQLite changes.
pub struct PinnedProfileCatalog {
    selection: ProfileRevisionSelection,
    manifest: ProfileRevisionV2,
    reader: Option<Arc<Mutex<CatalogReader>>>,
    pin: Option<ReaderPin>,
}

struct ReaderPin {
    db_path: PathBuf,
    pin_id: String,
    database_writer: DatabaseWriter,
}

impl fmt::Debug for PinnedProfileCatalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedProfileCatalog")
            .field("selection", &self.selection)
            .field("manifest", &self.manifest)
            .field(
                "catalog_path",
                &self
                    .reader
                    .as_ref()
                    .map(|reader| reader.lock().path().to_path_buf()),
            )
            .field("reader_pin", &self.pin.as_ref().map(|pin| &pin.pin_id))
            .finish()
    }
}

impl PinnedProfileCatalog {
    #[cfg(test)]
    pub(crate) fn from_verified_test_parts(
        pointer: RemiActiveProfileRevision,
        manifest: ProfileRevisionV2,
        reader: CatalogReader,
    ) -> Self {
        Self {
            selection: ProfileRevisionSelection::from(&pointer),
            manifest,
            reader: Some(Arc::new(Mutex::new(reader))),
            pin: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn shares_verified_reader_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(
            self.reader.as_ref().expect("reader"),
            other.reader.as_ref().expect("reader"),
        )
    }

    /// The exact selected source profile.
    #[must_use]
    pub fn source_profile(&self) -> &str {
        &self.selection.source_profile
    }

    /// The exact content address of this profile revision's manifest.
    #[must_use]
    pub fn profile_revision_sha256(&self) -> &str {
        &self.selection.profile_revision_sha256
    }

    /// Short alias for [`Self::profile_revision_sha256`].
    #[must_use]
    pub fn revision_sha256(&self) -> &str {
        self.profile_revision_sha256()
    }

    /// The complete immutable revision identity that selected this handle.
    #[must_use]
    pub fn selection(&self) -> &ProfileRevisionSelection {
        &self.selection
    }

    /// The strictly typed profile revision manifest used to verify the bundle.
    #[must_use]
    pub fn manifest(&self) -> &ProfileRevisionV2 {
        &self.manifest
    }

    /// The verified immutable catalog reader.
    pub fn reader(&self) -> MutexGuard<'_, CatalogReader> {
        self.reader
            .as_ref()
            .expect("profile catalog reader is present before drop")
            .lock()
    }

    /// The canonicalized path of the verified catalog SQLite file.
    #[must_use]
    pub fn catalog_path(&self) -> PathBuf {
        self.reader().path().to_path_buf()
    }
}

impl Drop for PinnedProfileCatalog {
    fn drop(&mut self) {
        drop(self.reader.take());
        if let Some(pin) = self.pin.take() {
            release_reader_pin(&pin.db_path, &pin.pin_id, &pin.database_writer);
        }
    }
}

fn release_reader_pin(db_path: &Path, pin_id: &str, database_writer: &DatabaseWriter) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let db_path = db_path.to_path_buf();
        let pin_id = pin_id.to_string();
        let database_writer = database_writer.clone();
        runtime.spawn_blocking(move || {
            release_reader_pin_now(&db_path, &pin_id, &database_writer);
        });
        return;
    }
    release_reader_pin_now(db_path, pin_id, database_writer);
}

fn release_reader_pin_now(db_path: &Path, pin_id: &str, database_writer: &DatabaseWriter) {
    let result = database_writer.execute(|| {
        open_runtime_db(db_path).and_then(|conn| RemiProfileRevisionPin::release(&conn, pin_id))
    });
    match result {
        Ok(true) => {}
        Ok(false) => tracing::error!(pin_id, "immutable catalog reader pin was already absent"),
        Err(error) => tracing::error!(
            pin_id,
            error = %error,
            "failed to release immutable catalog reader pin"
        ),
    }
}

/// Resolve and verify an active profile using an already-open operational
/// SQLite connection.
///
/// This narrow helper keeps the authority testable without introducing a
/// second database-opening policy. It intentionally queries only pointer and
/// resource metadata; package rows in operational SQLite are never consulted.
#[cfg(test)]
fn open_active_profile_from_connection(
    conn: &Connection,
    catalog_dir: &Path,
    source_profile: &str,
) -> Result<PinnedProfileCatalog> {
    open_resolved_profile(resolve_active_profile(conn, catalog_dir, source_profile)?)
}

struct ResolvedProfileCatalog {
    selection: ProfileRevisionSelection,
    manifest: ProfileRevisionV2,
    bundle_path: PathBuf,
}

fn resolve_active_profile(
    conn: &Connection,
    catalog_dir: &Path,
    source_profile: &str,
) -> Result<ResolvedProfileCatalog> {
    let pointer = RemiActiveProfileRevision::find(conn, source_profile)
        .context("resolve active Remi profile revision pointer")?
        .ok_or_else(|| {
            anyhow::anyhow!("profile '{source_profile}' has no active immutable catalog revision")
        })?;

    if pointer.source_profile != source_profile {
        bail!(
            "active profile pointer names '{}' while resolving '{source_profile}'",
            pointer.source_profile
        );
    }

    resolve_profile_selection(conn, catalog_dir, ProfileRevisionSelection::from(&pointer))
}

fn resolve_profile_selection(
    conn: &Connection,
    catalog_dir: &Path,
    selection: ProfileRevisionSelection,
) -> Result<ResolvedProfileCatalog> {
    let resource = RemiCatalogResource::find_profile_revision(
        conn,
        &selection.source_profile,
        &selection.profile_revision_sha256,
    )
    .context("resolve selected profile catalog resource")?
    .ok_or_else(|| {
        anyhow::anyhow!(
            "selected profile '{}' revision {} has no catalog resource",
            selection.source_profile,
            selection.profile_revision_sha256
        )
    })?;

    if resource.kind != RemiCatalogResourceKind::ProfileRevision {
        bail!(
            "selected profile '{}' revision {} has resource kind {:?}",
            selection.source_profile,
            selection.profile_revision_sha256,
            resource.kind
        );
    }
    if !resource.durable {
        bail!(
            "selected profile '{}' revision {} is not durable",
            selection.source_profile,
            selection.profile_revision_sha256
        );
    }
    if resource.resource_sha256 != selection.profile_revision_sha256 {
        bail!(
            "selected profile '{}' and resource revision digests disagree",
            selection.source_profile
        );
    }
    if resource.source_profile != selection.source_profile {
        bail!(
            "selected profile revision {} belongs to '{}' instead of '{}'",
            selection.profile_revision_sha256,
            resource.source_profile,
            selection.source_profile
        );
    }

    let manifest = deserialize_profile_revision(&resource)
        .context("deserialize selected profile revision manifest")?;
    if manifest.profile != selection.source_profile {
        bail!(
            "selected profile revision {} names '{}' instead of '{}'",
            selection.profile_revision_sha256,
            manifest.profile,
            selection.source_profile
        );
    }

    let manifest_digest = manifest
        .manifest_sha256()
        .context("compute selected profile revision digest")?;
    if manifest_digest != selection.profile_revision_sha256
        || manifest_digest != resource.resource_sha256
    {
        bail!(
            "selected profile '{}' manifest and resource digests disagree",
            selection.source_profile
        );
    }
    if resource.artifact_sha256 != manifest.catalog.sha256 {
        bail!(
            "selected profile '{}' resource and manifest artifact digests disagree",
            selection.source_profile
        );
    }
    let manifest_artifact_size = i64::try_from(manifest.catalog.size)
        .context("profile catalog artifact size exceeds SQLite integer range")?;
    if resource.artifact_size != manifest_artifact_size {
        bail!(
            "selected profile '{}' resource and manifest artifact sizes disagree",
            selection.source_profile
        );
    }
    if resource.logical_digest_sha256 != manifest.logical_digest_sha256 {
        bail!(
            "selected profile '{}' resource and manifest logical digests disagree",
            selection.source_profile
        );
    }

    // The path is derived solely from the typed manifest profile and the exact
    // pointer digest. No path-like value is accepted from operational SQLite.
    let bundle_path = catalog_dir
        .join("profiles")
        .join(&manifest.profile)
        .join(&selection.profile_revision_sha256);

    Ok(ResolvedProfileCatalog {
        selection,
        manifest,
        bundle_path,
    })
}

fn insert_reader_pin(
    conn: &Connection,
    pin_id: &str,
    selection: &ProfileRevisionSelection,
) -> Result<()> {
    let runtime_session = RemiRuntimeSession::current(conn)
        .context("resolve current Remi runtime session for reader pin")?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cannot pin immutable profile reader without a current Remi runtime session"
            )
        })?;
    RemiProfileRevisionPin {
        pin_id: pin_id.to_string(),
        source_profile: selection.source_profile.clone(),
        profile_revision_sha256: selection.profile_revision_sha256.clone(),
        owner_kind: RemiRevisionPinKind::Reader,
        owner_identity: pin_id.to_string(),
        runtime_session_id: Some(runtime_session.session_id),
        pinned_at: unix_seconds()?,
    }
    .insert(conn)
    .context("pin exact immutable profile revision for reader lifetime")?;
    Ok(())
}

fn open_resolved_profile(resolved: ResolvedProfileCatalog) -> Result<PinnedProfileCatalog> {
    let ResolvedProfileCatalog {
        selection,
        manifest,
        bundle_path,
    } = resolved;
    let reader = verify_profile_catalog_bundle(&bundle_path, &manifest).with_context(|| {
        format!(
            "verify active profile catalog bundle {}",
            bundle_path.display()
        )
    })?;

    Ok(PinnedProfileCatalog {
        selection,
        manifest,
        reader: Some(Arc::new(Mutex::new(reader))),
        pin: None,
    })
}

fn inspect_resolved_profile_files(resolved: &ResolvedProfileCatalog) -> Result<()> {
    let directory_metadata = fs::symlink_metadata(&resolved.bundle_path).with_context(|| {
        format!(
            "inspect active profile catalog directory {}",
            resolved.bundle_path.display()
        )
    })?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        bail!(
            "active profile catalog {} must be a real directory",
            resolved.bundle_path.display()
        );
    }

    let mut names = fs::read_dir(&resolved.bundle_path)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    names.sort();
    let mut expected_names = [CATALOG_FILE_NAME, CATALOG_MANIFEST_FILE_NAME]
        .map(std::ffi::OsString::from)
        .to_vec();
    expected_names.sort();
    if names != expected_names {
        bail!(
            "active profile catalog {} contains an unexpected file set",
            resolved.bundle_path.display()
        );
    }

    let manifest_path = resolved.bundle_path.join(CATALOG_MANIFEST_FILE_NAME);
    let manifest_metadata = fs::symlink_metadata(&manifest_path)?;
    if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_file() {
        bail!(
            "active profile manifest {} must be a regular file",
            manifest_path.display()
        );
    }
    let expected_manifest = conary_core::json::canonical_json(&resolved.manifest)
        .map_err(anyhow::Error::msg)
        .context("canonicalize active profile manifest")?;
    if fs::read(&manifest_path)? != expected_manifest {
        bail!(
            "active profile manifest {} disagrees with operational pointer authority",
            manifest_path.display()
        );
    }

    let catalog_path = resolved.bundle_path.join(CATALOG_FILE_NAME);
    let catalog_metadata = fs::symlink_metadata(&catalog_path)?;
    if catalog_metadata.file_type().is_symlink() || !catalog_metadata.is_file() {
        bail!(
            "active profile catalog {} must be a regular file",
            catalog_path.display()
        );
    }
    if catalog_metadata.len() != resolved.manifest.catalog.size {
        bail!(
            "active profile catalog {} has {} bytes; expected {}",
            catalog_path.display(),
            catalog_metadata.len(),
            resolved.manifest.catalog.size
        );
    }
    Ok(())
}

fn unix_seconds() -> Result<i64> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system time precedes Unix epoch")?
        .as_secs();
    i64::try_from(seconds).context("system time exceeds SQLite integer range")
}

fn deserialize_profile_revision(resource: &RemiCatalogResource) -> Result<ProfileRevisionV2> {
    let manifest: ProfileRevisionV2 = serde_json::from_str(&resource.manifest_json)
        .context("parse ProfileRevisionV2 manifest JSON")?;
    manifest
        .validate()
        .context("validate ProfileRevisionV2 manifest")?;
    let canonical = conary_core::json::canonical_json(&manifest)
        .map_err(anyhow::Error::msg)
        .context("canonicalize ProfileRevisionV2 manifest")?;
    if canonical != resource.manifest_json.as_bytes() {
        bail!("profile revision manifest JSON is not canonical");
    }
    let raw_digest = conary_core::hash::sha256(resource.manifest_json.as_bytes());
    if raw_digest != resource.resource_sha256 {
        bail!(
            "profile revision manifest digest mismatch: expected {}, got {}",
            resource.resource_sha256,
            raw_digest
        );
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests;
