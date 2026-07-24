// conary/src/commands/system/init.rs

use super::*;

const REMI_REPOSITORY_NAME: &str = "remi";
const REMI_ENDPOINT: &str = "https://remi.conary.io";
pub(super) const HOST_PROFILE_SETTING: &str = "system.host-profile";
const MAX_INIT_SYMLINK_DEPTH: usize = 40;

#[derive(Clone, Copy)]
pub(super) struct NativeRepositorySeed {
    pub(super) name: &'static str,
    pub(super) url: &'static str,
    pub(super) legacy_urls: &'static [&'static str],
    pub(super) priority: i32,
    pub(super) description: &'static str,
}

impl NativeRepositorySeed {
    fn owns_url(self, url: &str) -> bool {
        url == self.url || self.legacy_urls.contains(&url)
    }
}

pub(super) const NATIVE_REPOSITORY_SEEDS: &[NativeRepositorySeed] = &[
    NativeRepositorySeed {
        name: "arch-core",
        url: "https://geo.mirror.pkgbuild.com/core/os/x86_64",
        legacy_urls: &[],
        priority: 100,
        description: "Arch Linux",
    },
    NativeRepositorySeed {
        name: "arch-extra",
        url: "https://geo.mirror.pkgbuild.com/extra/os/x86_64",
        legacy_urls: &[],
        priority: 95,
        description: "Arch Linux",
    },
    NativeRepositorySeed {
        name: "fedora-44",
        url: "https://dl.fedoraproject.org/pub/fedora/linux/releases/44/Everything/x86_64/os",
        legacy_urls: &[],
        priority: 90,
        description: "Fedora 44",
    },
    NativeRepositorySeed {
        name: "arch-multilib",
        url: "https://geo.mirror.pkgbuild.com/multilib/os/x86_64",
        legacy_urls: &[],
        priority: 85,
        description: "Arch Linux",
    },
    NativeRepositorySeed {
        name: "ubuntu-26.04",
        url: "https://archive.ubuntu.com/ubuntu",
        legacy_urls: &["http://archive.ubuntu.com/ubuntu"],
        priority: 80,
        description: "Ubuntu 26.04 LTS",
    },
];

/// Initialize the Conary database and add default repositories
pub async fn cmd_init(db_path: &str, profile_id: &str) -> Result<()> {
    let profile = profile_by_public_id(profile_id).ok_or_else(|| {
        let supported = conary_core::repository::supported_profiles::public_profiles()
            .iter()
            .map(SupportedProfile::id)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow!("unsupported host profile '{profile_id}'; expected one of: {supported}")
    })?;

    info!("Initializing Conary database at: {}", db_path);
    let db_path_ref = Path::new(db_path);
    let runtime_root = ConaryRuntimeRoot::from_db_path(db_path_ref.to_path_buf());
    require_init_privileges(db_path_ref)?;
    conary_core::db::init(db_path)
        .map_err(|err| init_failure_context(db_path_ref, &runtime_root, err))?;
    crate::ui::status("Initialized", &format!("database at {db_path}"));

    let mut conn = open_db(db_path)?;
    info!("Adding default repositories...");

    // Collect messages inside the transaction; print after commit to avoid
    // interleaving output with a potential rollback log.
    let mut messages: Vec<(bool, String)> = Vec::new();

    conary_core::db::transaction(&mut conn, |tx| {
        reconcile_remi_seed(tx, profile, &mut messages)?;
        reconcile_native_repository_seeds(tx, profile, &mut messages)?;
        conary_core::db::models::settings::set(tx, HOST_PROFILE_SETTING, profile.id())?;

        Ok(())
    })?;

    for (is_warning, msg) in &messages {
        if *is_warning {
            crate::ui::warn(msg);
        } else {
            crate::ui::row(crate::ui::Status::Ok, &[msg.trim()]);
        }
    }

    crate::ui::status(
        "Configured",
        &format!("repositories for profile {}", profile.id()),
    );
    crate::ui::note("Run 'conary repo sync remi' to download preview metadata.");
    crate::ui::note(
        "Native metadata sources stay disabled until their signing trust is configured.",
    );
    Ok(())
}

fn require_init_privileges(db_path: &Path) -> Result<()> {
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

fn reconcile_remi_seed(
    conn: &rusqlite::Connection,
    profile: &SupportedProfile,
    messages: &mut Vec<(bool, String)>,
) -> conary_core::Result<()> {
    let Some(mut repo) = Repository::find_by_name(conn, REMI_REPOSITORY_NAME)? else {
        let mut repo = Repository::new(REMI_REPOSITORY_NAME.to_string(), REMI_ENDPOINT.to_string());
        repo.priority = 110;
        repo.default_strategy = Some("remi".to_string());
        repo.default_strategy_endpoint = Some(REMI_ENDPOINT.to_string());
        repo.default_strategy_distro = Some(profile.id().to_string());
        repo.insert(conn)?;
        messages.push((false, "  Added: remi (Conary Remi (CCS))".to_string()));
        return Ok(());
    };

    let canonical_seed = repo.url == REMI_ENDPOINT
        && repo.default_strategy.as_deref() == Some("remi")
        && repo.default_strategy_endpoint.as_deref() == Some(REMI_ENDPOINT);
    if !canonical_seed {
        messages.push((
            true,
            "Existing repository 'remi' is user-managed; leaving its endpoint and target unchanged"
                .to_string(),
        ));
        return Ok(());
    }

    if repo.default_strategy_distro.as_deref() != Some(profile.id()) {
        let repo_id = repo.id.ok_or_else(|| {
            conary_core::Error::MissingId("Remi repository has no ID".to_string())
        })?;
        RepositoryPackage::delete_by_repository(conn, repo_id)?;
        PackageResolution::delete_by_repository(conn, repo_id)?;
        conn.execute(
            "DELETE FROM repository_package_keys WHERE repository_id = ?1",
            [repo_id],
        )?;
        repo.default_strategy_distro = Some(profile.id().to_string());
        repo.last_sync = None;
        repo.update(conn)?;
        messages.push((false, format!("  Updated: remi target ({})", profile.id())));
    }

    Ok(())
}

fn reconcile_native_repository_seeds(
    conn: &rusqlite::Connection,
    profile: &SupportedProfile,
    messages: &mut Vec<(bool, String)>,
) -> conary_core::Result<()> {
    let previous_profile = conary_core::db::models::settings::get(conn, HOST_PROFILE_SETTING)?;
    let profile_changed = previous_profile.as_deref() != Some(profile.id());

    for seed in NATIVE_REPOSITORY_SEEDS {
        let selected = profile.matches_repository_name(seed.name);
        let existing = Repository::find_by_name(conn, seed.name)?;

        match existing {
            None if selected => {
                conary_core::repository::add_repository(
                    conn,
                    seed.name.to_string(),
                    seed.url.to_string(),
                    false,
                    seed.priority,
                )?;
                messages.push((
                    false,
                    format!(
                        "  Added: {} ({}, disabled pending signing trust)",
                        seed.name, seed.description
                    ),
                ));
            }
            Some(mut repo) if seed.owns_url(&repo.url) => {
                let mut changed = false;
                if repo.url != seed.url {
                    let repo_id = repo.id.ok_or_else(|| {
                        conary_core::Error::MissingId(format!(
                            "Repository '{}' has no ID",
                            seed.name
                        ))
                    })?;
                    RepositoryPackage::delete_by_repository(conn, repo_id)?;
                    PackageResolution::delete_by_repository(conn, repo_id)?;
                    repo.url = seed.url.to_string();
                    repo.last_sync = None;
                    changed = true;
                    messages.push((
                        false,
                        format!(
                            "  Updated: {} secure endpoint ({})",
                            seed.name, seed.description
                        ),
                    ));
                }
                if profile_changed && repo.enabled {
                    repo.enabled = false;
                    changed = true;
                    messages.push((
                        false,
                        format!("  Disabled: {} ({})", seed.name, seed.description),
                    ));
                }
                if changed {
                    repo.update(conn)?;
                }
            }
            Some(repo) if selected && repo.url != seed.url => messages.push((
                true,
                format!(
                    "Existing repository '{}' is user-managed; leaving it unchanged",
                    seed.name
                ),
            )),
            _ => {}
        }
    }

    Ok(())
}

fn init_failure_context(
    db_path: &Path,
    runtime_root: &ConaryRuntimeRoot,
    source: conary_core::Error,
) -> anyhow::Error {
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    anyhow!(
        "could not initialize Conary database at {}: {}\n\
         database parent: {}\n\
         runtime root: {}\n\
         safe next step: verify the database parent is a writable directory, \
         or pass --db-path to a writable test location; do not remove existing \
         Conary runtime state unless you have confirmed it is disposable",
        db_path.display(),
        source,
        parent.display(),
        runtime_root.root().display()
    )
}
