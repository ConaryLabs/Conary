// apps/remi/src/server/conversion/persistence.rs
//! Converted-package persistence and cache-hit reconstruction.

use super::lookup::PinnedConversionSource;
use super::storage::artifact::PublishedConversionArtifact;
use super::{ConversionService, ScriptletPackageMetadata, ServerConversionResult};
use anyhow::{Context, Result, anyhow, ensure};
use conary_core::ccs::convert::ConversionResult;
use conary_core::ccs::convert::ForeignConversionInput;
use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
use rusqlite::{Connection, TransactionBehavior};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::info;

pub(super) struct PersistConversionInput {
    pub(super) source_profile: String,
    pub(super) metadata: ForeignConversionInput,
    pub(super) format: &'static str,
    pub(super) source_checksum: String,
    pub(super) conversion_result: ConversionResult,
    /// Owns the pinned profile reader through the complete persistence call.
    pub(super) source: PinnedConversionSource,
    pub(super) profile_revision_sha256: String,
    pub(super) transport: conary_core::ccs::CcsTransportEnvelopeV1,
    /// Holds the private staging name and exact canonical read FD through DB commit.
    pub(super) artifact: PublishedConversionArtifact,
}

pub(super) struct PersistConversionOutput {
    pub(super) result: ServerConversionResult,
    pub(super) metrics: ConversionPersistenceMetrics,
}

#[derive(Debug, Default)]
pub(super) struct ConversionPersistenceMetrics {
    pub(super) database_persistence: Duration,
}

enum CacheInspection {
    Current(Box<ServerConversionResult>),
    Missing,
    Stale,
}

enum PersistOutcome {
    Existing(Box<ServerConversionResult>),
    Inserted,
}

impl ConversionService {
    fn inspect_cached_conversion(
        &self,
        conn: &Connection,
        source_profile: &str,
        profile_revision_sha256: &str,
        repo_pkg: &RepositoryPackage,
        source_checksum: &str,
    ) -> Result<CacheInspection> {
        let Some(existing) = ConvertedPackage::find_repository_by_checksum(
            conn,
            profile_revision_sha256,
            source_checksum,
        )?
        else {
            return Ok(CacheInspection::Missing);
        };

        let artifact = existing.repository_artifact()?;
        let existing_id = existing
            .id
            .context("repository conversion cache row has no durable identity")?;
        // A row with a missing or mismatched conversion pin is durable-state
        // corruption.  Never downgrade it to a cache miss and silently
        // replace the evidence.
        ConvertedPackage::require_conversion_pin(conn, existing_id)?;
        let expected_architecture = repo_pkg
            .architecture
            .as_deref()
            .context("repository conversion package has no exact architecture")?;
        let artifact_matches_package = artifact.source_profile == source_profile
            && artifact.package_name == repo_pkg.name
            && artifact.package_version == repo_pkg.version
            && artifact.package_architecture == expected_architecture;
        ensure!(
            artifact_matches_package,
            "conversion checksum maps to a conflicting catalog package identity"
        );
        let conversion_is_current =
            existing.repository_conversion_is_current_for_revision(profile_revision_sha256)?;
        let ccs_path = PathBuf::from(artifact.ccs_path);
        if conversion_is_current && ccs_path.exists() {
            return Ok(CacheInspection::Current(Box::new(
                self.build_result_from_existing(&existing)?,
            )));
        }

        Ok(CacheInspection::Stale)
    }

    pub(super) async fn cached_conversion_result_async(
        &self,
        source_profile: &str,
        repo_pkg: &RepositoryPackage,
        source_checksum: &str,
        profile_revision_sha256: &str,
    ) -> Result<Option<ServerConversionResult>> {
        let service = self.clone();
        let source_profile = source_profile.to_string();
        let repo_pkg = repo_pkg.clone();
        let source_checksum = source_checksum.to_string();
        let profile_revision_sha256 = profile_revision_sha256.to_string();

        tokio::task::spawn_blocking(move || {
            let mut conn = crate::server::open_runtime_db(&service.db_path)?;
            let tx = conn.transaction()?;
            let inspection = service.inspect_cached_conversion(
                &tx,
                &source_profile,
                &profile_revision_sha256,
                &repo_pkg,
                &source_checksum,
            )?;
            tx.commit()?;
            match inspection {
                CacheInspection::Current(result) => {
                    info!("Package already converted (checksum: {})", source_checksum);
                    return Ok(Some(*result));
                }
                CacheInspection::Missing => return Ok(None),
                CacheInspection::Stale => {}
            }

            let writer = service.database_writer.clone();
            writer.execute(|| {
                let mut conn = crate::server::open_runtime_db(&service.db_path)?;
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let inspection = service.inspect_cached_conversion(
                    &tx,
                    &source_profile,
                    &profile_revision_sha256,
                    &repo_pkg,
                    &source_checksum,
                )?;
                let result = match inspection {
                    CacheInspection::Current(result) => Some(*result),
                    CacheInspection::Missing => None,
                    CacheInspection::Stale => {
                        info!(
                            "Stale conversion record (CCS file missing or conversion input changed), re-converting"
                        );
                        if let Some(id) = ConvertedPackage::find_repository_by_checksum(
                            &tx,
                            &profile_revision_sha256,
                            &source_checksum,
                        )?
                        .and_then(|converted| converted.id)
                        {
                            ConvertedPackage::delete_with_conversion_pin_in_transaction(&tx, id)?;
                        }
                        None
                    }
                };
                tx.commit()?;
                Ok(result)
            })
        })
        .await
        .map_err(|e| anyhow!("conversion cache lookup task panicked: {e}"))?
    }

    pub(super) fn persist_conversion_result(
        &self,
        input: PersistConversionInput,
    ) -> Result<PersistConversionOutput> {
        let PersistConversionInput {
            source_profile,
            metadata,
            format,
            source_checksum,
            conversion_result,
            source,
            profile_revision_sha256,
            transport,
            mut artifact,
        } = input;
        ensure!(
            source.source_profile() == source_profile,
            "pinned conversion source profile contradicts persistence input"
        );
        ensure!(
            source.profile_revision_sha256() == profile_revision_sha256,
            "pinned conversion source revision contradicts persistence input"
        );
        let repo_pkg = source.repo_pkg.clone();
        let repository_provides_digest = source.catalog_provides_digest()?;

        artifact.require_publication_binding()?;
        let total_size = artifact.archive_bytes();
        let content_hash_text = format!("sha256:{}", artifact.archive_sha256());
        let persisted_total_size =
            i64::try_from(total_size).context("converted CCS size exceeds SQLite INTEGER range")?;

        let package_architecture = repo_pkg
            .architecture
            .clone()
            .context("pinned catalog package has no exact architecture identity")?;
        let final_ccs_path = artifact.path().to_path_buf();

        let mut converted = ConvertedPackage::new_repository(
            source_profile.clone(),
            profile_revision_sha256.clone(),
            repo_pkg.name.clone(),
            repo_pkg.version.clone(),
            package_architecture.clone(),
            format.to_string(),
            source_checksum.clone(),
            &transport,
            persisted_total_size,
            content_hash_text.clone(),
            final_ccs_path.to_string_lossy().to_string(),
            repository_provides_digest.clone(),
        );
        converted.set_scriptlet_metadata(&conversion_result.scriptlet_metadata)?;

        let database_started = Instant::now();
        // `source` owns the PinnedProfileCatalog. Keep it alive while this
        // transaction inserts the conversion row and its durable pin.
        let writer = self.database_writer.clone();
        let outcome = writer.execute(|| -> Result<PersistOutcome> {
            let mut conn = crate::server::open_runtime_db(&self.db_path)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            artifact.require_publication_binding()?;

            match self.inspect_cached_conversion(
                &tx,
                &source_profile,
                &profile_revision_sha256,
                &repo_pkg,
                &source_checksum,
            )? {
                CacheInspection::Current(result) => {
                    tx.commit()?;
                    return Ok(PersistOutcome::Existing(result));
                }
                CacheInspection::Stale => {
                    if let Some(id) = ConvertedPackage::find_repository_by_checksum(
                        &tx,
                        &profile_revision_sha256,
                        &source_checksum,
                    )?
                    .and_then(|existing| existing.id)
                    {
                        ConvertedPackage::delete_with_conversion_pin_in_transaction(&tx, id)?;
                    }
                }
                CacheInspection::Missing => {}
            }

            converted.insert_with_conversion_pin_in_transaction(&tx, unix_seconds()?)?;
            artifact.require_publication_binding()?;
            tx.commit()?;
            Ok(PersistOutcome::Inserted)
        })?;
        let database_persistence = database_started.elapsed();
        let metrics = ConversionPersistenceMetrics {
            database_persistence,
        };
        if let PersistOutcome::Existing(result) = outcome {
            let same_artifact = result.ccs_path == final_ccs_path
                && result.total_size == total_size
                && result.content_hash == content_hash_text;
            if result.ccs_path == final_ccs_path {
                ensure!(
                    same_artifact,
                    "current conversion row contradicts the exact published artifact"
                );
            }
            if same_artifact {
                artifact.retire_staging_after_commit();
            }
            return Ok(PersistConversionOutput {
                result: *result,
                metrics,
            });
        }
        artifact.retire_staging_after_commit();

        info!(
            "Recorded conversion in database (source_profile={}, name={}, version={})",
            source_profile,
            metadata.name(),
            metadata.version()
        );

        let scriptlet_summary = converted.scriptlet_summary()?;
        Ok(PersistConversionOutput {
            result: ServerConversionResult {
                name: metadata.name().to_string(),
                version: metadata.version().to_string(),
                source_profile: Some(source_profile),
                transport,
                total_size,
                content_hash: content_hash_text,
                ccs_path: final_ccs_path,
                cache_state: "cold".to_string(),
                scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
                timing: None,
            },
            metrics,
        })
    }

    /// Build a result from a current conversion record.
    fn build_result_from_existing(
        &self,
        existing: &ConvertedPackage,
    ) -> Result<ServerConversionResult> {
        let artifact = existing.repository_artifact()?;
        let scriptlet_summary = existing.scriptlet_summary()?;
        Ok(ServerConversionResult {
            name: artifact.package_name.to_string(),
            version: artifact.package_version.to_string(),
            source_profile: Some(artifact.source_profile.to_string()),
            transport: artifact.transport,
            total_size: artifact.total_size,
            content_hash: artifact.content_hash.to_string(),
            ccs_path: PathBuf::from(artifact.ccs_path),
            cache_state: "hot".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
            timing: None,
        })
    }
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
