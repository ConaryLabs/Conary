// src/commands/distro.rs
//! Distro pinning command implementations

use super::open_db;
use anyhow::Result;
use conary_core::db::models::{DistroPin, SystemAffinity};
use conary_core::model::parser::SourcePinConfig;

const VALID_MIXING_POLICIES: [&str; 3] = ["strict", "guarded", "permissive"];

fn validate_mixing_policy(policy: &str) -> Result<()> {
    if !VALID_MIXING_POLICIES.contains(&policy) {
        anyhow::bail!("Invalid mixing policy: {policy}. Use strict, guarded, or permissive.");
    }
    Ok(())
}

pub async fn cmd_distro_set(db_path: &str, distro: &str, mixing: &str) -> Result<()> {
    validate_mixing_policy(mixing)?;
    let conn = open_db(db_path)?;
    DistroPin::set_from_source_pin(
        &conn,
        &SourcePinConfig {
            distro: distro.to_string(),
            strength: Some(mixing.to_string()),
        },
    )?;
    println!("Pinned to {distro} (mixing: {mixing})");
    Ok(())
}

pub async fn cmd_distro_remove(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    DistroPin::remove(&conn)?;
    println!("Distro pin removed. System is now distro-agnostic.");
    Ok(())
}

pub async fn cmd_distro_info(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
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

// TODO: Drive this list from the database or registry instead of hardcoding
pub async fn cmd_distro_list() -> Result<()> {
    println!("Available distros:");
    println!("  ubuntu-noble     Ubuntu 24.04 LTS (Noble Numbat)");
    println!("  ubuntu-oracular  Ubuntu 24.10 (Oracular Oriole)");
    println!("  fedora-43        Fedora 43");
    println!("  debian-12        Debian 12 (Bookworm)");
    println!("  arch             Arch Linux (rolling)");
    Ok(())
}

pub async fn cmd_distro_mixing(db_path: &str, policy: &str) -> Result<()> {
    validate_mixing_policy(policy)?;
    let conn = open_db(db_path)?;
    if DistroPin::get_current(&conn)?.is_none() {
        anyhow::bail!(
            "No distro pin set. Use 'conary distro set <distro>' before changing mixing policy."
        );
    }
    DistroPin::set_mixing_policy(&conn, policy)?;
    println!("Mixing policy changed to {policy}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::schema;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, String, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().display().to_string();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, db_path, conn)
    }

    #[tokio::test]
    async fn test_cmd_distro_set_persists_compatibility_pin() {
        let (_temp, db_path, conn) = create_test_db();

        cmd_distro_set(&db_path, "arch", "strict").await.unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        let source_pin = pin.as_source_pin();
        assert_eq!(source_pin.distro, "arch");
        assert_eq!(source_pin.strength.as_deref(), Some("strict"));
    }

    #[tokio::test]
    async fn test_cmd_distro_remove_clears_pin() {
        let (_temp, db_path, conn) = create_test_db();
        DistroPin::set(&conn, "fedora-43", "guarded").unwrap();

        cmd_distro_remove(&db_path).await.unwrap();

        assert!(DistroPin::get_current(&conn).unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cmd_distro_mixing_requires_existing_pin() {
        let (_temp, db_path, _conn) = create_test_db();

        let err = cmd_distro_mixing(&db_path, "strict").await.unwrap_err();

        assert!(err.to_string().contains("No distro pin set"));
    }
}
