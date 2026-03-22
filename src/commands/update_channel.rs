// src/commands/update_channel.rs
//! Update channel management commands

use super::open_db;
use anyhow::Result;
use conary_core::db::models::settings;
use conary_core::self_update::{DEFAULT_UPDATE_CHANNEL, get_update_channel, set_update_channel};

pub async fn cmd_update_channel_get(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let channel = get_update_channel(&conn)?;
    let is_default = channel == DEFAULT_UPDATE_CHANNEL;
    println!("{}{}", channel, if is_default { " (default)" } else { "" });
    Ok(())
}

pub async fn cmd_update_channel_set(db_path: &str, url: &str) -> Result<()> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(anyhow::anyhow!("URL must use http:// or https:// scheme"));
    }
    let conn = open_db(db_path)?;
    set_update_channel(&conn, url)?;
    println!("Update channel set to: {url}");
    Ok(())
}

pub async fn cmd_update_channel_reset(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    settings::delete(&conn, "update-channel")?;
    println!("Update channel reset to default: {DEFAULT_UPDATE_CHANNEL}");
    Ok(())
}
