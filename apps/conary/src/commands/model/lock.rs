// src/commands/model/lock.rs

use std::path::{Path, PathBuf};

use super::super::open_db;
use super::context::load_model;
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::RemoteCollection;
use conary_core::model::lockfile::ModelLock;
use conary_core::model::parser::SystemModel;
use conary_core::model::remote::CollectionData;
use conary_core::model::{parse_trove_spec, resolve_includes};
use rusqlite::Connection;

fn collect_lock_data(
    model: &SystemModel,
    conn: &Connection,
) -> Result<Vec<(String, String, CollectionData)>> {
    let mut lock_data = Vec::new();
    for spec in &model.include.models {
        let (name, label) = parse_trove_spec(spec)?;
        let label_str = label.as_deref().unwrap_or("");
        if let Some(cached) = RemoteCollection::find_cached(conn, &name, Some(label_str))
            .map_err(|e| anyhow!("Database error: {}", e))?
        {
            let data: CollectionData = serde_json::from_str(&cached.data_json)
                .map_err(|e| anyhow!("Corrupt cache entry for '{}': {}", name, e))?;
            lock_data.push((name, label_str.to_string(), data));
        } else {
            return Err(anyhow!(
                "No cached data for '{}' after resolution -- this should not happen",
                spec
            ));
        }
    }
    Ok(lock_data)
}

fn build_lock_from_data(
    lock_data: &[(String, String, CollectionData)],
    model_path: &Path,
) -> Result<ModelLock> {
    let refs: Vec<(String, String, &CollectionData)> = lock_data
        .iter()
        .map(|(n, l, d)| (n.clone(), l.clone(), d))
        .collect();
    let mut lock = ModelLock::from_resolved(&refs);
    let model_bytes = std::fs::read(model_path)
        .with_context(|| format!("Failed to read model file '{}'", model_path.display()))?;
    lock.metadata.model_hash = format!("sha256:{}", conary_core::hash::sha256(&model_bytes));
    Ok(lock)
}

/// Lock remote include hashes for reproducibility
///
/// Resolves all remote includes and records their content hashes
/// in a model.lock file, preventing silent upstream changes.
pub async fn cmd_model_lock(model_path: &str, output: Option<&str>, db_path: &str) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = open_db(db_path)?;

    if !model.has_includes() {
        println!("No remote includes to lock");
        return Ok(());
    }

    let _resolved = resolve_includes(&model, &conn).await?;

    let lock_data = collect_lock_data(&model, &conn)?;
    let lock = build_lock_from_data(&lock_data, model_path)?;

    let lock_path = if let Some(out) = output {
        PathBuf::from(out)
    } else {
        let model_dir = model_path.parent().unwrap_or(Path::new("."));
        model_dir.join("model.lock")
    };

    lock.save(&lock_path)?;

    println!(
        "Locked {} collection(s) to {}",
        lock.collections.len(),
        lock_path.display()
    );
    for coll in &lock.collections {
        println!(
            "  {} ({}) - {} members, hash: {}",
            coll.name, coll.label, coll.member_count, coll.content_hash
        );
    }

    Ok(())
}

/// Update locked remote includes
///
/// Force-refreshes all remote includes, compares against the existing lock
/// file, and updates the lock with new hashes. Reports what changed.
pub async fn cmd_model_update(model_path: &str, db_path: &str) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = open_db(db_path)?;

    let model_dir = model_path.parent().unwrap_or(Path::new("."));
    let lock_path = model_dir.join("model.lock");

    if !lock_path.exists() {
        return Err(anyhow!(
            "No lock file found at {}. Run 'conary model lock' first.",
            lock_path.display()
        ));
    }

    let old_lock = ModelLock::load(&lock_path)?;

    if !model.has_includes() {
        println!("No remote includes to update");
        return Ok(());
    }

    // Force-refresh each include by purging cache first
    for spec in &model.include.models {
        let (name, label) = parse_trove_spec(spec)?;
        if let Some(label_str) = &label {
            let _ = RemoteCollection::purge_by_name(&conn, &name, Some(label_str));
        }
    }

    let _resolved = resolve_includes(&model, &conn).await?;

    let lock_data = collect_lock_data(&model, &conn)?;
    let current_hashes: Vec<(String, String, String)> = lock_data
        .iter()
        .map(|(n, l, d)| (n.clone(), l.clone(), d.content_hash.clone()))
        .collect();

    let drifts = old_lock.check_drift(&current_hashes);

    let new_lock = build_lock_from_data(&lock_data, model_path)?;
    new_lock.save(&lock_path)?;

    // Report results
    let changed = drifts.len();
    println!(
        "Updated {} collection(s), {} changed",
        new_lock.collections.len(),
        changed
    );

    if !drifts.is_empty() {
        println!();
        println!("Changes detected:");
        for drift in &drifts {
            println!(
                "  {} ({}): {} -> {}",
                drift.name, drift.label, drift.locked_hash, drift.current_hash
            );
        }
    }

    Ok(())
}
