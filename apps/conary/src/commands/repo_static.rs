// apps/conary/src/commands/repo_static.rs
//! Static repository trust establishment commands.

use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use conary_core::db::models::{Repository, RepositoryPackage};
use conary_core::repository::static_repo::{RepoIdentity, RepoLocation};
use conary_core::trust::client::TufClient;
use conary_core::trust::metadata::{Role, RootMetadata, Signed};
use conary_core::trust::verify::{extract_role_keys, verify_not_expired, verify_signatures};
use rusqlite::Connection;

use super::open_db;
use super::repo::RepoAddOptions;

const MAX_STATIC_REPO_IDENTITY_SIZE: u64 = 256 * 1024;
const MAX_STATIC_ROOT_SIZE: u64 = 10 * 1024 * 1024;
const KEY_ID_HEX_LEN: usize = 64;

pub(crate) async fn try_cmd_repo_add_static(opts: &RepoAddOptions) -> Result<bool> {
    let Some(normalized_base) = normalize_static_repo_base(&opts.url)? else {
        return Ok(false);
    };
    let location = RepoLocation::parse(&normalized_base)
        .with_context(|| format!("invalid static repository location {}", opts.url))?;

    let identity_probe = location
        .try_fetch_bytes("conary-repo.toml", MAX_STATIC_REPO_IDENTITY_SIZE)
        .await;
    let identity_bytes = match identity_probe {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(false),
        Err(error) => return Err(error).context("probe static repository identity"),
    };

    if !opts.debian_release_keys.is_empty()
        || !opts.rpm_metadata_keys.is_empty()
        || opts.rpm_metalink.is_some()
        || !opts.rpm_package_keys.is_empty()
        || opts.arch_keyring.is_some()
        || opts.arch_keyring_format.is_some()
        || !opts.arch_master_keys.is_empty()
        || opts.arch_packager_key_threshold.is_some()
        || opts.arch_database_signature.is_some()
    {
        bail!("Static repositories use TUF exclusively; native repository trust flags are invalid");
    }

    let identity_text =
        std::str::from_utf8(&identity_bytes).context("static repository identity is not UTF-8")?;
    let identity = RepoIdentity::parse(identity_text).context("parse conary-repo.toml")?;

    let root_bytes = location
        .fetch_bytes("metadata/root.json", MAX_STATIC_ROOT_SIZE)
        .await
        .context("fetch static repository root metadata")?;
    let signed_root = parse_verified_root(&root_bytes)?;
    let identity_root_key_ids = normalize_key_id_set(
        &identity.trust.root_key_ids,
        "conary-repo.toml trust.root_key_ids",
    )?;
    let root_role_key_ids = root_role_key_id_set(&signed_root)?;

    if identity_root_key_ids != root_role_key_ids {
        bail!(
            "conary-repo.toml trust.root_key_ids {} do not match root.json root role key IDs {}",
            format_key_set(&identity_root_key_ids),
            format_key_set(&root_role_key_ids)
        );
    }

    let supplied_fingerprints = normalize_fingerprints(&opts.fingerprints)?;
    if supplied_fingerprints.is_empty() {
        confirm_static_tofu(&identity, &root_role_key_ids, opts.yes)?;
    } else if supplied_fingerprints != root_role_key_ids {
        bail!(
            "Static repository fingerprint set {} does not match root role key IDs {}",
            format_key_set(&supplied_fingerprints),
            format_key_set(&root_role_key_ids)
        );
    }

    persist_static_repository(opts, &normalized_base, &root_bytes).await?;
    Ok(true)
}

pub async fn cmd_repo_reset_trust(name: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let mut repo = Repository::find_by_name(&conn, name)?
        .ok_or_else(|| anyhow!("Repository '{}' not found", name))?;
    let repo_id = repo
        .id
        .ok_or_else(|| anyhow!("Repository '{}' has no database ID", name))?;
    if repo.default_strategy.as_deref() != Some("static") {
        bail!("repo reset-trust is only supported for static repositories");
    }

    let tx = conn.unchecked_transaction()?;
    clear_static_repository_state(&tx, repo_id)?;
    repo.enabled = false;
    repo.tuf_enabled = false;
    repo.tuf_root_version = None;
    repo.last_sync = None;
    repo.update(&tx)?;
    tx.commit()?;

    println!("Reset static repository trust: {}", repo.name);
    println!("  Repository disabled until trust is re-established.");
    println!(
        "  Re-pin with: conary repo add {} {} --fingerprint <new-root-key-id> --replace",
        repo.name, repo.url
    );

    Ok(())
}

async fn persist_static_repository(
    opts: &RepoAddOptions,
    normalized_base: &str,
    root_bytes: &[u8],
) -> Result<()> {
    let conn = open_db(&opts.db_path)?;
    let existing = Repository::find_by_name(&conn, &opts.name)?;

    if existing.is_some() && !opts.replace {
        bail!(
            "Repository '{}' already exists.\nUse 'conary repo add {} {} --fingerprint <root-key-id> --replace' to re-pin static trust.",
            opts.name,
            opts.name,
            normalized_base
        );
    }

    let metadata_url = static_metadata_url(normalized_base);
    let tx = conn.unchecked_transaction()?;

    let (repo_id, repo) = if let Some(mut repo) = existing {
        let repo_id = repo
            .id
            .ok_or_else(|| anyhow!("Repository '{}' has no database ID", opts.name))?;
        clear_static_repository_state(&tx, repo_id)?;
        apply_static_repo_options(&mut repo, opts, normalized_base, &metadata_url);
        repo.update(&tx)?;
        (repo_id, repo)
    } else {
        let mut repo = Repository::new(opts.name.clone(), normalized_base.to_string());
        apply_static_repo_options(&mut repo, opts, normalized_base, &metadata_url);
        let repo_id = repo.insert(&tx)?;
        (repo_id, repo)
    };

    TufClient::new_static(repo_id, &repo.url, repo.tuf_root_url.as_deref())
        .map_err(|error| anyhow!(error))?
        .bootstrap(&tx, root_bytes)
        .map_err(|error| anyhow!(error))?;

    tx.commit()?;

    println!("Added static repository: {}", repo.name);
    println!("  Metadata URL: {}", repo.url);
    println!("  TUF Metadata URL: {}", metadata_url);
    println!("  Enabled: {}", repo.enabled);
    println!("  Priority: {}", repo.priority);
    println!("  Default Strategy: static");
    println!(
        "  Security Advisories: {}",
        repo.security_advisory_support.as_str()
    );

    Ok(())
}

fn apply_static_repo_options(
    repo: &mut Repository,
    opts: &RepoAddOptions,
    normalized_base: &str,
    metadata_url: &str,
) {
    repo.name = opts.name.clone();
    repo.url = normalized_base.to_string();
    repo.content_url = opts.content_url.clone();
    repo.enabled = !opts.disabled;
    repo.priority = opts.priority;
    repo.trust_policy = None;
    repo.default_strategy = Some("static".to_string());
    repo.default_strategy_endpoint = None;
    repo.default_strategy_distro = None;
    repo.tuf_enabled = true;
    repo.tuf_root_version = None;
    repo.tuf_root_url = Some(metadata_url.to_string());
    repo.security_advisory_support = opts.security_advisory_support;
    repo.last_sync = None;
}

fn clear_static_repository_state(conn: &Connection, repo_id: i64) -> Result<()> {
    RepositoryPackage::delete_by_repository(conn, repo_id)?;
    conn.execute(
        "DELETE FROM repository_package_keys WHERE repository_id = ?1",
        [repo_id],
    )?;
    conn.execute(
        "DELETE FROM tuf_targets WHERE repository_id = ?1",
        [repo_id],
    )?;
    conn.execute(
        "DELETE FROM tuf_metadata WHERE repository_id = ?1",
        [repo_id],
    )?;
    conn.execute("DELETE FROM tuf_keys WHERE repository_id = ?1", [repo_id])?;
    conn.execute("DELETE FROM tuf_roots WHERE repository_id = ?1", [repo_id])?;
    Ok(())
}

fn parse_verified_root(root_bytes: &[u8]) -> Result<Signed<RootMetadata>> {
    let signed_root: Signed<RootMetadata> =
        serde_json::from_slice(root_bytes).context("parse metadata/root.json")?;
    if signed_root.signed.type_field != "root" {
        bail!(
            "metadata/root.json type mismatch: expected root, got {}",
            signed_root.signed.type_field
        );
    }

    let (root_keys, root_threshold) =
        extract_role_keys(&signed_root.signed, Role::Root).map_err(|error| anyhow!(error))?;
    verify_signatures(&signed_root, Role::Root, &root_keys, root_threshold)
        .map_err(|error| anyhow!(error))?;
    verify_not_expired(Role::Root, &signed_root.signed.expires).map_err(|error| anyhow!(error))?;
    root_role_key_id_set(&signed_root)?;

    Ok(signed_root)
}

fn root_role_key_id_set(root: &Signed<RootMetadata>) -> Result<BTreeSet<String>> {
    let role = root
        .signed
        .roles
        .get("root")
        .ok_or_else(|| anyhow!("root.json missing root role definition"))?;
    for key_id in &role.keyids {
        if !root.signed.keys.contains_key(key_id) {
            bail!("root role references missing key ID {key_id}");
        }
    }
    normalize_key_id_set(&role.keyids, "root.json root role key ID")
}

fn normalize_fingerprints(fingerprints: &[String]) -> Result<BTreeSet<String>> {
    normalize_key_id_set(fingerprints, "--fingerprint")
}

fn normalize_key_id_set(values: &[String], label: &str) -> Result<BTreeSet<String>> {
    let mut normalized = BTreeSet::new();
    for value in values {
        let key_id = normalize_key_id(value, label)?;
        if !normalized.insert(key_id.clone()) {
            bail!("duplicate {label} value after normalization: {key_id}");
        }
    }
    Ok(normalized)
}

fn normalize_key_id(value: &str, label: &str) -> Result<String> {
    let normalized = value.to_ascii_lowercase();
    if normalized.len() != KEY_ID_HEX_LEN
        || !normalized
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        bail!("{label} must be a 64-character hex key ID");
    }
    Ok(normalized)
}

fn confirm_static_tofu(
    identity: &RepoIdentity,
    root_key_ids: &BTreeSet<String>,
    yes: bool,
) -> Result<()> {
    if is_non_interactive() {
        bail!(
            "Cannot establish static repository trust without --fingerprint in a non-interactive context"
        );
    }

    if yes {
        return Ok(());
    }

    let prompt = tofu_prompt_text(identity, root_key_ids);
    if prompt_for_tofu_acceptance(&prompt)? {
        Ok(())
    } else {
        bail!("Static repository trust was not confirmed")
    }
}

fn tofu_prompt_text(identity: &RepoIdentity, root_key_ids: &BTreeSet<String>) -> String {
    let description = identity
        .repo
        .description
        .as_deref()
        .unwrap_or("no description");
    format!(
        "Static repository: {}\nDescription: {}\nRoot key IDs: {}\n\n\
TOFU cannot detect a replayed old root whose keys were later rotated or compromised; \
an on-path attacker can pin a stale identity. Use --fingerprint from an out-of-band \
source for production trust establishment.",
        identity.repo.name,
        description,
        format_key_set(root_key_ids)
    )
}

fn prompt_for_tofu_acceptance(prompt: &str) -> Result<bool> {
    #[cfg(test)]
    if let Some(accept) = record_test_prompt(prompt) {
        return Ok(accept);
    }

    println!("{prompt}");
    print!("Trust this static repository root? Type 'yes' to continue: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim() == "yes")
}

fn is_non_interactive() -> bool {
    conary_non_interactive_env_is_enabled() || !stdin_is_interactive()
}

fn stdin_is_interactive() -> bool {
    #[cfg(test)]
    if let Some(interactive) = test_prompt_interactive_override() {
        return interactive;
    }

    io::stdin().is_terminal()
}

fn conary_non_interactive_env_is_enabled() -> bool {
    conary_non_interactive_env_is_enabled_for_value(
        std::env::var("CONARY_NON_INTERACTIVE").ok().as_deref(),
    )
}

fn conary_non_interactive_env_is_enabled_for_value(value: Option<&str>) -> bool {
    matches!(value, Some("1"))
}

fn normalize_static_repo_base(input: &str) -> Result<Option<String>> {
    if input.starts_with("http://") || input.starts_with("https://") {
        return Ok(Some(input.trim_end_matches('/').to_string()));
    }

    if let Some(path) = input.strip_prefix("file://") {
        return Ok(Some(format!(
            "file://{}",
            strip_trailing_path_slashes(path)
        )));
    }

    if has_url_scheme(input) {
        return Ok(None);
    }

    let current_dir = std::env::current_dir().context("determine current directory")?;
    normalize_static_repo_base_path(&current_dir, Path::new(input)).map(Some)
}

fn normalize_static_repo_base_path(current_dir: &Path, input: &Path) -> Result<String> {
    let path = if input.is_absolute() {
        PathBuf::from(input)
    } else {
        current_dir.join(input)
    };
    Ok(path.display().to_string())
}

fn static_metadata_url(base: &str) -> String {
    format!("{base}/metadata")
}

fn strip_trailing_path_slashes(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() { "/" } else { trimmed }
}

fn has_url_scheme(input: &str) -> bool {
    let Some(colon_index) = input.find(':') else {
        return false;
    };

    let scheme = &input[..colon_index];
    let mut bytes = scheme.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    first.is_ascii_alphabetic()
        && bytes.all(|byte| {
            matches!(
                byte,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.'
            )
        })
}

fn format_key_set(keys: &BTreeSet<String>) -> String {
    format!(
        "{{{}}}",
        keys.iter().cloned().collect::<Vec<_>>().join(", ")
    )
}

#[cfg(test)]
#[path = "repo_static/test_support.rs"]
mod test_support;
#[cfg(test)]
use test_support::{record_test_prompt, test_prompt_interactive_override};

#[cfg(test)]
#[path = "repo_static/tests.rs"]
mod tests;
