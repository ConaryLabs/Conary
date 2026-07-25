// src/commands/install/native_events/deb_state/triggers.rs

//! Debian trigger pending/awaited state ownership.

use super::*;
use conary_core::ccs::native_transaction::{DebTriggerActivationBoundary, DebTriggerBatch};
use std::collections::{BTreeMap, BTreeSet};

impl PreparedNativeTransaction {
    pub(crate) fn persist_trigger_activations_for(
        &self,
        conn: &rusqlite::Connection,
        transaction_index: usize,
        boundary: DebTriggerActivationBoundary,
    ) -> Result<()> {
        let unconfigured = self
            .plan
            .deb
            .trigger_unconfigurations
            .iter()
            .filter(|transition| {
                transition.transaction_index == transaction_index && transition.boundary == boundary
            })
            .map(|transition| transition.owner.clone())
            .collect::<Vec<_>>();
        for owner in unconfigured {
            self.clear_unconfigured_trigger_state(conn, &owner)?;
        }

        let mut pending_by_interested = BTreeMap::<NativePackageIdentity, BTreeSet<String>>::new();
        let mut awaited_by_triggering =
            BTreeMap::<NativePackageIdentity, BTreeSet<NativePackageIdentity>>::new();

        for activation in self
            .plan
            .deb
            .trigger_activations
            .iter()
            .filter(|activation| {
                activation.transaction_index == transaction_index && activation.boundary == boundary
            })
        {
            pending_by_interested
                .entry(activation.interested.clone())
                .or_default()
                .insert(activation.trigger_name.clone());
            if activation.awaited {
                awaited_by_triggering
                    .entry(activation.triggering.clone())
                    .or_default()
                    .insert(activation.interested.clone());
            }
        }

        for (identity, activated_triggers) in pending_by_interested {
            let owner = self
                .deb_owner_for_runtime_identity(&identity)?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Debian trigger activation targets missing lifecycle owner '{} {} {:?}'",
                        identity.package_name,
                        identity.package_version,
                        identity.package_arch
                    )
                })?;
            let existing = self.runtime_state(conn, owner)?;
            let mut pending = existing
                .pending_triggers
                .into_iter()
                .collect::<BTreeSet<_>>();
            pending.extend(activated_triggers);
            let pending = pending.into_iter().collect::<Vec<_>>();
            let state = if existing.awaited_packages.is_empty() {
                DebPackageState::TriggersPending
            } else {
                DebPackageState::TriggersAwaited
            };
            self.update_owner_state(
                conn,
                owner,
                state,
                Some(&pending),
                Some(&existing.awaited_packages),
            )?;
        }

        for (identity, interested_packages) in awaited_by_triggering {
            let Some(owner) = self.deb_owner_for_runtime_identity(&identity)? else {
                // A non-Debian payload can activate a Debian file trigger, but
                // it has no dpkg package state on which to persist an await edge.
                continue;
            };
            let existing = self.runtime_state(conn, owner)?;
            let mut awaited = existing
                .awaited_packages
                .into_iter()
                .collect::<BTreeSet<_>>();
            awaited.extend(interested_packages);
            let awaited = awaited.into_iter().collect::<Vec<_>>();
            self.update_owner_state(
                conn,
                owner,
                DebPackageState::TriggersAwaited,
                Some(&existing.pending_triggers),
                Some(&awaited),
            )?;
        }

        Ok(())
    }

    pub(crate) fn persist_trigger_pending(
        &self,
        conn: &rusqlite::Connection,
        event: &NativeTransactionEvent,
    ) -> Result<()> {
        let Some(batch) = self.trigger_batch_for_event(event) else {
            return Ok(());
        };
        let interested = self.owner_for_event(event)?;
        let existing = self.runtime_state(conn, interested)?;
        let mut pending = existing
            .pending_triggers
            .into_iter()
            .collect::<BTreeSet<_>>();
        pending.extend(batch.pending_triggers.iter().cloned());
        let pending = pending.into_iter().collect::<Vec<_>>();
        let state = if existing.awaited_packages.is_empty() {
            DebPackageState::TriggersPending
        } else {
            DebPackageState::TriggersAwaited
        };
        self.update_owner_state(
            conn,
            interested,
            state,
            Some(&pending),
            Some(&existing.awaited_packages),
        )
    }

    pub(super) fn finish_trigger_batch(
        &self,
        conn: &rusqlite::Connection,
        event: &NativeTransactionEvent,
        success: bool,
    ) -> Result<()> {
        let Some(batch) = self.trigger_batch_for_event(event) else {
            return Ok(());
        };
        let interested = self.owner_for_event(event)?;
        let interested_existing = self.runtime_state(conn, interested)?;
        let interested_state = if success {
            if interested_existing.awaited_packages.is_empty() {
                DebPackageState::Installed
            } else {
                DebPackageState::TriggersAwaited
            }
        } else {
            DebPackageState::HalfConfigured
        };
        self.update_owner_state(
            conn,
            interested,
            interested_state,
            Some(&[]),
            Some(&interested_existing.awaited_packages),
        )?;

        let mut seen = BTreeSet::new();
        for triggering in self.owners.iter().filter(|owner| {
            owner.bundle.source_format == SourceFormat::Deb && seen.insert(owner_identity(owner))
        }) {
            let existing = self.runtime_state(conn, triggering)?;
            if !existing.awaited_packages.contains(&batch.interested) {
                continue;
            }
            let mut awaited = existing.awaited_packages;
            awaited.retain(|package| package != &batch.interested);
            let state = settled_trigger_state(
                triggering,
                existing.lifecycle_state,
                &existing.pending_triggers,
                &awaited,
            );
            self.update_owner_state(
                conn,
                triggering,
                state,
                Some(&existing.pending_triggers),
                Some(&awaited),
            )?;
        }
        Ok(())
    }

    pub(super) fn trigger_batch_for_event(
        &self,
        event: &NativeTransactionEvent,
    ) -> Option<&DebTriggerBatch> {
        self.plan.deb.trigger_batches.iter().find(|batch| {
            batch.interested.package_name == event.owner_package
                && batch.interested.package_version == event.owner_version
                && batch.interested.package_arch == event.owner_arch
                && batch.pending_triggers == event.matched_targets
        })
    }

    pub(super) fn clear_unconfigured_trigger_state(
        &self,
        conn: &rusqlite::Connection,
        identity: &NativePackageIdentity,
    ) -> Result<()> {
        let owner = self
            .deb_owner_for_runtime_identity(identity)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Debian trigger unconfiguration targets missing lifecycle owner '{} {} {:?}'",
                    identity.package_name,
                    identity.package_version,
                    identity.package_arch
                )
            })?;
        self.update_owner_state(conn, owner, DebPackageState::Unpacked, Some(&[]), Some(&[]))?;

        let mut seen = BTreeSet::new();
        for awaiter in self.owners.iter().filter(|candidate| {
            candidate.bundle.source_format == SourceFormat::Deb
                && seen.insert(owner_identity(candidate))
        }) {
            if owner_identity(awaiter) == *identity {
                continue;
            }
            let existing = self.runtime_state(conn, awaiter)?;
            if !existing.awaited_packages.contains(identity) {
                continue;
            }
            let mut awaited = existing.awaited_packages;
            awaited.retain(|package| package != identity);
            let state = settled_trigger_state(
                awaiter,
                existing.lifecycle_state,
                &existing.pending_triggers,
                &awaited,
            );
            self.update_owner_state(
                conn,
                awaiter,
                state,
                Some(&existing.pending_triggers),
                Some(&awaited),
            )?;
        }
        Ok(())
    }
}
