// crates/conary-core/src/db/models/remi_catalog/resource.rs

//! Typed registration and host-local physical proof for immutable catalogs.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use rusqlite::{
    Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params, types::Type,
};
use serde::Serialize;

use super::{MEMBER_COLUMNS, RemiProfileRevisionMember};
use crate::error::{Error, Result};
use crate::repository::catalog::{
    PORTABLE_CHUNK_SIZE_V1, PortableManifestAttestationV1, ProfileRevisionV2,
    ProfileSourceMemberV2, SourceSnapshotV1, SourceStreamKindV1, portable_chunk_count_v1,
    portable_manifest_size_v1,
};

use super::validation::{
    validate_canonical_manifest, validate_identity, validate_sha256, validate_storage_component,
};

pub(super) const RESOURCE_COLUMNS: &str = "resource_sha256, resource_kind, source_profile, \
    artifact_sha256, artifact_size, logical_digest_sha256, manifest_json, \
    portable_manifest_sha256, portable_manifest_size, portable_chunk_size, \
    portable_chunk_count, durable, created_at";

/// The two resource classes that may be referenced by an activated profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemiCatalogResourceKind {
    SourceSnapshot,
    ProfileRevision,
}

impl RemiCatalogResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceSnapshot => "source_snapshot",
            Self::ProfileRevision => "profile_revision",
        }
    }

    pub(super) fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "source_snapshot" => Ok(Self::SourceSnapshot),
            "profile_revision" => Ok(Self::ProfileRevision),
            other => Err(invalid_text_column(
                column,
                format!("invalid Remi catalog resource kind {other}"),
            )),
        }
    }
}

/// Host-local physical integrity authority for one exact catalog artifact.
///
/// This is deliberately not part of the signed source/profile manifest. It is
/// immutable operational metadata bound to that manifest's exact catalog byte
/// identity on the host that published the durable bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiCatalogPhysicalAttestation {
    /// Exact identity and length of the portable chunk-digest manifest.
    pub portable_manifest: PortableManifestAttestationV1,
    /// Fixed v1 catalog chunk size recorded independently in operational state.
    pub chunk_size: u32,
    /// Exact number of chunks required to cover the catalog artifact.
    pub chunk_count: u64,
}

impl RemiCatalogPhysicalAttestation {
    pub fn new(
        portable_manifest: PortableManifestAttestationV1,
        catalog_size: u64,
    ) -> Result<Self> {
        let attestation = Self {
            portable_manifest,
            chunk_size: PORTABLE_CHUNK_SIZE_V1,
            chunk_count: portable_chunk_count_v1(catalog_size)
                .map_err(|error| Error::ConfigError(error.to_string()))?,
        };
        attestation.validate(catalog_size)?;
        Ok(attestation)
    }

    #[cfg(test)]
    pub(crate) fn test_for_catalog_size(catalog_size: u64) -> Self {
        let chunk_count = portable_chunk_count_v1(catalog_size).expect("fixture chunk count");
        Self::new(
            PortableManifestAttestationV1 {
                sha256: "a".repeat(64),
                size: portable_manifest_size_v1(chunk_count).expect("fixture manifest size"),
            },
            catalog_size,
        )
        .expect("fixture physical attestation")
    }

    pub(super) fn validate(&self, catalog_size: u64) -> Result<()> {
        validate_sha256(
            &self.portable_manifest.sha256,
            "catalog portable manifest SHA-256",
        )?;
        if self.chunk_size != PORTABLE_CHUNK_SIZE_V1 {
            return Err(Error::ConfigError(format!(
                "catalog portable chunk size {} does not match v1 size {PORTABLE_CHUNK_SIZE_V1}",
                self.chunk_size
            )));
        }
        let expected_count = portable_chunk_count_v1(catalog_size)
            .map_err(|error| Error::ConfigError(error.to_string()))?;
        if self.chunk_count != expected_count {
            return Err(Error::ConfigError(format!(
                "catalog portable chunk count {} does not match exact artifact count {expected_count}",
                self.chunk_count
            )));
        }
        let expected_manifest_size = portable_manifest_size_v1(self.chunk_count)
            .map_err(|error| Error::ConfigError(error.to_string()))?;
        if self.portable_manifest.size != expected_manifest_size {
            return Err(Error::ConfigError(format!(
                "catalog portable manifest size {} does not match exact v1 size {expected_manifest_size}",
                self.portable_manifest.size
            )));
        }
        Ok(())
    }

    fn from_db(
        manifest_sha256: String,
        manifest_size: i64,
        chunk_size: i64,
        chunk_count: i64,
        catalog_size: u64,
    ) -> rusqlite::Result<Self> {
        let manifest_size = u64::try_from(manifest_size).map_err(|_| {
            invalid_text_column(8, "catalog portable manifest size must not be negative")
        })?;
        let chunk_size = u32::try_from(chunk_size).map_err(|_| {
            invalid_text_column(9, "catalog portable chunk size is outside the u32 range")
        })?;
        let chunk_count = u64::try_from(chunk_count).map_err(|_| {
            invalid_text_column(10, "catalog portable chunk count must not be negative")
        })?;
        let attestation = Self {
            portable_manifest: PortableManifestAttestationV1 {
                sha256: manifest_sha256,
                size: manifest_size,
            },
            chunk_size,
            chunk_count,
        };
        attestation
            .validate(catalog_size)
            .map_err(|error| invalid_text_column(7, error.to_string()))?;
        Ok(attestation)
    }
}

/// One durable catalog artifact and the manifest metadata that binds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiCatalogResource {
    /// SHA-256 of the canonical SourceSnapshotV1 or ProfileRevisionV2 manifest.
    pub resource_sha256: String,
    pub kind: RemiCatalogResourceKind,
    pub source_profile: String,
    /// SHA-256 of the standalone catalog SQLite file named by the manifest.
    pub artifact_sha256: String,
    pub artifact_size: i64,
    pub logical_digest_sha256: String,
    /// Canonical JSON bytes of the versioned manifest, stored as UTF-8 text.
    pub manifest_json: String,
    /// Exact host-local physical verification authority for `artifact_sha256`.
    pub physical_attestation: RemiCatalogPhysicalAttestation,
    /// Set only after the artifact and its containing directory were durably
    /// synchronized and the immutable publication path exists.
    pub durable: bool,
    pub created_at: i64,
}

impl RemiCatalogResource {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        conn.execute(
            "INSERT INTO remi_catalog_resources (
                 resource_sha256, resource_kind, source_profile, artifact_sha256,
                 artifact_size, logical_digest_sha256, manifest_json,
                 portable_manifest_sha256, portable_manifest_size,
                 portable_chunk_size, portable_chunk_count, durable, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                &self.resource_sha256,
                self.kind.as_str(),
                &self.source_profile,
                &self.artifact_sha256,
                self.artifact_size,
                &self.logical_digest_sha256,
                &self.manifest_json,
                &self.physical_attestation.portable_manifest.sha256,
                i64::try_from(self.physical_attestation.portable_manifest.size).map_err(|_| {
                    Error::ConfigError(
                        "catalog portable manifest size exceeds SQLite integer range".to_string(),
                    )
                })?,
                i64::from(self.physical_attestation.chunk_size),
                i64::try_from(self.physical_attestation.chunk_count).map_err(|_| {
                    Error::ConfigError(
                        "catalog portable chunk count exceeds SQLite integer range".to_string(),
                    )
                })?,
                self.durable as i64,
                self.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn find_by_sha256(conn: &Connection, resource_sha256: &str) -> Result<Option<Self>> {
        validate_sha256(resource_sha256, "catalog resource SHA-256")?;
        let sql = format!(
            "SELECT {RESOURCE_COLUMNS} FROM remi_catalog_resources WHERE resource_sha256 = ?1"
        );
        conn.query_row(&sql, [resource_sha256], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn find_profile_revision(
        conn: &Connection,
        source_profile: &str,
        resource_sha256: &str,
    ) -> Result<Option<Self>> {
        validate_identity(source_profile, "catalog source profile")?;
        validate_sha256(resource_sha256, "profile revision SHA-256")?;
        let sql = format!(
            "SELECT {RESOURCE_COLUMNS} FROM remi_catalog_resources
             WHERE resource_sha256 = ?1 AND resource_kind = 'profile_revision'
               AND source_profile = ?2"
        );
        conn.query_row(
            &sql,
            params![resource_sha256, source_profile],
            Self::from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub(super) fn validate(&self) -> Result<()> {
        validate_sha256(&self.resource_sha256, "catalog resource SHA-256")?;
        validate_storage_component(&self.source_profile, "catalog source profile")?;
        validate_sha256(&self.artifact_sha256, "catalog artifact SHA-256")?;
        validate_sha256(&self.logical_digest_sha256, "catalog logical digest")?;
        if self.artifact_size < 0 {
            return Err(Error::ConfigError(
                "catalog artifact size must not be negative".to_string(),
            ));
        }
        self.physical_attestation
            .validate(u64::try_from(self.artifact_size).map_err(|_| {
                Error::ConfigError("catalog artifact size must not be negative".to_string())
            })?)?;
        if self.created_at < 0 {
            return Err(Error::ConfigError(
                "catalog resource creation time must not be negative".to_string(),
            ));
        }
        validate_canonical_manifest(&self.manifest_json)?;
        let manifest_sha256 = crate::hash::sha256(self.manifest_json.as_bytes());
        if manifest_sha256 != self.resource_sha256 {
            return Err(Error::ChecksumMismatch {
                expected: self.resource_sha256.clone(),
                actual: manifest_sha256,
            });
        }
        Ok(())
    }

    pub(super) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let artifact_size = row.get::<_, i64>(4)?;
        let catalog_size = u64::try_from(artifact_size)
            .map_err(|_| invalid_text_column(4, "catalog artifact size must not be negative"))?;
        let physical_attestation = RemiCatalogPhysicalAttestation::from_db(
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            catalog_size,
        )?;
        let resource = Self {
            resource_sha256: row.get(0)?,
            kind: RemiCatalogResourceKind::from_db(&row.get::<_, String>(1)?, 1)?,
            source_profile: row.get(2)?,
            artifact_sha256: row.get(3)?,
            artifact_size,
            logical_digest_sha256: row.get(5)?,
            manifest_json: row.get(6)?,
            physical_attestation,
            durable: row.get::<_, i64>(11)? != 0,
            created_at: row.get(12)?,
        };
        resource
            .validate()
            .map_err(|error| invalid_text_column(0, error.to_string()))?;
        Ok(resource)
    }
}

fn invalid_text_column(column: usize, message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        Type::Text,
        Box::new(io::Error::new(io::ErrorKind::InvalidData, message.into())),
    )
}

struct ManifestResourceIdentity {
    resource_sha256: String,
    kind: RemiCatalogResourceKind,
    source_profile: String,
    artifact_sha256: String,
    artifact_size: u64,
    logical_digest_sha256: String,
}

impl RemiCatalogResource {
    pub fn from_source_snapshot(
        manifest: &SourceSnapshotV1,
        physical_attestation: RemiCatalogPhysicalAttestation,
        created_at: i64,
    ) -> Result<Self> {
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
            physical_attestation,
            created_at,
        )
    }

    pub fn from_profile_revision(
        manifest: &ProfileRevisionV2,
        physical_attestation: RemiCatalogPhysicalAttestation,
        created_at: i64,
    ) -> Result<Self> {
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
            physical_attestation,
            created_at,
        )
    }

    fn from_manifest(
        identity: ManifestResourceIdentity,
        manifest: &impl Serialize,
        physical_attestation: RemiCatalogPhysicalAttestation,
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
            physical_attestation,
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
                 artifact_size, logical_digest_sha256, manifest_json,
                 portable_manifest_sha256, portable_manifest_size,
                 portable_chunk_size, portable_chunk_count, durable, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(resource_sha256) DO NOTHING",
            params![
                &self.resource_sha256,
                self.kind.as_str(),
                &self.source_profile,
                &self.artifact_sha256,
                self.artifact_size,
                &self.logical_digest_sha256,
                &self.manifest_json,
                &self.physical_attestation.portable_manifest.sha256,
                i64::try_from(self.physical_attestation.portable_manifest.size).map_err(|_| {
                    Error::ConfigError(
                        "catalog portable manifest size exceeds SQLite integer range".to_string(),
                    )
                })?,
                i64::from(self.physical_attestation.chunk_size),
                i64::try_from(self.physical_attestation.chunk_count).map_err(|_| {
                    Error::ConfigError(
                        "catalog portable chunk count exceeds SQLite integer range".to_string(),
                    )
                })?,
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
            && self.physical_attestation == other.physical_attestation
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
    source_physical_attestations: &BTreeMap<String, RemiCatalogPhysicalAttestation>,
    profile_manifest: &ProfileRevisionV2,
    profile_physical_attestation: RemiCatalogPhysicalAttestation,
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
    let mut expected_source_artifacts = BTreeMap::new();
    for manifest in source_manifests {
        manifest.validate()?;
        let digest = manifest.manifest_sha256()?;
        if sources.insert(digest.clone(), manifest).is_some() {
            return Err(Error::ConflictError(format!(
                "profile revision registration repeats source snapshot {digest}"
            )));
        }
        if let Some(previous_size) =
            expected_source_artifacts.insert(manifest.catalog.sha256.clone(), manifest.catalog.size)
            && previous_size != manifest.catalog.size
        {
            return Err(Error::ConflictError(format!(
                "source catalog artifact {} was supplied with sizes {previous_size} and {}",
                manifest.catalog.sha256, manifest.catalog.size
            )));
        }
    }
    for artifact_sha256 in source_physical_attestations.keys() {
        validate_sha256(
            artifact_sha256,
            "source catalog physical-attestation artifact key",
        )?;
    }
    let supplied_source_artifacts = source_physical_attestations
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let expected_source_artifact_keys = expected_source_artifacts
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    if supplied_source_artifacts != expected_source_artifact_keys {
        let missing = expected_source_artifact_keys
            .difference(&supplied_source_artifacts)
            .cloned()
            .collect::<Vec<_>>();
        let extra = supplied_source_artifacts
            .difference(&expected_source_artifact_keys)
            .cloned()
            .collect::<Vec<_>>();
        return Err(Error::ConflictError(format!(
            "source catalog physical attestations do not match exact manifest artifacts; missing [{}], extra [{}]",
            missing.join(", "),
            extra.join(", ")
        )));
    }
    for (artifact_sha256, artifact_size) in &expected_source_artifacts {
        source_physical_attestations[artifact_sha256].validate(*artifact_size)?;
    }
    profile_physical_attestation.validate(profile_manifest.catalog.size)?;
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
        let physical_attestation = source_physical_attestations[&source.catalog.sha256].clone();
        RemiCatalogResource::from_source_snapshot(source, physical_attestation, created_at)?
            .insert_exact(&tx)?;
    }
    let profile_resource = RemiCatalogResource::from_profile_revision(
        profile_manifest,
        profile_physical_attestation,
        created_at,
    )?;
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

    fn physical_attestation(
        catalog_size: u64,
        digest_byte: char,
    ) -> RemiCatalogPhysicalAttestation {
        let chunk_count = portable_chunk_count_v1(catalog_size).unwrap();
        RemiCatalogPhysicalAttestation::new(
            PortableManifestAttestationV1 {
                sha256: digest(digest_byte),
                size: portable_manifest_size_v1(chunk_count).unwrap(),
            },
            catalog_size,
        )
        .unwrap()
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

    fn source_attestations(
        sources: &[SourceSnapshotV1],
    ) -> BTreeMap<String, RemiCatalogPhysicalAttestation> {
        sources
            .iter()
            .map(|source| {
                (
                    source.catalog.sha256.clone(),
                    physical_attestation(source.catalog.size, '8'),
                )
            })
            .collect()
    }

    fn register_fixture(
        conn: &Connection,
        sources: &[SourceSnapshotV1],
        profile: &ProfileRevisionV2,
        created_at: i64,
    ) -> Result<()> {
        register_profile_catalog_revision(
            conn,
            sources,
            &source_attestations(sources),
            profile,
            physical_attestation(profile.catalog.size, '9'),
            created_at,
        )
    }

    #[test]
    fn exact_published_manifests_register_atomically_and_replay_idempotently() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let source = source_manifest();
        let profile = profile_manifest(&source);

        register_fixture(&conn, std::slice::from_ref(&source), &profile, 100).unwrap();
        register_fixture(&conn, std::slice::from_ref(&source), &profile, 200).unwrap();

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
        let stored_source =
            RemiCatalogResource::find_by_sha256(&conn, &source.manifest_sha256().unwrap())
                .unwrap()
                .unwrap();
        assert_eq!(stored_source.created_at, 100);
        assert_eq!(
            stored_source.physical_attestation,
            physical_attestation(source.catalog.size, '8')
        );
        let stored_profile =
            RemiCatalogResource::find_by_sha256(&conn, &profile.manifest_sha256().unwrap())
                .unwrap()
                .unwrap();
        assert_eq!(
            stored_profile.physical_attestation,
            physical_attestation(profile.catalog.size, '9')
        );
    }

    #[test]
    fn authenticated_root_change_can_register_same_projection_artifact() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let source = source_manifest();
        let profile = profile_manifest(&source);
        register_fixture(&conn, std::slice::from_ref(&source), &profile, 100).unwrap();

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

        register_fixture(&conn, std::slice::from_ref(&alias), &alias_profile, 200).unwrap();

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

        assert!(register_fixture(&conn, &[mixed], &profile, 100).is_err());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_catalog_resources", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }

    #[test]
    fn registration_rejects_missing_extra_and_noncanonical_attestation_keys() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let source = source_manifest();
        let profile = profile_manifest(&source);
        let profile_attestation = physical_attestation(profile.catalog.size, '9');

        for source_attestations in [
            BTreeMap::new(),
            BTreeMap::from([(digest('1'), physical_attestation(1, '7'))]),
            BTreeMap::from([("A".repeat(64), physical_attestation(1, '7'))]),
        ] {
            assert!(
                register_profile_catalog_revision(
                    &conn,
                    std::slice::from_ref(&source),
                    &source_attestations,
                    &profile,
                    profile_attestation.clone(),
                    100,
                )
                .is_err()
            );
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_catalog_resources", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }

    #[test]
    fn invalid_portable_manifest_digest_is_rejected_before_persistence() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let source = source_manifest();
        let error = RemiCatalogResource::from_source_snapshot(
            &source,
            RemiCatalogPhysicalAttestation {
                portable_manifest: PortableManifestAttestationV1 {
                    sha256: "A".repeat(64),
                    size: 96,
                },
                chunk_size: PORTABLE_CHUNK_SIZE_V1,
                chunk_count: 1,
            },
            100,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("exactly 64 lowercase hexadecimal characters"),
            "{error}"
        );
    }
}
