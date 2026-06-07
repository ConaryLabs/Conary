// apps/remi/src/server/conversion/persistence.rs
//! Converted-package persistence, cache-hit reconstruction, and publication outcomes.

use super::{ConversionService, ScriptletPackageMetadata, ServerConversionResult};
use crate::server::publication::{
    PublicationDecision, PublicationRefusal, ReviewArtifactInput, ServerConversionOutcome,
    classify_converted_package, decision_refusal, write_review_artifact,
};
use anyhow::{Result, anyhow};
use conary_core::ccs::convert::ConversionResult;
use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage, RepositoryPackage};
use conary_core::packages::common::PackageMetadata;
use std::path::PathBuf;
use tracing::info;

pub(super) struct PersistConversionInput {
    pub(super) distro: String,
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
        distro: &str,
        repo_pkg: &RepositoryPackage,
        original_checksum: &str,
    ) -> Result<Option<ServerConversionOutcome>> {
        let service = self.clone();
        let distro = distro.to_string();
        let repo_pkg = repo_pkg.clone();
        let original_checksum = original_checksum.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&service.db_path)?;
            let Some(existing) = ConvertedPackage::find_by_checksum(&conn, &original_checksum)?
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
                    .build_result_from_existing(&existing, &distro, &repo_pkg)
                    .map(Some);
            }

            info!(
                "Stale conversion record (CCS file missing or needs reconversion), re-converting"
            );
            ConvertedPackage::delete_by_checksum(&conn, &original_checksum)?;
            Ok(None)
        })
        .await
        .map_err(|e| anyhow!("conversion cache lookup task panicked: {e}"))?
    }

    pub(super) fn persist_conversion_result(
        &self,
        input: PersistConversionInput,
    ) -> Result<ServerConversionOutcome> {
        let PersistConversionInput {
            distro,
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

        let content_hash = Self::calculate_checksum(ccs_path)?;
        let total_size = std::fs::metadata(ccs_path)?.len();

        let package_architecture = repo_pkg
            .architecture
            .clone()
            .or_else(|| metadata.architecture.clone());
        let ccs_filename = Self::safe_ccs_filename_with_arch(
            &metadata.name,
            &metadata.version,
            package_architecture.as_deref(),
        )?;
        let final_ccs_path = self.cache_dir.join("packages").join(&ccs_filename);

        if let Some(parent) = final_ccs_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(ccs_path, &final_ccs_path)?;

        let mut converted = ConvertedPackage::new_server(
            distro.clone(),
            metadata.name.clone(),
            metadata.version.clone(),
            format.to_string(),
            original_checksum,
            conversion_result.fidelity.level.to_string(),
            &chunk_hashes,
            total_size as i64,
            content_hash.clone(),
            final_ccs_path.to_string_lossy().to_string(),
        );
        converted.detected_hooks = Some(serde_json::to_string(&conversion_result.detected_hooks)?);
        converted.set_scriptlet_metadata(&conversion_result.scriptlet_metadata)?;
        converted.package_architecture = package_architecture;
        let decision = classify_converted_package(&converted);
        if let Some(refusal) = decision_refusal(decision) {
            let mut report = match refusal {
                PublicationRefusal::ReviewRequired(report)
                | PublicationRefusal::Blocked(report) => report,
            };
            report.review_artifact_available = true;
            let conversion_fidelity = conversion_result.fidelity.level.to_string();
            let artifact_path = write_review_artifact(
                &self.cache_dir,
                ReviewArtifactInput {
                    distro: &distro,
                    package: &metadata.name,
                    version: &metadata.version,
                    architecture: converted.package_architecture.as_deref(),
                    original_format: &conversion_result.original_format,
                    conversion_fidelity: &conversion_fidelity,
                    conversion_version: CONVERSION_VERSION,
                    ccs_content_hash: &content_hash,
                    ccs_total_size: total_size,
                    publication: report,
                },
            )?;
            let mut summary = conversion_result.scriptlet_metadata.clone();
            summary.review_artifact_path = Some(artifact_path.to_string_lossy().to_string());
            converted.set_scriptlet_metadata(&summary)?;
        }
        converted.insert(&conn)?;

        info!(
            "Recorded conversion in database (distro={}, name={}, version={})",
            distro, metadata.name, metadata.version
        );

        let result = ServerConversionResult {
            name: metadata.name,
            version: metadata.version,
            distro: distro.clone(),
            chunk_hashes,
            total_size,
            content_hash,
            ccs_path: final_ccs_path,
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&converted.scriptlet_summary()),
            publication: None,
            timing: None,
        };
        Ok(Self::outcome_from_converted_result(&converted, result))
    }

    /// Build result from existing conversion record
    fn build_result_from_existing(
        &self,
        existing: &ConvertedPackage,
        distro: &str,
        repo_pkg: &RepositoryPackage,
    ) -> Result<ServerConversionOutcome> {
        // Use server-side fields from ConvertedPackage if available, else fallback to repo_pkg
        let name = existing
            .package_name
            .clone()
            .unwrap_or_else(|| repo_pkg.name.clone());
        let version = existing
            .package_version
            .clone()
            .unwrap_or_else(|| repo_pkg.version.clone());

        // Prefer stored CCS path if available
        let ccs_path = if let Some(stored_path) = &existing.ccs_path {
            PathBuf::from(stored_path)
        } else {
            let ccs_filename = Self::safe_ccs_filename_with_arch(
                &name,
                &version,
                existing.package_architecture.as_deref(),
            )?;
            self.cache_dir.join("packages").join(&ccs_filename)
        };

        // Parse chunk hashes from JSON if stored
        let chunk_hashes: Vec<String> = existing
            .chunk_hashes_json
            .as_ref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();

        let scriptlet_summary = existing.scriptlet_summary();

        let result = ServerConversionResult {
            name,
            version,
            distro: existing
                .distro
                .clone()
                .unwrap_or_else(|| distro.to_string()),
            chunk_hashes,
            total_size: u64::try_from(existing.total_size.unwrap_or(repo_pkg.size)).unwrap_or(0),
            content_hash: existing
                .content_hash
                .clone()
                .unwrap_or_else(|| existing.original_checksum.clone()),
            ccs_path,
            cache_state: "hot".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
            publication: None,
            timing: None,
        };
        Ok(Self::outcome_from_converted_result(existing, result))
    }

    fn outcome_from_converted_result(
        converted: &ConvertedPackage,
        mut result: ServerConversionResult,
    ) -> ServerConversionOutcome {
        match classify_converted_package(converted) {
            PublicationDecision::Ready => ServerConversionOutcome::Ready(result),
            PublicationDecision::ReviewRequired(report) => {
                result.publication = Some(report);
                ServerConversionOutcome::ReviewRequired(result)
            }
            PublicationDecision::Blocked(report) => {
                result.publication = Some(report);
                ServerConversionOutcome::Blocked(result)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        create_test_db, goal8a_scriptlet_summary, insert_package, insert_repo,
        make_conversion_result,
    };
    use super::*;
    use conary_core::ccs::convert::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
    use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
    use conary_core::packages::common::PackageMetadata;
    use std::path::PathBuf;

    #[test]
    fn test_build_result_from_existing_with_server_fields() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "nginx".to_string(),
            "1.24.0".to_string(),
            "rpm".to_string(),
            "sha256:orig".to_string(),
            "high".to_string(),
            &["chunk1".to_string(), "chunk2".to_string()],
            2048,
            "sha256:content_abc".to_string(),
            "/data/nginx.ccs".to_string(),
        );
        converted.insert(&conn).unwrap();

        let existing = ConvertedPackage::find_by_checksum(&conn, "sha256:orig")
            .unwrap()
            .unwrap();

        let repo_pkg = service
            .find_package(&conn, "fedora", "nginx", None, None)
            .unwrap();

        let outcome = service
            .build_result_from_existing(&existing, "fedora", &repo_pkg)
            .unwrap();
        let result = outcome.result();

        assert_eq!(result.name, "nginx");
        assert_eq!(result.version, "1.24.0");
        assert_eq!(result.distro, "fedora");
        assert_eq!(result.chunk_hashes, vec!["chunk1", "chunk2"]);
        assert_eq!(result.total_size, 2048);
        assert_eq!(result.content_hash, "sha256:content_abc");
        assert_eq!(result.ccs_path, PathBuf::from("/data/nginx.ccs"));
    }

    #[test]
    fn test_build_result_from_existing_without_chunk_hashes() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "curl", "8.5.0", 512);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        // Create a converted package with no chunk_hashes_json
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "curl".to_string(),
            "8.5.0".to_string(),
            "rpm".to_string(),
            "sha256:curl-orig".to_string(),
            "high".to_string(),
            &[],
            512,
            "sha256:curl-content".to_string(),
            "/data/curl.ccs".to_string(),
        );
        converted.insert(&conn).unwrap();

        let existing = ConvertedPackage::find_by_checksum(&conn, "sha256:curl-orig")
            .unwrap()
            .unwrap();

        let repo_pkg = service
            .find_package(&conn, "fedora", "curl", None, None)
            .unwrap();

        let outcome = service
            .build_result_from_existing(&existing, "fedora", &repo_pkg)
            .unwrap();
        let result = outcome.result();

        // Should return empty chunk list, not panic
        assert!(result.chunk_hashes.is_empty());
    }

    #[test]
    fn persisted_goal8a_golden_outcomes_respect_publication_gate() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("remi.db");
        conary_core::db::init(&db_path).unwrap();
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        let service = ConversionService::new(chunk_dir, cache_dir.clone(), db_path.clone(), None);

        let mut native_free = goal8a_scriptlet_summary("native-free", "source-native", "public");
        native_free.decision_counts = ScriptletDecisionCountsSummary::default();

        let mut fully_replaced =
            goal8a_scriptlet_summary("fully-replaced", "source-native", "public");
        fully_replaced.decision_counts = ScriptletDecisionCountsSummary {
            replaced: 2,
            ..ScriptletDecisionCountsSummary::default()
        };

        let mut legacy_replay =
            goal8a_scriptlet_summary("legacy-replay", "source-native", "private-review");
        legacy_replay.decision_counts = ScriptletDecisionCountsSummary {
            legacy: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        legacy_replay
            .review_reason_codes
            .push("legacy-replay-required".to_string());

        let mut review_required =
            goal8a_scriptlet_summary("review-required", "review-required", "private-review");
        review_required.decision_counts = ScriptletDecisionCountsSummary {
            review: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        review_required
            .review_reason_codes
            .push("review-class-deb-trigger".to_string());
        review_required
            .unknown_commands
            .push("custom-helper".to_string());

        let mut blocked = goal8a_scriptlet_summary("blocked", "blocked", "blocked");
        blocked.decision_counts = ScriptletDecisionCountsSummary {
            blocked: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        blocked
            .blocked_reason_codes
            .push("blocked-class-package-manager-recursion".to_string());
        blocked
            .blocked_classes
            .push("package-manager-recursion".to_string());

        let cases = [
            ("goal8a-native-free", native_free, "ready", true),
            ("goal8a-fully-replaced", fully_replaced, "ready", true),
            (
                "goal8a-legacy-replay",
                legacy_replay,
                "review-required",
                false,
            ),
            (
                "goal8a-review-required",
                review_required,
                "review-required",
                false,
            ),
            ("goal8a-blocked", blocked, "blocked", false),
        ];

        for (index, (name, summary, expected_outcome, public_ready)) in cases.iter().enumerate() {
            let output_ccs = temp.path().join("out").join(format!("{name}.ccs"));
            std::fs::create_dir_all(output_ccs.parent().unwrap()).unwrap();
            std::fs::write(&output_ccs, format!("ccs payload {name}")).unwrap();

            let metadata = PackageMetadata::new(
                PathBuf::from(format!("/tmp/{name}.rpm")),
                (*name).to_string(),
                "1.0".to_string(),
            );
            let mut result = make_conversion_result(Default::default());
            result.package_path = Some(output_ccs);
            result.scriptlet_metadata = summary.clone();

            let mut repo_pkg = RepositoryPackage::new(
                index as i64 + 1,
                (*name).to_string(),
                "1.0".to_string(),
                format!("sha256:{name}-source"),
                11,
                format!("https://example.invalid/{name}.rpm"),
            );
            repo_pkg.architecture = Some("x86_64".to_string());

            let outcome = service
                .persist_conversion_result(PersistConversionInput {
                    distro: "fedora".to_string(),
                    metadata,
                    format: "rpm",
                    original_checksum: format!("sha256:{name}-original"),
                    conversion_result: result,
                    repo_pkg: repo_pkg.clone(),
                    chunk_hashes: vec![format!("sha256:{name}-chunk")],
                })
                .unwrap();
            let observed_outcome = match &outcome {
                ServerConversionOutcome::Ready(_) => "ready",
                ServerConversionOutcome::ReviewRequired(_) => "review-required",
                ServerConversionOutcome::Blocked(_) => "blocked",
            };
            assert_eq!(observed_outcome, *expected_outcome, "{name}");

            let server_result = outcome.result();
            assert_eq!(
                server_result.scriptlets.scriptlet_fidelity, summary.scriptlet_fidelity,
                "{name}"
            );
            assert_eq!(
                server_result.scriptlets.publication_status, summary.publication_status,
                "{name}"
            );
            assert_eq!(server_result.publication.is_none(), *public_ready, "{name}");
            assert_eq!(
                server_result.scriptlets.review_artifact_available, !*public_ready,
                "{name}"
            );
            let public_metadata_json = serde_json::to_string(&server_result.scriptlets).unwrap();
            assert!(!public_metadata_json.contains("review_artifact_path"));
            assert!(!public_metadata_json.contains(cache_dir.to_str().unwrap()));
            if let Some(report) = &server_result.publication {
                let report_json = serde_json::to_string(report).unwrap();
                assert!(report.evidence_digest.is_some(), "{name}");
                assert!(!report_json.contains("review_artifact_path"));
                assert!(!report_json.contains(cache_dir.to_str().unwrap()));
                assert!(!report_json.contains("legacy_scriptlets"));
            }
        }

        let conn = conary_core::db::open(&db_path).unwrap();
        let candidates =
            ConvertedPackage::find_publication_candidates(&conn, "fedora", None).unwrap();
        assert_eq!(candidates.len(), cases.len());

        let public_ready_names: std::collections::BTreeSet<_> = candidates
            .iter()
            .filter(|converted| converted.is_scriptlet_public_ready())
            .map(|converted| converted.package_name.as_deref().unwrap())
            .collect();
        assert_eq!(
            public_ready_names,
            std::collections::BTreeSet::from(["goal8a-fully-replaced", "goal8a-native-free"])
        );

        for (name, summary, _expected_outcome, public_ready) in cases {
            let converted = ConvertedPackage::find_by_package_identity_with_arch(
                &conn,
                "fedora",
                name,
                Some("1.0"),
                Some("x86_64"),
            )
            .unwrap()
            .unwrap();
            assert_eq!(converted.scriptlet_fidelity, summary.scriptlet_fidelity);
            assert_eq!(converted.publication_status, summary.publication_status);
            assert_eq!(
                converted.is_scriptlet_public_ready(),
                public_ready,
                "{name}"
            );
            assert_eq!(
                converted.review_artifact_path.is_some(),
                !public_ready,
                "{name}"
            );
        }
    }

    #[test]
    fn persisted_conversion_records_scriptlet_metadata() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("remi.db");
        conary_core::db::init(&db_path).unwrap();
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        let output_ccs = temp.path().join("out/test.ccs");
        std::fs::create_dir_all(output_ccs.parent().unwrap()).unwrap();
        std::fs::write(&output_ccs, b"ccs payload").unwrap();
        let service = ConversionService::new(chunk_dir, cache_dir, db_path.clone(), None);
        let metadata = PackageMetadata::new(
            PathBuf::from("/tmp/test.rpm"),
            "test".to_string(),
            "1.0".to_string(),
        );
        let mut result = make_conversion_result(Default::default());
        result.package_path = Some(output_ccs);
        result.scriptlet_metadata = ScriptletBundleSummary {
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            publication_status: "private-review".to_string(),
            evidence_digest: Some(conary_core::hash::sha256_prefixed(b"remi-scriptlets")),
            blocked_reason_codes: vec!["blocked-class-network".to_string()],
            review_reason_codes: vec!["review-class-debconf".to_string()],
            unknown_commands: vec!["custom-helper".to_string()],
            blocked_classes: vec!["network".to_string()],
            review_artifact_path: Some("/tmp/review-artifact.json".to_string()),
            ..ScriptletBundleSummary::default()
        };
        let mut repo_pkg = RepositoryPackage::new(
            1,
            "test".to_string(),
            "1.0".to_string(),
            "sha256:repo".to_string(),
            11,
            "https://example.invalid/test.rpm".to_string(),
        );
        repo_pkg.architecture = Some("x86_64".to_string());
        let input = PersistConversionInput {
            distro: "fedora".to_string(),
            metadata,
            format: "rpm",
            original_checksum: "sha256:source".to_string(),
            conversion_result: result,
            repo_pkg: repo_pkg.clone(),
            chunk_hashes: vec!["sha256:chunk".to_string()],
        };

        let server_outcome = service.persist_conversion_result(input).unwrap();
        let server_result = server_outcome.result();

        assert_eq!(
            server_result.scriptlets.scriptlet_fidelity,
            "review-required"
        );
        assert!(server_result.scriptlets.review_artifact_available);
        let conn = conary_core::db::open(&db_path).unwrap();
        let converted = ConvertedPackage::find_by_package_identity_with_arch(
            &conn,
            "fedora",
            "test",
            Some("1.0"),
            Some("x86_64"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(converted.scriptlet_fidelity, "review-required");
        assert_eq!(converted.publication_status, "private-review");
        assert_eq!(
            converted.blocked_reason_codes_json,
            "[\"blocked-class-network\"]"
        );

        let hot = service
            .build_result_from_existing(&converted, "fedora", &repo_pkg)
            .unwrap();
        assert_eq!(
            hot.result().scriptlets.scriptlet_fidelity,
            "review-required"
        );
        assert!(hot.result().scriptlets.review_artifact_available);
    }
}
