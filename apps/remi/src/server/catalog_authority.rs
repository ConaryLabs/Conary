// apps/remi/src/server/catalog_authority.rs

//! Read-only authority for activated immutable Remi profile catalogs.
//!
//! Operational SQLite owns the fenced pointer and the exact resource metadata;
//! the package catalog itself is always read from the content-addressed bundle
//! below `ServerConfig::catalog_dir`.

use std::fmt;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::db::models::{
    RemiActiveProfileRevision, RemiCatalogResource, RemiCatalogResourceKind,
    RemiProfileRevisionPin, RemiRevisionPinKind, RemiRuntimeSession,
};
use conary_core::repository::catalog::{
    CatalogReader, ProfileRevisionV1, verify_profile_catalog_bundle,
};
use rusqlite::Connection;
use rusqlite::{Transaction, TransactionBehavior};

use super::database_writer::DatabaseWriter;
use super::open_runtime_db;

#[cfg(test)]
pub(crate) mod test_support;

/// The configured roots used to resolve one active immutable profile catalog.
#[derive(Clone)]
pub struct CatalogAuthority {
    db_path: PathBuf,
    catalog_dir: PathBuf,
    database_writer: DatabaseWriter,
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
        }
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
            let runtime_session = RemiRuntimeSession::current(&tx)
                .context("resolve current Remi runtime session for reader pin")?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "cannot pin immutable profile reader without a current Remi runtime session"
                    )
                })?;
            RemiProfileRevisionPin {
                pin_id: pin_id.clone(),
                source_profile: resolved.pointer.source_profile.clone(),
                profile_revision_sha256: resolved.pointer.profile_revision_sha256.clone(),
                owner_kind: RemiRevisionPinKind::Reader,
                owner_identity: pin_id.clone(),
                runtime_session_id: Some(runtime_session.session_id),
                pinned_at: unix_seconds()?,
            }
            .insert(&tx)
            .context("pin exact immutable profile revision for reader lifetime")?;
            tx.commit().context("commit immutable catalog reader pin")?;
            Ok::<_, anyhow::Error>(resolved)
        })?;

        match open_resolved_profile(resolved) {
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
    pointer: RemiActiveProfileRevision,
    manifest: ProfileRevisionV1,
    reader: Option<CatalogReader>,
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
            .field("pointer", &self.pointer)
            .field("manifest", &self.manifest)
            .field(
                "catalog_path",
                &self.reader.as_ref().map(CatalogReader::path),
            )
            .field("reader_pin", &self.pin.as_ref().map(|pin| &pin.pin_id))
            .finish()
    }
}

impl PinnedProfileCatalog {
    /// The exact source profile selected by the activation pointer.
    #[must_use]
    pub fn source_profile(&self) -> &str {
        &self.pointer.source_profile
    }

    /// The exact content address of this profile revision's manifest.
    #[must_use]
    pub fn profile_revision_sha256(&self) -> &str {
        &self.pointer.profile_revision_sha256
    }

    /// Short alias for [`Self::profile_revision_sha256`].
    #[must_use]
    pub fn revision_sha256(&self) -> &str {
        self.profile_revision_sha256()
    }

    /// The monotonic activation fence captured when this handle was opened.
    #[must_use]
    pub fn fencing_epoch(&self) -> i64 {
        self.pointer.fencing_epoch
    }

    /// The complete pointer identity that selected this handle.
    #[must_use]
    pub fn activation(&self) -> &RemiActiveProfileRevision {
        &self.pointer
    }

    /// The strictly typed profile revision manifest used to verify the bundle.
    #[must_use]
    pub fn manifest(&self) -> &ProfileRevisionV1 {
        &self.manifest
    }

    /// The verified immutable catalog reader.
    #[must_use]
    pub fn reader(&self) -> &CatalogReader {
        self.reader
            .as_ref()
            .expect("profile catalog reader is present before drop")
    }

    /// The canonicalized path of the verified catalog SQLite file.
    #[must_use]
    pub fn catalog_path(&self) -> &Path {
        self.reader().path()
    }
}

impl Deref for PinnedProfileCatalog {
    type Target = CatalogReader;

    fn deref(&self) -> &Self::Target {
        self.reader()
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
    pointer: RemiActiveProfileRevision,
    manifest: ProfileRevisionV1,
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

    let resource = RemiCatalogResource::find_profile_revision(
        conn,
        &pointer.source_profile,
        &pointer.profile_revision_sha256,
    )
    .context("resolve active profile catalog resource")?
    .ok_or_else(|| {
        anyhow::anyhow!(
            "active profile '{}' revision {} has no catalog resource",
            pointer.source_profile,
            pointer.profile_revision_sha256
        )
    })?;

    if resource.kind != RemiCatalogResourceKind::ProfileRevision {
        bail!(
            "active profile '{}' revision {} has resource kind {:?}",
            pointer.source_profile,
            pointer.profile_revision_sha256,
            resource.kind
        );
    }
    if !resource.durable {
        bail!(
            "active profile '{}' revision {} is not durable",
            pointer.source_profile,
            pointer.profile_revision_sha256
        );
    }
    if resource.resource_sha256 != pointer.profile_revision_sha256 {
        bail!(
            "active profile '{}' pointer and resource revision digests disagree",
            pointer.source_profile
        );
    }
    if resource.source_profile != pointer.source_profile {
        bail!(
            "active profile revision {} belongs to '{}' instead of '{}'",
            pointer.profile_revision_sha256,
            resource.source_profile,
            pointer.source_profile
        );
    }

    let manifest = deserialize_profile_revision(&resource)
        .context("deserialize active profile revision manifest")?;
    if manifest.profile != pointer.source_profile {
        bail!(
            "active profile revision {} names '{}' instead of '{}'",
            pointer.profile_revision_sha256,
            manifest.profile,
            pointer.source_profile
        );
    }

    let manifest_digest = manifest
        .manifest_sha256()
        .context("compute active profile revision digest")?;
    if manifest_digest != pointer.profile_revision_sha256
        || manifest_digest != resource.resource_sha256
    {
        bail!(
            "active profile '{}' manifest and resource digests disagree",
            pointer.source_profile
        );
    }
    if resource.artifact_sha256 != manifest.catalog.sha256 {
        bail!(
            "active profile '{}' resource and manifest artifact digests disagree",
            pointer.source_profile
        );
    }
    let manifest_artifact_size = i64::try_from(manifest.catalog.size)
        .context("profile catalog artifact size exceeds SQLite integer range")?;
    if resource.artifact_size != manifest_artifact_size {
        bail!(
            "active profile '{}' resource and manifest artifact sizes disagree",
            pointer.source_profile
        );
    }
    if resource.logical_digest_sha256 != manifest.logical_digest_sha256 {
        bail!(
            "active profile '{}' resource and manifest logical digests disagree",
            pointer.source_profile
        );
    }

    // The path is derived solely from the typed manifest profile and the exact
    // pointer digest. No path-like value is accepted from operational SQLite.
    let bundle_path = catalog_dir
        .join("profiles")
        .join(&manifest.profile)
        .join(&pointer.profile_revision_sha256);

    Ok(ResolvedProfileCatalog {
        pointer,
        manifest,
        bundle_path,
    })
}

fn open_resolved_profile(resolved: ResolvedProfileCatalog) -> Result<PinnedProfileCatalog> {
    let ResolvedProfileCatalog {
        pointer,
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
        pointer,
        manifest,
        reader: Some(reader),
        pin: None,
    })
}

fn unix_seconds() -> Result<i64> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system time precedes Unix epoch")?
        .as_secs();
    i64::try_from(seconds).context("system time exceeds SQLite integer range")
}

fn deserialize_profile_revision(resource: &RemiCatalogResource) -> Result<ProfileRevisionV1> {
    let manifest: ProfileRevisionV1 = serde_json::from_str(&resource.manifest_json)
        .context("parse ProfileRevisionV1 manifest JSON")?;
    manifest
        .validate()
        .context("validate ProfileRevisionV1 manifest")?;
    let canonical = conary_core::json::canonical_json(&manifest)
        .map_err(anyhow::Error::msg)
        .context("canonicalize ProfileRevisionV1 manifest")?;
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
