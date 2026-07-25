// apps/conary/src/commands/install/inner.rs
//! Inner install helper for callers that own the transaction lifecycle.
//!
//! `install_inner()` performs CAS storage and the DB operations (trove insert,
//! file entries, typed requirements, native lifecycle state, and CCS hooks)
//! using a caller-provided DB
//! transaction and changeset. It does NOT: create/commit a DB transaction,
//! create a changeset, or rebuild a generation from explicit installed state.
//! The caller handles all of those.

use anyhow::{Context, Result, anyhow};
use conary_core::components::ComponentType;
use conary_core::config_transaction::{is_config_artifact_kind, is_etc_config_payload};
use conary_core::db::models::{
    Component, ConfigFile, ConfigSource, FileEntry, InstallSource, InstalledCcsRemoveHook,
    InstalledFileCapability, InstalledNativeLifecycleBundle, InstalledRequirementGroup,
    ProvideEntry, Trove,
};
use conary_core::dependencies::DependencyClass;
use conary_core::filesystem::CasStore;
use conary_core::hash::HashAlgorithm;
use conary_core::payload::{
    PayloadContentAuthority, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};
use conary_core::repository::dependency_model::PackageRelationRemovalMode;
use conary_core::transaction::PackageRelationRemoval;
use conary_core::transaction::TransactionEngine;
use rusqlite::{OptionalExtension, Transaction};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::{info, warn};

use super::file_capabilities::is_regular_file_capability_payload;
use super::{ExtractionResult, TransactionContext, mark_upgraded_parent_deriveds_stale};
#[cfg(test)]
use super::{InstallPhase, InstallProgress};

const LIVE_ROOT_PACKAGE_NAME: &str = "conary-live-root";

/// Result from `install_inner` -- the trove ID of the installed package.
pub struct InnerInstallResult {
    pub trove_id: i64,
    pub retained_upgrade_trove_id: Option<i64>,
    pub retained_relation_trove_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StoredInstallFile {
    pub path: String,
    pub node: PayloadNode,
    pub content: Option<PayloadContentAuthority>,
    /// CAS identity for regular content or an exact symlink-target artifact.
    ///
    /// Non-content filesystem nodes do not receive a fabricated content
    /// identity.
    pub cas_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedInstallFile {
    pub path: String,
    pub node: ResolvedPayloadNode,
    pub content: Option<PayloadContentAuthority>,
    pub cas_hash: Option<String>,
}

/// Execute the install DB operations using a caller-owned DB transaction.
///
/// Stores files in CAS via the provided engine, then inserts the trove,
/// components, files, requirements, lifecycle state, and CCS hooks into the
/// caller-provided transaction under the provided `changeset_id`.
#[cfg(test)]
pub fn install_inner(
    tx: &Transaction<'_>,
    engine: &mut TransactionEngine,
    changeset_id: i64,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    progress: &InstallProgress,
) -> Result<InnerInstallResult> {
    progress.set_phase(pkg.name(), InstallPhase::Deploying);
    let stored_files = store_install_files_in_cas(engine, extraction)?;
    info!(
        "Stored {} files in CAS for {}",
        stored_files.len(),
        pkg.name()
    );
    let resolved_files = resolve_stored_install_files(Path::new(ctx.root), &stored_files)?;
    install_inner_with_stored_files(tx, changeset_id, pkg, extraction, ctx, &resolved_files)
}

pub(super) fn store_install_files_in_cas(
    engine: &TransactionEngine,
    extraction: &ExtractionResult,
) -> Result<Vec<StoredInstallFile>> {
    store_extracted_files_in_cas(engine, &extraction.extracted_files)
}

pub(super) fn store_extracted_files_in_cas(
    engine: &TransactionEngine,
    extracted_files: &[conary_core::packages::traits::ExtractedFile],
) -> Result<Vec<StoredInstallFile>> {
    if engine.cas().algorithm() != HashAlgorithm::Sha256 {
        anyhow::bail!(
            "installed payload authority requires a SHA-256 CAS, found {}",
            engine.cas().algorithm()
        );
    }
    let mut stored_files: Vec<StoredInstallFile> = Vec::with_capacity(extracted_files.len());
    for file in extracted_files {
        file.node
            .validate_content(file.content_authority.as_ref())
            .with_context(|| format!("Invalid payload authority for {}", file.path))?;
        let cas_hash = match &file.node.kind {
            PayloadNodeKind::Regular { .. } => {
                let authority = file.content_authority.as_ref().ok_or_else(|| {
                    anyhow!("regular payload {} has no content authority", file.path)
                })?;
                if file.content.len() as u64 != authority.size {
                    anyhow::bail!(
                        "payload content size mismatch for {}: expected {}, got {}",
                        file.path,
                        authority.size,
                        file.content.len()
                    );
                }
                let actual = CasStore::compute_sha256(&file.content);
                if actual != authority.sha256 {
                    anyhow::bail!(
                        "payload content digest mismatch for {}: expected {}, got {}",
                        file.path,
                        authority.sha256,
                        actual
                    );
                }
                let stored = engine
                    .cas()
                    .store(&file.content)
                    .with_context(|| format!("Failed to store {} in CAS", file.path))?;
                if stored != authority.sha256 {
                    anyhow::bail!(
                        "CAS returned {} for {}, expected authoritative digest {}",
                        stored,
                        file.path,
                        authority.sha256
                    );
                }
                Some(stored)
            }
            PayloadNodeKind::Symlink { target } => {
                if !file.content.is_empty() {
                    anyhow::bail!(
                        "symlink payload {} carries ambiguous archive content",
                        file.path
                    );
                }
                Some(
                    engine
                        .cas()
                        .store_symlink(target)
                        .with_context(|| format!("Failed to store symlink {} in CAS", file.path))?,
                )
            }
            PayloadNodeKind::Directory
            | PayloadNodeKind::Hardlink { .. }
            | PayloadNodeKind::BlockDevice { .. }
            | PayloadNodeKind::CharacterDevice { .. }
            | PayloadNodeKind::Fifo
            | PayloadNodeKind::Socket => {
                if !file.content.is_empty() {
                    anyhow::bail!(
                        "non-content payload node {} carries ambiguous archive bytes",
                        file.path
                    );
                }
                None
            }
        };
        stored_files.push(StoredInstallFile {
            path: file.path.clone(),
            node: file.node.clone(),
            content: file.content_authority.clone(),
            cas_hash,
        });
    }

    Ok(stored_files)
}

pub(super) fn resolve_stored_install_files(
    root: &Path,
    stored_files: &[StoredInstallFile],
) -> Result<Vec<ResolvedInstallFile>> {
    let mut paths = HashSet::with_capacity(stored_files.len());
    for file in stored_files {
        if !paths.insert(file.path.as_str()) {
            anyhow::bail!("payload path {} is declared more than once", file.path);
        }
    }
    let resolved_nodes = super::payload_identity::resolve_payload_nodes(
        root,
        stored_files.iter().map(|file| file.node.clone()),
    )?;
    Ok(stored_files
        .iter()
        .zip(resolved_nodes)
        .map(|(file, node)| ResolvedInstallFile {
            path: file.path.clone(),
            node,
            content: file.content.clone(),
            cas_hash: file.cas_hash.clone(),
        })
        .collect())
}

pub(super) fn install_inner_with_stored_files(
    tx: &Transaction<'_>,
    changeset_id: i64,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    stored_files: &[ResolvedInstallFile],
) -> Result<InnerInstallResult> {
    let package_scheme = pkg.version_scheme();
    if package_scheme != ctx.semantics.version_scheme {
        anyhow::bail!(
            "install semantics for {} declare {} versioning, but the parsed package owns {} versioning",
            pkg.name(),
            ctx.semantics.version_scheme.as_str(),
            package_scheme.as_str()
        );
    }
    if let Some(provenance) = ctx.repository_provenance.as_ref()
        && provenance.version_scheme != package_scheme
    {
        anyhow::bail!(
            "repository provenance for {} declares {} versioning, but the parsed package owns {} versioning",
            pkg.name(),
            provenance.version_scheme.as_str(),
            package_scheme.as_str()
        );
    }
    let is_upgrade = ctx.old_trove_to_upgrade.is_some();

    let selection_reason = ctx.selection_reason;
    let classified = &extraction.classified;
    let language_provides = &extraction.language_provides;
    let trove_id = {
        if let Some(old_trove) = ctx.old_trove_to_upgrade
            && let Some(old_id) = old_trove.id
            && !ctx.retain_replaced_payload_until_lifecycle
        {
            info!("Removing old version {} before upgrade", old_trove.version);
            conary_core::db::models::Trove::delete(tx, old_id)?;
        }

        let mut trove = pkg.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);

        if let Some(provenance) = ctx.repository_provenance.as_ref() {
            trove.install_source = InstallSource::Repository;
            trove.installed_from_repository_id = Some(provenance.repository_id);
            trove.source_distro = provenance.source_distro.clone();
        }

        if let Some(reason) = selection_reason {
            trove.selection_reason = Some(reason.to_string());
        }

        if ctx.repository_provenance.is_none() && trove.install_source == InstallSource::Repository
        {
            let repo_id: Option<i64> = tx
                .query_row(
                    "SELECT r.id FROM repository_packages rp
                     JOIN repositories r ON rp.repository_id = r.id
                     WHERE rp.name = ?1 AND rp.version = ?2
                       AND (?3 IS NULL OR rp.architecture IS NULL OR rp.architecture = ?3)
                     ORDER BY
                         (r.default_strategy_distro = (SELECT distro FROM distro_pin LIMIT 1)) DESC,
                         r.priority DESC, r.id ASC
                     LIMIT 1",
                    rusqlite::params![pkg.name(), pkg.version(), pkg.architecture()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(conary_core::Error::from)?;
            trove.installed_from_repository_id = repo_id;
        }

        let trove_id = trove.insert(tx)?;
        InstalledRequirementGroup::insert_groups(
            tx,
            trove_id,
            ctx.semantics.version_scheme,
            pkg.requirements(),
        )?;
        InstalledRequirementGroup::insert_groups(
            tx,
            trove_id,
            ctx.semantics.version_scheme,
            pkg.relations(),
        )?;

        if let Some(bundle) = ctx.native_lifecycle_bundle {
            let mut installed_bundle =
                InstalledNativeLifecycleBundle::new(trove_id, Some(changeset_id), bundle)?;
            if bundle.source_format == conary_core::ccs::native_lifecycle::SourceFormat::Deb {
                // The bundle row is committed atomically with the unpacked
                // payload. Configuration happens after this transaction.
                installed_bundle.set_lifecycle_state(
                    conary_core::ccs::native_transaction::DebPackageState::Unpacked,
                );
            }
            installed_bundle.insert_or_replace(tx)?;
        }

        let mut path_to_component: HashMap<&str, i64> = HashMap::new();
        if let (Some(component_names), Some(component_names_by_path)) = (
            extraction.installed_component_names.as_ref(),
            extraction.component_names_by_path.as_ref(),
        ) {
            let mut component_ids: HashMap<&str, i64> = HashMap::new();
            for component_name in component_names {
                let mut component = Component::new(trove_id, component_name.clone());
                component.description = Some(format!("{component_name} files"));
                let comp_id = component.insert(tx)?;
                component_ids.insert(component_name.as_str(), comp_id);
            }
            for (path, component_name) in component_names_by_path {
                if let Some(&comp_id) = component_ids.get(component_name.as_str()) {
                    path_to_component.insert(path.as_str(), comp_id);
                }
            }
        } else {
            let mut component_ids: HashMap<ComponentType, i64> = HashMap::new();
            for comp_type in classified.keys() {
                let mut component = Component::from_type(trove_id, *comp_type);
                component.description = Some(format!("{} files", comp_type.as_str()));
                let comp_id = component.insert(tx)?;
                component_ids.insert(*comp_type, comp_id);
            }

            for (comp_type, files) in classified {
                if let Some(&comp_id) = component_ids.get(comp_type) {
                    for path in files {
                        path_to_component.insert(path.as_str(), comp_id);
                    }
                }
            }
        }

        let mut installed_file_metadata: HashMap<String, (i64, Option<String>)> =
            HashMap::with_capacity(stored_files.len());
        for file in stored_files {
            let path = &file.path;
            if let Some(content) = file.content.as_ref() {
                tx.execute(
                    "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                    rusqlite::params![
                        &content.sha256,
                        format!("objects/{}/{}", &content.sha256[0..2], &content.sha256[2..]),
                        i64::try_from(content.size)
                            .context("payload content size exceeds SQLite range")?,
                    ],
                )?;
            }

            let component_id = path_to_component.get(path.as_str()).copied();
            let mut file_entry = FileEntry::new(
                path.clone(),
                file.node.clone(),
                file.content.clone(),
                trove_id,
            );
            file_entry.component_id = component_id;
            let file_id = insert_file_entry_claiming_live_root_overlap(
                tx,
                &mut file_entry,
                pkg.name(),
                ctx.relation_removals,
            )?;
            installed_file_metadata.insert(path.clone(), (file_id, file.cas_hash.clone()));

            let action = if is_upgrade { "modify" } else { "add" };
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    changeset_id,
                    path,
                    file.content.as_ref().map(|content| &content.sha256),
                    action
                ],
            )?;
        }

        persist_config_files(
            tx,
            trove_id,
            pkg,
            ctx,
            &installed_file_metadata,
            stored_files,
            &extraction.extracted_files,
        )?;
        persist_declared_file_capabilities(
            tx,
            trove_id,
            pkg,
            ctx,
            &installed_file_metadata,
            stored_files,
        )?;

        if let Some(hook) = extraction.ccs_remove_hook.as_ref() {
            InstalledCcsRemoveHook::new(trove_id, hook.script.clone(), hook.reversible)
                .insert_or_replace(tx)?;
        }

        for lang_dep in language_provides {
            let kind = match lang_dep.class {
                DependencyClass::Package => "package",
                _ => lang_dep.class.prefix(),
            };
            let mut provide = ProvideEntry::new_typed(
                trove_id,
                kind,
                lang_dep.name.clone(),
                lang_dep.version_constraint.clone(),
            );
            provide.insert_or_ignore(tx)?;
        }

        let mut pkg_provide = ProvideEntry::new(
            trove_id,
            pkg.name().to_string(),
            Some(pkg.version().to_string()),
        );
        pkg_provide.insert_or_ignore(tx)?;

        trove_id
    };

    if let Some(old_trove) = ctx.old_trove_to_upgrade {
        mark_upgraded_parent_deriveds_stale(
            tx,
            pkg.name(),
            Some(&old_trove.version),
            pkg.version(),
        );
    }

    let mut retained_relation_trove_ids = ctx
        .relation_removals
        .iter()
        .map(|removal| removal.trove_id)
        .collect::<Vec<_>>();
    retained_relation_trove_ids.sort_unstable();
    retained_relation_trove_ids.dedup();
    Ok(InnerInstallResult {
        trove_id,
        retained_upgrade_trove_id: ctx
            .retain_replaced_payload_until_lifecycle
            .then(|| ctx.old_trove_to_upgrade.and_then(|trove| trove.id))
            .flatten(),
        retained_relation_trove_ids,
    })
}

fn persist_config_files(
    tx: &Transaction<'_>,
    trove_id: i64,
    pkg: &dyn conary_core::packages::PackageFormat,
    ctx: &TransactionContext<'_>,
    installed_file_metadata: &HashMap<String, (i64, Option<String>)>,
    stored_files: &[ResolvedInstallFile],
    extracted_files: &[conary_core::packages::traits::ExtractedFile],
) -> Result<()> {
    let declared_source = config_source_for_context(ctx);
    let mut declarations = HashMap::new();
    for config_info in pkg.config_files() {
        if declarations
            .insert(config_info.path.as_str(), config_info)
            .is_some()
        {
            anyhow::bail!(
                "package {} declares config path {} more than once",
                pkg.name(),
                config_info.path
            );
        }
        if config_info.remove_on_upgrade {
            if declared_source != ConfigSource::Deb || config_info.ghost {
                anyhow::bail!(
                    "remove-on-upgrade declaration {} from {} is not a Debian conffile",
                    config_info.path,
                    pkg.name()
                );
            }
            super::config_files::persist_debian_remove_on_upgrade(tx, &config_info.path, trove_id)?;
            continue;
        }
        if config_info.ghost {
            let mut config = ConfigFile::new_ghost(config_info.path.clone(), trove_id);
            config.noreplace = config_info.noreplace;
            config.source = declared_source;
            config.upsert(tx)?;
        }
    }

    let mut persisted = HashSet::new();
    for file in stored_files {
        let declaration = declarations.get(file.path.as_str()).copied();
        if declaration.is_none() && !is_etc_config_payload(&file.path, &file.node.source.kind) {
            continue;
        }
        if declaration.is_some_and(|config| config.ghost) {
            anyhow::bail!(
                "ghost config {} from {} unexpectedly has a payload entry",
                file.path,
                pkg.name()
            );
        }
        if declaration.is_some_and(|config| config.remove_on_upgrade) {
            anyhow::bail!(
                "Debian remove-on-upgrade conffile {} from {} unexpectedly has a payload entry",
                file.path,
                pkg.name()
            );
        }
        if !is_config_artifact_kind(&file.node.source.kind) {
            anyhow::bail!(
                "declared config {} from {} is not a regular file or symlink",
                file.path,
                pkg.name()
            );
        }
        let (file_id, hash) = installed_file_metadata.get(&file.path).ok_or_else(|| {
            anyhow!(
                "config payload {} from {} has no installed file identity",
                file.path,
                pkg.name()
            )
        })?;
        let hash = hash.as_ref().ok_or_else(|| {
            anyhow!(
                "config payload {} from {} has no exact content identity",
                file.path,
                pkg.name()
            )
        })?;
        let noreplace = declaration.is_some_and(|config| config.noreplace);
        let mut config = if noreplace {
            ConfigFile::new_noreplace(file.path.clone(), trove_id, hash.clone())
        } else {
            ConfigFile::new(file.path.clone(), trove_id, hash.clone())
        };
        let extracted = extracted_files
            .iter()
            .find(|extracted| extracted.path == file.path)
            .with_context(|| {
                format!(
                    "config payload {} from {} has no extracted content",
                    file.path,
                    pkg.name()
                )
            })?;
        config.original_md5 = super::config_files::debian_original_md5(
            declared_source,
            declaration.is_some(),
            &extracted.content,
        );
        config.file_id = Some(*file_id);
        config.source = declaration.map_or(ConfigSource::Auto, |_| declared_source);
        config.upsert(tx)?;
        persisted.insert(file.path.as_str());
    }

    for config_info in pkg
        .config_files()
        .iter()
        .filter(|config| !config.ghost && !config.remove_on_upgrade)
    {
        if !persisted.contains(config_info.path.as_str()) {
            anyhow::bail!(
                "declared config {} from {} is missing from the installed payload",
                config_info.path,
                pkg.name()
            );
        }
    }

    Ok(())
}

fn persist_declared_file_capabilities(
    tx: &Transaction<'_>,
    trove_id: i64,
    pkg: &dyn conary_core::packages::PackageFormat,
    ctx: &TransactionContext<'_>,
    installed_file_metadata: &HashMap<String, (i64, Option<String>)>,
    stored_files: &[ResolvedInstallFile],
) -> Result<()> {
    let Some(file_capabilities) = ctx.ccs_file_capabilities else {
        return Ok(());
    };

    let stored_files_by_path = stored_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<HashMap<_, _>>();
    let selected = file_capabilities
        .iter()
        .filter(|capability| {
            if !installed_file_metadata.contains_key(&capability.path) {
                warn!(
                    "Skipping declared file capability {} for {} because it was not installed",
                    capability.path,
                    pkg.name()
                );
                return false;
            }
            true
        })
        .map(|capability| {
            let stored_file = stored_files_by_path
                .get(capability.path.as_str())
                .ok_or_else(|| {
                    anyhow!(
                        "file capability target {} for {} is missing stored file metadata",
                        capability.path,
                        pkg.name()
                    )
                })?;
            if !is_regular_file_capability_payload(&stored_file.node.source.kind) {
                anyhow::bail!(
                    "file capability target {} for {} is not a regular installed file",
                    capability.path,
                    pkg.name()
                );
            }
            Ok(capability.clone())
        })
        .collect::<Result<Vec<_>>>()?;

    InstalledFileCapability::replace_for_trove(tx, trove_id, &selected)
        .with_context(|| format!("Failed to persist CCS file capabilities for {}", pkg.name()))?;

    Ok(())
}

fn config_source_for_context(ctx: &TransactionContext<'_>) -> ConfigSource {
    match ctx.semantics.source {
        super::PreparedSourceKind::NativePackage { format } => match format {
            crate::commands::PackageFormatType::Rpm => ConfigSource::Rpm,
            crate::commands::PackageFormatType::Deb => ConfigSource::Deb,
            crate::commands::PackageFormatType::Arch => ConfigSource::Arch,
        },
        super::PreparedSourceKind::Ccs => ConfigSource::Auto,
    }
}

pub(super) fn preflight_file_ownership(
    conn: &rusqlite::Connection,
    paths: impl IntoIterator<Item = impl AsRef<str>>,
    package_name: &str,
    relation_removals: &[PackageRelationRemoval],
) -> Result<()> {
    for path in paths {
        let path = path.as_ref();
        let Some(existing) = FileEntry::find_by_path(conn, path)? else {
            continue;
        };

        let owner = Trove::find_by_id(conn, existing.trove_id)?.ok_or_else(|| {
            anyhow!(
                "Path {} is already tracked by missing trove {}",
                path,
                existing.trove_id
            )
        })?;

        if owner.name == LIVE_ROOT_PACKAGE_NAME
            || owner.name == package_name
            || relation_removals.iter().any(|removal| {
                removal.trove_id == existing.trove_id
                    && removal.mode == PackageRelationRemovalMode::OwnershipTransfer
                    && removal
                        .ownership_transfer_packages
                        .iter()
                        .any(|incoming| incoming == package_name)
            })
        {
            continue;
        }

        return Err(anyhow!(
            "Path {} is already tracked by package {}",
            path,
            owner.name
        ));
    }

    Ok(())
}

pub(super) fn insert_file_entry_claiming_live_root_overlap(
    tx: &Transaction<'_>,
    file_entry: &mut FileEntry,
    package_name: &str,
    relation_removals: &[PackageRelationRemoval],
) -> Result<i64> {
    let Some(existing) = FileEntry::find_by_path(tx, &file_entry.path)? else {
        return Ok(file_entry.insert(tx)?);
    };

    let owner = Trove::find_by_id(tx, existing.trove_id)?.ok_or_else(|| {
        anyhow!(
            "Path {} is already tracked by missing trove {}",
            file_entry.path,
            existing.trove_id
        )
    })?;

    if owner.name == LIVE_ROOT_PACKAGE_NAME
        || owner.name == package_name
        || relation_removals.iter().any(|removal| {
            removal.trove_id == existing.trove_id
                && removal.mode == PackageRelationRemovalMode::OwnershipTransfer
                && removal
                    .ownership_transfer_packages
                    .iter()
                    .any(|incoming| incoming == package_name)
        })
    {
        info!(
            "Claiming {} from tracked package {} for {}",
            file_entry.path, owner.name, package_name
        );
        return Ok(file_entry.insert_or_replace(tx)?);
    }

    Err(anyhow!(
        "Path {} is already tracked by package {}",
        file_entry.path,
        owner.name
    ))
}

#[cfg(test)]
#[path = "inner/tests.rs"]
mod tests;
