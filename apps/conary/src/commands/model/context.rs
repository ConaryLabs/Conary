// apps/conary/src/commands/model/context.rs

use std::path::Path;

use super::super::open_db;
use anyhow::{Result, anyhow};
use conary_core::model::parser::SystemModel;
use conary_core::model::{
    ModelDiff, SystemState, capture_current_state, compute_diff,
    compute_diff_with_includes_offline, parse_model_file,
};
use rusqlite::Connection;

pub(super) fn load_model(model_path: &Path) -> Result<SystemModel> {
    if !model_path.exists() {
        return Err(anyhow!("Model file not found: {}", model_path.display()));
    }
    Ok(parse_model_file(model_path)?)
}

pub(super) async fn load_model_and_diff(
    model_path: &Path,
    db_path: &str,
    offline: bool,
    announce_includes: bool,
) -> Result<(SystemModel, Connection, ModelDiff)> {
    let model = load_model(model_path)?;
    let conn = open_db(db_path)?;
    let state = capture_current_state(&conn)?;
    let diff = compute_model_diff(&model, &state, &conn, offline, announce_includes).await?;
    Ok((model, conn, diff))
}

pub(super) async fn compute_model_diff(
    model: &SystemModel,
    state: &SystemState,
    conn: &Connection,
    offline: bool,
    announce: bool,
) -> Result<ModelDiff> {
    if model.has_includes() {
        if announce {
            let mode = if offline { " (offline mode)" } else { "" };
            println!(
                "Resolving {} remote include(s){}...",
                model.include.models.len(),
                mode
            );
        }
        Ok(compute_diff_with_includes_offline(model, state, conn, offline).await?)
    } else {
        Ok(compute_diff(model, state))
    }
}
