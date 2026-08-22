// src/commands/registry.rs
//! Registry management command implementations

use super::open_db;
use anyhow::Result;

pub async fn cmd_registry_update(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    println!("Syncing canonical registry...");

    let authorities: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT name, default_strategy_endpoint
             FROM repositories
             WHERE enabled = 1
               AND default_strategy = 'remi'
               AND default_strategy_endpoint IS NOT NULL
             ORDER BY priority DESC",
        )?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    if authorities.is_empty() {
        anyhow::bail!(
            "No canonical authority is configured; add an enabled repository with an explicit Remi strategy endpoint"
        );
    }

    let mut errors = Vec::new();
    for (name, configured_endpoint) in &authorities {
        let endpoint = configured_endpoint.trim_end_matches('/');
        match conary_core::repository::universe::sync_remi_universe(
            std::path::Path::new(db_path),
            endpoint,
        )
        .await
        {
            Ok(conary_core::repository::universe::RemiUniverseSyncOutcome::Activated {
                package_count,
                ..
            }) => {
                println!(
                    "Activated a signed universe with {package_count} packages from {name} ({endpoint})"
                );
                crate::ui::status("Updated", "registry.");
                return Ok(());
            }
            Ok(conary_core::repository::universe::RemiUniverseSyncOutcome::Unchanged {
                ..
            }) => {
                println!("Signed Remi universe is current");
                crate::ui::status("Updated", "registry.");
                return Ok(());
            }
            Err(e) => {
                errors.push(format!("{name} ({endpoint}): {e}"));
            }
        }
    }

    anyhow::bail!(
        "All configured signed-universe authorities failed: {}",
        errors.join("; ")
    )
}

pub async fn cmd_registry_stats(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let canonical_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM resolved_canonical_packages",
        [],
        |row| row.get(0),
    )?;
    let impl_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM resolved_package_implementations",
        [],
        |row| row.get(0),
    )?;
    let group_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM resolved_canonical_packages WHERE kind = 'group'",
        [],
        |row| row.get(0),
    )?;

    println!("Canonical registry statistics:");
    println!("  Canonical packages: {canonical_count}");
    println!("  Package groups:     {group_count}");
    println!("  Implementations:    {impl_count}");
    println!();

    let mut stmt = conn.prepare(
        "SELECT source, COUNT(*) FROM resolved_package_implementations GROUP BY source ORDER BY COUNT(*) DESC",
    )?;
    let sources: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if !sources.is_empty() {
        println!("  By source:");
        for (source, count) in &sources {
            println!("    {source}: {count}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry_stats_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db_str = db_path.to_str().unwrap();

        // Initialize the database (creates file + schema)
        conary_core::db::init(db_str).unwrap();

        // Stats should succeed on an empty database
        let result = cmd_registry_stats(db_str).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn registry_update_requires_explicit_authority() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db_str = db_path.to_str().unwrap();

        // Initialize the database (creates file + schema)
        conary_core::db::init(db_str).unwrap();

        let error = cmd_registry_update(db_str).await.unwrap_err();
        assert!(error.to_string().contains("No canonical authority"));
    }
}
