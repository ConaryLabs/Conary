// src/commands/distro.rs
//! Distro pinning command implementations

use anyhow::Result;
use conary_core::db::models::{DistroPin, SystemAffinity};

pub fn cmd_distro_set(db_path: &str, distro: &str, mixing: &str) -> Result<()> {
    if !["strict", "guarded", "permissive"].contains(&mixing) {
        anyhow::bail!("Invalid mixing policy: {mixing}. Use strict, guarded, or permissive.");
    }
    let conn = conary_core::db::open(db_path)?;
    DistroPin::set(&conn, distro, mixing)?;
    println!("Pinned to {distro} (mixing: {mixing})");
    Ok(())
}

pub fn cmd_distro_remove(db_path: &str) -> Result<()> {
    let conn = conary_core::db::open(db_path)?;
    DistroPin::remove(&conn)?;
    println!("Distro pin removed. System is now distro-agnostic.");
    Ok(())
}

pub fn cmd_distro_info(db_path: &str) -> Result<()> {
    let conn = conary_core::db::open(db_path)?;
    match DistroPin::get_current(&conn)? {
        Some(pin) => {
            println!("Distro: {}", pin.distro);
            println!("Mixing: {}", pin.mixing_policy);
            println!();
            println!("Source affinity:");
            let affinities = SystemAffinity::list(&conn)?;
            if affinities.is_empty() {
                println!("  (no data yet -- run a sync first)");
            } else {
                for a in &affinities {
                    println!(
                        "  {}: {} packages ({:.1}%)",
                        a.distro, a.package_count, a.percentage
                    );
                }
            }
        }
        None => {
            println!("No distro pin set. System is distro-agnostic.");
        }
    }
    Ok(())
}

pub fn cmd_distro_list() -> Result<()> {
    println!("Available distros:");
    println!("  ubuntu-noble     Ubuntu 24.04 LTS (Noble Numbat)");
    println!("  ubuntu-oracular  Ubuntu 24.10 (Oracular Oriole)");
    println!("  fedora-41        Fedora 41");
    println!("  fedora-42        Fedora 42");
    println!("  debian-12        Debian 12 (Bookworm)");
    println!("  arch             Arch Linux (rolling)");
    Ok(())
}

pub fn cmd_distro_mixing(db_path: &str, policy: &str) -> Result<()> {
    if !["strict", "guarded", "permissive"].contains(&policy) {
        anyhow::bail!("Invalid mixing policy: {policy}. Use strict, guarded, or permissive.");
    }
    let conn = conary_core::db::open(db_path)?;
    DistroPin::set_mixing_policy(&conn, policy)?;
    println!("Mixing policy changed to {policy}");
    Ok(())
}
