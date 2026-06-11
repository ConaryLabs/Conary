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

    if opts.gpg_key.is_some() || opts.no_gpg_check || opts.gpg_strict {
        bail!("Static repositories use TUF exclusively; GPG flags are not supported");
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
    repo.gpg_check = false;
    repo.gpg_strict = false;
    repo.gpg_key_url = None;
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
#[derive(Clone)]
struct PromptOverride {
    interactive: bool,
    accept: bool,
    prompt: Option<String>,
}

#[cfg(test)]
thread_local! {
    static PROMPT_OVERRIDE: std::cell::RefCell<Option<PromptOverride>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
async fn with_static_repo_prompt_override<F, Fut, T>(
    interactive: bool,
    accept: bool,
    f: F,
) -> (T, Option<String>)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    PROMPT_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = Some(PromptOverride {
            interactive,
            accept,
            prompt: None,
        });
    });

    let output = f().await;
    let prompt =
        PROMPT_OVERRIDE.with(|cell| cell.borrow_mut().take().and_then(|state| state.prompt));
    (output, prompt)
}

#[cfg(test)]
fn test_prompt_interactive_override() -> Option<bool> {
    PROMPT_OVERRIDE.with(|cell| cell.borrow().as_ref().map(|state| state.interactive))
}

#[cfg(test)]
fn record_test_prompt(prompt: &str) -> Option<bool> {
    PROMPT_OVERRIDE.with(|cell| {
        let mut state = cell.borrow_mut();
        let state = state.as_mut()?;
        state.prompt = Some(prompt.to_string());
        Some(state.accept)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::future::Future;
    use std::path::Path;

    use clap::Parser;
    use conary_core::ccs::signing::SigningKeyPair;
    use conary_core::db::models::{
        Repository, RepositoryPackage, RepositoryPackageKey, RepositoryPackageKeyStatus,
        SecurityAdvisorySupport,
    };
    use conary_core::trust::ceremony::{create_initial_root, create_initial_root_single_key};
    use conary_core::trust::keys::{sign_tuf_metadata, signing_keypair_to_tuf_key};
    use conary_core::trust::metadata::{RootMetadata, Signed};
    use rusqlite::{Connection, params};

    use super::*;
    use crate::cli::{Cli, Commands, RepoCommands};
    use crate::commands::{RepoAddOptions, cmd_repo_add};

    const OTHER_VALID_KEY_ID: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";

    struct TestDb {
        _tempdir: tempfile::TempDir,
        db_path: String,
    }

    impl TestDb {
        fn new() -> Self {
            let tempdir = tempfile::tempdir().unwrap();
            let db_path = tempdir.path().join("conary.db");
            conary_core::db::init(&db_path).unwrap();
            Self {
                _tempdir: tempdir,
                db_path: db_path.to_string_lossy().to_string(),
            }
        }

        fn conn(&self) -> Connection {
            conary_core::db::open(&self.db_path).unwrap()
        }
    }

    struct StaticRepoFixture {
        _tempdir: tempfile::TempDir,
        root_key_ids: Vec<String>,
        base_url: String,
    }

    impl StaticRepoFixture {
        fn single_key(name: &str) -> Self {
            let root_key = SigningKeyPair::generate();
            let root = create_initial_root_single_key(&root_key, 365).unwrap();
            let root_key_ids = root_role_key_ids(&root);
            Self::from_root(name, Some("fixture static repo"), root_key_ids, root)
        }

        fn multi_root_key(name: &str) -> Self {
            let root_key = SigningKeyPair::generate();
            let second_root_key = SigningKeyPair::generate();
            let targets_key = SigningKeyPair::generate();
            let snapshot_key = SigningKeyPair::generate();
            let timestamp_key = SigningKeyPair::generate();

            let mut root =
                create_initial_root(&root_key, &targets_key, &snapshot_key, &timestamp_key, 365)
                    .unwrap();
            let (second_key_id, second_tuf_key) =
                signing_keypair_to_tuf_key(&second_root_key).unwrap();
            root.signed
                .keys
                .insert(second_key_id.clone(), second_tuf_key);
            root.signed
                .roles
                .get_mut("root")
                .unwrap()
                .keyids
                .push(second_key_id);
            root.signatures = vec![sign_tuf_metadata(&root_key, &root.signed).unwrap()];

            let root_key_ids = root_role_key_ids(&root);
            Self::from_root(name, Some("fixture static repo"), root_key_ids, root)
        }

        fn with_identity_root_ids(name: &str, identity_root_key_ids: Vec<String>) -> Self {
            let root_key = SigningKeyPair::generate();
            let root = create_initial_root_single_key(&root_key, 365).unwrap();
            Self::from_root(
                name,
                Some("fixture static repo"),
                identity_root_key_ids,
                root,
            )
        }

        fn with_relabelled_root_key(name: &str) -> Self {
            let victim_key = SigningKeyPair::generate();
            let attacker_key = SigningKeyPair::generate();
            let mut root = create_initial_root_single_key(&victim_key, 365).unwrap();
            let victim_key_id = root_role_key_ids(&root)[0].clone();
            let (_, attacker_tuf_key) = signing_keypair_to_tuf_key(&attacker_key).unwrap();
            root.signed
                .keys
                .insert(victim_key_id.clone(), attacker_tuf_key);
            let mut attacker_signature = sign_tuf_metadata(&attacker_key, &root.signed).unwrap();
            attacker_signature.keyid = victim_key_id.clone();
            root.signatures = vec![attacker_signature];

            Self::from_root(name, Some("fixture static repo"), vec![victim_key_id], root)
        }

        fn with_zero_root_threshold(name: &str) -> Self {
            let root_key = SigningKeyPair::generate();
            let mut root = create_initial_root_single_key(&root_key, 365).unwrap();
            root.signed.roles.get_mut("root").unwrap().threshold = 0;
            let root_key_ids = root_role_key_ids(&root);
            Self::from_root(name, Some("fixture static repo"), root_key_ids, root)
        }

        fn from_root(
            name: &str,
            description: Option<&str>,
            identity_root_key_ids: Vec<String>,
            root: Signed<RootMetadata>,
        ) -> Self {
            let tempdir = tempfile::tempdir().unwrap();
            let metadata_dir = tempdir.path().join("metadata");
            std::fs::create_dir_all(&metadata_dir).unwrap();
            std::fs::write(
                tempdir.path().join("conary-repo.toml"),
                repo_identity_toml(name, description, &identity_root_key_ids),
            )
            .unwrap();
            std::fs::write(
                metadata_dir.join("root.json"),
                serde_json::to_vec_pretty(&root).unwrap(),
            )
            .unwrap();
            let base_url = format!("file://{}", tempdir.path().display());
            Self {
                _tempdir: tempdir,
                root_key_ids: root_role_key_ids(&root),
                base_url,
            }
        }

        fn metadata_url(&self) -> String {
            format!("{}/metadata", self.base_url)
        }
    }

    fn repo_identity_toml(
        name: &str,
        description: Option<&str>,
        root_key_ids: &[String],
    ) -> String {
        let root_keys = root_key_ids
            .iter()
            .map(|key_id| format!("\"{key_id}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let description = description
            .map(|value| format!("description = \"{value}\"\n"))
            .unwrap_or_default();
        format!(
            "schema = 1\n[repo]\nname = \"{name}\"\n{description}[trust]\nroot_key_ids = [{root_keys}]\n"
        )
    }

    fn root_role_key_ids(root: &Signed<RootMetadata>) -> Vec<String> {
        root.signed.roles["root"].keyids.clone()
    }

    async fn add_static_repo(
        db: &TestDb,
        fixture: &StaticRepoFixture,
        fingerprints: Vec<String>,
    ) -> anyhow::Result<()> {
        add_static_repo_with(db, fixture, fingerprints, false, false, false, false).await
    }

    async fn add_static_repo_with(
        db: &TestDb,
        fixture: &StaticRepoFixture,
        fingerprints: Vec<String>,
        replace: bool,
        yes: bool,
        no_gpg_check: bool,
        gpg_strict: bool,
    ) -> anyhow::Result<()> {
        cmd_repo_add(RepoAddOptions {
            name: "acme".to_string(),
            url: fixture.base_url.clone(),
            db_path: db.db_path.clone(),
            content_url: None,
            priority: 50,
            disabled: false,
            gpg_key: None,
            no_gpg_check,
            gpg_strict,
            default_strategy: None,
            remi_endpoint: None,
            remi_distro: None,
            security_advisory_support: SecurityAdvisorySupport::Unknown,
            fingerprints,
            yes,
            replace,
        })
        .await
    }

    fn assert_no_repo(conn: &Connection, name: &str) {
        assert!(
            Repository::find_by_name(conn, name).unwrap().is_none(),
            "repository should not have been persisted"
        );
    }

    fn repo(conn: &Connection) -> Repository {
        Repository::find_by_name(conn, "acme").unwrap().unwrap()
    }

    fn stored_tuf_key_ids(conn: &Connection, repo_id: i64) -> BTreeSet<String> {
        conn.prepare("SELECT id FROM tuf_keys WHERE repository_id = ?1 ORDER BY id")
            .unwrap()
            .query_map([repo_id], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<BTreeSet<_>>>()
            .unwrap()
    }

    fn count_rows(conn: &Connection, table: &str, repo_id: i64) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE repository_id = ?1");
        conn.query_row(&sql, [repo_id], |row| row.get(0)).unwrap()
    }

    fn insert_synced_visibility(conn: &Connection, repo_id: i64) {
        let mut package = RepositoryPackage::new(
            repo_id,
            "acme-widget".to_string(),
            "1.0-1".to_string(),
            "abc".to_string(),
            42,
            "packages/acme-widget/acme-widget-1.0-1-x86_64.ccs".to_string(),
        );
        package.architecture = Some("x86_64".to_string());
        let package_id = package.insert(conn).unwrap();
        conn.execute(
            "INSERT INTO repository_provides (repository_package_id, capability, kind)
             VALUES (?1, ?2, 'package')",
            params![package_id, "acme-widget"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tuf_targets
             (repository_id, target_path, sha256, length, custom_json, targets_version)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params![repo_id, "packages/acme-widget.ccs", "abc", 42, 1],
        )
        .unwrap();
        RepositoryPackageKey::replace_for_repository(
            conn,
            repo_id,
            &[RepositoryPackageKey {
                repository_id: repo_id,
                public_key: "package-key".to_string(),
                key_id: Some("package-key-id".to_string()),
                status: RepositoryPackageKeyStatus::Active,
                synced_at: None,
            }],
        )
        .unwrap();
    }

    async fn with_prompt_override<F, Fut, T>(
        interactive: bool,
        accept: bool,
        f: F,
    ) -> (T, Option<String>)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        with_static_repo_prompt_override(interactive, accept, f).await
    }

    #[tokio::test]
    async fn fingerprint_mismatch_fails_before_insert() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::single_key("acme-static");

        let err = add_static_repo(&db, &fixture, vec![OTHER_VALID_KEY_ID.to_string()])
            .await
            .unwrap_err();

        assert!(err.to_string().contains("fingerprint"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[tokio::test]
    async fn single_key_fingerprint_exact_set_match_inserts_tuf_enabled_repo() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::single_key("acme-static");

        add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
            .await
            .unwrap();

        let conn = db.conn();
        let repo = repo(&conn);
        assert!(repo.tuf_enabled);
        assert_eq!(
            repo.tuf_root_url.as_deref(),
            Some(fixture.metadata_url().as_str())
        );
    }

    #[tokio::test]
    async fn multi_key_root_exact_set_fingerprint_match_inserts_tuf_enabled_repo() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::multi_root_key("acme-static");

        add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
            .await
            .unwrap();

        assert!(repo(&db.conn()).tuf_enabled);
    }

    #[tokio::test]
    async fn fingerprint_subset_fails_when_root_role_has_extra_key_ids() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::multi_root_key("acme-static");

        let err = add_static_repo(&db, &fixture, vec![fixture.root_key_ids[0].clone()])
            .await
            .unwrap_err();

        assert!(err.to_string().contains("fingerprint"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[tokio::test]
    async fn fingerprint_superset_fails_when_supplied_set_contains_unserved_key_id() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::single_key("acme-static");
        let mut fingerprints = fixture.root_key_ids.clone();
        fingerprints.push(OTHER_VALID_KEY_ID.to_string());

        let err = add_static_repo(&db, &fixture, fingerprints)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("fingerprint"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[tokio::test]
    async fn duplicate_fingerprints_after_normalization_fail_as_ambiguous_input() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::single_key("acme-static");
        let lower = fixture.root_key_ids[0].clone();
        let upper = lower.to_ascii_uppercase();

        let err = add_static_repo(&db, &fixture, vec![lower, upper])
            .await
            .unwrap_err();

        assert!(err.to_string().contains("duplicate"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[tokio::test]
    async fn inserted_static_repo_has_metadata_url_and_static_strategy() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::single_key("acme-static");

        add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
            .await
            .unwrap();

        let repo = repo(&db.conn());
        assert_eq!(
            repo.tuf_root_url.as_deref(),
            Some(fixture.metadata_url().as_str())
        );
        assert_eq!(repo.default_strategy.as_deref(), Some("static"));
    }

    #[tokio::test]
    async fn static_repo_add_rejects_gpg_flags_after_probe_without_fingerprint() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::single_key("acme-static");

        let err = add_static_repo_with(&db, &fixture, Vec::new(), false, false, true, false)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("GPG"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[test]
    fn manual_default_strategy_static_remains_rejected_at_parse_time() {
        assert!(
            Cli::try_parse_from([
                "conary",
                "repo",
                "add",
                "acme",
                "file:///tmp/repo",
                "--default-strategy",
                "static",
            ])
            .is_err()
        );
    }

    #[tokio::test]
    async fn non_interactive_tofu_fails_when_no_fingerprint_is_supplied() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::single_key("acme-static");

        let (result, _prompt) = with_prompt_override(false, true, || async {
            add_static_repo(&db, &fixture, Vec::new()).await
        })
        .await;

        let err = result.unwrap_err();
        assert!(err.to_string().contains("non-interactive"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[test]
    fn conary_non_interactive_env_one_is_non_interactive() {
        assert!(conary_non_interactive_env_is_enabled_for_value(Some("1")));
    }

    #[tokio::test]
    async fn interactive_tofu_prompt_includes_stale_root_replay_caveat() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::single_key("acme-static");

        let (result, prompt) = with_prompt_override(true, true, || async {
            add_static_repo(&db, &fixture, Vec::new()).await
        })
        .await;

        result.unwrap();
        let prompt = prompt.expect("interactive TOFU should render a prompt");
        assert!(prompt.contains("TOFU cannot detect a replayed old root"));
        assert!(prompt.contains("on-path attacker can pin a stale identity"));
    }

    #[tokio::test]
    async fn reset_trust_removes_static_trust_material_and_synced_package_visibility() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::single_key("acme-static");
        add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
            .await
            .unwrap();
        let conn = db.conn();
        let added_repo = repo(&conn);
        let repo_id = added_repo.id.unwrap();
        insert_synced_visibility(&conn, repo_id);
        drop(conn);

        cmd_repo_reset_trust("acme", &db.db_path).await.unwrap();

        let conn = db.conn();
        let repo = repo(&conn);
        assert!(!repo.enabled);
        assert!(!repo.tuf_enabled);
        assert_eq!(repo.tuf_root_version, None);
        assert_eq!(repo.default_strategy.as_deref(), Some("static"));
        assert_eq!(count_rows(&conn, "tuf_roots", repo_id), 0);
        assert_eq!(count_rows(&conn, "tuf_keys", repo_id), 0);
        assert_eq!(count_rows(&conn, "tuf_metadata", repo_id), 0);
        assert_eq!(count_rows(&conn, "tuf_targets", repo_id), 0);
        assert_eq!(count_rows(&conn, "repository_package_keys", repo_id), 0);
        assert_eq!(
            RepositoryPackage::find_by_repository(&conn, repo_id)
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn reset_trust_rejects_non_static_repositories_without_changing_visibility() {
        let db = TestDb::new();
        let conn = db.conn();
        let mut native = Repository::new(
            "acme".to_string(),
            "https://example.invalid/repo".to_string(),
        );
        native.default_strategy = Some("binary".to_string());
        let repo_id = native.insert(&conn).unwrap();
        insert_synced_visibility(&conn, repo_id);
        drop(conn);

        let err = cmd_repo_reset_trust("acme", &db.db_path).await.unwrap_err();

        let conn = db.conn();
        let repo = repo(&conn);
        assert!(err.to_string().contains("static repositories"));
        assert!(repo.enabled);
        assert_eq!(repo.default_strategy.as_deref(), Some("binary"));
        assert_eq!(
            RepositoryPackage::find_by_repository(&conn, repo_id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(count_rows(&conn, "repository_package_keys", repo_id), 1);
    }

    #[tokio::test]
    async fn duplicate_name_static_add_without_replace_fails_before_changing_existing_trust() {
        let db = TestDb::new();
        let first = StaticRepoFixture::single_key("acme-static");
        let second = StaticRepoFixture::single_key("acme-static");
        add_static_repo(&db, &first, first.root_key_ids.clone())
            .await
            .unwrap();
        let conn = db.conn();
        let repo_id = repo(&conn).id.unwrap();
        let before = stored_tuf_key_ids(&conn, repo_id);
        drop(conn);

        let err = add_static_repo(&db, &second, second.root_key_ids.clone())
            .await
            .unwrap_err();

        let conn = db.conn();
        assert!(err.to_string().contains("already exists"));
        assert_eq!(stored_tuf_key_ids(&conn, repo_id), before);
    }

    #[tokio::test]
    async fn duplicate_name_static_add_replace_updates_existing_row_and_bootstraps_new_root() {
        let db = TestDb::new();
        let first = StaticRepoFixture::single_key("acme-static");
        let second = StaticRepoFixture::single_key("acme-static");
        add_static_repo(&db, &first, first.root_key_ids.clone())
            .await
            .unwrap();
        let conn = db.conn();
        let repo_id = repo(&conn).id.unwrap();
        drop(conn);

        add_static_repo_with(
            &db,
            &second,
            second.root_key_ids.clone(),
            true,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        let conn = db.conn();
        let repo = repo(&conn);
        assert_eq!(repo.id, Some(repo_id));
        assert_eq!(repo.url, second.base_url);
        assert_eq!(
            stored_tuf_key_ids(&conn, repo_id),
            second.root_key_ids.iter().cloned().collect()
        );
    }

    #[tokio::test]
    async fn reset_then_repin_with_replace_reestablishes_trust_and_reenables_sync() {
        let db = TestDb::new();
        let first = StaticRepoFixture::single_key("acme-static");
        let second = StaticRepoFixture::single_key("acme-static");
        add_static_repo(&db, &first, first.root_key_ids.clone())
            .await
            .unwrap();
        cmd_repo_reset_trust("acme", &db.db_path).await.unwrap();

        add_static_repo_with(
            &db,
            &second,
            second.root_key_ids.clone(),
            true,
            false,
            false,
            false,
        )
        .await
        .unwrap();

        let conn = db.conn();
        let repo = repo(&conn);
        assert!(repo.enabled);
        assert!(repo.tuf_enabled);
        assert_eq!(repo.default_strategy.as_deref(), Some("static"));
        assert_eq!(
            repo.tuf_root_url.as_deref(),
            Some(second.metadata_url().as_str())
        );
        assert_eq!(
            stored_tuf_key_ids(&conn, repo.id.unwrap()),
            second.root_key_ids.iter().cloned().collect()
        );
    }

    #[tokio::test]
    async fn identity_root_key_mismatch_fails_before_insert() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::with_identity_root_ids(
            "acme-static",
            vec![OTHER_VALID_KEY_ID.into()],
        );

        let err = add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("conary-repo.toml"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[tokio::test]
    async fn relabelled_root_key_id_fails_before_insert() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::with_relabelled_root_key("acme-static");

        let err = add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Root key ID"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[tokio::test]
    async fn zero_root_threshold_fails_before_insert() {
        let db = TestDb::new();
        let fixture = StaticRepoFixture::with_zero_root_threshold("acme-static");

        let err = add_static_repo(&db, &fixture, fixture.root_key_ids.clone())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("threshold"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[tokio::test]
    async fn static_identity_probe_error_does_not_fall_back_to_native_add() {
        let db = TestDb::new();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("conary-repo.toml")).unwrap();
        let fixture = StaticRepoFixture {
            _tempdir: dir,
            root_key_ids: Vec::new(),
            base_url: String::new(),
        };
        let base_url = format!("file://{}", fixture._tempdir.path().display());

        let result = cmd_repo_add(RepoAddOptions {
            name: "acme".to_string(),
            url: base_url,
            db_path: db.db_path.clone(),
            content_url: None,
            priority: 50,
            disabled: false,
            gpg_key: None,
            no_gpg_check: false,
            gpg_strict: false,
            default_strategy: None,
            remi_endpoint: None,
            remi_distro: None,
            security_advisory_support: SecurityAdvisorySupport::Unknown,
            fingerprints: Vec::new(),
            yes: false,
            replace: false,
        })
        .await;

        let err = result.unwrap_err();
        assert!(err.to_string().contains("probe static repository identity"));
        assert_no_repo(&db.conn(), "acme");
    }

    #[test]
    fn repo_reset_trust_parse_shape_stays_routed_to_repo_command() {
        let cli = Cli::try_parse_from(["conary", "repo", "reset-trust", "acme"]).unwrap();

        assert!(matches!(
            cli.command,
            Some(Commands::Repo(RepoCommands::ResetTrust { .. }))
        ));
    }

    #[test]
    fn normalizes_bare_static_repo_path_to_absolute_storage_path() {
        let tempdir = tempfile::tempdir().unwrap();
        let normalized = normalize_static_repo_base_path(Path::new("."), tempdir.path()).unwrap();

        assert!(Path::new(&normalized).is_absolute());
    }
}
