// apps/conary/src/commands/install/native_events/execution.rs

//! Runtime dispatch and recovery for typed native transaction graph events.

use super::{NativeBundleOwner, PreparedNativeTransaction, executor_for_owner, runtime};
use anyhow::{Context, Result};
use conary_core::ccs::native_lifecycle::SourceFormat;
use conary_core::ccs::native_transaction::{
    DebRecoveryDisposition, DebRecoveryResult, NativeEventProgram, NativeTransactionEvent,
};
use conary_core::db::models::DebianDebconfState;
use conary_core::scriptlet::{ExecutionMode, SandboxMode, ScriptletExecutor};
use std::path::Path;
use tracing::warn;

impl PreparedNativeTransaction {
    pub(super) fn execute_event_at_debian_boundary(
        &self,
        conn: &rusqlite::Connection,
        event: &NativeTransactionEvent,
        root: &Path,
        mode: &ExecutionMode,
    ) -> Result<std::result::Result<(), anyhow::Error>> {
        let snapshot = self.materialize_debian_admin(conn, Some(event), root)?;
        let bridge = self.prepare_debian_lifecycle_bridge(conn, event, root)?;
        let execution = self.execute_event(event, root, mode, bridge.as_ref());
        let debconf_settlement = settle_debian_lifecycle_event(conn, bridge.as_ref(), execution);
        let mut admin_capture = Ok(());
        if let Some(snapshot) = snapshot.as_ref() {
            admin_capture = self.capture_debian_admin_mutations(conn, snapshot, root, event);
        }
        let execution = debconf_settlement?;
        admin_capture?;
        Ok(execution)
    }

    pub(super) fn execute_event(
        &self,
        event: &NativeTransactionEvent,
        root: &Path,
        mode: &ExecutionMode,
        bridge: Option<&super::debian_runtime::DebianLifecycleBridge>,
    ) -> Result<()> {
        let owner = self.owner_for_event(event)?;
        let mut executor = executor_for_owner(owner, root)?;
        if let Some(bridge) = bridge {
            executor = executor.with_lifecycle_bridge(bridge.config());
        }

        let result = match &event.program {
            NativeEventProgram::BundleEntry { entry_id } => runtime::execute_entry(
                owner,
                &executor,
                runtime::EntryInvocation::new(
                    entry_id,
                    mode,
                    &event.args,
                    &event.stdin,
                    event.deb_package_refcount,
                ),
            )
            .map(|execution| {
                if let runtime::EntryExecution::ContinuedAfterFailure(failure) = execution {
                    self.continued_lifecycle_failures.borrow_mut().push(
                        super::ContinuedLifecycleFailure {
                            package: event.owner_package.clone(),
                            version: event.owner_version.clone(),
                            entry: entry_id.clone(),
                            failure: *failure,
                        },
                    );
                }
            }),
            NativeEventProgram::Command { argv } => executor
                .execute_native_command("native-transaction-hook", argv, &event.stdin)
                .map_err(anyhow::Error::from),
            NativeEventProgram::RpmSysusers { source_path } => executor
                .execute_rpm_sysusers(
                    self.rpm_sysusers_interface()?,
                    source_path.as_deref(),
                    &event.stdin,
                )
                .map_err(anyhow::Error::from),
        };
        let captured = executor.take_activation_invocations();
        if result.is_ok() {
            let source_entry = match &event.program {
                NativeEventProgram::BundleEntry { entry_id } => entry_id.clone(),
                NativeEventProgram::Command { .. } => event.order_key.clone(),
                NativeEventProgram::RpmSysusers { .. } => event.order_key.clone(),
            };
            self.activation_invocations
                .borrow_mut()
                .extend(
                    captured
                        .into_iter()
                        .map(|invocation| super::CapturedNativeActivation {
                            source_package: event.owner_package.clone(),
                            source_version: event.owner_version.clone(),
                            source_entry: source_entry.clone(),
                            invocation,
                        }),
                );
        }
        result
    }

    fn prepare_debian_lifecycle_bridge(
        &self,
        conn: &rusqlite::Connection,
        event: &NativeTransactionEvent,
        root: &Path,
    ) -> Result<Option<super::debian_runtime::DebianLifecycleBridge>> {
        let owner = self.owner_for_event(event)?;
        if owner.bundle.source_format != SourceFormat::Deb {
            return Ok(None);
        }
        let state = DebianDebconfState::load(conn)
            .context("cannot load normalized Debian debconf state before lifecycle event")?;
        let templates = owner.bundle.debconf_templates().with_context(|| {
            format!(
                "cannot resolve debconf templates authority for {} {}",
                owner.package_name, owner.package_version
            )
        })?;
        if let Some(authority) = &templates
            && (authority.source_package != owner.package_name
                || authority.source_version != owner.package_version)
        {
            anyhow::bail!(
                "debconf templates authority {} {} does not match lifecycle event owner {} {}",
                authority.source_package,
                authority.source_version,
                owner.package_name,
                owner.package_version
            );
        }
        let records = templates
            .as_ref()
            .map_or([].as_slice(), |authority| authority.records.as_slice());
        super::debian_runtime::DebianLifecycleBridge::new(root, &owner.package_name, state, records)
            .map(Some)
    }

    pub(super) fn execute_deb_recovery(
        &self,
        conn: &rusqlite::Connection,
        recovery: &DebRecoveryResult,
        root: &Path,
        mode: &ExecutionMode,
    ) -> Result<DebRecoveryDisposition> {
        let mut current = recovery;
        loop {
            match current {
                DebRecoveryResult::Disposition(disposition) => return Ok(*disposition),
                DebRecoveryResult::PersistState(transition) => {
                    self.persist_deb_recovery_state(
                        conn,
                        &transition.package,
                        transition.role,
                        transition.package_state,
                    )?;
                    current = &transition.next;
                }
                DebRecoveryResult::Run(node) => {
                    current = if self
                        .execute_event_at_debian_boundary(conn, &node.event, root, mode)?
                        .is_ok()
                    {
                        &node.on_success
                    } else {
                        &node.on_failure
                    };
                }
            }
        }
    }

    pub(super) fn owner_for_event(
        &self,
        event: &NativeTransactionEvent,
    ) -> Result<&NativeBundleOwner> {
        self.owners
            .iter()
            .find(|owner| {
                owner.package_name == event.owner_package
                    && owner.package_version == event.owner_version
                    && owner.bundle.source_arch == event.owner_arch
            })
            .with_context(|| {
                format!(
                    "native event owner {} {} {:?} disappeared from the transaction plan",
                    event.owner_package, event.owner_version, event.owner_arch
                )
            })
    }

    pub(super) fn execute_arch_ldconfig(&self, root: &Path) -> Result<()> {
        let Some(argv) = self.arch_ldconfig_argv(root)? else {
            // libalpm treats a missing /etc/ld.so.conf or non-executable
            // target-root ldconfig as an exact successful no-op.
            return Ok(());
        };
        let executor = ScriptletExecutor::new(
            root,
            "libalpm-transaction",
            "1",
            conary_core::scriptlet::PackageFormat::Arch,
        )
        .with_sandbox_mode(SandboxMode::Always);
        if let Err(error) = executor.execute_native_command("arch-implicit-ldconfig", &argv, &[]) {
            // libalpm ignores the ldconfig return value after invoking it.
            warn!(error = %error, "Arch implicit ldconfig invocation failed");
        }
        Ok(())
    }

    /// Drain the lifecycle failures this transaction ran past.
    ///
    /// Warn-and-continue is the source format's own posture, not an absence of
    /// a failure. The transaction keeps each one as typed evidence so the run
    /// is never reported as an unqualified success.
    pub(crate) fn take_continued_lifecycle_failures(
        &self,
    ) -> Vec<super::ContinuedLifecycleFailure> {
        std::mem::take(&mut *self.continued_lifecycle_failures.borrow_mut())
    }

    pub(crate) fn take_activation_requests(
        &self,
    ) -> Vec<conary_core::db::models::NewActivationRequest> {
        std::mem::take(&mut *self.activation_invocations.borrow_mut())
            .into_iter()
            .map(|captured| conary_core::db::models::NewActivationRequest {
                source_kind: conary_core::db::models::ActivationRequestSourceKind::captured_for(
                    &captured.invocation,
                ),
                source_package: captured.source_package,
                source_version: captured.source_version,
                source_entry: captured.source_entry,
                invocation: captured.invocation,
            })
            .collect()
    }

    pub(super) fn arch_ldconfig_argv(&self, root: &Path) -> Result<Option<Vec<String>>> {
        if !self.arch_transaction {
            return Ok(None);
        }
        let capabilities = self
            .host_capabilities
            .as_ref()
            .context("Arch transaction lost its preflighted typed host capability inventory")?;
        if !self.arch_ldconfig_required_after {
            return Ok(None);
        }
        let Some(executable) = capabilities.arch_ldconfig_executable_path(root) else {
            return Ok(None);
        };
        Ok(Some(vec![
            executable
                .to_str()
                .with_context(|| {
                    format!(
                        "persisted target-root ldconfig path is not UTF-8: {}",
                        executable.display()
                    )
                })?
                .to_string(),
        ]))
    }
}

pub(super) fn settle_debian_lifecycle_event(
    conn: &rusqlite::Connection,
    bridge: Option<&super::debian_runtime::DebianLifecycleBridge>,
    execution: Result<()>,
) -> Result<Result<()>> {
    if let Some(bridge) = bridge {
        let state = bridge.state()?;
        DebianDebconfState::replace(conn, &state)
            .context("cannot persist Debian debconf state after lifecycle event")?;
    }
    Ok(execution)
}
