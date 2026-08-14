// src/commands/distro.rs
//! Named feed and installed source-affinity diagnostics.

use super::open_db;
use anyhow::{Context, Result};
use conary_core::db::models::{Repository, SystemAffinity};
use conary_core::repository::distro::source_feeds;
use rusqlite::Connection;

pub async fn cmd_distro_info(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    print!("{}", render_distro_info(&conn)?);
    Ok(())
}

pub fn render_distro_info(conn: &Connection) -> Result<String> {
    let mut output = String::from("Installed source affinity (diagnostic only):\n");
    let affinities = SystemAffinity::list(conn)?;
    if affinities.is_empty() {
        output.push_str("  (no data yet -- run a sync first)\n");
    } else {
        for affinity in &affinities {
            output.push_str(&format!(
                "  {}: {} packages ({:.1}%)\n",
                affinity.source_identity, affinity.package_count, affinity.percentage
            ));
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
    let mut output = String::from("Available source feeds:\n");

    for distro in source_feeds() {
        let matching_repos: Vec<_> = repos
            .iter()
            .filter(|repo| {
                repo.name == distro.id || repo.source_profile.as_deref() == Some(distro.id.as_str())
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
        schema::ensure_current(&conn).unwrap();
        (temp_file, db_path, conn)
    }

    #[test]
    fn distro_info_reports_affinity_as_diagnostic_only() {
        let (_temp, _db_path, conn) = create_test_db();

        let rendered = render_distro_info(&conn).unwrap();

        assert!(rendered.contains("diagnostic only"));
        assert!(rendered.contains("no data yet"));
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
        fedora.source_profile = Some("fedora-44".to_string());
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
