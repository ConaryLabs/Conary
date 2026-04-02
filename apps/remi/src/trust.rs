// apps/remi/src/trust.rs
//! Remi-owned TUF admin helpers.

use anyhow::{Context, Result, anyhow};
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::db;
use conary_core::db::models::Repository;
use conary_core::trust::RootMetadata;
use conary_core::trust::Signed;
use conary_core::trust::ceremony;
use conary_core::trust::client::TufClient;
use rusqlite::{Connection, params};
use std::path::Path;

fn open_db(path: &str) -> Result<Connection> {
    db::open(path).context("Failed to open package database")
}

fn get_repo_with_id(conn: &Connection, repo_name: &str) -> Result<(Repository, i64)> {
    let repo = Repository::find_by_name(conn, repo_name)?
        .ok_or_else(|| anyhow!("Repository not found: {repo_name}"))?;
    let repo_id = repo.id.ok_or_else(|| anyhow!("Repository has no ID"))?;
    Ok((repo, repo_id))
}

pub fn sign_targets(repo_name: &str, key_path: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let (_repo, _repo_id) = get_repo_with_id(&conn, repo_name)?;

    let _key = SigningKeyPair::load_from_file(Path::new(key_path))
        .with_context(|| format!("Failed to load signing key: {key_path}"))?;

    println!("Targets signing for Remi-owned repository admin is pending full server integration");

    Ok(())
}

pub fn rotate_key(
    role: &str,
    old_key_path: &str,
    new_key_path: &str,
    root_key_path: &str,
    repo_name: &str,
    db_path: &str,
) -> Result<()> {
    let conn = open_db(db_path)?;
    let (repo, repo_id) = get_repo_with_id(&conn, repo_name)?;

    let old_key = SigningKeyPair::load_from_file(Path::new(old_key_path))
        .with_context(|| format!("Failed to load old key: {old_key_path}"))?;
    let new_key = SigningKeyPair::load_from_file(Path::new(new_key_path))
        .with_context(|| format!("Failed to load new key: {new_key_path}"))?;
    let root_key = SigningKeyPair::load_from_file(Path::new(root_key_path))
        .with_context(|| format!("Failed to load root key: {root_key_path}"))?;

    let client = TufClient::new(repo_id, &repo.url, repo.tuf_root_url.as_deref())?;

    let root_json: String = conn.query_row(
        "SELECT signed_metadata FROM tuf_roots WHERE repository_id = ?1 ORDER BY version DESC LIMIT 1",
        params![repo_id],
        |row| row.get(0),
    )?;

    let current_root: Signed<RootMetadata> = serde_json::from_str(&root_json)?;
    let new_root = ceremony::rotate_key(&current_root, role, &old_key, &new_key, &root_key, 365)?;

    let new_root_json = serde_json::to_vec(&new_root)?;
    client.bootstrap(&conn, &new_root_json)?;

    let (new_key_id, _) =
        conary_core::trust::signing_keypair_to_tuf_key(&new_key).map_err(|err| anyhow!("{err}"))?;
    println!("Key rotation complete for role: {role}");
    println!("New root version: {}", new_root.signed.version);
    println!("New key ID: {new_key_id}");

    Ok(())
}
