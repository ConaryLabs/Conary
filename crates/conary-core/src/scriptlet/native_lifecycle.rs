// conary-core/src/scriptlet/native_lifecycle.rs

mod arch_install;
mod contracts;

use contracts::{
    NATIVE_LIFECYCLE_SAFE_PATH, validate_chroot_contract, validate_environment_key,
    validate_stdin_contract,
};
pub use contracts::{
    NativeInterpreterAvailability, NativeInvocationRuntime, NativeLifecycleExecution,
};

use super::process::ScriptletProcess;
use super::rpm_runtime::{execute_embedded_lua, prepare_body as prepare_rpm_body};
use super::{ExecutionMode, ScriptletExecutor, ScriptletFailureKind, ScriptletOutcome};
use crate::ccs::native_lifecycle::{RpmProgram, RpmRuntimeMetadata};
use anyhow::{Result as AnyhowResult, bail};
use std::time::Duration;

const NATIVE_LIFECYCLE_MIN_TIMEOUT_MS: u64 = 1_000;
const NATIVE_LIFECYCLE_MAX_TIMEOUT_MS: u64 = 300_000;

impl ScriptletExecutor {
    /// Preflight a native lifecycle bundle entry before mutation or temporary-file writes.
    pub fn preflight_native_lifecycle_entry(
        &self,
        execution: &NativeLifecycleExecution<'_>,
        runtime: &NativeInvocationRuntime<'_>,
        interpreter_availability: NativeInterpreterAvailability,
    ) -> AnyhowResult<()> {
        self.validate_native_lifecycle_contracts(execution, runtime, None, interpreter_availability)
    }

    /// Preflight an RPM lifecycle entry, including its exact body transforms
    /// and bundled program contract.
    pub fn preflight_rpm_native_lifecycle_entry(
        &self,
        execution: &NativeLifecycleExecution<'_>,
        runtime: &NativeInvocationRuntime<'_>,
        rpm: &RpmRuntimeMetadata,
        interpreter_availability: NativeInterpreterAvailability,
    ) -> AnyhowResult<()> {
        self.validate_native_lifecycle_contracts(
            execution,
            runtime,
            Some(rpm),
            interpreter_availability,
        )
    }

    /// Execute a native lifecycle bundle entry and return typed outcome metadata.
    pub fn execute_native_lifecycle_entry_with_outcome(
        &self,
        execution: &NativeLifecycleExecution<'_>,
        runtime: &NativeInvocationRuntime<'_>,
    ) -> ScriptletOutcome {
        self.execute_native_lifecycle_entry_impl(execution, runtime, None)
    }

    /// Execute an RPM lifecycle entry with its persisted RPM runtime.
    pub fn execute_rpm_native_lifecycle_entry_with_outcome(
        &self,
        execution: &NativeLifecycleExecution<'_>,
        runtime: &NativeInvocationRuntime<'_>,
        rpm: &RpmRuntimeMetadata,
    ) -> ScriptletOutcome {
        self.execute_native_lifecycle_entry_impl(execution, runtime, Some(rpm))
    }

    fn execute_native_lifecycle_entry_impl(
        &self,
        execution: &NativeLifecycleExecution<'_>,
        runtime: &NativeInvocationRuntime<'_>,
        rpm: Option<&RpmRuntimeMetadata>,
    ) -> ScriptletOutcome {
        let requested_sandbox_mode = self.sandbox_mode;
        let effective_sandbox = self.effective_sandbox();

        if let Err(error) = self.validate_native_lifecycle_contracts(
            execution,
            runtime,
            rpm,
            NativeInterpreterAvailability::CurrentRoot,
        ) {
            return self.failure_outcome(
                execution.phase,
                ScriptletFailureKind::SandboxSetupUnavailable,
                requested_sandbox_mode,
                effective_sandbox,
                error.to_string(),
            );
        }

        let body_bytes = match decode_native_lifecycle_body_bytes(execution) {
            Ok(body_bytes) => body_bytes,
            Err(error) => {
                return self.failure_outcome(
                    execution.phase,
                    ScriptletFailureKind::SandboxSetupUnavailable,
                    requested_sandbox_mode,
                    effective_sandbox,
                    error.to_string(),
                );
            }
        };
        let args = match self.derive_native_args(execution, runtime) {
            Ok(args) => args,
            Err(error) => {
                return self.failure_outcome(
                    execution.phase,
                    ScriptletFailureKind::SandboxSetupUnavailable,
                    requested_sandbox_mode,
                    effective_sandbox,
                    error.to_string(),
                );
            }
        };
        let env = match self.native_lifecycle_environment(execution) {
            Ok(env) => env,
            Err(error) => {
                return self.failure_outcome(
                    execution.phase,
                    ScriptletFailureKind::SandboxSetupUnavailable,
                    requested_sandbox_mode,
                    effective_sandbox,
                    error.to_string(),
                );
            }
        };
        let env_refs: Vec<(&str, &str)> = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let executor = self.clone_with_timeout(Duration::from_millis(execution.timeout_ms));

        if execution.shell_entrypoint.is_some() {
            let effective_sandbox = executor.effective_sandbox();
            let result = arch_install::execute(
                &executor,
                execution,
                &body_bytes,
                &args,
                &env_refs,
                runtime.stdin,
            );
            return match result {
                Ok(()) => ScriptletOutcome::Success {
                    phase: execution.phase.to_string(),
                    requested_sandbox_mode,
                    effective_sandbox,
                },
                Err(error) => executor.failure_from_error(
                    execution.phase,
                    requested_sandbox_mode,
                    effective_sandbox,
                    error,
                ),
            };
        }

        let transformed_rpm_body = match rpm {
            Some(rpm) => {
                let script_content = match std::str::from_utf8(&body_bytes) {
                    Ok(script_content) => script_content,
                    Err(error) => {
                        return self.failure_outcome(
                            execution.phase,
                            ScriptletFailureKind::ContractViolation,
                            requested_sandbox_mode,
                            effective_sandbox,
                            format!(
                                "RPM lifecycle entry '{}' body is not UTF-8 text: {error}",
                                execution.entry_id
                            ),
                        );
                    }
                };
                match prepare_rpm_body(&executor, script_content, &env, rpm) {
                    Ok(script_content) => Some(script_content),
                    Err(error) => {
                        return self.failure_outcome(
                            execution.phase,
                            ScriptletFailureKind::ContractViolation,
                            requested_sandbox_mode,
                            effective_sandbox,
                            error.to_string(),
                        );
                    }
                }
            }
            None => None,
        };
        let executable_body = transformed_rpm_body
            .as_deref()
            .map_or(body_bytes.as_slice(), str::as_bytes);
        let effective_sandbox = self.effective_sandbox();

        let result = if rpm.is_some_and(|rpm| rpm.program == RpmProgram::EmbeddedLua) {
            execute_embedded_lua(
                &executor,
                execution.phase,
                transformed_rpm_body
                    .as_deref()
                    .expect("embedded Lua always has RPM text authority"),
                &args,
                runtime.stdin,
                rpm.expect("embedded Lua predicate requires RPM metadata"),
                Duration::from_millis(execution.timeout_ms),
            )
        } else {
            executor.execute_in_target(
                ScriptletProcess {
                    phase: execution.phase,
                    interpreter: execution.interpreter,
                    interpreter_args: execution.interpreter_args,
                    args: &args,
                    env: &env_refs,
                    stdin: runtime.stdin,
                },
                executable_body,
            )
        };

        match result {
            Ok(()) => ScriptletOutcome::Success {
                phase: execution.phase.to_string(),
                requested_sandbox_mode,
                effective_sandbox,
            },
            Err(error) => executor.failure_from_error(
                execution.phase,
                requested_sandbox_mode,
                effective_sandbox,
                error,
            ),
        }
    }

    fn validate_native_lifecycle_contracts(
        &self,
        execution: &NativeLifecycleExecution<'_>,
        runtime: &NativeInvocationRuntime<'_>,
        rpm: Option<&RpmRuntimeMetadata>,
        interpreter_availability: NativeInterpreterAvailability,
    ) -> AnyhowResult<()> {
        self.require_target_root()
            .map_err(|error| anyhow::anyhow!("SandboxRequirementUnsupported: {error}"))?;
        if execution.timeout_ms < NATIVE_LIFECYCLE_MIN_TIMEOUT_MS
            || execution.timeout_ms > NATIVE_LIFECYCLE_MAX_TIMEOUT_MS
        {
            bail!(
                "TimeoutOutOfRange: native lifecycle entry '{}' timeout_ms {} is outside {}..={}",
                execution.entry_id,
                execution.timeout_ms,
                NATIVE_LIFECYCLE_MIN_TIMEOUT_MS,
                NATIVE_LIFECYCLE_MAX_TIMEOUT_MS
            );
        }

        let body_bytes = decode_native_lifecycle_body_bytes(execution)?;
        let args = self.derive_native_args(execution, runtime)?;
        if execution.shell_entrypoint.is_some() {
            if rpm.is_some() {
                bail!(
                    "RpmProgramContractError: RPM lifecycle entry '{}' declares an Arch .INSTALL function",
                    execution.entry_id
                );
            }
            arch_install::validate(self, execution, &args, runtime.stdin)?;
        }
        let embedded_lua = rpm.is_some_and(|rpm| rpm.program == RpmProgram::EmbeddedLua);
        if let Some(rpm) = rpm {
            match rpm.program {
                RpmProgram::External => {}
                RpmProgram::EmbeddedLua => {
                    if execution.interpreter != "<lua>" {
                        bail!(
                            "RpmEmbeddedLuaContractError: entry '{}' declares embedded Lua with interpreter '{}'",
                            execution.entry_id,
                            execution.interpreter
                        );
                    }
                    if !execution.interpreter_args.is_empty() {
                        bail!(
                            "RpmEmbeddedLuaContractError: entry '{}' declares interpreter arguments for <lua>",
                            execution.entry_id
                        );
                    }
                }
                RpmProgram::Sysusers => bail!(
                    "RpmProgramContractError: sysusers entry '{}' reached the scriptlet executor",
                    execution.entry_id
                ),
            }
        }
        self.validate_native_interpreter_args(execution)?;
        let environment = self.native_lifecycle_environment(execution)?;
        validate_stdin_contract(execution)?;
        validate_chroot_contract(execution)?;

        if let Some(rpm) = rpm {
            let body = std::str::from_utf8(&body_bytes).map_err(|error| {
                anyhow::anyhow!(
                    "RPM lifecycle entry '{}' body is not UTF-8 text: {error}",
                    execution.entry_id
                )
            })?;
            prepare_rpm_body(self, body, &environment, rpm)?;
        }

        if embedded_lua {
            return Ok(());
        }

        let interpreter_present = match interpreter_availability {
            NativeInterpreterAvailability::CurrentRoot => {
                let interpreter_check_path = self
                    .root
                    .join(execution.interpreter.trim_start_matches('/'));
                interpreter_check_path.exists()
            }
            NativeInterpreterAvailability::ProjectedPresent => true,
            NativeInterpreterAvailability::ProjectedMissing => false,
        };

        if !interpreter_present {
            bail!(
                "SandboxRequirementUnsupported: Interpreter not found: {}. Cannot execute native lifecycle entry '{}'.",
                execution.interpreter,
                execution.entry_id
            );
        }

        Ok(())
    }

    fn validate_native_interpreter_args(
        &self,
        execution: &NativeLifecycleExecution<'_>,
    ) -> AnyhowResult<()> {
        for arg in execution.interpreter_args {
            if arg.contains('\0') {
                bail!(
                    "NativeArgsContractUnsupported: native lifecycle entry '{}' has an interpreter arg containing NUL",
                    execution.entry_id
                );
            }
        }

        Ok(())
    }

    fn derive_native_args(
        &self,
        execution: &NativeLifecycleExecution<'_>,
        runtime: &NativeInvocationRuntime<'_>,
    ) -> AnyhowResult<Vec<String>> {
        if let Some(args) = runtime.resolved_native_args {
            if args.iter().any(|arg| arg.contains('\0')) {
                bail!(
                    "NativeArgsContractUnsupported: resolved native arguments contain NUL for entry '{}'",
                    execution.entry_id
                );
            }
            return Ok(args.to_vec());
        }
        if execution.native_args.is_empty() {
            return self
                .get_args(runtime.mode, execution.phase)
                .map_err(anyhow::Error::from);
        }

        let mut args = Vec::with_capacity(execution.native_args.len());
        for contract in execution.native_args {
            if let Some(literal) = contract.strip_prefix("raw:") {
                args.push(literal.to_string());
                continue;
            }

            let Some((position, projection)) = contract.split_once(':') else {
                bail!("NativeArgsContractUnsupported: malformed native arg contract '{contract}'");
            };
            let parsed_position = position.parse::<usize>().map_err(|_| {
                anyhow::anyhow!(
                    "NativeArgsContractUnsupported: malformed native arg position '{position}'"
                )
            })?;
            if parsed_position == 0 {
                bail!("NativeArgsContractUnsupported: native arg positions are one-based");
            }

            let Some((name, runtime_key)) = projection.split_once('=') else {
                bail!(
                    "NativeArgsContractUnsupported: malformed native arg projection '{projection}'"
                );
            };
            if name != runtime_key {
                bail!(
                    "NativeArgsContractUnsupported: native arg projection '{projection}' is not supported"
                );
            }

            let value = match runtime_key {
                "old-version" => runtime_old_version(runtime),
                "new-version" => Ok(runtime_new_version(self, runtime)),
                "package-instance-count" | "count" => runtime
                    .package_instance_count
                    .map(|count| count.to_string())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "NativeArgsContractUnsupported: package-instance-count is unavailable for native args"
                        )
                    }),
                other => Err(anyhow::anyhow!(
                    "NativeArgsContractUnsupported: unsupported native arg runtime value '{other}'"
                )),
            }?;
            args.push(value);
        }

        Ok(args)
    }

    fn native_lifecycle_environment(
        &self,
        execution: &NativeLifecycleExecution<'_>,
    ) -> AnyhowResult<Vec<(String, String)>> {
        let mut env = vec![
            ("CONARY_PACKAGE_NAME".to_string(), self.package_name.clone()),
            (
                "CONARY_PACKAGE_VERSION".to_string(),
                self.package_version.clone(),
            ),
            ("CONARY_ROOT".to_string(), "/".to_string()),
            ("CONARY_PHASE".to_string(), execution.phase.to_string()),
            ("PATH".to_string(), NATIVE_LIFECYCLE_SAFE_PATH.to_string()),
        ];

        for item in execution.native_environment {
            let Some((key, value)) = item.split_once('=') else {
                bail!(
                    "NativeArgsContractUnsupported: bare native environment key '{}' requires an explicit runtime value",
                    item
                );
            };
            validate_environment_key(key)?;
            env.push((key.to_string(), value.to_string()));
        }

        Ok(env)
    }
}

fn decode_native_lifecycle_body_bytes(
    execution: &NativeLifecycleExecution<'_>,
) -> AnyhowResult<Vec<u8>> {
    let body_bytes = match execution.body_encoding.unwrap_or("utf-8") {
        "utf-8" => execution.body.as_bytes().to_vec(),
        "base64" => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(&execution.body)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "NativeArgsContractUnsupported: native lifecycle entry '{}' body base64 decode failed: {error}",
                        execution.entry_id
                    )
                })?
        }
        other => bail!(
            "NativeArgsContractUnsupported: native lifecycle entry '{}' body_encoding '{}' is unsupported",
            execution.entry_id,
            other
        ),
    };

    let actual = crate::hash::sha256_prefixed(&body_bytes);
    if !actual.eq_ignore_ascii_case(&execution.body_sha256) {
        bail!(
            "NativeArgsContractUnsupported: native lifecycle entry '{}' body_sha256 mismatch: expected {}, got {}",
            execution.entry_id,
            execution.body_sha256,
            actual
        );
    }

    Ok(body_bytes)
}

fn runtime_old_version(runtime: &NativeInvocationRuntime<'_>) -> AnyhowResult<String> {
    runtime
        .old_version
        .map(str::to_string)
        .or_else(|| match runtime.mode {
            ExecutionMode::Upgrade { old_version } => Some(old_version.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "NativeArgsContractUnsupported: old-version is unavailable for native args"
            )
        })
}

fn runtime_new_version(
    executor: &ScriptletExecutor,
    runtime: &NativeInvocationRuntime<'_>,
) -> String {
    runtime
        .new_version
        .map(str::to_string)
        .or_else(|| match runtime.mode {
            ExecutionMode::UpgradeRemoval { new_version } => Some(new_version.clone()),
            _ => None,
        })
        .unwrap_or_else(|| executor.package_version.clone())
}

#[cfg(test)]
mod tests {
    use super::super::{
        ExecutionMode, PackageFormat, SandboxMode, ScriptletExecutor, ScriptletOutcome,
    };
    use super::{NativeInterpreterAvailability, NativeInvocationRuntime, NativeLifecycleExecution};
    use crate::ccs::native_lifecycle::{
        RpmBodyTransform, RpmCriticality, RpmHeaderContext, RpmHeaderFact, RpmHeaderFactSource,
        RpmHeaderValue, RpmMacroContext, RpmProgram, RpmRuntimeMetadata,
    };
    use crate::scriptlet::test_support::materialized_root;
    use std::path::Path;

    fn native_lifecycle_execution_with_contracts(
        native_args: &[String],
    ) -> NativeLifecycleExecution<'_> {
        let body = "echo native_lifecycle\n".to_string();
        NativeLifecycleExecution {
            entry_id: "native_lifecycle-entry",
            phase: "post-install",
            interpreter: "/bin/sh",
            interpreter_args: &[],
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            body_encoding: None,
            shell_entrypoint: None,
            native_args,
            native_environment: &[],
            stdin_contract: None,
            chroot_contract: None,
            timeout_ms: 30_000,
        }
    }

    fn upgrade_runtime(mode: &ExecutionMode) -> NativeInvocationRuntime<'_> {
        NativeInvocationRuntime {
            mode,
            old_version: Some("0.9.0"),
            new_version: Some("1.0.0"),
            package_instance_count: Some(2),
            resolved_native_args: None,
            stdin: &[],
        }
    }

    #[test]
    fn native_lifecycle_native_arg_contracts_use_runtime_versions_and_literals() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);
        let contracts = vec![
            "1:old-version=old-version".to_string(),
            "2:new-version=new-version".to_string(),
            "raw:literal".to_string(),
        ];
        let execution = native_lifecycle_execution_with_contracts(&contracts);
        let mode = ExecutionMode::Upgrade {
            old_version: "should-not-leak".to_string(),
        };

        let args = executor
            .derive_native_args(&execution, &upgrade_runtime(&mode))
            .expect("contracts derive");

        assert_eq!(args, vec!["0.9.0", "1.0.0", "literal"]);
    }

    #[test]
    fn native_lifecycle_native_arg_contracts_use_runtime_remove_count() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        let contracts = vec!["1:count=count".to_string()];
        let execution = native_lifecycle_execution_with_contracts(&contracts);
        let mode = ExecutionMode::Remove;
        let runtime = NativeInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: Some(0),
            resolved_native_args: None,
            stdin: &[],
        };

        let args = executor
            .derive_native_args(&execution, &runtime)
            .expect("remove count contract derives");

        assert_eq!(args, vec!["0"]);
    }

    #[test]
    fn protected_live_sandbox_accepts_exact_interpreter_arguments() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        let interpreter_args = vec!["-eu".to_string(), "-o".to_string(), "pipefail".to_string()];
        let execution = NativeLifecycleExecution {
            interpreter_args: &interpreter_args,
            ..native_lifecycle_execution_with_contracts(&[])
        };

        executor
            .validate_native_interpreter_args(&execution)
            .expect("sandbox command builder preserves interpreter arguments");
    }

    #[test]
    fn native_lifecycle_native_arg_contracts_refuse_malformed_or_missing_runtime_values() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);

        for contracts in [
            vec!["old-version=old-version".to_string()],
            vec!["1:unknown=unsupported".to_string()],
            vec!["1:old-version=old-version".to_string()],
        ] {
            let execution = native_lifecycle_execution_with_contracts(&contracts);
            let mode = ExecutionMode::Install;
            let runtime = NativeInvocationRuntime {
                mode: &mode,
                old_version: None,
                new_version: None,
                package_instance_count: None,
                resolved_native_args: None,
                stdin: &[],
            };

            let error = executor
                .derive_native_args(&execution, &runtime)
                .expect_err("unsupported contract should refuse");
            assert!(
                error.to_string().contains("NativeArgsContractUnsupported"),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn native_lifecycle_preflight_refuses_unsupported_invocation_fields() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::Always);
        let mode = ExecutionMode::Install;
        let runtime = NativeInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: Some(1),
            resolved_native_args: None,
            stdin: &[],
        };

        let env = vec!["LD_PRELOAD=/tmp/libhack.so".to_string()];
        let path_env = vec!["PATH=/tmp/hijack".to_string()];
        let bare_env = vec!["RPM_INSTALL_PREFIX".to_string()];

        let cases = [
            NativeLifecycleExecution {
                stdin_contract: Some("unknown"),
                ..native_lifecycle_execution_with_contracts(&[])
            },
            NativeLifecycleExecution {
                chroot_contract: Some("host-root"),
                ..native_lifecycle_execution_with_contracts(&[])
            },
            NativeLifecycleExecution {
                chroot_contract: Some("unknown"),
                ..native_lifecycle_execution_with_contracts(&[])
            },
            NativeLifecycleExecution {
                native_environment: &env,
                ..native_lifecycle_execution_with_contracts(&[])
            },
            NativeLifecycleExecution {
                native_environment: &path_env,
                ..native_lifecycle_execution_with_contracts(&[])
            },
            NativeLifecycleExecution {
                native_environment: &bare_env,
                ..native_lifecycle_execution_with_contracts(&[])
            },
            NativeLifecycleExecution {
                timeout_ms: 999,
                ..native_lifecycle_execution_with_contracts(&[])
            },
            NativeLifecycleExecution {
                timeout_ms: 300_001,
                ..native_lifecycle_execution_with_contracts(&[])
            },
        ];

        for execution in cases {
            let error = executor
                .preflight_native_lifecycle_entry(
                    &execution,
                    &runtime,
                    NativeInterpreterAvailability::CurrentRoot,
                )
                .expect_err("unsupported invocation field should refuse");
            let message = error.to_string();
            assert!(
                message.contains("NativeArgsContractUnsupported")
                    || message.contains("SandboxRequirementUnsupported")
                    || message.contains("TimeoutOutOfRange"),
                "unexpected error: {message}"
            );
        }
    }

    #[test]
    fn native_lifecycle_preflight_rejects_body_hash_mismatch() {
        let Some(root) = materialized_root(
            "scriptlet::native_lifecycle::tests::native_lifecycle_preflight_rejects_body_hash_mismatch",
            &["/bin/sh"],
        ) else {
            return;
        };
        let executor = ScriptletExecutor::new(root.path(), "test-pkg", "1.0.0", PackageFormat::Rpm)
            .with_sandbox_mode(SandboxMode::Always);
        let mode = ExecutionMode::Install;
        let runtime = NativeInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: None,
            resolved_native_args: None,
            stdin: &[],
        };
        let execution = NativeLifecycleExecution {
            body_sha256: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            ..native_lifecycle_execution_with_contracts(&[])
        };

        let error = executor
            .preflight_native_lifecycle_entry(
                &execution,
                &runtime,
                NativeInterpreterAvailability::CurrentRoot,
            )
            .expect_err("body hash mismatch should refuse");

        assert!(
            error.to_string().contains("body_sha256 mismatch"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn native_lifecycle_execution_uses_safe_path_and_derived_args() {
        let Some(root) = materialized_root(
            "scriptlet::native_lifecycle::tests::native_lifecycle_execution_uses_safe_path_and_derived_args",
            &["/bin/sh"],
        ) else {
            return;
        };
        let executor = ScriptletExecutor::new(root.path(), "test-pkg", "1.0.0", PackageFormat::Deb)
            .with_sandbox_mode(SandboxMode::Always);
        let contracts = vec![
            "1:old-version=old-version".to_string(),
            "2:new-version=new-version".to_string(),
            "raw:literal".to_string(),
        ];
        let body = r#"
                test "$PATH" = "/usr/sbin:/usr/bin:/sbin:/bin"
                test "$1" = "0.9.0"
                test "$2" = "1.0.0"
                test "$3" = "literal"
            "#
        .to_string();
        let execution = NativeLifecycleExecution {
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            ..native_lifecycle_execution_with_contracts(&contracts)
        };
        let mode = ExecutionMode::Upgrade {
            old_version: "should-not-leak".to_string(),
        };

        let outcome = executor
            .execute_native_lifecycle_entry_with_outcome(&execution, &upgrade_runtime(&mode));

        assert!(
            matches!(outcome, ScriptletOutcome::Success { .. }),
            "{outcome:?}"
        );
    }

    #[test]
    fn debian_binary_maintainer_body_executes_as_exact_bytes() {
        use base64::Engine as _;

        let Some(root) = materialized_root(
            "scriptlet::native_lifecycle::tests::debian_binary_maintainer_body_executes_as_exact_bytes",
            &["/bin/sh"],
        ) else {
            return;
        };
        let executor = ScriptletExecutor::new(root.path(), "test-pkg", "1.0.0", PackageFormat::Deb)
            .with_sandbox_mode(SandboxMode::Always);
        let bytes = b"exit 0\n# binary suffix: \xff\n";
        let execution = NativeLifecycleExecution {
            body: base64::engine::general_purpose::STANDARD.encode(bytes),
            body_sha256: crate::hash::sha256_prefixed(bytes),
            body_encoding: Some("base64"),
            ..native_lifecycle_execution_with_contracts(&[])
        };
        let mode = ExecutionMode::Install;
        let args = Vec::new();
        let runtime = NativeInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: Some("1.0.0"),
            package_instance_count: None,
            resolved_native_args: Some(&args),
            stdin: &[],
        };

        executor
            .preflight_native_lifecycle_entry(
                &execution,
                &runtime,
                NativeInterpreterAvailability::CurrentRoot,
            )
            .expect("non-UTF-8 Debian body is valid external-interpreter input");
        let outcome = executor.execute_native_lifecycle_entry_with_outcome(&execution, &runtime);
        assert!(
            matches!(outcome, ScriptletOutcome::Success { .. }),
            "{outcome:?}"
        );
    }

    #[test]
    fn native_lifecycle_execution_fails_when_target_interpreter_is_absent() {
        let root = tempfile::tempdir().expect("target root");
        let executor = ScriptletExecutor::new(root.path(), "test-pkg", "1.0.0", PackageFormat::Rpm)
            .with_sandbox_mode(SandboxMode::Always);
        let mode = ExecutionMode::Remove;
        let runtime = NativeInvocationRuntime {
            mode: &mode,
            old_version: Some("1.0.0"),
            new_version: None,
            package_instance_count: Some(0),
            resolved_native_args: None,
            stdin: &[],
        };
        let execution = NativeLifecycleExecution {
            phase: "post-remove",
            ..native_lifecycle_execution_with_contracts(&[])
        };

        let outcome = executor.execute_native_lifecycle_entry_with_outcome(&execution, &runtime);

        assert!(
            matches!(outcome, ScriptletOutcome::Failure(_)),
            "{outcome:?}"
        );
    }

    #[test]
    fn rpm_native_lifecycle_executes_query_format_and_embedded_lua_programs() {
        let Some(root) = materialized_root(
            "scriptlet::native_lifecycle::tests::rpm_native_lifecycle_executes_query_format_and_embedded_lua_programs",
            &["/bin/sh"],
        ) else {
            return;
        };
        let executor = ScriptletExecutor::new(root.path(), "test-pkg", "1.0.0", PackageFormat::Rpm)
            .with_sandbox_mode(SandboxMode::Always);
        let mode = ExecutionMode::Install;
        let args = ["1".to_string()];
        let invocation = NativeInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: Some("1.0.0"),
            package_instance_count: Some(1),
            resolved_native_args: Some(&args),
            stdin: &[],
        };
        let rpm = RpmRuntimeMetadata {
            program: RpmProgram::External,
            body_transforms: vec![RpmBodyTransform::HeaderQueryFormat],
            critical: false,
            criticality: RpmCriticality::WarningOnly,
            raw_flags: 0,
            unknown_flags: 0,
            install_prefixes: Vec::new(),
            macro_context: RpmMacroContext::default(),
            header_context: RpmHeaderContext {
                facts: vec![RpmHeaderFact {
                    tag: 1000,
                    name: Some("NAME".to_string()),
                    value: RpmHeaderValue::String("test-pkg".to_string()),
                    source: RpmHeaderFactSource::Header,
                }],
                format_environment: Default::default(),
                database_instance: 0,
            },
            package_rpm_version: Some("6.0.0".to_string()),
        };
        let body = "test \"%{NAME}\" = \"test-pkg\"\n".to_string();
        let execution = NativeLifecycleExecution {
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            ..native_lifecycle_execution_with_contracts(&[])
        };

        let transformed =
            executor.execute_rpm_native_lifecycle_entry_with_outcome(&execution, &invocation, &rpm);
        assert!(
            matches!(transformed, ScriptletOutcome::Success { .. }),
            "{transformed:?}"
        );

        let lua_body = "assert(arg[1] == '<lua>'); assert(arg[2] == 1)\n".to_string();
        let lua_execution = NativeLifecycleExecution {
            interpreter: "<lua>",
            body_sha256: crate::hash::sha256_prefixed(lua_body.as_bytes()),
            body: lua_body,
            ..native_lifecycle_execution_with_contracts(&[])
        };
        let lua_rpm = RpmRuntimeMetadata {
            program: RpmProgram::EmbeddedLua,
            body_transforms: Vec::new(),
            header_context: RpmHeaderContext::default(),
            ..rpm
        };
        let embedded = executor.execute_rpm_native_lifecycle_entry_with_outcome(
            &lua_execution,
            &invocation,
            &lua_rpm,
        );
        assert!(
            matches!(embedded, ScriptletOutcome::Success { .. }),
            "{embedded:?}"
        );
    }
}
