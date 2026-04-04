// src/commands/model/publish.rs

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use conary_core::db;
use conary_core::db::models::{CollectionMember, RemoteCollection, Repository, Trove, TroveType};
use conary_core::model::parser::SystemModel;
use rusqlite::Connection;
use tracing::info;

use super::super::open_db;
use super::load_model;

/// Validated inputs for a publish operation.
struct PublishInputs {
    model: SystemModel,
    model_path: PathBuf,
    group_name: String,
    conn: Connection,
    repo_url: String,
}

/// Publish a system model as a versioned collection to a repository.
///
/// Supports both local (file://) and remote (http/https) repositories.
/// For remote repos, the collection is sent via HTTP PUT to the Remi
/// server's admin API.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_model_publish(
    model_path: &str,
    name: &str,
    version: &str,
    repo_name: &str,
    description: Option<&str>,
    db_path: &str,
    force: bool,
    sign_key_path: Option<&str>,
) -> Result<()> {
    let mut inputs = validate_publish_inputs(model_path, name, repo_name, db_path)?;

    let is_remote =
        inputs.repo_url.starts_with("http://") || inputs.repo_url.starts_with("https://");

    if is_remote {
        publish_remote(&inputs, version, repo_name, force, sign_key_path).await?;
    } else {
        publish_local(&mut inputs, version, repo_name, description, force)?;
    }

    println!();
    println!("Other systems can now include this collection:");
    println!("  [include]");
    println!(
        "  models = [\"{}@{}:stable\"]",
        inputs.group_name, repo_name
    );

    Ok(())
}

/// Validate inputs, open the database, and resolve the repository.
fn validate_publish_inputs(
    model_path: &str,
    name: &str,
    repo_name: &str,
    db_path: &str,
) -> Result<PublishInputs> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;

    let group_name = if name.starts_with("group-") {
        name.to_string()
    } else {
        format!("group-{}", name)
    };

    println!("Publishing model as collection '{}'...", group_name);

    let conn = open_db(db_path)?;

    let repo = Repository::find_by_name(&conn, repo_name)?
        .ok_or_else(|| anyhow!("Repository '{}' not found", repo_name))?;

    Ok(PublishInputs {
        model,
        model_path: model_path.to_path_buf(),
        group_name,
        conn,
        repo_url: repo.url.clone(),
    })
}

/// Publish a collection to a remote (HTTP/HTTPS) repository.
///
/// The signing key path is loaded here (if provided) to avoid naming
/// the `ed25519_dalek::SigningKey` type in shared structs.
async fn publish_remote(
    inputs: &PublishInputs,
    version: &str,
    repo_name: &str,
    force: bool,
    sign_key_path: Option<&str>,
) -> Result<()> {
    let data = conary_core::model::remote::build_collection_data_from_model(
        &inputs.model,
        &inputs.group_name,
        version,
    );

    if let Some(key_path) = sign_key_path {
        let key = conary_core::model::signing::load_signing_key(Path::new(key_path))
            .map_err(|e| anyhow!("Failed to load signing key: {e}"))?;
        let key_id = conary_core::model::signing::key_id(&key.verifying_key());
        println!("  Signing with key: {}", key_id);

        let signature = conary_core::model::signing::sign_collection(&data, &key)
            .map_err(|e| anyhow!("{e}"))?;
        println!(
            "  Signed collection ({} bytes, key {})",
            signature.len(),
            key_id
        );

        let mut sig_cache = RemoteCollection::new(
            inputs.group_name.clone(),
            Some(repo_name.to_string()),
            String::new(),
            serde_json::to_string(&data).unwrap_or_default(),
            "2099-12-31T23:59:59".to_string(),
        );
        sig_cache.version = Some(version.to_string());
        sig_cache.signature = Some(signature);
        sig_cache.signer_key_id = Some(key_id);
        let _ = sig_cache.upsert(&inputs.conn);
    }

    conary_core::model::remote::publish_remote_collection(&inputs.repo_url, &data, force)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    let member_count = data.members.len();
    println!();
    println!(
        "Published {} v{} to remote repository '{}'",
        inputs.group_name, version, repo_name
    );
    println!("  Members: {} package(s)", member_count);

    Ok(())
}

/// Publish a collection to a local (file://) repository.
fn publish_local(
    inputs: &mut PublishInputs,
    version: &str,
    repo_name: &str,
    description: Option<&str>,
    force: bool,
) -> Result<()> {
    let repo_url = &inputs.repo_url;

    if !repo_url.starts_with("file://") && !repo_url.starts_with('/') {
        return Err(anyhow!(
            "Repository URL scheme not supported: '{}'. Use file://, http://, or https://",
            repo_url
        ));
    }

    let repo_path = repo_url.strip_prefix("file://").unwrap_or(repo_url);
    let repo_dir = Path::new(repo_path);

    if !repo_dir.exists() {
        return Err(anyhow!("Repository path does not exist: {}", repo_path));
    }
    if !repo_dir.is_dir() {
        return Err(anyhow!("Repository path is not a directory: {}", repo_path));
    }

    let test_path = repo_dir.join(".conary_write_test");
    std::fs::write(&test_path, b"test")
        .map_err(|e| anyhow!("No write permission to repository {}: {}", repo_path, e))?;
    std::fs::remove_file(&test_path)?;

    let existing = Trove::find_by_name(&inputs.conn, &inputs.group_name)?;
    let has_existing_collection = existing
        .iter()
        .any(|t| t.trove_type == TroveType::Collection);

    if has_existing_collection && !force {
        return Err(anyhow!(
            "Collection '{}' already exists. Use --force to overwrite.",
            inputs.group_name
        ));
    }

    let ids_to_delete: Vec<i64> = if force {
        existing
            .iter()
            .filter(|t| t.trove_type == TroveType::Collection)
            .filter_map(|t| t.id)
            .collect()
    } else {
        Vec::new()
    };

    let model_path_display = inputs.model_path.display().to_string();
    let group_name = inputs.group_name.clone();
    let model = &inputs.model;

    db::transaction(&mut inputs.conn, |tx| {
        for id in &ids_to_delete {
            CollectionMember::delete_all_for_collection(tx, *id)?;
            Trove::delete(tx, *id)?;
        }

        let mut trove = Trove::new(
            group_name.clone(),
            version.to_string(),
            TroveType::Collection,
        );
        trove.description = description.map(|s| s.to_string());
        trove.selection_reason = Some(format!("Published from {}", model_path_display));
        let collection_id = trove.insert(tx)?;

        info!(
            "Created collection '{}' with id={}",
            group_name, collection_id
        );

        for pkg_name in &model.config.install {
            let version_constraint = model.pin.get(pkg_name).cloned();
            let is_optional = model.optional.packages.contains(pkg_name);

            let mut member = CollectionMember::new(collection_id, pkg_name.clone());
            if let Some(v) = version_constraint {
                member = member.with_version(v);
            }
            if is_optional {
                member = member.optional();
            }
            member.insert(tx)?;
        }

        for pkg_name in &model.optional.packages {
            if !model.config.install.contains(pkg_name) {
                let mut member = CollectionMember::new(collection_id, pkg_name.clone()).optional();
                if let Some(v) = model.pin.get(pkg_name) {
                    member = member.with_version(v.clone());
                }
                member.insert(tx)?;
            }
        }

        Ok(collection_id)
    })?;

    let member_count = model.config.install.len()
        + model
            .optional
            .packages
            .iter()
            .filter(|p| !model.config.install.contains(*p))
            .count();
    let optional_count = model.optional.packages.len();
    let pinned_count = model.pin.len();

    println!();
    println!(
        "Published {} v{} to repository '{}'",
        inputs.group_name, version, repo_name
    );
    println!("  Members: {} package(s)", member_count);
    if optional_count > 0 {
        println!("  Optional: {} package(s)", optional_count);
    }
    if pinned_count > 0 {
        println!("  Pinned: {} package(s)", pinned_count);
    }
    if !model.config.exclude.is_empty() {
        println!("  Exclude: {} package(s)", model.config.exclude.len());
    }

    Ok(())
}
