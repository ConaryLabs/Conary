// src/commands/install/transaction.rs

use super::{ExtractionResult, InstallSemantics, RepositoryInstallProvenance};
use anyhow::{Context, Result};
use conary_core::db::models::{ConfigFile, ConfigSource, ProvideEntry};
use conary_core::dependencies::DependencyClass;
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
    pub(super) ccs_manifest_provides: Option<&'a conary_core::ccs::manifest::Provides>,
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
    preflight_generation_file_capabilities(ctx)?;
    preflight_selected_generation_file_capability_targets(ctx, extraction)
}

fn preflight_generation_file_capabilities(ctx: &TransactionContext<'_>) -> Result<()> {
    preflight_generation_file_capabilities_with_xattr_support(
        ctx,
        conary_core::generation::builder::erofs_xattr_image_support_available(),
    )
}

fn preflight_selected_generation_file_capability_targets(
    ctx: &TransactionContext<'_>,
    extraction: &ExtractionResult,
) -> Result<()> {
    let Some(file_capabilities) = ctx.ccs_file_capabilities else {
        return Ok(());
    };
    if file_capabilities.is_empty() {
        return Ok(());
    }

    for capability in file_capabilities {
        let Some(selected_file) = extraction
            .extracted_files
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
    ctx: &TransactionContext<'_>,
    xattr_image_support_available: bool,
) -> Result<()> {
    let Some(file_capabilities) = ctx.ccs_file_capabilities else {
        return Ok(());
    };
    if file_capabilities.is_empty() {
        return Ok(());
    }
    for capability in file_capabilities {
        capability.validate()?;
    }
    if ctx.defer_generation {
        anyhow::bail!(
            "CCS file_capabilities require immediate generation publication and cannot use --defer-generation"
        );
    }
    if !xattr_image_support_available {
        anyhow::bail!(
            "CCS file_capabilities require generation image xattr propagation, but generation image xattr propagation is unavailable in this build"
        );
    }
    Ok(())
}

fn persist_ccs_manifest_provides(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    package_name: &str,
    provides: &conary_core::ccs::manifest::Provides,
) -> Result<()> {
    for capability in &provides.capabilities {
        if capability == package_name {
            continue;
        }
        let mut provide = ProvideEntry::new(trove_id, capability.clone(), None);
        provide.insert_or_ignore(tx)?;
    }

    for soname in &provides.sonames {
        insert_ccs_manifest_typed_provide(tx, trove_id, DependencyClass::Soname.prefix(), soname)?;
    }
    for binary in &provides.binaries {
        insert_ccs_manifest_typed_provide(tx, trove_id, DependencyClass::Binary.prefix(), binary)?;
    }
    for module in &provides.pkgconfig {
        insert_ccs_manifest_typed_provide(
            tx,
            trove_id,
            DependencyClass::PkgConfig.prefix(),
            module,
        )?;
    }
    Ok(())
}

fn insert_ccs_manifest_typed_provide(
    tx: &rusqlite::Transaction<'_>,
    trove_id: i64,
    kind: &str,
    capability: &str,
) -> Result<()> {
    let mut provide = ProvideEntry::new_typed(trove_id, kind, capability.to_string(), None);
    provide.insert_or_ignore(tx)?;
    tx.execute(
        "UPDATE provides
         SET kind = ?3
         WHERE trove_id = ?1
           AND capability = ?2
           AND kind = 'package'",
        rusqlite::params![trove_id, capability, kind],
    )?;
    Ok(())
}
