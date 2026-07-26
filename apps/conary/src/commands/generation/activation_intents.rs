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
    let cmdline =
        std::fs::read_to_string("/proc/cmdline").context("failed to read /proc/cmdline")?;
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
    let artifact = conary_core::generation::artifact::load_generation_artifact_for_activation(
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
        RuntimeActivationInvocation::SecurityPolicy(policy) => {
            let identity = policy.executable();
            Ok(ResolvedActivationCommand {
                executable: verify_booted_provider_executable(identity)?,
                arguments: policy.arguments().to_vec(),
                provider: policy.provider(),
            })
        }
    }
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
    discover: impl FnOnce() -> HostCapabilityInventory,
) -> Result<std::path::PathBuf> {
    if let Ok(inventory) = HostCapabilityInventory::load_required(conn)
        && let Some(executable) = inventory.live_systemctl_path()
    {
        return Ok(executable.to_path_buf());
    }

    let refreshed = discover();
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
mod tests {
    use super::*;
    use clap::Parser;
    use conary_core::activation::{
        SecurityPolicyInvocationDisposition, SystemdActivationAction, SystemdActivationInvocation,
        parse_security_policy_invocation,
    };
    use conary_core::ccs::{
        ExecutableInterface, HostExecutableContract, HostExecutableImplementation,
        InitSystemCapability, SystemdInterface, SystemdOperation,
    };
    use conary_core::db::models::{
        ActivationRequest, ActivationRequestSourceKind, Changeset, ChangesetStatus,
        GenerationActivationIntentStatus, NewActivationRequest,
    };
    use std::collections::BTreeSet;

    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        conn
    }

    fn request(source_entry: &str, action: SystemdActivationAction) -> NewActivationRequest {
        NewActivationRequest {
            source_kind: ActivationRequestSourceKind::CapturedSystemctl,
            source_package: "demo".to_string(),
            source_version: "1".to_string(),
            source_entry: source_entry.to_string(),
            invocation: SystemdActivationInvocation {
                action,
                units: vec![source_entry.to_string()],
                job_mode: None,
                all: false,
                no_block: false,
                no_ask_password: true,
                quiet: false,
                no_warn: false,
                wait: false,
                show_transaction: false,
            }
            .into(),
        }
    }

    #[cfg(unix)]
    fn security_request(
        source_entry: &str,
        identity: ActivationExecutableIdentity,
    ) -> NewActivationRequest {
        let invoked_path = identity.invoked_path.clone();
        let SecurityPolicyInvocationDisposition::Generation(invocation) =
            parse_security_policy_invocation(
                &invoked_path,
                &[
                    "-P".to_string(),
                    "demo_can_network".to_string(),
                    "on".to_string(),
                ],
                identity,
            )
            .unwrap()
            .unwrap()
        else {
            panic!("expected generation activation");
        };
        NewActivationRequest {
            source_kind: ActivationRequestSourceKind::CapturedSelinux,
            source_package: "demo-policy".to_string(),
            source_version: "1".to_string(),
            source_entry: source_entry.to_string(),
            invocation: invocation.into(),
        }
    }

    #[cfg(unix)]
    fn write_test_provider(
        program: &str,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        ActivationExecutableIdentity,
    ) {
        use std::os::unix::fs::PermissionsExt;

        let tools = tempfile::tempdir().unwrap();
        let executable = tools.path().join(program);
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let canonical = std::fs::canonicalize(&executable).unwrap();
        let identity = ActivationExecutableIdentity {
            invoked_path: executable.display().to_string(),
            canonical_path: canonical.display().to_string(),
            sha256: conary_core::hash::sha256_prefixed(&std::fs::read(&canonical).unwrap()),
        };
        (tools, executable, identity)
    }

    #[cfg(unix)]
    fn write_test_systemctl() -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let tools = tempfile::tempdir().unwrap();
        let executable = tools.path().join("systemctl");
        std::fs::write(
            &executable,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'systemd 257'; exit 0; fi\nexit 0\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        (tools, executable)
    }

    #[cfg(unix)]
    fn test_systemd_inventory(executable: std::path::PathBuf) -> HostCapabilityInventory {
        let executable_sha256 =
            conary_core::hash::sha256_prefixed(&std::fs::read(&executable).unwrap());
        HostCapabilityInventory {
            schema_version: conary_core::ccs::HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION,
            init_system: InitSystemCapability::Systemd,
            systemd: Some(SystemdInterface {
                command: ExecutableInterface {
                    executable,
                    contract: HostExecutableContract::SystemdSystemctl,
                    implementation: HostExecutableImplementation::Systemd,
                    implementation_version: "257".to_string(),
                    executable_sha256,
                },
                operations: BTreeSet::from([
                    SystemdOperation::Enable,
                    SystemdOperation::Disable,
                    SystemdOperation::Start,
                    SystemdOperation::Stop,
                    SystemdOperation::Restart,
                    SystemdOperation::DaemonReload,
                    SystemdOperation::OfflineEnable,
                    SystemdOperation::OfflineDisable,
                ]),
            }),
            ..HostCapabilityInventory::default()
        }
    }

    #[cfg(unix)]
    fn persist_test_systemd_inventory(conn: &Connection) -> tempfile::TempDir {
        let (tools, executable) = write_test_systemctl();
        test_systemd_inventory(executable).persist(conn).unwrap();
        tools
    }

    #[test]
    fn packaged_boot_service_command_parses_without_interactive_acknowledgement() {
        let cli = crate::cli::Cli::try_parse_from([
            "conary",
            "system",
            "generation",
            "activate",
            "--db-path",
            "/var/lib/conary/conary.db",
        ])
        .unwrap();
        let policy = crate::command_risk::classify_cli(&cli).unwrap();
        assert_eq!(
            policy.risk,
            crate::command_risk::CommandRisk::GenerationBootActivation
        );
        assert!(!policy.requires_ack());
    }

    #[test]
    fn kernel_contract_requires_one_exact_generation_argument() {
        assert_eq!(
            generation_from_kernel_cmdline("quiet conary.generation=12 ro").unwrap(),
            Some(12)
        );
        assert_eq!(generation_from_kernel_cmdline("quiet ro").unwrap(), None);
        assert!(generation_from_kernel_cmdline("conary.generation=").is_err());
        assert!(generation_from_kernel_cmdline("conary.generation=-1").is_err());
        assert!(
            generation_from_kernel_cmdline("conary.generation=1 quiet conary.generation=2")
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn booted_generation_refreshes_drifted_systemd_interface_without_operator_work() {
        let conn = create_test_db();
        let (_old_tools, old_executable) = write_test_systemctl();
        test_systemd_inventory(old_executable.clone())
            .persist(&conn)
            .unwrap();
        std::fs::write(&old_executable, "#!/bin/sh\nexit 1\n").unwrap();

        let (_current_tools, current_executable) = write_test_systemctl();
        let refreshed = test_systemd_inventory(current_executable.clone());
        let resolved = resolve_booted_systemctl_with(&conn, || refreshed.clone()).unwrap();

        assert_eq!(resolved, current_executable);
        let stored = HostCapabilityInventory::load_required(&conn).unwrap();
        assert_eq!(stored, refreshed);
    }

    #[cfg(unix)]
    #[test]
    fn exact_generation_executes_once_and_persists_completion() {
        let conn = create_test_db();
        let _tools = persist_test_systemd_inventory(&conn);

        let mut changeset = Changeset::new("install demo".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        ActivationRequest::append_batch(
            &conn,
            changeset_id,
            &[request("demo.service", SystemdActivationAction::Restart)],
        )
        .unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();
        SystemState::new(3, "generation 3".to_string())
            .insert(&conn)
            .unwrap();
        GenerationActivationIntent::project_through(&conn, 3, Some(changeset_id)).unwrap();

        let mut calls = Vec::new();
        let first = apply_generation_intents(&conn, 3, |_, args| {
            calls.push(args.to_vec());
            Ok(ActivationCommandOutcome {
                success: true,
                code: Some(0),
            })
        })
        .unwrap();
        let second = apply_generation_intents(&conn, 3, |_, _| {
            panic!("completed activation must not replay")
        })
        .unwrap();

        assert_eq!(first.applied, 1);
        assert_eq!(second.applied, 0);
        assert_eq!(
            calls,
            vec![vec![
                "--system",
                "--no-ask-password",
                "restart",
                "--",
                "demo.service"
            ]]
        );
        let request = ActivationRequest::find_by_id(&conn, 1).unwrap().unwrap();
        let mut statement = conn
            .prepare(
                "SELECT status FROM generation_activation_intents
                 WHERE generation_number = 3 AND request_id = ?1",
            )
            .unwrap();
        let status: String = statement.query_row([request.id], |row| row.get(0)).unwrap();
        assert_eq!(GenerationActivationIntentStatus::Applied.as_str(), status);
    }

    #[cfg(unix)]
    #[test]
    fn failed_invocation_is_durable_and_retryable() {
        let conn = create_test_db();
        let _tools = persist_test_systemd_inventory(&conn);
        let mut changeset = Changeset::new("install demo".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        ActivationRequest::append_batch(
            &conn,
            changeset_id,
            &[request("demo.service", SystemdActivationAction::Start)],
        )
        .unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();
        SystemState::new(4, "generation 4".to_string())
            .insert(&conn)
            .unwrap();
        GenerationActivationIntent::project_through(&conn, 4, Some(changeset_id)).unwrap();

        assert!(
            apply_generation_intents(&conn, 4, |_, _| {
                Ok(ActivationCommandOutcome {
                    success: false,
                    code: Some(5),
                })
            })
            .is_err(),
            "failed systemctl execution must remain retryable"
        );
        let ready = GenerationActivationIntent::ready_for_generation(&conn, 4).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].status, GenerationActivationIntentStatus::Failed);
        assert_eq!(ready[0].attempt_count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn one_failed_request_does_not_starve_later_generation_work() {
        let conn = create_test_db();
        let _tools = persist_test_systemd_inventory(&conn);
        let mut changeset = Changeset::new("install two services".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        ActivationRequest::append_batch(
            &conn,
            changeset_id,
            &[
                request("first.service", SystemdActivationAction::Start),
                request("second.service", SystemdActivationAction::Restart),
            ],
        )
        .unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();
        SystemState::new(5, "generation 5".to_string())
            .insert(&conn)
            .unwrap();
        GenerationActivationIntent::project_through(&conn, 5, Some(changeset_id)).unwrap();

        let mut first_pass = Vec::new();
        let error = apply_generation_intents(&conn, 5, |_, args| {
            first_pass.push(args.to_vec());
            let success = args.last().is_some_and(|unit| unit == "second.service");
            Ok(ActivationCommandOutcome {
                success,
                code: Some(if success { 0 } else { 1 }),
            })
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("1 generation activation request")
        );
        assert_eq!(first_pass.len(), 2);
        let ready = GenerationActivationIntent::ready_for_generation(&conn, 5).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].request.source_entry, "first.service");

        let retry = apply_generation_intents(&conn, 5, |_, args| {
            assert_eq!(args.last().map(String::as_str), Some("first.service"));
            Ok(ActivationCommandOutcome {
                success: true,
                code: Some(0),
            })
        })
        .unwrap();
        assert_eq!(retry.applied, 1);
        assert!(
            GenerationActivationIntent::ready_for_generation(&conn, 5)
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_security_provider_identity_and_argv_are_consumed_once() {
        let conn = create_test_db();
        let (_tools, executable, identity) = write_test_provider("setsebool");
        let mut changeset = Changeset::new("install SELinux policy".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        ActivationRequest::append_batch(
            &conn,
            changeset_id,
            &[security_request("rpm:%post", identity)],
        )
        .unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();
        SystemState::new(6, "generation 6".to_string())
            .insert(&conn)
            .unwrap();
        GenerationActivationIntent::project_through(&conn, 6, Some(changeset_id)).unwrap();

        let mut calls = Vec::new();
        let first = apply_generation_intents(&conn, 6, |path, args| {
            calls.push((path.to_path_buf(), args.to_vec()));
            Ok(ActivationCommandOutcome {
                success: true,
                code: Some(0),
            })
        })
        .unwrap();
        let second = apply_generation_intents(&conn, 6, |_, _| {
            panic!("applied policy request must not replay")
        })
        .unwrap();

        assert_eq!(first.applied, 1);
        assert_eq!(second.applied, 0);
        assert_eq!(
            calls,
            vec![(
                executable,
                vec!["-P", "demo_can_network", "on"]
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            )]
        );
    }

    #[cfg(unix)]
    #[test]
    fn provider_identity_drift_is_durable_and_retryable_without_execution() {
        let conn = create_test_db();
        let (_tools, executable, identity) = write_test_provider("setsebool");
        let mut changeset = Changeset::new("install SELinux policy".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        ActivationRequest::append_batch(
            &conn,
            changeset_id,
            &[security_request("rpm:%post", identity)],
        )
        .unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();
        SystemState::new(7, "generation 7".to_string())
            .insert(&conn)
            .unwrap();
        GenerationActivationIntent::project_through(&conn, 7, Some(changeset_id)).unwrap();
        std::fs::write(&executable, "#!/bin/sh\nexit 9\n").unwrap();

        assert!(
            apply_generation_intents(&conn, 7, |_, _| {
                panic!("identity drift must fail before execution")
            })
            .is_err()
        );
        let ready = GenerationActivationIntent::ready_for_generation(&conn, 7).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].status, GenerationActivationIntentStatus::Failed);
        assert!(ready[0].last_error.as_deref().unwrap().contains("digest"));
    }
}
