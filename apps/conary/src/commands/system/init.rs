// conary/src/commands/system/init.rs

use super::*;
use anyhow::Context;
use conary_core::runtime_root::ConaryRuntimeRoot;

const MAX_INIT_SYMLINK_DEPTH: usize = 40;

/// Initialize the Conary database and add default repositories
pub async fn cmd_init(db_path: &str) -> Result<()> {
    info!("Initializing Conary database at: {}", db_path);
    let db_path_ref = Path::new(db_path);
    let runtime_root = ConaryRuntimeRoot::from_db_path(db_path_ref.to_path_buf());
    require_init_privileges(db_path_ref)?;
    conary_core::db::init(db_path)
        .map_err(|err| init_failure_context(db_path_ref, &runtime_root, err))?;
    crate::ui::status("Initialized", &format!("database at {db_path}"));

    configure_current_database(db_path).await
}

pub(super) async fn configure_current_database(db_path: &str) -> Result<()> {
    let mut conn = open_db(db_path)?;
    info!("Adding default repositories...");
    let host_capabilities = conary_core::ccs::HostCapabilityInventory::discover()
        .context("could not discover typed host capability inventory")?;

    // Collect messages inside the transaction; print after commit to avoid
    // interleaving output with a potential rollback log.
    let mut messages: Vec<(bool, String)> = Vec::new();

    conary_core::db::transaction(&mut conn, |tx| {
        host_capabilities.persist(tx).map_err(|error| {
            conary_core::Error::InitError(format!(
                "could not persist typed host capability inventory: {error}"
            ))
        })?;
        reconcile_remi_seeds(tx, &mut messages)?;

        Ok(())
    })?;

    for (is_warning, msg) in &messages {
        if *is_warning {
            crate::ui::warn(msg);
        } else {
            crate::ui::row(crate::ui::Status::Ok, &[msg.trim()]);
        }
    }

    crate::ui::status("Configured", "built-in Remi package source feeds");
    crate::ui::status(
        "Discovered",
        "typed host lifecycle interfaces (service manager, sysusers, tmpfiles, sysctl, ldconfig)",
    );
    crate::ui::note("Run 'conary repo sync' to download metadata from every enabled Remi feed.");
    crate::ui::note("Enroll native sources with exact trust, identity, stream, and update policy.");
    Ok(())
}

pub(super) fn require_init_privileges(db_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        validate_init_privileges(db_path, nix::unistd::Uid::effective().is_root())?;
    }
    Ok(())
}

pub(super) fn validate_init_privileges(db_path: &Path, is_root: bool) -> Result<()> {
    let default_db_path = ConaryRuntimeRoot::default().db_path().to_path_buf();
    if paths_refer_to_same_location(db_path, &default_db_path)? {
        let absolute_input = if db_path.is_absolute() {
            db_path.to_path_buf()
        } else {
            std::env::current_dir()?.join(db_path)
        };
        if lexically_normalize_path(&absolute_input) != default_db_path {
            return Err(anyhow!(
                "the system database must be addressed by its canonical path {}; refusing alias {} so database and runtime state cannot diverge",
                default_db_path.display(),
                db_path.display()
            ));
        }
        if !is_root {
            return Err(anyhow!(
                "initializing the system database at {} requires root privileges; re-run with sudo, or pass --db-path to an isolated writable database for source-build and test workflows",
                db_path.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn paths_refer_to_same_location(left: &Path, right: &Path) -> Result<bool> {
    Ok(resolve_path_for_comparison(left, 0)? == resolve_path_for_comparison(right, 0)?)
}

fn resolve_path_for_comparison(path: &Path, symlink_depth: usize) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let normalized = lexically_normalize_path(&absolute);

    if let Ok(canonical) = std::fs::canonicalize(&normalized) {
        return Ok(canonical);
    }

    // `canonicalize` cannot follow a dangling final symlink. Resolve that link
    // explicitly so a non-root caller cannot spell the system DB through a
    // writable alias before the target exists.
    if std::fs::symlink_metadata(&normalized)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        if symlink_depth >= MAX_INIT_SYMLINK_DEPTH {
            return Err(anyhow!(
                "refusing to initialize through a symlink chain deeper than {MAX_INIT_SYMLINK_DEPTH}: {}",
                path.display()
            ));
        }
        let target = std::fs::read_link(&normalized).map_err(|error| {
            anyhow!(
                "could not safely resolve database symlink {}: {error}",
                normalized.display()
            )
        })?;
        let target = if target.is_absolute() {
            target
        } else {
            normalized
                .parent()
                .unwrap_or_else(|| Path::new("/"))
                .join(target)
        };
        return resolve_path_for_comparison(&target, symlink_depth + 1);
    }

    if let (Some(parent), Some(file_name)) = (normalized.parent(), normalized.file_name())
        && let Ok(canonical_parent) = std::fs::canonicalize(parent)
    {
        return Ok(canonical_parent.join(file_name));
    }

    Ok(normalized)
}

fn lexically_normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = normalized
                    .file_name()
                    .is_some_and(|name| name != std::ffi::OsStr::new(".."));
                if can_pop {
                    normalized.pop();
                } else if !path.is_absolute() {
                    normalized.push("..");
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn reconcile_remi_seeds(
    conn: &rusqlite::Transaction<'_>,
    messages: &mut Vec<(bool, String)>,
) -> conary_core::Result<()> {
    let endpoint = conary_core::repository::remi_authority::canonical_remi_endpoint();
    let universe_root =
        conary_core::repository::remi_authority::canonical_remi_universe_root(endpoint)
            .ok_or_else(|| {
                conary_core::Error::InitError(
                    "canonical Remi authority is missing its signed universe root".to_string(),
                )
            })?;
    if matches!(
        conary_core::repository::universe::enroll_remi_universe_root(
            conn,
            endpoint,
            universe_root,
        )?,
        conary_core::repository::universe::RemiUniverseEnrollmentOutcome::Enrolled
    ) {
        messages.push((
            false,
            "  Enrolled: canonical Remi signed universe authority".to_string(),
        ));
    }
    for (index, feed) in conary_core::repository::supported_profiles::public_profiles()
        .iter()
        .enumerate()
    {
        let name = format!("remi-{}", feed.id());
        let Some(mut repo) = Repository::find_by_name(conn, &name)? else {
            let mut repo = Repository::new(name.clone(), endpoint.to_string());
            repo.priority = 110 - i32::try_from(index).unwrap_or(0);
            repo.default_strategy = Some("remi".to_string());
            repo.default_strategy_endpoint = Some(endpoint.to_string());
            repo.source_profile = Some(feed.id().to_string());
            repo.set_parser_config(conary_core::repository::RepositoryParserConfig::Json)?;
            let repo_id = repo.insert(conn)?;
            let authority = canonical_remi_authority_rows(repo_id, feed)?;
            RepositoryPackageKey::reconcile_for_repository_in_transaction(
                conn, repo_id, &authority,
            )?;
            messages.push((
                false,
                format!(
                    "  Added: {name} (Conary Remi, {} source)",
                    feed.display_name()
                ),
            ));
            continue;
        };

        let canonical_seed = repo.url == endpoint
            && repo.default_strategy.as_deref() == Some("remi")
            && repo.default_strategy_endpoint.as_deref() == Some(endpoint);
        if !canonical_seed {
            messages.push((
                true,
                format!(
                    "Existing repository '{name}' is user-managed; leaving its endpoint and source feed unchanged"
                ),
            ));
            continue;
        }

        let parser_config = conary_core::repository::RepositoryParserConfig::Json;
        if repo.source_profile.as_deref() != Some(feed.id())
            || repo.parser_config.as_ref() != Some(&parser_config)
        {
            let repo_id = repo.id.ok_or_else(|| {
                conary_core::Error::MissingId(format!("Remi repository '{name}' has no ID"))
            })?;
            RepositoryPackage::delete_by_repository(conn, repo_id)?;
            PackageResolution::delete_by_repository(conn, repo_id)?;
            repo.source_profile = Some(feed.id().to_string());
            repo.set_parser_config(parser_config)?;
            repo.last_checked_at = None;
            repo.last_changed_at = None;
            repo.last_validated_at = None;
            repo.last_published_at = None;
            repo.update(conn)?;
            messages.push((false, format!("  Updated: {name} source contract")));
        }
        let repo_id = repo.id.ok_or_else(|| {
            conary_core::Error::MissingId(format!("Remi repository '{name}' has no ID"))
        })?;
        let authority = canonical_remi_authority_rows(repo_id, feed)?;
        if RepositoryPackageKey::reconcile_for_repository_in_transaction(conn, repo_id, &authority)?
        {
            messages.push((false, format!("  Updated: {name} CCS package authority")));
        }
    }

    Ok(())
}

fn canonical_remi_authority_rows(
    repository_id: i64,
    profile: &conary_core::repository::supported_profiles::SupportedProfile,
) -> conary_core::Result<Vec<RepositoryPackageKey>> {
    let endpoint = conary_core::repository::remi_authority::canonical_remi_endpoint();
    let keys = conary_core::repository::remi_authority::canonical_remi_package_keys(
        endpoint,
        profile.id(),
    )
    .ok_or_else(|| {
        conary_core::Error::InitError(format!(
            "canonical Remi authority is missing exact profile '{}'",
            profile.id()
        ))
    })?;
    Ok(keys
        .iter()
        .map(|key| RepositoryPackageKey {
            repository_id,
            public_key: key.public_key.clone(),
            key_id: key.key_id.clone(),
            status: match key.status {
                conary_core::repository::PackageKeyStatus::Active => {
                    RepositoryPackageKeyStatus::Active
                }
                conary_core::repository::PackageKeyStatus::Retired => {
                    RepositoryPackageKeyStatus::Retired
                }
            },
            synced_at: None,
        })
        .collect())
}

fn init_failure_context(
    db_path: &Path,
    runtime_root: &ConaryRuntimeRoot,
    source: conary_core::Error,
) -> anyhow::Error {
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    let safe_next_step = if matches!(&source, conary_core::Error::SchemaRebuildRequired { .. }) {
        "run `conary system rebuild-db --discard-state --yes` with the same --db-path to preserve a retired snapshot and replace the active database; do not run it unless the active Conary state is disposable"
    } else {
        "verify the database parent is a writable directory, or pass --db-path to a writable test location; do not remove existing Conary runtime state unless you have confirmed it is disposable"
    };
    anyhow!(
        "could not initialize Conary database at {}: {}\n\
         database parent: {}\n\
         runtime root: {}\n\
         safe next step: {}",
        db_path.display(),
        source,
        parent.display(),
        runtime_root.root().display(),
        safe_next_step
    )
}
