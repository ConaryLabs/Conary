// conary/src/commands/install/ccs_removal_hooks.rs

//! Exact CCS pre-remove authority for install-driven package removals.

use crate::commands::remove::{
    execute_preflighted_ccs_remove_hook, load_ccs_remove_hook, preflight_loaded_ccs_remove_hook,
};
use anyhow::{Context, Result};
use conary_core::db::models::{InstalledCcsRemoveHook, Trove};
use conary_core::scriptlet::SandboxMode;
use conary_core::transaction::PackageRelationRemoval;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct PlannedCcsRemovalHook {
    owner: Trove,
    hook: InstalledCcsRemoveHook,
}

/// Exact installed CCS hooks owned by packages this install will remove.
///
/// The plan is built from persisted typed rows only. It deliberately has no
/// script text discovery, package-format projection, or native fallback.
#[derive(Debug, Clone, Default)]
pub(super) struct CcsRemovalHookPlan {
    hooks: Vec<PlannedCcsRemovalHook>,
}

/// Runtime-validated CCS removal hooks bound to the root and sandbox contract
/// used during preflight.
pub(super) struct PreflightedCcsRemovalHooks {
    hooks: Vec<PlannedCcsRemovalHook>,
    root: PathBuf,
    sandbox_mode: SandboxMode,
}

impl CcsRemovalHookPlan {
    pub(super) fn prepare<'a>(
        conn: &Connection,
        upgraded_troves: impl IntoIterator<Item = &'a Trove>,
        relation_removals: impl IntoIterator<Item = &'a PackageRelationRemoval>,
    ) -> Result<Self> {
        let mut owners = BTreeMap::new();
        for trove in upgraded_troves {
            let trove_id = trove
                .id
                .context("installed upgrade target has no database identity")?;
            owners.insert(trove_id, trove.clone());
        }
        for removal in relation_removals {
            let owner = Trove::find_by_id(conn, removal.trove_id)?.with_context(|| {
                format!(
                    "CCS removal-hook owner {} {} disappeared",
                    removal.package_name, removal.package_version
                )
            })?;
            owners.insert(removal.trove_id, owner);
        }

        let mut hooks = Vec::new();
        for owner in owners.into_values() {
            if let Some(hook) = load_ccs_remove_hook(conn, &owner)? {
                hooks.push(PlannedCcsRemovalHook { owner, hook });
            }
        }
        Ok(Self { hooks })
    }

    pub(super) fn preflight(
        &self,
        root: &Path,
        sandbox_mode: SandboxMode,
    ) -> Result<PreflightedCcsRemovalHooks> {
        for planned in &self.hooks {
            preflight_loaded_ccs_remove_hook(&planned.owner, &planned.hook, root, sandbox_mode)
                .with_context(|| {
                    format!(
                        "CCS pre-remove hook preflight failed for '{}'",
                        planned.owner.name
                    )
                })?;
        }
        Ok(PreflightedCcsRemovalHooks {
            hooks: self.hooks.clone(),
            root: root.to_path_buf(),
            sandbox_mode,
        })
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.hooks.len()
    }
}

impl PreflightedCcsRemovalHooks {
    /// Execute every hook only after the complete set passed preflight.
    pub(super) fn execute(&self) -> Result<()> {
        for planned in &self.hooks {
            execute_preflighted_ccs_remove_hook(
                &planned.owner,
                &planned.hook,
                &self.root,
                self.sandbox_mode,
            )
            .with_context(|| {
                format!(
                    "CCS pre-remove hook failed for '{}'; package authority remains installed",
                    planned.owner.name
                )
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{InstallSource, InstalledCcsRemoveHook, Trove, TroveType};
    use conary_core::repository::dependency_model::{
        PackageRelationRemovalMode, RepositoryRequirementKind,
    };
    use conary_core::transaction::PackageRelationIncomingIdentity;

    fn installed_ccs(conn: &Connection, name: &str, script: &str) -> (Trove, i64) {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::File,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(conn).unwrap();
        InstalledCcsRemoveHook::new(trove_id, script.to_string(), Some(false))
            .insert_or_replace(conn)
            .unwrap();
        (trove, trove_id)
    }

    fn relation_removal(trove_id: i64, package_name: &str) -> PackageRelationRemoval {
        PackageRelationRemoval {
            trove_id,
            package_name: package_name.to_string(),
            package_version: "1".to_string(),
            package_architecture: None,
            triggering_incoming: PackageRelationIncomingIdentity {
                transaction_index: 0,
                package_name: "replacement".to_string(),
                package_version: "2".to_string(),
                package_architecture: None,
            },
            incoming_packages: vec!["replacement".to_string()],
            ownership_transfer_packages: vec!["replacement".to_string()],
            kind: RepositoryRequirementKind::Replace,
            mode: PackageRelationRemovalMode::OwnershipTransfer,
            native_text: Some(package_name.to_string()),
        }
    }

    #[test]
    fn plan_deduplicates_upgrade_and_relation_removal_authority() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        let (owner, trove_id) = installed_ccs(&conn, "old-ccs", "exit 0");
        let relation = relation_removal(trove_id, "old-ccs");

        let plan =
            CcsRemovalHookPlan::prepare(&conn, std::iter::once(&owner), std::iter::once(&relation))
                .unwrap();

        assert_eq!(plan.len(), 1);
    }

    #[test]
    fn target_root_preflight_failure_keeps_installed_authority_intact() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        let (owner, trove_id) = installed_ccs(&conn, "old-ccs", "exit 42");
        let plan = CcsRemovalHookPlan::prepare(&conn, std::iter::once(&owner), std::iter::empty())
            .unwrap();
        let target_root = tempfile::tempdir().unwrap();

        let error = match plan.preflight(target_root.path(), SandboxMode::Always) {
            Ok(_) => panic!("target root without /bin/sh must fail CCS hook preflight"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("CCS pre-remove hook preflight failed")
        );
        assert!(
            format!("{error:#}").contains("Interpreter /bin/sh not found in target root"),
            "{error:#}"
        );
        assert!(Trove::find_by_id(&conn, trove_id).unwrap().is_some());
        assert!(
            InstalledCcsRemoveHook::find_by_trove(&conn, trove_id)
                .unwrap()
                .is_some()
        );
    }
}
