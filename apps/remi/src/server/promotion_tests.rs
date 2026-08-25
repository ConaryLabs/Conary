// apps/remi/src/server/promotion_tests.rs

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;

    use conary_core::ccs::attestation::{
        BuildOutputIdentity, FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1, ForeignConversionBoundary,
    };
    use conary_core::ccs::{CcsTransportEnvelopeV1, CcsTransportObjectV1};
    use conary_core::db::models::{ConvertedPackage, MetadataTable, set_metadata};
    use conary_core::repository::catalog::{
        NATIVE_PARITY_COMPARISON_SCHEMA_V1, NATIVE_RESOLUTION_COMPARISON_SCHEMA_V1,
        NativeParityComparisonV1, NativeParityCountsV1, NativeResolutionComparisonV1,
        NativeResolutionCountsV1,
    };

    use super::*;
    use crate::server::catalog_authority::ProfileRevisionSelection;
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::conversion::{ScriptletPackageMetadata, ServerConversionResult};
    use crate::server::conversion_crawl::{
        CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1, CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
        CcsArtifactReopenProofV1, CcsTargetCompatibilityProofV1, ConversionCrawlPackageOutcomeV4,
        ConversionCrawlProfileV4, ConversionProofDispositionV1, ConversionProofStore,
        REMI_CONVERSION_CRAWL_SCHEMA_V4, ReopenedCcsArtifactEvidence,
        write_and_reopen_conversion_crawl,
    };
    use crate::server::promotion_evidence::{
        REMI_PROMOTION_EVIDENCE_SCHEMA_V1, RemiPromotionCanonicalMapV1,
        RemiPromotionProfileEvidenceV1,
    };
    use crate::server::signing_authority::ensure_universe_authority;

    struct PromotionFixture {
        catalogs: ActiveCatalogFixture,
        config: RemiPromotionActivationConfig,
        object_path: PathBuf,
    }

    impl PromotionFixture {
        fn new() -> Self {
            let catalogs = ActiveCatalogFixture::new();
            let root = catalogs
                .catalog_dir()
                .parent()
                .expect("promotion fixture root")
                .to_path_buf();
            let candidate_dir = root.join("universe-candidates");
            let chunk_dir = root.join("chunks");
            let keys_dir = root.join("repository-keys");
            fs::create_dir(&candidate_dir).expect("create promotion candidate directory");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&keys_dir)
                .expect("create promotion key directory");
            ensure_universe_authority(&keys_dir).expect("provision universe keys");

            let conn = catalogs.connection();
            set_metadata(&conn, MetadataTable::Server, "canonical_map_revision", "1")
                .expect("set canonical revision");
            set_metadata(
                &conn,
                MetadataTable::Server,
                "last_canonical_rebuild",
                "2026-08-25T00:00:00Z",
            )
            .expect("set canonical timestamp");
            drop(conn);

            let object_bytes = b"durable promotion object";
            let object_sha256 = conary_core::hash::sha256(object_bytes);
            let object_path = crate::server::handlers::cas_object_path(&chunk_dir, &object_sha256);
            fs::create_dir_all(object_path.parent().expect("CAS object parent"))
                .expect("create CAS object directory");
            fs::write(&object_path, object_bytes).expect("write durable CAS object");

            let evidence_dir = root.join("evidence");
            fs::create_dir(&evidence_dir).expect("create evidence directory");
            let crawl_path = evidence_dir.join("crawl.json");
            let evidence_path = evidence_dir.join("promotion.json");
            let mut crawl_profiles = Vec::new();
            let mut evidence_profiles = Vec::new();
            for (ordinal, profile) in conary_core::repository::supported_profiles::public_profiles()
                .iter()
                .enumerate()
            {
                let architecture = if profile.id() == "ubuntu-26.04" {
                    "amd64"
                } else {
                    "x86_64"
                };
                let mut input = package(
                    profile.id(),
                    "demo",
                    "1.0",
                    "1",
                    Some(architecture),
                    42,
                    profile.id(),
                );
                input.checksum = format!(
                    "sha256:{}",
                    conary_core::hash::sha256(profile.id().as_bytes())
                );
                let revision_sha256 = catalogs.candidate(
                    profile.id(),
                    i64::try_from(ordinal + 1).expect("fence fits i64"),
                    vec![input],
                );
                let pin = catalogs
                    .authority()
                    .open_selected_profile(&ProfileRevisionSelection {
                        source_profile: profile.id().to_string(),
                        profile_revision_sha256: revision_sha256.clone(),
                    })
                    .expect("open promotion candidate");
                let revision = pin.manifest().clone();
                let packages = pin.reader().packages().expect("read candidate packages");
                let candidate_package = packages.first().expect("fixture candidate package");
                let proof = install_conversion_proof(
                    &catalogs,
                    &revision_sha256,
                    candidate_package,
                    &object_sha256,
                    object_bytes,
                    &root,
                );
                crawl_profiles.push(ConversionCrawlProfileV4 {
                    profile: profile.id().to_string(),
                    profile_revision_sha256: revision_sha256.clone(),
                    expected_packages: 1,
                    outcomes: vec![ConversionCrawlPackageOutcomeV4 {
                        package_key_sha256: candidate_package.package_key_sha256.clone(),
                        name: candidate_package.name.clone(),
                        version: candidate_package.version.clone(),
                        package_release: candidate_package.package_release.clone(),
                        architecture: candidate_package.architecture.clone(),
                        repository_checksum: candidate_package.checksum.clone(),
                        state: ConversionCrawlOutcomeStateV4::Succeeded,
                        proof_disposition: Some(ConversionProofDispositionV1::Validated),
                        conversion_proof: Some(proof),
                        failure: None,
                    }],
                });
                let package_oracle_sha256 = conary_core::hash::sha256(
                    format!("package-oracle-{}", profile.id()).as_bytes(),
                );
                evidence_profiles.push(RemiPromotionProfileEvidenceV1 {
                    ordinal: u32::try_from(ordinal).expect("ordinal fits u32"),
                    profile: profile.id().to_string(),
                    profile_revision_sha256: revision_sha256.clone(),
                    catalog_sha256: revision.catalog.sha256.clone(),
                    catalog_size: revision.catalog.size,
                    package_parity: NativeParityComparisonV1 {
                        schema_version: NATIVE_PARITY_COMPARISON_SCHEMA_V1,
                        profile: profile.id().to_string(),
                        profile_revision_sha256: revision_sha256.clone(),
                        oracle_manifest_sha256: package_oracle_sha256.clone(),
                        counts: NativeParityCountsV1::from(revision.counts),
                    },
                    resolution_parity: NativeResolutionComparisonV1 {
                        schema_version: NATIVE_RESOLUTION_COMPARISON_SCHEMA_V1,
                        profile: profile.id().to_string(),
                        profile_revision_sha256: revision_sha256,
                        package_oracle_manifest_sha256: package_oracle_sha256,
                        oracle_manifest_sha256: conary_core::hash::sha256(
                            format!("native-resolution-{}", profile.id()).as_bytes(),
                        ),
                        candidate_manifest_sha256: conary_core::hash::sha256(
                            format!("candidate-resolution-{}", profile.id()).as_bytes(),
                        ),
                        counts: NativeResolutionCountsV1 {
                            roots: 1,
                            resolved_roots: 1,
                            unresolved_roots: 0,
                            closure_package_references: 1,
                            unresolved_dependencies: 0,
                        },
                    },
                });
            }
            let crawl = RemiConversionCrawlV4 {
                schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V4,
                profiles: crawl_profiles,
            };
            write_and_reopen_conversion_crawl(&crawl_path, &crawl)
                .expect("write complete promotion crawl");
            let canonical_map = {
                let conn = catalogs.connection();
                load_canonical_map_snapshot(&conn).expect("load fixture canonical map")
            };
            let canonical_bytes = canonical_bytes(&canonical_map).expect("canonical map bytes");
            let evidence = RemiPromotionEvidenceV1 {
                schema_version: REMI_PROMOTION_EVIDENCE_SCHEMA_V1,
                conversion_crawl_sha256: conary_core::hash::sha256(
                    &fs::read(&crawl_path).expect("read crawl bytes"),
                ),
                canonical_map: RemiPromotionCanonicalMapV1 {
                    sha256: conary_core::hash::sha256(&canonical_bytes),
                    revision: canonical_map.revision,
                    entry_count: u64::try_from(canonical_map.entries.len())
                        .expect("entry count fits u64"),
                },
                profiles: evidence_profiles,
            };
            fs::write(
                &evidence_path,
                conary_core::json::canonical_json(&evidence).expect("serialize promotion evidence"),
            )
            .expect("write promotion evidence");

            let config = RemiPromotionActivationConfig {
                db_path: catalogs.db_path().to_path_buf(),
                catalog_dir: catalogs.catalog_dir().to_path_buf(),
                catalog_candidate_dir: candidate_dir,
                chunk_dir,
                repository_keys_dir: keys_dir,
                promotion_evidence_path: evidence_path,
                conversion_crawl_path: crawl_path,
            };
            Self {
                catalogs,
                config,
                object_path,
            }
        }

        async fn activate(&self) -> Result<RemiPromotionActivationOutcome> {
            activate_remi_promotion(
                &self.config,
                &DatabaseWriter::default(),
                self.catalogs.authority(),
                None,
            )
            .await
        }

        fn plan(
            &self,
        ) -> (
            Vec<PromotionProfile>,
            CanonicalMapSnapshot,
            SignedUniverseCandidate,
            String,
            String,
        ) {
            let evidence = reopen_remi_promotion_evidence(&self.config.promotion_evidence_path)
                .expect("reopen fixture promotion evidence");
            let evidence_sha256 = conary_core::hash::sha256(
                &conary_core::json::canonical_json(&evidence)
                    .expect("canonical fixture promotion evidence"),
            );
            let (crawl, crawl_bytes) =
                reopen_conversion_crawl(&self.config.conversion_crawl_path)
                    .expect("reopen fixture crawl");
            let crawl_sha256 = conary_core::hash::sha256(&crawl_bytes);
            let conn = self.catalogs.connection();
            let canonical = load_canonical_map_snapshot(&conn).expect("load canonical map");
            let profiles = resolve_profiles(
                &conn,
                &self.config.catalog_dir,
                self.catalogs.authority(),
                &evidence,
                &crawl,
            )
            .expect("resolve fixture promotion profiles");
            drop(conn);
            let candidate = build_signed_candidate(
                0,
                &profiles,
                &canonical,
                &self.config.repository_keys_dir,
            )
            .expect("build fixture signed universe");
            (
                profiles,
                canonical,
                candidate,
                evidence_sha256,
                crawl_sha256,
            )
        }
    }

    fn install_conversion_proof(
        catalogs: &ActiveCatalogFixture,
        revision_sha256: &str,
        package: &conary_core::repository::catalog::CatalogPackageRecordV1,
        object_sha256: &str,
        object_bytes: &[u8],
        root: &Path,
    ) -> crate::server::conversion_crawl::ConversionProofV1 {
        let source_sha256 = package
            .checksum
            .strip_prefix("sha256:")
            .expect("fixture source checksum");
        let boundary = ForeignConversionBoundary {
            schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            source_format: conary_core::repository::supported_profiles::profile_by_public_id(
                &package.source_profile,
            )
            .expect("public fixture profile")
            .package_format()
            .as_str()
            .to_string(),
            source_checksum: package.checksum.clone(),
            output_identity: BuildOutputIdentity {
                file_merkle_root: "e".repeat(64),
                package_name: package.name.clone(),
                package_version: package.version.clone(),
                package_release: package.package_release.clone(),
                architecture: package.architecture.clone(),
                origin_class: "foreign_conversion".to_string(),
                hardening_level: "converted".to_string(),
                hermetic_evidence_hash: "f".repeat(64),
                canonical_content_identity: "1".repeat(64),
            },
            build_risk_report_hash: None,
            build_risk_report: None,
            scriptlet_risk_report_hash: None,
            scriptlet_risk_report: None,
            diagnostics: Vec::new(),
        };
        let transport = CcsTransportEnvelopeV1 {
            schema_version: conary_core::ccs::transport::CCS_TRANSPORT_SCHEMA_V1,
            manifest_base64: String::new(),
            signature_json: "{}".to_string(),
            debug_toml_base64: None,
            build_attestation_json: None,
            foreign_conversion_boundary_json: Some(
                serde_json::to_string(&boundary).expect("serialize fixture boundary"),
            ),
            objects: vec![CcsTransportObjectV1 {
                sha256: object_sha256.to_string(),
                size: u64::try_from(object_bytes.len()).expect("object size fits u64"),
            }],
        };
        let ccs_bytes = format!("fixture CCS for {}", package.source_profile).into_bytes();
        let ccs_sha256 = conary_core::hash::sha256(&ccs_bytes);
        let ccs_path = root.join(format!("{}.ccs", package.source_profile));
        fs::write(&ccs_path, &ccs_bytes).expect("write fixture CCS");
        let signer_sha256 = "3".repeat(64);
        let reopened = ReopenedCcsArtifactEvidence {
            source_artifact_sha256: source_sha256.to_string(),
            ccs_sha256: ccs_sha256.clone(),
            reopen_proof: CcsArtifactReopenProofV1 {
                schema_version: CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1,
                ccs_format_version: conary_core::ccs::v3::FORMAT_VERSION_V3,
                foreign_conversion_boundary_schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
                signer_public_key_sha256: signer_sha256.clone(),
                transport_sha256: conary_core::hash::sha256(
                    &conary_core::json::canonical_json(&transport)
                        .expect("serialize fixture transport"),
                ),
                verified_files: 1,
                verified_objects: 1,
            },
            target_compatibility_proofs: conary_core::ccs::supported_target_contracts()
                .iter()
                .map(|contract| CcsTargetCompatibilityProofV1 {
                    schema_version: CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                    ccs_sha256: ccs_sha256.clone(),
                    compatibility: conary_core::ccs::StaticTargetCompatibilityProofV1 {
                        schema_version:
                            conary_core::ccs::STATIC_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                        target_profile: contract.target_profile,
                        target_contract_sha256: contract.sha256().expect("target contract digest"),
                        required_capabilities: Vec::new(),
                        required_systemd_operations: Vec::new(),
                        required_linux_process_capabilities: Vec::new(),
                    },
                })
                .collect(),
        };
        let result = ServerConversionResult {
            name: package.name.clone(),
            version: package.version.clone(),
            source_profile: Some(package.source_profile.clone()),
            transport: transport.clone(),
            total_size: u64::try_from(ccs_bytes.len()).expect("CCS size fits u64"),
            content_hash: format!("sha256:{ccs_sha256}"),
            ccs_path: ccs_path.clone(),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata {
                scriptlet_fidelity: "native-free".to_string(),
                evidence_digest: None,
            },
            timing: None,
        };
        let original_format = conary_core::repository::supported_profiles::profile_by_public_id(
            &package.source_profile,
        )
        .expect("public fixture profile")
        .package_format()
        .as_str()
        .to_string();
        let conn = catalogs.connection();
        let mut converted = ConvertedPackage::new_repository(
            package.source_profile.clone(),
            revision_sha256.to_string(),
            package.name.clone(),
            package.version.clone(),
            package.architecture.clone().expect("fixture architecture"),
            original_format,
            package.checksum.clone(),
            &transport,
            i64::try_from(ccs_bytes.len()).expect("CCS size fits i64"),
            format!("sha256:{ccs_sha256}"),
            ccs_path.to_string_lossy().to_string(),
            conary_core::ccs::attestation::canonical_json_hash(&package.provides)
                .expect("provides digest"),
        );
        converted
            .set_scriptlet_metadata(&conary_core::ccs::convert::ScriptletBundleSummary::default())
            .expect("scriptlet summary");
        converted
            .insert_with_conversion_pin(&conn, 1)
            .expect("insert fixture conversion");
        drop(conn);
        ConversionProofStore::new(catalogs.db_path().to_path_buf(), DatabaseWriter::default())
            .publish(package, revision_sha256, &signer_sha256, &result, &reopened)
            .expect("publish fixture conversion proof")
    }

    #[tokio::test]
    async fn exact_promotion_activates_atomically_and_replays() {
        let fixture = PromotionFixture::new();
        let first = fixture.activate().await.expect("activate exact promotion");
        let RemiPromotionActivationOutcome::Activated {
            manifest_sha256,
            sequence: 1,
            promoted_profiles: 3,
            reopened_objects: 1,
        } = first
        else {
            panic!("initial promotion did not atomically activate")
        };
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_profile_revisions",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM repository_sync_runs WHERE state = 'published'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            3
        );
        let evidence_sha256 =
            conary_core::hash::sha256(&fs::read(&fixture.config.promotion_evidence_path).unwrap());
        let crawl_sha256 =
            conary_core::hash::sha256(&fs::read(&fixture.config.conversion_crawl_path).unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT manifest_sha256, sequence, promotion_evidence_sha256,
                        conversion_crawl_sha256
                 FROM remi_universe_revisions",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?
                )),
            )
            .unwrap(),
            (manifest_sha256.clone(), 1, evidence_sha256, crawl_sha256)
        );
        drop(conn);
        assert_eq!(
            fixture.activate().await.expect("replay exact promotion"),
            RemiPromotionActivationOutcome::AlreadyActive {
                manifest_sha256,
                sequence: 1,
            }
        );
    }

    #[tokio::test]
    async fn corrupt_cas_object_preserves_all_candidate_state() {
        let fixture = PromotionFixture::new();
        fs::write(&fixture.object_path, b"corrupt").expect("corrupt CAS object");
        let error = fixture.activate().await.expect_err("corrupt CAS must fail");
        assert!(format!("{error:#}").contains("size drifted"), "{error:#}");
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_profile_revisions",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM repository_sync_runs WHERE state = 'candidate'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_universe_revision",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn successor_candidate_fences_stale_promotion() {
        let fixture = PromotionFixture::new();
        fixture.catalogs.candidate(
            "fedora-44",
            10,
            vec![package(
                "fedora-44",
                "demo",
                "2.0",
                "1",
                Some("x86_64"),
                43,
                "successor",
            )],
        );
        let error = fixture
            .activate()
            .await
            .expect_err("stale promotion must fail");
        assert!(
            format!("{error:#}").contains("current candidate changed after promotion evidence"),
            "{error:#}"
        );
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_profile_revisions",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_universe_revision",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn signed_metadata_fault_preserves_private_candidates() {
        let fixture = PromotionFixture::new();
        let (_, _, mut candidate, _, _) = fixture.plan();
        candidate.timestamp_bytes = b"{}\n".to_vec();

        let error = publish_candidate_files(
            &fixture.config.catalog_candidate_dir,
            &fixture.config.catalog_dir,
            &candidate,
            Some(fixture.catalogs.authority()),
        )
        .expect_err("tampered signed metadata must fail");
        assert!(format!("{error:#}").contains("timestamp"), "{error:#}");
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM repository_sync_runs WHERE state = 'candidate'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_universe_revision",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn mid_transaction_candidate_loss_rolls_back_every_pointer() {
        let fixture = PromotionFixture::new();
        let (profiles, canonical, candidate, evidence_sha256, crawl_sha256) = fixture.plan();
        let bundle = publish_candidate_files(
            &fixture.config.catalog_candidate_dir,
            &fixture.config.catalog_dir,
            &candidate,
            Some(fixture.catalogs.authority()),
        )
        .expect("publish fixture signed bundle");
        let conn = fixture.catalogs.connection();
        conn.execute(
            "UPDATE repository_sync_runs SET state = 'failed',
                    failure_stage = 'publishing', failure_category = 'internal',
                    failure_evidence = 'injected transaction fault'
             WHERE source_profile = 'ubuntu-26.04' AND state = 'candidate'",
            [],
        )
        .expect("inject candidate loss");
        drop(conn);

        let error = activate_transaction(
            &fixture.config.db_path,
            &profiles,
            &candidate,
            &bundle,
            None,
            &evidence_sha256,
            &crawl_sha256,
            &canonical,
        )
        .expect_err("candidate loss must roll back activation");
        assert!(
            format!("{error:#}").contains("lost run"),
            "{error:#}"
        );
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_profile_revisions",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM repository_sync_runs WHERE state = 'candidate'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_universe_revision",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }
}
