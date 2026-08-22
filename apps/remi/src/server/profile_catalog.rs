// apps/remi/src/server/profile_catalog.rs

//! Typed projections from one pinned immutable profile catalog.
//!
//! This is the serving-side owner for profile package metadata. It deliberately
//! accepts a [`PinnedProfileCatalog`] instead of opening SQLite itself, so every
//! read remains bound to the reader pin and exact activated revision held by
//! that handle.

use anyhow::{Context, Result, bail};
use conary_core::repository::catalog::{
    CatalogPackageNamePageV1, CatalogPackageRecordV1, CatalogRequirementGroupV1,
};
use conary_core::repository::remi_metadata::{
    RemiProvide, RemiRequirement, RemiRequirementGroup, RemiSparseResolutionVersionEntry,
    RemiSparseRevision,
};

use super::catalog_authority::PinnedProfileCatalog;

/// A read-only projection view whose lifetime is bounded by the exact pinned
/// catalog reader supplied by the caller.
pub struct ProfileCatalog<'a> {
    pinned: &'a PinnedProfileCatalog,
}

impl<'a> ProfileCatalog<'a> {
    /// Borrow one pinned immutable profile catalog for typed serving reads.
    #[must_use]
    pub fn new(pinned: &'a PinnedProfileCatalog) -> Self {
        Self { pinned }
    }

    /// Return the exact source profile selected by the activation pointer.
    #[must_use]
    pub fn source_profile(&self) -> &str {
        self.pinned.source_profile()
    }

    /// Return the full exact profile-revision manifest digest.
    #[must_use]
    pub fn profile_revision_sha256(&self) -> &str {
        self.pinned.profile_revision_sha256()
    }

    /// Return the monotonic activation fence captured by this reader.
    #[must_use]
    pub fn fencing_epoch(&self) -> i64 {
        self.pinned.fencing_epoch()
    }

    /// Return the sparse wire revision for this exact profile reader.
    ///
    /// The wire state ID is the first 128 bits of the already verified
    /// profile-revision SHA-256. The complete 256-bit identity remains
    /// available through [`Self::profile_revision_sha256`]; the bounded wire
    /// field is only the sparse protocol's state token. A non-positive fencing
    /// epoch is rejected rather than converted with a wrapping cast.
    pub fn revision(&self) -> Result<RemiSparseRevision> {
        sparse_revision(self.fencing_epoch(), self.profile_revision_sha256())
    }

    /// Return one deterministic page of downloadable package names.
    pub fn package_name_page(
        &self,
        offset: usize,
        limit: usize,
        minimum_size: u64,
    ) -> Result<CatalogPackageNamePageV1> {
        self.pinned
            .reader()
            .find_downloadable_package_name_page(offset, limit, minimum_size)
            .map_err(anyhow::Error::from)
    }

    /// Look up every version for one exact package name and project it into
    /// Remi's resolution wire type. The immutable catalog reader supplies
    /// package-key ordering, while each package's provide and requirement
    /// ordinals are preserved from the verified catalog contract.
    pub fn find_downloadable_packages_by_name(
        &self,
        name: &str,
        minimum_size: u64,
    ) -> Result<Vec<RemiSparseResolutionVersionEntry>> {
        self.find_package_records_by_name(name)?
            .into_iter()
            .filter(|package| package.size >= minimum_size)
            .map(|package| project_package(&package))
            .collect()
    }

    /// Look up raw catalog records for one exact package name.
    ///
    /// The core reader orders by package key. This serving boundary applies
    /// the complete public tuple as a final deterministic ordering contract,
    /// which remains stable if the storage query gains another access path.
    pub fn find_package_records_by_name(&self, name: &str) -> Result<Vec<CatalogPackageRecordV1>> {
        let mut packages = self
            .pinned
            .reader()
            .find_packages_by_name(name)
            .map_err(anyhow::Error::from)?;
        packages.sort_by(|left, right| {
            (
                &left.name,
                &left.version,
                &left.package_release,
                &left.architecture,
                &left.package_key_sha256,
            )
                .cmp(&(
                    &right.name,
                    &right.version,
                    &right.package_release,
                    &right.architecture,
                    &right.package_key_sha256,
                ))
        });
        Ok(packages)
    }

    /// Project one verified catalog package into the sparse resolution wire
    /// type. This is public so callers that already hold a bounded catalog
    /// batch can retain the catalog's deterministic record order.
    pub fn project_package(
        &self,
        package: &CatalogPackageRecordV1,
    ) -> Result<RemiSparseResolutionVersionEntry> {
        project_package(package)
    }

    /// Expose the underlying pinned handle for callers that need a lifetime
    /// witness while composing several immutable projections.
    #[must_use]
    pub fn pinned(&self) -> &'a PinnedProfileCatalog {
        self.pinned
    }
}

fn sparse_revision(
    fencing_epoch: i64,
    profile_revision_sha256: &str,
) -> Result<RemiSparseRevision> {
    if fencing_epoch <= 0 {
        bail!("immutable profile fencing epoch must be positive");
    }
    let sequence = u64::try_from(fencing_epoch)
        .context("immutable profile fencing epoch exceeds wire range")?;
    if profile_revision_sha256.len() != 64
        || !profile_revision_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("immutable profile revision must be exactly 64 lowercase hexadecimal characters");
    }
    RemiSparseRevision::new(sequence, &profile_revision_sha256[..32]).map_err(anyhow::Error::msg)
}

fn project_package(package: &CatalogPackageRecordV1) -> Result<RemiSparseResolutionVersionEntry> {
    let metadata = package
        .metadata
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .with_context(|| {
            format!(
                "immutable catalog package '{}' version '{}' has malformed persisted metadata",
                package.name, package.version
            )
        })?;
    let size = i64::try_from(package.size).with_context(|| {
        format!(
            "immutable catalog package '{}' version '{}' size {} exceeds Remi wire range",
            package.name, package.version, package.size
        )
    })?;

    Ok(RemiSparseResolutionVersionEntry {
        version: package.version.clone(),
        release: (!package.package_release.is_empty()).then(|| package.package_release.clone()),
        provides: package
            .provides
            .iter()
            .map(|provide| RemiProvide {
                capability: provide.capability.clone(),
                version: provide.version.clone(),
                version_relation: provide.version_relation,
                kind: provide.kind.clone(),
                raw: provide.raw.clone(),
                version_scheme: provide.version_scheme,
                architecture_qualifier: provide.architecture_qualifier.clone(),
            })
            .collect(),
        requirement_groups: package
            .requirement_groups
            .iter()
            .map(project_requirement_group)
            .collect(),
        architecture: package.architecture.clone(),
        size,
        metadata,
    })
}

fn project_requirement_group(group: &CatalogRequirementGroupV1) -> RemiRequirementGroup {
    RemiRequirementGroup {
        kind: group.kind.clone(),
        behavior: group.behavior.clone(),
        description: group.description.clone(),
        native_text: group.native_text.clone(),
        expression_json: group.expression_json.clone(),
        clauses: group
            .atoms
            .iter()
            .map(|atom| RemiRequirement {
                capability: atom.capability.clone(),
                version_constraint: atom.version_constraint.clone(),
                kind: atom.kind.clone(),
                dependency_type: atom.dependency_type.clone(),
                raw: atom.raw.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::repository::catalog::{
        CatalogPackageOriginV1, CatalogProvideRecordV1, CatalogRequirementAtomV1,
    };
    use conary_core::repository::dependency_model::{
        CapabilityProvenance, ProvideArchitectureQualifier,
    };
    use conary_core::repository::versioning::VersionScheme;

    fn package() -> CatalogPackageRecordV1 {
        CatalogPackageRecordV1 {
            package_key_sha256: "a".repeat(64),
            origin: CatalogPackageOriginV1::Source {
                source_identity: "source".to_string(),
                repository_identity: "repository".to_string(),
            },
            source_profile: "profile".to_string(),
            name: "demo".to_string(),
            version: "1.2.3".to_string(),
            package_release: "4".to_string(),
            architecture: Some("x86_64".to_string()),
            debian_multi_arch: None,
            description: None,
            checksum: "b".repeat(64),
            size: 4096,
            download_url: "https://example.test/demo".to_string(),
            metadata: Some(r#"{"z":2,"a":1}"#.to_string()),
            is_security_update: false,
            severity: None,
            cve_ids: None,
            advisory_id: None,
            advisory_url: None,
            version_scheme: VersionScheme::Rpm,
            provides: vec![CatalogProvideRecordV1 {
                capability: "demo".to_string(),
                version: Some("1.2.3".to_string()),
                version_relation: Some(
                    conary_core::repository::dependency_model::ProvideVersionRelation::Equal,
                ),
                kind: "name".to_string(),
                raw: Some("demo = 1.2.3".to_string()),
                version_scheme: VersionScheme::Rpm,
                architecture_qualifier: ProvideArchitectureQualifier::Implicit,
                provenance: CapabilityProvenance::ExactIdentity,
            }],
            requirement_groups: vec![CatalogRequirementGroupV1 {
                kind: "requires".to_string(),
                behavior: "all".to_string(),
                description: Some("runtime".to_string()),
                native_text: Some("libc".to_string()),
                expression_json: r#"{"kind":"atom"}"#.to_string(),
                atoms: vec![CatalogRequirementAtomV1 {
                    capability: "libc".to_string(),
                    version_constraint: Some(">= 2".to_string()),
                    kind: "name".to_string(),
                    dependency_type: "runtime".to_string(),
                    raw: Some("libc >= 2".to_string()),
                }],
            }],
        }
    }

    #[test]
    fn sparse_revision_preserves_exact_sequence_and_digest_identity() {
        let digest = "0123456789abcdef".repeat(4);
        let revision = sparse_revision(17, &digest).expect("valid profile revision");

        assert_eq!(revision.sequence, 17);
        assert_eq!(
            revision.state_id.as_str(),
            "0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn sparse_revision_rejects_negative_or_malformed_identity() {
        assert!(sparse_revision(-1, &"a".repeat(64)).is_err());
        assert!(sparse_revision(0, &"a".repeat(64)).is_err());
        assert!(sparse_revision(1, &"A".repeat(64)).is_err());
        assert!(sparse_revision(1, &"a".repeat(63)).is_err());
    }

    #[test]
    fn package_projection_maps_metadata_and_typed_dependency_groups() {
        let projected = project_package(&package()).expect("project immutable package");

        assert_eq!(projected.version, "1.2.3");
        assert_eq!(projected.release.as_deref(), Some("4"));
        assert_eq!(projected.size, 4096);
        assert_eq!(
            projected.metadata,
            Some(serde_json::json!({"z": 2, "a": 1}))
        );
        assert_eq!(projected.provides.len(), 1);
        assert_eq!(projected.provides[0].capability, "demo");
        assert_eq!(projected.requirement_groups.len(), 1);
        assert_eq!(
            projected.requirement_groups[0].clauses[0].capability,
            "libc"
        );
    }

    #[test]
    fn package_projection_rejects_sizes_that_do_not_fit_wire_i64() {
        let mut package = package();
        package.size = u64::MAX;

        let error = project_package(&package).expect_err("oversized package must fail closed");
        assert!(error.to_string().contains("exceeds Remi wire range"));
    }

    #[test]
    fn package_projection_preserves_catalog_order() {
        let mut package = package();
        package.provides.push(CatalogProvideRecordV1 {
            capability: "zzz".to_string(),
            version: None,
            version_relation: None,
            kind: "name".to_string(),
            raw: None,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::ExactIdentity,
        });

        let projected = project_package(&package).expect("project immutable package");
        assert_eq!(projected.provides[0].capability, "demo");
        assert_eq!(projected.provides[1].capability, "zzz");
    }
}
