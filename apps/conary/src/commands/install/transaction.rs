// src/commands/install/transaction.rs

use super::{ExtractionResult, InstallSemantics, RepositoryInstallProvenance};
use anyhow::{Context, Result};
use conary_core::db::models::{ConfigFile, ConfigSource, ProvideEntry};
use conary_core::packages::PackageFormat;
use conary_core::transaction::{PackageRelationDeconfiguration, PackageRelationRemoval};

#[path = "transaction/selected_root.rs"]
mod selected_root;

pub(super) use selected_root::{
    execute_install_transaction_in_selected_root,
    execute_install_transaction_in_selected_root_with_post_graph,
};

/// Typed inputs for one selected-root install transaction.
pub(super) struct TransactionContext<'a> {
    pub(super) db_path: &'a str,
    /// Exact isolated root selected for this transaction.
    pub(super) root: &'a str,
    pub(super) semantics: InstallSemantics,
    pub(super) selection_reason: Option<&'a str>,
    pub(super) old_trove_to_upgrade: Option<&'a conary_core::db::models::Trove>,
    pub(super) ccs_capabilities: Option<&'a conary_core::capability::CapabilityDeclaration>,
    pub(super) ccs_file_capabilities: Option<&'a [conary_core::ccs::manifest::FileCapability]>,
    pub(super) defer_generation: bool,
    pub(super) repository_provenance: Option<RepositoryInstallProvenance>,
    pub(super) native_lifecycle_bundle:
        Option<&'a conary_core::ccs::native_lifecycle::NativeLifecycleBundle>,
    /// Exact installed packages that the incoming native relation contract
    /// authorizes this transaction to remove.
    pub(super) relation_removals: &'a [PackageRelationRemoval],
    /// Exact installed packages whose configured state is lowered by the
    /// native transaction while their payload and trove remain installed.
    pub(super) relation_deconfigurations: &'a [PackageRelationDeconfiguration],
    /// Keep obsolete files visible until the typed package-manager lifecycle
    /// reaches its payload-removal boundary inside the selected-root graph.
    pub(super) retain_replaced_payload_until_lifecycle: bool,
}

/// Result from a successful selected-root transaction and publication attempt.
pub(super) struct InstallTransactionResult {
    pub(super) trove_id: i64,
    pub(super) changeset_id: i64,
    pub(super) triggers_executed: bool,
}

pub(super) fn delete_non_residual_config_rows(
    conn: &rusqlite::Connection,
    trove_id: i64,
) -> Result<()> {
    for config in ConfigFile::find_by_trove(conn, trove_id)? {
        if config.source == ConfigSource::Deb {
            continue;
        }
        ConfigFile::delete(
            conn,
            config
                .id
                .context("tracked config file has no database identity")?,
        )?;
    }
    Ok(())
}

pub(super) fn preflight_generation_file_capabilities_for_install(
    ctx: &TransactionContext<'_>,
    extraction: &ExtractionResult,
) -> Result<()> {
    preflight_generation_file_capabilities(
        ctx.ccs_file_capabilities,
        ctx.defer_generation,
        &extraction.extracted_files,
    )
}

pub(super) fn preflight_generation_file_capabilities(
    file_capabilities: Option<&[conary_core::ccs::manifest::FileCapability]>,
    defer_generation: bool,
    extracted_files: &[conary_core::packages::payload::PackagePayloadFile],
) -> Result<()> {
    preflight_generation_file_capabilities_with_xattr_support(
        file_capabilities,
        defer_generation,
        extracted_files,
        conary_core::generation::builder::erofs_xattr_image_support_available(),
    )
}

fn preflight_selected_generation_file_capability_targets(
    file_capabilities: Option<&[conary_core::ccs::manifest::FileCapability]>,
    extracted_files: &[conary_core::packages::payload::PackagePayloadFile],
) -> Result<()> {
    let Some(file_capabilities) = file_capabilities else {
        return Ok(());
    };
    if file_capabilities.is_empty() {
        return Ok(());
    }

    for capability in file_capabilities {
        let Some(selected_file) = extracted_files
            .iter()
            .find(|file| file.path == capability.path)
        else {
            continue;
        };
        if conary_core::generation::metadata::is_excluded(&capability.path) {
            anyhow::bail!(
                "CCS file_capabilities target {} is excluded from generation; selected-root installs require file capability authority to be represented in the generated artifact",
                capability.path
            );
        }
        if !super::file_capabilities::is_regular_file_capability_payload(&selected_file.node.kind) {
            anyhow::bail!(
                "CCS file_capabilities target {} is not a regular installed file; selected-root installs require file capability authority on regular generated payload files",
                capability.path
            );
        }
    }

    Ok(())
}

fn preflight_generation_file_capabilities_with_xattr_support(
    file_capabilities: Option<&[conary_core::ccs::manifest::FileCapability]>,
    defer_generation: bool,
    extracted_files: &[conary_core::packages::payload::PackagePayloadFile],
    xattr_image_support_available: bool,
) -> Result<()> {
    let Some(file_capabilities) = file_capabilities else {
        return Ok(());
    };
    if file_capabilities.is_empty() {
        return Ok(());
    }
    for capability in file_capabilities {
        capability.validate()?;
    }
    if defer_generation {
        anyhow::bail!(
            "CCS file_capabilities require immediate generation publication and cannot use --defer-generation"
        );
    }
    if !xattr_image_support_available {
        anyhow::bail!(
            "CCS file_capabilities require generation image xattr propagation, but generation image xattr propagation is unavailable in this build"
        );
    }
    preflight_selected_generation_file_capability_targets(Some(file_capabilities), extracted_files)
}

pub(super) fn persist_package_provides(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    package: &dyn PackageFormat,
) -> Result<()> {
    let provides = package.resolution_capabilities()?;
    persist_declared_provides(
        tx,
        trove_id,
        package.name(),
        package.version(),
        package.version_scheme(),
        &provides,
    )
}

pub(super) fn persist_declared_provides(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    package_name: &str,
    package_version: &str,
    package_scheme: conary_core::repository::versioning::VersionScheme,
    provides: &[conary_core::repository::dependency_model::ProvidedCapability],
) -> Result<()> {
    ProvideEntry::insert_package_capabilities(
        tx,
        trove_id,
        package_name,
        package_version,
        package_scheme,
        provides,
    )?;
    Ok(())
}
