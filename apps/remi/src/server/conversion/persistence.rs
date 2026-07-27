// apps/remi/src/server/conversion/persistence.rs
//! Converted-package persistence and cache-hit reconstruction.

use super::{ConversionService, ScriptletPackageMetadata, ServerConversionResult};
use anyhow::{Result, anyhow};
use conary_core::ccs::convert::ConversionResult;
use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
use conary_core::packages::common::PackageMetadata;
use std::path::PathBuf;
use tracing::info;

pub(super) struct PersistConversionInput {
    pub(super) source_profile: String,
    pub(super) metadata: PackageMetadata,
    pub(super) format: &'static str,
    pub(super) original_checksum: String,
    pub(super) conversion_result: ConversionResult,
    pub(super) repo_pkg: RepositoryPackage,
    pub(super) chunk_hashes: Vec<String>,
}

impl ConversionService {
    pub(super) async fn cached_conversion_result_async(
        &self,
        source_profile: &str,
        repo_pkg: &RepositoryPackage,
        original_checksum: &str,
    ) -> Result<Option<ServerConversionResult>> {
        let service = self.clone();
        let source_profile = source_profile.to_string();
        let repo_pkg = repo_pkg.clone();
        let original_checksum = original_checksum.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&service.db_path)?;
            let Some(existing) = ConvertedPackage::find_repository_by_checksum(
                &conn,
                &source_profile,
                &original_checksum,
            )?
            else {
                return Ok(None);
            };

            let ccs_filename = Self::safe_ccs_filename_with_arch(
                &repo_pkg.name,
                &repo_pkg.version,
                repo_pkg.architecture.as_deref(),
            )?;
            let ccs_path = service.cache_dir.join("packages").join(&ccs_filename);
            if !existing.needs_reconversion() && ccs_path.exists() {
                info!(
                    "Package already converted (checksum: {})",
                    original_checksum
                );
                return service
                    .build_result_from_existing(&existing, &repo_pkg)
                    .map(Some);
            }

            info!(
                "Stale conversion record (CCS file missing or needs reconversion), re-converting"
            );
            ConvertedPackage::delete_repository_by_checksum(
                &conn,
                &source_profile,
                &original_checksum,
            )?;
            Ok(None)
        })
        .await
        .map_err(|e| anyhow!("conversion cache lookup task panicked: {e}"))?
    }

    pub(super) fn persist_conversion_result(
        &self,
        input: PersistConversionInput,
    ) -> Result<ServerConversionResult> {
        let PersistConversionInput {
            source_profile,
            metadata,
            format,
            original_checksum,
            conversion_result,
            repo_pkg,
            chunk_hashes,
        } = input;

        let conn = conary_core::db::open(&self.db_path)?;
        let ccs_path = conversion_result
            .package_path
            .as_ref()
            .ok_or_else(|| anyhow!("No CCS package path"))?;

        let content_hash = Self::calculate_sha256(ccs_path)?.to_prefixed_string();
        let total_size = std::fs::metadata(ccs_path)?.len();

        let package_architecture = repo_pkg
            .architecture
            .clone()
            .or_else(|| metadata.architecture.clone())
            .ok_or_else(|| anyhow!("converted package has no exact architecture identity"))?;
        let ccs_filename = Self::safe_ccs_filename_with_arch(
            &metadata.name,
            &metadata.version,
            Some(&package_architecture),
        )?;
        let final_ccs_path = self.cache_dir.join("packages").join(&ccs_filename);

        if let Some(parent) = final_ccs_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(ccs_path, &final_ccs_path)?;

        let mut converted = ConvertedPackage::new_repository(
            source_profile.clone(),
            metadata.name.clone(),
            metadata.version.clone(),
            package_architecture.clone(),
            format.to_string(),
            original_checksum,
            &chunk_hashes,
            total_size as i64,
            content_hash.clone(),
            final_ccs_path.to_string_lossy().to_string(),
        );
        converted.set_scriptlet_metadata(&conversion_result.scriptlet_metadata)?;
        converted.insert(&conn)?;

        info!(
            "Recorded conversion in database (source_profile={}, name={}, version={})",
            source_profile, metadata.name, metadata.version
        );

        let scriptlet_summary = converted.scriptlet_summary()?;
        Ok(ServerConversionResult {
            name: metadata.name,
            version: metadata.version,
            source_profile: Some(source_profile),
            chunk_hashes,
            total_size,
            content_hash,
            ccs_path: final_ccs_path,
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
            timing: None,
        })
    }

    /// Build a result from a current conversion record.
    fn build_result_from_existing(
        &self,
        existing: &ConvertedPackage,
        _repo_pkg: &RepositoryPackage,
    ) -> Result<ServerConversionResult> {
        let artifact = existing.repository_artifact()?;
        let scriptlet_summary = existing.scriptlet_summary()?;

        Ok(ServerConversionResult {
            name: artifact.package_name.to_string(),
            version: artifact.package_version.to_string(),
            source_profile: Some(artifact.source_profile.to_string()),
            chunk_hashes: artifact.chunk_hashes,
            total_size: artifact.total_size,
            content_hash: artifact.content_hash.to_string(),
            ccs_path: PathBuf::from(artifact.ccs_path),
            cache_state: "hot".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
            timing: None,
        })
    }
}
