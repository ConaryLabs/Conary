// src/commands/mod.rs
//! Command handlers for the Conary CLI

mod adopt;
mod collection;
mod config;
mod install;
mod label;
pub mod progress;
mod query;
mod repo;
mod restore;
mod state;
mod system;
mod triggers;
mod update;

// Re-export all command handlers
pub use adopt::{cmd_adopt, cmd_adopt_status, cmd_adopt_system, cmd_conflicts};
pub use collection::{
    cmd_collection_add, cmd_collection_create, cmd_collection_delete, cmd_collection_install,
    cmd_collection_list, cmd_collection_remove_member, cmd_collection_show,
};
pub use config::{
    cmd_config_backup, cmd_config_backups, cmd_config_check, cmd_config_diff, cmd_config_list,
    cmd_config_restore,
};
pub use install::{cmd_autoremove, cmd_install, cmd_remove};
pub use label::{cmd_label_add, cmd_label_list, cmd_label_path, cmd_label_query, cmd_label_remove, cmd_label_set, cmd_label_show};
// cmd_scripts is defined in this module, no need to re-export from submodule
pub use query::{cmd_depends, cmd_deptree, cmd_history, cmd_list_components, cmd_query, cmd_query_component, cmd_query_reason, cmd_rdepends, cmd_repquery, cmd_whatbreaks, cmd_whatprovides, QueryOptions};
pub use repo::{
    cmd_key_import, cmd_key_list, cmd_key_remove, cmd_repo_add, cmd_repo_disable,
    cmd_repo_enable, cmd_repo_list, cmd_repo_remove, cmd_repo_sync, cmd_search,
};
pub use restore::{cmd_restore, cmd_restore_all};
pub use state::{
    cmd_state_create, cmd_state_diff, cmd_state_list, cmd_state_prune, cmd_state_restore,
    cmd_state_show,
};
pub use system::{cmd_init, cmd_rollback, cmd_verify};
pub use triggers::{
    cmd_trigger_add, cmd_trigger_disable, cmd_trigger_enable, cmd_trigger_list,
    cmd_trigger_remove, cmd_trigger_run, cmd_trigger_show,
};
pub use update::{cmd_delta_stats, cmd_list_pinned, cmd_pin, cmd_unpin, cmd_update, cmd_update_group};

use anyhow::Result;
use conary::components::{ComponentClassifier, ComponentType};
use conary::db::models::{Component, ConfigFile, ConfigSource, ProvideEntry};
use conary::dependencies::LanguageDepDetector;
use conary::packages::arch::ArchPackage;
use conary::packages::deb::DebPackage;
use conary::packages::rpm::RpmPackage;
use conary::packages::traits::{DependencyType, ScriptletPhase};
use conary::packages::PackageFormat;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::info;

/// Package format types we support
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormatType {
    Rpm,
    Deb,
    Arch,
}

/// Detect package format from file path and magic bytes
pub fn detect_package_format(path: &str) -> Result<PackageFormatType> {
    // First try file extension
    if path.ends_with(".rpm") {
        return Ok(PackageFormatType::Rpm);
    } else if path.ends_with(".deb") {
        return Ok(PackageFormatType::Deb);
    } else if path.ends_with(".pkg.tar.zst") || path.ends_with(".pkg.tar.xz") {
        return Ok(PackageFormatType::Arch);
    }

    // Fallback to magic bytes detection
    let mut file = File::open(path)?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;

    // RPM magic: 0xED 0xAB 0xEE 0xDB
    if magic[0..4] == [0xED, 0xAB, 0xEE, 0xDB] {
        return Ok(PackageFormatType::Rpm);
    }

    // DEB magic: "!<arch>\n"
    if magic[0..7] == *b"!<arch>" {
        return Ok(PackageFormatType::Deb);
    }

    // Arch: zstd magic
    if magic[0..4] == [0x28, 0xB5, 0x2F, 0xFD] {
        return Ok(PackageFormatType::Arch);
    }

    // Arch: xz magic
    if magic[0..6] == [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00] {
        return Ok(PackageFormatType::Arch);
    }

    Err(anyhow::anyhow!("Unable to detect package format for: {}", path))
}

/// Install a package from a file path (used by install and update commands)
///
/// # Arguments
/// * `package_path` - Path to the package file
/// * `conn` - Database connection
/// * `root` - Filesystem root for installation
/// * `db_path` - Path to the database
/// * `old_trove` - Previous version being upgraded (if any)
/// * `selection_reason` - Human-readable reason for installation (e.g., "Required by nginx")
pub fn install_package_from_file(
    package_path: &Path,
    conn: &mut rusqlite::Connection,
    root: &str,
    db_path: &str,
    old_trove: Option<&conary::db::models::Trove>,
    selection_reason: Option<&str>,
) -> Result<()> {
    let path_str = package_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;
    let format = detect_package_format(path_str)?;
    info!("Detected package format: {:?}", format);

    let package: Box<dyn PackageFormat> = match format {
        PackageFormatType::Rpm => Box::new(RpmPackage::parse(path_str)?),
        PackageFormatType::Deb => Box::new(DebPackage::parse(path_str)?),
        PackageFormatType::Arch => Box::new(ArchPackage::parse(path_str)?),
    };

    info!(
        "Parsed package: {} version {} ({} files, {} dependencies)",
        package.name(),
        package.version(),
        package.files().len(),
        package.dependencies().len()
    );

    info!("Extracting file contents from package...");
    let extracted_files = package.extract_file_contents()?;
    info!("Extracted {} files", extracted_files.len());

    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    // Classify files into components
    let file_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let classified = ComponentClassifier::classify_all(&file_paths);
    info!(
        "Classified {} files into {} component types",
        file_paths.len(),
        classified.len()
    );

    conary::db::transaction(conn, |tx| {
        let changeset_desc = if let Some(old) = old_trove {
            format!(
                "Upgrade {} from {} to {}",
                package.name(),
                old.version,
                package.version()
            )
        } else {
            format!("Install {}-{}", package.name(), package.version())
        };
        let mut changeset = conary::db::models::Changeset::new(changeset_desc);
        let changeset_id = changeset.insert(tx)?;

        if let Some(old) = old_trove
            && let Some(old_id) = old.id
        {
            info!("Removing old version {} before upgrade", old.version);
            conary::db::models::Trove::delete(tx, old_id)?;
        }

        let mut trove = package.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);

        // Set selection reason if provided (e.g., for dependencies)
        if let Some(reason) = selection_reason {
            trove.selection_reason = Some(reason.to_string());
            // If it's a dependency reason, also set install_reason
            if reason.starts_with("Required by") {
                trove.install_reason = conary::db::models::InstallReason::Dependency;
            }
        }

        let trove_id = trove.insert(tx)?;

        // Create components and build path-to-component-id mapping
        let mut component_ids: HashMap<ComponentType, i64> = HashMap::new();
        for comp_type in classified.keys() {
            let mut component = Component::from_type(trove_id, *comp_type);
            component.description = Some(format!("{} files", comp_type.as_str()));
            let comp_id = component.insert(tx)?;
            component_ids.insert(*comp_type, comp_id);
            info!("Created component :{} (id={})", comp_type.as_str(), comp_id);
        }

        // Build path-to-component-id lookup for efficient file insertion
        let mut path_to_component: HashMap<&str, i64> = HashMap::new();
        for (comp_type, files) in &classified {
            if let Some(&comp_id) = component_ids.get(comp_type) {
                for path in files {
                    path_to_component.insert(path.as_str(), comp_id);
                }
            }
        }

        for file in &extracted_files {
            if deployer.file_exists(&file.path) {
                if let Some(existing) =
                    conary::db::models::FileEntry::find_by_path(tx, &file.path)?
                {
                    let owner_trove =
                        conary::db::models::Trove::find_by_id(tx, existing.trove_id)?;
                    if let Some(owner) = owner_trove
                        && owner.name != package.name()
                    {
                        return Err(conary::Error::InitError(format!(
                            "File conflict: {} is owned by package {}",
                            file.path, owner.name
                        )));
                    }
                } else if old_trove.is_none() {
                    return Err(conary::Error::InitError(format!(
                        "File conflict: {} exists but is not tracked by any package",
                        file.path
                    )));
                }
            }

            let hash = deployer.cas().store(&file.content)?;
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                [&hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &file.size.to_string()],
            )?;

            // Look up the component ID for this file
            let component_id = path_to_component.get(file.path.as_str()).copied();

            let mut file_entry = conary::db::models::FileEntry::new(
                file.path.clone(),
                hash.clone(),
                file.size,
                file.mode,
                trove_id,
            );
            file_entry.component_id = component_id;
            file_entry.insert(tx)?;

            let action = if deployer.file_exists(&file.path) {
                "modify"
            } else {
                "add"
            };
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                [&changeset_id.to_string(), &file.path, &hash, action],
            )?;
        }

        for dep in package.dependencies() {
            let dep_type_str = match dep.dep_type {
                DependencyType::Runtime => "runtime",
                DependencyType::Build => "build",
                DependencyType::Optional => "optional",
            };
            let mut dep_entry = conary::db::models::DependencyEntry::new(
                trove_id,
                dep.name.clone(),
                dep.version.clone(),
                dep_type_str.to_string(),
                None,
            );
            dep_entry.insert(tx)?;
        }

        // Detect and store language-specific provides (python, perl, ruby, etc.)
        let language_provides = LanguageDepDetector::detect_all_provides(&file_paths);
        if !language_provides.is_empty() {
            info!("Detected {} language-specific provides", language_provides.len());
        }
        for lang_dep in &language_provides {
            let mut provide = ProvideEntry::new(
                trove_id,
                lang_dep.to_dep_string(),
                lang_dep.version_constraint.clone(),
            );
            provide.insert_or_ignore(tx)?;
        }

        // Also store the package name itself as a provide
        let mut pkg_provide = ProvideEntry::new(
            trove_id,
            package.name().to_string(),
            Some(package.version().to_string()),
        );
        pkg_provide.insert_or_ignore(tx)?;

        // Track configuration files from the package
        let pkg_config_files = package.config_files();
        if !pkg_config_files.is_empty() {
            info!("Tracking {} configuration files", pkg_config_files.len());

            // Build path-to-hash map from extracted files
            let path_to_hash: HashMap<&str, String> = extracted_files
                .iter()
                .map(|f| (f.path.as_str(), conary::filesystem::CasStore::compute_hash(&f.content)))
                .collect();

            // Determine config source from package format
            let config_source = match format {
                PackageFormatType::Rpm => ConfigSource::Rpm,
                PackageFormatType::Deb => ConfigSource::Deb,
                PackageFormatType::Arch => ConfigSource::Arch,
            };

            for config_info in &pkg_config_files {
                if let Some(hash) = path_to_hash.get(config_info.path.as_str()) {
                    let mut config_file = ConfigFile::new(
                        config_info.path.clone(),
                        trove_id,
                        hash.clone(),
                    );
                    config_file.noreplace = config_info.noreplace;
                    config_file.source = config_source;

                    // Use upsert in case the file is already tracked (e.g., upgrade scenario)
                    config_file.upsert(tx)?;
                    info!("  Tracked config: {} (noreplace={})", config_info.path, config_info.noreplace);
                }
            }
        }

        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;
        Ok(())
    })?;

    info!("Deploying files to filesystem...");
    for file in &extracted_files {
        let hash = conary::filesystem::CasStore::compute_hash(&file.content);
        deployer.deploy_file(&file.path, &hash, file.mode as u32)?;
    }
    info!("Successfully deployed {} files", extracted_files.len());

    Ok(())
}

/// Display scriptlets from a package file
pub fn cmd_scripts(package_path: &str) -> Result<()> {
    let format = detect_package_format(package_path)?;

    let package: Box<dyn PackageFormat> = match format {
        PackageFormatType::Rpm => Box::new(RpmPackage::parse(package_path)?),
        PackageFormatType::Deb => Box::new(DebPackage::parse(package_path)?),
        PackageFormatType::Arch => Box::new(ArchPackage::parse(package_path)?),
    };

    let scriptlets = package.scriptlets();

    if scriptlets.is_empty() {
        println!("[INFO] {} v{} has no scriptlets", package.name(), package.version());
        return Ok(());
    }

    println!("Package: {} v{}", package.name(), package.version());
    println!("Scriptlets: {}", scriptlets.len());
    println!();

    for scriptlet in scriptlets {
        let phase_name = match scriptlet.phase {
            ScriptletPhase::PreInstall => "pre-install",
            ScriptletPhase::PostInstall => "post-install",
            ScriptletPhase::PreRemove => "pre-remove",
            ScriptletPhase::PostRemove => "post-remove",
            ScriptletPhase::PreUpgrade => "pre-upgrade",
            ScriptletPhase::PostUpgrade => "post-upgrade",
            ScriptletPhase::PreTransaction => "pre-transaction",
            ScriptletPhase::PostTransaction => "post-transaction",
            ScriptletPhase::Trigger => "trigger",
        };

        println!("=== {} ===", phase_name);
        println!("Interpreter: {}", scriptlet.interpreter);
        if let Some(flags) = &scriptlet.flags {
            println!("Flags: {}", flags);
        }
        println!("---");
        // Print script content
        for line in scriptlet.content.lines() {
            println!("{}", line);
        }
        println!("---");
        println!();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format_from_rpm_extension() {
        let result = detect_package_format("/path/to/package.rpm");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageFormatType::Rpm);
    }

    #[test]
    fn test_detect_format_from_deb_extension() {
        let result = detect_package_format("/path/to/package.deb");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageFormatType::Deb);
    }

    #[test]
    fn test_detect_format_from_arch_extension() {
        let result = detect_package_format("/path/to/package.pkg.tar.zst");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageFormatType::Arch);
    }
}
