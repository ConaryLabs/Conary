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
    RemiActiveProfileRevision, RemiCatalogPhysicalAttestation, RemiProfileRevisionPin,
    RemiRevisionPinKind, RemiRuntimeSession,
};
use conary_core::repository::catalog::{
    CATALOG_FILE_NAME, CatalogReader, ProfileRevisionV2,
    authenticate_registered_profile_catalog_layout, verify_registered_profile_catalog_bundle,
    verify_registered_profile_catalog_bundle_complete,
};
use parking_lot::{Mutex, MutexGuard};
use rusqlite::Connection;
use rusqlite::{Transaction, TransactionBehavior};

use super::database_writer::DatabaseWriter;
use super::open_runtime_db;

mod revision_inspection;
mod source;
#[cfg(test)]
#[path = "catalog_authority/tests/test_support.rs"]
pub(crate) mod test_support;

pub(crate) use revision_inspection::{ProfileRevisionInspection, SelectedProfileInspection};
use revision_inspection::{ResolvedProfileCatalog, resolve_profile_selection};

/// The configured roots used to resolve one active immutable profile catalog.
#[derive(Clone)]
pub struct CatalogAuthority {
    db_path: PathBuf,
    catalog_dir: PathBuf,
    database_writer: DatabaseWriter,
    verified_readers: Arc<Mutex<BTreeMap<String, CachedProfileReader>>>,
    verified_source_readers: Arc<Mutex<source::SourceReaderCache>>,
}

struct CachedProfileReader {
    profile_revision_sha256: String,
    physical_attestation: RemiCatalogPhysicalAttestation,
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
            verified_source_readers: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Build an authority for a process-external read-only inspection.
    pub(crate) fn for_inspection(
        db_path: impl Into<PathBuf>,
        catalog_dir: impl Into<PathBuf>,
    ) -> Self {
        Self::from_paths(db_path, catalog_dir, DatabaseWriter::default())
    }

    /// Resolve only the current operational pointer for one profile.
    ///
    /// This deliberately performs no catalog filesystem inspection or proof
    /// validation. Callers that need a reader must pass the returned exact
    /// selection through one of the registered catalog open methods.
    pub(crate) fn active_profile_selection(
        &self,
        source_profile: &str,
    ) -> Result<ProfileRevisionSelection> {
        let flags =
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&self.db_path, flags).with_context(|| {
            format!("open Remi operational database to select profile '{source_profile}'")
        })?;
        let pointer = RemiActiveProfileRevision::find(&conn, source_profile)
            .context("resolve active Remi profile revision pointer")?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "profile '{source_profile}' has no active immutable catalog revision"
                )
            })?;
        Ok(ProfileRevisionSelection::from(&pointer))
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

    /// Completely verify one exact registered revision for an explicit
    /// inspection or repair boundary. This authenticates the registered proof
    /// artifact, then independently hashes the complete catalog, runs SQLite
    /// integrity, validates the stored binding, and replays logical authority.
    /// Serving callers must use the portable registered reopen above.
    pub(crate) fn verify_selected_profile_complete(
        &self,
        selection: &ProfileRevisionSelection,
    ) -> Result<SelectedProfileInspection> {
        let flags =
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&self.db_path, flags).with_context(|| {
            format!(
                "open Remi operational database to completely inspect profile '{}' revision {}",
                selection.source_profile, selection.profile_revision_sha256
            )
        })?;
        let resolved = resolve_profile_selection(&conn, &self.catalog_dir, selection.clone())?;
        inspect_resolved_profile_files(&resolved)?;
        let reader = verify_registered_profile_catalog_bundle_complete(
            &resolved.bundle_path,
            &resolved.manifest,
            &resolved.physical_attestation.portable_manifest,
        )
        .with_context(|| {
            format!(
                "completely verify selected profile catalog bundle {}",
                resolved.bundle_path.display()
            )
        })?;
        drop(reader);
        Ok(SelectedProfileInspection {
            manifest: resolved.manifest,
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
        let mut pinned = self.open_selected_profiles(std::slice::from_ref(selection))?;
        pinned.pop().ok_or_else(|| {
            anyhow::anyhow!("selected profile batch returned no pinned catalog reader")
        })
    }

    /// Pin one complete exact revision set atomically before reopening any
    /// catalog bytes.
    ///
    /// This is the multi-profile export boundary: a slow verification of the
    /// first catalog cannot leave a later selected revision exposed to GC or
    /// candidate supersession before its reader pin exists.
    pub(crate) fn open_selected_profiles(
        &self,
        selections: &[ProfileRevisionSelection],
    ) -> Result<Vec<PinnedProfileCatalog>> {
        let requests = selections
            .iter()
            .map(|selection| (uuid::Uuid::new_v4().to_string(), selection.clone()))
            .collect::<Vec<_>>();
        let resolved = self.database_writer.execute(|| {
            let conn = open_runtime_db(&self.db_path).with_context(|| {
                "open Remi operational database for exact selected profile set".to_string()
            })?;
            let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)
                .context("acquire exact catalog reader-pin set transaction")?;
            let mut resolved = Vec::with_capacity(requests.len());
            for (pin_id, selection) in &requests {
                let profile = resolve_profile_selection(&tx, &self.catalog_dir, selection.clone())
                    .with_context(|| {
                        format!(
                            "resolve selected profile '{}' revision {} for atomic reader pinning",
                            selection.source_profile, selection.profile_revision_sha256
                        )
                    })?;
                insert_reader_pin(&tx, pin_id, &profile.selection)?;
                resolved.push((pin_id.clone(), profile));
            }
            tx.commit().context("commit exact catalog reader-pin set")?;
            Ok::<_, anyhow::Error>(resolved)
        })?;

        let mut opened = Vec::with_capacity(resolved.len());
        let mut remaining = resolved.into_iter();
        while let Some((pin_id, resolved)) = remaining.next() {
            let selection = resolved.selection.clone();
            match self.open_cached_resolution(resolved).with_context(|| {
                format!(
                    "open atomically pinned profile '{}' revision {}",
                    selection.source_profile, selection.profile_revision_sha256
                )
            }) {
                Ok(mut profile) => {
                    profile.pin = Some(self.reader_pin(pin_id));
                    opened.push(profile);
                }
                Err(error) => {
                    let mut release = vec![self.reader_pin(pin_id)];
                    release.extend(remaining.map(|(pin_id, _resolved)| self.reader_pin(pin_id)));
                    release.extend(opened.iter_mut().filter_map(|profile| profile.pin.take()));
                    drop(opened);
                    for pin in release {
                        release_reader_pin_now(&pin.db_path, &pin.pin_id, &pin.database_writer);
                    }
                    return Err(error);
                }
            }
        }
        Ok(opened)
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
                pinned.pin = Some(self.reader_pin(pin_id));
                Ok(pinned)
            }
            Err(error) => {
                release_reader_pin(&self.db_path, &pin_id, &self.database_writer);
                Err(error)
            }
        }
    }

    fn reader_pin(&self, pin_id: String) -> ReaderPin {
        ReaderPin {
            db_path: self.db_path.clone(),
            pin_id,
            database_writer: self.database_writer.clone(),
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
            Some(cached)
                if cached.profile_revision_sha256 == revision
                    && cached.physical_attestation == resolved.physical_attestation =>
            {
                let reader = Arc::clone(&cached.reader);
                drop(cache);
                authenticate_registered_profile_catalog_layout(
                    &resolved.bundle_path,
                    &resolved.manifest,
                    &resolved.physical_attestation.portable_manifest,
                )
                .with_context(|| {
                    format!(
                        "reauthenticate cached profile revision {revision} registered bundle layout and portable proof"
                    )
                })?;
                reader.lock().require_path_unchanged()?;
                reader
            }
            Some(cached) if cached.profile_revision_sha256 == revision => {
                bail!(
                    "cached profile revision {} portable attestation disagrees with its registered resource",
                    revision
                );
            }
            _ => {
                let reader = Arc::new(Mutex::new(
                    verify_registered_profile_catalog_bundle(
                        &resolved.bundle_path,
                        &resolved.manifest,
                        &resolved.physical_attestation.portable_manifest,
                    )
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
                        physical_attestation: resolved.physical_attestation.clone(),
                        reader: Arc::clone(&reader),
                    },
                );
                reader
            }
        };
        Ok(PinnedProfileCatalog {
            selection: resolved.selection,
            manifest: resolved.manifest,
            physical_attestation: resolved.physical_attestation,
            reader: Some(reader),
            pin: None,
        })
    }

    /// Drop the cache-owned reference for one exact profile bundle after
    /// filesystem removal. Any in-flight pinned reader retains its own `Arc`.
    pub(crate) fn evict_removed_profile_catalog(
        &self,
        source_profile: &str,
        profile_revision_sha256: &str,
    ) -> bool {
        let mut cache = self.verified_readers.lock();
        if cache
            .get(source_profile)
            .is_some_and(|cached| cached.profile_revision_sha256 == profile_revision_sha256)
        {
            cache.remove(source_profile);
            return true;
        }
        false
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
/// `CatalogReader` owns an authenticated portable-VFS connection over a
/// retained read-only descriptor. The pointer and manifest are copied into
/// this handle so the profile revision identity remains available after
/// operational SQLite changes.
pub struct PinnedProfileCatalog {
    selection: ProfileRevisionSelection,
    manifest: ProfileRevisionV2,
    physical_attestation: RemiCatalogPhysicalAttestation,
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
        let chunk_count =
            conary_core::repository::catalog::portable_chunk_count_v1(manifest.catalog.size)
                .expect("fixture catalog chunk count");
        let physical_attestation = RemiCatalogPhysicalAttestation::new(
            conary_core::repository::catalog::PortableManifestAttestationV1 {
                sha256: "a".repeat(64),
                size: conary_core::repository::catalog::portable_manifest_size_v1(chunk_count)
                    .expect("fixture portable manifest size"),
            },
            manifest.catalog.size,
        )
        .expect("fixture portable attestation");
        Self {
            selection: ProfileRevisionSelection::from(&pointer),
            manifest,
            physical_attestation,
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

    /// The exact persisted portable physical authority for this revision.
    #[must_use]
    pub fn physical_attestation(&self) -> &RemiCatalogPhysicalAttestation {
        &self.physical_attestation
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
        physical_attestation,
    } = resolved;
    let reader = verify_registered_profile_catalog_bundle(
        &bundle_path,
        &manifest,
        &physical_attestation.portable_manifest,
    )
    .with_context(|| {
        format!(
            "verify active profile catalog bundle {}",
            bundle_path.display()
        )
    })?;

    Ok(PinnedProfileCatalog {
        selection,
        manifest,
        physical_attestation,
        reader: Some(Arc::new(Mutex::new(reader))),
        pin: None,
    })
}

fn inspect_resolved_profile_files(resolved: &ResolvedProfileCatalog) -> Result<()> {
    authenticate_registered_profile_catalog_layout(
        &resolved.bundle_path,
        &resolved.manifest,
        &resolved.physical_attestation.portable_manifest,
    )
    .with_context(|| {
        format!(
            "authenticate active profile portable manifest in {}",
            resolved.bundle_path.display()
        )
    })?;

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

#[cfg(test)]
mod tests;
