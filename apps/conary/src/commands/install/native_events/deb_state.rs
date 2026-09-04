// apps/conary/src/commands/install/native_events/deb_state.rs

//! Persisted Debian package and trigger state transitions.

use super::{NativeBundleOwner, PreparedNativeTransaction, owner_identity};
use anyhow::Result;
use conary_core::ccs::native_lifecycle::SourceFormat;
use conary_core::ccs::native_transaction::{
    DebPackageState, NativeBundleRole, NativeDebPackageState, NativeEventStage,
    NativePackageIdentity, NativeTransactionEvent, NativeTransactionOperation,
};
use conary_core::db::models::{InstalledNativeLifecycleBundle, NativeLifecycleResidualState};

#[path = "deb_state/triggers.rs"]
mod triggers;

#[derive(Debug)]
pub(super) struct DebRuntimeState {
    pub(super) lifecycle_state: DebPackageState,
    pub(super) pending_triggers: Vec<String>,
    pub(super) awaited_packages: Vec<NativePackageIdentity>,
}

pub(super) fn transaction_package_states(
    conn: &rusqlite::Connection,
    owners: &[NativeBundleOwner],
) -> Result<Vec<NativeDebPackageState>> {
    let mut states = owners
        .iter()
        .filter(|owner| {
            owner.bundle.source_format == SourceFormat::Deb
                && owner.initial_package_state != DebPackageState::NotInstalled
        })
        .map(|owner| NativeDebPackageState {
            package_name: owner.package_name.clone(),
            package_version: owner.package_version.clone(),
            source_arch: owner.bundle.source_arch.clone(),
            lifecycle_state: owner.initial_package_state,
            pending_triggers: owner.initial_pending_triggers.clone(),
            awaited_packages: owner.initial_awaited_packages.clone(),
        })
        .collect::<Vec<_>>();
    for residual in NativeLifecycleResidualState::find_all(conn)? {
        if residual.source_format != SourceFormat::Deb.as_str() {
            continue;
        }
        residual.bundle()?;
        let identity = residual.identity();
        if states.iter().any(|state| {
            state.package_name == identity.package_name
                && state.package_version == identity.package_version
                && state.source_arch == identity.package_arch
        }) {
            anyhow::bail!(
                "Debian package state is duplicated across installed and residual storage for '{} {} {:?}'",
                identity.package_name,
                identity.package_version,
                identity.package_arch
            );
        }
        states.push(NativeDebPackageState {
            package_name: residual.source_package,
            package_version: residual.source_version,
            source_arch: residual.source_arch,
            lifecycle_state: residual.lifecycle_state,
            pending_triggers: residual.pending_triggers,
            awaited_packages: residual.awaited_packages,
        });
    }
    Ok(states)
}

impl PreparedNativeTransaction {
    pub(crate) fn finalize_successful_debian_installs(
        &self,
        conn: &rusqlite::Connection,
    ) -> Result<()> {
        for owner in self.owners.iter().filter(|owner| {
            owner.bundle.source_format == SourceFormat::Deb
                && owner.role == NativeBundleRole::Installing
        }) {
            let runtime = self.runtime_state(conn, owner)?;
            let state = match runtime.lifecycle_state {
                DebPackageState::Unpacked if !runtime.awaited_packages.is_empty() => {
                    DebPackageState::TriggersAwaited
                }
                DebPackageState::Unpacked if !runtime.pending_triggers.is_empty() => {
                    DebPackageState::TriggersPending
                }
                DebPackageState::Unpacked => DebPackageState::Installed,
                _ => continue,
            };
            self.update_owner_state(
                conn,
                owner,
                state,
                Some(&runtime.pending_triggers),
                Some(&runtime.awaited_packages),
            )?;
        }
        Ok(())
    }

    pub(super) fn persist_event_success(
        &self,
        conn: &rusqlite::Connection,
        event: &NativeTransactionEvent,
    ) -> Result<()> {
        if self.trigger_batch_for_event(event).is_some() {
            return self.finish_trigger_batch(conn, event, true);
        }
        let owner = self.owner_for_event(event)?;
        if owner.bundle.source_format != SourceFormat::Deb {
            return Ok(());
        }
        if event.stage == NativeEventStage::DebPreDeconfigure {
            return self.clear_unconfigured_trigger_state(conn, &owner_identity(owner));
        }
        let state = match event.stage {
            NativeEventStage::DebPreRemoveUpgrade => Some(DebPackageState::Unpacked),
            NativeEventStage::DebPreRemoveInFavour => Some(DebPackageState::HalfInstalled),
            NativeEventStage::DebPostRemoveUpgrade => Some(DebPackageState::HalfInstalled),
            NativeEventStage::PackagePreRemove => Some(DebPackageState::Unpacked),
            NativeEventStage::DebPostInstall => Some(DebPackageState::Installed),
            NativeEventStage::PackagePostRemove => Some(DebPackageState::ConfigFiles),
            NativeEventStage::DebPostRemovePurge | NativeEventStage::DebPostRemoveDisappear => {
                Some(DebPackageState::NotInstalled)
            }
            _ => None,
        };
        if let Some(state) = state {
            self.update_owner_state(conn, owner, state, None, None)?;
        }
        Ok(())
    }

    pub(super) fn persist_event_failure(
        &self,
        conn: &rusqlite::Connection,
        event: &NativeTransactionEvent,
        state: DebPackageState,
    ) -> Result<()> {
        if self.trigger_batch_for_event(event).is_some() {
            return self.finish_trigger_batch(conn, event, false);
        }
        let primary_owner = self.owner_for_event(event)?;
        let state_owner = match event.stage {
            NativeEventStage::DebPreRemoveUpgrade
            | NativeEventStage::PackagePreInstall
            | NativeEventStage::DebPostRemoveUpgrade => self
                .owners
                .iter()
                .find(|owner| {
                    owner.package_name == event.owner_package
                        && owner.role == NativeBundleRole::Removing
                        && owner.bundle.source_format == SourceFormat::Deb
                        && owner.bundle.source_arch == event.owner_arch
                })
                .unwrap_or(primary_owner),
            _ => primary_owner,
        };
        self.update_owner_state(conn, state_owner, state, None, None)?;
        Ok(())
    }

    fn runtime_row(
        &self,
        conn: &rusqlite::Connection,
        owner: &NativeBundleOwner,
    ) -> Result<Option<InstalledNativeLifecycleBundle>> {
        let mut matches = InstalledNativeLifecycleBundle::find_all(conn)?
            .into_iter()
            .filter(|row| {
                row.source_package == owner.package_name
                    && row.source_version == owner.package_version
                    && row.source_arch == owner.bundle.source_arch
            });
        let first = matches.next();
        if matches.next().is_some() {
            anyhow::bail!(
                "native lifecycle runtime identity is ambiguous for {} {} {:?}",
                owner.package_name,
                owner.package_version,
                owner.bundle.source_arch
            );
        }
        Ok(first)
    }

    pub(super) fn runtime_state(
        &self,
        conn: &rusqlite::Connection,
        owner: &NativeBundleOwner,
    ) -> Result<DebRuntimeState> {
        let identity = owner_identity(owner);
        let installed = self.runtime_row(conn, owner)?;
        let residual =
            NativeLifecycleResidualState::find_exact(conn, SourceFormat::Deb.as_str(), &identity)?;
        if installed.is_some() && residual.is_some() {
            anyhow::bail!(
                "Debian lifecycle state is duplicated across installed and residual storage for '{} {} {:?}'",
                identity.package_name,
                identity.package_version,
                identity.package_arch
            );
        }
        if let Some(installed) = installed {
            return Ok(DebRuntimeState {
                lifecycle_state: installed.lifecycle_state,
                pending_triggers: installed.pending_triggers,
                awaited_packages: installed.awaited_packages,
            });
        }
        if let Some(residual) = residual {
            return Ok(DebRuntimeState {
                lifecycle_state: residual.lifecycle_state,
                pending_triggers: residual.pending_triggers,
                awaited_packages: residual.awaited_packages,
            });
        }
        Ok(DebRuntimeState {
            lifecycle_state: owner.initial_package_state,
            pending_triggers: owner.initial_pending_triggers.clone(),
            awaited_packages: owner.initial_awaited_packages.clone(),
        })
    }

    pub(super) fn update_owner_state(
        &self,
        conn: &rusqlite::Connection,
        owner: &NativeBundleOwner,
        state: DebPackageState,
        pending_triggers: Option<&[String]>,
        awaited_packages: Option<&[NativePackageIdentity]>,
    ) -> Result<()> {
        let existing = self.runtime_row(conn, owner)?;
        let residual = NativeLifecycleResidualState::find_exact(
            conn,
            SourceFormat::Deb.as_str(),
            &owner_identity(owner),
        )?;
        if existing.is_some() && residual.is_some() {
            anyhow::bail!(
                "Debian lifecycle state is duplicated across installed and residual storage for '{} {} {:?}'",
                owner.package_name,
                owner.package_version,
                owner.bundle.source_arch
            );
        }
        let pending_triggers = pending_triggers
            .or_else(|| existing.as_ref().map(|row| row.pending_triggers.as_slice()))
            .or_else(|| residual.as_ref().map(|row| row.pending_triggers.as_slice()))
            .unwrap_or(owner.initial_pending_triggers.as_slice());
        let awaited_packages = awaited_packages
            .or_else(|| existing.as_ref().map(|row| row.awaited_packages.as_slice()))
            .or_else(|| residual.as_ref().map(|row| row.awaited_packages.as_slice()))
            .unwrap_or(owner.initial_awaited_packages.as_slice());
        if let Some(existing) = existing.as_ref() {
            let updated = InstalledNativeLifecycleBundle::update_runtime_state_by_trove(
                conn,
                existing.trove_id,
                state,
                pending_triggers,
                awaited_packages,
            )?;
            if updated != 1 {
                anyhow::bail!(
                    "native lifecycle runtime update affected {updated} rows for trove {}",
                    existing.trove_id
                );
            }
            delete_residual_state(conn, owner)?;
        } else if state == DebPackageState::NotInstalled {
            delete_residual_state(conn, owner)?;
        } else {
            persist_residual_state(conn, owner, state, pending_triggers, awaited_packages)?;
        }
        Ok(())
    }

    pub(crate) fn mark_install_payload_applied_for(
        &self,
        conn: &rusqlite::Connection,
        package: &NativePackageIdentity,
    ) -> Result<()> {
        let Some(owner) =
            self.deb_owner_for_identity_and_role(package, NativeBundleRole::Installing)?
        else {
            return Ok(());
        };
        self.update_owner_state(conn, owner, DebPackageState::Unpacked, None, None)
    }

    pub(crate) fn mark_remove_payload_started_for(
        &self,
        conn: &rusqlite::Connection,
        package: &NativePackageIdentity,
    ) -> Result<()> {
        let Some(owner) =
            self.deb_owner_for_identity_and_role(package, NativeBundleRole::Removing)?
        else {
            return Ok(());
        };
        self.update_owner_state(conn, owner, DebPackageState::HalfInstalled, None, None)
    }

    pub(crate) fn mark_remove_payload_completed_for(
        &self,
        conn: &rusqlite::Connection,
        package: &NativePackageIdentity,
        change_index: usize,
    ) -> Result<()> {
        let Some(owner) =
            self.deb_owner_for_identity_and_role(package, NativeBundleRole::Removing)?
        else {
            return Ok(());
        };
        let state = match self.operations.get(change_index).copied().ok_or_else(|| {
            anyhow::anyhow!("native transaction has no operation for removal change {change_index}")
        })? {
            NativeTransactionOperation::Upgrade | NativeTransactionOperation::Disappear { .. } => {
                DebPackageState::NotInstalled
            }
            NativeTransactionOperation::Remove
            | NativeTransactionOperation::Purge
            | NativeTransactionOperation::RemoveInFavour { .. } => DebPackageState::ConfigFiles,
            NativeTransactionOperation::Install => {
                anyhow::bail!(
                    "native transaction marks install change {change_index} as removal-complete"
                )
            }
            NativeTransactionOperation::DeconfigureInFavour { .. } => {
                anyhow::bail!(
                    "native transaction marks deconfiguration change {change_index} as removal-complete"
                )
            }
        };
        self.update_owner_state(conn, owner, state, None, None)
    }

    pub(super) fn persist_deb_recovery_state(
        &self,
        conn: &rusqlite::Connection,
        package: &NativePackageIdentity,
        role: NativeBundleRole,
        state: DebPackageState,
    ) -> Result<()> {
        let owner = self
            .deb_owner_for_identity_and_role(package, role)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Debian recovery state targets missing {role:?} owner '{} {} {:?}'",
                    package.package_name,
                    package.package_version,
                    package.package_arch
                )
            })?;
        self.update_owner_state(conn, owner, state, None, None)
    }

    fn deb_owner_for_identity_and_role(
        &self,
        package: &NativePackageIdentity,
        role: NativeBundleRole,
    ) -> Result<Option<&NativeBundleOwner>> {
        let mut matches = self
            .owners
            .iter()
            .filter(|owner| owner.role == role && owner_identity(owner) == *package);
        let owner = matches.next();
        if matches.next().is_some() {
            anyhow::bail!(
                "native transaction has ambiguous {role:?} lifecycle owners for '{} {} {:?}'",
                package.package_name,
                package.package_version,
                package.package_arch
            );
        }
        Ok(owner.filter(|owner| owner.bundle.source_format == SourceFormat::Deb))
    }

    pub(super) fn deb_owner_for_runtime_identity(
        &self,
        package: &NativePackageIdentity,
    ) -> Result<Option<&NativeBundleOwner>> {
        let candidates = self
            .owners
            .iter()
            .filter(|owner| {
                owner.bundle.source_format == SourceFormat::Deb && owner_identity(owner) == *package
            })
            .collect::<Vec<_>>();
        if candidates.len() <= 1 {
            return Ok(candidates.first().copied());
        }
        let installing = candidates
            .iter()
            .copied()
            .filter(|owner| owner.role == NativeBundleRole::Installing)
            .collect::<Vec<_>>();
        let removing = candidates
            .iter()
            .copied()
            .filter(|owner| owner.role == NativeBundleRole::Removing)
            .collect::<Vec<_>>();
        if candidates.len() == 2 && installing.len() == 1 && removing.len() == 1 {
            return Ok(installing.first().copied());
        }
        anyhow::bail!(
            "native transaction has ambiguous Debian lifecycle owners for '{} {} {:?}'",
            package.package_name,
            package.package_version,
            package.package_arch
        )
    }
}

fn persist_residual_state(
    conn: &rusqlite::Connection,
    owner: &NativeBundleOwner,
    state: DebPackageState,
    pending_triggers: &[String],
    awaited_packages: &[NativePackageIdentity],
) -> Result<()> {
    NativeLifecycleResidualState::new(
        &owner.bundle,
        state,
        pending_triggers.to_vec(),
        awaited_packages.to_vec(),
    )?
    .upsert(conn)
}

fn delete_residual_state(conn: &rusqlite::Connection, owner: &NativeBundleOwner) -> Result<()> {
    NativeLifecycleResidualState::delete_exact(
        conn,
        owner.bundle.source_format.as_str(),
        &owner_identity(owner),
    )?;
    Ok(())
}

fn settled_trigger_state(
    owner: &NativeBundleOwner,
    current: DebPackageState,
    pending_triggers: &[String],
    awaited_packages: &[NativePackageIdentity],
) -> DebPackageState {
    if !awaited_packages.is_empty() {
        return DebPackageState::TriggersAwaited;
    }
    if !pending_triggers.is_empty() {
        return DebPackageState::TriggersPending;
    }
    match current {
        DebPackageState::NotInstalled
        | DebPackageState::ConfigFiles
        | DebPackageState::HalfInstalled
        | DebPackageState::Unpacked
        | DebPackageState::HalfConfigured => current,
        DebPackageState::TriggersAwaited
        | DebPackageState::TriggersPending
        | DebPackageState::Installed => match owner.role {
            NativeBundleRole::Removing => DebPackageState::ConfigFiles,
            NativeBundleRole::Installing
            | NativeBundleRole::Installed
            | NativeBundleRole::Deconfiguring => DebPackageState::Installed,
        },
    }
}

#[cfg(test)]
#[path = "deb_state/tests.rs"]
mod tests;
