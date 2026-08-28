// crates/conary-core/src/db/models/remi_catalog/resource.rs

//! Typed registration of already-published immutable catalog resources.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;

use super::{
    MEMBER_COLUMNS, RemiCatalogResource, RemiCatalogResourceKind, RemiProfileRevisionMember,
};
use crate::error::{Error, Result};
use crate::repository::catalog::{
    ProfileRevisionV2, ProfileSourceMemberV2, SourceSnapshotV1, SourceStreamKindV1,
};

struct ManifestResourceIdentity {
    resource_sha256: String,
    kind: RemiCatalogResourceKind,
    source_profile: String,
    artifact_sha256: String,
    artifact_size: u64,
    logical_digest_sha256: String,
}

impl RemiCatalogResource {
    pub fn from_source_snapshot(manifest: &SourceSnapshotV1, created_at: i64) -> Result<Self> {
        manifest.validate()?;
        Self::from_manifest(
            ManifestResourceIdentity {
                resource_sha256: manifest.manifest_sha256()?,
                kind: RemiCatalogResourceKind::SourceSnapshot,
                source_profile: manifest.source_profile.clone(),
                artifact_sha256: manifest.catalog.sha256.clone(),
                artifact_size: manifest.catalog.size,
                logical_digest_sha256: manifest.logical_digest_sha256.clone(),
            },
            manifest,
            created_at,
        )
    }

    pub fn from_profile_revision(manifest: &ProfileRevisionV2, created_at: i64) -> Result<Self> {
        manifest.validate()?;
        Self::from_manifest(
            ManifestResourceIdentity {
                resource_sha256: manifest.manifest_sha256()?,
                kind: RemiCatalogResourceKind::ProfileRevision,
                source_profile: manifest.profile.clone(),
                artifact_sha256: manifest.catalog.sha256.clone(),
                artifact_size: manifest.catalog.size,
                logical_digest_sha256: manifest.logical_digest_sha256.clone(),
            },
            manifest,
            created_at,
        )
    }

    fn from_manifest(
        identity: ManifestResourceIdentity,
        manifest: &impl Serialize,
        created_at: i64,
    ) -> Result<Self> {
        let artifact_size = i64::try_from(identity.artifact_size).map_err(|_| {
            Error::ConfigError("catalog artifact size exceeds SQLite integer range".to_string())
        })?;
        let manifest_json =
            String::from_utf8(crate::json::canonical_json(manifest).map_err(|error| {
                Error::ParseError(format!("serialize catalog resource manifest: {error}"))
            })?)
            .map_err(|error| {
                Error::InternalError(format!("canonical catalog manifest is not UTF-8: {error}"))
            })?;
        let resource = Self {
            resource_sha256: identity.resource_sha256,
            kind: identity.kind,
            source_profile: identity.source_profile,
            artifact_sha256: identity.artifact_sha256,
            artifact_size,
            logical_digest_sha256: identity.logical_digest_sha256,
            manifest_json,
            durable: true,
            created_at,
        };
        resource.validate()?;
        Ok(resource)
    }

    fn insert_exact(&self, tx: &Transaction<'_>) -> Result<()> {
        self.validate()?;
        let inserted = tx.execute(
            "INSERT INTO remi_catalog_resources (
                 resource_sha256, resource_kind, source_profile, artifact_sha256,
                 artifact_size, logical_digest_sha256, manifest_json, durable, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(resource_sha256) DO NOTHING",
            params![
                &self.resource_sha256,
                self.kind.as_str(),
                &self.source_profile,
                &self.artifact_sha256,
                self.artifact_size,
                &self.logical_digest_sha256,
                &self.manifest_json,
                self.durable as i64,
                self.created_at,
            ],
        )?;
        if inserted == 0 {
            let stored = Self::find_by_sha256(tx, &self.resource_sha256)?.ok_or_else(|| {
                Error::InternalError("catalog resource conflict disappeared".to_string())
            })?;
            if !stored.has_same_immutable_metadata(self) {
                return Err(Error::ConflictError(format!(
                    "catalog resource {} was already registered with different exact metadata",
                    self.resource_sha256
                )));
            }
        }
        Ok(())
    }

    fn has_same_immutable_metadata(&self, other: &Self) -> bool {
        self.resource_sha256 == other.resource_sha256
            && self.kind == other.kind
            && self.source_profile == other.source_profile
            && self.artifact_sha256 == other.artifact_sha256
            && self.artifact_size == other.artifact_size
            && self.logical_digest_sha256 == other.logical_digest_sha256
            && self.manifest_json == other.manifest_json
            && self.durable == other.durable
    }
}

impl RemiProfileRevisionMember {
    fn from_contract(profile_revision_sha256: &str, member: &ProfileSourceMemberV2) -> Self {
        Self {
            profile_revision_sha256: profile_revision_sha256.to_string(),
            ordinal: i64::from(member.ordinal),
            source_snapshot_sha256: member.source_snapshot_sha256.clone(),
            source_identity: member.source_identity.clone(),
            repository_identity: member.repository_identity.clone(),
            stream_kind: match member.stream.kind {
                SourceStreamKindV1::Release => "release",
                SourceStreamKindV1::Channel => "channel",
                SourceStreamKindV1::Rolling => "rolling",
            }
            .to_string(),
            stream_identity: member.stream.identity.clone(),
            role: member.role,
            precedence: i64::from(member.precedence),
            required: member.required,
        }
    }

    fn insert_exact(&self, tx: &Transaction<'_>) -> Result<()> {
        self.validate()?;
        let inserted = tx.execute(
            "INSERT INTO remi_profile_revision_members (
                 profile_revision_sha256, ordinal, source_snapshot_sha256,
                 source_identity, repository_identity, stream_kind, stream_identity,
                 role, precedence, required
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(profile_revision_sha256, ordinal) DO NOTHING",
            params![
                &self.profile_revision_sha256,
                self.ordinal,
                &self.source_snapshot_sha256,
                &self.source_identity,
                &self.repository_identity,
                &self.stream_kind,
                &self.stream_identity,
                self.role.as_str(),
                self.precedence,
                self.required as i64,
            ],
        )?;
        if inserted == 0 {
            let sql = format!(
                "SELECT {MEMBER_COLUMNS} FROM remi_profile_revision_members
                 WHERE profile_revision_sha256 = ?1 AND ordinal = ?2"
            );
            let stored = tx
                .query_row(
                    &sql,
                    params![&self.profile_revision_sha256, self.ordinal],
                    Self::from_row,
                )
                .optional()?
                .ok_or_else(|| {
                    Error::InternalError("profile revision member conflict disappeared".to_string())
                })?;
            if stored != *self {
                return Err(Error::ConflictError(format!(
                    "profile revision {} member {} was already registered with different exact metadata",
                    self.profile_revision_sha256, self.ordinal
                )));
            }
        }
        Ok(())
    }
}

/// Register one already-durable profile bundle and the exact source bundles it
/// names. The transaction is idempotent only for byte-identical manifests.
pub fn register_profile_catalog_revision(
    conn: &Connection,
    source_manifests: &[SourceSnapshotV1],
    profile_manifest: &ProfileRevisionV2,
    created_at: i64,
) -> Result<()> {
    profile_manifest.validate()?;
    if source_manifests.len() != profile_manifest.members.len() {
        return Err(Error::ConflictError(format!(
            "profile revision '{}' declares {} members but registration supplied {} source snapshots",
            profile_manifest.profile,
            profile_manifest.members.len(),
            source_manifests.len()
        )));
    }
    let mut sources = BTreeMap::new();
    for manifest in source_manifests {
        manifest.validate()?;
        let digest = manifest.manifest_sha256()?;
        if sources.insert(digest.clone(), manifest).is_some() {
            return Err(Error::ConflictError(format!(
                "profile revision registration repeats source snapshot {digest}"
            )));
        }
    }
    for member in &profile_manifest.members {
        let source = sources.get(&member.source_snapshot_sha256).ok_or_else(|| {
            Error::ConflictError(format!(
                "profile revision member {} lacks source snapshot {}",
                member.ordinal, member.source_snapshot_sha256
            ))
        })?;
        if source.source_profile != profile_manifest.profile
            || source.source_identity != member.source_identity
            || source.repository_identity != member.repository_identity
            || source.stream != member.stream
        {
            return Err(Error::ConflictError(format!(
                "profile revision member {} disagrees with source snapshot {}",
                member.ordinal, member.source_snapshot_sha256
            )));
        }
    }

    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    for member in &profile_manifest.members {
        let source = sources[&member.source_snapshot_sha256];
        RemiCatalogResource::from_source_snapshot(source, created_at)?.insert_exact(&tx)?;
    }
    let profile_resource =
        RemiCatalogResource::from_profile_revision(profile_manifest, created_at)?;
    profile_resource.insert_exact(&tx)?;
    for member in &profile_manifest.members {
        RemiProfileRevisionMember::from_contract(&profile_resource.resource_sha256, member)
            .insert_exact(&tx)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::ensure_current;
    use crate::repository::catalog::{
        CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V2, SOURCE_SNAPSHOT_SCHEMA_V1,
        SourceEcosystemV1, SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1,
        SourceStreamV1,
    };
    use crate::repository::{
        OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
    };

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn source_manifest() -> SourceSnapshotV1 {
        let parser_config = RepositoryParserConfig::Rpm {
            architecture: "x86_64".to_string(),
        };
        let trust_policy = RepositoryTrustPolicy::Rpm {
            metadata: RpmMetadataAuthority::Metalink {
                url: "https://example.test/metalink".to_string(),
            },
            package_keys: vec![
                OpenPgpTrustRoot::new(
                    "https://example.test/fedora.gpg".to_string(),
                    "A".repeat(40),
                )
                .unwrap(),
            ],
        };
        SourceSnapshotV1 {
            schema_version: SOURCE_SNAPSHOT_SCHEMA_V1,
            source_profile: "fedora-44".to_string(),
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "44".to_string(),
            },
            stream_binding_sha256: digest('a'),
            parser_projection_version:
                crate::repository::catalog::SOURCE_CATALOG_PROJECTION_VERSION_V2,
            provenance: SourceProvenanceV1 {
                ecosystem: SourceEcosystemV1::Rpm,
                metadata_url: "https://example.test/repository".to_string(),
                content_url: None,
                parser_config_sha256: crate::hash::sha256(
                    &crate::json::canonical_json(&parser_config).unwrap(),
                ),
                parser_config,
                trust_policy_sha256: crate::hash::sha256(
                    &crate::json::canonical_json(&trust_policy).unwrap(),
                ),
                trust_policy,
            },
            authenticated_root: CatalogArtifactV1 {
                sha256: digest('b'),
                size: 1024,
            },
            authenticated_objects: vec![SourceMetadataObjectV1 {
                role: SourceMetadataObjectRoleV1::RpmPrimary,
                source_path: "repodata/primary.xml.zst".to_string(),
                sha256: digest('c'),
                size: 2048,
            }],
            catalog: CatalogArtifactV1 {
                sha256: digest('d'),
                size: 4096,
            },
            logical_digest_sha256: digest('e'),
            counts: CatalogCountsV1 {
                source_evidence: 1,
                ..CatalogCountsV1::default()
            },
        }
    }

    fn profile_manifest(source: &SourceSnapshotV1) -> ProfileRevisionV2 {
        ProfileRevisionV2 {
            schema_version: PROFILE_REVISION_SCHEMA_V2,
            profile: "fedora-44".to_string(),
            projection_version: 1,
            members: vec![ProfileSourceMemberV2 {
                ordinal: 0,
                source_identity: source.source_identity.clone(),
                repository_identity: source.repository_identity.clone(),
                stream: source.stream.clone(),
                role: crate::repository::supported_profiles::ProfileSourceRole::Base,
                precedence: 10,
                required: true,
                source_snapshot_sha256: source.manifest_sha256().unwrap(),
            }],
            catalog: CatalogArtifactV1 {
                sha256: digest('f'),
                size: 8192,
            },
            logical_digest_sha256: digest('0'),
            counts: CatalogCountsV1 {
                source_evidence: 1,
                ..CatalogCountsV1::default()
            },
        }
    }

    #[test]
    fn exact_published_manifests_register_atomically_and_replay_idempotently() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let source = source_manifest();
        let profile = profile_manifest(&source);

        register_profile_catalog_revision(&conn, std::slice::from_ref(&source), &profile, 100)
            .unwrap();
        register_profile_catalog_revision(&conn, std::slice::from_ref(&source), &profile, 200)
            .unwrap();

        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_catalog_resources", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_profile_revision_members",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        assert_eq!(
            RemiCatalogResource::find_by_sha256(&conn, &source.manifest_sha256().unwrap())
                .unwrap()
                .unwrap()
                .created_at,
            100
        );
    }

    #[test]
    fn authenticated_root_change_can_register_same_projection_artifact() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let source = source_manifest();
        let profile = profile_manifest(&source);
        register_profile_catalog_revision(&conn, std::slice::from_ref(&source), &profile, 100)
            .unwrap();

        let mut alias = source.clone();
        alias.authenticated_root.sha256 = digest('1');
        assert_ne!(
            alias.manifest_sha256().unwrap(),
            source.manifest_sha256().unwrap()
        );
        assert_eq!(alias.catalog, source.catalog);
        let mut alias_profile = profile_manifest(&alias);
        alias_profile.catalog.sha256 = digest('2');
        alias_profile.logical_digest_sha256 = digest('3');

        register_profile_catalog_revision(&conn, std::slice::from_ref(&alias), &alias_profile, 200)
            .unwrap();

        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_catalog_resources
                 WHERE resource_kind = 'source_snapshot' AND artifact_sha256 = ?1",
                [&source.catalog.sha256],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_catalog_resources", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            4
        );
        assert!(
            RemiCatalogResource::find_by_sha256(&conn, &alias.manifest_sha256().unwrap())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn mixed_member_registration_writes_nothing() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let source = source_manifest();
        let profile = profile_manifest(&source);
        let mut mixed = source.clone();
        mixed.repository_identity = "fedora-updates-x86_64".to_string();

        assert!(register_profile_catalog_revision(&conn, &[mixed], &profile, 100).is_err());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_catalog_resources", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }
}
