// apps/remi/src/server/profile_catalog.rs

//! Typed projections from one pinned immutable profile catalog.
//!
//! This is the serving-side owner for profile package metadata. It deliberately
//! accepts a [`PinnedProfileCatalog`] instead of opening SQLite itself, so every
//! read remains bound to the reader pin and exact selected revision held by
//! that handle.

use anyhow::{Context, Result};
use conary_core::repository::catalog::{
    CatalogPackageNamePageV1, CatalogPackageRecordV1, CatalogRequirementGroupV1,
};
use conary_core::repository::remi_metadata::{RemiProvide, RemiRequirement, RemiRequirementGroup};

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

    /// Return the exact selected source profile.
    #[must_use]
    pub fn source_profile(&self) -> &str {
        self.pinned.source_profile()
    }

    /// Return the full exact profile-revision manifest digest.
    #[must_use]
    pub fn profile_revision_sha256(&self) -> &str {
        self.pinned.profile_revision_sha256()
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
    ) -> Result<Vec<conary_core::repository::remi_metadata::RemiSparseVersionEntry>> {
        self.find_downloadable_package_records_by_name(name, minimum_size)?
            .into_iter()
            .map(|package| project_package(&package))
            .collect()
    }

    /// Look up raw downloadable catalog records for one exact package name.
    /// Different native variants remain visible across every declared member.
    /// Exact-identity duplicates were already resolved during profile
    /// construction, before this immutable reader could be opened.
    pub(crate) fn find_downloadable_package_records_by_name(
        &self,
        name: &str,
        minimum_size: u64,
    ) -> Result<Vec<CatalogPackageRecordV1>> {
        let mut packages = self
            .pinned
            .reader()
            .find_packages_by_name(name)
            .map_err(anyhow::Error::from)?
            .into_iter()
            .filter(|package| package.size >= minimum_size)
            .collect::<Vec<_>>();
        sort_packages(&mut packages);
        Ok(packages)
    }

    /// Look up raw catalog records for one exact package name.
    ///
    /// The serving boundary preserves every distinct native variant and
    /// applies the complete public tuple as a deterministic ordering contract.
    pub fn find_package_records_by_name(&self, name: &str) -> Result<Vec<CatalogPackageRecordV1>> {
        let mut packages = self
            .pinned
            .reader()
            .find_packages_by_name(name)
            .map_err(anyhow::Error::from)?;
        sort_packages(&mut packages);
        Ok(packages)
    }

    /// Return the one package bound to an exact immutable package key.
    pub(crate) fn find_package_record_by_key(
        &self,
        package_key_sha256: &str,
    ) -> Result<Option<CatalogPackageRecordV1>> {
        self.pinned
            .reader()
            .find_package_by_key(package_key_sha256)
            .map_err(anyhow::Error::from)
    }

    /// Return the authoritative package universe for this profile revision.
    /// Every distinct native variant remains available. Exact duplicates were
    /// collapsed, or rejected as conflicts, while the profile was built.
    pub(crate) fn package_records(&self) -> Result<Vec<CatalogPackageRecordV1>> {
        self.package_records_matching(|_| true)
    }

    /// Return the authoritative downloadable package universe after applying
    /// the caller's minimum-size eligibility threshold. This keeps sparse,
    /// search, index, prewarm, and benchmark projections on one universe.
    pub(crate) fn downloadable_package_records(
        &self,
        minimum_size: u64,
    ) -> Result<Vec<CatalogPackageRecordV1>> {
        self.package_records_matching(|package| package.size >= minimum_size)
    }

    fn package_records_matching(
        &self,
        eligible: impl Fn(&CatalogPackageRecordV1) -> bool,
    ) -> Result<Vec<CatalogPackageRecordV1>> {
        let mut packages = self
            .pinned
            .reader()
            .packages()
            .map_err(anyhow::Error::from)?
            .into_iter()
            .filter(eligible)
            .collect::<Vec<_>>();
        sort_packages(&mut packages);
        Ok(packages)
    }

    /// Project one verified catalog package into the sparse resolution wire
    /// type. This is public so callers that already hold a bounded catalog
    /// batch can retain the catalog's deterministic record order.
    pub fn project_package(
        &self,
        package: &CatalogPackageRecordV1,
    ) -> Result<conary_core::repository::remi_metadata::RemiSparseVersionEntry> {
        project_package(package)
    }

    /// Expose the underlying pinned handle for callers that need a lifetime
    /// witness while composing several immutable projections.
    #[must_use]
    pub fn pinned(&self) -> &'a PinnedProfileCatalog {
        self.pinned
    }
}

fn sort_packages(packages: &mut [CatalogPackageRecordV1]) {
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
}

fn project_package(
    package: &CatalogPackageRecordV1,
) -> Result<conary_core::repository::remi_metadata::RemiSparseVersionEntry> {
    let size = i64::try_from(package.size).with_context(|| {
        format!(
            "immutable catalog package '{}' version '{}' size {} exceeds Remi wire range",
            package.name, package.version, package.size
        )
    })?;

    Ok(
        conary_core::repository::remi_metadata::RemiSparseVersionEntry {
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
            converted: false,
            content_hash: None,
        },
    )
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
    use conary_core::db::models::RemiActiveProfileRevision;
    use conary_core::repository::catalog::{
        CatalogArtifactV1, CatalogContentV1, CatalogCountsV1, CatalogPackageOriginV1,
        CatalogProvideRecordV1, CatalogReader, CatalogRequirementAtomV1, CatalogScopeV1,
        CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V2, ProfileRevisionV2,
        ProfileSourceMemberV2, SourceStreamKindV1, SourceStreamV1, write_catalog_candidate,
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

    fn profile_manifest() -> ProfileRevisionV2 {
        ProfileRevisionV2 {
            schema_version: PROFILE_REVISION_SCHEMA_V2,
            profile: "profile".to_string(),
            projection_version: 1,
            members: vec![
                ProfileSourceMemberV2 {
                    ordinal: 0,
                    role: conary_core::repository::supported_profiles::ProfileSourceRole::Updates,
                    source_identity: "higher-source".to_string(),
                    repository_identity: "higher-repository".to_string(),
                    stream: SourceStreamV1 {
                        kind: SourceStreamKindV1::Release,
                        identity: "stable".to_string(),
                    },
                    precedence: 20,
                    required: true,
                    source_snapshot_sha256: "d".repeat(64),
                },
                ProfileSourceMemberV2 {
                    ordinal: 1,
                    role: conary_core::repository::supported_profiles::ProfileSourceRole::Base,
                    source_identity: "lower-source".to_string(),
                    repository_identity: "lower-repository".to_string(),
                    stream: SourceStreamV1 {
                        kind: SourceStreamKindV1::Release,
                        identity: "stable".to_string(),
                    },
                    precedence: 10,
                    required: true,
                    source_snapshot_sha256: "c".repeat(64),
                },
            ],
            catalog: CatalogArtifactV1 {
                sha256: "e".repeat(64),
                size: 1,
            },
            logical_digest_sha256: "f".repeat(64),
            counts: CatalogCountsV1::default(),
        }
    }

    fn profile_package(name: &str, member_ordinal: u32) -> CatalogPackageRecordV1 {
        let manifest = profile_manifest();
        let member = &manifest.members[member_ordinal as usize];
        let mut package = package();
        package.name = name.to_string();
        package.origin = CatalogPackageOriginV1::Profile {
            member_ordinal,
            source_identity: member.source_identity.clone(),
            repository_identity: member.repository_identity.clone(),
            source_snapshot_sha256: member.source_snapshot_sha256.clone(),
        };
        package
    }

    #[test]
    fn package_projection_maps_metadata_and_typed_dependency_groups() {
        let projected = project_package(&package()).expect("project immutable package");

        assert_eq!(projected.version, "1.2.3");
        assert_eq!(projected.release.as_deref(), Some("4"));
        assert_eq!(projected.size, 4096);
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

    #[test]
    fn profile_projection_preserves_distinct_variants_before_downloadability() {
        let mut higher_placeholder = profile_package("demo", 0);
        higher_placeholder.version = "2.0".to_string();
        higher_placeholder.size = 0;
        higher_placeholder.package_key_sha256.clear();
        higher_placeholder.provides.clear();
        higher_placeholder.requirement_groups.clear();
        let mut lower_artifact = profile_package("demo", 1);
        lower_artifact.version = "1.0".to_string();
        lower_artifact.size = 42;
        lower_artifact.package_key_sha256.clear();
        lower_artifact.provides.clear();
        lower_artifact.requirement_groups.clear();

        let manifest_template = profile_manifest();
        let evidence = manifest_template
            .members
            .iter()
            .map(|member| CatalogSourceEvidenceV1::SourceSnapshot {
                member_ordinal: member.ordinal,
                source_identity: member.source_identity.clone(),
                repository_identity: member.repository_identity.clone(),
                source_snapshot_sha256: member.source_snapshot_sha256.clone(),
            })
            .collect();
        let content = CatalogContentV1::new(
            CatalogScopeV1::Profile {
                profile: "profile".to_string(),
            },
            evidence,
            vec![higher_placeholder, lower_artifact],
        )
        .expect("build multi-member profile catalog");
        let root = tempfile::tempdir().expect("create profile catalog root");
        let catalog_path = root.path().join("catalog.sqlite");
        let binding =
            write_catalog_candidate(&catalog_path, &content).expect("write profile catalog");
        let mut manifest = manifest_template;
        manifest.catalog = binding.artifact.clone();
        manifest.logical_digest_sha256 = binding.logical_digest_sha256.clone();
        manifest.counts = binding.counts;
        let revision = manifest.manifest_sha256().expect("hash profile manifest");
        let reader = CatalogReader::open_verified(&catalog_path, &binding)
            .expect("open verified profile catalog");
        let pinned = PinnedProfileCatalog::from_verified_test_parts(
            RemiActiveProfileRevision {
                source_profile: "profile".to_string(),
                profile_revision_sha256: revision,
                fencing_epoch: 1,
                activation_run_id: uuid::Uuid::new_v4().to_string(),
                owner_instance_uuid: uuid::Uuid::new_v4().to_string(),
                activated_at: 1,
            },
            manifest,
            reader,
        );
        let catalog = ProfileCatalog::new(&pinned);

        let all = catalog
            .find_package_records_by_name("demo")
            .expect("load complete native variants");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].version, "1.0");
        assert_eq!(all[1].version, "2.0");

        let downloadable = catalog
            .find_downloadable_package_records_by_name("demo", 1)
            .expect("select authoritative downloadable tier");
        assert_eq!(downloadable.len(), 1);
        assert_eq!(downloadable[0].version, "1.0");
        assert_eq!(downloadable[0].size, 42);
    }
}
