// apps/conary/src/commands/install/config_files.rs

//! Package-format config-file transactions for mutable roots.
//!
//! rpm, dpkg, and libalpm all make config decisions from structural package
//! metadata plus three file identities: the version originally installed, the
//! file currently on disk, and the version in the incoming package.  Keep that
//! decision here instead of inferring intent from paths or script text.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use conary_core::config_transaction::{
    ConfigDematerializationDecision, ConfigInstallDecision, ConfigRemovalDecision, ConfigSuffix,
    config_dematerialization_contract, decide_config_dematerialization, decide_config_install,
    decide_config_removal, decide_deb_remove_on_upgrade, is_etc_config_payload,
};
use conary_core::db::models::{ConfigFile, ConfigSource, ConfigStatus};
use conary_core::packages::config_authority::{ConfigPayloadAssociation, SourceConfigDeclaration};
use rusqlite::Connection;

use crate::commands::{LiveRootContent, LiveRootFile, live_root};

use super::{InstallSemantics, PackageFormatType, PreparedSourceKind};

/// A journal-ready mutable-root payload after applying native config policy.
pub(crate) struct ConfigInstallPlan {
    pub(crate) files: Vec<LiveRootFile>,
    pub(crate) remove_paths: Vec<String>,
}

pub(crate) struct ConfigRemovalPlan {
    pub(crate) files: Vec<LiveRootFile>,
    pub(crate) remove_paths: Vec<String>,
    retained: Vec<ConfigFile>,
}

/// Exact Debian conffile set captured while the owning trove still exists.
///
/// Ordinary payload removal detaches these rows through the `troves` foreign
/// key.  A later purge boundary removes the captured filesystem paths and only
/// then deletes the residual rows, so a failed `postrm remove` still leaves a
/// retryable `config-files` package state.
pub(crate) struct DebianConfigPurgePlan {
    pub(crate) remove_paths: Vec<String>,
    config_ids: Vec<i64>,
}

impl ConfigRemovalPlan {
    pub(crate) fn persist_retained(&mut self, conn: &Connection) -> Result<()> {
        for config in &mut self.retained {
            config.upsert(conn)?;
        }
        Ok(())
    }
}

impl DebianConfigPurgePlan {
    pub(crate) fn delete_rows(&self, conn: &Connection) -> Result<()> {
        for config_id in &self.config_ids {
            ConfigFile::delete(conn, *config_id)?;
        }
        Ok(())
    }
}

pub(super) fn source_for_semantics(semantics: InstallSemantics) -> ConfigSource {
    match semantics.source {
        PreparedSourceKind::NativePackage { format } => match format {
            PackageFormatType::Rpm => ConfigSource::Rpm,
            PackageFormatType::Deb => ConfigSource::Deb,
            PackageFormatType::Arch => ConfigSource::Arch,
            PackageFormatType::Eopkg => ConfigSource::Eopkg,
        },
        PreparedSourceKind::Ccs => ConfigSource::Auto,
    }
}

/// Compute dpkg's exact shipped `Conffiles` digest for a declared Debian
/// conffile. This compatibility digest never replaces Conary's SHA-256
/// content authority.
#[cfg(test)]
pub(crate) fn debian_original_md5(
    source: ConfigSource,
    declared: bool,
    content: &[u8],
) -> Option<String> {
    (source == ConfigSource::Deb && declared).then(|| {
        conary_core::hash::hash_bytes(conary_core::hash::HashAlgorithm::Md5, content).value
    })
}

/// Stream dpkg's shipped conffile digest from a reopenable payload source.
pub(crate) fn debian_original_md5_reader(
    source: ConfigSource,
    declared: bool,
    reader: &mut dyn std::io::Read,
) -> Result<Option<String>> {
    if source != ConfigSource::Deb || !declared {
        return Ok(None);
    }
    Ok(Some(
        conary_core::hash::hash_reader(conary_core::hash::HashAlgorithm::Md5, reader)?.value,
    ))
}

pub(crate) fn debian_original_md5_payload(
    source: ConfigSource,
    declared: bool,
    file: &conary_core::packages::payload::PackagePayloadFile,
) -> Result<Option<String>> {
    if source != ConfigSource::Deb || !declared {
        return Ok(None);
    }
    let mut reader = file
        .open_content()
        .with_context(|| format!("Failed to open Debian conffile payload {}", file.path))?;
    debian_original_md5_reader(source, declared, reader.as_mut())
}

/// Transfer an incoming `remove-on-upgrade` declaration to the new package
/// identity while preserving the old shipped hashes that dpkg exposes until
/// the obsolete conffile transition is complete.
pub(crate) fn persist_debian_remove_on_upgrade(
    conn: &Connection,
    path: &str,
    trove_id: i64,
) -> Result<()> {
    let mut config = prepare_debian_remove_on_upgrade(conn, path, trove_id)?;
    if let Some(config) = config.as_mut() {
        config.upsert(conn)?;
    }
    Ok(())
}

pub(crate) fn prepare_debian_remove_on_upgrade(
    conn: &Connection,
    path: &str,
    trove_id: i64,
) -> Result<Option<ConfigFile>> {
    let Some(mut config) = ConfigFile::find_by_path(conn, path)? else {
        // The declaration is durable authority even when no prior conffile
        // identity exists: persist it as declaration-only Debian conffile
        // state that performs no filesystem mutation.
        let mut config = ConfigFile::new_declaration(path.to_string(), trove_id);
        config.noreplace = true;
        config.source = ConfigSource::Deb;
        config.remove_on_upgrade = true;
        return Ok(Some(config));
    };
    if config.source != ConfigSource::Deb || config.original_md5.is_none() {
        bail!(
            "Debian remove-on-upgrade declaration {path} has no exact prior Debian conffile identity"
        );
    }
    config.file_id = None;
    config.materialized = false;
    config.trove_id = Some(trove_id);
    config.current_hash = None;
    config.status = ConfigStatus::Missing;
    config.modified_at = None;
    config.remove_on_upgrade = true;
    Ok(Some(config))
}

#[derive(Debug)]
struct ExistingConfig {
    file: LiveRootFile,
    hash: String,
}

/// Apply the exact config-file decision table owned by the source package
/// format. The returned files are ordered so saved copies are durable before
/// the incoming package version replaces the primary path.
pub(crate) fn prepare_config_install(
    conn: &Connection,
    root: &Path,
    source: ConfigSource,
    declared: &[SourceConfigDeclaration],
    replacing_trove_id: Option<i64>,
    files: Vec<LiveRootFile>,
) -> Result<ConfigInstallPlan> {
    let declarations = declared
        .iter()
        .filter(|config| !config.ghost())
        .map(|config| (config.path(), config))
        .collect::<HashMap<_, _>>();
    let incoming_paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut planned = Vec::with_capacity(files.len() + declarations.len());
    let mut remove_paths = BTreeSet::new();

    for declaration in declared.iter().filter(|declaration| {
        declaration.payload() == ConfigPayloadAssociation::Absent
            && !declaration.remove_on_upgrade()
    }) {
        let Some(old) = ConfigFile::find_by_path(conn, declaration.path())? else {
            continue;
        };
        if old.trove_id != replacing_trove_id || !old.materialized {
            continue;
        }
        let current = read_existing(root, declaration.path())?;
        let contract = config_dematerialization_contract(source, declaration.ghost())?;
        match decide_config_dematerialization(
            contract,
            old.original_hash.as_deref(),
            current.as_ref().map(|existing| existing.hash.as_str()),
        ) {
            ConfigDematerializationDecision::PreserveCurrent => {}
            ConfigDematerializationDecision::Remove => {
                remove_paths.insert(declaration.path().to_string());
            }
            ConfigDematerializationDecision::RotatePacsaveAndSaveCurrent => {
                if let Some(existing) = current {
                    let mut rotated_remove_paths = Vec::new();
                    rotate_pacsave(
                        root,
                        declaration.path(),
                        existing.file,
                        &mut rotated_remove_paths,
                        &mut planned,
                    )?;
                    remove_paths.extend(rotated_remove_paths);
                }
                remove_paths.insert(declaration.path().to_string());
            }
        }
    }

    for declaration in declared
        .iter()
        .filter(|declaration| declaration.remove_on_upgrade())
    {
        if source != ConfigSource::Deb || declaration.ghost() {
            bail!(
                "remove-on-upgrade declaration {} is only valid for a Debian conffile",
                declaration.path()
            );
        }
        if incoming_paths.contains(declaration.path()) {
            bail!(
                "Debian remove-on-upgrade conffile {} must be absent from the incoming payload",
                declaration.path()
            );
        }

        let old = ConfigFile::find_by_path(conn, declaration.path())?;
        if old
            .as_ref()
            .is_some_and(|config| config.trove_id != replacing_trove_id)
        {
            // dpkg ignores remove-on-upgrade when another package owns the
            // same path.
            continue;
        }
        if old.is_none() {
            // dpkg also ignores a remove-on-upgrade conffile that was never a
            // tracked conffile: there is no prior identity to remove or save,
            // so the declaration persists without touching the path.
            continue;
        }
        let current = read_existing(root, declaration.path())?;
        remove_paths.insert(format!(
            "{}{}",
            declaration.path(),
            ConfigSuffix::DpkgDist.as_str()
        ));
        if let ConfigRemovalDecision::SaveCurrent(ConfigSuffix::DpkgOld) =
            decide_deb_remove_on_upgrade(
                old.as_ref()
                    .and_then(|config| config.original_hash.as_deref()),
                current.as_ref().map(|existing| existing.hash.as_str()),
            )
            && let Some(existing) = current
        {
            planned.push(with_suffix(existing.file, ConfigSuffix::DpkgOld.as_str()));
        }
        remove_paths.insert(declaration.path().to_string());
    }

    for file in files {
        let declaration = declarations.get(file.path.as_str()).copied();
        if declaration.is_none() && !is_etc_config_payload(&file.path, &file.node.source.kind) {
            planned.push(file);
            continue;
        }

        let old = ConfigFile::find_by_path(conn, &file.path)?;
        let old_hash = old
            .as_ref()
            .and_then(|config| config.original_hash.as_deref());
        let current = read_existing(root, &file.path)?;
        let new_hash = live_file_hash(&file)?;
        let incoming_source = declaration.map_or(ConfigSource::Auto, |_| source);
        let noreplace = declaration.is_some_and(|config| config.noreplace());
        let decision = decide_config_install(
            incoming_source,
            noreplace,
            old_hash,
            current.as_ref().map(|existing| existing.hash.as_str()),
            &new_hash,
        );

        match decision {
            ConfigInstallDecision::Install => planned.push(file),
            ConfigInstallDecision::Keep => {
                // libalpm extracts over an existing .pacnew before deciding to
                // keep an unchanged package version. Preserve that observable
                // behavior without using the native package manager.
                if incoming_source == ConfigSource::Arch
                    && old_hash == Some(new_hash.as_str())
                    && auxiliary_exists(root, &file.path, ".pacnew")?
                {
                    planned.push(with_suffix(file, ".pacnew"));
                }
            }
            ConfigInstallDecision::InstallAlternative(suffix) => {
                planned.push(with_suffix(file, suffix.as_str()));
            }
            ConfigInstallDecision::SaveCurrentAndInstall(suffix) => {
                if let Some(existing) = current {
                    planned.push(with_suffix(existing.file, suffix.as_str()));
                }
                planned.push(file);
            }
        }
    }

    Ok(ConfigInstallPlan {
        files: planned,
        remove_paths: remove_paths.into_iter().collect(),
    })
}

/// Apply source-format removal semantics. Debian conffiles survive an ordinary
/// remove and remain attached to their stable package identity; explicit purge
/// removes them. Modified RPM and Arch configs are saved using their native
/// suffix contracts before the primary path is removed.
pub(crate) fn prepare_config_removal(
    conn: &Connection,
    root: &Path,
    trove_id: i64,
    package_paths: impl IntoIterator<Item = String>,
    purge: bool,
) -> Result<ConfigRemovalPlan> {
    let mut configs = ConfigFile::find_by_trove(conn, trove_id)?
        .into_iter()
        .map(|config| (config.path.clone(), config))
        .collect::<HashMap<_, _>>();
    let mut files = Vec::new();
    let mut remove_paths = Vec::new();
    let mut retained = Vec::new();

    for path in package_paths {
        let Some(mut config) = configs.remove(&path) else {
            remove_paths.push(path);
            continue;
        };
        if !config.materialized && !config.ghost {
            continue;
        }
        let existing = read_existing(root, &config.path)?;
        let decision = decide_config_removal(
            config.source,
            config.ghost,
            purge,
            config.original_hash.as_deref(),
            existing.as_ref().map(|existing| existing.hash.as_str()),
        );
        if purge && config.source == ConfigSource::Deb {
            remove_debian_backup_paths(&config.path, &mut remove_paths);
        }
        record_live_state(&mut config, root)?;

        match decision {
            ConfigRemovalDecision::KeepResidual => {
                retained.push(config);
                continue;
            }
            ConfigRemovalDecision::RotatePacsaveAndSaveCurrent => {
                if let Some(existing) = existing {
                    rotate_pacsave(
                        root,
                        &config.path,
                        existing.file,
                        &mut remove_paths,
                        &mut files,
                    )?;
                }
            }
            ConfigRemovalDecision::SaveCurrent(suffix) => {
                if let Some(existing) = existing {
                    files.push(with_suffix(existing.file, suffix.as_str()));
                }
            }
            ConfigRemovalDecision::Remove => {}
        }
        remove_paths.push(path);
    }

    // RPM ghosts have no package payload entry. rpm never creates or backs
    // them up, but erases an existing path with the package.
    for (_, mut config) in configs {
        if !config.materialized && !config.ghost {
            continue;
        }
        let existing = read_existing(root, &config.path)?;
        let decision = decide_config_removal(
            config.source,
            config.ghost,
            purge,
            config.original_hash.as_deref(),
            existing.as_ref().map(|existing| existing.hash.as_str()),
        );
        if purge && config.source == ConfigSource::Deb {
            remove_debian_backup_paths(&config.path, &mut remove_paths);
        }
        record_live_state(&mut config, root)?;
        match decision {
            ConfigRemovalDecision::KeepResidual => retained.push(config),
            ConfigRemovalDecision::Remove => remove_paths.push(config.path),
            ConfigRemovalDecision::SaveCurrent(suffix) => {
                if let Some(existing) = existing {
                    files.push(with_suffix(existing.file, suffix.as_str()));
                }
                remove_paths.push(config.path);
            }
            ConfigRemovalDecision::RotatePacsaveAndSaveCurrent => {
                if let Some(existing) = existing {
                    rotate_pacsave(
                        root,
                        &config.path,
                        existing.file,
                        &mut remove_paths,
                        &mut files,
                    )?;
                }
                remove_paths.push(config.path);
            }
        }
    }

    Ok(ConfigRemovalPlan {
        files,
        remove_paths,
        retained,
    })
}

/// Capture the exact Debian conffiles owned by an installed trove for the
/// second phase of dpkg-compatible purge.
pub(crate) fn prepare_debian_config_purge(
    conn: &Connection,
    root: &Path,
    trove_id: i64,
) -> Result<DebianConfigPurgePlan> {
    let configs = ConfigFile::find_by_trove(conn, trove_id)?
        .into_iter()
        .filter(|config| config.source == ConfigSource::Deb)
        .collect::<Vec<_>>();
    let mut config_ids = Vec::with_capacity(configs.len());
    let mut remove_paths = Vec::with_capacity(configs.len() * 3);
    for config in configs {
        let decision = decide_config_removal(
            config.source,
            config.ghost,
            true,
            config.original_hash.as_deref(),
            read_existing(root, &config.path)?
                .as_ref()
                .map(|existing| existing.hash.as_str()),
        );
        if decision != ConfigRemovalDecision::Remove {
            bail!(
                "Debian purge for {} produced non-removal decision {decision:?}",
                config.path
            );
        }
        remove_debian_backup_paths(&config.path, &mut remove_paths);
        remove_paths.push(config.path);
        config_ids.push(
            config
                .id
                .context("tracked Debian conffile has no database identity")?,
        );
    }
    Ok(DebianConfigPurgePlan {
        remove_paths,
        config_ids,
    })
}

fn remove_debian_backup_paths(package_path: &str, remove_paths: &mut Vec<String>) {
    remove_paths.extend(
        [ConfigSuffix::DpkgDist, ConfigSuffix::DpkgOld]
            .map(|suffix| format!("{package_path}{}", suffix.as_str())),
    );
}

/// Project the live mutable-root state into the persisted config record after
/// the journaled filesystem mutation has completed.
pub(crate) fn record_live_state(config: &mut ConfigFile, root: &Path) -> Result<()> {
    match read_existing(root, &config.path)? {
        Some(existing) if config.original_hash.as_deref() == Some(existing.hash.as_str()) => {
            config.current_hash = Some(existing.hash);
            config.status = ConfigStatus::Pristine;
            config.modified_at = None;
        }
        Some(existing) => {
            config.current_hash = Some(existing.hash);
            config.status = ConfigStatus::Modified;
            config.modified_at = Some(chrono::Utc::now().to_rfc3339());
        }
        None => {
            config.current_hash = None;
            config.status = ConfigStatus::Missing;
            config.modified_at = None;
        }
    }
    Ok(())
}

fn with_suffix(mut file: LiveRootFile, suffix: &str) -> LiveRootFile {
    file.path.push_str(suffix);
    file
}

fn auxiliary_exists(root: &Path, package_path: &str, suffix: &str) -> Result<bool> {
    let suffixed = format!("{package_path}{suffix}");
    let path = live_root::target_path(root, &suffixed)?;
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn rotate_pacsave(
    root: &Path,
    package_path: &str,
    current: LiveRootFile,
    remove_paths: &mut Vec<String>,
    files: &mut Vec<LiveRootFile>,
) -> Result<()> {
    let base = format!("{package_path}.pacsave");
    let mut numbered = Vec::new();
    if let Some(existing) = read_existing(root, &base)? {
        numbered.push((0_u64, base.clone(), existing.file));
    }

    let target = live_root::target_path(root, package_path)?;
    let parent = target
        .parent()
        .context("config path has no parent directory")?;
    let basename = target
        .file_name()
        .and_then(|name| name.to_str())
        .context("config path basename is not UTF-8")?;
    let numbered_prefix = format!("{basename}.pacsave.");
    match fs::read_dir(parent) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let Some(number) = name
                    .strip_prefix(&numbered_prefix)
                    .filter(|value| !value.is_empty())
                    .and_then(|value| value.parse::<u64>().ok())
                else {
                    continue;
                };
                let path = format!("{base}.{number}");
                if let Some(existing) = read_existing(root, &path)? {
                    numbered.push((number, path, existing.file));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    numbered.sort_by_key(|(number, _, _)| std::cmp::Reverse(*number));
    let mut replacement_paths = numbered
        .iter()
        .map(|(number, _, _)| format!("{base}.{}", number + 1))
        .collect::<BTreeSet<_>>();
    replacement_paths.insert(base.clone());
    for (number, old_path, mut old_file) in numbered {
        if !replacement_paths.contains(&old_path) {
            remove_paths.push(old_path);
        }
        old_file.path = format!("{base}.{}", number + 1);
        files.push(old_file);
    }
    files.push(with_suffix(current, ".pacsave"));
    Ok(())
}

fn read_existing(root: &Path, package_path: &str) -> Result<Option<ExistingConfig>> {
    let path = live_root::target_path(root, package_path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect config {}", path.display()));
        }
    };
    let node = conary_core::generation::root_manifest::capture_existing_payload_node(&path)?;

    let file = if metadata.file_type().is_symlink() {
        LiveRootFile {
            path: package_path.to_string(),
            content: LiveRootContent::absent(),
            node,
        }
    } else if metadata.is_file() {
        LiveRootFile {
            path: package_path.to_string(),
            content: LiveRootContent::capture_file(&path)?,
            node,
        }
    } else {
        bail!(
            "Declared config {} is neither a regular file nor a symlink",
            path.display()
        );
    };
    let hash = live_file_hash(&file)?;
    Ok(Some(ExistingConfig { file, hash }))
}

fn live_file_hash(file: &LiveRootFile) -> Result<String> {
    match &file.node.source.kind {
        conary_core::payload::PayloadNodeKind::Symlink { target } => {
            Ok(conary_core::hash::sha256(target.as_bytes()))
        }
        conary_core::payload::PayloadNodeKind::Regular { .. } => Ok(file
            .content
            .authority()
            .context("regular config payload has no content authority")?
            .sha256
            .clone()),
        _ => unreachable!("config policy accepts only regular files and symlinks"),
    }
}

#[cfg(test)]
mod tests;
