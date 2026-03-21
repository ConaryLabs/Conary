// src/commands/verify.rs

//! Verification command handlers.

use anyhow::Result;
use conary_core::derivation::executor::ExecutorConfig;
use conary_core::derivation::index::DerivationIndex;
use conary_core::derivation::profile::BuildProfile;
use conary_core::derivation::{DerivationExecutor, ExecutionResult};
use conary_core::filesystem::CasStore;

/// Trace all derivations in a profile back to the seed.
///
/// Walks every stage/derivation in the profile, looks each up in the local
/// derivation index, and reports trust levels, provenance status, and an
/// overall chain verdict (COMPLETE or BROKEN).
pub fn cmd_verify_chain(profile_path: &str, verbose: bool, _json: bool) -> Result<()> {
    let content = std::fs::read_to_string(profile_path)?;
    let profile: BuildProfile = toml::from_str(&content)?;

    let db_path = "/var/lib/conary/conary.db";
    let conn = super::open_db(db_path)?;
    let index = DerivationIndex::new(&conn);

    let mut total = 0usize;
    let mut found = 0usize;
    let mut trust_counts = [0usize; 5]; // levels 0-4
    let mut warnings = Vec::new();
    let mut chain_broken = false;

    println!("Seed: {} ({})", profile.seed.id, profile.seed.source);
    println!();

    for stage in &profile.stages {
        println!(
            "Stage: {} ({} packages)",
            stage.name,
            stage.derivations.len()
        );

        for drv in &stage.derivations {
            total += 1;
            if drv.derivation_id == "pending" {
                println!("  {}-{}    [pending]", drv.package, drv.version);
                continue;
            }

            match index.lookup(&drv.derivation_id) {
                Ok(Some(record)) => {
                    found += 1;
                    let level = record.trust_level.min(4) as usize;
                    trust_counts[level] += 1;

                    let trust_name = match record.trust_level {
                        0 => "unverified",
                        1 => "substituted",
                        2 => "locally built",
                        3 => "independently verified",
                        4 => "diverse-verified",
                        _ => "unknown",
                    };

                    println!(
                        "  {}-{}    [level {}: {}]",
                        drv.package, drv.version, record.trust_level, trust_name
                    );

                    if verbose {
                        if let Some(ref prov_hash) = record.provenance_cas_hash {
                            println!("    provenance: {prov_hash}");
                        }
                        let display_len = 16.min(record.output_hash.len());
                        println!("    output: {}", &record.output_hash[..display_len]);
                    }

                    if record.provenance_cas_hash.is_none() {
                        warnings.push(format!("{}: missing provenance", drv.package));
                    }
                }
                Ok(None) => {
                    chain_broken = true;
                    println!(
                        "  {}-{}    [MISSING from local index]",
                        drv.package, drv.version
                    );
                }
                Err(e) => {
                    chain_broken = true;
                    println!("  {}-{}    [ERROR: {}]", drv.package, drv.version, e);
                }
            }
        }
        println!();
    }

    // Summary
    let status = if chain_broken { "BROKEN" } else { "COMPLETE" };
    println!("Chain: {status}");
    let seed_display_len = 16.min(profile.seed.id.len());
    println!(
        "  {found}/{total} derivations traced to seed {}",
        &profile.seed.id[..seed_display_len]
    );

    let above_2: usize = trust_counts[2..].iter().sum();
    println!("  {above_2}/{total} at trust level >= 2");

    for w in &warnings {
        println!("  [WARN] {w}");
    }

    if chain_broken {
        std::process::exit(1);
    }

    Ok(())
}

/// Rebuild a derivation and compare output hash against the original.
pub fn cmd_verify_rebuild(derivation: &str, work_dir: &str) -> Result<()> {
    let db_path = "/var/lib/conary/conary.db";
    let conn = super::open_db(db_path)?;
    let index = DerivationIndex::new(&conn);

    // Resolve derivation ID (could be a package name)
    let record = if derivation.len() == 64 && derivation.chars().all(|c| c.is_ascii_hexdigit()) {
        index
            .lookup(derivation)?
            .ok_or_else(|| anyhow::anyhow!("derivation {derivation} not found"))?
    } else {
        // Treat as package name
        let records = index.by_package(derivation)?;
        records
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no derivation found for package '{derivation}'"))?
    };

    println!(
        "Rebuilding {}-{} (derivation {}...)",
        record.package_name,
        record.package_version,
        &record.derivation_id[..16.min(record.derivation_id.len())]
    );

    // Resolve recipe from recipes/ directory
    let recipe_path = find_recipe(&record.package_name)?;
    let recipe = conary_core::recipe::parse_recipe_file(&recipe_path)?;

    // Create fresh in-memory DB for the rebuild (bypasses cache)
    let rebuild_conn = rusqlite::Connection::open_in_memory()?;
    conary_core::db::schema::migrate(&rebuild_conn)?;

    // Set up executor with fresh DB
    let cas_dir = std::path::PathBuf::from(work_dir).join("cas");
    std::fs::create_dir_all(&cas_dir)?;
    let cas = CasStore::new(&cas_dir)?;
    let exec_config = ExecutorConfig::default();
    let executor = DerivationExecutor::new(cas, cas_dir.clone(), exec_config);

    let build_env_hash = record.build_env_hash.as_deref().unwrap_or("unknown");
    let sysroot = std::path::PathBuf::from(work_dir).join("sysroot");
    std::fs::create_dir_all(&sysroot)?;

    let dep_ids = std::collections::BTreeMap::new(); // simplified for now
    let target = "x86_64-unknown-linux-gnu";

    match executor.execute(
        &recipe,
        build_env_hash,
        &dep_ids,
        target,
        &sysroot,
        &rebuild_conn,
    ) {
        Ok(ExecutionResult::Built { output, .. }) => {
            let new_hash = &output.manifest.output_hash;
            let original_hash = &record.output_hash;

            if new_hash == original_hash {
                let display_len = 16.min(original_hash.len());
                println!("  Original output: {}...", &original_hash[..display_len]);
                println!("  Rebuild output:  {}...  MATCH", &new_hash[..display_len]);
                println!();
                index.set_trust_level(&record.derivation_id, 3)?;
                index.set_reproducible(&record.derivation_id, true)?;
                println!(
                    "  Trust level: {} -> 3 (independently verified)",
                    record.trust_level
                );
                println!("  Reproducible: true");
            } else {
                let orig_display = 16.min(original_hash.len());
                let new_display = 16.min(new_hash.len());
                println!("  Original output: {}...", &original_hash[..orig_display]);
                println!(
                    "  Rebuild output:  {}...  MISMATCH",
                    &new_hash[..new_display]
                );
                println!();
                index.set_reproducible(&record.derivation_id, false)?;
                println!("  Reproducible: false");
            }
        }
        Ok(ExecutionResult::CacheHit { .. }) => {
            anyhow::bail!("unexpected cache hit on fresh DB -- this should not happen");
        }
        Err(e) => {
            println!("  Rebuild failed: {e}");
            println!("  Cannot verify reproducibility.");
        }
    }

    Ok(())
}

/// Find a recipe file by package name in the recipes/ directory.
fn find_recipe(package_name: &str) -> Result<std::path::PathBuf> {
    for dir in &[
        "recipes/system",
        "recipes/cross-tools",
        "recipes/tier2",
        "recipes",
    ] {
        let path = std::path::PathBuf::from(dir).join(format!("{package_name}.toml"));
        if path.exists() {
            return Ok(path);
        }
    }
    anyhow::bail!("recipe for '{package_name}' not found in recipes/ directory")
}
