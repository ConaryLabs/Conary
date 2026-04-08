// src/commands/distro.rs
//! Distro pinning command implementations

use super::open_db;
use anyhow::Result;
use conary_core::db::models::{DistroPin, SystemAffinity, settings};
use conary_core::model::parser::SourcePinConfig;
use conary_core::repository::resolution_policy::{RequestScope, SelectionMode};
use conary_core::repository::{SETTINGS_KEY_SELECTION_MODE, load_effective_policy};
use rusqlite::Connection;

const VALID_MIXING_POLICIES: [&str; 3] = ["strict", "guarded", "permissive"];
const VALID_SELECTION_MODES: [&str; 2] = ["policy", "latest"];

fn validate_mixing_policy(policy: &str) -> Result<()> {
    if !VALID_MIXING_POLICIES.contains(&policy) {
        anyhow::bail!("Invalid mixing policy: {policy}. Use strict, guarded, or permissive.");
    }
    Ok(())
}

fn validate_selection_mode(mode: &str) -> Result<()> {
    if !VALID_SELECTION_MODES.contains(&mode) {
        anyhow::bail!("Invalid selection mode: {mode}. Use policy or latest.");
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
    print!("{}", render_distro_info(&conn)?);
    Ok(())
}

pub fn render_distro_info(conn: &Connection) -> Result<String> {
    let effective_policy = load_effective_policy(conn, RequestScope::Any)?;
    let configured_selection_mode = settings::get(conn, SETTINGS_KEY_SELECTION_MODE)?;
    let selection_mode = match configured_selection_mode {
        Some(mode) => format!("Selection mode: {mode}"),
        None => format!(
            "Selection mode: {} (runtime default)",
            match effective_policy.resolution.selection_mode {
                SelectionMode::Policy => "policy",
                SelectionMode::Latest => "latest",
            }
        ),
    };

    let mut output = String::new();
    match DistroPin::get_current(conn)? {
        Some(pin) => {
            output.push_str(&format!("Distro: {}\n", pin.distro));
            output.push_str(&format!("Mixing: {}\n", pin.mixing_policy));
            output.push_str(&format!("{selection_mode}\n\n"));
            output.push_str("Source affinity:\n");
            let affinities = SystemAffinity::list(&conn)?;
            if affinities.is_empty() {
                output.push_str("  (no data yet -- run a sync first)\n");
            } else {
                for a in &affinities {
                    output.push_str(&format!(
                        "  {}: {} packages ({:.1}%)\n",
                        a.distro, a.package_count, a.percentage
                    ));
                }
            }
        }
        None => {
            output.push_str("No distro pin set. System is distro-agnostic.\n");
            output.push_str(&format!("{selection_mode}\n"));
        }
    }
    Ok(output)
}

// TODO: Drive this list from the database or registry instead of hardcoding.
// This static list must be kept in sync with supported distros manually.
pub async fn cmd_distro_list() -> Result<()> {
    println!("Available distros:");
    println!("  ubuntu-noble     Ubuntu 24.04 LTS (Noble Numbat)");
    println!("  ubuntu-plucky    Ubuntu 25.04 (Plucky Puffin)");
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

pub async fn cmd_distro_selection_mode(db_path: &str, mode: &str) -> Result<()> {
    validate_selection_mode(mode)?;
    let conn = open_db(db_path)?;
    settings::set(&conn, SETTINGS_KEY_SELECTION_MODE, mode)?;
    println!("Selection mode changed to {mode}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::settings;
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

    #[tokio::test]
    async fn test_cmd_distro_selection_mode_persists_latest() {
        let (_temp, db_path, conn) = create_test_db();

        cmd_distro_selection_mode(&db_path, "latest").await.unwrap();

        assert_eq!(
            settings::get(&conn, "source.selection-mode")
                .unwrap()
                .as_deref(),
            Some("latest")
        );
    }

    #[tokio::test]
    async fn test_cmd_distro_info_includes_effective_selection_mode() {
        let (_temp, _db_path, conn) = create_test_db();
        settings::set(&conn, "source.selection-mode", "latest").unwrap();

        let rendered = render_distro_info(&conn).unwrap();
        assert!(rendered.contains("Selection mode: latest"));
    }
}
