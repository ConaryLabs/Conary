// src/commands/repo.rs
//! Repository management commands

mod authority;
#[cfg(test)]
mod authority_tests;
mod options;
mod trust_display;

pub use options::RepoAddOptions;

use super::open_db;
use anyhow::{Context, Result};
use conary_core::db::models::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, RepositoryPackageKey,
    RepositoryPolicyScope, RepositorySourcePolicy, RepositoryUpdateMode,
};
use conary_core::repository::{
    ArchKeyringFormat, ArchKeyringTrust, ArchSigLevel, ArchSignatureRequirement, OpenPgpTrustRoot,
    RepositoryFormat, RepositoryParserConfig, RepositoryTrustPolicy,
};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use tracing::info;

use authority::resolve_ccs_package_authority;

/// Add a new repository
pub async fn cmd_repo_add(opts: RepoAddOptions) -> Result<()> {
    info!("Adding repository: {} ({})", opts.name, opts.url);

    // Validate Remi strategy configuration before any static-repository probe.
    if let Some(ref strategy) = opts.default_strategy
        && strategy == "remi"
        && opts.remi_endpoint.is_none()
    {
        anyhow::bail!("--remi-endpoint is required when --default-strategy=remi");
    }
    if !opts.ccs_package_keys.is_empty() && opts.default_strategy.as_deref() != Some("remi") {
        anyhow::bail!("--ccs-package-key requires --default-strategy=remi");
    }

    let supplied_profile = opts
        .source_profile
        .as_deref()
        .map(|source_profile| {
            conary_core::repository::supported_profiles::profile_by_public_id(source_profile)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "unsupported source profile '{source_profile}'; use an exact public profile ID"
                    )
                })
        })
        .transpose()?;

    if opts.package_format.is_none()
        && (opts.source_id.is_some()
            || opts.repository_id.is_some()
            || opts.stream_kind.is_some()
            || opts.stream_id.is_some()
            || opts.policy_group.is_some()
            || opts.follow
            || opts.pin_snapshot_sha256.is_some())
    {
        anyhow::bail!(
            "native source policy options require an explicit rpm, deb, arch, or eopkg package format"
        );
    }

    if super::repo_static::try_cmd_repo_add_static(&opts).await? {
        return Ok(());
    }

    if !opts.fingerprints.is_empty() {
        anyhow::bail!("--fingerprint is only supported for static repositories");
    }

    let package_format = opts.package_format.ok_or_else(|| {
        anyhow::anyhow!(
            "--package-format is required for non-static repositories; choose rpm, deb, arch, eopkg, or json"
        )
    })?;
    if let Some(profile) = supplied_profile {
        let format_matches = matches!(
            (package_format, profile.package_format()),
            (
                RepositoryFormat::Fedora,
                conary_core::repository::supported_profiles::ProfilePackageFormat::Rpm
            ) | (
                RepositoryFormat::Debian,
                conary_core::repository::supported_profiles::ProfilePackageFormat::Deb
            ) | (
                RepositoryFormat::Arch,
                conary_core::repository::supported_profiles::ProfilePackageFormat::Arch
            ) | (
                RepositoryFormat::Eopkg,
                conary_core::repository::supported_profiles::ProfilePackageFormat::Eopkg
            ) | (RepositoryFormat::Json, _)
        );
        if !format_matches {
            anyhow::bail!(
                "source profile '{}' uses '{}' packages, not '{}' metadata",
                profile.id(),
                profile.package_format().as_str(),
                package_format.as_str()
            );
        }
    }
    let profile_id = supplied_profile.map(|profile| profile.id().to_string());
    let package_authority = if opts.default_strategy.as_deref() == Some("remi") {
        let profile_id = profile_id.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "--source-profile is required for Remi repositories; use an exact public profile ID"
            )
        })?;
        resolve_ccs_package_authority(
            opts.default_strategy.as_deref(),
            opts.remi_endpoint.as_deref(),
            profile_id,
            &opts.ccs_package_keys,
        )?
    } else {
        Vec::new()
    };
    let parser_config = exact_parser_config(
        package_format,
        opts.distribution,
        opts.component,
        opts.architecture,
        opts.database,
    )?;
    let trust_policy = exact_trust_policy(ExactTrustPolicyInput {
        package_format,
        debian_release_keys: opts.debian_release_keys,
        rpm_metadata_keys: opts.rpm_metadata_keys,
        rpm_metalink: opts.rpm_metalink,
        rpm_package_keys: opts.rpm_package_keys,
        arch_keyring: opts.arch_keyring,
        arch_keyring_format: opts.arch_keyring_format,
        arch_master_keys: opts.arch_master_keys,
        arch_packager_key_threshold: opts.arch_packager_key_threshold,
        arch_database_signature: opts.arch_database_signature,
        eopkg_origin: format!("{}/", opts.url.trim_end_matches('/')),
    })?;

    // Create the repository with all settings
    let mut repo = conary_core::db::models::Repository::new(opts.name, opts.url);
    repo.set_parser_config(parser_config)?;
    if let Some(policy) = trust_policy {
        repo.set_trust_policy(policy)?;
    }
    repo.content_url = opts.content_url;
    repo.enabled = !opts.disabled;
    repo.priority = opts.priority;
    repo.default_strategy = opts.default_strategy;
    repo.default_strategy_endpoint = opts.remi_endpoint;
    repo.source_profile = profile_id;
    repo.security_advisory_support = opts.security_advisory_support;

    let native_format = matches!(
        package_format,
        RepositoryFormat::Arch
            | RepositoryFormat::Debian
            | RepositoryFormat::Fedora
            | RepositoryFormat::Eopkg
    );
    if native_format {
        let source_identity = opts
            .source_id
            .ok_or_else(|| anyhow::anyhow!("--source-id is required for native repositories"))?;
        let repository_identity = opts.repository_id.ok_or_else(|| {
            anyhow::anyhow!("--repository-id is required for native repositories")
        })?;
        let stream_kind = opts
            .stream_kind
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--stream-kind is required for native repositories"))?;
        let stream_identity = opts
            .stream_id
            .ok_or_else(|| anyhow::anyhow!("--stream-id is required for native repositories"))?;
        let stream = match stream_kind {
            "release" => NativeSourceStream::release(stream_identity)?,
            "channel" => NativeSourceStream::channel(stream_identity)?,
            "rolling" => NativeSourceStream::rolling(stream_identity)?,
            other => anyhow::bail!("unsupported native stream kind '{other}'"),
        };
        let (update_mode, pinned_snapshot) = match (
            opts.follow,
            opts.pin_snapshot_sha256
                .map(AuthenticatedSnapshotIdentity::from_sha256),
        ) {
            (true, None) => (RepositoryUpdateMode::Follow, None),
            (false, Some(snapshot)) => (RepositoryUpdateMode::Pin, Some(snapshot?)),
            (false, None) => anyhow::bail!(
                "exactly one of --follow or --pin-snapshot-sha256 is required for native repositories"
            ),
            (true, Some(_)) => {
                anyhow::bail!("--follow and --pin-snapshot-sha256 are mutually exclusive")
            }
        };
        let scope = match opts.policy_group {
            Some(group) => RepositoryPolicyScope::group(group)?,
            None => RepositoryPolicyScope::repository(repository_identity.clone())?,
        };
        let ecosystem = NativeSourceEcosystem::from_repository_format(package_format)?;
        let policy =
            RepositorySourcePolicy::new(source_identity, scope, ecosystem, stream, update_mode)?;
        repo.set_native_source_policy(policy, repository_identity, pinned_snapshot)?;
    } else if opts.source_id.is_some()
        || opts.repository_id.is_some()
        || opts.stream_kind.is_some()
        || opts.stream_id.is_some()
        || opts.policy_group.is_some()
        || opts.follow
        || opts.pin_snapshot_sha256.is_some()
    {
        anyhow::bail!("native source policy options require rpm, deb, arch, or eopkg metadata");
    }

    let db_path = opts.db_path;
    let mut conn = open_db(&db_path)?;
    let tx = conn.transaction()?;
    if opts.replace
        && let Some(existing) = conary_core::db::models::Repository::find_by_name(&tx, &repo.name)?
    {
        let existing_id = existing.id.context("persisted repository has no ID")?;
        conary_core::db::models::Repository::delete(&tx, existing_id)?;
    }

    if let Err(error) = repo.insert(&tx) {
        if matches!(&error, conary_core::Error::ConflictError(_)) {
            return Err(anyhow::Error::new(conary_core::Error::ConflictError(
                format!(
                    "Repository '{}' already exists.\nUse 'conary repo list' to see configured repositories.",
                    repo.name
                ),
            )));
        }
        return Err(anyhow::anyhow!(
            "Failed to add repository '{}': {}",
            repo.name,
            error
        ));
    }
    let repository_id = repo
        .id
        .context("new repository has no ID after successful insertion")?;
    let package_authority_rows = package_authority
        .iter()
        .map(|key| RepositoryPackageKey {
            repository_id,
            public_key: key.public_key.clone(),
            key_id: key.key_id.clone(),
            status: key.status.clone(),
            synced_at: None,
        })
        .collect::<Vec<_>>();
    if !package_authority_rows.is_empty() {
        RepositoryPackageKey::reconcile_for_repository_in_transaction(
            &tx,
            repository_id,
            &package_authority_rows,
        )
        .context("persist verified CCS package authority")?;
    }
    tx.commit()?;

    println!("Added repository: {}", repo.name);
    println!("  Metadata URL: {}", repo.url);
    if let Some(ref content) = repo.content_url {
        println!("  Content URL: {} (reference mirror)", content);
    }
    println!("  Enabled: {}", repo.enabled);
    println!("  Priority: {}", repo.priority);
    if let Some(source_profile) = repo.source_profile.as_deref() {
        println!("  Source Profile: {source_profile}");
    }
    if let (Some(policy), Some(repository_identity)) = (
        repo.source_policy.as_ref(),
        repo.repository_identity.as_deref(),
    ) {
        println!("  Source Identity: {}", policy.source_identity);
        println!("  Repository Identity: {repository_identity}");
        println!("  Update Policy: {}", policy.update_mode.as_str());
    }
    if let Some(policy) = repo.trust_policy.as_ref() {
        println!("  Repository Trust: {}", trust_display::describe(policy));
    } else if !package_authority_rows.is_empty() {
        println!(
            "  Repository Trust: {} pinned CCS package key(s)",
            package_authority_rows.len()
        );
    } else {
        println!("  Repository Trust: typed JSON/Remi authority");
    }
    println!(
        "  Security Advisories: {}",
        repo.security_advisory_support.as_str()
    );
    // Show default strategy if configured
    if let Some(ref strategy) = repo.default_strategy {
        println!("  Default Strategy: {}", strategy);
        if strategy == "remi"
            && let Some(ref endpoint) = repo.default_strategy_endpoint
        {
            println!("  Remi Endpoint: {}", endpoint);
        }
    }

    Ok(())
}

struct ExactTrustPolicyInput {
    package_format: RepositoryFormat,
    debian_release_keys: Vec<OpenPgpTrustRoot>,
    rpm_metadata_keys: Vec<OpenPgpTrustRoot>,
    rpm_metalink: Option<String>,
    rpm_package_keys: Vec<OpenPgpTrustRoot>,
    arch_keyring: Option<String>,
    arch_keyring_format: Option<ArchKeyringFormat>,
    arch_master_keys: Vec<String>,
    arch_packager_key_threshold: Option<usize>,
    arch_database_signature: Option<ArchSignatureRequirement>,
    eopkg_origin: String,
}

fn exact_trust_policy(input: ExactTrustPolicyInput) -> Result<Option<RepositoryTrustPolicy>> {
    let ExactTrustPolicyInput {
        package_format,
        debian_release_keys,
        rpm_metadata_keys,
        rpm_metalink,
        rpm_package_keys,
        arch_keyring,
        arch_keyring_format,
        arch_master_keys,
        arch_packager_key_threshold,
        arch_database_signature,
        eopkg_origin,
    } = input;
    let reject = |flag: &str, present: bool| -> Result<()> {
        if present {
            anyhow::bail!("{flag} is not valid for {}", package_format.as_str());
        }
        Ok(())
    };

    let policy = match package_format {
        RepositoryFormat::Debian => {
            reject("--rpm-metadata-key", !rpm_metadata_keys.is_empty())?;
            reject("--rpm-metalink", rpm_metalink.is_some())?;
            reject("--rpm-package-key", !rpm_package_keys.is_empty())?;
            reject("--arch-keyring", arch_keyring.is_some())?;
            reject("--arch-keyring-format", arch_keyring_format.is_some())?;
            reject("--arch-master-key", !arch_master_keys.is_empty())?;
            reject(
                "--arch-packager-key-threshold",
                arch_packager_key_threshold.is_some(),
            )?;
            reject(
                "--arch-database-signature",
                arch_database_signature.is_some(),
            )?;
            Some(RepositoryTrustPolicy::Debian {
                release_keys: debian_release_keys,
            })
        }
        RepositoryFormat::Fedora => {
            reject("--debian-release-key", !debian_release_keys.is_empty())?;
            reject("--arch-keyring", arch_keyring.is_some())?;
            reject("--arch-keyring-format", arch_keyring_format.is_some())?;
            reject("--arch-master-key", !arch_master_keys.is_empty())?;
            reject(
                "--arch-packager-key-threshold",
                arch_packager_key_threshold.is_some(),
            )?;
            reject(
                "--arch-database-signature",
                arch_database_signature.is_some(),
            )?;
            let metadata = match rpm_metalink {
                Some(url) => {
                    if !rpm_metadata_keys.is_empty() {
                        anyhow::bail!(
                            "--rpm-metalink and --rpm-metadata-key are mutually exclusive"
                        );
                    }
                    conary_core::repository::RpmMetadataAuthority::Metalink { url }
                }
                None => conary_core::repository::RpmMetadataAuthority::OpenPgp {
                    keys: rpm_metadata_keys,
                },
            };
            Some(RepositoryTrustPolicy::Rpm {
                metadata,
                package_keys: rpm_package_keys,
            })
        }
        RepositoryFormat::Arch => {
            reject("--debian-release-key", !debian_release_keys.is_empty())?;
            reject("--rpm-metadata-key", !rpm_metadata_keys.is_empty())?;
            reject("--rpm-metalink", rpm_metalink.is_some())?;
            reject("--rpm-package-key", !rpm_package_keys.is_empty())?;
            let keyring_url = arch_keyring.ok_or_else(|| {
                anyhow::anyhow!("--arch-keyring is required for --package-format=arch")
            })?;
            let keyring_format = arch_keyring_format.ok_or_else(|| {
                anyhow::anyhow!("--arch-keyring-format is required for --package-format=arch")
            })?;
            let packager_key_threshold = arch_packager_key_threshold.ok_or_else(|| {
                anyhow::anyhow!(
                    "--arch-packager-key-threshold is required for --package-format=arch"
                )
            })?;
            Some(RepositoryTrustPolicy::Arch {
                keyring: ArchKeyringTrust {
                    url: keyring_url,
                    format: keyring_format,
                    master_fingerprints: arch_master_keys,
                    packager_key_threshold,
                },
                sig_level: ArchSigLevel {
                    database: arch_database_signature.unwrap_or(ArchSignatureRequirement::Optional),
                    package: ArchSignatureRequirement::Required,
                    trust: conary_core::repository::ArchTrustLevel::TrustedOnly,
                },
            })
        }
        RepositoryFormat::Eopkg => {
            reject("--debian-release-key", !debian_release_keys.is_empty())?;
            reject("--rpm-metadata-key", !rpm_metadata_keys.is_empty())?;
            reject("--rpm-metalink", rpm_metalink.is_some())?;
            reject("--rpm-package-key", !rpm_package_keys.is_empty())?;
            reject("--arch-keyring", arch_keyring.is_some())?;
            reject("--arch-keyring-format", arch_keyring_format.is_some())?;
            reject("--arch-master-key", !arch_master_keys.is_empty())?;
            reject(
                "--arch-packager-key-threshold",
                arch_packager_key_threshold.is_some(),
            )?;
            reject(
                "--arch-database-signature",
                arch_database_signature.is_some(),
            )?;
            Some(RepositoryTrustPolicy::Eopkg {
                origin: eopkg_origin,
            })
        }
        RepositoryFormat::Json => {
            reject("--debian-release-key", !debian_release_keys.is_empty())?;
            reject("--rpm-metadata-key", !rpm_metadata_keys.is_empty())?;
            reject("--rpm-metalink", rpm_metalink.is_some())?;
            reject("--rpm-package-key", !rpm_package_keys.is_empty())?;
            reject("--arch-keyring", arch_keyring.is_some())?;
            reject("--arch-keyring-format", arch_keyring_format.is_some())?;
            reject("--arch-master-key", !arch_master_keys.is_empty())?;
            reject(
                "--arch-packager-key-threshold",
                arch_packager_key_threshold.is_some(),
            )?;
            reject(
                "--arch-database-signature",
                arch_database_signature.is_some(),
            )?;
            None
        }
        RepositoryFormat::Unspecified => {
            anyhow::bail!("repository package format must be explicit")
        }
    };
    if let Some(policy) = policy.as_ref() {
        policy.validate()?;
    }
    Ok(policy)
}

fn exact_parser_config(
    package_format: RepositoryFormat,
    distribution: Option<String>,
    component: Option<String>,
    architecture: Option<String>,
    database: Option<String>,
) -> Result<RepositoryParserConfig> {
    let reject = |flag: &str, present: bool| -> Result<()> {
        if present {
            anyhow::bail!("{flag} is not valid for {}", package_format.as_str());
        }
        Ok(())
    };
    let required = |flag: &str, value: Option<String>| {
        value.ok_or_else(|| {
            anyhow::anyhow!(
                "{flag} is required for --package-format={}",
                package_format.as_str()
            )
        })
    };

    let config = match package_format {
        RepositoryFormat::Fedora => {
            reject("--distribution", distribution.is_some())?;
            reject("--component", component.is_some())?;
            reject("--database", database.is_some())?;
            RepositoryParserConfig::Rpm {
                architecture: required("--architecture", architecture)?,
            }
        }
        RepositoryFormat::Debian => {
            reject("--database", database.is_some())?;
            RepositoryParserConfig::Deb {
                distribution: required("--distribution", distribution)?,
                component: required("--component", component)?,
                architecture: required("--architecture", architecture)?,
            }
        }
        RepositoryFormat::Arch => {
            reject("--distribution", distribution.is_some())?;
            reject("--component", component.is_some())?;
            reject("--architecture", architecture.is_some())?;
            RepositoryParserConfig::Arch {
                database: required("--database", database)?,
            }
        }
        RepositoryFormat::Eopkg => {
            reject("--distribution", distribution.is_some())?;
            reject("--component", component.is_some())?;
            reject("--database", database.is_some())?;
            RepositoryParserConfig::Eopkg {
                architecture: required("--architecture", architecture)?,
            }
        }
        RepositoryFormat::Json => {
            reject("--distribution", distribution.is_some())?;
            reject("--component", component.is_some())?;
            reject("--architecture", architecture.is_some())?;
            reject("--database", database.is_some())?;
            RepositoryParserConfig::Json
        }
        RepositoryFormat::Unspecified => {
            anyhow::bail!("repository package format must be explicit")
        }
    };
    config.validate()?;
    Ok(config)
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
            println!(
                "      security advisories: {}",
                repo.security_advisory_support.as_str()
            );
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

    let spinner_style = ProgressStyle::default_spinner()
        .template("  {spinner:.cyan} {msg}")
        .expect("Invalid spinner template");

    let mut results: Vec<(String, conary_core::Result<usize>)> = Vec::new();
    for repo in &repos_needing_sync {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(spinner_style.clone());
        spinner.enable_steady_tick(Duration::from_millis(100));
        spinner.set_message(format!("Syncing {}...", repo.name));

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

        results.push((repo.name.clone(), sync_result));
    }

    let mut failures = Vec::new();

    for (name, result) in results {
        match result {
            Ok(count) => {
                let row = format!("Synchronized {count} packages from {name}");
                crate::ui::row(crate::ui::Status::Ok, &[&row]);
            }
            Err(e) => {
                let row = format!("Failed to sync {name}: {e}");
                crate::ui::row(crate::ui::Status::Fail, &[&row]);
                failures.push((name, e.to_string()));
            }
        }
    }

    // Canonical authority comes only from explicitly configured Remi
    // strategy endpoints. Ordered endpoint failover is transport behavior;
    // failure of every declared authority fails the sync.
    {
        let endpoints: Vec<String> = conn
            .prepare(
                "SELECT default_strategy_endpoint
                 FROM repositories
                 WHERE enabled = 1
                   AND default_strategy = 'remi'
                   AND default_strategy_endpoint IS NOT NULL
                 GROUP BY default_strategy_endpoint
                 ORDER BY MAX(priority) DESC",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut canonical_synced = endpoints.is_empty();
        let mut canonical_errors = Vec::new();
        for configured_endpoint in &endpoints {
            let endpoint = configured_endpoint.trim_end_matches('/');
            match conary_core::canonical::client::fetch_canonical_map(&conn, endpoint).await {
                Ok(Some(n)) => {
                    tracing::info!("Canonical map updated: {n} entries from {endpoint}");
                    canonical_synced = true;
                    break;
                }
                Ok(None) => {
                    tracing::debug!("Canonical map is current (304)");
                    canonical_synced = true;
                    break;
                }
                Err(e) => {
                    canonical_errors.push(format!("{endpoint}: {e}"));
                }
            }
        }
        if !canonical_synced {
            failures.push(("canonical map".to_string(), canonical_errors.join("; ")));
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

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{
        NativeSourceStream, Repository, RepositoryUpdateMode, SecurityAdvisorySupport,
    };

    #[tokio::test]
    async fn repo_add_rejects_internal_source_route_slug() {
        let err = cmd_repo_add(RepoAddOptions {
            name: "remi-fedora".to_string(),
            url: "https://remi.example.invalid".to_string(),
            package_format: Some(RepositoryFormat::Json),
            distribution: None,
            component: None,
            architecture: None,
            database: None,
            db_path: "/unused/conary.db".to_string(),
            content_url: None,
            priority: 50,
            disabled: false,
            debian_release_keys: Vec::new(),
            rpm_metadata_keys: Vec::new(),
            rpm_metalink: None,
            rpm_package_keys: Vec::new(),
            arch_keyring: None,
            arch_keyring_format: None,
            arch_master_keys: Vec::new(),
            arch_packager_key_threshold: None,
            arch_database_signature: None,
            fingerprints: Vec::new(),
            yes: false,
            replace: false,
            default_strategy: Some("remi".to_string()),
            remi_endpoint: Some("https://remi.example.invalid".to_string()),
            ccs_package_keys: Vec::new(),
            source_profile: Some("fedora".to_string()),
            source_id: None,
            repository_id: None,
            stream_kind: None,
            stream_id: None,
            policy_group: None,
            follow: false,
            pin_snapshot_sha256: None,
            security_advisory_support: SecurityAdvisorySupport::Unknown,
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains("unsupported source profile"));
    }

    #[tokio::test]
    async fn repo_add_persists_security_advisory_support() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_string = db_path.to_string_lossy().to_string();
        let repo_dir = temp_dir.path().join("repo");
        std::fs::create_dir(&repo_dir).unwrap();
        conary_core::db::init(&db_path).unwrap();

        cmd_repo_add(RepoAddOptions {
            name: "security-supported".to_string(),
            url: repo_dir.display().to_string(),
            package_format: Some(RepositoryFormat::Json),
            distribution: None,
            component: None,
            architecture: None,
            database: None,
            db_path: db_path_string.clone(),
            content_url: None,
            priority: 50,
            disabled: false,
            debian_release_keys: Vec::new(),
            rpm_metadata_keys: Vec::new(),
            rpm_metalink: None,
            rpm_package_keys: Vec::new(),
            arch_keyring: None,
            arch_keyring_format: None,
            arch_master_keys: Vec::new(),
            arch_packager_key_threshold: None,
            arch_database_signature: None,
            fingerprints: Vec::new(),
            yes: false,
            replace: false,
            default_strategy: None,
            remi_endpoint: None,
            ccs_package_keys: Vec::new(),
            source_profile: Some("fedora-44".to_string()),
            source_id: None,
            repository_id: None,
            stream_kind: None,
            stream_id: None,
            policy_group: None,
            follow: false,
            pin_snapshot_sha256: None,
            security_advisory_support: SecurityAdvisorySupport::Supported,
        })
        .await
        .unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        let repo = Repository::find_by_name(&conn, "security-supported")
            .unwrap()
            .unwrap();
        assert_eq!(
            repo.security_advisory_support,
            SecurityAdvisorySupport::Supported
        );
    }

    #[tokio::test]
    async fn repo_add_replaces_uncatalogued_native_policy_without_orphans() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_string = db_path.to_string_lossy().to_string();
        conary_core::db::init(&db_path).unwrap();

        for (source_id, stream_id, replace) in [
            ("third-party:widgets", "stable", false),
            ("third-party:widgets-next", "candidate", true),
        ] {
            cmd_repo_add(RepoAddOptions {
                name: "widgets".to_string(),
                url: "https://packages.example.test/widgets".to_string(),
                package_format: Some(RepositoryFormat::Fedora),
                distribution: None,
                component: None,
                architecture: Some("x86_64".to_string()),
                database: None,
                db_path: db_path_string.clone(),
                content_url: None,
                priority: 75,
                disabled: false,
                debian_release_keys: Vec::new(),
                rpm_metadata_keys: vec![OpenPgpTrustRoot {
                    url: "https://keys.example.test/metadata.gpg".to_string(),
                    fingerprint: "A".repeat(40),
                }],
                rpm_metalink: None,
                rpm_package_keys: vec![OpenPgpTrustRoot {
                    url: "https://keys.example.test/packages.gpg".to_string(),
                    fingerprint: "B".repeat(40),
                }],
                arch_keyring: None,
                arch_keyring_format: None,
                arch_master_keys: Vec::new(),
                arch_packager_key_threshold: None,
                arch_database_signature: None,
                fingerprints: Vec::new(),
                yes: false,
                replace,
                default_strategy: None,
                remi_endpoint: None,
                ccs_package_keys: Vec::new(),
                source_profile: None,
                source_id: Some(source_id.to_string()),
                repository_id: Some("widgets:x86_64".to_string()),
                stream_kind: Some("channel".to_string()),
                stream_id: Some(stream_id.to_string()),
                policy_group: None,
                follow: true,
                pin_snapshot_sha256: None,
                security_advisory_support: SecurityAdvisorySupport::Unknown,
            })
            .await
            .unwrap();
        }

        let conn = conary_core::db::open(&db_path).unwrap();
        let repository = Repository::find_by_name(&conn, "widgets").unwrap().unwrap();
        assert_eq!(repository.source_profile, None);
        let policy = repository.source_policy.unwrap();
        assert_eq!(policy.source_identity, "third-party:widgets-next");
        assert_eq!(
            policy.stream,
            NativeSourceStream::channel("candidate").unwrap()
        );
        assert_eq!(policy.update_mode, RepositoryUpdateMode::Follow);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM repository_source_policies",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn static_probe_cannot_discard_native_source_policy_options() {
        let error = cmd_repo_add(RepoAddOptions {
            name: "ambiguous".to_string(),
            url: "/does/not/matter".to_string(),
            package_format: None,
            distribution: None,
            component: None,
            architecture: None,
            database: None,
            db_path: "/does/not/matter.db".to_string(),
            content_url: None,
            priority: 50,
            disabled: false,
            debian_release_keys: Vec::new(),
            rpm_metadata_keys: Vec::new(),
            rpm_metalink: None,
            rpm_package_keys: Vec::new(),
            arch_keyring: None,
            arch_keyring_format: None,
            arch_master_keys: Vec::new(),
            arch_packager_key_threshold: None,
            arch_database_signature: None,
            fingerprints: Vec::new(),
            yes: false,
            replace: false,
            default_strategy: None,
            remi_endpoint: None,
            ccs_package_keys: Vec::new(),
            source_profile: None,
            source_id: Some("source".to_string()),
            repository_id: None,
            stream_kind: None,
            stream_id: None,
            policy_group: None,
            follow: false,
            pin_snapshot_sha256: None,
            security_advisory_support: SecurityAdvisorySupport::Unknown,
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("explicit rpm, deb, or arch"));
    }
}
