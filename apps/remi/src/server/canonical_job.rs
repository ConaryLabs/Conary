// apps/remi/src/server/canonical_job.rs

//! Scheduled job that builds the canonical package mapping from all
//! indexed distros. Runs after mirror sync or on demand.
//!
//! The rebuild runs a 3-phase pipeline, each phase committing independently
//! for short write locks:
//!
//! 1. **Curated Rules** — YAML rules from `config.rules_dir`
//! 2. **Repology** — Cross-distro mappings from `repology_cache`
//! 3. **AppStream** — Enrichment with AppStream component IDs
//!
//! Package names, provides, and payload paths are never canonical mapping
//! authority. Every persisted mapping comes from one of the parsed contracts
//! above.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use conary_core::canonical::rules::RulesEngine;
use conary_core::db::models::{
    AppstreamCacheEntry, CanonicalPackage, MetadataTable, PackageImplementation,
    RepologyCacheEntry, get_metadata, set_metadata,
};
use rusqlite::Connection;
use tracing::{debug, info, warn};

use crate::server::config::CanonicalSection;

// ---------------------------------------------------------------------------
// Debounce helpers
// ---------------------------------------------------------------------------

/// Check whether enough time has elapsed since the last rebuild.
///
/// Reads `last_canonical_rebuild` from `server_metadata`. Returns `true`
/// if the timestamp is missing or older than `cooldown_minutes`.
pub fn should_rebuild(conn: &Connection, cooldown_minutes: u64) -> bool {
    let value = match get_metadata(conn, MetadataTable::Server, "last_canonical_rebuild") {
        Ok(Some(v)) => v,
        _ => return true,
    };

    let last = match chrono::DateTime::parse_from_rfc3339(&value) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(_) => return true,
    };

    let elapsed = Utc::now().signed_duration_since(last);
    elapsed.num_minutes() >= cooldown_minutes as i64
}

/// Record the current UTC time as the last rebuild timestamp.
pub fn record_rebuild_timestamp(conn: &Connection) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    set_metadata(conn, MetadataTable::Server, "last_canonical_rebuild", &now)?;
    Ok(())
}

/// Atomically increment and return the canonical map version counter.
pub fn bump_map_version(conn: &Connection) -> Result<u64> {
    let current = match get_metadata(conn, MetadataTable::Server, "canonical_map_version")? {
        Some(value) => value
            .parse::<u64>()
            .map_err(|error| anyhow::anyhow!("invalid canonical map version '{value}': {error}"))?,
        None => 0,
    };

    let next = current + 1;
    set_metadata(
        conn,
        MetadataTable::Server,
        "canonical_map_version",
        &next.to_string(),
    )?;
    Ok(next)
}

// ---------------------------------------------------------------------------
// 3-phase rebuild
// ---------------------------------------------------------------------------

/// Rebuild the canonical map from all enabled repositories.
///
/// Opens the database at `db_path` and runs three phases, each committing
/// independently for short write locks. After all phases, records the
/// rebuild timestamp and bumps the map version.
///
/// Returns the total count of newly created canonical package entries.
pub fn rebuild_canonical_map(db_path: &Path, config: &CanonicalSection) -> Result<u64> {
    let conn = crate::server::open_runtime_db(db_path)?;
    let rules_dir = Path::new(&config.rules_dir);
    let mut total_new: u64 = 0;

    // Phase 1 — Curated Rules
    total_new += phase_curated_rules(&conn, rules_dir)?;

    // Phase 2 — Repology
    total_new += phase_repology(&conn)?;

    // Phase 3 — AppStream
    total_new += phase_appstream(&conn)?;

    // Finalise: record timestamp and bump version
    record_rebuild_timestamp(&conn)?;
    let version = bump_map_version(&conn)?;

    info!(
        "Canonical map rebuild complete: {} new mappings, map version {}",
        total_new, version
    );

    Ok(total_new)
}

// ---------------------------------------------------------------------------
// Phase 1 — Curated Rules
// ---------------------------------------------------------------------------

/// Load YAML rules from `rules_dir` and insert canonical entries for every
/// rule that has both a `setname` and a `name`.
fn phase_curated_rules(conn: &Connection, rules_dir: &Path) -> Result<u64> {
    let engine = if rules_dir.is_dir() {
        let engine = RulesEngine::load_from_dir(rules_dir)?;
        info!(
            "Phase 1: loaded {} curated rules from {}",
            engine.rule_count(),
            rules_dir.display()
        );
        engine
    } else {
        debug!(
            "Phase 1: no rules directory at {}, skipping",
            rules_dir.display()
        );
        return Ok(0);
    };

    let tx = conn.unchecked_transaction()?;
    let mut new_count: u64 = 0;

    for rule in engine.rules() {
        if rule.setname.is_empty() || rule.name.is_empty() {
            continue;
        }

        let Some(repository_identities) = rule.repo.as_ref() else {
            warn!(
                "Phase 1: canonical rule '{}' -> '{}' has no repository identity; skipping",
                rule.name, rule.setname
            );
            continue;
        };
        let identities: Vec<&str> = match repository_identities {
            conary_core::canonical::rules::StringOrVec::Single(identity) => {
                vec![identity.as_str()]
            }
            conary_core::canonical::rules::StringOrVec::Multiple(identities) => {
                identities.iter().map(String::as_str).collect()
            }
        };
        let mut profile_ids = identities
            .into_iter()
            .filter_map(|identity| {
                conary_core::repository::supported_profiles::profile_by_repository_identity(
                    identity,
                )
                .map(|profile| profile.id().to_string())
            })
            .collect::<Vec<_>>();
        profile_ids.sort();
        profile_ids.dedup();
        if profile_ids.is_empty() {
            continue;
        }

        let kind = rule.kind.clone().unwrap_or_else(|| "package".to_string());

        let mut canonical = CanonicalPackage::new(rule.setname.clone(), kind);
        let already_exists = CanonicalPackage::find_by_name(&tx, &rule.setname)?.is_some();
        let id = canonical.insert_or_ignore(&tx)?;

        if let Some(canonical_id) = id {
            for profile_id in profile_ids {
                let mut imp = PackageImplementation::new(
                    canonical_id,
                    profile_id,
                    rule.name.clone(),
                    "curated".to_string(),
                );
                imp.insert_or_ignore(&tx)?;
            }

            if !already_exists {
                new_count += 1;
            }
        }
    }

    tx.commit()?;
    info!(
        "Phase 1: {} new canonical entries from curated rules",
        new_count
    );
    Ok(new_count)
}

// ---------------------------------------------------------------------------
// Phase 2 — Repology
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepologyMappingStatus {
    Newest,
    Devel,
    Unique,
    Outdated,
    Rolling,
    Legacy,
}

impl RepologyMappingStatus {
    fn parse(value: Option<&str>) -> Result<Self> {
        match value {
            Some("newest") => Ok(Self::Newest),
            Some("devel") => Ok(Self::Devel),
            Some("unique") => Ok(Self::Unique),
            Some("outdated") => Ok(Self::Outdated),
            Some("rolling") => Ok(Self::Rolling),
            Some("legacy") => Ok(Self::Legacy),
            Some(other) => anyhow::bail!("unsupported Repology package status '{other}'"),
            None => anyhow::bail!("Repology package status is missing"),
        }
    }

    fn is_mapping_eligible(self) -> bool {
        matches!(
            self,
            Self::Newest | Self::Devel | Self::Unique | Self::Outdated | Self::Rolling
        )
    }
}

/// Read the `repology_cache` table, group by `project_name`, and insert
/// canonical entries for projects that appear in 2+ distinct distros with
/// an acceptable status.
fn phase_repology(conn: &Connection) -> Result<u64> {
    let entries = RepologyCacheEntry::find_all(conn)?;

    if entries.is_empty() {
        warn!("Phase 2: repology_cache is empty, skipping");
        return Ok(0);
    }

    // Group by project_name.
    let mut by_project: HashMap<String, Vec<RepologyCacheEntry>> = HashMap::new();
    for entry in entries {
        by_project
            .entry(entry.project_name.clone())
            .or_default()
            .push(entry);
    }

    let tx = conn.unchecked_transaction()?;
    let mut new_count: u64 = 0;

    for (project_name, entries) in &by_project {
        // Filter to acceptable statuses and collect distinct distros.
        let mut good: Vec<(
            &RepologyCacheEntry,
            &conary_core::repository::supported_profiles::SupportedProfile,
        )> = Vec::new();
        for entry in entries {
            let status = RepologyMappingStatus::parse(entry.status.as_deref())?;
            if !status.is_mapping_eligible() {
                continue;
            }
            if let Some(profile) =
                conary_core::repository::supported_profiles::profile_by_public_id(&entry.distro)
            {
                good.push((entry, profile));
            }
        }

        let distinct_distros: HashSet<&str> =
            good.iter().map(|(_, profile)| profile.id()).collect();

        if distinct_distros.len() < 2 {
            continue;
        }

        let already_exists = CanonicalPackage::find_by_name(&tx, project_name)?.is_some();
        let mut canonical = CanonicalPackage::new(project_name.clone(), "package".to_string());
        let id = canonical.insert_or_ignore(&tx)?;

        if let Some(canonical_id) = id {
            for (entry, profile) in &good {
                let mut imp = PackageImplementation::new(
                    canonical_id,
                    profile.id().to_string(),
                    entry.distro_name.clone(),
                    "repology".to_string(),
                );
                imp.insert_or_ignore(&tx)?;
            }

            if !already_exists {
                new_count += 1;
            }
        }
    }

    tx.commit()?;
    info!(
        "Phase 2: {} new canonical entries from repology ({} projects examined)",
        new_count,
        by_project.len()
    );
    Ok(new_count)
}

// ---------------------------------------------------------------------------
// Phase 3 — AppStream
// ---------------------------------------------------------------------------

/// Enrich canonical entries with AppStream component IDs.
///
/// For each cached AppStream component, look for an existing
/// `package_implementations` row where both `distro` and `distro_name` match
/// the AppStream entry.
/// If found, set `appstream_id` on the parent `canonical_packages` row.
/// If no existing mapping, insert a new canonical entry with the
/// AppStream ID attached.
fn phase_appstream(conn: &Connection) -> Result<u64> {
    let entries = AppstreamCacheEntry::find_all(conn)?;

    if entries.is_empty() {
        debug!("Phase 3: appstream_cache is empty, skipping");
        return Ok(0);
    }

    let tx = conn.unchecked_transaction()?;
    let mut new_count: u64 = 0;

    for entry in &entries {
        let existing =
            PackageImplementation::find_by_distro_name(&tx, &entry.distro, &entry.pkgname)?;

        if let Some(impl_row) = existing {
            // Enrich the parent canonical package with the AppStream ID.
            tx.execute(
                "UPDATE canonical_packages SET appstream_id = ?1 WHERE id = ?2 AND appstream_id IS NULL",
                rusqlite::params![entry.appstream_id, impl_row.canonical_id],
            )?;
        } else {
            // No existing mapping -- create a new canonical entry.
            let already_exists = CanonicalPackage::find_by_name(&tx, &entry.pkgname)?.is_some();
            let mut canonical = CanonicalPackage::new(entry.pkgname.clone(), "package".to_string());
            canonical.appstream_id = Some(entry.appstream_id.clone());
            let id = canonical.insert_or_ignore(&tx)?;

            if let Some(canonical_id) = id {
                // If the entry was already there (insert_or_ignore), still try
                // to set the appstream_id if it was NULL.
                tx.execute(
                    "UPDATE canonical_packages SET appstream_id = ?1 WHERE id = ?2 AND appstream_id IS NULL",
                    rusqlite::params![entry.appstream_id, canonical_id],
                )?;

                let mut imp = PackageImplementation::new(
                    canonical_id,
                    entry.distro.clone(),
                    entry.pkgname.clone(),
                    "appstream".to_string(),
                );
                imp.insert_or_ignore(&tx)?;

                if !already_exists {
                    new_count += 1;
                }
            }
        }
    }

    tx.commit()?;
    info!(
        "Phase 3: {} new canonical entries from appstream ({} components examined)",
        new_count,
        entries.len()
    );
    Ok(new_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::schema;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn create_test_db(dir: &TempDir) -> std::path::PathBuf {
        let db_path = dir.path().join("conary.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();

        // Insert a test repository
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, default_strategy_distro)
             VALUES ('fedora-44', 'https://example.com/fedora', 1, 'fedora')",
            [],
        )
        .unwrap();

        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE name = 'fedora-44'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Insert test packages
        conn.execute(
            "INSERT INTO repository_packages
                (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES
                (?1, 'curl', '8.5.0', 'abc123', 1000, 'https://example.com/curl.rpm', 'rpm')",
            [repo_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repository_packages
                (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES
                (?1, 'wget', '1.21', 'def456', 2000, 'https://example.com/wget.rpm', 'rpm')",
            [repo_id],
        )
        .unwrap();

        // Insert a second repository with the same package names. Similarity
        // must never create canonical mapping authority.
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, default_strategy_distro)
             VALUES ('arch', 'https://example.com/arch', 1, 'arch')",
            [],
        )
        .unwrap();

        let repo2_id: i64 = conn
            .query_row("SELECT id FROM repositories WHERE name = 'arch'", [], |r| {
                r.get(0)
            })
            .unwrap();

        conn.execute(
            "INSERT INTO repository_packages
                (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES
                (?1, 'curl', '8.5.0', 'abc124', 1000, 'https://example.com/curl.pkg.tar.zst', 'arch')",
            [repo2_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repository_packages
                (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES
                (?1, 'wget', '1.21', 'def457', 2000, 'https://example.com/wget.pkg.tar.zst', 'arch')",
            [repo2_id],
        )
        .unwrap();

        db_path
    }

    #[test]
    fn test_should_rebuild_respects_cooldown() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();

        // No previous rebuild -- should proceed
        assert!(should_rebuild(&conn, 5));

        // Record a rebuild
        record_rebuild_timestamp(&conn).unwrap();

        // Immediately after -- should skip (within 5 min cooldown)
        assert!(!should_rebuild(&conn, 5));
    }

    #[test]
    fn test_bump_map_version() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();

        let v1 = bump_map_version(&conn).unwrap();
        assert_eq!(v1, 1);

        let v2 = bump_map_version(&conn).unwrap();
        assert_eq!(v2, 2);
    }

    #[test]
    fn bump_map_version_rejects_corrupt_persisted_state() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        set_metadata(
            &conn,
            MetadataTable::Server,
            "canonical_map_version",
            "not-a-version",
        )
        .unwrap();

        let error = bump_map_version(&conn).unwrap_err();
        assert!(error.to_string().contains("invalid canonical map version"));
    }

    #[test]
    fn test_rebuild_canonical_map_empty_db() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("conary.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();
        drop(conn);

        let config = CanonicalSection {
            rules_dir: dir.path().join("rules").to_string_lossy().to_string(),
            ..Default::default()
        };
        let count = rebuild_canonical_map(&db_path, &config).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn repository_similarity_does_not_create_canonical_authority() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);
        let config = CanonicalSection {
            rules_dir: dir.path().join("rules").to_string_lossy().to_string(),
            ..Default::default()
        };

        let count = rebuild_canonical_map(&db_path, &config).unwrap();
        assert_eq!(count, 0);

        let conn = conary_core::db::open(&db_path).unwrap();
        assert!(
            CanonicalPackage::find_by_name(&conn, "curl")
                .unwrap()
                .is_none(),
            "matching package names across repositories are discovery evidence, not mapping authority"
        );
        assert!(
            CanonicalPackage::find_by_name(&conn, "wget")
                .unwrap()
                .is_none(),
            "matching package names across repositories are discovery evidence, not mapping authority"
        );
    }

    #[test]
    fn test_rebuild_canonical_map_with_rules() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);

        let rules_dir = dir.path().join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(
            rules_dir.join("01-rename.yaml"),
            "rules:\n  - name: curl\n    setname: curl-tools\n    repo: fedora\n",
        )
        .unwrap();

        let config = CanonicalSection {
            rules_dir: rules_dir.to_string_lossy().to_string(),
            ..Default::default()
        };
        let count = rebuild_canonical_map(&db_path, &config).unwrap();
        assert!(count > 0);

        // Verify the curated rule took effect
        let conn = conary_core::db::open(&db_path).unwrap();
        let pkg =
            conary_core::db::models::CanonicalPackage::find_by_name(&conn, "curl-tools").unwrap();
        assert!(
            pkg.is_some(),
            "curated rule should create 'curl-tools' canonical entry"
        );
    }

    #[test]
    fn configured_invalid_curated_rules_are_a_hard_error() {
        let dir = TempDir::new().unwrap();
        let rules_dir = dir.path().join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("invalid.yaml"), "rules: [").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();

        let error = phase_curated_rules(&conn, &rules_dir).unwrap_err();
        assert!(
            error.to_string().contains("YAML parse error"),
            "configured rules are an authority contract and parse failures must abort: {error}"
        );
    }

    #[test]
    fn test_phase_repology_filters_by_status_and_distro_count() {
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();

        // Insert cache entries: python in 2 distros (newest) -- should be included
        RepologyCacheEntry::insert_or_replace(
            &conn,
            &RepologyCacheEntry {
                project_name: "python".into(),
                distro: "fedora-44".into(),
                distro_name: "python3".into(),
                version: Some("3.12.0".into()),
                status: Some("newest".into()),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();
        RepologyCacheEntry::insert_or_replace(
            &conn,
            &RepologyCacheEntry {
                project_name: "python".into(),
                distro: "arch".into(),
                distro_name: "python".into(),
                version: Some("3.12.0".into()),
                status: Some("newest".into()),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();

        // Insert cache entry: legacy-pkg in 1 distro only -- should be skipped
        RepologyCacheEntry::insert_or_replace(
            &conn,
            &RepologyCacheEntry {
                project_name: "legacy-pkg".into(),
                distro: "arch".into(),
                distro_name: "legacy-pkg".into(),
                version: Some("0.1".into()),
                status: Some("legacy".into()),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();

        let count = phase_repology(&conn).unwrap();
        assert_eq!(count, 1, "only python should be inserted");

        let pkg = CanonicalPackage::find_by_name(&conn, "python").unwrap();
        assert!(pkg.is_some());

        let legacy = CanonicalPackage::find_by_name(&conn, "legacy-pkg").unwrap();
        assert!(legacy.is_none());
    }

    #[test]
    fn test_phase_appstream_enriches_existing() {
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();

        // Create a canonical package with an implementation
        let mut pkg = CanonicalPackage::new("firefox".into(), "package".into());
        let can_id = pkg.insert(&conn).unwrap();
        let mut imp = PackageImplementation::new(
            can_id,
            "fedora".into(),
            "firefox".into(),
            "repology".into(),
        );
        imp.insert(&conn).unwrap();

        // Insert AppStream entry matching the implementation
        AppstreamCacheEntry::insert_or_replace(
            &conn,
            &AppstreamCacheEntry {
                appstream_id: "org.mozilla.firefox".into(),
                pkgname: "firefox".into(),
                display_name: Some("Firefox".into()),
                summary: Some("Web Browser".into()),
                distro: "fedora".into(),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();

        let count = phase_appstream(&conn).unwrap();
        assert_eq!(count, 0, "no new canonical entries, just enrichment");

        let updated = CanonicalPackage::find_by_name(&conn, "firefox")
            .unwrap()
            .unwrap();
        assert_eq!(updated.appstream_id.as_deref(), Some("org.mozilla.firefox"));
    }

    #[test]
    fn appstream_enrichment_uses_exact_distro_identity() {
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();

        let mut fedora_pkg = CanonicalPackage::new("fedora-editor".into(), "package".into());
        let fedora_id = fedora_pkg.insert(&conn).unwrap();
        let mut fedora_imp = PackageImplementation::new(
            fedora_id,
            "fedora".into(),
            "editor".into(),
            "repology".into(),
        );
        fedora_imp.insert(&conn).unwrap();

        let mut arch_pkg = CanonicalPackage::new("arch-editor".into(), "package".into());
        let arch_id = arch_pkg.insert(&conn).unwrap();
        let mut arch_imp =
            PackageImplementation::new(arch_id, "arch".into(), "editor".into(), "repology".into());
        arch_imp.insert(&conn).unwrap();

        AppstreamCacheEntry::insert_or_replace(
            &conn,
            &AppstreamCacheEntry {
                appstream_id: "org.example.Editor".into(),
                pkgname: "editor".into(),
                display_name: Some("Editor".into()),
                summary: None,
                distro: "fedora".into(),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();

        let count = phase_appstream(&conn).unwrap();
        assert_eq!(count, 0);

        let fedora = CanonicalPackage::find_by_name(&conn, "fedora-editor")
            .unwrap()
            .unwrap();
        let arch = CanonicalPackage::find_by_name(&conn, "arch-editor")
            .unwrap()
            .unwrap();
        assert_eq!(fedora.appstream_id.as_deref(), Some("org.example.Editor"));
        assert_eq!(
            arch.appstream_id, None,
            "same package name in another distro is not AppStream identity authority"
        );
    }
}
