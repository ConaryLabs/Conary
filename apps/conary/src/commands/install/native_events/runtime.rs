// apps/conary/src/commands/install/native_events/runtime.rs

//! Shared native lifecycle entry execution and preflight.

use super::{NativeBundleOwner, debian_runtime};
use anyhow::{Context, Result};
use conary_core::ccs::native_lifecycle::NativeLifecycleEntry;
use conary_core::scriptlet::{
    ExecutionMode, FailurePosture, LifecycleFailureClass, NativeInterpreterAvailability,
    NativeInvocationRuntime, NativeLifecycleExecution, ScriptletExecutor, ScriptletFailureOutcome,
    ScriptletOutcome,
};
use tracing::warn;

/// What executing one native lifecycle entry did to the transaction.
///
/// A continued failure is evidence, not a success: the source format's own
/// contract says the transaction proceeds past it, and the caller retains the
/// typed outcome so the transaction can report what it ran past.
pub(super) enum EntryExecution {
    Completed,
    ContinuedAfterFailure(Box<ScriptletFailureOutcome>),
}

struct EntryRuntimeContext {
    environment: Vec<String>,
    package_instance_count: Option<u32>,
}

pub(super) struct EntryInvocation<'a> {
    entry_id: &'a str,
    mode: &'a ExecutionMode,
    args: &'a [String],
    stdin: &'a [u8],
    deb_package_refcount: Option<u32>,
}

impl<'a> EntryInvocation<'a> {
    pub(super) const fn new(
        entry_id: &'a str,
        mode: &'a ExecutionMode,
        args: &'a [String],
        stdin: &'a [u8],
        deb_package_refcount: Option<u32>,
    ) -> Self {
        Self {
            entry_id,
            mode,
            args,
            stdin,
            deb_package_refcount,
        }
    }
}

pub(super) fn execute_entry(
    owner: &NativeBundleOwner,
    executor: &ScriptletExecutor,
    invocation: EntryInvocation<'_>,
) -> Result<EntryExecution> {
    let entry = bundle_entry_for_event(owner, invocation.entry_id)?;
    let context = entry_runtime_context(owner, entry, invocation.deb_package_refcount)?;
    let execution = native_lifecycle_execution(entry, &context.environment);
    let runtime = native_invocation_runtime(
        invocation.mode,
        context.package_instance_count,
        invocation.args,
        invocation.stdin,
    );
    let outcome = if let Some(rpm) = &entry.rpm_runtime {
        executor.execute_rpm_native_lifecycle_entry_with_outcome(&execution, &runtime, rpm)
    } else {
        executor.execute_native_lifecycle_entry_with_outcome(&execution, &runtime)
    };
    let ScriptletOutcome::Failure(failure) = outcome else {
        return outcome
            .into_result()
            .map(|()| EntryExecution::Completed)
            .map_err(anyhow::Error::from);
    };
    // The source format's own per-scriptlet-class table decides this, never the
    // entry's name. See docs/specs/foreign-package-lifecycle-contracts.md,
    // "RPM Scriptlet Failure Posture".
    let posture = FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
        source_format: owner.bundle.source_format,
        rpm_criticality: entry.rpm_runtime.as_ref().map(|rpm| rpm.criticality),
        failure_kind: failure.failure_kind,
    });
    match posture {
        FailurePosture::AbortsTransaction => Err(anyhow::Error::from(
            ScriptletOutcome::Failure(failure)
                .into_result()
                .expect_err("a failure outcome must convert into a typed scriptlet error"),
        )),
        FailurePosture::WarnAndContinue => {
            warn!(
                package = %owner.package_name,
                version = %owner.package_version,
                entry = %entry.id,
                phase = %failure.phase,
                failure_kind = failure.failure_kind.as_str(),
                requested_sandbox = failure.requested_sandbox_mode.as_str(),
                effective_sandbox = failure.effective_sandbox.as_str(),
                error = %failure.message,
                "native lifecycle event failed in a warn-and-continue class; continuing transaction"
            );
            Ok(EntryExecution::ContinuedAfterFailure(Box::new(failure)))
        }
    }
}

pub(super) fn preflight_entry(
    owner: &NativeBundleOwner,
    executor: &ScriptletExecutor,
    invocation: EntryInvocation<'_>,
    interpreter_availability: NativeInterpreterAvailability,
) -> Result<()> {
    let entry = bundle_entry_for_event(owner, invocation.entry_id)?;
    let context = entry_runtime_context(owner, entry, invocation.deb_package_refcount)?;
    let execution = native_lifecycle_execution(entry, &context.environment);
    let runtime = native_invocation_runtime(
        invocation.mode,
        context.package_instance_count,
        invocation.args,
        invocation.stdin,
    );
    if let Some(rpm) = &entry.rpm_runtime {
        executor.preflight_rpm_native_lifecycle_entry(
            &execution,
            &runtime,
            rpm,
            interpreter_availability,
        )
    } else {
        executor.preflight_native_lifecycle_entry(&execution, &runtime, interpreter_availability)
    }
}

pub(super) fn bundle_entry_for_event<'a>(
    owner: &'a NativeBundleOwner,
    entry_id: &str,
) -> Result<&'a NativeLifecycleEntry> {
    owner
        .bundle
        .entries
        .iter()
        .find(|entry| entry.id == entry_id)
        .with_context(|| format!("native transaction event references missing entry {entry_id}"))
}

fn entry_runtime_context(
    owner: &NativeBundleOwner,
    entry: &NativeLifecycleEntry,
    deb_package_refcount: Option<u32>,
) -> Result<EntryRuntimeContext> {
    if let Some(context) = debian_runtime::for_entry(owner, entry, deb_package_refcount)? {
        return Ok(EntryRuntimeContext {
            environment: context.environment,
            package_instance_count: Some(context.package_refcount),
        });
    }
    Ok(EntryRuntimeContext {
        environment: entry.native_invocation.environment.clone(),
        package_instance_count: None,
    })
}

fn native_lifecycle_execution<'a>(
    entry: &'a NativeLifecycleEntry,
    environment: &'a [String],
) -> NativeLifecycleExecution<'a> {
    NativeLifecycleExecution {
        entry_id: &entry.id,
        phase: entry.phase.as_str(),
        interpreter: &entry.interpreter,
        interpreter_args: &entry.interpreter_args,
        body: entry.body.clone(),
        body_sha256: entry.body_sha256.clone(),
        body_encoding: entry.body_encoding.as_deref(),
        shell_entrypoint: entry
            .arch_install
            .as_ref()
            .map(|metadata| metadata.called_function.as_str()),
        native_args: &entry.native_invocation.args,
        native_environment: environment,
        stdin_contract: entry.native_invocation.stdin.as_deref(),
        chroot_contract: entry.native_invocation.chroot.as_deref(),
        timeout_ms: entry.timeout_ms,
    }
}

fn native_invocation_runtime<'a>(
    mode: &'a ExecutionMode,
    package_instance_count: Option<u32>,
    args: &'a [String],
    stdin: &'a [u8],
) -> NativeInvocationRuntime<'a> {
    NativeInvocationRuntime {
        mode,
        old_version: None,
        new_version: None,
        package_instance_count,
        resolved_native_args: Some(args),
        stdin,
    }
}
