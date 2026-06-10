// apps/conary/src/commands/bootstrap/convergence.rs

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::run_record::load_completed_bootstrap_run_record;

/// Verify convergence between builds from two different seeds
pub async fn cmd_bootstrap_verify_convergence(
    run_a: &str,
    run_b: &str,
    seed_a: Option<&str>,
    seed_b: Option<&str>,
    diff: bool,
) -> Result<()> {
    use conary_core::derivation::{Seed, compare_build_sets, load_build_set};

    let record_a = load_completed_bootstrap_run_record(Path::new(run_a))?;
    let record_b = load_completed_bootstrap_run_record(Path::new(run_b))?;

    if let Some(seed_path) = seed_a {
        let seed = Seed::load_local(Path::new(seed_path))
            .map_err(|error| anyhow::anyhow!("Failed to load seed A: {error}"))?;
        if seed.build_env_hash() != record_a.seed_id {
            anyhow::bail!(
                "Seed A hash {} does not match run A seed {}",
                seed.build_env_hash(),
                record_a.seed_id
            );
        }
    }
    if let Some(seed_path) = seed_b {
        let seed = Seed::load_local(Path::new(seed_path))
            .map_err(|error| anyhow::anyhow!("Failed to load seed B: {error}"))?;
        if seed.build_env_hash() != record_b.seed_id {
            anyhow::bail!(
                "Seed B hash {} does not match run B seed {}",
                seed.build_env_hash(),
                record_b.seed_id
            );
        }
    }

    let conn_a = Connection::open(&record_a.derivation_db_path)
        .with_context(|| format!("Failed to open {}", record_a.derivation_db_path.display()))?;
    let conn_b = Connection::open(&record_b.derivation_db_path)
        .with_context(|| format!("Failed to open {}", record_b.derivation_db_path.display()))?;

    let builds_a = load_build_set(&conn_a, &record_a.seed_id)
        .map_err(|error| anyhow::anyhow!("Failed to load build set A: {error}"))?;
    let builds_b = load_build_set(&conn_b, &record_b.seed_id)
        .map_err(|error| anyhow::anyhow!("Failed to load build set B: {error}"))?;
    let report = compare_build_sets(builds_a, builds_b);

    if report.total() == 0 {
        anyhow::bail!(
            "No comparable packages found between {} and {}",
            run_a,
            run_b
        );
    }

    println!("Compared {} packages", report.total());
    println!("Matched: {}", report.matched());
    println!("Mismatched: {}", report.mismatched());
    println!("Skipped: {}", report.skipped_total());
    println!("Only in run A: {}", report.only_in_a().len());
    println!("Only in run B: {}", report.only_in_b().len());

    if diff {
        if !report.mismatches().is_empty() {
            println!("\nMismatched packages:");
            for mismatch in report.mismatches() {
                println!(
                    "  {}: {} != {}",
                    mismatch.package, mismatch.hash_a, mismatch.hash_b
                );
            }
        }
        if !report.only_in_a().is_empty() {
            println!("\nOnly in run A:");
            for package in report.only_in_a() {
                println!("  {package}");
            }
        }
        if !report.only_in_b().is_empty() {
            println!("\nOnly in run B:");
            for package in report.only_in_b() {
                println!("  {package}");
            }
        }
    }

    if report.mismatched() > 0 {
        anyhow::bail!(
            "Convergence verification failed with {} mismatched packages",
            report.mismatched()
        );
    }

    println!("[COMPLETE] All compared packages converged.");
    Ok(())
}

/// Diff two seed EROFS images
pub async fn cmd_bootstrap_diff_seeds(path_a: &str, path_b: &str) -> Result<()> {
    let report = conary_core::derivation::diff_seed_dirs(Path::new(path_a), Path::new(path_b))
        .map_err(|error| anyhow::anyhow!("Failed to diff seeds: {error}"))?;

    println!("Seed A: {path_a}");
    println!("Seed B: {path_b}");

    match (&report.erofs_hash_a, &report.erofs_hash_b) {
        (Some(hash_a), Some(hash_b)) => {
            println!("EROFS hash A: {hash_a}");
            println!("EROFS hash B: {hash_b}");
        }
        (Some(hash_a), None) => {
            println!("EROFS hash A: {hash_a}");
            println!("EROFS hash B: <missing>");
        }
        (None, Some(hash_b)) => {
            println!("EROFS hash A: <missing>");
            println!("EROFS hash B: {hash_b}");
        }
        (None, None) => {
            println!("EROFS hash A: <missing>");
            println!("EROFS hash B: <missing>");
        }
    }

    if report.metadata_differences.is_empty()
        && report.artifact_differences.is_empty()
        && report.erofs_hash_a == report.erofs_hash_b
    {
        println!("[COMPLETE] No metadata, artifact, or hash differences found.");
        return Ok(());
    }

    if !report.metadata_differences.is_empty() {
        println!("\nMetadata differences:");
        for line in &report.metadata_differences {
            println!("  {line}");
        }
    }

    if !report.artifact_differences.is_empty() {
        println!("\nArtifact differences:");
        for line in &report.artifact_differences {
            println!("  {line}");
        }
    }

    Ok(())
}
