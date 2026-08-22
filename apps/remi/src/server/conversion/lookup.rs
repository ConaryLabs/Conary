// apps/remi/src/server/conversion/lookup.rs
//! Immutable catalog package lookup and pinned upstream download for conversion.

use super::ConversionService;
use crate::server::catalog_authority::PinnedProfileCatalog;
use crate::server::profile_catalog::{ProfileCatalog, RankedProfilePackage};
use anyhow::{Context, Result, anyhow, bail};
use conary_core::db::models::{RemiActiveProfileRevision, Repository, RepositoryPackage};
use conary_core::repository::catalog::{
    CatalogPackageOriginV1, CatalogPackageRecordV1, SourceSnapshotV1,
};
use conary_core::repository::remi_metadata::REMI_SPARSE_MIN_PACKAGE_SIZE;
use conary_core::repository::versioning::compare_repo_versions;
use conary_core::repository::{DownloadOptions, download_package_verified};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The complete immutable source identity used by one conversion.
///
/// The profile catalog reader is intentionally owned here.  Keeping this
/// object alive keeps the reader pin alive, so a concurrent profile activation
/// or catalog GC cannot change the source authority between download, parse,
/// and conversion persistence.
pub(super) struct PinnedConversionSource {
    pub(super) catalog: PinnedProfileCatalog,
    pub(super) package: CatalogPackageRecordV1,
    pub(super) source_snapshot: SourceSnapshotV1,
    pub(super) repo_pkg: RepositoryPackage,
    pub(super) repository_id: i64,
    pub(super) repository_key_name: String,
}

impl PinnedConversionSource {
    pub(super) fn source_profile(&self) -> &str {
        self.catalog.source_profile()
    }

    pub(super) fn profile_revision_sha256(&self) -> &str {
        self.catalog.profile_revision_sha256()
    }

    /// Digest the exact canonical provides projection carried by the pinned
    /// catalog record.  This is diagnostic metadata on the conversion row;
    /// it never consults the mutable operational provides projection.
    pub(super) fn catalog_provides_digest(&self) -> Result<String> {
        conary_core::ccs::attestation::canonical_json_hash(&self.package.provides)
    }
}

pub(super) struct PackageDownloadRequest<'a> {
    pub(super) source: PinnedConversionSource,
    pub(super) dest_dir: &'a Path,
}

#[derive(Debug, Error)]
pub(super) enum CatalogPackageLookupError {
    #[error(
        "catalog package '{package_name}' was not found for profile '{profile}'{version}{architecture}"
    )]
    NotFound {
        profile: String,
        package_name: String,
        version: String,
        architecture: String,
    },
    #[error(
        "catalog package '{package_name}' is ambiguous for profile '{profile}'{version}{architecture}: {candidate_count} exact candidates"
    )]
    Ambiguous {
        profile: String,
        package_name: String,
        version: String,
        architecture: String,
        candidate_count: usize,
    },
}

impl ConversionService {
    fn download_options_for_source(
        &self,
        source: &PinnedConversionSource,
    ) -> Result<(RepositoryPackage, DownloadOptions)> {
        // The operational repository row was consulted once, while resolving
        // this source, solely for its prepared key material name and numeric
        // identity.  Its trust policy is deliberately replaced with the
        // policy carried by the durable SourceSnapshotV1 manifest.
        // Copy every download input before awaiting. `PinnedProfileCatalog`
        // owns a rusqlite reader and is deliberately not `Sync`; borrowing the
        // source across this await would make the request future impossible
        // to send to the runtime worker.
        let repository_key_name = source.repository_key_name.clone();
        let metadata_url = source.source_snapshot.provenance.metadata_url.clone();
        let package_format = source.source_snapshot.provenance.parser_config.format();
        let trust_policy = source.source_snapshot.provenance.trust_policy.clone();
        let mut repository = Repository::new(repository_key_name, metadata_url);
        repository.package_format = package_format;
        repository.trust_policy = Some(trust_policy);
        let keyring = conary_core::db::paths::keyring_dir(&self.db_path.display().to_string());
        let trust = DownloadOptions::for_repository(&repository, &keyring)?;
        Ok((source.repo_pkg.clone(), trust))
    }

    async fn download_trusted_repository_package(
        &self,
        repo_pkg: RepositoryPackage,
        dest_dir: &Path,
        trust: DownloadOptions,
    ) -> Result<PathBuf> {
        download_package_verified(&repo_pkg, dest_dir, &trust)
            .await
            .map_err(anyhow::Error::from)
    }

    pub(super) async fn find_package_for_conversion_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<PinnedConversionSource> {
        let service = self.clone();
        let distro = distro.to_string();
        let package_name = package_name.to_string();
        let version = version.map(ToString::to_string);
        let architecture = architecture.map(ToString::to_string);

        tokio::task::spawn_blocking(move || {
            service.find_catalog_package(
                &distro,
                &package_name,
                version.as_deref(),
                architecture.as_deref(),
                None,
            )
        })
        .await
        .map_err(|e| anyhow!("package lookup task panicked: {e}"))?
    }

    pub(super) async fn find_package_for_selected_revision_async(
        &self,
        selection: RemiActiveProfileRevision,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<PinnedConversionSource> {
        let service = self.clone();
        let package_name = package_name.to_string();
        let version = version.map(ToString::to_string);
        let architecture = architecture.map(ToString::to_string);

        tokio::task::spawn_blocking(move || {
            service.find_catalog_package(
                &selection.source_profile.clone(),
                &package_name,
                version.as_deref(),
                architecture.as_deref(),
                Some(&selection),
            )
        })
        .await
        .map_err(|e| anyhow!("selected package lookup task panicked: {e}"))?
    }

    fn find_catalog_package(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
        selection: Option<&RemiActiveProfileRevision>,
    ) -> Result<PinnedConversionSource> {
        let profile = conary_core::repository::supported_profiles::profile_by_public_id(distro)
            .ok_or_else(|| anyhow!("unsupported public profile: {}", distro))?;
        let catalog_authority = self.catalog_authority.as_ref().ok_or_else(|| {
            anyhow!("repository conversion requires an immutable profile catalog authority")
        })?;
        let catalog = match selection {
            Some(selection) => {
                if selection.source_profile != profile.id() {
                    bail!(
                        "selected catalog profile '{}' does not match conversion profile '{}'",
                        selection.source_profile,
                        profile.id()
                    );
                }
                catalog_authority
                    .open_selected_profile(selection)
                    .with_context(|| {
                        format!(
                            "open selected catalog for profile '{}' revision {}",
                            profile.id(),
                            selection.profile_revision_sha256
                        )
                    })?
            }
            None => catalog_authority
                .open_active_profile(profile.id())
                .with_context(|| format!("open pinned catalog for profile '{}'", profile.id()))?,
        };
        let candidates =
            ProfileCatalog::new(&catalog).ranked_package_records_by_name(package_name)?;
        let package = select_catalog_package(
            profile.id(),
            package_name,
            version,
            architecture,
            candidates,
        )?;
        let source_snapshot = catalog_authority.source_snapshot_for_package(&catalog, &package)?;
        let (repository_id, repository_key_name) =
            lookup_repository_key_material(&self.db_path, profile.id(), &package.origin)?;
        let repo_pkg = repository_package_from_catalog(&package, repository_id)?;

        Ok(PinnedConversionSource {
            catalog,
            package,
            source_snapshot,
            repo_pkg,
            repository_id,
            repository_key_name,
        })
    }

    pub(super) async fn download_package_async(
        &self,
        request: PackageDownloadRequest<'_>,
    ) -> Result<(PinnedConversionSource, PathBuf)> {
        let PackageDownloadRequest { source, dest_dir } = request;
        let (repo_pkg, trust) = self.download_options_for_source(&source)?;
        let path = self
            .download_trusted_repository_package(repo_pkg, dest_dir, trust)
            .await?;
        Ok((source, path))
    }
}

fn select_catalog_package(
    profile: &str,
    package_name: &str,
    version: Option<&str>,
    architecture: Option<&str>,
    candidates: Vec<RankedProfilePackage>,
) -> Result<CatalogPackageRecordV1> {
    let version_label = version
        .map(|value| format!(" version '{value}'"))
        .unwrap_or_default();
    let architecture_label = architecture
        .map(|value| format!(" architecture '{value}'"))
        .unwrap_or_default();
    let not_found = || {
        anyhow::Error::new(CatalogPackageLookupError::NotFound {
            profile: profile.to_string(),
            package_name: package_name.to_string(),
            version: version_label.clone(),
            architecture: architecture_label.clone(),
        })
    };
    let minimum_size = u64::try_from(REMI_SPARSE_MIN_PACKAGE_SIZE)
        .context("Remi minimum downloadable package size is negative")?;

    let mut candidates = candidates
        .into_iter()
        .filter(|candidate| {
            candidate.package.name == package_name
                && candidate.package.source_profile == profile
                && candidate.package.size >= minimum_size
                && version.is_none_or(|requested| candidate.package.version == requested)
                && architecture.is_none_or(|requested| {
                    candidate.package.architecture.as_deref() == Some(requested)
                })
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(not_found());
    }

    candidates = ProfileCatalog::retain_highest_priority(candidates);

    if version.is_none() {
        let scheme = candidates[0].package.version_scheme;
        let mut latest_version = candidates[0].package.version.clone();
        for candidate in candidates.iter().skip(1) {
            match compare_repo_versions(scheme, &candidate.package.version, &latest_version)? {
                Ordering::Greater => latest_version = candidate.package.version.clone(),
                Ordering::Equal | Ordering::Less => {}
            }
        }
        candidates.retain(|candidate| candidate.package.version == latest_version);
    }

    // A request without an architecture is only unambiguous when the selected
    // version has one exact catalog record.  Never let SQLite insertion order
    // or a LIMIT clause choose a native artifact for us.
    if candidates.len() != 1 {
        return Err(anyhow::Error::new(CatalogPackageLookupError::Ambiguous {
            profile: profile.to_string(),
            package_name: package_name.to_string(),
            version: version_label,
            architecture: architecture_label,
            candidate_count: candidates.len(),
        }));
    }
    Ok(candidates
        .pop()
        .expect("candidate count checked as one")
        .package)
}

fn lookup_repository_key_material(
    db_path: &Path,
    profile: &str,
    origin: &CatalogPackageOriginV1,
) -> Result<(i64, String)> {
    let CatalogPackageOriginV1::Profile {
        source_identity: _,
        repository_identity,
        ..
    } = origin
    else {
        bail!("conversion source package must carry a profile origin");
    };
    let conn = conary_core::db::open_fast(db_path)?;
    let mut statement = conn.prepare(
        "SELECT id, name FROM repositories
         WHERE source_profile = ?1 AND repository_identity = ?2
         ORDER BY id",
    )?;
    let ids = statement
        .query_map(rusqlite::params![profile, repository_identity], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    match ids.as_slice() {
        [] => bail!(
            "no operational repository carries source profile '{}' and repository identity '{}' for prepared key material",
            profile,
            repository_identity
        ),
        [(repository_id, repository_name)] => Ok((*repository_id, repository_name.clone())),
        ids => bail!(
            "operational repository identity '{}' for profile '{}' is ambiguous ({} rows)",
            repository_identity,
            profile,
            ids.len()
        ),
    }
}

fn repository_package_from_catalog(
    package: &CatalogPackageRecordV1,
    repository_id: i64,
) -> Result<RepositoryPackage> {
    let size = i64::try_from(package.size)
        .context("catalog package size exceeds RepositoryPackage integer range")?;
    Ok(RepositoryPackage {
        id: None,
        repository_id,
        name: package.name.clone(),
        version: package.version.clone(),
        package_release: package.package_release.clone(),
        architecture: package.architecture.clone(),
        debian_multi_arch: package.debian_multi_arch,
        description: package.description.clone(),
        checksum: package.checksum.clone(),
        size,
        download_url: package.download_url.clone(),
        metadata: package.metadata.clone(),
        synced_at: None,
        is_security_update: package.is_security_update,
        severity: package.severity.clone(),
        cve_ids: package.cve_ids.clone(),
        advisory_id: package.advisory_id.clone(),
        advisory_url: package.advisory_url.clone(),
        source_profile: Some(package.source_profile.clone()),
        version_scheme: package.version_scheme,
        canonical_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::catalog_authority::test_support::ActiveCatalogFixture;

    fn package(version: &str, architecture: Option<&str>, marker: &str) -> CatalogPackageRecordV1 {
        let mut package = crate::server::catalog_authority::test_support::package(
            "fedora-44",
            "demo",
            version,
            "",
            architecture,
            42,
            marker,
        );
        package.package_key_sha256 = String::new();
        package
    }

    fn ranked(package: CatalogPackageRecordV1, member_priority: i32) -> RankedProfilePackage {
        RankedProfilePackage {
            package,
            member_priority,
        }
    }

    #[test]
    fn catalog_lookup_picks_latest_unique_version() {
        let selected = select_catalog_package(
            "fedora-44",
            "demo",
            None,
            Some("x86_64"),
            vec![
                ranked(package("1.0-1.fc44", Some("x86_64"), "old"), 0),
                ranked(package("1.1-1.fc44", Some("x86_64"), "new"), 0),
            ],
        )
        .unwrap();
        assert_eq!(selected.version, "1.1-1.fc44");
        assert_eq!(selected.checksum, conary_core::hash::sha256(b"new"));
    }

    #[test]
    fn catalog_lookup_rejects_exact_ambiguity() {
        let error = select_catalog_package(
            "fedora-44",
            "demo",
            Some("1.1-1.fc44"),
            None,
            vec![
                ranked(package("1.1-1.fc44", Some("x86_64"), "one"), 0),
                ranked(package("1.1-1.fc44", Some("aarch64"), "two"), 0),
            ],
        )
        .unwrap_err();
        assert!(
            error
                .downcast_ref::<CatalogPackageLookupError>()
                .is_some_and(|error| matches!(error, CatalogPackageLookupError::Ambiguous { .. }))
        );
    }

    #[test]
    fn catalog_lookup_reports_missing_without_operational_fallback() {
        let error =
            select_catalog_package("fedora-44", "missing", None, None, Vec::new()).unwrap_err();
        assert!(
            error
                .downcast_ref::<CatalogPackageLookupError>()
                .is_some_and(|error| matches!(error, CatalogPackageLookupError::NotFound { .. }))
        );
    }

    #[test]
    fn catalog_lookup_applies_priority_before_native_version_ordering() {
        let selected = select_catalog_package(
            "fedora-44",
            "demo",
            None,
            Some("x86_64"),
            vec![
                ranked(package("2.0-1.fc44", Some("x86_64"), "lower"), 10),
                ranked(package("1.0-1.fc44", Some("x86_64"), "higher"), 20),
            ],
        )
        .expect("higher-priority eligible member wins");

        assert_eq!(selected.version, "1.0-1.fc44");
        assert_eq!(selected.checksum, conary_core::hash::sha256(b"higher"));
    }

    #[test]
    fn catalog_lookup_filters_exact_version_before_priority() {
        let selected = select_catalog_package(
            "fedora-44",
            "demo",
            Some("1.0-1.fc44"),
            Some("x86_64"),
            vec![
                ranked(package("2.0-1.fc44", Some("x86_64"), "higher"), 20),
                ranked(package("1.0-1.fc44", Some("x86_64"), "eligible"), 10),
            ],
        )
        .expect("priority applies only within the eligible exact version");

        assert_eq!(selected.version, "1.0-1.fc44");
        assert_eq!(selected.checksum, conary_core::hash::sha256(b"eligible"));
    }

    #[test]
    fn catalog_lookup_filters_downloadability_before_priority() {
        let mut higher_placeholder = package("2.0-1.fc44", Some("x86_64"), "placeholder");
        higher_placeholder.size = 0;
        let selected = select_catalog_package(
            "fedora-44",
            "demo",
            None,
            Some("x86_64"),
            vec![
                ranked(higher_placeholder, 20),
                ranked(package("1.0-1.fc44", Some("x86_64"), "downloadable"), 10),
            ],
        )
        .expect("downloadable lower-priority member remains eligible");

        assert_eq!(selected.version, "1.0-1.fc44");
        assert_eq!(
            selected.checksum,
            conary_core::hash::sha256(b"downloadable")
        );
    }

    #[tokio::test]
    async fn conversion_lookup_uses_only_the_pinned_profile_catalog() {
        let fixture = ActiveCatalogFixture::new();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![package("1.0-1.fc44", Some("x86_64"), "catalog-source")],
        );
        let service = ConversionService::new(
            fixture.db_path().with_extension("chunks"),
            fixture.db_path().with_extension("cache"),
            fixture.db_path().to_path_buf(),
            None,
        )
        .with_catalog_authority(fixture.authority().clone());

        let source = service
            .find_package_for_conversion_async(
                "fedora-44",
                "demo",
                Some("1.0-1.fc44"),
                Some("x86_64"),
            )
            .await
            .expect("resolve exact catalog conversion source");

        assert_eq!(source.profile_revision_sha256(), revision);
        assert_eq!(
            source.package.checksum,
            conary_core::hash::sha256(b"catalog-source")
        );
        assert_eq!(source.repo_pkg.id, None);
        assert_eq!(source.source_snapshot.source_profile, "fedora-44");
        assert_eq!(
            source.source_snapshot.repository_identity,
            "repository-fedora-44"
        );
        let conn = fixture.connection();
        let operational_packages = conn
            .query_row("SELECT COUNT(*) FROM repository_packages", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count operational package rows");
        assert_eq!(operational_packages, 0);
    }

    #[tokio::test]
    async fn conversion_lookup_reopens_the_exact_selection_after_activation() {
        let fixture = ActiveCatalogFixture::new();
        let old_revision = fixture.activate(
            "fedora-44",
            1,
            vec![package("1.0-1.fc44", Some("x86_64"), "old-source")],
        );
        let selection_pin = fixture
            .authority()
            .open_active_profile("fedora-44")
            .expect("pin old active selection");
        let selection = selection_pin.activation().clone();
        let new_revision = fixture.activate(
            "fedora-44",
            2,
            vec![package("1.0-1.fc44", Some("x86_64"), "new-source")],
        );
        let service = ConversionService::new(
            fixture.db_path().with_extension("chunks"),
            fixture.db_path().with_extension("cache"),
            fixture.db_path().to_path_buf(),
            None,
        )
        .with_catalog_authority(fixture.authority().clone());

        let source = service
            .find_package_for_selected_revision_async(
                selection,
                "demo",
                Some("1.0-1.fc44"),
                Some("x86_64"),
            )
            .await
            .expect("reopen exact old catalog selection");

        assert_eq!(source.profile_revision_sha256(), old_revision);
        assert_ne!(source.profile_revision_sha256(), new_revision);
        assert_eq!(
            source.package.checksum,
            conary_core::hash::sha256(b"old-source")
        );
        drop(selection_pin);
    }
}
