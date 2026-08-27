// apps/remi/src/server/catalog_authority/test_support.rs

//! Current-schema immutable catalog fixtures shared by Remi reader tests.

use std::path::{Path, PathBuf};

use conary_core::db::models::{
    RemiCatalogResource, RemiCatalogResourceKind, RemiProfileRevisionMember, RemiRuntimeSession,
};
use conary_core::repository::catalog::{
    CATALOG_CONTENT_SCHEMA_V1, CATALOG_FILE_NAME, CatalogArtifactV1, CatalogContentV1,
    CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogScopeV1, CatalogSourceEvidenceV1,
    PROFILE_REVISION_SCHEMA_V2, ProfileRevisionV2, ProfileSourceMemberV2,
    SOURCE_METADATA_DIRECTORY_NAME, SOURCE_SNAPSHOT_SCHEMA_V1, SourceEcosystemV1,
    SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1, SourceSnapshotV1,
    SourceStreamKindV1, SourceStreamV1, publish_profile_catalog_bundle,
    publish_source_catalog_bundle, write_catalog_candidate, write_profile_catalog_manifest,
    write_source_catalog_manifest,
};
use conary_core::repository::dependency_model::DebianMultiArch;
use conary_core::repository::supported_profiles::ProfilePackageFormat;
use conary_core::repository::universe::{
    REMI_UNIVERSE_SCHEMA_V2, RemiUniverseCanonicalMapObjectV2, RemiUniverseCatalogObjectV2,
    RemiUniverseManifestV2, RemiUniverseProfileV2,
};
use conary_core::repository::versioning::VersionScheme;
use conary_core::repository::{
    ArchKeyringFormat, ArchKeyringTrust, ArchSigLevel, OpenPgpTrustRoot, RepositoryParserConfig,
    RepositoryTrustPolicy, RpmMetadataAuthority,
};

use super::CatalogAuthority;
use crate::server::database_writer::DatabaseWriter;

pub(crate) struct ActiveCatalogFixture {
    root: tempfile::TempDir,
    db_path: PathBuf,
    catalog_dir: PathBuf,
    authority: CatalogAuthority,
}

impl ActiveCatalogFixture {
    pub(crate) fn new() -> Self {
        let root = tempfile::tempdir().expect("create active catalog fixture");
        let db_path = root.path().join("metadata/conary.db");
        let catalog_dir = root.path().join("catalogs");
        std::fs::create_dir_all(db_path.parent().expect("metadata parent"))
            .expect("create metadata directory");
        std::fs::create_dir_all(&catalog_dir).expect("create catalog directory");
        conary_core::db::init(&db_path).expect("initialize current schema");
        let conn = conary_core::db::open_fast(&db_path).expect("open fixture database");
        RemiRuntimeSession::begin(&conn, 1).expect("install fixture runtime session");
        let database_writer = DatabaseWriter::default();
        let authority =
            CatalogAuthority::from_paths(db_path.clone(), catalog_dir.clone(), database_writer);
        Self {
            root,
            db_path,
            catalog_dir,
            authority,
        }
    }

    pub(crate) fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub(crate) fn authority(&self) -> &CatalogAuthority {
        &self.authority
    }

    pub(crate) fn catalog_dir(&self) -> &Path {
        &self.catalog_dir
    }

    pub(crate) fn connection(&self) -> rusqlite::Connection {
        conary_core::db::open_fast(&self.db_path).expect("open fixture database")
    }

    pub(crate) fn activate(
        &self,
        profile: &str,
        fencing_epoch: i64,
        packages: Vec<CatalogPackageRecordV1>,
    ) -> String {
        self.publish_revision(profile, fencing_epoch, packages, true, false)
    }

    pub(crate) fn register(
        &self,
        profile: &str,
        fencing_epoch: i64,
        packages: Vec<CatalogPackageRecordV1>,
    ) -> String {
        self.publish_revision(profile, fencing_epoch, packages, false, false)
    }

    pub(crate) fn candidate(
        &self,
        profile: &str,
        fencing_epoch: i64,
        packages: Vec<CatalogPackageRecordV1>,
    ) -> String {
        self.publish_revision(profile, fencing_epoch, packages, false, true)
    }

    /// Bind every currently active public profile into one immutable universe.
    /// Candidate-tier profiles are deliberately excluded by typed support tier.
    pub(crate) fn activate_universe(&self, sequence: u64) -> String {
        let conn = self.connection();
        let mut statement = conn
            .prepare(
                "SELECT resource.manifest_json
                 FROM remi_active_profile_revisions active
                 JOIN remi_catalog_resources resource
                   ON resource.resource_sha256 = active.profile_revision_sha256
                 WHERE resource.resource_kind = 'profile_revision' AND resource.durable = 1
                 ORDER BY active.source_profile",
            )
            .expect("prepare active fixture profile query");
        let mut revisions = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query active fixture profiles")
            .map(|row| {
                serde_json::from_str::<ProfileRevisionV2>(
                    &row.expect("read active fixture profile manifest"),
                )
                .expect("parse active fixture profile manifest")
            })
            .filter(|revision| {
                conary_core::repository::supported_profiles::profile_by_public_id(&revision.profile)
                    .is_some()
            })
            .collect::<Vec<_>>();
        revisions.sort_by(|left, right| left.profile.cmp(&right.profile));
        let profiles = revisions
            .into_iter()
            .enumerate()
            .map(|(ordinal, revision)| RemiUniverseProfileV2 {
                ordinal: u32::try_from(ordinal).expect("fixture universe ordinal fits u32"),
                profile_revision_sha256: revision
                    .manifest_sha256()
                    .expect("hash fixture profile revision"),
                catalog: RemiUniverseCatalogObjectV2 {
                    schema_version: CATALOG_CONTENT_SCHEMA_V1,
                    sha256: revision.catalog.sha256.clone(),
                    size: revision.catalog.size,
                    logical_digest_sha256: revision.logical_digest_sha256.clone(),
                },
                revision,
            })
            .collect();
        let generated_at = chrono::Utc::now();
        let manifest = RemiUniverseManifestV2 {
            schema_version: REMI_UNIVERSE_SCHEMA_V2,
            sequence,
            metadata_root_sha256: conary_core::hash::sha256(b"fixture universe root"),
            generated_at,
            expires_at: generated_at + chrono::Duration::days(7),
            profiles,
            canonical_map: RemiUniverseCanonicalMapObjectV2 {
                schema_version: conary_core::canonical::CANONICAL_MAP_SCHEMA_VERSION,
                sha256: conary_core::hash::sha256(b"fixture canonical map"),
                size: 0,
                revision: 0,
                entry_count: 0,
            },
        };
        manifest
            .validate()
            .expect("validate fixture public universe");
        let manifest_sha256 = manifest
            .manifest_sha256()
            .expect("hash fixture public universe");
        let sequence = i64::try_from(sequence).expect("fixture universe sequence fits i64");
        let manifest_json = String::from_utf8(
            conary_core::json::canonical_json(&manifest)
                .expect("serialize fixture public universe"),
        )
        .expect("fixture public universe JSON is UTF-8");
        conn.execute(
            "INSERT INTO remi_universe_revisions (
                 manifest_sha256, sequence, promotion_evidence_sha256,
                 conversion_crawl_sha256, metadata_root_sha256,
                 canonical_map_sha256, canonical_map_size, targets_version,
                 snapshot_version, timestamp_version, manifest_json, durable, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?2, ?2, ?2, ?7, 1, ?2)",
            rusqlite::params![
                &manifest_sha256,
                sequence,
                conary_core::hash::sha256(b"fixture promotion evidence"),
                conary_core::hash::sha256(b"fixture conversion crawl"),
                &manifest.metadata_root_sha256,
                &manifest.canonical_map.sha256,
                manifest_json,
            ],
        )
        .expect("insert fixture universe revision");
        for profile in &manifest.profiles {
            conn.execute(
                "INSERT INTO remi_universe_profile_revisions (
                     manifest_sha256, ordinal, source_profile, profile_revision_sha256,
                     catalog_sha256, catalog_size
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    &manifest_sha256,
                    i64::from(profile.ordinal),
                    &profile.revision.profile,
                    &profile.profile_revision_sha256,
                    &profile.catalog.sha256,
                    i64::try_from(profile.catalog.size).expect("fixture catalog size fits i64"),
                ],
            )
            .expect("insert fixture universe profile");
        }
        conn.execute(
            "INSERT INTO remi_active_universe_revision (
                 singleton, manifest_sha256, sequence, activated_at
             ) VALUES (1, ?1, ?2, ?2)
             ON CONFLICT(singleton) DO UPDATE SET
                 manifest_sha256 = excluded.manifest_sha256,
                 sequence = excluded.sequence,
                 activated_at = excluded.activated_at",
            rusqlite::params![&manifest_sha256, sequence],
        )
        .expect("activate fixture public universe");
        manifest_sha256
    }

    fn publish_revision(
        &self,
        profile: &str,
        fencing_epoch: i64,
        packages: Vec<CatalogPackageRecordV1>,
        activate: bool,
        candidate: bool,
    ) -> String {
        let (parser_config, trust_policy, ecosystem, evidence_role) =
            source_fixture_authority(profile);
        let parser_config_sha256 = conary_core::hash::sha256(
            &conary_core::json::canonical_json(&parser_config)
                .expect("serialize source parser fixture"),
        );
        let trust_policy_sha256 = conary_core::hash::sha256(
            &conary_core::json::canonical_json(&trust_policy)
                .expect("serialize source trust fixture"),
        );
        let source_stream = SourceStreamV1 {
            kind: SourceStreamKindV1::Release,
            identity: "stable".to_string(),
        };
        let mut member_contracts =
            conary_core::repository::supported_profiles::profile_by_id(profile)
                .map(|profile| {
                    profile
                        .members()
                        .iter()
                        .map(|member| {
                            (
                                member.repository_identity.clone(),
                                member.role,
                                member.precedence,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| {
                    vec![(
                        format!("repository-{profile}"),
                        conary_core::repository::supported_profiles::ProfileSourceRole::Base,
                        0,
                    )]
                });
        member_contracts.sort_by_key(|member| std::cmp::Reverse(member.2));

        let mut source_manifests = Vec::with_capacity(member_contracts.len());
        let mut members = Vec::with_capacity(member_contracts.len());
        let mut evidence = Vec::with_capacity(member_contracts.len());
        for (index, (repository_identity, role, precedence)) in
            member_contracts.into_iter().enumerate()
        {
            let ordinal = u32::try_from(index).expect("fixture member ordinal fits u32");
            let source_identity = format!("source-{repository_identity}");
            let source_object_bytes =
                format!("source-object-{profile}-{repository_identity}-{fencing_epoch}")
                    .into_bytes();
            let source_object = SourceMetadataObjectV1 {
                role: evidence_role.clone(),
                source_path: format!("metadata/{repository_identity}"),
                sha256: conary_core::hash::sha256(&source_object_bytes),
                size: source_object_bytes.len() as u64,
            };
            let source_packages = if ordinal == 0 {
                packages
                    .iter()
                    .cloned()
                    .map(|mut package| {
                        package.package_key_sha256.clear();
                        package.origin = CatalogPackageOriginV1::Source {
                            source_identity: source_identity.clone(),
                            repository_identity: repository_identity.clone(),
                        };
                        package
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let source_content = CatalogContentV1::new(
                CatalogScopeV1::Source {
                    source_profile: profile.to_string(),
                    source_identity: source_identity.clone(),
                    repository_identity: repository_identity.clone(),
                },
                vec![CatalogSourceEvidenceV1::AuthenticatedObject {
                    role: source_object.role.clone(),
                    source_path: source_object.source_path.clone(),
                    sha256: source_object.sha256.clone(),
                    size: source_object.size,
                }],
                source_packages,
            )
            .expect("build active source catalog");
            let source_candidate_dir = self
                .root
                .path()
                .join(format!("source-candidate-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&source_candidate_dir)
                .expect("create source candidate directory");
            let source_binding = write_catalog_candidate(
                source_candidate_dir.join(CATALOG_FILE_NAME),
                &source_content,
            )
            .expect("write source catalog candidate");
            let source_manifest = SourceSnapshotV1 {
                schema_version: SOURCE_SNAPSHOT_SCHEMA_V1,
                source_profile: profile.to_string(),
                source_identity: source_identity.clone(),
                repository_identity: repository_identity.clone(),
                stream: source_stream.clone(),
                stream_binding_sha256: conary_core::hash::sha256(
                    format!("stream-binding-{profile}-{repository_identity}").as_bytes(),
                ),
                parser_projection_version:
                    conary_core::repository::catalog::SOURCE_CATALOG_PROJECTION_VERSION_V2,
                provenance: SourceProvenanceV1 {
                    ecosystem,
                    metadata_url: format!(
                        "https://example.invalid/{profile}/{repository_identity}/metadata"
                    ),
                    content_url: Some(format!(
                        "https://example.invalid/{profile}/{repository_identity}/content"
                    )),
                    parser_config: parser_config.clone(),
                    parser_config_sha256: parser_config_sha256.clone(),
                    trust_policy: trust_policy.clone(),
                    trust_policy_sha256: trust_policy_sha256.clone(),
                },
                authenticated_root: CatalogArtifactV1 {
                    sha256: conary_core::hash::sha256(
                        format!("source-root-{profile}-{repository_identity}-{fencing_epoch}")
                            .as_bytes(),
                    ),
                    size: 1,
                },
                authenticated_objects: vec![source_object],
                catalog: source_binding.artifact.clone(),
                logical_digest_sha256: source_binding.logical_digest_sha256.clone(),
                counts: source_binding.counts,
            };
            write_fixture_source_metadata(
                &source_candidate_dir,
                &source_manifest.authenticated_objects[0],
                &source_object_bytes,
            );
            write_source_catalog_manifest(&source_candidate_dir, &source_manifest)
                .expect("write source snapshot manifest");
            publish_source_catalog_bundle(
                &source_candidate_dir,
                &self.catalog_dir,
                &source_manifest,
            )
            .expect("publish source catalog");
            let source_snapshot_sha256 = source_manifest
                .manifest_sha256()
                .expect("hash source snapshot manifest");
            members.push(ProfileSourceMemberV2 {
                ordinal,
                role,
                source_identity: source_identity.clone(),
                repository_identity: repository_identity.clone(),
                stream: source_stream.clone(),
                precedence,
                required: true,
                source_snapshot_sha256: source_snapshot_sha256.clone(),
            });
            evidence.push(CatalogSourceEvidenceV1::SourceSnapshot {
                member_ordinal: ordinal,
                source_identity,
                repository_identity,
                source_snapshot_sha256: source_snapshot_sha256.clone(),
            });
            source_manifests.push((source_snapshot_sha256, source_manifest));
        }
        let primary_member = members.first().expect("fixture profile has a member");
        let packages = packages
            .into_iter()
            .map(|mut package| {
                package.package_key_sha256.clear();
                package.origin = CatalogPackageOriginV1::Profile {
                    member_ordinal: 0,
                    source_identity: primary_member.source_identity.clone(),
                    repository_identity: primary_member.repository_identity.clone(),
                    source_snapshot_sha256: primary_member.source_snapshot_sha256.clone(),
                };
                package
            })
            .collect();
        let content = CatalogContentV1::new(
            CatalogScopeV1::Profile {
                profile: profile.to_string(),
            },
            evidence,
            packages,
        )
        .expect("build active profile catalog");
        let candidate_dir = self
            .root
            .path()
            .join(format!("candidate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&candidate_dir).expect("create catalog candidate");
        let binding = write_catalog_candidate(candidate_dir.join(CATALOG_FILE_NAME), &content)
            .expect("write catalog candidate");
        let manifest = ProfileRevisionV2 {
            schema_version: PROFILE_REVISION_SCHEMA_V2,
            profile: profile.to_string(),
            projection_version: crate::server::catalog_refresh::PROFILE_CATALOG_PROJECTION_VERSION,
            members,
            catalog: binding.artifact.clone(),
            logical_digest_sha256: binding.logical_digest_sha256.clone(),
            counts: binding.counts,
        };
        write_profile_catalog_manifest(&candidate_dir, &manifest).expect("write profile manifest");
        publish_profile_catalog_bundle(&candidate_dir, &self.catalog_dir, &manifest)
            .expect("publish profile catalog");

        let profile_revision_sha256 = manifest.manifest_sha256().expect("hash profile revision");
        let profile_manifest_json = String::from_utf8(
            conary_core::json::canonical_json(&manifest).expect("serialize profile revision"),
        )
        .expect("profile fixture JSON is UTF-8");
        let conn = self.connection();
        // Operational rows bind refresh ownership and prepared key material;
        // package authority remains in the immutable bundles above.
        let (ecosystem, version_scheme, package_format) = match ecosystem {
            SourceEcosystemV1::Rpm => ("rpm", "rpm", "rpm"),
            SourceEcosystemV1::Deb => ("deb", "debian", "deb"),
            SourceEcosystemV1::Alpm => ("alpm", "arch", "arch"),
            SourceEcosystemV1::Eopkg => ("eopkg", "eopkg", "eopkg"),
        };
        for (member, (_, source_manifest)) in manifest.members.iter().zip(&source_manifests) {
            conn.execute(
                "INSERT OR IGNORE INTO repository_source_policies (
                     source_identity, scope_kind, scope_identity, ecosystem,
                     version_scheme, stream_kind, stream_identity, update_mode
                 ) VALUES (?1, 'repository', ?2, ?3, ?4, 'release', 'stable', 'follow')",
                rusqlite::params![
                    member.source_identity,
                    member.repository_identity,
                    ecosystem,
                    version_scheme,
                ],
            )
            .expect("insert fixture source policy");
            let source_policy_id = conn
                .query_row(
                    "SELECT id FROM repository_source_policies
                     WHERE source_identity = ?1 AND scope_kind = 'repository'
                       AND scope_identity = ?2",
                    rusqlite::params![member.source_identity, member.repository_identity],
                    |row| row.get::<_, i64>(0),
                )
                .expect("resolve fixture source policy");
            let parser_config_json =
                serde_json::to_string(&source_manifest.provenance.parser_config)
                    .expect("serialize fixture parser configuration");
            let trust_policy_json = serde_json::to_string(&source_manifest.provenance.trust_policy)
                .expect("serialize fixture trust policy");
            conn.execute(
                "INSERT OR IGNORE INTO repositories (
                     name, url, enabled, priority, profile_member_role,
                     profile_member_required, source_profile, trust_policy_json,
                     package_format, parser_config_json, source_policy_id,
                     repository_identity, stream_binding_sha256
                 ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![
                    format!("fixture-{profile}-{}", member.repository_identity),
                    &source_manifest.provenance.metadata_url,
                    i64::from(member.precedence),
                    member.role.as_str(),
                    member.required,
                    profile,
                    trust_policy_json,
                    package_format,
                    parser_config_json,
                    source_policy_id,
                    member.repository_identity,
                    source_manifest.stream_binding_sha256,
                ],
            )
            .expect("insert fixture repository row");
        }
        for (source_snapshot_sha256, source_manifest) in &source_manifests {
            let source_manifest_json = String::from_utf8(
                conary_core::json::canonical_json(source_manifest)
                    .expect("serialize source snapshot manifest"),
            )
            .expect("source snapshot manifest is UTF-8");
            RemiCatalogResource {
                resource_sha256: source_snapshot_sha256.clone(),
                kind: RemiCatalogResourceKind::SourceSnapshot,
                source_profile: profile.to_string(),
                artifact_sha256: source_manifest.catalog.sha256.clone(),
                artifact_size: i64::try_from(source_manifest.catalog.size)
                    .expect("source artifact size fits"),
                logical_digest_sha256: source_manifest.logical_digest_sha256.clone(),
                manifest_json: source_manifest_json,
                durable: true,
                created_at: fencing_epoch,
            }
            .insert(&conn)
            .expect("insert source resource");
        }
        RemiCatalogResource {
            resource_sha256: profile_revision_sha256.clone(),
            kind: RemiCatalogResourceKind::ProfileRevision,
            source_profile: profile.to_string(),
            artifact_sha256: manifest.catalog.sha256.clone(),
            artifact_size: i64::try_from(manifest.catalog.size).expect("artifact size fits"),
            logical_digest_sha256: manifest.logical_digest_sha256.clone(),
            manifest_json: profile_manifest_json,
            durable: true,
            created_at: fencing_epoch,
        }
        .insert(&conn)
        .expect("insert profile resource");
        for member in &manifest.members {
            RemiProfileRevisionMember {
                profile_revision_sha256: profile_revision_sha256.clone(),
                ordinal: i64::from(member.ordinal),
                source_snapshot_sha256: member.source_snapshot_sha256.clone(),
                source_identity: member.source_identity.clone(),
                repository_identity: member.repository_identity.clone(),
                stream_kind: "release".to_string(),
                stream_identity: "stable".to_string(),
                role: member.role,
                precedence: i64::from(member.precedence),
                required: member.required,
            }
            .insert(&conn)
            .expect("insert profile member");
        }
        if activate {
            let run_id = uuid::Uuid::new_v4().to_string();
            let owner_instance_uuid = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO repository_sync_runs (
                 run_id, source_profile, owner_instance_uuid, fencing_epoch,
                 input_profile_digest, candidate_profile_digest, state,
                 started_at, heartbeat_at, lease_expires_at, finished_at
             ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'published', ?4, ?4, ?4, ?4)",
                rusqlite::params![
                    run_id,
                    profile,
                    owner_instance_uuid,
                    fencing_epoch,
                    profile_revision_sha256,
                ],
            )
            .expect("insert activation run");
            conn.execute(
                "INSERT INTO remi_active_profile_revisions (
                 source_profile, profile_revision_sha256, fencing_epoch,
                 activation_run_id, owner_instance_uuid, activated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?3)
             ON CONFLICT(source_profile) DO UPDATE SET
                 profile_revision_sha256 = excluded.profile_revision_sha256,
                 fencing_epoch = excluded.fencing_epoch,
                 activation_run_id = excluded.activation_run_id,
                 owner_instance_uuid = excluded.owner_instance_uuid,
                 activated_at = excluded.activated_at",
                rusqlite::params![
                    profile,
                    profile_revision_sha256,
                    fencing_epoch,
                    run_id,
                    owner_instance_uuid,
                ],
            )
            .expect("activate profile revision");
        } else if candidate {
            let run_id = uuid::Uuid::new_v4().to_string();
            let owner_instance_uuid = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO repository_sync_runs (
                     run_id, source_profile, owner_instance_uuid, fencing_epoch,
                     input_profile_digest, candidate_profile_digest, state,
                     started_at, heartbeat_at, lease_expires_at, finished_at
                 ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'candidate', ?4, ?4, ?4, ?4)",
                rusqlite::params![
                    run_id,
                    profile,
                    owner_instance_uuid,
                    fencing_epoch,
                    profile_revision_sha256,
                ],
            )
            .expect("insert candidate run");
            conn.execute(
                "INSERT INTO repository_sync_scopes (
                     source_profile, fencing_epoch, current_run_id
                 ) VALUES (?1, ?2, ?3)
                 ON CONFLICT(source_profile) DO UPDATE SET
                     fencing_epoch = excluded.fencing_epoch,
                     current_run_id = excluded.current_run_id",
                rusqlite::params![profile, fencing_epoch, run_id],
            )
            .expect("select fixture candidate run");
            for member in &manifest.members {
                let repository_id = conn
                    .query_row(
                        "SELECT id FROM repositories
                         WHERE source_profile = ?1 AND repository_identity = ?2",
                        rusqlite::params![profile, member.repository_identity],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("resolve fixture run member repository");
                conn.execute(
                    "INSERT INTO repository_sync_run_members (
                         run_id, ordinal, repository_id, source_identity,
                         repository_identity, stream_kind, stream_identity, role,
                         precedence, required, input_source_snapshot_sha256,
                         candidate_source_snapshot_sha256
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'release', 'stable', ?6, ?7, ?8, NULL, ?9)",
                    rusqlite::params![
                        run_id,
                        i64::from(member.ordinal),
                        repository_id,
                        member.source_identity,
                        member.repository_identity,
                        member.role.as_str(),
                        i64::from(member.precedence),
                        member.required,
                        member.source_snapshot_sha256,
                    ],
                )
                .expect("insert fixture run member");
            }
        }
        profile_revision_sha256
    }
}

fn source_fixture_authority(
    profile: &str,
) -> (
    RepositoryParserConfig,
    RepositoryTrustPolicy,
    SourceEcosystemV1,
    SourceMetadataObjectRoleV1,
) {
    let format = conary_core::repository::supported_profiles::profile_by_id(profile)
        .map(|profile| profile.package_format())
        .unwrap_or(ProfilePackageFormat::Rpm);
    let fingerprint = "A".repeat(40);
    match format {
        ProfilePackageFormat::Rpm => {
            let parser = RepositoryParserConfig::Rpm {
                architecture: "x86_64".to_string(),
            };
            let trust = RepositoryTrustPolicy::Rpm {
                metadata: RpmMetadataAuthority::Metalink {
                    url: format!("https://example.invalid/{profile}/metalink"),
                },
                package_keys: vec![
                    OpenPgpTrustRoot::new(
                        format!("https://example.invalid/{profile}/keys"),
                        fingerprint,
                    )
                    .expect("valid RPM fixture trust root"),
                ],
            };
            (
                parser,
                trust,
                SourceEcosystemV1::Rpm,
                SourceMetadataObjectRoleV1::RpmPrimary,
            )
        }
        ProfilePackageFormat::Deb => {
            let parser = RepositoryParserConfig::Deb {
                distribution: "stable".to_string(),
                component: "main".to_string(),
                architecture: "amd64".to_string(),
            };
            let trust = RepositoryTrustPolicy::Debian {
                release_keys: vec![
                    OpenPgpTrustRoot::new(
                        format!("https://example.invalid/{profile}/keys"),
                        fingerprint,
                    )
                    .expect("valid Debian fixture trust root"),
                ],
            };
            (
                parser,
                trust,
                SourceEcosystemV1::Deb,
                SourceMetadataObjectRoleV1::DebianPackages,
            )
        }
        ProfilePackageFormat::Arch => {
            let parser = RepositoryParserConfig::Arch {
                database: "core".to_string(),
            };
            let trust = RepositoryTrustPolicy::Arch {
                keyring: ArchKeyringTrust {
                    url: format!("https://example.invalid/{profile}/keyring"),
                    format: ArchKeyringFormat::OpenPgp,
                    master_fingerprints: vec![fingerprint],
                    packager_key_threshold: 1,
                },
                sig_level: ArchSigLevel::distribution_default(),
            };
            (
                parser,
                trust,
                SourceEcosystemV1::Alpm,
                SourceMetadataObjectRoleV1::ArchDatabase,
            )
        }
        ProfilePackageFormat::Eopkg => {
            let parser = RepositoryParserConfig::Eopkg {
                architecture: "x86_64".to_string(),
            };
            let trust = RepositoryTrustPolicy::Eopkg {
                origin: format!("https://example.invalid/{profile}/"),
            };
            (
                parser,
                trust,
                SourceEcosystemV1::Eopkg,
                SourceMetadataObjectRoleV1::EopkgIndex,
            )
        }
    }
}

fn write_fixture_source_metadata(candidate: &Path, object: &SourceMetadataObjectV1, bytes: &[u8]) {
    assert_eq!(conary_core::hash::sha256(bytes), object.sha256);
    assert_eq!(bytes.len() as u64, object.size);
    let directory = candidate.join(SOURCE_METADATA_DIRECTORY_NAME);
    std::fs::create_dir(&directory).expect("create fixture source metadata directory");
    let path = directory.join(&object.sha256);
    std::fs::write(&path, bytes).expect("write fixture source metadata object");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("set fixture source metadata directory permissions");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("set fixture source metadata object permissions");
    }
}

pub(crate) fn package(
    profile: &str,
    name: &str,
    version: &str,
    release: &str,
    architecture: Option<&str>,
    size: u64,
    checksum_marker: &str,
) -> CatalogPackageRecordV1 {
    let version_scheme = conary_core::repository::supported_profiles::profile_by_id(profile)
        .map_or(VersionScheme::Rpm, |profile| profile.version_scheme());
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: format!("source-{profile}"),
            repository_identity: format!("repository-{profile}"),
            source_snapshot_sha256: conary_core::hash::sha256(
                format!("package-snapshot-{profile}").as_bytes(),
            ),
        },
        source_profile: profile.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        package_release: release.to_string(),
        architecture: architecture.map(str::to_string),
        debian_multi_arch: (version_scheme == VersionScheme::Debian).then_some(DebianMultiArch::No),
        description: None,
        checksum: conary_core::hash::sha256(checksum_marker.as_bytes()),
        size,
        download_url: format!("https://example.invalid/{name}-{version}-{release}.rpm"),
        metadata: None,
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme,
        provides: Vec::new(),
        requirement_groups: Vec::new(),
    }
}
