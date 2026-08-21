// conary-core/src/repository/sync/remi/sparse.rs

//! Bounded-page Remi sparse-index synchronization.

use crate::db::models::Repository;
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::remi_metadata::{
    REMI_SPARSE_SYNC_PAGE_SIZE, RemiSparsePackagePage, RemiSparseResolutionEntry,
    RemiSparseRevision, validate_remi_public_name,
};
use crate::repository::retry::{RetryConfig, with_retry_async};
use std::collections::HashSet;

use super::super::types::SyncedPackageRow;
use super::remi_sync_row;

pub(super) struct RemiSparseSync {
    client: RepositoryClient,
    retry_policy: RetryConfig,
    endpoint: String,
    configured_target: String,
    route_slug: String,
    repo_id: i64,
    pub(super) next_page: usize,
    expected_total_names: Option<usize>,
    expected_revision: Option<RemiSparseRevision>,
    pub(super) names_seen: usize,
    last_name: Option<String>,
}

impl RemiSparseSync {
    pub(super) fn new(repo: &Repository) -> Result<Self> {
        let configured_target = repo.source_profile.as_deref().ok_or_else(|| {
            Error::ConfigError(format!(
                "Repository '{}' has strategy 'remi' but no source profile configured (use --source-profile)",
                repo.name
            ))
        })?;
        let profile =
            crate::repository::supported_profiles::profile_for_remi_target(configured_target)
                .ok_or_else(|| {
                    Error::ConfigError(format!("unsupported Remi target: {configured_target}"))
                })?;
        let endpoint = repo
            .default_strategy_endpoint
            .as_deref()
            .unwrap_or(&repo.url)
            .trim_end_matches('/')
            .to_string();
        let repo_id = repo
            .id
            .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

        Ok(Self {
            client: RepositoryClient::new()?,
            retry_policy: RetryConfig::quick(),
            endpoint,
            configured_target: configured_target.to_string(),
            route_slug: profile.remi_route_slug().to_string(),
            repo_id,
            next_page: 1,
            expected_total_names: None,
            expected_revision: None,
            names_seen: 0,
            last_name: None,
        })
    }

    pub(super) async fn next_rows(&mut self) -> Result<Option<Vec<SyncedPackageRow>>> {
        if self
            .expected_total_names
            .is_some_and(|total| self.names_seen == total)
        {
            return Ok(None);
        }

        let page_url = format!(
            "{}/v1/index/{}?page={}&per_page={}&include=versions",
            self.endpoint, self.route_slug, self.next_page, REMI_SPARSE_SYNC_PAGE_SIZE
        );
        let page =
            fetch_sparse_page_with_retry(&self.client, &page_url, &self.retry_policy).await?;
        self.consume_page(page).map(Some)
    }

    pub(super) fn consume_page(
        &mut self,
        page: RemiSparsePackagePage,
    ) -> Result<Vec<SyncedPackageRow>> {
        self.validate_page(&page)?;

        let page_total = page.total;
        let page_name_count = page.packages.len();
        let page_last_name = page.packages.last().map(|entry| entry.name.clone());
        let next_names_seen = self
            .names_seen
            .checked_add(page_name_count)
            .ok_or_else(|| Error::ParseError("Remi sparse package count overflow".to_string()))?;
        let next_page = self
            .next_page
            .checked_add(1)
            .ok_or_else(|| Error::ParseError("Remi sparse page counter overflow".to_string()))?;
        let mut rows = Vec::new();
        for entry in page.packages {
            let requested_name = entry.name.clone();
            let mut seen_versions = HashSet::new();
            for version in entry.versions {
                let key = (
                    version.version.clone(),
                    version.release.clone(),
                    version.architecture.clone(),
                );
                if !seen_versions.insert(key) {
                    return Err(Error::ParseError(format!(
                        "Remi sparse entry '{}' repeats an exact version/release/architecture identity",
                        requested_name
                    )));
                }
                rows.push(remi_sync_row(
                    self.repo_id,
                    self.endpoint.clone(),
                    self.configured_target.clone(),
                    requested_name.clone(),
                    version,
                )?);
            }
        }

        self.names_seen = next_names_seen;
        self.expected_total_names.get_or_insert(page_total);
        self.expected_revision.get_or_insert(page.revision);
        self.last_name = page_last_name.or(self.last_name.take());
        self.next_page = next_page;
        Ok(rows)
    }

    pub(super) fn revision(&self) -> Option<RemiSparseRevision> {
        self.expected_revision
    }

    fn validate_page(&self, page: &RemiSparsePackagePage) -> Result<()> {
        if page.distro != self.route_slug {
            return Err(Error::ParseError(format!(
                "Remi sparse page distro '{}' disagrees with requested route '{}'",
                page.distro, self.route_slug
            )));
        }
        if page.source_profile != self.configured_target {
            return Err(Error::ParseError(format!(
                "Remi sparse page source profile '{}' disagrees with configured target '{}'",
                page.source_profile, self.configured_target
            )));
        }
        if page.page != self.next_page || page.per_page != REMI_SPARSE_SYNC_PAGE_SIZE {
            return Err(Error::ParseError(format!(
                "Remi sparse page pagination disagrees with request: page={}, per_page={}",
                page.page, page.per_page
            )));
        }
        if page.packages.len() > REMI_SPARSE_SYNC_PAGE_SIZE {
            return Err(Error::ParseError(format!(
                "Remi sparse page returned {} entries for a {}-name page",
                page.packages.len(),
                REMI_SPARSE_SYNC_PAGE_SIZE
            )));
        }
        match self.expected_total_names {
            Some(total) if total != page.total => {
                return Err(Error::ParseError(format!(
                    "Remi sparse page changed total package-name count during sync: {total} -> {}",
                    page.total
                )));
            }
            _ => {}
        }
        match self.expected_revision {
            Some(revision) if revision != page.revision => {
                return Err(Error::ParseError(format!(
                    "Remi sparse page changed server revision during sync: {} -> {}",
                    revision.sequence, page.revision.sequence
                )));
            }
            _ => {}
        }
        if page.packages.is_empty() && self.names_seen < page.total {
            return Err(Error::ParseError(format!(
                "Remi sparse page ended after {} of {} package names",
                self.names_seen, page.total
            )));
        }
        if self
            .names_seen
            .checked_add(page.packages.len())
            .is_none_or(|count| count > page.total)
        {
            return Err(Error::ParseError(
                "Remi sparse page returned more package names than its declared total".to_string(),
            ));
        }

        let mut previous = self.last_name.as_deref();
        for entry in &page.packages {
            validate_sparse_entry(&self.route_slug, previous, entry)?;
            previous = Some(entry.name.as_str());
        }
        Ok(())
    }
}

fn validate_sparse_entry(
    route_slug: &str,
    previous_name: Option<&str>,
    entry: &RemiSparseResolutionEntry,
) -> Result<()> {
    validate_sparse_name(&entry.name)?;
    if previous_name.is_some_and(|prior| prior >= entry.name.as_str()) {
        return Err(Error::ParseError(format!(
            "Remi sparse package names are not globally unique and ordered at '{}'",
            entry.name
        )));
    }
    if entry.distro != route_slug {
        return Err(Error::ParseError(format!(
            "Remi sparse entry distro '{}' disagrees with requested route '{route_slug}'",
            entry.distro
        )));
    }
    if entry.versions.is_empty() {
        return Err(Error::ParseError(format!(
            "Remi sparse entry '{}' has no versions",
            entry.name
        )));
    }
    Ok(())
}

fn validate_sparse_name(name: &str) -> Result<()> {
    validate_remi_public_name(name).map_err(|reason| {
        Error::ParseError(format!(
            "Remi sparse page contains invalid package name {name:?}: {reason}"
        ))
    })
}

pub(super) async fn fetch_sparse_page_with_retry(
    client: &RepositoryClient,
    url: &str,
    retry_policy: &RetryConfig,
) -> Result<RemiSparsePackagePage> {
    with_retry_async(retry_policy, || async {
        let response = fetch_sparse_response(client, url).await?;
        response.json().await.map_err(|error| {
            Error::ParseError(format!(
                "Failed to parse Remi sparse package page from {url}: {error}"
            ))
        })
    })
    .await
}

async fn fetch_sparse_response(client: &RepositoryClient, url: &str) -> Result<reqwest::Response> {
    crate::repository::client::validate_url_scheme(url)?;
    let response = client
        .inner()
        .get(url)
        .timeout(crate::repository::client::TimeoutConfig::default().download)
        .send()
        .await
        .map_err(|error| Error::DownloadError(format!("{error}: {url}")))?;
    if !response.status().is_success() {
        return Err(Error::HttpStatus {
            status: response.status().as_u16(),
            url: url.to_string(),
        });
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::dependency_model::{
        ProvideArchitectureQualifier, ProvideVersionRelation,
    };
    use crate::repository::remi_metadata::{RemiProvide, RemiSparseResolutionVersionEntry};
    use crate::repository::versioning::VersionScheme;

    fn test_sync() -> RemiSparseSync {
        let mut repo =
            Repository::new("fedora-sparse".to_string(), "https://remi.test".to_string());
        repo.id = Some(7);
        repo.default_strategy = Some("remi".to_string());
        repo.default_strategy_endpoint = Some("https://remi.test".to_string());
        repo.source_profile = Some("fedora-44".to_string());
        RemiSparseSync::new(&repo).unwrap()
    }

    fn test_version(name: &str) -> RemiSparseResolutionVersionEntry {
        RemiSparseResolutionVersionEntry {
            version: "1.0-1.fc44".to_string(),
            release: None,
            provides: vec![RemiProvide {
                capability: name.to_string(),
                version: Some("1.0-1.fc44".to_string()),
                version_relation: Some(ProvideVersionRelation::Equal),
                kind: "package".to_string(),
                raw: Some(name.to_string()),
                version_scheme: VersionScheme::Rpm,
                architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            }],
            requirement_groups: Vec::new(),
            architecture: Some("x86_64".to_string()),
            size: 1024,
            metadata: None,
        }
    }

    fn test_entry(name: &str) -> RemiSparseResolutionEntry {
        RemiSparseResolutionEntry {
            name: name.to_string(),
            distro: "fedora".to_string(),
            versions: vec![test_version(name)],
        }
    }

    fn test_page(
        page: usize,
        total: usize,
        packages: Vec<RemiSparseResolutionEntry>,
    ) -> RemiSparsePackagePage {
        RemiSparsePackagePage {
            distro: "fedora".to_string(),
            source_profile: "fedora-44".to_string(),
            revision: RemiSparseRevision { sequence: 7 },
            packages,
            total,
            page,
            per_page: REMI_SPARSE_SYNC_PAGE_SIZE,
        }
    }

    #[test]
    fn page_validation_rejects_a_changed_total_and_cross_page_reordering() {
        let mut changed_total = test_sync();
        changed_total
            .consume_page(test_page(1, 2, vec![test_entry("alpha")]))
            .unwrap();
        let error = changed_total
            .consume_page(test_page(2, 3, vec![test_entry("bravo")]))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("changed total package-name count")
        );

        let mut reordered = test_sync();
        reordered
            .consume_page(test_page(1, 2, vec![test_entry("bravo")]))
            .unwrap();
        let error = reordered
            .consume_page(test_page(2, 2, vec![test_entry("alpha")]))
            .unwrap_err();
        assert!(error.to_string().contains("globally unique and ordered"));
    }

    #[test]
    fn page_validation_rejects_a_changed_server_revision_with_the_same_total() {
        let mut sync = test_sync();
        sync.consume_page(test_page(1, 2, vec![test_entry("alpha")]))
            .unwrap();
        let mut changed = test_page(2, 2, vec![test_entry("bravo")]);
        changed.revision.sequence += 1;

        let error = sync.consume_page(changed).unwrap_err();
        assert!(error.to_string().contains("changed server revision"));
        assert_eq!(sync.names_seen, 1);
        assert_eq!(sync.revision(), Some(RemiSparseRevision { sequence: 7 }));
    }

    #[test]
    fn page_validation_rejects_a_different_exact_source_profile() {
        let mut page = test_page(1, 1, vec![test_entry("alpha")]);
        page.source_profile = "fedora-45".to_string();

        let error = test_sync().consume_page(page).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("disagrees with configured target 'fedora-44'")
        );
    }

    #[test]
    fn page_validation_rejects_unfetchable_and_duplicate_exact_versions() {
        let mut empty_versions = test_entry("alpha");
        empty_versions.versions.clear();
        let error = test_sync()
            .consume_page(test_page(1, 1, vec![empty_versions]))
            .unwrap_err();
        assert!(error.to_string().contains("has no versions"));

        let mut duplicate = test_entry("alpha");
        duplicate.versions.push(duplicate.versions[0].clone());
        let error = test_sync()
            .consume_page(test_page(1, 1, vec![duplicate]))
            .unwrap_err();
        assert!(error.to_string().contains("repeats an exact version"));
    }

    #[test]
    fn page_validation_rejects_invalid_names_and_incomplete_pagination() {
        let error = test_sync()
            .consume_page(test_page(1, 1, vec![test_entry("../escape")]))
            .unwrap_err();
        assert!(error.to_string().contains("invalid package name"));

        let error = test_sync()
            .consume_page(test_page(1, 2, Vec::new()))
            .unwrap_err();
        assert!(error.to_string().contains("ended after 0 of 2"));
    }

    #[test]
    fn page_validation_rejects_page_counter_overflow_without_advancing_state() {
        let mut sync = test_sync();
        sync.next_page = usize::MAX;
        let error = sync
            .consume_page(test_page(usize::MAX, 1, vec![test_entry("alpha")]))
            .unwrap_err();

        assert!(error.to_string().contains("page counter overflow"));
        assert_eq!(sync.next_page, usize::MAX);
        assert_eq!(sync.names_seen, 0);
        assert_eq!(sync.expected_total_names, None);
        assert_eq!(sync.last_name, None);
    }
}
