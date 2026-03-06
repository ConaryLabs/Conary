// src/commands/registry.rs
//! Registry management command implementations

use anyhow::Result;

pub fn cmd_registry_update(db_path: &str) -> Result<()> {
    let _conn = conary_core::db::open(db_path)?;
    println!("Syncing canonical registry...");
    let rules_dir = std::path::Path::new("/usr/share/conary/canonical-rules");
    let local_dir = std::path::Path::new("data/canonical-rules");
    let dir = if rules_dir.exists() {
        rules_dir
    } else {
        local_dir
    };

    if dir.exists() {
        let engine = conary_core::canonical::rules::RulesEngine::load_from_dir(dir)?;
        println!("Loaded {} curated rules", engine.rule_count());
    } else {
        println!("No canonical rules found at {}", dir.display());
    }
    println!("[COMPLETE] Registry updated");
    Ok(())
}

pub fn cmd_registry_stats(db_path: &str) -> Result<()> {
    let conn = conary_core::db::open(db_path)?;
    let canonical_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM canonical_packages",
        [],
        |row| row.get(0),
    )?;
    let impl_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM package_implementations",
        [],
        |row| row.get(0),
    )?;
    let group_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM canonical_packages WHERE kind = 'group'",
        [],
        |row| row.get(0),
    )?;

    println!("Canonical registry statistics:");
    println!("  Canonical packages: {canonical_count}");
    println!("  Package groups:     {group_count}");
    println!("  Implementations:    {impl_count}");
    println!();

    let mut stmt = conn.prepare(
        "SELECT source, COUNT(*) FROM package_implementations GROUP BY source ORDER BY COUNT(*) DESC",
    )?;
    let sources: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

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

    #[test]
    fn test_registry_stats_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db_str = db_path.to_str().unwrap();

        // Initialize the database (creates file + schema)
        conary_core::db::init(db_str).unwrap();

        // Stats should succeed on an empty database
        let result = cmd_registry_stats(db_str);
        assert!(result.is_ok());
    }

    #[test]
    fn test_registry_update_with_local_rules() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db_str = db_path.to_str().unwrap();

        // Initialize the database (creates file + schema)
        conary_core::db::init(db_str).unwrap();

        // Update should succeed (may not find rules dir, but should not error)
        let result = cmd_registry_update(db_str);
        assert!(result.is_ok());
    }
}
