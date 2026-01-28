// src/commands/mod.rs
//! Command handlers for the Conary CLI

mod adopt;
mod automation;
mod bootstrap;
mod capability;
pub mod ccs;
mod collection;
mod config;
mod convert_pkgbuild;
mod cook;
mod derived;
mod federation;
mod install;
mod label;
mod model;
mod provenance;
mod redirect;
mod remove;
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
pub use capability::{
    cmd_capability_show, cmd_capability_validate, cmd_capability_list,
    cmd_capability_generate, cmd_capability_audit, cmd_capability_run,
};
pub use automation::{
    cmd_automation_status, cmd_automation_check, cmd_automation_apply,
    cmd_automation_configure, cmd_automation_daemon, cmd_automation_history,
};
#[cfg(feature = "experimental")]
pub use automation::{cmd_ai_find, cmd_ai_translate, cmd_ai_query, cmd_ai_explain};
pub use bootstrap::{
    cmd_bootstrap_base, cmd_bootstrap_check, cmd_bootstrap_clean, cmd_bootstrap_image,
    cmd_bootstrap_init, cmd_bootstrap_resume, cmd_bootstrap_stage0, cmd_bootstrap_stage1,
    cmd_bootstrap_status,
};
pub use collection::{
    cmd_collection_add, cmd_collection_create, cmd_collection_delete, cmd_collection_install,
    cmd_collection_list, cmd_collection_remove_member, cmd_collection_show,
};
pub use config::{
    cmd_config_backup, cmd_config_backups, cmd_config_check, cmd_config_diff, cmd_config_list,
    cmd_config_restore,
};
pub use derived::{
    cmd_derive_build, cmd_derive_create, cmd_derive_delete, cmd_derive_list,
    cmd_derive_override, cmd_derive_patch, cmd_derive_show, cmd_derive_stale,
};
pub use convert_pkgbuild::cmd_convert_pkgbuild;
pub use cook::cmd_cook;
pub use install::cmd_install;
pub use model::{cmd_model_apply, cmd_model_check, cmd_model_diff, cmd_model_publish, cmd_model_snapshot};
pub use provenance::{
    cmd_provenance_show, cmd_provenance_verify, cmd_provenance_diff, cmd_provenance_find_by_dep,
    cmd_provenance_export, cmd_provenance_register, cmd_provenance_audit,
};
pub use remove::{cmd_autoremove, cmd_remove};
pub use conary::scriptlet::SandboxMode;
pub use label::{cmd_label_add, cmd_label_delegate, cmd_label_link, cmd_label_list, cmd_label_path, cmd_label_query, cmd_label_remove, cmd_label_set, cmd_label_show};
// cmd_scripts is defined in this module, no need to re-export from submodule
pub use query::{cmd_depends, cmd_deptree, cmd_history, cmd_list_components, cmd_query, cmd_query_component, cmd_query_reason, cmd_rdepends, cmd_repquery, cmd_sbom, cmd_whatbreaks, cmd_whatprovides, QueryOptions};
pub use redirect::{
    cmd_redirect_add, cmd_redirect_list, cmd_redirect_remove, cmd_redirect_resolve,
    cmd_redirect_show,
};
pub use repo::{
    cmd_key_import, cmd_key_list, cmd_key_remove, cmd_repo_add, cmd_repo_disable,
    cmd_repo_enable, cmd_repo_list, cmd_repo_remove, cmd_repo_sync, cmd_search,
};
pub use restore::{cmd_restore, cmd_restore_all};
pub use state::{
    cmd_state_create, cmd_state_diff, cmd_state_list, cmd_state_prune, cmd_state_restore,
    cmd_state_show,
};
pub use system::{cmd_gc, cmd_init, cmd_rollback, cmd_verify};
pub use triggers::{
    cmd_trigger_add, cmd_trigger_disable, cmd_trigger_enable, cmd_trigger_list,
    cmd_trigger_remove, cmd_trigger_run, cmd_trigger_show,
};
pub use update::{cmd_delta_stats, cmd_list_pinned, cmd_pin, cmd_unpin, cmd_update, cmd_update_group};
pub use federation::{
    cmd_federation_status, cmd_federation_peers, cmd_federation_add_peer,
    cmd_federation_remove_peer, cmd_federation_stats, cmd_federation_enable_peer,
    cmd_federation_test,
};
#[cfg(feature = "server")]
pub use federation::cmd_federation_scan;

use anyhow::Result;
use conary::packages::arch::ArchPackage;
use conary::packages::deb::DebPackage;
use conary::packages::rpm::RpmPackage;
use conary::packages::traits::ScriptletPhase;
use conary::packages::PackageFormat;
use std::fs::File;
use std::io::Read;

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

/// Create a state snapshot after a successful operation
pub(crate) fn create_state_snapshot(conn: &rusqlite::Connection, changeset_id: i64, summary: &str) -> Result<()> {
    use conary::db::models::StateEngine;
    use tracing::{info, warn};

    let engine = StateEngine::new(conn);
    match engine.create_snapshot(summary, None, Some(changeset_id)) {
        Ok(state) => {
            info!("Created state {} ({})", state.state_number, summary);
        }
        Err(e) => {
            warn!("Failed to create state snapshot: {}", e);
            // Don't fail the operation if snapshot creation fails
        }
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
