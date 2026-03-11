// conary-core/src/canonical/repology.rs

//! Repology API client for bootstrapping the canonical package registry.
//!
//! Repology tracks packaging across hundreds of repositories and distributions.
//! This module fetches project data from the Repology API and maps it into
//! Conary's canonical package model.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::db::models::{CanonicalPackage, PackageImplementation};
use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single package entry from the Repology API response.
///
/// Repology returns an array of these for each project, one per repository.
#[derive(Debug, Clone, Deserialize)]
pub struct RepologyPackage {
    pub repo: String,
    pub visiblename: String,
    pub version: String,
    pub status: String,
}

/// A Repology project with all its cross-distro implementations.
#[derive(Debug, Clone)]
pub struct RepologyProject {
    /// The project name (Repology key).
    pub name: String,
    /// Per-repo implementations extracted from the API response.
    pub implementations: Vec<RepologyImplementation>,
}

/// A single distro implementation within a Repology project.
///
/// Mirrors `RepologyPackage` but is not `Deserialize` — constructed from parsed
/// API responses rather than directly from JSON.
#[derive(Debug, Clone)]
pub struct RepologyImplementation {
    pub repo: String,
    pub visiblename: String,
    pub version: String,
    pub status: String,
}

impl From<RepologyPackage> for RepologyImplementation {
    fn from(p: RepologyPackage) -> Self {
        Self {
            repo: p.repo,
            visiblename: p.visiblename,
            version: p.version,
            status: p.status,
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing functions (pure, no network)
// ---------------------------------------------------------------------------

/// Parse a Repology `/api/v1/project/{name}` response (JSON array) into a
/// `RepologyProject`.
pub fn parse_project_response(name: &str, json: &str) -> Result<RepologyProject> {
    let packages: Vec<RepologyPackage> =
        serde_json::from_str(json).map_err(|e| Error::ParseError(e.to_string()))?;

    let implementations = packages
        .into_iter()
        .map(RepologyImplementation::from)
        .collect();

    Ok(RepologyProject {
        name: name.to_string(),
        implementations,
    })
}

/// Parse a Repology `/api/v1/projects/{start}/` response (JSON object mapping
/// project names to arrays of packages) into a `Vec<RepologyProject>`.
pub fn parse_projects_batch(json: &str) -> Result<Vec<RepologyProject>> {
    let map: BTreeMap<String, Vec<RepologyPackage>> =
        serde_json::from_str(json).map_err(|e| Error::ParseError(e.to_string()))?;

    let projects = map
        .into_iter()
        .map(|(name, packages)| {
            let implementations = packages
                .into_iter()
                .map(RepologyImplementation::from)
                .collect();
            RepologyProject {
                name,
                implementations,
            }
        })
        .collect();

    Ok(projects)
}

/// Map a Conary distro identifier to a Repology-style repository ID.
///
/// This is the inverse of `repo_to_distro`. Returns `None` for unrecognised distros.
pub fn distro_to_repo(distro: &str) -> Option<String> {
    match distro {
        "arch" => return Some("arch".to_string()),
        "ubuntu-noble" => return Some("ubuntu_24_04".to_string()),
        "ubuntu-jammy" => return Some("ubuntu_22_04".to_string()),
        "opensuse-tumbleweed" => return Some("opensuse_tumbleweed".to_string()),
        _ => {}
    }

    // Pattern: fedora-NN -> fedora_NN
    if let Some(version) = distro.strip_prefix("fedora-")
        && version.chars().all(|c| c.is_ascii_digit())
    {
        return Some(format!("fedora_{version}"));
    }

    // Pattern: opensuse-* -> opensuse_*
    if let Some(suffix) = distro.strip_prefix("opensuse-") {
        return Some(format!("opensuse_{suffix}"));
    }

    None
}

/// Map a Repology repository ID to a Conary distro identifier.
///
/// Returns `None` for repositories we do not recognise.
pub fn repo_to_distro(repo: &str) -> Option<String> {
    // Exact matches first
    match repo {
        "arch" => return Some("arch".to_string()),
        "ubuntu_24_04" => return Some("ubuntu-noble".to_string()),
        "ubuntu_22_04" => return Some("ubuntu-jammy".to_string()),
        "opensuse_tumbleweed" => return Some("opensuse-tumbleweed".to_string()),
        _ => {}
    }

    // Pattern: fedora_NN -> fedora-NN
    if let Some(version) = repo.strip_prefix("fedora_")
        && version.chars().all(|c| c.is_ascii_digit())
    {
        return Some(format!("fedora-{version}"));
    }

    // Pattern: opensuse_* -> opensuse-*
    if let Some(suffix) = repo.strip_prefix("opensuse_") {
        return Some(format!("opensuse-{suffix}"));
    }

    None
}

// ---------------------------------------------------------------------------
// Async client
// ---------------------------------------------------------------------------

/// Async client for the Repology REST API.
///
/// Note: Repology enforces strict rate limits (~1 request/second). Callers
/// should throttle requests when fetching in bulk.
pub struct RepologyClient {
    client: reqwest::Client,
    base_url: String,
}

const USER_AGENT: &str = "conary/0.1 (https://conary.io)";

fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| Error::DownloadError(format!("failed to build HTTP client: {e}")))
}

impl RepologyClient {
    /// Create a new client pointing at the public Repology API.
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: build_client()?,
            base_url: "https://repology.org".to_string(),
        })
    }

    /// Create a client with a custom base URL (useful for testing against a
    /// local mock server).
    pub fn with_base_url(url: &str) -> Result<Self> {
        Ok(Self {
            client: build_client()?,
            base_url: url.trim_end_matches('/').to_string(),
        })
    }

    /// Fetch the response body from a URL, checking for HTTP errors.
    async fn get_text(&self, url: &str) -> Result<String> {
        self.client
            .get(url)
            .send()
            .await
            .map_err(|e| Error::DownloadError(e.to_string()))?
            .error_for_status()
            .map_err(|e| Error::DownloadError(e.to_string()))?
            .text()
            .await
            .map_err(|e| Error::DownloadError(e.to_string()))
    }

    /// Fetch a single project by name.
    pub async fn fetch_project(&self, name: &str) -> Result<RepologyProject> {
        let encoded = urlencoding::encode(name);
        let url = format!("{}/api/v1/project/{encoded}", self.base_url);
        let body = self.get_text(&url).await?;
        parse_project_response(name, &body)
    }

    /// Fetch a batch of projects starting at the given name (alphabetical).
    pub async fn fetch_projects_batch(&self, start: &str) -> Result<Vec<RepologyProject>> {
        let encoded = urlencoding::encode(start);
        let url = format!("{}/api/v1/projects/{encoded}/", self.base_url);
        let body = self.get_text(&url).await?;
        parse_projects_batch(&body)
    }

    /// Fetch a batch of projects and sync recognised implementations into the
    /// database. Returns the number of projects synced.
    pub async fn sync_to_db(&self, conn: &rusqlite::Connection, start: &str) -> Result<usize> {
        let projects = self.fetch_projects_batch(start).await?;
        let mut count = 0;

        // Wrap all inserts in a single transaction for atomicity and performance
        let tx = conn.unchecked_transaction()?;

        for project in &projects {
            // Filter to implementations we can map to a known distro
            let known: Vec<_> = project
                .implementations
                .iter()
                .filter_map(|imp| {
                    repo_to_distro(&imp.repo).map(|distro| (distro, imp.visiblename.clone()))
                })
                .collect();

            if known.is_empty() {
                continue;
            }

            // Upsert the canonical package — if it already exists, look up its ID
            let mut canonical = CanonicalPackage::new(project.name.clone(), "package".to_string());
            let can_id = match canonical.insert_or_ignore(&tx)? {
                Some(id) => id,
                None => {
                    // Already exists — look up by name
                    match CanonicalPackage::find_by_name(&tx, &project.name)? {
                        Some(existing) => match existing.id {
                            Some(id) => id,
                            None => continue,
                        },
                        None => continue,
                    }
                }
            };

            // Upsert each distro implementation
            for (distro, distro_name) in known {
                let mut imp =
                    PackageImplementation::new(can_id, distro, distro_name, "repology".to_string());
                imp.insert_or_ignore(&tx)?;
            }

            count += 1;
        }

        tx.commit()?;

        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_repology_project_response() {
        let json = r#"[
            {"repo": "fedora_41", "visiblename": "curl", "version": "8.9.1", "status": "newest"},
            {"repo": "ubuntu_24_04", "visiblename": "curl", "version": "8.5.0", "status": "outdated"},
            {"repo": "arch", "visiblename": "curl", "version": "8.9.1", "status": "newest"}
        ]"#;
        let project = parse_project_response("curl", json).unwrap();
        assert_eq!(project.name, "curl");
        assert_eq!(project.implementations.len(), 3);
        assert_eq!(project.implementations[0].repo, "fedora_41");
    }

    #[test]
    fn test_parse_repology_projects_batch() {
        let json = r#"{
            "curl": [
                {"repo": "fedora_41", "visiblename": "curl", "version": "8.9.1", "status": "newest"}
            ],
            "wget": [
                {"repo": "fedora_41", "visiblename": "wget", "version": "1.24.5", "status": "newest"},
                {"repo": "ubuntu_24_04", "visiblename": "wget", "version": "1.21.4", "status": "outdated"}
            ]
        }"#;
        let projects = parse_projects_batch(json).unwrap();
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn test_repo_id_to_distro() {
        assert_eq!(repo_to_distro("fedora_41"), Some("fedora-41".to_string()));
        assert_eq!(
            repo_to_distro("ubuntu_24_04"),
            Some("ubuntu-noble".to_string())
        );
        assert_eq!(repo_to_distro("arch"), Some("arch".to_string()));
        assert_eq!(repo_to_distro("unknown_repo_xyz"), None);
    }

    #[test]
    fn test_repo_to_distro_opensuse() {
        assert_eq!(
            repo_to_distro("opensuse_tumbleweed"),
            Some("opensuse-tumbleweed".to_string())
        );
        assert_eq!(
            repo_to_distro("opensuse_leap_15_5"),
            Some("opensuse-leap_15_5".to_string())
        );
    }

    #[test]
    fn test_parse_empty_project() {
        let json = "[]";
        let project = parse_project_response("empty", json).unwrap();
        assert_eq!(project.name, "empty");
        assert!(project.implementations.is_empty());
    }

    #[test]
    fn test_parse_invalid_json() {
        let result = parse_project_response("bad", "not json");
        assert!(result.is_err());
    }
}
