// src/commands/profile.rs

//! Build profile command handlers

use std::path::Path;

use anyhow::{Context, Result};
use conary_core::derivation::profile::BuildProfile;

/// Generate a build profile from a system manifest.
///
/// Profile generation requires loading all recipes referenced by the manifest,
/// computing derivation IDs, and assigning stages. This is a complex pipeline
/// that is not yet fully wired up, so this command prints an informational
/// message with the manifest path.
pub fn cmd_profile_generate(manifest: &Path, output: Option<&Path>) -> Result<()> {
    // Verify the manifest file exists and is readable.
    if !manifest.exists() {
        anyhow::bail!("Manifest not found: {}", manifest.display());
    }

    println!("Manifest: {}", manifest.display());
    if let Some(out) = output {
        println!("Output:   {}", out.display());
    }

    println!();
    println!("[TODO] Profile generation requires the full recipe loading pipeline.");
    println!("       Use 'conary profile show' to inspect an existing profile.");

    Ok(())
}

/// Display a build profile from a TOML file.
///
/// Loads the profile, recomputes its hash for verification, and prints a
/// human-readable summary including seed, stages, and derivation counts.
pub fn cmd_profile_show(path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read profile: {}", path.display()))?;

    let profile = BuildProfile::from_toml(&content)
        .with_context(|| format!("Failed to parse profile: {}", path.display()))?;

    let computed_hash = profile.compute_hash();

    println!("Profile: {}", profile.profile.manifest);
    println!("Target:  {}", profile.profile.target);
    println!("Hash:    {computed_hash}");
    if !profile.profile.generated_at.is_empty() {
        println!("Generated: {}", profile.profile.generated_at);
    }
    println!();

    println!(
        "Seed: {} (source: {})",
        profile.seed.id, profile.seed.source
    );
    println!();

    let total_derivations: usize = profile.stages.iter().map(|s| s.derivations.len()).sum();

    println!(
        "Stages: {}  Derivations: {}",
        profile.stages.len(),
        total_derivations
    );
    println!();

    for stage in &profile.stages {
        println!(
            "  [{}] env={} ({} derivations)",
            stage.name,
            stage.build_env,
            stage.derivations.len()
        );
        for drv in &stage.derivations {
            println!(
                "    {} v{} ({}...)",
                drv.package,
                drv.version,
                &drv.derivation_id[..12.min(drv.derivation_id.len())]
            );
        }
    }

    Ok(())
}

/// Compare two build profiles and display the diff.
///
/// Loads both profiles, computes their diff, and prints added, removed, and
/// changed packages.
pub fn cmd_profile_diff(old_path: &Path, new_path: &Path) -> Result<()> {
    let old_content = std::fs::read_to_string(old_path)
        .with_context(|| format!("Failed to read old profile: {}", old_path.display()))?;
    let new_content = std::fs::read_to_string(new_path)
        .with_context(|| format!("Failed to read new profile: {}", new_path.display()))?;

    let old_profile = BuildProfile::from_toml(&old_content)
        .with_context(|| format!("Failed to parse old profile: {}", old_path.display()))?;
    let new_profile = BuildProfile::from_toml(&new_content)
        .with_context(|| format!("Failed to parse new profile: {}", new_path.display()))?;

    let old_hash = old_profile.compute_hash();
    let new_hash = new_profile.compute_hash();

    if old_hash == new_hash {
        println!("Profiles are identical (hash: {old_hash})");
        return Ok(());
    }

    println!("Old: {} (hash: {})", old_profile.profile.manifest, old_hash);
    println!("New: {} (hash: {})", new_profile.profile.manifest, new_hash);
    println!();

    let diff = old_profile.diff(&new_profile);

    if !diff.added.is_empty() {
        println!("Added ({}):", diff.added.len());
        for pkg in &diff.added {
            println!("  + {pkg}");
        }
    }

    if !diff.removed.is_empty() {
        println!("Removed ({}):", diff.removed.len());
        for pkg in &diff.removed {
            println!("  - {pkg}");
        }
    }

    if !diff.changed.is_empty() {
        println!("Changed ({}):", diff.changed.len());
        for pkg in &diff.changed {
            println!("  ~ {pkg}");
        }
    }

    if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
        println!("No package changes (metadata-only difference).");
    }

    Ok(())
}

/// Publish a profile to a remote Remi endpoint.
pub fn cmd_profile_publish(
    profile_path: &str,
    endpoint: Option<&str>,
    token: Option<&str>,
) -> Result<()> {
    let content =
        std::fs::read(profile_path).map_err(|e| anyhow::anyhow!("failed to read profile: {e}"))?;

    let hash = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(&content))
    };

    let endpoint =
        endpoint.ok_or_else(|| anyhow::anyhow!("--endpoint is required for profile publish"))?;
    let token = token.ok_or_else(|| anyhow::anyhow!("--token is required for profile publish"))?;

    let client = reqwest::blocking::Client::new();
    let resp = client
        .put(format!("{endpoint}/v1/profiles/{hash}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/toml")
        .body(content)
        .send()
        .map_err(|e| anyhow::anyhow!("HTTP error: {e}"))?;

    if resp.status().is_success() {
        println!("Published profile to {endpoint}/v1/profiles/{hash}");
    } else {
        anyhow::bail!(
            "Server returned {}: {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
    }

    Ok(())
}
