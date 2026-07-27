// conary-core/src/ccs/budget.rs

//! The single canonical structural and operator-resource budget for CCS v2.
//!
//! Both ends of the format share this owner. Authoring preflight admits the
//! authority it is about to sign, and untrusted inspection plus signature-first
//! verification admit the authority they decode. There is no second table of
//! limits, so the writer cannot construct a package the reader refuses.
//!
//! Budgeting is structural, not one guessed serialized-byte ceiling. Counts,
//! per-item string lengths, install-path depth, aggregate variable-length
//! pools, payload objects, and CBOR nesting are each an explicit dimension
//! with its own typed diagnostic. Byte ceilings are *derived* from those
//! dimensions: the global ceiling bounds decoder memory before allocation, and
//! the census ceiling bounds the exact document a specific package may occupy.

use super::v2::schema::{
    AuthorityDocumentV2, FileAuthorityV2, LifecycleAuthorityV2, PackageKindV2,
};
use crate::payload::PayloadNodeKind;

#[cfg(test)]
#[path = "budget/tests.rs"]
mod tests;

/// Exact CBOR cost of one `FileAuthorityV2` record excluding its variable
/// length authority (install path, component name, link target, xattrs).
///
/// A minimal regular-file record measures 238 bytes and the widest variant with
/// maximal scalars and config semantics measures 323.
/// `file_authority_fixed_bytes_is_an_upper_bound` proves this constant bounds
/// every node variant the schema admits.
const FILE_AUTHORITY_FIXED_BYTES: u64 = 384;

/// CBOR cost of the document map itself once every section is accounted for.
const AUTHORITY_ENVELOPE_BYTES: u64 = 4096;

/// The archived signature document has a fixed shape: algorithm, base64
/// Ed25519 signature, base64 public key, optional key id and timestamp.
const SIGNATURE_DOCUMENT_BYTES: u64 = 8 * 1024;

/// How much larger the TOML rendering of one record may be than its CBOR form.
const DEBUG_PROJECTION_TEXT_EXPANSION: u64 = 4;

/// Fixed table headers and comments the TOML projection always writes.
const DEBUG_PROJECTION_ENVELOPE_BYTES: u64 = 64 * 1024;

/// Every structurally independent way a CCS package can exceed its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetDimension {
    FileCount,
    ComponentCount,
    ConfigCount,
    ProvideCount,
    RequirementCount,
    RelationCount,
    LifecycleEntryCount,
    PathBytes,
    PathDepth,
    NameBytes,
    LinkTargetBytes,
    XattrCount,
    XattrNameBytes,
    XattrValueBytes,
    ScriptBodyBytes,
    TotalPathBytes,
    TotalXattrBytes,
    NonPayloadAuthorityBytes,
    AuthorityBytes,
    DebugProjectionBytes,
    AttestationBytes,
    SignatureBytes,
    MetadataBytes,
    PayloadObjectCount,
    PayloadObjectBytes,
    TotalPayloadBytes,
    ArchiveEntryCount,
    CborNestingDepth,
    Arithmetic,
}

impl BudgetDimension {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileCount => "file-count",
            Self::ComponentCount => "component-count",
            Self::ConfigCount => "config-count",
            Self::ProvideCount => "provide-count",
            Self::RequirementCount => "requirement-count",
            Self::RelationCount => "relation-count",
            Self::LifecycleEntryCount => "lifecycle-entry-count",
            Self::PathBytes => "path-bytes",
            Self::PathDepth => "path-depth",
            Self::NameBytes => "name-bytes",
            Self::LinkTargetBytes => "link-target-bytes",
            Self::XattrCount => "xattr-count",
            Self::XattrNameBytes => "xattr-name-bytes",
            Self::XattrValueBytes => "xattr-value-bytes",
            Self::ScriptBodyBytes => "script-body-bytes",
            Self::TotalPathBytes => "total-path-bytes",
            Self::TotalXattrBytes => "total-xattr-bytes",
            Self::NonPayloadAuthorityBytes => "non-payload-authority-bytes",
            Self::AuthorityBytes => "authority-bytes",
            Self::DebugProjectionBytes => "debug-projection-bytes",
            Self::AttestationBytes => "attestation-bytes",
            Self::SignatureBytes => "signature-bytes",
            Self::MetadataBytes => "metadata-bytes",
            Self::PayloadObjectCount => "payload-object-count",
            Self::PayloadObjectBytes => "payload-object-bytes",
            Self::TotalPayloadBytes => "total-payload-bytes",
            Self::ArchiveEntryCount => "archive-entry-count",
            Self::CborNestingDepth => "cbor-nesting-depth",
            Self::Arithmetic => "arithmetic",
        }
    }
}

impl std::fmt::Display for BudgetDimension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A typed refusal naming the exact dimension, location, observation, and limit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("CCS structural budget {dimension} exceeded at {field}: {observed} exceeds limit {limit}")]
pub struct BudgetError {
    pub dimension: BudgetDimension,
    pub field: String,
    pub observed: u64,
    pub limit: u64,
}

impl BudgetError {
    fn new(
        dimension: BudgetDimension,
        field: impl Into<String>,
        observed: u64,
        limit: u64,
    ) -> Self {
        Self {
            dimension,
            field: field.into(),
            observed,
            limit,
        }
    }

    fn overflow(field: impl Into<String>) -> Self {
        Self {
            dimension: BudgetDimension::Arithmetic,
            field: field.into(),
            observed: u64::MAX,
            limit: u64::MAX,
        }
    }
}

type BudgetResult<T> = Result<T, BudgetError>;

fn admit(
    dimension: BudgetDimension,
    field: impl Into<String>,
    observed: u64,
    limit: u64,
) -> BudgetResult<()> {
    if observed > limit {
        return Err(BudgetError::new(dimension, field, observed, limit));
    }
    Ok(())
}

fn sum(field: &str, values: impl IntoIterator<Item = u64>) -> BudgetResult<u64> {
    values.into_iter().try_fold(0_u64, |total, value| {
        total
            .checked_add(value)
            .ok_or_else(|| BudgetError::overflow(field))
    })
}

fn product(field: &str, left: u64, right: u64) -> BudgetResult<u64> {
    left.checked_mul(right)
        .ok_or_else(|| BudgetError::overflow(field))
}

/// Exact structural measurement of one decoded authority document.
///
/// The census is what makes writer and reader agree: authoring measures the
/// document it is about to sign, verification measures the document it decoded,
/// and both compare against the same derived ceiling.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorityCensus {
    pub files: u64,
    pub components: u64,
    pub config_entries: u64,
    pub payload_objects: u64,
    pub payload_bytes: u64,
    pub path_bytes: u64,
    pub xattr_bytes: u64,
    pub name_bytes: u64,
    /// Exact encoded size of every signed section other than the file array.
    pub non_payload_authority_bytes: u64,
}

/// The canonical CCS v2 budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CcsStructuralBudget {
    pub max_files: u64,
    pub max_components: u64,
    pub max_config_entries: u64,
    pub max_provides: u64,
    pub max_requirement_groups: u64,
    pub max_relation_groups: u64,
    pub max_lifecycle_entries: u64,
    pub max_path_bytes: u64,
    pub max_path_components: u64,
    pub max_name_bytes: u64,
    pub max_link_target_bytes: u64,
    pub max_xattrs_per_node: u64,
    pub max_xattr_name_bytes: u64,
    pub max_xattr_value_bytes: u64,
    pub max_script_body_bytes: u64,
    pub max_total_path_bytes: u64,
    pub max_total_xattr_bytes: u64,
    pub max_non_payload_authority_bytes: u64,
    pub max_attestation_bytes: u64,
    pub max_payload_object_bytes: u64,
    pub max_total_payload_bytes: u64,
    pub max_cbor_nesting_depth: usize,
}

/// The budget every CCS producer and consumer in this workspace uses.
pub const CCS_BUDGET: CcsStructuralBudget = CcsStructuralBudget::DEFAULT;

impl CcsStructuralBudget {
    pub const DEFAULT: Self = Self {
        // Payload node count. The largest ordinary distribution packages
        // measured here sit near 60,000 nodes.
        max_files: 1 << 20,
        max_components: 1024,
        max_config_entries: 1 << 20,
        max_provides: 1 << 16,
        max_requirement_groups: 1 << 16,
        max_relation_groups: 1 << 16,
        max_lifecycle_entries: 1 << 16,
        // POSIX PATH_MAX and a depth well beyond any real install tree.
        max_path_bytes: 4096,
        max_path_components: 128,
        max_name_bytes: 1024,
        max_link_target_bytes: 4096,
        max_xattrs_per_node: 256,
        max_xattr_name_bytes: 255,
        max_xattr_value_bytes: 64 * 1024,
        max_script_body_bytes: 4 * 1024 * 1024,
        // Aggregate variable-length pools. These, not a per-document byte
        // guess, are what bound decoder memory for a maximal package.
        max_total_path_bytes: 128 * 1024 * 1024,
        max_total_xattr_bytes: 64 * 1024 * 1024,
        max_non_payload_authority_bytes: 128 * 1024 * 1024,
        max_attestation_bytes: 4 * 1024 * 1024,
        max_payload_object_bytes: 512 * 1024 * 1024,
        max_total_payload_bytes: 64 * 1024 * 1024 * 1024,
        max_cbor_nesting_depth: 64,
    };

    /// Decoder-memory ceiling for signed authority, derived from the structural
    /// dimensions above rather than chosen as a serialized-byte constant.
    ///
    /// A reader applies this before reading a declared MANIFEST entry, so a
    /// hostile length declaration fails before allocation. The much tighter
    /// [`Self::authority_bytes_ceiling`] then applies to the decoded census.
    pub fn max_authority_bytes(&self) -> u64 {
        let files = product(
            "max_authority_bytes",
            self.max_files,
            FILE_AUTHORITY_FIXED_BYTES,
        )
        .unwrap_or(u64::MAX);
        sum(
            "max_authority_bytes",
            [
                AUTHORITY_ENVELOPE_BYTES,
                files,
                self.max_total_path_bytes,
                self.max_total_xattr_bytes,
                self.max_non_payload_authority_bytes,
            ],
        )
        .unwrap_or(u64::MAX)
    }

    /// Exact ceiling this specific package's authority may occupy.
    pub fn authority_bytes_ceiling(&self, census: &AuthorityCensus) -> BudgetResult<u64> {
        let files = product(
            "authority_bytes_ceiling",
            census.files,
            FILE_AUTHORITY_FIXED_BYTES,
        )?;
        sum(
            "authority_bytes_ceiling",
            [
                AUTHORITY_ENVELOPE_BYTES,
                files,
                census.path_bytes,
                census.xattr_bytes,
                census.name_bytes,
                census.non_payload_authority_bytes,
            ],
        )
    }

    /// The diagnostic TOML projection is a text rendering of authority the
    /// package already declares, so its ceiling scales with the same census.
    ///
    /// Text keys, quoting, and table headers cost more per record than CBOR, so
    /// the projection is allowed a bounded expansion over the authority
    /// ceiling. It is still structural: a package that declares little
    /// authority may not ship a large projection.
    pub fn debug_projection_bytes_ceiling(&self, census: &AuthorityCensus) -> BudgetResult<u64> {
        let authority = self.authority_bytes_ceiling(census)?;
        let expanded = product(
            "debug_projection_bytes_ceiling",
            authority,
            DEBUG_PROJECTION_TEXT_EXPANSION,
        )?;
        sum(
            "debug_projection_bytes_ceiling",
            [expanded, DEBUG_PROJECTION_ENVELOPE_BYTES],
        )
    }

    /// Archive entries a maximal CCS package may contain.
    ///
    /// One object per unique payload digest, the two-hex fanout directories,
    /// the `objects` root, and the control documents. CCS owns this bound
    /// instead of the generic foreign-archive entry guard, so the writer and
    /// reader cannot disagree about how many entries are admissible.
    pub fn max_archive_entries(&self) -> u64 {
        self.max_files.saturating_add(256 + 1 + 8)
    }

    pub fn admit_archive_entry(&self, entries_seen: u64) -> BudgetResult<()> {
        admit(
            BudgetDimension::ArchiveEntryCount,
            "archive entries",
            entries_seen,
            self.max_archive_entries(),
        )
    }

    pub fn signature_bytes_ceiling(&self) -> u64 {
        SIGNATURE_DOCUMENT_BYTES
    }

    pub fn attestation_bytes_ceiling(&self) -> u64 {
        self.max_attestation_bytes
    }

    /// Aggregate ceiling on every control document in one archive.
    pub fn metadata_bytes_ceiling(&self, census: &AuthorityCensus) -> BudgetResult<u64> {
        let authority = self.authority_bytes_ceiling(census)?;
        let projection = self.debug_projection_bytes_ceiling(census)?;
        sum(
            "metadata_bytes_ceiling",
            [
                authority,
                projection,
                self.signature_bytes_ceiling(),
                self.attestation_bytes_ceiling(),
                self.attestation_bytes_ceiling(),
            ],
        )
    }

    /// Admit a declared or observed control-document length.
    pub fn admit_control_bytes(
        &self,
        dimension: BudgetDimension,
        field: &str,
        observed: u64,
        limit: u64,
    ) -> BudgetResult<()> {
        admit(dimension, field, observed, limit)
    }

    /// Decode signed authority under the nesting-depth bound.
    ///
    /// `ciborium` refuses a declared nesting depth beyond the limit before it
    /// descends, so a hostile depth declaration costs no allocation.
    pub fn decode_authority(&self, raw: &[u8]) -> anyhow::Result<AuthorityDocumentV2> {
        admit(
            BudgetDimension::AuthorityBytes,
            "MANIFEST",
            raw.len() as u64,
            self.max_authority_bytes(),
        )?;
        ciborium::de::from_reader_with_recursion_limit(raw, self.max_cbor_nesting_depth)
            .map_err(|error| anyhow::anyhow!("invalid CCS v2 MANIFEST: {error}"))
    }

    /// Measure and admit a decoded authority document against every dimension.
    ///
    /// This is the one function authoring preflight and verification share.
    pub fn admit_authority(
        &self,
        authority: &AuthorityDocumentV2,
    ) -> BudgetResult<AuthorityCensus> {
        let mut census = AuthorityCensus::default();

        self.admit_identity_strings(authority, &mut census)?;
        self.admit_sections(authority, &mut census)?;

        census.components = authority.components.len() as u64;
        admit(
            BudgetDimension::ComponentCount,
            "components",
            census.components,
            self.max_components,
        )?;
        for name in authority.components.keys() {
            self.admit_name(name, "components")?;
            census.name_bytes = sum("components", [census.name_bytes, name.len() as u64])?;
        }

        if let PackageKindV2::Package(data) = &authority.kind {
            census.files = data.files.len() as u64;
            admit(
                BudgetDimension::FileCount,
                "kind.package.files",
                census.files,
                self.max_files,
            )?;
            let mut payload_objects = std::collections::BTreeSet::new();
            for file in &data.files {
                self.admit_file(file, &mut census, &mut payload_objects)?;
            }
            census.payload_objects = payload_objects.len() as u64;
            admit(
                BudgetDimension::PayloadObjectCount,
                "kind.package.files.content",
                census.payload_objects,
                self.max_files,
            )?;

            census.config_entries = data.config.len() as u64;
            admit(
                BudgetDimension::ConfigCount,
                "kind.package.config",
                census.config_entries,
                self.max_config_entries,
            )?;
            for config in &data.config {
                self.admit_path(&config.path, "kind.package.config.path")?;
                census.path_bytes = sum(
                    "kind.package.config.path",
                    [census.path_bytes, config.path.len() as u64],
                )?;
            }
        }

        admit(
            BudgetDimension::TotalPathBytes,
            "kind.package.files.path",
            census.path_bytes,
            self.max_total_path_bytes,
        )?;
        admit(
            BudgetDimension::TotalXattrBytes,
            "kind.package.files.node.xattrs",
            census.xattr_bytes,
            self.max_total_xattr_bytes,
        )?;
        admit(
            BudgetDimension::TotalPayloadBytes,
            "kind.package.files.content.size",
            census.payload_bytes,
            self.max_total_payload_bytes,
        )?;
        Ok(census)
    }

    /// Prove a serialized authority document fits the ceiling its own structure
    /// derives. Authoring runs this on the bytes it is about to sign and
    /// verification runs it on the exact archived bytes.
    pub fn admit_encoded_authority(
        &self,
        census: &AuthorityCensus,
        encoded_len: u64,
    ) -> BudgetResult<()> {
        admit(
            BudgetDimension::AuthorityBytes,
            "MANIFEST",
            encoded_len,
            self.authority_bytes_ceiling(census)?,
        )
    }

    fn admit_identity_strings(
        &self,
        authority: &AuthorityDocumentV2,
        census: &mut AuthorityCensus,
    ) -> BudgetResult<()> {
        for (field, value) in [
            ("identity.name", Some(authority.identity.name.as_str())),
            (
                "identity.version",
                Some(authority.identity.version.as_str()),
            ),
            (
                "identity.release",
                Some(authority.identity.release.as_str()),
            ),
            (
                "identity.architecture",
                authority.identity.architecture.as_deref(),
            ),
            ("identity.platform", authority.identity.platform.as_deref()),
        ] {
            let Some(value) = value else { continue };
            self.admit_name(value, field)?;
            census.name_bytes = sum(field, [census.name_bytes, value.len() as u64])?;
        }
        Ok(())
    }

    /// Measure every signed section other than the payload file array exactly.
    ///
    /// These sections are count-bounded first, so encoding them to obtain an
    /// exact byte census is itself bounded work.
    fn admit_sections(
        &self,
        authority: &AuthorityDocumentV2,
        census: &mut AuthorityCensus,
    ) -> BudgetResult<()> {
        admit(
            BudgetDimension::ProvideCount,
            "provides",
            authority.provides.len() as u64,
            self.max_provides,
        )?;
        admit(
            BudgetDimension::RequirementCount,
            "requirements",
            authority.requirements.len() as u64,
            self.max_requirement_groups,
        )?;
        admit(
            BudgetDimension::RelationCount,
            "relations",
            authority.relations.len() as u64,
            self.max_relation_groups,
        )?;
        self.admit_lifecycle(&authority.lifecycle)?;

        let mut bytes = 0_u64;
        for (field, encoded) in [
            ("identity", encoded_len(&authority.identity, "identity")?),
            ("provides", encoded_len(&authority.provides, "provides")?),
            (
                "requirements",
                encoded_len(&authority.requirements, "requirements")?,
            ),
            ("relations", encoded_len(&authority.relations, "relations")?),
            (
                "capabilities",
                encoded_len(&authority.capabilities, "capabilities")?,
            ),
            (
                "components",
                encoded_len(&authority.components, "components")?,
            ),
            ("lifecycle", encoded_len(&authority.lifecycle, "lifecycle")?),
            (
                "provenance",
                encoded_len(&authority.provenance, "provenance")?,
            ),
        ] {
            bytes = sum(field, [bytes, encoded])?;
        }
        admit(
            BudgetDimension::NonPayloadAuthorityBytes,
            "authority sections",
            bytes,
            self.max_non_payload_authority_bytes,
        )?;
        census.non_payload_authority_bytes = bytes;
        Ok(())
    }

    fn admit_lifecycle(&self, lifecycle: &LifecycleAuthorityV2) -> BudgetResult<()> {
        let native_entries = lifecycle
            .native_lifecycle
            .as_ref()
            .map_or(0, |bundle| bundle.entries.len() as u64);
        let entries = sum(
            "lifecycle",
            [
                lifecycle.users.len() as u64,
                lifecycle.groups.len() as u64,
                lifecycle.directories.len() as u64,
                lifecycle.services.len() as u64,
                lifecycle.systemd.len() as u64,
                lifecycle.tmpfiles.len() as u64,
                lifecycle.sysctl.len() as u64,
                lifecycle.alternatives.len() as u64,
                lifecycle.script_capabilities.len() as u64,
                native_entries,
            ],
        )?;
        admit(
            BudgetDimension::LifecycleEntryCount,
            "lifecycle",
            entries,
            self.max_lifecycle_entries,
        )?;
        for (field, script) in [
            ("lifecycle.post_install", lifecycle.post_install.as_ref()),
            ("lifecycle.pre_remove", lifecycle.pre_remove.as_ref()),
        ] {
            let Some(script) = script else { continue };
            admit(
                BudgetDimension::ScriptBodyBytes,
                field,
                script.body.len() as u64,
                self.max_script_body_bytes,
            )?;
        }
        Ok(())
    }

    fn admit_file(
        &self,
        file: &FileAuthorityV2,
        census: &mut AuthorityCensus,
        payload_objects: &mut std::collections::BTreeSet<String>,
    ) -> BudgetResult<()> {
        self.admit_path(&file.path, "kind.package.files.path")?;
        self.admit_name(&file.component, "kind.package.files.component")?;
        census.path_bytes = sum(
            "kind.package.files.path",
            [census.path_bytes, file.path.len() as u64],
        )?;
        census.name_bytes = sum(
            "kind.package.files.component",
            [census.name_bytes, file.component.len() as u64],
        )?;

        match &file.node.kind {
            PayloadNodeKind::Symlink { target } => {
                self.admit_link_target(target, "kind.package.files.node.kind.target")?;
                census.path_bytes = sum(
                    "kind.package.files.node",
                    [census.path_bytes, target.len() as u64],
                )?;
            }
            PayloadNodeKind::Hardlink { target, identity } => {
                self.admit_link_target(target, "kind.package.files.node.kind.target")?;
                self.admit_link_target(identity, "kind.package.files.node.kind.identity")?;
                census.path_bytes = sum(
                    "kind.package.files.node",
                    [
                        census.path_bytes,
                        target.len() as u64,
                        identity.len() as u64,
                    ],
                )?;
            }
            PayloadNodeKind::Regular {
                hardlink_identity: Some(identity),
            } => {
                self.admit_link_target(identity, "kind.package.files.node.kind.hardlink_identity")?;
                census.path_bytes = sum(
                    "kind.package.files.node",
                    [census.path_bytes, identity.len() as u64],
                )?;
            }
            _ => {}
        }

        admit(
            BudgetDimension::XattrCount,
            "kind.package.files.node.xattrs",
            file.node.xattrs.len() as u64,
            self.max_xattrs_per_node,
        )?;
        for (name, value) in &file.node.xattrs {
            admit(
                BudgetDimension::XattrNameBytes,
                "kind.package.files.node.xattrs",
                name.len() as u64,
                self.max_xattr_name_bytes,
            )?;
            admit(
                BudgetDimension::XattrValueBytes,
                "kind.package.files.node.xattrs",
                value.len() as u64,
                self.max_xattr_value_bytes,
            )?;
            census.xattr_bytes = sum(
                "kind.package.files.node.xattrs",
                [census.xattr_bytes, name.len() as u64, value.len() as u64],
            )?;
        }

        if let Some(content) = &file.content {
            admit(
                BudgetDimension::PayloadObjectBytes,
                "kind.package.files.content.size",
                content.size,
                self.max_payload_object_bytes,
            )?;
            if payload_objects.insert(content.sha256.clone()) {
                census.payload_bytes = sum(
                    "kind.package.files.content.size",
                    [census.payload_bytes, content.size],
                )?;
            }
        }
        Ok(())
    }

    fn admit_path(&self, path: &str, field: &str) -> BudgetResult<()> {
        admit(
            BudgetDimension::PathBytes,
            field,
            path.len() as u64,
            self.max_path_bytes,
        )?;
        let depth = path.split('/').filter(|part| !part.is_empty()).count() as u64;
        admit(
            BudgetDimension::PathDepth,
            field,
            depth,
            self.max_path_components,
        )
    }

    fn admit_name(&self, name: &str, field: &str) -> BudgetResult<()> {
        admit(
            BudgetDimension::NameBytes,
            field,
            name.len() as u64,
            self.max_name_bytes,
        )
    }

    fn admit_link_target(&self, target: &str, field: &str) -> BudgetResult<()> {
        admit(
            BudgetDimension::LinkTargetBytes,
            field,
            target.len() as u64,
            self.max_link_target_bytes,
        )
    }
}

fn encoded_len<T: serde::Serialize>(value: &T, field: &str) -> BudgetResult<u64> {
    let mut counter = ByteCounter::default();
    ciborium::into_writer(value, &mut counter).map_err(|_| BudgetError::overflow(field))?;
    Ok(counter.bytes)
}

/// Counts encoded bytes without retaining them, so measuring a section never
/// duplicates the document.
#[derive(Default)]
struct ByteCounter {
    bytes: u64,
}

impl std::io::Write for ByteCounter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(data.len() as u64);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
