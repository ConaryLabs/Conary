// apps/conary/src/commands/install/batch/config.rs
//! Batch-owned configuration capture and persisted config identity.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use conary_core::config_transaction::{is_config_artifact_kind, is_etc_config_payload};
use conary_core::db::models::{ConfigFile, ConfigSource, InstalledNativeLifecycleBundle};
use rusqlite::{Connection, Transaction};

use super::{BatchInstaller, PackageFormatType, PreparedPackage, inner};

impl BatchInstaller<'_> {
    pub(super) fn retains_old_payload_for_lifecycle(
        conn: &Connection,
        package: &PreparedPackage,
    ) -> Result<bool> {
        let installed_bundle = package
            .old_trove_id()?
            .map(|trove_id| InstalledNativeLifecycleBundle::find_by_trove(conn, trove_id))
            .transpose()?
            .flatten()
            .is_some();
        Ok(!package.relation_removals.is_empty()
            || package.native_lifecycle_state.bundle_to_persist.is_some()
            || installed_bundle)
    }

    pub(super) fn insert_config_rows(
        tx: &Transaction<'_>,
        pkg: &PreparedPackage,
        trove_id: i64,
        installed_file_metadata: &HashMap<String, (i64, Option<String>)>,
        stored_files: &[inner::ResolvedInstallFile],
    ) -> Result<()> {
        let declared_source = source_for_format(pkg.format);

        let mut declarations = HashMap::new();
        for config_info in &pkg.config_files {
            if declarations
                .insert(config_info.path.as_str(), config_info)
                .is_some()
            {
                anyhow::bail!(
                    "package {} declares config path {} more than once",
                    pkg.name,
                    config_info.path
                );
            }
            if config_info.remove_on_upgrade {
                if declared_source != ConfigSource::Deb || config_info.ghost {
                    anyhow::bail!(
                        "remove-on-upgrade declaration {} from {} is not a Debian conffile",
                        config_info.path,
                        pkg.name
                    );
                }
                super::super::config_files::persist_debian_remove_on_upgrade(
                    tx,
                    &config_info.path,
                    trove_id,
                )?;
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
                    pkg.name
                );
            }
            if declaration.is_some_and(|config| config.remove_on_upgrade) {
                anyhow::bail!(
                    "Debian remove-on-upgrade conffile {} from {} unexpectedly has a payload entry",
                    file.path,
                    pkg.name
                );
            }
            if !is_config_artifact_kind(&file.node.source.kind) {
                anyhow::bail!(
                    "declared config {} from {} is not a regular file or symlink",
                    file.path,
                    pkg.name
                );
            }
            let (file_id, hash) = installed_file_metadata.get(&file.path).ok_or_else(|| {
                anyhow::anyhow!(
                    "config payload {} from {} has no installed file identity",
                    file.path,
                    pkg.name
                )
            })?;
            let hash = hash.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "config payload {} from {} has no exact content identity",
                    file.path,
                    pkg.name
                )
            })?;
            let noreplace = declaration.is_some_and(|config| config.noreplace);
            let mut config = if noreplace {
                ConfigFile::new_noreplace(file.path.clone(), trove_id, hash.clone())
            } else {
                ConfigFile::new(file.path.clone(), trove_id, hash.clone())
            };
            let extracted = pkg
                .extracted_files
                .iter()
                .find(|extracted| extracted.path == file.path)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "config payload {} from {} has no extracted content",
                        file.path,
                        pkg.name
                    )
                })?;
            config.original_md5 = super::super::config_files::debian_original_md5(
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
            .config_files
            .iter()
            .filter(|config| !config.ghost && !config.remove_on_upgrade)
        {
            if !persisted.contains(config_info.path.as_str()) {
                anyhow::bail!(
                    "declared config {} from {} is missing from the installed payload",
                    config_info.path,
                    pkg.name
                );
            }
        }

        Ok(())
    }
}

fn source_for_format(format: PackageFormatType) -> ConfigSource {
    match format {
        PackageFormatType::Rpm => ConfigSource::Rpm,
        PackageFormatType::Deb => ConfigSource::Deb,
        PackageFormatType::Arch => ConfigSource::Arch,
    }
}
