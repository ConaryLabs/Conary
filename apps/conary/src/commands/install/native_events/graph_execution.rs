// src/commands/install/native_events/graph_execution.rs

//! Execution adapters for the core native transaction graph.

use super::{DebLifecycleFailure, PreparedNativeTransaction};
use anyhow::{Context, Result};
use conary_core::ccs::native_transaction::{
    DebRecoveryDisposition, NativeEventStage, NativeTransactionStep,
};
use conary_core::scriptlet::ExecutionMode;
use std::path::Path;

impl PreparedNativeTransaction {
    pub(crate) fn graph_steps(&self) -> &[NativeTransactionStep] {
        &self.plan.graph.steps
    }

    pub(crate) fn graph_event_stage(&self, event_index: usize) -> Result<NativeEventStage> {
        self.plan
            .events
            .get(event_index)
            .map(|event| event.stage)
            .with_context(|| {
                format!("native transaction graph refers to missing event {event_index}")
            })
    }

    pub(crate) fn execute_graph_event(
        &self,
        conn: &rusqlite::Connection,
        event_index: usize,
        root: &Path,
        mode: &ExecutionMode,
    ) -> Result<()> {
        let event = self.plan.events.get(event_index).with_context(|| {
            format!("native transaction graph refers to missing event {event_index}")
        })?;
        self.persist_trigger_pending(conn, event)?;
        if let Err(error) = self.execute_event_at_debian_boundary(conn, event, root, mode)? {
            if let Some(recovery) = self.plan.deb.recovery_for_event(event) {
                match self.execute_deb_recovery(conn, recovery, root, mode)? {
                    DebRecoveryDisposition::Continue => {
                        self.persist_event_success(conn, event)?;
                        return Ok(());
                    }
                    DebRecoveryDisposition::Abort {
                        package_state,
                        restore_previous_payload: _,
                    } => {
                        self.persist_event_failure(conn, event, package_state)?;
                        return Err(DebLifecycleFailure {
                            package_name: event.owner_package.clone(),
                            package_state,
                            primary_error: error.to_string(),
                        }
                        .into());
                    }
                }
            }
            if let Some(package_state) = self.plan.deb.terminal_failure_state(event) {
                self.persist_event_failure(conn, event, package_state)?;
            }
            return Err(error).with_context(|| {
                format!(
                    "native transaction event failed for {} {}",
                    event.owner_package, event.owner_version
                )
            });
        } else {
            self.persist_event_success(conn, event)?;
        }
        Ok(())
    }

    pub(crate) fn execute_graph_transaction_finalizer(&self, root: &Path) -> Result<()> {
        self.execute_arch_ldconfig(root)
    }
}
