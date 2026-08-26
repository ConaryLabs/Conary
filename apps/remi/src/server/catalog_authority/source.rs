// apps/remi/src/server/catalog_authority/source.rs

//! Exact durable source-catalog authority below one pinned profile revision.

use anyhow::{Context, Result, bail};
use conary_core::db::models::{RemiCatalogResource, RemiCatalogResourceKind};
use conary_core::repository::catalog::{
    CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogReader, ProfileSourceMemberV2,
    SourceSnapshotV1, verify_source_catalog_bundle,
};

use super::{CatalogAuthority, PinnedProfileCatalog};
use crate::server::open_runtime_db;

struct VerifiedSourceCatalog {
    manifest: SourceSnapshotV1,
    reader: CatalogReader,
}

impl CatalogAuthority {
    /// Reopen the exact durable source snapshot bound to one profile member.
    ///
    /// Native parity input materialization consumes every member rather than
    /// arriving through a package row, so member ordinal and manifest identity
    /// remain the only selection authority.
    pub(crate) fn source_snapshot_for_member(
        &self,
        pinned: &PinnedProfileCatalog,
        member_ordinal: u32,
    ) -> Result<SourceSnapshotV1> {
        let member = pinned
            .manifest()
            .members
            .iter()
            .find(|member| member.ordinal == member_ordinal)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "profile '{}' has no source member ordinal {}",
                    pinned.source_profile(),
                    member_ordinal
                )
            })?;
        Ok(self.verified_source_catalog(pinned, member)?.manifest)
    }

    /// Resolve the exact source snapshot that authenticated a package in one
    /// pinned profile revision.
    ///
    /// The profile catalog carries only the source-member identity and the
    /// content address of the source snapshot. This method resolves that
    /// address through durable operational resource metadata, then verifies
    /// the immutable source bundle before returning its owned manifest. No
    /// operational package, provide, or requirement rows participate in the
    /// lookup.
    pub fn source_snapshot_for_package(
        &self,
        pinned: &PinnedProfileCatalog,
        package: &CatalogPackageRecordV1,
    ) -> Result<SourceSnapshotV1> {
        if package.source_profile != pinned.source_profile() {
            bail!(
                "profile '{}' package '{}' carries source profile '{}'",
                pinned.source_profile(),
                package.name,
                package.source_profile
            );
        }
        let (member_ordinal, source_identity, repository_identity, source_snapshot_sha256) =
            match &package.origin {
                CatalogPackageOriginV1::Profile {
                    member_ordinal,
                    source_identity,
                    repository_identity,
                    source_snapshot_sha256,
                } => (
                    *member_ordinal,
                    source_identity.as_str(),
                    repository_identity.as_str(),
                    source_snapshot_sha256.as_str(),
                ),
                CatalogPackageOriginV1::Source { .. } => {
                    bail!(
                        "profile catalog package '{}' has source origin without a profile member",
                        package.name
                    );
                }
            };

        let member = pinned
            .manifest()
            .members
            .iter()
            .find(|member| member.ordinal == member_ordinal)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "profile '{}' package '{}' names missing source member ordinal {}",
                    pinned.source_profile(),
                    package.name,
                    member_ordinal
                )
            })?;
        if member.source_identity != source_identity
            || member.repository_identity != repository_identity
            || member.source_snapshot_sha256 != source_snapshot_sha256
        {
            bail!(
                "profile '{}' package '{}' origin disagrees with pinned member ordinal {}",
                pinned.source_profile(),
                package.name,
                member_ordinal
            );
        }

        let verified = self.verified_source_catalog(pinned, member)?;
        let matching_source_packages = verified
            .reader
            .find_packages_by_name(&package.name)
            .map_err(anyhow::Error::from)?
            .into_iter()
            .filter(|source_package| source_package_matches_profile(package, source_package))
            .count();
        if matching_source_packages != 1 {
            bail!(
                "source snapshot {} has {} records matching profile package '{}' version '{}'",
                source_snapshot_sha256,
                matching_source_packages,
                package.name,
                package.version
            );
        }

        Ok(verified.manifest)
    }

    fn verified_source_catalog(
        &self,
        pinned: &PinnedProfileCatalog,
        member: &ProfileSourceMemberV2,
    ) -> Result<VerifiedSourceCatalog> {
        let source_snapshot_sha256 = member.source_snapshot_sha256.clone();
        let db_path = self.db_path.clone();
        let resource = self.database_writer.execute(|| {
            let conn = open_runtime_db(&db_path).with_context(|| {
                format!(
                    "open Remi operational database for source snapshot {}",
                    source_snapshot_sha256
                )
            })?;
            RemiCatalogResource::find_by_sha256(&conn, &source_snapshot_sha256)
                .context("resolve durable source snapshot resource")?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "source snapshot resource {} is not registered",
                        source_snapshot_sha256
                    )
                })
        })?;

        if resource.kind != RemiCatalogResourceKind::SourceSnapshot {
            bail!(
                "resource {} is {:?}, expected source snapshot",
                resource.resource_sha256,
                resource.kind
            );
        }
        if !resource.durable {
            bail!(
                "source snapshot resource {} is not durable",
                resource.resource_sha256
            );
        }
        if resource.source_profile != pinned.source_profile() {
            bail!(
                "source snapshot {} belongs to '{}' instead of '{}'",
                resource.resource_sha256,
                resource.source_profile,
                pinned.source_profile()
            );
        }

        let manifest = deserialize_source_snapshot(&resource)
            .context("deserialize durable source snapshot manifest")?;
        let manifest_digest = manifest
            .manifest_sha256()
            .context("compute durable source snapshot digest")?;
        if manifest_digest != source_snapshot_sha256
            || manifest.source_profile != pinned.source_profile()
            || manifest.source_identity != member.source_identity
            || manifest.repository_identity != member.repository_identity
        {
            bail!(
                "source snapshot {} identity disagrees with pinned profile member",
                source_snapshot_sha256
            );
        }
        if member.stream != manifest.stream {
            bail!(
                "source snapshot {} stream disagrees with pinned profile member",
                source_snapshot_sha256
            );
        }
        let artifact_size = i64::try_from(manifest.catalog.size)
            .context("source snapshot catalog size exceeds SQLite integer range")?;
        if resource.artifact_sha256 != manifest.catalog.sha256
            || resource.artifact_size != artifact_size
            || resource.logical_digest_sha256 != manifest.logical_digest_sha256
        {
            bail!(
                "source snapshot {} resource metadata disagrees with its manifest",
                source_snapshot_sha256
            );
        }

        let bundle_path = self
            .catalog_dir
            .join("sources")
            .join(&source_snapshot_sha256);
        let reader = verify_source_catalog_bundle(&bundle_path, &manifest).with_context(|| {
            format!(
                "verify durable source snapshot bundle {}",
                bundle_path.display()
            )
        })?;
        Ok(VerifiedSourceCatalog { manifest, reader })
    }
}

fn deserialize_source_snapshot(resource: &RemiCatalogResource) -> Result<SourceSnapshotV1> {
    let manifest: SourceSnapshotV1 = serde_json::from_str(&resource.manifest_json)
        .context("parse SourceSnapshotV1 manifest JSON")?;
    manifest
        .validate()
        .context("validate SourceSnapshotV1 manifest")?;
    let canonical = conary_core::json::canonical_json(&manifest)
        .map_err(anyhow::Error::msg)
        .context("canonicalize SourceSnapshotV1 manifest")?;
    if canonical != resource.manifest_json.as_bytes() {
        bail!("source snapshot manifest JSON is not canonical");
    }
    let raw_digest = conary_core::hash::sha256(resource.manifest_json.as_bytes());
    if raw_digest != resource.resource_sha256 {
        bail!(
            "source snapshot manifest digest mismatch: expected {}, got {}",
            resource.resource_sha256,
            raw_digest
        );
    }
    Ok(manifest)
}

fn source_package_matches_profile(
    profile_package: &CatalogPackageRecordV1,
    source_package: &CatalogPackageRecordV1,
) -> bool {
    profile_package.source_profile == source_package.source_profile
        && profile_package.name == source_package.name
        && profile_package.version == source_package.version
        && profile_package.package_release == source_package.package_release
        && profile_package.architecture == source_package.architecture
        && profile_package.debian_multi_arch == source_package.debian_multi_arch
        && profile_package.description == source_package.description
        && profile_package.checksum == source_package.checksum
        && profile_package.size == source_package.size
        && profile_package.download_url == source_package.download_url
        && profile_package.metadata == source_package.metadata
        && profile_package.is_security_update == source_package.is_security_update
        && profile_package.severity == source_package.severity
        && profile_package.cve_ids == source_package.cve_ids
        && profile_package.advisory_id == source_package.advisory_id
        && profile_package.advisory_url == source_package.advisory_url
        && profile_package.version_scheme == source_package.version_scheme
        && profile_package.provides == source_package.provides
        && profile_package.requirement_groups == source_package.requirement_groups
}
