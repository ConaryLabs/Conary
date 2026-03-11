// src/commands/trust.rs

//! TUF trust management command implementations

use anyhow::{Context, Result, anyhow};
#[cfg(feature = "server")]
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::db;
use conary_core::db::models::Repository;
use conary_core::trust::ceremony;
use conary_core::trust::client::TufClient;
use conary_core::trust::metadata::Role;
use rusqlite::{Connection, params};
use std::path::Path;

/// Look up a repository by name and extract its ID
fn get_repo_with_id(conn: &Connection, repo_name: &str) -> Result<(Repository, i64)> {
    let repo = Repository::find_by_name(conn, repo_name)?
        .ok_or_else(|| anyhow::anyhow!("Repository not found: {repo_name}"))?;
    let repo_id = repo
        .id
        .ok_or_else(|| anyhow::anyhow!("Repository has no ID"))?;
    Ok((repo, repo_id))
}

/// Generate a new Ed25519 key pair for a TUF role
pub fn cmd_trust_key_gen(role: &str, output: &str) -> Result<()> {
    // Validate role name
    let _: Role = role.parse().map_err(|e| anyhow::anyhow!("{}", e))?;

    let output_dir = Path::new(output);
    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir)
            .with_context(|| format!("Failed to create output directory: {output}"))?;
    }

    let keypair = ceremony::generate_role_key(role, output_dir)?;
    let (key_id, _) =
        conary_core::trust::signing_keypair_to_tuf_key(&keypair).map_err(|e| anyhow!("{}", e))?;

    println!("Generated {role} key pair:");
    println!("  Private key: {output}/{role}.private");
    println!("  Public key:  {output}/{role}.public");
    println!("  Key ID:      {key_id}");

    Ok(())
}

/// Bootstrap TUF for a repository with initial root metadata
pub fn cmd_trust_init(repo_name: &str, root_path: &str, db_path: &str) -> Result<()> {
    let conn = db::open(db_path)?;
    let (repo, repo_id) = get_repo_with_id(&conn, repo_name)?;

    // Read root.json
    let root_json = std::fs::read(root_path)
        .with_context(|| format!("Failed to read root metadata: {root_path}"))?;

    // Bootstrap TUF
    let client = TufClient::new(repo_id, &repo.url, repo.tuf_root_url.as_deref())?;
    client.bootstrap(&conn, &root_json)?;

    // Enable TUF for this repository
    conn.execute(
        "UPDATE repositories SET tuf_enabled = 1 WHERE id = ?1",
        params![repo_id],
    )?;

    println!("TUF initialized for repository: {repo_name}");
    println!("TUF verification is now enabled.");

    Ok(())
}

/// Enable TUF verification for a repository
pub fn cmd_trust_enable(repo_name: &str, tuf_url: Option<&str>, db_path: &str) -> Result<()> {
    let conn = db::open(db_path)?;
    let (_repo, repo_id) = get_repo_with_id(&conn, repo_name)?;

    // Check that TUF has been bootstrapped
    let has_root: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM tuf_roots WHERE repository_id = ?1",
        params![repo_id],
        |row| row.get(0),
    )?;

    if !has_root {
        anyhow::bail!(
            "No TUF root found. Run 'conary trust init {repo_name} --root <root.json>' first."
        );
    }

    conn.execute(
        "UPDATE repositories SET tuf_enabled = 1, tuf_root_url = ?1 WHERE id = ?2",
        params![tuf_url, repo_id],
    )?;

    println!("TUF verification enabled for: {repo_name}");
    if let Some(url) = tuf_url {
        println!("TUF metadata URL: {url}");
    }

    Ok(())
}

/// Disable TUF verification for a repository (unsafe operation)
pub fn cmd_trust_disable(repo_name: &str, force: bool, db_path: &str) -> Result<()> {
    if !force {
        anyhow::bail!("Disabling TUF removes supply chain protection. Use --force to confirm.");
    }

    let conn = db::open(db_path)?;
    let (_repo, repo_id) = get_repo_with_id(&conn, repo_name)?;

    conn.execute(
        "UPDATE repositories SET tuf_enabled = 0 WHERE id = ?1",
        params![repo_id],
    )?;

    println!("[WARNING] TUF verification disabled for: {repo_name}");
    println!("This repository is now vulnerable to supply chain attacks.");

    Ok(())
}

/// Show TUF metadata status for a repository
pub fn cmd_trust_status(repo_name: &str, db_path: &str) -> Result<()> {
    let conn = db::open(db_path)?;
    let (repo, repo_id) = get_repo_with_id(&conn, repo_name)?;

    println!("Repository: {repo_name}");
    println!(
        "TUF enabled: {}",
        if repo.tuf_enabled { "yes" } else { "no" }
    );

    if let Some(url) = &repo.tuf_root_url {
        println!("TUF URL: {url}");
    }

    if !repo.tuf_enabled {
        return Ok(());
    }

    // Show metadata versions and expiry
    let mut stmt = conn.prepare(
        "SELECT role, version, expires_at, verified_at FROM tuf_metadata
         WHERE repository_id = ?1 ORDER BY role",
    )?;

    let rows: Vec<(String, i64, String, String)> = stmt
        .query_map(params![repo_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    if rows.is_empty() {
        println!("No TUF metadata stored yet.");
    } else {
        println!();
        println!(
            "{:<12} {:<8} {:<25} {:<25}",
            "Role", "Version", "Expires", "Last Verified"
        );
        println!("{}", "-".repeat(70));
        for (role, version, expires, verified) in &rows {
            println!("{role:<12} v{version:<6} {expires:<25} {verified:<25}");
        }
    }

    // Show target count
    let target_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tuf_targets WHERE repository_id = ?1",
        params![repo_id],
        |row| row.get(0),
    )?;
    println!();
    println!("Verified targets: {target_count}");

    // Show key count
    let key_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tuf_keys WHERE repository_id = ?1",
        params![repo_id],
        |row| row.get(0),
    )?;
    println!("Trusted keys: {key_count}");

    Ok(())
}

/// Verify all TUF metadata for a repository
pub fn cmd_trust_verify(repo_name: &str, db_path: &str) -> Result<()> {
    let conn = db::open(db_path)?;
    let (repo, repo_id) = get_repo_with_id(&conn, repo_name)?;

    if !repo.tuf_enabled {
        println!("TUF is not enabled for repository: {repo_name}");
        return Ok(());
    }

    println!("Verifying TUF metadata for: {repo_name}");

    let client = TufClient::new(repo_id, &repo.url, repo.tuf_root_url.as_deref())?;
    match client.update(&conn) {
        Ok(state) => {
            println!("[OK] Root:      v{}", state.root_version);
            println!(
                "[OK] Targets:   v{} ({} targets)",
                state.targets_version,
                state.targets.len()
            );
            println!("[OK] Snapshot:  v{}", state.snapshot_version);
            println!("[OK] Timestamp: v{}", state.timestamp_version);
            println!();
            println!("All TUF metadata verified successfully.");
        }
        Err(e) => {
            println!("[FAILED] TUF verification error: {e}");
            anyhow::bail!("TUF verification failed: {e}");
        }
    }

    Ok(())
}

/// Sign targets metadata (server-side operation)
#[cfg(feature = "server")]
pub fn cmd_trust_sign_targets(repo_name: &str, key_path: &str, db_path: &str) -> Result<()> {
    let conn = db::open(db_path)?;
    let (_repo, _repo_id) = get_repo_with_id(&conn, repo_name)?;

    let _key = SigningKeyPair::load_from_file(Path::new(key_path))
        .with_context(|| format!("Failed to load signing key: {key_path}"))?;

    // TODO: Read packages from repository and generate targets
    println!(
        "Targets signing for server-side use - implementation pending full server integration"
    );

    Ok(())
}

/// Rotate a TUF role key (server-side operation)
#[cfg(feature = "server")]
pub fn cmd_trust_rotate_key(
    role: &str,
    old_key_path: &str,
    new_key_path: &str,
    root_key_path: &str,
    repo_name: &str,
    db_path: &str,
) -> Result<()> {
    let conn = db::open(db_path)?;
    let (repo, repo_id) = get_repo_with_id(&conn, repo_name)?;

    let old_key = SigningKeyPair::load_from_file(Path::new(old_key_path))
        .with_context(|| format!("Failed to load old key: {old_key_path}"))?;
    let new_key = SigningKeyPair::load_from_file(Path::new(new_key_path))
        .with_context(|| format!("Failed to load new key: {new_key_path}"))?;
    let root_key = SigningKeyPair::load_from_file(Path::new(root_key_path))
        .with_context(|| format!("Failed to load root key: {root_key_path}"))?;

    // Load current root
    let client = TufClient::new(repo_id, &repo.url, repo.tuf_root_url.as_deref())?;

    // Read current root from DB
    let root_json: String = conn.query_row(
        "SELECT signed_metadata FROM tuf_roots WHERE repository_id = ?1 ORDER BY version DESC LIMIT 1",
        params![repo_id],
        |row| row.get(0),
    )?;

    let current_root: conary_core::trust::Signed<conary_core::trust::RootMetadata> =
        serde_json::from_str(&root_json)?;

    let new_root = ceremony::rotate_key(&current_root, role, &old_key, &new_key, &root_key, 365)?;

    // Bootstrap the new root
    let new_root_json = serde_json::to_vec(&new_root)?;
    client.bootstrap(&conn, &new_root_json)?;

    let (new_key_id, _) =
        conary_core::trust::signing_keypair_to_tuf_key(&new_key).map_err(|e| anyhow!("{}", e))?;
    println!("Key rotation complete for role: {role}");
    println!("New root version: {}", new_root.signed.version);
    println!("New key ID: {new_key_id}");

    Ok(())
}
