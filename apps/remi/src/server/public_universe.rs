// apps/remi/src/server/public_universe.rs
//! Exact read authority selected by the signed endpoint-wide Remi universe.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use conary_core::repository::universe::RemiUniverseManifestV2;
use rusqlite::OptionalExtension;

use super::catalog_authority::ProfileRevisionSelection;

/// Stable identity shared by every projection of one public universe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublicUniverseIdentity {
    pub(crate) manifest_sha256: String,
    pub(crate) sequence: u64,
}

/// One immutable snapshot of the active signed public universe.
///
/// Operational profile pointers may move while a replacement universe is
/// being prepared. Public readers instead reopen the exact durable revisions
/// named by this snapshot.
#[derive(Debug, Clone)]
pub(crate) struct PublicUniverseSnapshot {
    identity: PublicUniverseIdentity,
    profiles: BTreeMap<String, ProfileRevisionSelection>,
}

impl PublicUniverseSnapshot {
    /// Load the active pointer and immutable manifest in one SQLite statement.
    /// An absent pointer is ordinary unavailability; malformed persisted
    /// authority is an error.
    pub(crate) fn load(db_path: &Path) -> Result<Option<Self>> {
        let conn = super::open_runtime_db(db_path)
            .context("open Remi operational database for public universe")?;
        let active = conn
            .query_row(
                "SELECT active.manifest_sha256, active.sequence, revision.manifest_json,
                        revision.durable
                 FROM remi_active_universe_revision active
                 LEFT JOIN remi_universe_revisions revision
                   ON revision.manifest_sha256 = active.manifest_sha256
                 WHERE active.singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((manifest_sha256, sequence, manifest_json, durable)) = active else {
            return Ok(None);
        };
        ensure!(
            durable == Some(1),
            "active Remi universe pointer does not name a durable immutable revision"
        );
        let manifest_json = manifest_json
            .context("active Remi universe pointer does not name an immutable manifest")?;

        let sequence = u64::try_from(sequence).context("active universe sequence is negative")?;
        let manifest = serde_json::from_str::<RemiUniverseManifestV2>(&manifest_json)
            .context("parse active Remi universe manifest")?;
        manifest
            .validate()
            .map_err(anyhow::Error::from)
            .context("validate active Remi universe manifest")?;
        ensure!(
            manifest.sequence == sequence && manifest.manifest_sha256()? == manifest_sha256,
            "active Remi universe pointer disagrees with its immutable manifest"
        );

        let persisted_members = {
            let mut statement = conn.prepare(
                "SELECT ordinal, source_profile, profile_revision_sha256
                 FROM remi_universe_profile_revisions
                 WHERE manifest_sha256 = ?1
                 ORDER BY ordinal",
            )?;
            statement
                .query_map([&manifest_sha256], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let manifest_members = manifest
            .profiles
            .iter()
            .map(|profile| {
                (
                    i64::from(profile.ordinal),
                    profile.revision.profile.clone(),
                    profile.profile_revision_sha256.clone(),
                )
            })
            .collect::<Vec<_>>();
        ensure!(
            persisted_members == manifest_members,
            "active Remi universe member rows disagree with its immutable manifest"
        );

        let profiles = manifest
            .profiles
            .into_iter()
            .map(|profile| {
                let source_profile = profile.revision.profile;
                (
                    source_profile.clone(),
                    ProfileRevisionSelection {
                        source_profile,
                        profile_revision_sha256: profile.profile_revision_sha256,
                    },
                )
            })
            .collect();
        Ok(Some(Self {
            identity: PublicUniverseIdentity {
                manifest_sha256,
                sequence,
            },
            profiles,
        }))
    }

    #[must_use]
    pub(crate) fn identity(&self) -> &PublicUniverseIdentity {
        &self.identity
    }

    #[must_use]
    pub(crate) fn profile(&self, source_profile: &str) -> Option<&ProfileRevisionSelection> {
        self.profiles.get(source_profile)
    }

    pub(crate) fn profiles(&self) -> impl ExactSizeIterator<Item = &ProfileRevisionSelection> + '_ {
        self.profiles.values()
    }
}
