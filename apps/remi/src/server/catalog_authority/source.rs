// apps/remi/src/server/catalog_authority/source.rs

//! Exact durable source-catalog authority below one pinned profile revision.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use conary_core::db::models::{RemiCatalogResource, RemiCatalogResourceKind};
use conary_core::repository::DurableSourceCatalogReuseV1;
use conary_core::repository::catalog::{
    CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogReader, ProfileSourceMemberV2,
    SourceSnapshotV1, verify_registered_source_catalog_bundle,
};
use parking_lot::Mutex;

use super::{CachedSourceReader, CatalogAuthority, PinnedProfileCatalog, ProfileRevisionSelection};
use crate::server::open_runtime_db;

struct VerifiedSourceCatalog {
    manifest: SourceSnapshotV1,
    reader: Arc<Mutex<CatalogReader>>,
    bundle_path: std::path::PathBuf,
}

pub(crate) struct VerifiedSourceBundle {
    pub(crate) manifest: SourceSnapshotV1,
    pub(crate) bundle_path: std::path::PathBuf,
}

struct RegisteredSourceCatalog {
    manifest: SourceSnapshotV1,
    bundle_path: std::path::PathBuf,
}

impl CatalogAuthority {
    /// Resolve every exact registered source below one selected profile
    /// without opening catalog bytes. The native parser later accepts a
    /// selection only after current authenticated metadata matches and then
    /// performs the one complete durable reopen.
    pub(crate) fn inspect_source_reuse_for_selection(
        &self,
        selection: &ProfileRevisionSelection,
    ) -> Result<Vec<(u32, DurableSourceCatalogReuseV1)>> {
        let inspected = self.inspect_selected_profile(selection)?;
        let mut reusable = Vec::with_capacity(inspected.manifest.members.len());
        for member in &inspected.manifest.members {
            let registered =
                self.resolve_registered_source_catalog(&selection.source_profile, member)?;
            reusable.push((
                member.ordinal,
                DurableSourceCatalogReuseV1::new(registered.manifest, registered.bundle_path)
                    .context("construct durable source reuse selection")?,
            ));
        }
        Ok(reusable)
    }

    /// Reopen one exact source member and retain its verified bundle path.
    ///
    /// Native-oracle materialization uses this path to consume retained
    /// parser-native bytes. It never reconstructs an upstream URL.
    pub(crate) fn source_bundle_for_member(
        &self,
        pinned: &PinnedProfileCatalog,
        member_ordinal: u32,
    ) -> Result<VerifiedSourceBundle> {
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
        let verified = self.verified_source_catalog(pinned, member)?;
        Ok(VerifiedSourceBundle {
            manifest: verified.manifest,
            bundle_path: verified.bundle_path,
        })
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
            .lock()
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
        let registered = self.resolve_registered_source_catalog(pinned.source_profile(), member)?;
        let source_snapshot_sha256 = member.source_snapshot_sha256.clone();
        let mut cache = self.verified_source_readers.lock();
        let reader = match cache.get(&source_snapshot_sha256) {
            Some(cached) => {
                if cached.source_profile != pinned.source_profile()
                    || cached.manifest != registered.manifest
                    || cached.bundle_path != registered.bundle_path
                {
                    bail!(
                        "cached source snapshot {} authority disagrees with its pinned registered bundle",
                        source_snapshot_sha256
                    );
                }
                Arc::clone(&cached.reader)
            }
            None => {
                let reader = Arc::new(Mutex::new(
                    verify_registered_source_catalog_bundle(
                        &registered.bundle_path,
                        &registered.manifest,
                    )
                    .with_context(|| {
                        format!(
                            "verify registered source snapshot bundle {}",
                            registered.bundle_path.display()
                        )
                    })?,
                ));
                cache.insert(
                    source_snapshot_sha256,
                    CachedSourceReader {
                        source_profile: pinned.source_profile().to_string(),
                        manifest: registered.manifest.clone(),
                        bundle_path: registered.bundle_path.clone(),
                        reader: Arc::clone(&reader),
                    },
                );
                reader
            }
        };
        Ok(VerifiedSourceCatalog {
            manifest: registered.manifest,
            reader,
            bundle_path: registered.bundle_path,
        })
    }

    fn resolve_registered_source_catalog(
        &self,
        source_profile: &str,
        member: &ProfileSourceMemberV2,
    ) -> Result<RegisteredSourceCatalog> {
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
        if resource.source_profile != source_profile {
            bail!(
                "source snapshot {} belongs to '{}' instead of '{}'",
                resource.resource_sha256,
                resource.source_profile,
                source_profile
            );
        }

        let manifest = deserialize_source_snapshot(&resource)
            .context("deserialize durable source snapshot manifest")?;
        let manifest_digest = manifest
            .manifest_sha256()
            .context("compute durable source snapshot digest")?;
        if manifest_digest != source_snapshot_sha256
            || manifest.source_profile != source_profile
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
        Ok(RegisteredSourceCatalog {
            manifest,
            bundle_path,
        })
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::server::catalog_authority::test_support::ActiveCatalogFixture;

    fn active_source(
        fixture: &ActiveCatalogFixture,
        epoch: i64,
    ) -> (PinnedProfileCatalog, ProfileSourceMemberV2) {
        fixture.activate("fedora-44", epoch, Vec::new());
        let pinned = fixture
            .authority()
            .open_active_profile("fedora-44")
            .expect("open active fixture profile");
        let member = pinned
            .manifest()
            .members
            .first()
            .expect("fixture profile source member")
            .clone();
        (pinned, member)
    }

    #[test]
    fn repeated_registered_source_open_reuses_one_verified_reader() {
        let fixture = ActiveCatalogFixture::new();
        let (pinned, member) = active_source(&fixture, 1);

        let first = fixture
            .authority()
            .verified_source_catalog(&pinned, &member)
            .expect("first registered source open");
        let second = fixture
            .authority()
            .clone()
            .verified_source_catalog(&pinned, &member)
            .expect("repeated registered source open");

        assert!(Arc::ptr_eq(&first.reader, &second.reader));
        assert_eq!(first.manifest, second.manifest);
        assert_eq!(first.bundle_path, second.bundle_path);
    }

    #[test]
    fn successor_snapshot_cannot_reuse_predecessor_reader() {
        let fixture = ActiveCatalogFixture::new();
        let (first_pin, first_member) = active_source(&fixture, 1);
        let first = fixture
            .authority()
            .verified_source_catalog(&first_pin, &first_member)
            .expect("open predecessor source");

        let (second_pin, second_member) = active_source(&fixture, 2);
        let second = fixture
            .authority()
            .verified_source_catalog(&second_pin, &second_member)
            .expect("open successor source");

        assert_ne!(
            first_member.source_snapshot_sha256,
            second_member.source_snapshot_sha256
        );
        assert!(!Arc::ptr_eq(&first.reader, &second.reader));
        assert_ne!(first.manifest, second.manifest);
    }

    #[test]
    fn cached_source_reader_fails_closed_on_authority_mismatch() {
        let fixture = ActiveCatalogFixture::new();
        let (pinned, member) = active_source(&fixture, 1);
        fixture
            .authority()
            .verified_source_catalog(&pinned, &member)
            .expect("seed exact source reader cache");
        fixture
            .authority()
            .verified_source_readers
            .lock()
            .get_mut(&member.source_snapshot_sha256)
            .expect("cached source reader")
            .source_profile = "ubuntu-26.04".to_string();

        let error = fixture
            .authority()
            .verified_source_catalog(&pinned, &member)
            .err()
            .expect("mismatched cached authority must fail");
        assert!(error.to_string().contains("cached source snapshot"));
        assert!(error.to_string().contains("authority disagrees"));
    }
}
