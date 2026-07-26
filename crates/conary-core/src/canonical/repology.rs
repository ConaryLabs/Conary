// conary-core/src/canonical/repology.rs

//! Repology discovery-metadata client.
//!
//! Repology tracks packaging across hundreds of repositories and distributions.
//! This module fetches and caches project observations. Repology observations
//! never create canonical equivalence or rank mutation candidates.

use std::collections::BTreeMap;

use serde::Deserialize;

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

/// Map an exact Conary public source-profile ID to a Repology repository ID.
///
/// This is the inverse of `repo_to_profile`. Family and route slugs are not
/// profile aliases.
pub fn profile_to_repo(profile_id: &str) -> Option<String> {
    crate::repository::supported_profiles::profile_by_public_id(profile_id)
        .map(|profile| profile.repology_repo().to_string())
}

/// Map a Repology repository ID to an exact Conary public source-profile ID.
///
/// Returns `None` for repositories we do not recognise.
///
/// A mapping here does not imply that Remi serves the profile. Serving support
/// is owned by the typed source-profile catalog and repository configuration.
pub fn repo_to_profile(repo: &str) -> Option<String> {
    crate::repository::supported_profiles::public_profiles()
        .iter()
        .find(|profile| profile.repology_repo() == repo)
        .map(|profile| profile.id().to_string())
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

const USER_AGENT: &str = concat!(
    "conary/",
    env!("CARGO_PKG_VERSION"),
    " (https://conary.io; canonical-registry-sync)"
);

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
}

// ---------------------------------------------------------------------------
// Cache persistence
// ---------------------------------------------------------------------------

/// Write a batch of Repology projects to the `repology_cache` table.
/// Maps Repology repo IDs to Conary distro names, skipping unrecognised repos.
/// Returns the number of cache entries written.
pub fn cache_projects_to_db(
    conn: &rusqlite::Connection,
    projects: &[RepologyProject],
) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut count = 0;

    for project in projects {
        for imp in &project.implementations {
            let Some(distro) = repo_to_profile(&imp.repo) else {
                continue;
            };
            let entry = crate::db::models::RepologyCacheEntry {
                project_name: project.name.clone(),
                distro,
                distro_name: imp.visiblename.clone(),
                version: Some(imp.version.clone()),
                status: Some(imp.status.clone()),
                fetched_at: now.clone(),
            };
            crate::db::models::RepologyCacheEntry::insert_or_replace(&tx, &entry)?;
            count += 1;
        }
    }

    tx.commit()?;
    Ok(count)
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
                {"repo": "ubuntu_26_04", "visiblename": "wget", "version": "1.21.4", "status": "outdated"}
            ]
        }"#;
        let projects = parse_projects_batch(json).unwrap();
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn supported_profile_catalog_owns_repology_identity() {
        assert_eq!(repo_to_profile("fedora_44"), Some("fedora-44".to_string()));
        assert_eq!(
            repo_to_profile("ubuntu_26_04"),
            Some("ubuntu-26.04".to_string())
        );
        assert_eq!(repo_to_profile("arch"), Some("arch".to_string()));
        assert_eq!(profile_to_repo("fedora-44"), Some("fedora_44".to_string()));
        assert_eq!(profile_to_repo("fedora"), None);
        assert_eq!(
            profile_to_repo("ubuntu-26.04"),
            Some("ubuntu_26_04".to_string())
        );
        assert_eq!(repo_to_profile("unknown_repo_xyz"), None);
        assert_eq!(repo_to_profile("fedora_41"), None);
        assert_eq!(repo_to_profile("debian_10"), None);
        assert_eq!(repo_to_profile("opensuse_tumbleweed"), None);
        assert_eq!(profile_to_repo("debian-10"), None);
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

    #[test]
    fn test_cache_repology_projects() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();

        let projects = vec![RepologyProject {
            name: "python".into(),
            implementations: vec![
                RepologyImplementation {
                    repo: "fedora_44".into(),
                    visiblename: "python3".into(),
                    version: "3.12.0".into(),
                    status: "newest".into(),
                },
                RepologyImplementation {
                    repo: "arch".into(),
                    visiblename: "python".into(),
                    version: "3.12.0".into(),
                    status: "newest".into(),
                },
            ],
        }];

        let count = cache_projects_to_db(&conn, &projects).unwrap();
        assert_eq!(count, 2);

        let entries = crate::db::models::RepologyCacheEntry::find_all(&conn).unwrap();
        assert_eq!(entries.len(), 2);
    }
}
