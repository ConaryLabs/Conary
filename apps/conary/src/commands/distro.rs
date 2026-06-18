// src/commands/distro.rs
//! Distro pinning command implementations

use super::open_db;
use anyhow::{Context, Result};
use conary_core::db::models::{DistroPin, Repository, SystemAffinity, settings};
use conary_core::model::parser::SourcePinConfig;
use conary_core::repository::distro::supported_user_distros;
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
    let profile = conary_core::repository::supported_profiles::profile_by_public_id(distro)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Unsupported distro: {distro}. Use 'conary distro list' to see supported targets."
            )
        })?;
    let conn = open_db(db_path)?;
    DistroPin::set_from_source_pin(
        &conn,
        &SourcePinConfig {
            distro: profile.id().to_string(),
            strength: Some(mixing.to_string()),
        },
    )?;
    println!("Pinned to {} (mixing: {mixing})", profile.id());
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
            let affinities = SystemAffinity::list(conn)?;
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

pub async fn cmd_distro_list(db_path: &str) -> Result<()> {
    match conary_core::db::open(db_path) {
        Ok(conn) => print!("{}", render_distro_list(&conn)?),
        Err(conary_core::Error::DatabaseNotFound(_)) => {
            print!("{}", render_distro_list_for_repos(&[]));
        }
        Err(error) => Err(error).context("Failed to open package database")?,
    }
    Ok(())
}

pub fn render_distro_list(conn: &Connection) -> Result<String> {
    let repos = Repository::list_all(conn)?;
    Ok(render_distro_list_for_repos(&repos))
}

fn render_distro_list_for_repos(repos: &[Repository]) -> String {
    let mut output = String::from("Available distros:\n");

    for distro in supported_user_distros() {
        let matching_repos: Vec<_> = repos
            .iter()
            .filter(|repo| {
                repo.name == distro.id
                    || repo.default_strategy_distro.as_deref() == Some(distro.id.as_str())
            })
            .collect();
        let enabled_count = matching_repos.iter().filter(|repo| repo.enabled).count();
        let status = match (matching_repos.len(), enabled_count) {
            (0, _) => "not configured".to_string(),
            (total, 0) => format!("configured/disabled ({total} repo{})", plural(total)),
            (total, enabled) if total == enabled => {
                format!("configured/enabled ({enabled} repo{})", plural(enabled))
            }
            (total, enabled) => format!(
                "configured/enabled ({enabled}/{total} repo{} enabled)",
                plural(total)
            ),
        };
        output.push_str(&format!(
            "  {:<15} {:<24} {}\n",
            distro.id, distro.display_name, status
        ));
    }

    output
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
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
    async fn test_cmd_distro_set_persists_supported_public_pin() {
        let (_temp, db_path, conn) = create_test_db();

        cmd_distro_set(&db_path, "arch", "strict").await.unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        let source_pin = pin.as_source_pin();
        assert_eq!(source_pin.distro, "arch");
        assert_eq!(source_pin.strength.as_deref(), Some("strict"));
    }

    #[tokio::test]
    async fn test_cmd_distro_set_rejects_unsupported_public_id() {
        let (_temp, db_path, conn) = create_test_db();

        let err = cmd_distro_set(&db_path, "debian-13", "strict")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Unsupported distro"));
        assert!(DistroPin::get_current(&conn).unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cmd_distro_set_rejects_internal_only_route_slug() {
        let (_temp, db_path, conn) = create_test_db();

        let err = cmd_distro_set(&db_path, "fedora", "strict")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Unsupported distro"));
        assert!(DistroPin::get_current(&conn).unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cmd_distro_remove_clears_pin() {
        let (_temp, db_path, conn) = create_test_db();
        DistroPin::set(&conn, "fedora-44", "guarded").unwrap();

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

    #[test]
    fn test_render_distro_list_uses_supported_catalog() {
        let (_temp, _db_path, conn) = create_test_db();

        let rendered = render_distro_list(&conn).unwrap();

        assert!(rendered.contains("fedora-44"));
        assert!(rendered.contains("Fedora 44"));
        assert!(rendered.contains("ubuntu-26.04"));
        assert!(rendered.contains("Ubuntu 26.04 LTS"));
        assert!(rendered.contains("arch"));
        assert!(!rendered.contains("linux-mint"));
        assert!(!rendered.contains("Debian"));
    }

    #[test]
    fn test_render_distro_list_marks_exact_supported_repos() {
        let (_temp, _db_path, conn) = create_test_db();
        let mut fedora = Repository::new(
            "fedora-main".to_string(),
            "https://example.com/fedora".to_string(),
        );
        fedora.default_strategy_distro = Some("fedora-44".to_string());
        fedora.insert(&conn).unwrap();

        let mut arch = Repository::new("arch".to_string(), "https://example.com/arch".to_string());
        arch.enabled = false;
        arch.insert(&conn).unwrap();

        let rendered = render_distro_list(&conn).unwrap();

        assert!(rendered.contains("fedora-44"));
        assert!(rendered.contains("configured/enabled (1 repo)"));
        assert!(rendered.contains("arch"));
        assert!(rendered.contains("configured/disabled (1 repo)"));
    }

    #[test]
    fn test_render_distro_list_does_not_infer_from_parser_families() {
        let (_temp, _db_path, conn) = create_test_db();
        let mut debian = Repository::new(
            "debian-bookworm".to_string(),
            "https://deb.debian.org/debian".to_string(),
        );
        debian.default_strategy_distro = Some("debian".to_string());
        debian.insert(&conn).unwrap();

        let mut mint = Repository::new(
            "linux-mint".to_string(),
            "https://packages.linuxmint.com".to_string(),
        );
        mint.insert(&conn).unwrap();

        let rendered = render_distro_list(&conn).unwrap();

        let ubuntu_line = rendered
            .lines()
            .find(|line| line.contains("ubuntu-26.04"))
            .unwrap();
        assert!(ubuntu_line.contains("not configured"));
    }
}
