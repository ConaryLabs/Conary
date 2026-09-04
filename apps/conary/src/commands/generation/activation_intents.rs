// apps/conary/src/commands/generation/activation_intents.rs

//! Consume exact runtime lifecycle work for the generation proven at boot.

use anyhow::{Context, Result, anyhow, bail};
use conary_core::activation::{ActivationExecutableIdentity, RuntimeActivationInvocation};
use conary_core::ccs::HostCapabilityInventory;
use conary_core::db::models::{GenerationActivationIntent, SystemState};
use conary_core::runtime_root::ConaryRuntimeRoot;
use rusqlite::Connection;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActivationCommandOutcome {
    success: bool,
    code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActivationRunSummary {
    pub(crate) generation_number: Option<i64>,
    pub(crate) applied: usize,
}

/// Apply lifecycle work from the immutable generation selected by the kernel
/// command line. Absence of a Conary generation is an exact successful no-op,
/// allowing the boot service to remain installed on native boots.
pub(crate) fn cmd_generation_activate(db_path: &str) -> Result<ActivationRunSummary> {
    let cmdline_path = crate::test_hooks::get()
        .proc_cmdline_path()
        .unwrap_or_else(|| std::path::PathBuf::from("/proc/cmdline"));
    let cmdline = std::fs::read_to_string(&cmdline_path)
        .with_context(|| format!("failed to read {}", cmdline_path.display()))?;
    let Some(generation_number) = generation_from_kernel_cmdline(&cmdline)? else {
        return Ok(ActivationRunSummary {
            generation_number: None,
            applied: 0,
        });
    };
    let runtime_root = ConaryRuntimeRoot::from_db_path(db_path);
    validate_generation_artifact(&runtime_root, generation_number)?;
    let conn = crate::commands::open_db(db_path)?;
    if SystemState::find_by_number(&conn, generation_number)?.is_none() {
        bail!(
            "booted generation {generation_number} has no matching database state; activation intents were not consumed"
        );
    }

    apply_generation_intents(&conn, generation_number, |executable, arguments| {
        let status = Command::new(executable)
            .args(arguments)
            .stdin(Stdio::null())
            .status()
            .with_context(|| {
                format!(
                    "failed to execute persisted generation activation interface {}",
                    executable.display()
                )
            })?;
        Ok(ActivationCommandOutcome {
            success: status.success(),
            code: status.code(),
        })
    })
}

pub(super) fn generation_from_kernel_cmdline(cmdline: &str) -> Result<Option<i64>> {
    let values = cmdline
        .split_ascii_whitespace()
        .filter_map(|argument| argument.strip_prefix("conary.generation="))
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(None),
        [value] => {
            let generation = value.parse::<i64>().with_context(|| {
                format!("Invalid conary.generation value in /proc/cmdline: {value}")
            })?;
            if generation < 0 {
                bail!("conary.generation must be a non-negative integer");
            }
            Ok(Some(generation))
        }
        _ => bail!(
            "kernel command line contains multiple conary.generation arguments; activation ownership is ambiguous"
        ),
    }
}

fn validate_generation_artifact(
    runtime_root: &ConaryRuntimeRoot,
    generation_number: i64,
) -> Result<()> {
    let generation_dir = runtime_root.generation_path(generation_number);
    let artifact = conary_core::generation::artifact::load_generation_artifact_with_verified_cas(
        &generation_dir,
    )
    .with_context(|| {
        format!("booted generation {generation_number} does not have a valid activation artifact")
    })?;
    if artifact.generation != generation_number {
        bail!(
            "booted generation {generation_number} artifact declares generation {}",
            artifact.generation
        );
    }
    Ok(())
}

fn apply_generation_intents<F>(
    conn: &Connection,
    generation_number: i64,
    mut execute: F,
) -> Result<ActivationRunSummary>
where
    F: FnMut(&Path, &[String]) -> Result<ActivationCommandOutcome>,
{
    let recovered =
        GenerationActivationIntent::recover_interrupted_for_generation(conn, generation_number)?;
    if recovered > 0 {
        tracing::warn!(
            generation_number,
            recovered,
            "retrying exact activation requests interrupted before durable completion"
        );
    }

    let intents = GenerationActivationIntent::ready_for_generation(conn, generation_number)?;
    if intents.is_empty() {
        return Ok(ActivationRunSummary {
            generation_number: Some(generation_number),
            applied: 0,
        });
    }
    let mut applied = 0;
    let mut failures = Vec::new();
    for intent in intents {
        intent.mark_executing(conn)?;
        let command = match resolve_activation_command(conn, &intent.request.invocation) {
            Ok(command) => command,
            Err(error) => {
                let message = format!("activation interface verification failed: {error:#}");
                intent.mark_failed(conn, &message)?;
                failures.push(request_failure(&intent, &message));
                continue;
            }
        };
        let outcome = match execute(&command.executable, &command.arguments) {
            Ok(outcome) => outcome,
            Err(error) => {
                let message = format!(
                    "failed to start {} activation interface: {error:#}",
                    command.provider
                );
                intent.mark_failed(conn, &message)?;
                failures.push(request_failure(&intent, &message));
                continue;
            }
        };
        if !outcome.success {
            let message = format!(
                "{} activation interface exited unsuccessfully with status {:?}",
                command.provider, outcome.code
            );
            intent.mark_failed(conn, &message)?;
            failures.push(request_failure(&intent, &message));
            continue;
        }
        intent.mark_applied(conn)?;
        applied += 1;
    }
    if !failures.is_empty() {
        bail!(
            "{} generation activation request(s) remain retryable after this pass: {}",
            failures.len(),
            failures.join("; ")
        );
    }

    Ok(ActivationRunSummary {
        generation_number: Some(generation_number),
        applied,
    })
}

struct ResolvedActivationCommand {
    executable: std::path::PathBuf,
    arguments: Vec<String>,
    provider: &'static str,
}

fn resolve_activation_command(
    conn: &Connection,
    invocation: &RuntimeActivationInvocation,
) -> Result<ResolvedActivationCommand> {
    match invocation {
        RuntimeActivationInvocation::Systemd(systemd) => Ok(ResolvedActivationCommand {
            executable: resolve_booted_systemctl(conn)?,
            arguments: systemd.systemctl_args(),
            provider: "systemd",
        }),
        RuntimeActivationInvocation::OpenRc(openrc) => Ok(ResolvedActivationCommand {
            executable: resolve_booted_openrc_service(conn)?,
            arguments: openrc.rc_service_args(),
            provider: "openrc",
        }),
        RuntimeActivationInvocation::SecurityPolicy(policy) => {
            let identity = policy.executable();
            Ok(ResolvedActivationCommand {
                executable: verify_booted_provider_executable(identity)?,
                arguments: policy.arguments().to_vec(),
                provider: policy.provider(),
            })
        }
        RuntimeActivationInvocation::BootRuntime(boot_runtime) => Ok(ResolvedActivationCommand {
            executable: verify_booted_provider_executable(&boot_runtime.executable)?,
            arguments: boot_runtime.arguments.clone(),
            provider: boot_runtime.program.as_str(),
        }),
    }
}

fn resolve_booted_openrc_service(conn: &Connection) -> Result<std::path::PathBuf> {
    if let Ok(inventory) = HostCapabilityInventory::load_required(conn)
        && let Some(executable) = inventory.live_openrc_service_path()
    {
        return Ok(executable.to_path_buf());
    }
    let refreshed = HostCapabilityInventory::discover()
        .context("failed to discover refreshed OpenRC host capability inventory")?;
    let executable = refreshed
        .live_openrc_service_path()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            anyhow!("generation activation requires a verified OpenRC rc-service interface")
        })?;
    refreshed.persist(conn)?;
    Ok(executable)
}

fn verify_booted_provider_executable(
    identity: &ActivationExecutableIdentity,
) -> Result<std::path::PathBuf> {
    identity
        .validate()
        .map_err(|error| anyhow!(error.to_string()))?;
    let invoked = std::path::PathBuf::from(&identity.invoked_path);
    let canonical = std::fs::canonicalize(&invoked).with_context(|| {
        format!(
            "booted generation is missing captured provider path {}",
            invoked.display()
        )
    })?;
    let expected = std::path::Path::new(&identity.canonical_path);
    if canonical != expected {
        bail!(
            "booted provider path {} resolves to {}, expected captured identity {}",
            invoked.display(),
            canonical.display(),
            expected.display()
        );
    }
    let metadata = canonical.metadata().with_context(|| {
        format!(
            "failed to inspect booted provider executable {}",
            canonical.display()
        )
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        bail!(
            "booted provider identity {} is not an executable regular file",
            canonical.display()
        );
    }
    let observed =
        conary_core::hash::sha256_prefixed(&std::fs::read(&canonical).with_context(|| {
            format!(
                "failed to read booted provider executable {}",
                canonical.display()
            )
        })?);
    if observed != identity.sha256 {
        bail!(
            "booted provider executable {} has digest {}, expected {}",
            canonical.display(),
            observed,
            identity.sha256
        );
    }
    Ok(invoked)
}

fn request_failure(intent: &GenerationActivationIntent, message: &str) -> String {
    format!(
        "request {} from {} {} ({}): {message}",
        intent.request.id,
        intent.request.source_package,
        intent.request.source_version,
        intent.request.source_entry
    )
}

fn resolve_booted_systemctl(conn: &Connection) -> Result<std::path::PathBuf> {
    resolve_booted_systemctl_with(conn, HostCapabilityInventory::discover)
}

fn resolve_booted_systemctl_with(
    conn: &Connection,
    discover: impl FnOnce() -> std::result::Result<
        HostCapabilityInventory,
        conary_core::ccs::HostCapabilityInventoryError,
    >,
) -> Result<std::path::PathBuf> {
    if let Ok(inventory) = HostCapabilityInventory::load_required(conn)
        && let Some(executable) = inventory.live_systemctl_path()
    {
        return Ok(executable.to_path_buf());
    }

    let refreshed = discover()
        .context("failed to discover refreshed booted-generation host capability inventory")?;
    let executable = refreshed
        .live_systemctl_path()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            anyhow!(
                "generation activation requires a structurally verified systemd manager and systemctl executable in the booted generation"
            )
        })?;
    refreshed
        .persist(conn)
        .context("failed to persist refreshed booted-generation host capability inventory")?;
    tracing::info!(
        executable = %executable.display(),
        "refreshed exact host capability inventory from the booted generation"
    );
    Ok(executable)
}

#[cfg(test)]
#[path = "activation_intents/tests.rs"]
mod tests;
