// src/commands/model/remote_diff.rs

use std::path::Path;

use super::super::open_db;
use super::context::load_model;
use anyhow::{Result, anyhow};
use conary_core::db::models::RemoteCollection;
use conary_core::model::remote::fetch_remote_collection;
use conary_core::model::{capture_current_state, parse_trove_spec};
use rusqlite::Connection;
use tracing::debug;

/// Compare local state against remote model collections
///
/// Fetches each remote include from the model (with optional forced refresh),
/// then compares remote collection members against installed packages to
/// detect drift.
pub async fn cmd_model_remote_diff(model_path: &str, db_path: &str, refresh: bool) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = open_db(db_path)?;
    let state = capture_current_state(&conn)?;

    if !model.has_includes() {
        println!("No remote includes in model");
        return Ok(());
    }

    let include_specs = &model.include.models;
    println!("Remote drift report:");
    println!();

    let mut total_drift = 0u32;
    let mut collections_checked = 0u32;

    for spec in include_specs {
        let (name, label) = parse_trove_spec(spec)?;

        let label_str = match &label {
            Some(l) => l.as_str(),
            None => {
                eprintln!("  Skipping '{}': no label for remote fetch", name);
                continue;
            }
        };

        // Purge cache if refresh requested
        if refresh {
            let purged =
                RemoteCollection::purge_by_name(&conn, &name, Some(label_str)).unwrap_or(0);
            if purged > 0 {
                debug!(name = %name, label = %label_str, "Purged {} cache entries", purged);
            }
        }

        // Fetch the remote collection
        let collection = match fetch_remote_collection(&conn, &name, label_str, false).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  Failed to fetch '{}': {}", spec, e);
                continue;
            }
        };

        collections_checked += 1;

        // Compare remote members against local state
        let mut missing: Vec<String> = Vec::new();
        let mut version_drift: Vec<(String, String, String)> = Vec::new();

        for member in &collection.members {
            if let Some(installed) = state.get_package(&member.name) {
                // Package is installed - check version constraint
                if let Some(constraint) = &member.version_constraint
                    && !version_matches_constraint(&installed.version, constraint)
                {
                    version_drift.push((
                        member.name.clone(),
                        constraint.clone(),
                        installed.version.clone(),
                    ));
                }
            } else {
                // Package not installed
                let suffix = if member.is_optional {
                    " (optional)"
                } else {
                    " (required)"
                };
                missing.push(format!("{}{}", member.name, suffix));
            }
        }

        let drift_count = missing.len() + version_drift.len();

        if drift_count > 0 {
            println!(
                "  {} ({}):",
                spec,
                format_version_info(&conn, &name, label_str)
            );

            if !missing.is_empty() {
                println!("    Missing locally:");
                for entry in &missing {
                    println!("      - {}", entry);
                }
            }

            if !version_drift.is_empty() {
                println!("    Version constraint drift:");
                for (pkg, constraint, installed) in &version_drift {
                    println!(
                        "      - {}: remote pins {}, installed {}",
                        pkg, constraint, installed
                    );
                }
            }

            println!();
        }

        total_drift += drift_count as u32;
    }

    println!(
        "Summary: {} collection(s) checked, {} drift(s) found",
        collections_checked, total_drift
    );

    if total_drift > 0 {
        return Err(anyhow!(
            "remote drift detected: {} drift(s) found",
            total_drift
        ));
    }

    Ok(())
}

/// Check if an installed version satisfies a version constraint pattern
///
/// Supports glob-style patterns (e.g. "1.24.*") and prefix comparisons.
fn version_matches_constraint(installed: &str, constraint: &str) -> bool {
    if constraint == installed {
        return true;
    }

    // Glob-style: "1.24.*" matches "1.24.0", "1.24.3", etc.
    if let Some(prefix) = constraint.strip_suffix(".*") {
        return installed == prefix || installed.starts_with(&format!("{}.", prefix));
    }

    // Prefix match: "1.24" matches "1.24.0"
    if installed.starts_with(constraint) && installed[constraint.len()..].starts_with('.') {
        return true;
    }

    false
}

/// Get version info string for display from cached collection data
fn format_version_info(conn: &Connection, name: &str, label: &str) -> String {
    if let Ok(Some(cached)) = RemoteCollection::find_cached(conn, name, Some(label))
        && let Some(version) = &cached.version
    {
        return format!("v{}", version);
    }
    "unknown version".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::RemoteCollection;
    use conary_core::model::SystemState;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn test_version_matches_constraint_exact() {
        assert!(version_matches_constraint("1.24.0", "1.24.0"));
        assert!(!version_matches_constraint("1.24.1", "1.24.0"));
    }

    #[test]
    fn test_version_matches_constraint_glob() {
        assert!(version_matches_constraint("1.24.0", "1.24.*"));
        assert!(version_matches_constraint("1.24.3", "1.24.*"));
        assert!(version_matches_constraint("1.24", "1.24.*"));
        assert!(!version_matches_constraint("1.25.0", "1.24.*"));
        assert!(!version_matches_constraint("2.24.0", "1.24.*"));
    }

    #[test]
    fn test_version_matches_constraint_prefix() {
        assert!(version_matches_constraint("1.24.0", "1.24"));
        assert!(!version_matches_constraint("1.25.0", "1.24"));
    }

    #[tokio::test]
    async fn test_remote_diff_detects_missing() {
        // Create test DB and populate cache
        let (_temp_file, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();

        // Create a cached remote collection with members
        let collection_data = serde_json::json!({
            "name": "group-test",
            "version": "1.0",
            "members": [
                {"name": "nginx", "version_constraint": "1.24.*", "is_optional": false},
                {"name": "redis", "version_constraint": null, "is_optional": false},
                {"name": "memcached", "version_constraint": null, "is_optional": true}
            ],
            "includes": [],
            "pins": {},
            "exclude": [],
            "content_hash": "sha256:test123",
            "published_at": "2026-01-01T00:00:00Z"
        });

        let mut cache_entry = RemoteCollection::new(
            "group-test".to_string(),
            Some("myrepo:stable".to_string()),
            "sha256:test123".to_string(),
            serde_json::to_string(&collection_data).unwrap(),
            "2099-12-31T23:59:59".to_string(),
        );
        cache_entry.version = Some("1.0".to_string());
        cache_entry.upsert(&conn).unwrap();

        // Create a system state with only nginx installed
        let state = SystemState {
            installed: HashMap::from([(
                "nginx".to_string(),
                vec![conary_core::model::InstalledPackage {
                    name: "nginx".to_string(),
                    version: "1.24.2".to_string(),
                    architecture: None,
                    explicit: true,
                    pinned: false,
                    label: None,
                }],
            )]),
            explicit: HashSet::from(["nginx".to_string()]),
            pinned: HashSet::new(),
            source_pin: None,
            selection_mode: None,
            allowed_distros: Vec::new(),
        };

        // Fetch the collection from cache
        let fetched = conary_core::model::remote::fetch_remote_collection(
            &conn,
            "group-test",
            "myrepo:stable",
            false,
        )
        .await
        .unwrap();

        // Simulate the drift detection logic from cmd_model_remote_diff
        let mut missing = Vec::new();
        let mut version_drift = Vec::new();

        for member in &fetched.members {
            if let Some(installed) = state.get_package(&member.name) {
                if let Some(constraint) = &member.version_constraint
                    && !version_matches_constraint(&installed.version, constraint)
                {
                    version_drift.push(member.name.clone());
                }
            } else {
                missing.push(member.name.clone());
            }
        }

        // redis and memcached should be missing
        assert_eq!(missing.len(), 2);
        assert!(missing.contains(&"redis".to_string()));
        assert!(missing.contains(&"memcached".to_string()));

        // nginx 1.24.2 matches constraint 1.24.* so no version drift
        assert!(version_drift.is_empty());
    }
}
