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
#[derive(Debug, Clone)]
pub struct RepologyImplementation {
    pub repo: String,
    pub visiblename: String,
    pub version: String,
    pub status: String,
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
        .map(|p| RepologyImplementation {
            repo: p.repo,
            visiblename: p.visiblename,
            version: p.version,
            status: p.status,
        })
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
                .map(|p| RepologyImplementation {
                    repo: p.repo,
                    visiblename: p.visiblename,
                    version: p.version,
                    status: p.status,
                })
                .collect();
            RepologyProject {
                name,
                implementations,
            }
        })
        .collect();

    Ok(projects)
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
pub struct RepologyClient {
    client: reqwest::Client,
    base_url: String,
}

impl Default for RepologyClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RepologyClient {
    /// Create a new client pointing at the public Repology API.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: "https://repology.org".to_string(),
        }
    }

    /// Create a client with a custom base URL (useful for testing against a
    /// local mock server).
    pub fn with_base_url(url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: url.trim_end_matches('/').to_string(),
        }
    }

    /// Fetch a single project by name.
    pub async fn fetch_project(&self, name: &str) -> Result<RepologyProject> {
        let url = format!("{}/api/v1/project/{name}", self.base_url);
        let body = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::DownloadError(e.to_string()))?
            .text()
            .await
            .map_err(|e| Error::DownloadError(e.to_string()))?;

        parse_project_response(name, &body)
    }

    /// Fetch a batch of projects starting at the given name (alphabetical).
    pub async fn fetch_projects_batch(&self, start: &str) -> Result<Vec<RepologyProject>> {
        let url = format!("{}/api/v1/projects/{start}/", self.base_url);
        let body = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::DownloadError(e.to_string()))?
            .text()
            .await
            .map_err(|e| Error::DownloadError(e.to_string()))?;

        parse_projects_batch(&body)
    }

    /// Fetch a batch of projects and sync recognised implementations into the
    /// database. Returns the number of projects synced.
    pub async fn sync_to_db(
        &self,
        conn: &rusqlite::Connection,
        start: &str,
    ) -> Result<usize> {
        let projects = self.fetch_projects_batch(start).await?;
        let mut count = 0;

        for project in &projects {
            // Filter to implementations we can map to a known distro
            let known: Vec<_> = project
                .implementations
                .iter()
                .filter_map(|imp| {
                    repo_to_distro(&imp.repo)
                        .map(|distro| (distro, imp.visiblename.clone()))
                })
                .collect();

            if known.is_empty() {
                continue;
            }

            // Upsert the canonical package
            let mut canonical = CanonicalPackage::new(
                project.name.clone(),
                "package".to_string(),
            );
            let can_id = canonical.insert_or_ignore(conn)?;

            let Some(can_id) = can_id else {
                continue;
            };

            // Upsert each distro implementation
            for (distro, distro_name) in known {
                let mut imp = PackageImplementation::new(
                    can_id,
                    distro,
                    distro_name,
                    "repology".to_string(),
                );
                imp.insert_or_ignore(conn)?;
            }

            count += 1;
        }

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
