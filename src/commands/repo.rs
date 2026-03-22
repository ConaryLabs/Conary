// src/commands/repo.rs
//! Repository management commands

use super::open_db;
use anyhow::Result;
use conary_core::db::paths::keyring_dir;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::time::Duration;
use tracing::info;

/// Add a new repository
#[allow(clippy::too_many_arguments)]
pub async fn cmd_repo_add(
    name: &str,
    url: &str,
    db_path: &str,
    content_url: Option<String>,
    priority: i32,
    disabled: bool,
    gpg_key: Option<String>,
    no_gpg_check: bool,
    gpg_strict: bool,
    default_strategy: Option<String>,
    remi_endpoint: Option<String>,
    remi_distro: Option<String>,
) -> Result<()> {
    info!("Adding repository: {} ({})", name, url);

    // Validate remi strategy configuration
    if let Some(ref strategy) = default_strategy
        && strategy == "remi"
    {
        if remi_endpoint.is_none() {
            anyhow::bail!("--remi-endpoint is required when --default-strategy=remi");
        }
        if remi_distro.is_none() {
            anyhow::bail!("--remi-distro is required when --default-strategy=remi");
        }
    }

    let conn = open_db(db_path)?;

    // Create the repository with all settings
    let mut repo = conary_core::db::models::Repository::new(name.to_string(), url.to_string());
    repo.content_url = content_url;
    repo.enabled = !disabled;
    repo.priority = priority;
    repo.gpg_check = !no_gpg_check;
    repo.gpg_strict = gpg_strict;
    repo.gpg_key_url = gpg_key.clone();
    repo.default_strategy = default_strategy.clone();
    repo.default_strategy_endpoint = remi_endpoint;
    repo.default_strategy_distro = remi_distro;

    if let Err(e) = repo.insert(&conn) {
        let msg = e.to_string();
        if msg.contains("UNIQUE constraint failed") {
            anyhow::bail!(
                "Repository '{}' already exists.\nUse 'conary repo list' to see configured repositories.",
                name
            );
        }
        return Err(anyhow::anyhow!(
            "Failed to add repository '{}': {}",
            name,
            e
        ));
    }

    println!("Added repository: {}", repo.name);
    println!("  Metadata URL: {}", repo.url);
    if let Some(ref content) = repo.content_url {
        println!("  Content URL: {} (reference mirror)", content);
    }
    println!("  Enabled: {}", repo.enabled);
    println!("  Priority: {}", repo.priority);
    println!("  GPG Check: {}", repo.gpg_check);
    if repo.gpg_strict {
        println!("  GPG Strict: true (missing signatures will fail)");
    }

    // Show default strategy if configured
    if let Some(ref strategy) = repo.default_strategy {
        println!("  Default Strategy: {}", strategy);
        if strategy == "remi" {
            if let Some(ref endpoint) = repo.default_strategy_endpoint {
                println!("  Remi Endpoint: {}", endpoint);
            }
            if let Some(ref distro) = repo.default_strategy_distro {
                println!("  Remi Distro: {}", distro);
            }
        }
    }

    // If GPG key was provided, import it
    if let Some(key_source) = gpg_key {
        println!("  Importing GPG key...");
        match import_gpg_key(name, &key_source, db_path).await {
            Ok(fingerprint) => println!("  GPG Key: {}", fingerprint),
            Err(e) => println!("  Warning: Failed to import GPG key: {}", e),
        }
    }

    Ok(())
}

/// List repositories
pub async fn cmd_repo_list(db_path: &str, all: bool) -> Result<()> {
    info!("Listing repositories");
    let conn = open_db(db_path)?;
    let repos = if all {
        conary_core::db::models::Repository::list_all(&conn)?
    } else {
        conary_core::db::models::Repository::list_enabled(&conn)?
    };

    if repos.is_empty() {
        println!("No repositories configured");
    } else {
        println!("Repositories:");
        for repo in repos {
            let enabled_mark = if repo.enabled { "[x]" } else { "[ ]" };
            let sync_status = repo
                .last_sync
                .as_ref()
                .map(|ts| format!("synced {}", ts))
                .unwrap_or_else(|| "never synced".to_string());
            println!(
                "  {} {} (priority: {}, {})",
                enabled_mark, repo.name, repo.priority, sync_status
            );
            println!("      metadata: {}", repo.url);
            if let Some(ref content) = repo.content_url {
                println!("      content:  {} (reference mirror)", content);
            }
        }
    }
    Ok(())
}

/// Remove a repository
pub async fn cmd_repo_remove(name: &str, db_path: &str) -> Result<()> {
    info!("Removing repository: {}", name);
    let conn = open_db(db_path)?;
    conary_core::repository::remove_repository(&conn, name)?;
    println!("Removed repository: {}", name);
    Ok(())
}

/// Enable a repository
pub async fn cmd_repo_enable(name: &str, db_path: &str) -> Result<()> {
    set_repo_enabled(name, db_path, true)
}

/// Disable a repository
pub async fn cmd_repo_disable(name: &str, db_path: &str) -> Result<()> {
    set_repo_enabled(name, db_path, false)
}

fn set_repo_enabled(name: &str, db_path: &str, enabled: bool) -> Result<()> {
    let action = if enabled { "Enabling" } else { "Disabling" };
    info!("{} repository: {}", action, name);
    let conn = open_db(db_path)?;
    conary_core::repository::set_repository_enabled(&conn, name, enabled)?;
    let past = if enabled { "Enabled" } else { "Disabled" };
    println!("{} repository: {}", past, name);
    Ok(())
}

/// Sync repository metadata
pub async fn cmd_repo_sync(name: Option<String>, db_path: &str, force: bool) -> Result<()> {
    info!("Synchronizing repository metadata");

    let conn = open_db(db_path)?;

    let repos_to_sync = if let Some(repo_name) = name {
        let repo = conary_core::db::models::Repository::find_by_name(&conn, &repo_name)?
            .ok_or_else(|| anyhow::anyhow!("Repository '{}' not found", repo_name))?;
        vec![repo]
    } else {
        conary_core::db::models::Repository::list_enabled(&conn)?
    };

    if repos_to_sync.is_empty() {
        println!("No repositories to sync");
        return Ok(());
    }

    let repos_needing_sync: Vec<_> = repos_to_sync
        .into_iter()
        .filter(|repo| force || conary_core::repository::needs_sync(repo))
        .collect();

    if repos_needing_sync.is_empty() {
        println!("All repositories are up to date");
        return Ok(());
    }

    let keyring_dir = keyring_dir(db_path);

    let spinner_style = ProgressStyle::default_spinner()
        .template("  {spinner:.cyan} {msg}")
        .expect("Invalid spinner template");

    let mut results: Vec<(String, conary_core::Result<usize>, Option<String>)> = Vec::new();
    for repo in &repos_needing_sync {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(spinner_style.clone());
        spinner.enable_steady_tick(Duration::from_millis(100));
        spinner.set_message(format!("Syncing {}...", repo.name));

        // Try to fetch GPG key if configured and gpg_check is enabled
        let gpg_result = if repo.gpg_check {
            spinner.set_message(format!("Fetching GPG key for {}...", repo.name));
            match conary_core::repository::maybe_fetch_gpg_key(repo, &keyring_dir).await {
                Ok(Some(fingerprint)) => Some(fingerprint),
                Ok(None) => None,
                Err(e) => {
                    spinner.suspend(|| {
                        println!("  Warning: GPG key fetch failed for {}: {}", repo.name, e);
                    });
                    None
                }
            }
        } else {
            None
        };

        spinner.set_message(format!("Syncing metadata for {}...", repo.name));
        let sync_result = {
            let conn = conary_core::db::open(db_path)?;
            let mut repo_mut = repo.clone();
            conary_core::repository::sync_repository(&conn, &mut repo_mut).await
        };

        match &sync_result {
            Ok(count) => {
                spinner.finish_with_message(format!("{}: {} packages", repo.name, count));
            }
            Err(e) => {
                spinner.finish_with_message(format!("{}: FAILED ({})", repo.name, e));
            }
        }

        results.push((repo.name.clone(), sync_result, gpg_result));
    }

    let mut failures = Vec::new();

    for (name, result, gpg_key) in results {
        match result {
            Ok(count) => {
                let gpg_note = gpg_key
                    .map(|fp| format!(" (GPG key imported: {})", &fp[..16]))
                    .unwrap_or_default();
                println!(
                    "  [OK] Synchronized {} packages from {}{}",
                    count, name, gpg_note
                );
            }
            Err(e) => {
                println!("  [FAILED] Failed to sync {}: {}", name, e);
                failures.push((name, e.to_string()));
            }
        }
    }

    // Best-effort canonical map sync from Remi
    {
        let repos: Vec<String> = conn
            .prepare("SELECT url FROM repositories WHERE enabled = 1 ORDER BY priority DESC")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        for url in &repos {
            let endpoint = url.trim_end_matches('/');
            match conary_core::canonical::client::fetch_canonical_map(&conn, endpoint).await {
                Ok(Some(n)) => {
                    tracing::info!("Canonical map updated: {n} entries from {endpoint}");
                    break;
                }
                Ok(None) => {
                    tracing::debug!("Canonical map is current (304)");
                    break;
                }
                Err(e) => {
                    tracing::debug!("Canonical map fetch from {endpoint} skipped: {e}");
                    continue;
                }
            }
        }
    }

    if !failures.is_empty() {
        let failed_names = failures
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("Failed to sync repository metadata for: {failed_names}");
    }

    Ok(())
}

/// Search for packages
pub async fn cmd_search(pattern: &str, db_path: &str) -> Result<()> {
    info!("Searching for packages matching: {}", pattern);
    let conn = open_db(db_path)?;
    let packages = conary_core::repository::search_packages(&conn, pattern)?;

    if packages.is_empty() {
        println!("No packages found matching '{}'", pattern);
    } else {
        println!("Found {} packages matching '{}':", packages.len(), pattern);
        for pkg in packages {
            let arch_str = pkg.architecture.as_deref().unwrap_or("noarch");
            println!("  {} {} ({})", pkg.name, pkg.version, arch_str);
            if let Some(desc) = &pkg.description {
                println!("      {}", desc);
            }
        }
    }
    Ok(())
}

// =============================================================================
// GPG Key Management Commands
// =============================================================================

/// Internal helper to import a GPG key from file or URL
async fn import_gpg_key(repository: &str, key_source: &str, db_path: &str) -> Result<String> {
    use conary_core::repository::GpgVerifier;

    let keyring_dir = keyring_dir(db_path);
    let verifier = GpgVerifier::new(keyring_dir)?;

    // Check if it's a URL
    if key_source.starts_with("http://") || key_source.starts_with("https://") {
        info!("Fetching GPG key from URL: {}", key_source);
        let response = reqwest::get(key_source)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch GPG key: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to fetch GPG key: HTTP {}",
                response.status()
            ));
        }

        let key_data = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read GPG key data: {}", e))?;

        Ok(verifier.import_key(&key_data, repository)?)
    } else {
        // It's a local file path
        info!("Importing GPG key from file: {}", key_source);
        let key_path = Path::new(key_source);
        if !key_path.exists() {
            anyhow::bail!("GPG key file not found: {}", key_source);
        }
        Ok(verifier.import_key_from_file(key_path, repository)?)
    }
}

/// Import a GPG key for a repository
pub async fn cmd_key_import(repository: &str, key_source: &str, db_path: &str) -> Result<()> {
    info!("Importing GPG key for repository: {}", repository);

    // Verify repository exists
    let conn = open_db(db_path)?;
    let repo = conary_core::db::models::Repository::find_by_name(&conn, repository)?
        .ok_or_else(|| anyhow::anyhow!("Repository '{}' not found", repository))?;

    let fingerprint = import_gpg_key(repository, key_source, db_path).await?;

    println!("Imported GPG key for repository '{}'", repo.name);
    println!("  Fingerprint: {}", fingerprint);

    // Update repository's gpg_key_url if it was a URL
    if key_source.starts_with("http://") || key_source.starts_with("https://") {
        let mut repo = repo;
        repo.gpg_key_url = Some(key_source.to_string());
        repo.update(&conn)?;
        println!("  Updated repository gpg_key_url");
    }

    Ok(())
}

/// List all imported GPG keys
pub async fn cmd_key_list(db_path: &str) -> Result<()> {
    use conary_core::repository::GpgVerifier;

    info!("Listing GPG keys");
    let keyring_dir = keyring_dir(db_path);
    let verifier = GpgVerifier::new(keyring_dir)?;

    let keys = verifier.list_keys()?;

    if keys.is_empty() {
        println!("No GPG keys imported");
    } else {
        println!("GPG Keys:");
        for (repo_name, fingerprint) in keys {
            println!("  {} -> {}", repo_name, fingerprint);
        }
    }
    Ok(())
}

/// Remove a GPG key for a repository
pub async fn cmd_key_remove(repository: &str, db_path: &str) -> Result<()> {
    use conary_core::repository::GpgVerifier;

    info!("Removing GPG key for repository: {}", repository);
    let keyring_dir = keyring_dir(db_path);
    let verifier = GpgVerifier::new(keyring_dir)?;

    if !verifier.has_key(repository) {
        return Err(anyhow::anyhow!(
            "No GPG key found for repository '{}'",
            repository
        ));
    }

    verifier.remove_key(repository)?;
    println!("Removed GPG key for repository '{}'", repository);
    Ok(())
}
