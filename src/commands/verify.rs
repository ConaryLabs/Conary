// src/commands/verify.rs

//! Verification command handlers.

use anyhow::Result;
use conary_core::derivation::index::DerivationIndex;
use conary_core::derivation::profile::BuildProfile;

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
