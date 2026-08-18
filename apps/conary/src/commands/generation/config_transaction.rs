// apps/conary/src/commands/generation/config_transaction.rs
//! Durable capture and atomic generation-overlay materialization for `/etc`.

use std::collections::{BTreeSet, HashMap};
use std::ffi::{CStr, CString};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::config_transaction::{
    ConfigArtifact, ConfigDematerializationDecision, ConfigInstallDecision, ConfigPackageState,
    ConfigPathTransaction, ConfigRemovalDecision, ConfigSuffix, ConfigTransactionOperation,
    GenerationConfigTransaction, config_dematerialization_contract,
    decide_config_dematerialization, decide_config_install, decide_config_removal,
    decide_deb_remove_on_upgrade, is_config_artifact_kind, is_etc_config_payload,
};
use conary_core::db::models::{ConfigFile, ConfigSource, ConfigStatus, FileEntry};
use conary_core::filesystem::CasStore;
use conary_core::filesystem::durable::sync_parent_directory;
use conary_core::generation::mount::current_generation;
use conary_core::packages::config_authority::{ConfigPayloadAssociation, SourceConfigDeclaration};
use conary_core::payload::PayloadNodeKind;
use conary_core::runtime_root::ConaryRuntimeRoot;
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
enum OverlayMutation {
    Remove(String),
    Write(String, Box<ConfigArtifact>),
    Whiteout(String),
}

fn overlay_write(path: String, artifact: ConfigArtifact) -> OverlayMutation {
    OverlayMutation::Write(path, Box::new(artifact))
}

pub(crate) struct ConfigInstallCapture<'a> {
    pub(crate) source: ConfigSource,
    pub(crate) declared: &'a [SourceConfigDeclaration],
    pub(crate) incoming: &'a [crate::commands::LiveRootFile],
    pub(crate) replacing_trove_id: Option<i64>,
    pub(crate) replaced_trove_ids: &'a [i64],
}

pub(crate) fn capture_install(
    conn: &Connection,
    root: &Path,
    cas: &CasStore,
    capture: ConfigInstallCapture<'_>,
) -> Result<GenerationConfigTransaction> {
    let ConfigInstallCapture {
        source,
        declared,
        incoming,
        replacing_trove_id,
        replaced_trove_ids,
    } = capture;
    let mut declarations = HashMap::new();
    for declaration in declared {
        if declarations
            .insert(declaration.path(), declaration)
            .is_some()
        {
            bail!(
                "incoming package declares generation config path {} more than once",
                declaration.path()
            );
        }
    }
    let incoming_files = incoming
        .iter()
        .filter(|file| is_etc_config_payload(&file.path, &file.node.source.kind))
        .map(|file| (file.path.as_str(), file))
        .collect::<HashMap<_, _>>();

    let mut paths = incoming_files
        .keys()
        .map(|path| (*path).to_string())
        .collect::<BTreeSet<_>>();
    paths.extend(
        declared
            .iter()
            .filter(|declaration| generation_config_path(declaration.path()))
            .map(|declaration| declaration.path().to_string()),
    );
    for trove_id in replaced_trove_ids {
        paths.extend(
            conary_core::db::models::PackagePayloadOwnership::load(conn, *trove_id)?
                .entries()
                .iter()
                .filter(|file| is_etc_config_payload(&file.path, &file.node.source.kind))
                .map(|file| file.path.clone()),
        );
        paths.extend(
            ConfigFile::find_by_trove(conn, *trove_id)?
                .into_iter()
                .filter(|config| generation_config_path(&config.path))
                .map(|config| config.path),
        );
    }

    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        let before = package_state_before(conn, &path)?;
        let current = read_artifact(root, cas, &path)?;
        let declaration = declarations.get(path.as_str()).copied();
        let incoming_file = incoming_files.get(path.as_str()).copied();
        if let Some(declaration) = declaration.filter(|declaration| declaration.remove_on_upgrade())
        {
            if source != ConfigSource::Deb || declaration.ghost() {
                bail!(
                    "remove-on-upgrade declaration {} is only valid for a Debian conffile",
                    declaration.path()
                );
            }
            if incoming_file.is_some() {
                bail!(
                    "Debian remove-on-upgrade conffile {} must be absent from the incoming payload",
                    declaration.path()
                );
            }
            let owner = ConfigFile::find_by_path(conn, &path)?;
            if owner
                .as_ref()
                .is_some_and(|config| config.trove_id != replacing_trove_id)
            {
                // dpkg ignores this declaration while another package owns
                // the path.
                continue;
            }
            if owner.is_none() {
                // dpkg also ignores a remove-on-upgrade conffile that was
                // never a tracked conffile: the declaration persists without
                // any filesystem mutation.
                continue;
            }
        }
        let after = match (declaration, incoming_file) {
            (Some(declaration), _) if declaration.ghost() => Some(ConfigPackageState {
                source,
                noreplace: declaration.noreplace(),
                ghost: true,
                materialized: false,
                original_sha256: None,
                artifact: None,
            }),
            (Some(declaration), Some(_)) if declaration.remove_on_upgrade() => {
                bail!(
                    "Debian remove-on-upgrade conffile {} must be absent from the incoming payload",
                    declaration.path()
                );
            }
            (declaration, Some(file)) => {
                if !is_config_artifact_kind(&file.node.source.kind) {
                    bail!(
                        "declared generation config {} is not a regular file or symlink",
                        file.path
                    );
                }
                let artifact = artifact_from_live_root(file, cas)?;
                Some(ConfigPackageState {
                    source: declaration.map_or(ConfigSource::Auto, |_| source),
                    noreplace: declaration.is_some_and(|config| config.noreplace()),
                    ghost: false,
                    materialized: true,
                    original_sha256: Some(artifact.sha256().to_string()),
                    artifact: Some(artifact),
                })
            }
            (Some(declaration), None) if declaration.remove_on_upgrade() => {
                if source != ConfigSource::Deb || declaration.ghost() {
                    bail!(
                        "remove-on-upgrade declaration {} is only valid for a Debian conffile",
                        declaration.path()
                    );
                }
                None
            }
            (Some(declaration), None)
                if declaration.payload() == ConfigPayloadAssociation::Absent =>
            {
                Some(ConfigPackageState {
                    source,
                    noreplace: declaration.noreplace(),
                    ghost: false,
                    materialized: false,
                    original_sha256: None,
                    artifact: None,
                })
            }
            (None, None) => None,
            (Some(declaration), None) => {
                bail!(
                    "declared generation config {} is missing from the incoming payload",
                    declaration.path()
                );
            }
        };
        let operation = if declaration.is_some_and(|declaration| declaration.remove_on_upgrade()) {
            ConfigTransactionOperation::RemoveOnUpgrade
        } else if before.as_ref().is_some_and(|state| state.materialized)
            && after.as_ref().is_some_and(|state| !state.materialized)
        {
            ConfigTransactionOperation::Dematerialize
        } else if after.is_some() {
            ConfigTransactionOperation::Install
        } else {
            ConfigTransactionOperation::Remove
        };
        entries.push(ConfigPathTransaction {
            path: path.clone(),
            operation,
            before,
            current,
            after,
            auxiliaries: capture_auxiliaries(root, cas, &path)?,
        });
    }

    let transaction = GenerationConfigTransaction {
        entries,
        ..Default::default()
    };
    transaction.validate()?;
    Ok(transaction)
}

pub(crate) fn capture_removal(
    conn: &Connection,
    root: &Path,
    cas: &CasStore,
    trove_id: i64,
    purge: bool,
) -> Result<GenerationConfigTransaction> {
    let mut paths = conary_core::db::models::PackagePayloadOwnership::load(conn, trove_id)?
        .entries()
        .iter()
        .filter(|file| is_etc_config_payload(&file.path, &file.node.source.kind))
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    paths.extend(
        ConfigFile::find_by_trove(conn, trove_id)?
            .into_iter()
            .filter(|config| generation_config_path(&config.path))
            .map(|config| config.path),
    );

    let operation = if purge {
        ConfigTransactionOperation::Purge
    } else {
        ConfigTransactionOperation::Remove
    };
    let entries = paths
        .into_iter()
        .map(|path| {
            Ok(ConfigPathTransaction {
                before: package_state_before(conn, &path)?,
                current: read_artifact(root, cas, &path)?,
                after: None,
                auxiliaries: capture_auxiliaries(root, cas, &path)?,
                path,
                operation,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let transaction = GenerationConfigTransaction {
        entries,
        ..Default::default()
    };
    transaction.validate()?;
    Ok(transaction)
}

fn generation_config_path(path: &str) -> bool {
    path.strip_prefix("/etc/")
        .is_some_and(|relative| !relative.is_empty())
}

fn package_state_before(conn: &Connection, path: &str) -> Result<Option<ConfigPackageState>> {
    if let Some(config) = ConfigFile::find_by_path(conn, path)? {
        return Ok(Some(ConfigPackageState {
            source: config.source,
            noreplace: config.noreplace,
            ghost: config.ghost,
            materialized: config.materialized,
            original_sha256: config.original_hash,
            artifact: None,
        }));
    }
    Ok(
        FileEntry::find_by_path(conn, path)?.map(|file| ConfigPackageState {
            source: ConfigSource::Auto,
            noreplace: false,
            ghost: false,
            materialized: true,
            original_sha256: config_hash_for_file_entry(&file),
            artifact: None,
        }),
    )
}

fn artifact_from_live_root(
    file: &crate::commands::LiveRootFile,
    cas: &CasStore,
) -> Result<ConfigArtifact> {
    let artifact = match &file.node.source.kind {
        PayloadNodeKind::Symlink { target } => {
            ConfigArtifact::symlink(target.clone(), file.node.clone())?
        }
        PayloadNodeKind::Regular { .. } => {
            let authority = file
                .content
                .authority()
                .context("regular generation config has no content authority")?
                .clone();
            let mut reader = file.content.open()?;
            cas.store_reader_expected(&mut reader, authority.size, &authority.sha256)?;
            ConfigArtifact::regular(authority, file.node.clone())?
        }
        _ => bail!(
            "generation config {} is not a regular file or symlink",
            file.path
        ),
    };
    Ok(artifact)
}

fn config_hash_for_file_entry(file: &FileEntry) -> Option<String> {
    match &file.node.source.kind {
        PayloadNodeKind::Regular { .. } => {
            file.content.as_ref().map(|content| content.sha256.clone())
        }
        PayloadNodeKind::Symlink { target } => Some(conary_core::hash::sha256(target.as_bytes())),
        _ => None,
    }
}

fn read_artifact(
    root: &Path,
    cas: &CasStore,
    package_path: &str,
) -> Result<Option<ConfigArtifact>> {
    let path = crate::commands::live_root::target_path(root, package_path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect config {}", path.display()));
        }
    };
    let node = conary_core::generation::root_manifest::capture_existing_payload_node(&path)?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(&path)
            .with_context(|| format!("failed to read config symlink {}", path.display()))?
            .into_os_string()
            .into_string()
            .map_err(|_| anyhow::anyhow!("config symlink {} is not UTF-8", path.display()))?;
        return Ok(Some(ConfigArtifact::symlink(target, node)?));
    }
    if metadata.is_file() {
        let content = crate::commands::LiveRootContent::capture_file(&path)?;
        let authority = content
            .authority()
            .context("captured regular config has no content authority")?
            .clone();
        let mut reader = content.open()?;
        cas.store_reader_expected(&mut reader, authority.size, &authority.sha256)?;
        return Ok(Some(ConfigArtifact::regular(authority, node)?));
    }
    bail!(
        "generation config {} is neither a regular file nor a symlink",
        path.display()
    )
}

fn capture_auxiliaries(
    root: &Path,
    cas: &CasStore,
    package_path: &str,
) -> Result<Vec<(String, ConfigArtifact)>> {
    let mut paths = ConfigSuffix::ALL
        .into_iter()
        .map(|suffix| format!("{package_path}{}", suffix.as_str()))
        .collect::<BTreeSet<_>>();

    let primary = crate::commands::live_root::target_path(root, package_path)?;
    if let (Some(parent), Some(basename)) = (
        primary.parent(),
        primary.file_name().and_then(|name| name.to_str()),
    ) {
        let numbered_prefix = format!("{basename}.pacsave.");
        match fs::read_dir(parent) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    if name
                        .strip_prefix(&numbered_prefix)
                        .and_then(conary_core::config_transaction::parse_canonical_positive_decimal)
                        .is_some()
                    {
                        paths.insert(format!(
                            "{package_path}.pacsave.{}",
                            &name[numbered_prefix.len()..]
                        ));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    paths
        .into_iter()
        .filter_map(|path| match read_artifact(root, cas, &path) {
            Ok(Some(artifact)) => Some(Ok((path, artifact))),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub(crate) fn materialize(
    runtime_root: &ConaryRuntimeRoot,
    generation_number: i64,
    transactions: &[GenerationConfigTransaction],
) -> Result<()> {
    for transaction in transactions {
        transaction.validate()?;
    }
    let cas = CasStore::new(runtime_root.objects_dir())?;
    let etc_state = runtime_root.etc_state_dir();
    fs::create_dir_all(&etc_state)?;
    let stage = etc_state.join(format!(
        ".{generation_number}.config-{}",
        uuid::Uuid::new_v4()
    ));
    let final_upper = etc_state.join(generation_number.to_string());
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    fs::create_dir(&stage)?;

    let result = (|| -> Result<()> {
        // The exact mutable-state manifest is seeded at the unpublished
        // generation path before config decisions are applied. Preserve that
        // `/etc` authority as the base of the config overlay. `/var` and
        // `/srv` remain owned by their live mutable roots.
        if final_upper.is_dir() {
            copy_overlay_tree(&final_upper, &stage)?;
        }
        if let Some(current) = current_generation(runtime_root.root())? {
            let current_upper = etc_state.join(current.to_string());
            if current_upper.is_dir() {
                copy_overlay_tree(&current_upper, &stage)?;
            }
        }

        for transaction in transactions {
            for entry in &transaction.entries {
                let mutations = plan_entry(entry)?;
                for mutation in mutations {
                    apply_mutation(&stage, &cas, mutation)?;
                }
            }
        }

        sync_tree(&stage)?;
        if final_upper.exists() {
            fs::remove_dir_all(&final_upper).with_context(|| {
                format!(
                    "failed to replace unpublished generation config upper {}",
                    final_upper.display()
                )
            })?;
        }
        fs::rename(&stage, &final_upper)?;
        sync_parent_directory(&final_upper)?;
        Ok(())
    })();
    if result.is_err() && stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

/// Project config status only after the matching generation link is durable.
///
/// The publication debt remains recoverable if this transaction fails, so a
/// selected generation is never preceded by a speculative config-status
/// update.
pub(crate) fn project_persisted_statuses(
    conn: &Connection,
    transactions: &[GenerationConfigTransaction],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for transaction in transactions {
        for entry in &transaction.entries {
            project_persisted_status(&tx, entry)?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn plan_entry(entry: &ConfigPathTransaction) -> Result<Vec<OverlayMutation>> {
    let mut mutations = Vec::new();
    match entry.operation {
        ConfigTransactionOperation::Install => {
            let after = entry
                .after
                .as_ref()
                .context("install config transaction has no after state")?;
            if after.ghost || !after.materialized {
                return Ok(mutations);
            }
            let new = after
                .artifact
                .as_ref()
                .context("non-ghost install config transaction has no artifact")?;
            let old = entry
                .before
                .as_ref()
                .and_then(|state| state.original_sha256.as_deref());
            let current = entry.current.as_ref().map(ConfigArtifact::sha256);
            let mut decision =
                decide_config_install(after.source, after.noreplace, old, current, new.sha256());
            if decision == ConfigInstallDecision::Keep
                && after.source == ConfigSource::Arch
                && old == Some(new.sha256())
                && auxiliary(entry, ConfigSuffix::PacNew).is_some()
            {
                decision = ConfigInstallDecision::InstallAlternative(ConfigSuffix::PacNew);
            }
            match decision {
                ConfigInstallDecision::Install => {
                    mutations.push(OverlayMutation::Remove(entry.path.clone()));
                }
                ConfigInstallDecision::Keep => preserve_primary(entry, new, &mut mutations),
                ConfigInstallDecision::InstallAlternative(suffix) => {
                    preserve_primary(entry, new, &mut mutations);
                    mutations.push(overlay_write(
                        format!("{}{}", entry.path, suffix.as_str()),
                        new.clone(),
                    ));
                }
                ConfigInstallDecision::SaveCurrentAndInstall(suffix) => {
                    if let Some(current) = &entry.current {
                        mutations.push(overlay_write(
                            format!("{}{}", entry.path, suffix.as_str()),
                            current.clone(),
                        ));
                    }
                    mutations.push(OverlayMutation::Remove(entry.path.clone()));
                }
            }
        }
        ConfigTransactionOperation::RemoveOnUpgrade => {
            mutations.push(OverlayMutation::Remove(format!(
                "{}{}",
                entry.path,
                ConfigSuffix::DpkgDist.as_str()
            )));
            if let ConfigRemovalDecision::SaveCurrent(ConfigSuffix::DpkgOld) =
                decide_deb_remove_on_upgrade(
                    entry
                        .before
                        .as_ref()
                        .and_then(|state| state.original_sha256.as_deref()),
                    entry.current.as_ref().map(ConfigArtifact::sha256),
                )
                && let Some(current) = &entry.current
            {
                mutations.push(overlay_write(
                    format!("{}{}", entry.path, ConfigSuffix::DpkgOld.as_str()),
                    current.clone(),
                ));
            }
            mutations.push(OverlayMutation::Remove(entry.path.clone()));
        }
        ConfigTransactionOperation::Dematerialize => {
            let before = entry
                .before
                .as_ref()
                .context("dematerialize config transaction has no exact prior package state")?;
            let after = entry
                .after
                .as_ref()
                .context("dematerialize config transaction has no declaration-only after state")?;
            let contract = config_dematerialization_contract(after.source, after.ghost)?;
            match decide_config_dematerialization(
                contract,
                before.original_sha256.as_deref(),
                entry.current.as_ref().map(ConfigArtifact::sha256),
            ) {
                ConfigDematerializationDecision::PreserveCurrent => {}
                ConfigDematerializationDecision::Remove => {
                    mutations.push(OverlayMutation::Remove(entry.path.clone()));
                }
                ConfigDematerializationDecision::RotatePacsaveAndSaveCurrent => {
                    rotate_pacsaves(entry, &mut mutations)?;
                    if let Some(current) = &entry.current {
                        mutations.push(overlay_write(
                            format!("{}{}", entry.path, ConfigSuffix::PacSave.as_str()),
                            current.clone(),
                        ));
                    }
                    mutations.push(OverlayMutation::Remove(entry.path.clone()));
                }
            }
        }
        ConfigTransactionOperation::Remove | ConfigTransactionOperation::Purge => {
            let before = entry
                .before
                .as_ref()
                .context("remove/purge config transaction has no exact prior package state")?;
            if !before.materialized && !before.ghost {
                return Ok(mutations);
            }
            let decision = decide_config_removal(
                before.source,
                before.ghost,
                entry.operation == ConfigTransactionOperation::Purge,
                before.original_sha256.as_deref(),
                entry.current.as_ref().map(ConfigArtifact::sha256),
            );
            if entry.operation == ConfigTransactionOperation::Purge
                && before.source == ConfigSource::Deb
            {
                for suffix in [ConfigSuffix::DpkgDist, ConfigSuffix::DpkgOld] {
                    mutations.push(OverlayMutation::Remove(format!(
                        "{}{}",
                        entry.path,
                        suffix.as_str()
                    )));
                }
            }
            match decision {
                ConfigRemovalDecision::KeepResidual => {
                    if let Some(current) = &entry.current {
                        mutations.push(overlay_write(entry.path.clone(), current.clone()));
                    } else {
                        mutations.push(OverlayMutation::Remove(entry.path.clone()));
                    }
                }
                ConfigRemovalDecision::Remove => {
                    mutations.push(OverlayMutation::Remove(entry.path.clone()));
                }
                ConfigRemovalDecision::SaveCurrent(suffix) => {
                    if let Some(current) = &entry.current {
                        mutations.push(overlay_write(
                            format!("{}{}", entry.path, suffix.as_str()),
                            current.clone(),
                        ));
                    }
                    mutations.push(OverlayMutation::Remove(entry.path.clone()));
                }
                ConfigRemovalDecision::RotatePacsaveAndSaveCurrent => {
                    rotate_pacsaves(entry, &mut mutations)?;
                    if let Some(current) = &entry.current {
                        mutations.push(overlay_write(
                            format!("{}{}", entry.path, ConfigSuffix::PacSave.as_str()),
                            current.clone(),
                        ));
                    }
                    mutations.push(OverlayMutation::Remove(entry.path.clone()));
                }
            }
        }
        ConfigTransactionOperation::Restore => {
            for path in entry.restore_owned_paths() {
                mutations.push(OverlayMutation::Remove(path));
            }
            match &entry.current {
                Some(current) => mutations.push(overlay_write(entry.path.clone(), current.clone())),
                None => mutations.push(OverlayMutation::Whiteout(entry.path.clone())),
            }
            for (path, artifact) in &entry.auxiliaries {
                mutations.push(overlay_write(path.clone(), artifact.clone()));
            }
        }
    }
    Ok(mutations)
}

fn preserve_primary(
    entry: &ConfigPathTransaction,
    new: &ConfigArtifact,
    mutations: &mut Vec<OverlayMutation>,
) {
    match &entry.current {
        Some(current) => mutations.push(overlay_write(entry.path.clone(), current.clone())),
        None => {
            let _ = new;
            mutations.push(OverlayMutation::Whiteout(entry.path.clone()));
        }
    }
}

fn auxiliary(entry: &ConfigPathTransaction, suffix: ConfigSuffix) -> Option<&ConfigArtifact> {
    let path = format!("{}{}", entry.path, suffix.as_str());
    entry
        .auxiliaries
        .iter()
        .find_map(|(candidate, artifact)| (candidate == &path).then_some(artifact))
}

fn rotate_pacsaves(
    entry: &ConfigPathTransaction,
    mutations: &mut Vec<OverlayMutation>,
) -> Result<()> {
    let base = format!("{}{}", entry.path, ConfigSuffix::PacSave.as_str());
    let mut numbered = entry
        .auxiliaries
        .iter()
        .filter_map(|(path, artifact)| {
            if path == &base {
                Some((0_u64, path, artifact))
            } else {
                path.strip_prefix(&(base.clone() + "."))
                    .and_then(conary_core::config_transaction::parse_canonical_positive_decimal)
                    .map(|number| (number, path, artifact))
            }
        })
        .collect::<Vec<_>>();
    numbered.sort_by_key(|(number, _, _)| std::cmp::Reverse(*number));
    for (number, old_path, artifact) in numbered {
        let successor = number.checked_add(1).ok_or_else(|| {
            anyhow::anyhow!(
                "cannot rotate {} because its canonical pacsave sequence is exhausted",
                old_path
            )
        })?;
        mutations.push(OverlayMutation::Remove(old_path.clone()));
        mutations.push(overlay_write(
            format!("{base}.{successor}"),
            artifact.clone(),
        ));
    }
    Ok(())
}

fn project_persisted_status(conn: &Connection, entry: &ConfigPathTransaction) -> Result<()> {
    let Some(config) = ConfigFile::find_by_path(conn, &entry.path)? else {
        return Ok(());
    };
    let projected = match entry.operation {
        ConfigTransactionOperation::Install => {
            let after = entry.after.as_ref().context("missing after state")?;
            if after.ghost {
                entry.current.as_ref()
            } else if !after.materialized {
                None
            } else {
                let new = after.artifact.as_ref().context("missing after artifact")?;
                match decide_config_install(
                    after.source,
                    after.noreplace,
                    entry
                        .before
                        .as_ref()
                        .and_then(|state| state.original_sha256.as_deref()),
                    entry.current.as_ref().map(ConfigArtifact::sha256),
                    new.sha256(),
                ) {
                    ConfigInstallDecision::Install
                    | ConfigInstallDecision::SaveCurrentAndInstall(_) => Some(new),
                    ConfigInstallDecision::Keep | ConfigInstallDecision::InstallAlternative(_) => {
                        entry.current.as_ref()
                    }
                }
            }
        }
        ConfigTransactionOperation::Dematerialize => None,
        ConfigTransactionOperation::Remove => entry.current.as_ref(),
        ConfigTransactionOperation::RemoveOnUpgrade => None,
        ConfigTransactionOperation::Purge => None,
        ConfigTransactionOperation::Restore => entry.current.as_ref(),
    };
    match projected {
        Some(artifact) if config.original_hash.as_deref() == Some(artifact.sha256()) => {
            config.update_status(conn, ConfigStatus::Pristine, Some(artifact.sha256()))?;
        }
        Some(artifact) => {
            config.update_status(conn, ConfigStatus::Modified, Some(artifact.sha256()))?;
        }
        None => config.update_status(conn, ConfigStatus::Missing, None)?,
    }
    Ok(())
}

fn overlay_path(upper: &Path, package_path: &str) -> Result<PathBuf> {
    let relative = package_path
        .strip_prefix("/etc/")
        .filter(|relative| !relative.is_empty())
        .context("generation overlay mutation path is outside /etc")?;
    crate::commands::live_root::target_path(upper, relative)
}

fn apply_mutation(upper: &Path, cas: &CasStore, mutation: OverlayMutation) -> Result<()> {
    match mutation {
        OverlayMutation::Remove(path) => remove_overlay_entry(&overlay_path(upper, &path)?),
        OverlayMutation::Write(path, artifact) => {
            write_artifact(&overlay_path(upper, &path)?, cas, &artifact)
        }
        OverlayMutation::Whiteout(path) => create_whiteout(&overlay_path(upper, &path)?),
    }
}

fn remove_overlay_entry(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn write_artifact(path: &Path, cas: &CasStore, artifact: &ConfigArtifact) -> Result<()> {
    remove_overlay_entry(path)?;
    let parent = path
        .parent()
        .context("generation config artifact has no parent")?;
    fs::create_dir_all(parent)?;
    match artifact {
        ConfigArtifact::Regular { content, node } => {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(path)?;
            crate::commands::LiveRootContent::from_cas(cas, content.clone(), &content.sha256)?
                .copy_verified_to(&mut file)?;
            file.sync_all()?;
            conary_core::generation::root_manifest::apply_resolved_payload_metadata(path, node)?;
        }
        ConfigArtifact::Symlink { target, node, .. } => {
            std::os::unix::fs::symlink(target, path)?;
            conary_core::generation::root_manifest::apply_resolved_payload_metadata(path, node)?;
        }
    }
    Ok(())
}

pub(super) fn create_whiteout(path: &Path) -> Result<()> {
    remove_overlay_entry(path)?;
    let parent = path
        .parent()
        .context("generation whiteout has no parent directory")?;
    fs::create_dir_all(parent)?;
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let status =
        unsafe { libc::mknod(c_path.as_ptr(), libc::S_IFCHR | 0o600, libc::makedev(0, 0)) };
    if status == 0 {
        return Ok(());
    }
    let mknod_error = io::Error::last_os_error();

    fs::File::create(path)?;
    let attr = c"trusted.overlay.whiteout";
    let value = b"y";
    let status = unsafe {
        libc::lsetxattr(
            c_path.as_ptr(),
            attr.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        let xattr_error = io::Error::last_os_error();
        let _ = fs::remove_file(path);
        bail!(
            "failed to create overlay whiteout {} as device ({mknod_error}) or trusted xattr ({xattr_error})",
            path.display()
        )
    }
}

fn copy_overlay_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&from)?;
        if metadata.is_dir() {
            match fs::symlink_metadata(&to) {
                Ok(existing) if existing.is_dir() => {}
                Ok(_) => {
                    remove_overlay_entry(&to)?;
                    fs::create_dir(&to)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&to)?,
                Err(error) => return Err(error.into()),
            }
            copy_overlay_tree(&from, &to)?;
            fs::set_permissions(
                &to,
                fs::Permissions::from_mode(metadata.permissions().mode()),
            )?;
            copy_xattrs(&from, &to)?;
        } else if metadata.file_type().is_symlink() {
            remove_overlay_entry(&to)?;
            std::os::unix::fs::symlink(fs::read_link(&from)?, &to)?;
            copy_xattrs(&from, &to)?;
        } else if metadata.is_file() {
            remove_overlay_entry(&to)?;
            fs::copy(&from, &to)?;
            fs::set_permissions(
                &to,
                fs::Permissions::from_mode(metadata.permissions().mode()),
            )?;
            copy_xattrs(&from, &to)?;
        } else if metadata.file_type().is_char_device() && metadata.rdev() == 0 {
            remove_overlay_entry(&to)?;
            create_whiteout(&to)?;
        } else {
            bail!(
                "unsupported special file in generation /etc upper: {}",
                from.display()
            );
        }
    }
    Ok(())
}

fn copy_xattrs(source: &Path, destination: &Path) -> Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    let size = unsafe { libc::llistxattr(source.as_ptr(), std::ptr::null_mut(), 0) };
    if size < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if size == 0 {
        return Ok(());
    }
    let mut names = vec![0_u8; size as usize];
    let read = unsafe { libc::llistxattr(source.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
    if read < 0 {
        return Err(io::Error::last_os_error().into());
    }
    for name in names[..read as usize].split(|byte| *byte == 0) {
        if name.is_empty() {
            continue;
        }
        let name = CStr::from_bytes_with_nul(
            &name
                .iter()
                .copied()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>(),
        )?
        .to_owned();
        let value_size =
            unsafe { libc::lgetxattr(source.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
        if value_size < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let mut value = vec![0_u8; value_size as usize];
        let value_read = unsafe {
            libc::lgetxattr(
                source.as_ptr(),
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        if value_read < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let status = unsafe {
            libc::lsetxattr(
                destination.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value_read as usize,
                0,
            )
        };
        if status < 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    Ok(())
}

fn sync_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child)?;
        if metadata.is_dir() {
            sync_tree(&child)?;
        } else if metadata.is_file() {
            fs::File::open(&child)?.sync_all()?;
        }
    }
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "config_transaction/tests.rs"]
mod tests;
