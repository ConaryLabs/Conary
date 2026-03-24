// src/commands/cache.rs

//! Implementation of `conary cache` commands.

use anyhow::Result;

/// Pre-fetch derivation outputs from configured substituters.
pub async fn cmd_cache_populate(profile_path: &str, sources_only: bool, full: bool, db_path: &str) -> Result<()> {
    // Read profile TOML
    let content = std::fs::read_to_string(profile_path)
        .map_err(|e| anyhow::anyhow!("failed to read profile: {e}"))?;

    let profile: conary_core::derivation::BuildProfile =
        toml::from_str(&content).map_err(|e| anyhow::anyhow!("failed to parse profile: {e}"))?;

    // Collect all derivation IDs from all stages
    let mut derivation_ids: Vec<String> = Vec::new();
    for stage in &profile.stages {
        for drv in &stage.derivations {
            if drv.derivation_id != "pending" {
                derivation_ids.push(drv.derivation_id.clone());
            }
        }
    }

    let total = derivation_ids.len();
    println!(
        "Profile has {total} derivations across {} stages.",
        profile.stages.len()
    );

    if sources_only {
        println!("--sources-only: source tarball download requires local recipe files.");
        println!("Source download not yet implemented.");
        return Ok(());
    }

    // Load substituter configuration from DB
    let conn = super::open_db(db_path)?;

    let mut substituter =
        match conary_core::derivation::substituter::DerivationSubstituter::from_db(&conn) {
            Ok(s) => s,
            Err(e) => {
                anyhow::bail!(
                    "No substituter peers configured: {e}. Add peers with substituter config first."
                );
            }
        };

    // Batch probe to find available derivations
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

    // Fetch each available derivation
    let cas_dir = conary_core::db::paths::objects_dir(db_path);
    let cas = conary_core::filesystem::CasStore::new(&cas_dir)
        .map_err(|e| anyhow::anyhow!("failed to open CAS: {e}"))?;

    let mut fetched = 0u64;
    let mut total_bytes = 0u64;

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
                        fetched += 1;
                        total_bytes += report.bytes_transferred;
                    }
                    Err(e) => {
                        eprintln!("\n  [WARN] Failed to fetch objects for {short_id}: {e}");
                    }
                }
            }
            conary_core::derivation::substituter::CacheQueryResult::Miss => {}
        }
    }

    println!(
        "\n\nDownloaded {fetched}/{} derivation outputs ({:.1} MB)",
        available.len(),
        total_bytes as f64 / 1_048_576.0,
    );
    if unavailable > 0 {
        println!("{unavailable} derivations will be built from source.");
    }

    if full {
        println!("\n--full: source tarball download not yet implemented.");
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

        let mut stmt = conn
            .prepare(
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
