// src/commands/cache.rs

//! Implementation of `conary cache` commands.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::profile::resolve_recipe_root_for_manifest;
use conary_core::bootstrap::{BootstrapConfig, PackageBuildRunner};
use conary_core::derivation::{BuildProfile, load_recipes};

#[derive(Debug, Default)]
struct SourcePrefetchStats {
    downloaded: u64,
    skipped: u64,
}

#[derive(Debug, Default)]
struct OutputPrefetchStats {
    total: usize,
    available: usize,
    unavailable: usize,
    fetched: u64,
    total_bytes: u64,
}

fn source_cache_dir(db_path: &str) -> PathBuf {
    conary_core::db::paths::db_dir(db_path).join("sources")
}

fn profile_derivation_ids(profile: &BuildProfile) -> Vec<String> {
    profile
        .stages
        .iter()
        .flat_map(|stage| stage.derivations.iter())
        .filter(|drv| drv.derivation_id != "pending")
        .map(|drv| drv.derivation_id.clone())
        .collect()
}

fn prefetch_profile_sources(profile: &BuildProfile, db_path: &str) -> Result<SourcePrefetchStats> {
    let manifest_path = PathBuf::from(&profile.profile.manifest);
    if !manifest_path.exists() {
        anyhow::bail!("Profile manifest not found: {}", manifest_path.display());
    }

    let recipe_root = resolve_recipe_root_for_manifest(&manifest_path)?;
    let recipes = load_recipes(&recipe_root)
        .with_context(|| format!("Failed to load recipes from {}", recipe_root.display()))?;

    let sources_dir = source_cache_dir(db_path);
    std::fs::create_dir_all(&sources_dir)
        .with_context(|| format!("Failed to create source cache: {}", sources_dir.display()))?;

    let runner = PackageBuildRunner::new(&sources_dir, &BootstrapConfig::new());
    let mut seen_packages = HashSet::new();
    let mut stats = SourcePrefetchStats::default();

    for drv in profile
        .stages
        .iter()
        .flat_map(|stage| stage.derivations.iter())
    {
        if !seen_packages.insert(drv.package.clone()) {
            continue;
        }

        let recipe = recipes.get(&drv.package).ok_or_else(|| {
            anyhow::anyhow!(
                "recipe for '{}' not found in {}",
                drv.package,
                recipe_root.display()
            )
        })?;

        let target_path = sources_dir.join(recipe.archive_filename());
        let cached = target_path.exists()
            && runner
                .verify_checksum(&drv.package, &recipe.source.checksum, &target_path)
                .is_ok();

        runner
            .fetch_source(&drv.package, recipe)
            .map_err(|e| anyhow::anyhow!("Failed to prefetch source for {}: {e}", drv.package))?;

        if cached {
            stats.skipped += 1;
        } else {
            stats.downloaded += 1;
        }
    }

    Ok(stats)
}

async fn prefetch_remote_outputs(
    profile: &BuildProfile,
    db_path: &str,
) -> Result<OutputPrefetchStats> {
    let derivation_ids = profile_derivation_ids(profile);
    let total = derivation_ids.len();

    if total == 0 {
        return Ok(OutputPrefetchStats::default());
    }

    let conn = super::open_db(db_path)?;

    let mut substituter = conary_core::derivation::substituter::DerivationSubstituter::from_db(
        &conn,
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "No substituter peers configured: {e}. Add peers with substituter config first."
        )
    })?;

    let peers = substituter.peers();
    if peers.is_empty() {
        anyhow::bail!("No substituter peers available");
    }
    let probe_endpoint = peers[0].endpoint.clone();

    println!("Probing {probe_endpoint} for {total} derivations...");

    let availability = substituter
        .batch_probe(&derivation_ids, &probe_endpoint)
        .await
        .map_err(|e| anyhow::anyhow!("probe failed: {e}"))?;

    let available: Vec<&String> = derivation_ids
        .iter()
        .filter(|id| availability.get(*id).copied().unwrap_or(false))
        .collect();
    let unavailable = total - available.len();

    println!(
        "{} available remotely, {} will need local build.",
        available.len(),
        unavailable
    );

    let cas_dir = conary_core::db::paths::objects_dir(db_path);
    let cas = conary_core::filesystem::CasStore::new(&cas_dir)
        .map_err(|e| anyhow::anyhow!("failed to open CAS: {e}"))?;

    let mut stats = OutputPrefetchStats {
        total,
        available: available.len(),
        unavailable,
        ..OutputPrefetchStats::default()
    };

    for (i, id) in available.iter().enumerate() {
        let short_id = &id[..16.min(id.len())];
        print!("\r  Fetching {}/{}: {short_id}...", i + 1, available.len());
        match substituter.query(id).await {
            conary_core::derivation::substituter::CacheQueryResult::Hit { manifest, peer } => {
                match substituter
                    .fetch_missing_objects(&manifest, &cas, &peer)
                    .await
                {
                    Ok(report) => {
                        stats.fetched += 1;
                        stats.total_bytes += report.bytes_transferred;
                    }
                    Err(e) => {
                        eprintln!("\n  [WARN] Failed to fetch objects for {short_id}: {e}");
                    }
                }
            }
            conary_core::derivation::substituter::CacheQueryResult::Miss => {}
        }
    }

    Ok(stats)
}

/// Pre-fetch derivation outputs from configured substituters.
pub async fn cmd_cache_populate(
    profile_path: &str,
    sources_only: bool,
    full: bool,
    db_path: &str,
) -> Result<()> {
    // Read profile TOML
    let content = std::fs::read_to_string(profile_path)
        .map_err(|e| anyhow::anyhow!("failed to read profile: {e}"))?;

    let profile: BuildProfile =
        toml::from_str(&content).map_err(|e| anyhow::anyhow!("failed to parse profile: {e}"))?;

    let total = profile_derivation_ids(&profile).len();
    println!(
        "Profile has {total} derivations across {} stages.",
        profile.stages.len()
    );

    if sources_only {
        let stats = prefetch_profile_sources(&profile, db_path)?;
        println!(
            "Downloaded {} source archives, skipped {} already cached.",
            stats.downloaded, stats.skipped
        );
        return Ok(());
    }

    match prefetch_remote_outputs(&profile, db_path).await {
        Ok(stats) => {
            println!(
                "\n\nDownloaded {}/{} derivation outputs ({:.1} MB)",
                stats.fetched,
                stats.available,
                stats.total_bytes as f64 / 1_048_576.0,
            );
            if stats.unavailable > 0 {
                println!(
                    "{} derivations will be built from source.",
                    stats.unavailable
                );
            }
            if stats.total == 0 {
                println!("No derivation outputs needed prefetch.");
            }
        }
        Err(err) if full => {
            eprintln!("[WARN] Failed to prefetch derivation outputs: {err}");
        }
        Err(err) => return Err(err),
    }

    if full {
        let stats = prefetch_profile_sources(&profile, db_path)?;
        println!(
            "\nDownloaded {} source archives, skipped {} already cached.",
            stats.downloaded, stats.skipped
        );
    }

    Ok(())
}

/// Show cache statistics and substituter peer health.
pub async fn cmd_cache_status(db_path: &str) -> Result<()> {
    // CAS directory info
    let cas_dir = conary_core::db::paths::objects_dir(db_path);
    if cas_dir.exists() {
        let size = dir_size(&cas_dir);
        let count = dir_file_count(&cas_dir);
        println!(
            "CAS directory: {} ({:.1} MB, {} objects)",
            cas_dir.display(),
            size as f64 / 1_048_576.0,
            count,
        );
    } else {
        println!("CAS directory: {} (not found)", cas_dir.display());
    }

    // Derivation index count and substituter peers
    if let Ok(conn) = super::open_db(db_path) {
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM derivation_index", [], |r| r.get(0))
            .unwrap_or(0);
        println!("Cached derivations: {count}");

        let mut stmt = conn.prepare(
            "SELECT endpoint, success_count, failure_count, last_seen \
                 FROM substituter_peers ORDER BY priority",
        )?;
        let peers: Vec<(String, i64, i64, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        if peers.is_empty() {
            println!("Substituter peers: none configured");
        } else {
            println!("Substituter peers:");
            for (endpoint, success, failure, last_seen) in &peers {
                let seen = last_seen.as_deref().unwrap_or("never");
                println!(
                    "  {endpoint}  (success: {success}, failures: {failure}, last seen: {seen})"
                );
            }
        }
    }

    Ok(())
}

fn dir_size(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

fn dir_file_count(path: &std::path::Path) -> usize {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .count()
}

#[cfg(test)]
mod tests {
    use super::cmd_cache_populate;
    use crate::commands::profile::cmd_profile_generate;
    use conary_core::derivation::compose::erofs_image_hash;
    use conary_core::derivation::seed::{SeedMetadata, SeedSource};
    use conary_core::hash;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn write_archive(root: &Path, package: &str) -> (PathBuf, Vec<u8>, String) {
        let archive_dir = root.join("distfiles");
        fs::create_dir_all(&archive_dir).unwrap();

        let archive_path = archive_dir.join(format!("{package}-1.0.0.tar.gz"));
        let contents = format!("phase3 cache source for {package}").into_bytes();
        fs::write(&archive_path, &contents).unwrap();

        (
            archive_path,
            contents.clone(),
            format!("sha256:{}", hash::sha256(&contents)),
        )
    }

    fn write_recipe(
        recipe_root: &Path,
        relative_path: &str,
        name: &str,
        archive: &Path,
        checksum: &str,
    ) {
        let path = recipe_root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }

        fs::write(
            path,
            format!(
                r#"[package]
name = "{name}"
version = "1.0.0"

[source]
archive = "file://{}"
checksum = "{checksum}"

[build]
requires = []
makedepends = []
install = "make install DESTDIR=%(destdir)s"
"#,
                archive.display()
            ),
        )
        .unwrap();
    }

    fn write_seed_dir(root: &Path) -> PathBuf {
        let seed_dir = root.join("seed");
        fs::create_dir_all(&seed_dir).unwrap();

        let image_path = seed_dir.join("seed.erofs");
        fs::write(&image_path, b"phase3 cache test seed").unwrap();
        let seed_id = erofs_image_hash(&image_path).unwrap();

        let seed = SeedMetadata {
            seed_id,
            source: SeedSource::SelfBuilt,
            origin_url: None,
            builder: Some("test".to_string()),
            packages: vec!["hello".to_string()],
            target_triple: "x86_64-unknown-linux-gnu".to_string(),
            verified_by: vec![],
            origin_distro: None,
            origin_version: None,
        };

        fs::write(seed_dir.join("seed.toml"), toml::to_string(&seed).unwrap()).unwrap();
        seed_dir
    }

    fn write_manifest(root: &Path, seed_source: &Path) -> PathBuf {
        let manifest = root.join("system.toml");
        fs::write(
            &manifest,
            format!(
                r#"[system]
name = "phase3-cache-test"
target = "x86_64-unknown-linux-gnu"

[seed]
source = "{}"

[packages]
include = ["hello"]
"#,
                seed_source.display()
            ),
        )
        .unwrap();
        manifest
    }

    async fn generate_profile_fixture(
        project_root: &Path,
        profile_dir: &Path,
    ) -> (PathBuf, PathBuf, Vec<u8>) {
        let recipe_root = project_root.join("recipes");
        let (archive_path, contents, checksum) = write_archive(project_root, "hello");
        write_recipe(
            &recipe_root,
            "system/hello.toml",
            "hello",
            &archive_path,
            &checksum,
        );
        let seed_dir = write_seed_dir(project_root);
        let manifest = write_manifest(project_root, &seed_dir);

        fs::create_dir_all(profile_dir).unwrap();
        let profile_path = profile_dir.join("profile.toml");
        cmd_profile_generate(&manifest, Some(&profile_path))
            .await
            .unwrap();

        (profile_path, archive_path, contents)
    }

    #[tokio::test]
    async fn test_cache_populate_sources_only_downloads_recipe_archives() {
        let temp = tempfile::tempdir().unwrap();
        let (profile_path, archive_path, contents) =
            generate_profile_fixture(temp.path(), temp.path()).await;
        let db_path = temp.path().join("conary.db");

        cmd_cache_populate(
            profile_path.to_str().unwrap(),
            true,
            false,
            db_path.to_str().unwrap(),
        )
        .await
        .unwrap();

        let cached_source = temp
            .path()
            .join("sources")
            .join(archive_path.file_name().unwrap());
        assert_eq!(fs::read(cached_source).unwrap(), contents);
    }

    #[tokio::test]
    async fn test_cache_populate_full_downloads_sources_after_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let (profile_path, archive_path, contents) =
            generate_profile_fixture(temp.path(), temp.path()).await;
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(db_path.to_str().unwrap()).unwrap();

        cmd_cache_populate(
            profile_path.to_str().unwrap(),
            false,
            true,
            db_path.to_str().unwrap(),
        )
        .await
        .unwrap();

        let cached_source = temp
            .path()
            .join("sources")
            .join(archive_path.file_name().unwrap());
        assert_eq!(fs::read(cached_source).unwrap(), contents);
    }

    #[tokio::test]
    async fn test_cache_populate_sources_only_uses_profile_manifest_to_find_recipes() {
        let temp = tempfile::tempdir().unwrap();
        let project_root = temp.path().join("project");
        let profile_dir = temp.path().join("consumer");
        fs::create_dir_all(&project_root).unwrap();
        let (profile_path, archive_path, contents) =
            generate_profile_fixture(&project_root, &profile_dir).await;
        let db_path = temp.path().join("conary.db");

        cmd_cache_populate(
            profile_path.to_str().unwrap(),
            true,
            false,
            db_path.to_str().unwrap(),
        )
        .await
        .unwrap();

        let cached_source = temp
            .path()
            .join("sources")
            .join(archive_path.file_name().unwrap());
        assert_eq!(fs::read(cached_source).unwrap(), contents);
    }
}
