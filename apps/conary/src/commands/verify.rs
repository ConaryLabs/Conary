// src/commands/verify.rs

//! Verification command handlers.

use anyhow::Result;
use conary_core::derivation::index::{self, DerivationIndex};
use conary_core::derivation::profile::BuildProfile;

/// Trace all derivations in a profile back to the seed.
///
/// Walks every stage/derivation in the profile, looks each up in the local
/// derivation index, and reports trust levels, provenance status, and an
/// overall chain verdict (COMPLETE or BROKEN).
pub async fn cmd_verify_chain(
    profile_path: &str,
    verbose: bool,
    _json: bool,
    db_path: &str,
) -> Result<()> {
    let content = std::fs::read_to_string(profile_path)?;
    let profile: BuildProfile = toml::from_str(&content)?;

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
                let package = format!("{}-{}", drv.package, drv.version);
                crate::ui::row(crate::ui::Status::Pending, &[&package]);
                continue;
            }

            match index.lookup(&drv.derivation_id) {
                Ok(Some(record)) => {
                    found += 1;
                    let level = record.trust_level.min(4) as usize;
                    trust_counts[level] += 1;

                    println!(
                        "  {}-{}    [level {}: {}]",
                        drv.package,
                        drv.version,
                        record.trust_level,
                        index::trust_level_name(record.trust_level)
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
        crate::ui::warn(w);
    }

    if chain_broken {
        std::process::exit(1);
    }

    Ok(())
}

/// Compare builds from two different seeds for diverse verification.
pub async fn cmd_verify_diverse(
    profile_a_path: &str,
    profile_b_path: &str,
    db_path: &str,
) -> Result<()> {
    let a_content = std::fs::read_to_string(profile_a_path)?;
    let b_content = std::fs::read_to_string(profile_b_path)?;
    let profile_a: BuildProfile = toml::from_str(&a_content)?;
    let profile_b: BuildProfile = toml::from_str(&b_content)?;

    // Verify different seeds
    if profile_a.seed.id == profile_b.seed.id {
        let display_len = 16.min(profile_a.seed.id.len());
        anyhow::bail!(
            "both profiles use the same seed ({}...). Diverse verification requires different seeds.",
            &profile_a.seed.id[..display_len]
        );
    }

    let a_seed_display = 16.min(profile_a.seed.id.len());
    let b_seed_display = 16.min(profile_b.seed.id.len());
    println!("Comparing builds from 2 seeds:");
    println!(
        "  Seed A: {}... ({})",
        &profile_a.seed.id[..a_seed_display],
        profile_a.seed.source
    );
    println!(
        "  Seed B: {}... ({})",
        &profile_b.seed.id[..b_seed_display],
        profile_b.seed.source
    );
    println!();

    let conn = super::open_db(db_path)?;
    let index = DerivationIndex::new(&conn);

    // Build lookup map: (package_name, version) -> derivation_id from profile A
    let a_map: std::collections::HashMap<(String, String), String> = profile_a
        .stages
        .iter()
        .flat_map(|s| s.derivations.iter())
        .filter(|d| d.derivation_id != "pending")
        .map(|d| {
            (
                (d.package.clone(), d.version.clone()),
                d.derivation_id.clone(),
            )
        })
        .collect();

    let mut matches = 0usize;
    let mut mismatches = 0usize;
    let mut unmatched = 0usize;

    for stage in &profile_b.stages {
        for drv in &stage.derivations {
            if drv.derivation_id == "pending" {
                continue;
            }

            let key = (drv.package.clone(), drv.version.clone());
            let Some(a_id) = a_map.get(&key) else {
                unmatched += 1;
                continue;
            };

            // Load both records
            let a_record = index
                .lookup(a_id)?
                .ok_or_else(|| anyhow::anyhow!("missing record for {a_id}"))?;
            let b_record = index
                .lookup(&drv.derivation_id)?
                .ok_or_else(|| anyhow::anyhow!("missing record for {}", drv.derivation_id))?;

            if a_record.output_hash == b_record.output_hash {
                matches += 1;
                println!(
                    "  {}-{}:  MATCH (diverse-verified)",
                    drv.package, drv.version
                );
                index.set_trust_level(a_id, 4)?;
                index.set_trust_level(&drv.derivation_id, 4)?;
            } else {
                mismatches += 1;
                println!("  {}-{}:  MISMATCH", drv.package, drv.version);
            }
        }
    }

    println!();
    let total = matches + mismatches;
    println!("  {matches}/{total} packages diverse-verified");
    if mismatches > 0 {
        println!("  {mismatches} packages with environment-dependent differences");
    }
    if unmatched > 0 {
        println!("  {unmatched} packages only in one profile (skipped)");
    }

    Ok(())
}
