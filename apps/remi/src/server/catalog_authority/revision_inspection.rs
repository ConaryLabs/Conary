// apps/remi/src/server/catalog_authority/revision_inspection.rs

//! Upgrade and rebuild inspection for stored immutable profile revisions.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::db::models::{
    RemiActiveProfileRevision, RemiCatalogPhysicalAttestation, RemiCatalogResource,
    RemiCatalogResourceKind,
};
use conary_core::repository::catalog::{PROFILE_REVISION_SCHEMA_V3, ProfileRevisionV2};
use rusqlite::Connection;

use super::{CatalogAuthority, ProfileRevisionSelection, inspect_resolved_profile_files};

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

/// Upgrade-window inspection of a stored profile revision.
///
/// Current manifests remain strict. A retired schema is only enough authority
/// to decline reuse and rebuild; it never becomes a serving manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProfileRevisionInspection<T> {
    Current(T),
    ObsoleteSchema { found: u32, required: u32 },
}

pub(super) struct ResolvedProfileCatalog {
    pub(super) selection: ProfileRevisionSelection,
    pub(super) manifest: ProfileRevisionV2,
    pub(super) bundle_path: PathBuf,
    pub(super) physical_attestation: RemiCatalogPhysicalAttestation,
}

impl CatalogAuthority {
    /// Inspect an active revision while allowing refresh and deployment probes
    /// to classify a retired manifest as rebuildable state.
    pub(crate) fn inspect_active_profile_for_upgrade(
        &self,
        source_profile: &str,
    ) -> Result<ProfileRevisionInspection<ActiveProfileInspection>> {
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
        let resolved = resolve_profile_selection_for_upgrade(
            &conn,
            &self.catalog_dir,
            ProfileRevisionSelection::from(&pointer),
        )?;
        match resolved {
            ProfileRevisionInspection::Current(resolved) => {
                inspect_resolved_profile_files(&resolved)?;
                Ok(ProfileRevisionInspection::Current(
                    ActiveProfileInspection {
                        pointer,
                        manifest: resolved.manifest,
                    },
                ))
            }
            ProfileRevisionInspection::ObsoleteSchema { found, required } => {
                Ok(ProfileRevisionInspection::ObsoleteSchema { found, required })
            }
        }
    }

    /// Inspect one exact registered revision without opening or hashing its
    /// catalog contents. Callers may use these bounded facts to decide whether
    /// an exact registered, independently reopened bundle is eligible for
    /// immutable reuse.
    pub(crate) fn inspect_selected_profile(
        &self,
        selection: &ProfileRevisionSelection,
    ) -> Result<SelectedProfileInspection> {
        match self.inspect_selected_profile_for_upgrade(selection)? {
            ProfileRevisionInspection::Current(inspection) => Ok(inspection),
            ProfileRevisionInspection::ObsoleteSchema { found, required } => bail!(
                "selected profile revision schema {found} is obsolete; required schema is {required}"
            ),
        }
    }

    /// Inspect an exact registered revision for reuse or upgrade diagnostics.
    pub(crate) fn inspect_selected_profile_for_upgrade(
        &self,
        selection: &ProfileRevisionSelection,
    ) -> Result<ProfileRevisionInspection<SelectedProfileInspection>> {
        let flags =
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&self.db_path, flags).with_context(|| {
            format!(
                "open Remi operational database to inspect profile '{}' revision {}",
                selection.source_profile, selection.profile_revision_sha256
            )
        })?;
        match resolve_profile_selection_for_upgrade(&conn, &self.catalog_dir, selection.clone())? {
            ProfileRevisionInspection::Current(resolved) => {
                inspect_resolved_profile_files(&resolved)?;
                Ok(ProfileRevisionInspection::Current(
                    SelectedProfileInspection {
                        manifest: resolved.manifest,
                    },
                ))
            }
            ProfileRevisionInspection::ObsoleteSchema { found, required } => {
                Ok(ProfileRevisionInspection::ObsoleteSchema { found, required })
            }
        }
    }

    /// Classify a stored revision schema without touching catalog files.
    /// Complete deployment inspection uses this before deciding whether the
    /// registered bundle is current enough to reopen.
    pub(crate) fn classify_selected_profile_for_upgrade(
        &self,
        selection: &ProfileRevisionSelection,
    ) -> Result<ProfileRevisionInspection<SelectedProfileInspection>> {
        let flags =
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&self.db_path, flags).with_context(|| {
            format!(
                "open Remi operational database to classify profile '{}' revision {}",
                selection.source_profile, selection.profile_revision_sha256
            )
        })?;
        match resolve_profile_selection_for_upgrade(&conn, &self.catalog_dir, selection.clone())? {
            ProfileRevisionInspection::Current(resolved) => Ok(ProfileRevisionInspection::Current(
                SelectedProfileInspection {
                    manifest: resolved.manifest,
                },
            )),
            ProfileRevisionInspection::ObsoleteSchema { found, required } => {
                Ok(ProfileRevisionInspection::ObsoleteSchema { found, required })
            }
        }
    }
}

pub(super) fn resolve_profile_selection(
    conn: &Connection,
    catalog_dir: &Path,
    selection: ProfileRevisionSelection,
) -> Result<ResolvedProfileCatalog> {
    match resolve_profile_selection_for_upgrade(conn, catalog_dir, selection)? {
        ProfileRevisionInspection::Current(resolved) => Ok(resolved),
        ProfileRevisionInspection::ObsoleteSchema { found, required } => {
            bail!("profile revision schema {found} is obsolete; required schema is {required}")
        }
    }
}

fn resolve_profile_selection_for_upgrade(
    conn: &Connection,
    catalog_dir: &Path,
    selection: ProfileRevisionSelection,
) -> Result<ProfileRevisionInspection<ResolvedProfileCatalog>> {
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

    let manifest = match deserialize_profile_revision(&resource)
        .context("deserialize selected profile revision manifest")?
    {
        ProfileRevisionInspection::Current(manifest) => manifest,
        ProfileRevisionInspection::ObsoleteSchema { found, required } => {
            return Ok(ProfileRevisionInspection::ObsoleteSchema { found, required });
        }
    };
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

    Ok(ProfileRevisionInspection::Current(ResolvedProfileCatalog {
        selection,
        manifest,
        bundle_path,
        physical_attestation: resource.physical_attestation,
    }))
}

fn deserialize_profile_revision(
    resource: &RemiCatalogResource,
) -> Result<ProfileRevisionInspection<ProfileRevisionV2>> {
    let raw: serde_json::Value = serde_json::from_str(&resource.manifest_json)
        .context("parse profile revision manifest JSON")?;
    let canonical_raw = conary_core::json::canonical_json(&raw)
        .map_err(anyhow::Error::msg)
        .context("canonicalize profile revision manifest JSON")?;
    if canonical_raw != resource.manifest_json.as_bytes() {
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
    let schema = raw
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|schema| u32::try_from(schema).ok())
        .context("profile revision schema_version must be an unsigned 32-bit integer")?;
    if schema < PROFILE_REVISION_SCHEMA_V3 {
        return Ok(ProfileRevisionInspection::ObsoleteSchema {
            found: schema,
            required: PROFILE_REVISION_SCHEMA_V3,
        });
    }
    if schema > PROFILE_REVISION_SCHEMA_V3 {
        bail!(
            "profile revision schema {schema} is unsupported; expected {}",
            PROFILE_REVISION_SCHEMA_V3
        );
    }
    let manifest: ProfileRevisionV2 =
        serde_json::from_value(raw).context("parse current ProfileRevisionV2 manifest JSON")?;
    manifest
        .validate()
        .context("validate ProfileRevisionV2 manifest")?;
    let canonical = conary_core::json::canonical_json(&manifest)
        .map_err(anyhow::Error::msg)
        .context("canonicalize ProfileRevisionV2 manifest")?;
    if canonical != resource.manifest_json.as_bytes() {
        bail!("profile revision manifest JSON is not canonical");
    }
    Ok(ProfileRevisionInspection::Current(manifest))
}
