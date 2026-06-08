// conary-core/src/scriptlet/legacy.rs

use super::{ExecutionMode, ScriptletExecutor, ScriptletFailureKind, ScriptletOutcome};
use anyhow::{Result as AnyhowResult, bail};
use std::path::PathBuf;
use std::time::Duration;
use tracing::warn;

const LEGACY_MIN_TIMEOUT_MS: u64 = 1_000;
const LEGACY_MAX_TIMEOUT_MS: u64 = 300_000;
const LEGACY_SAFE_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const DANGEROUS_LEGACY_ENV_KEYS: [&str; 6] = [
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "BASH_ENV",
    "ENV",
    "PYTHONPATH",
    "PATH",
];

/// Executor-facing view of a legacy bundle entry.
pub struct LegacyScriptletExecution<'a> {
    pub entry_id: &'a str,
    pub phase: &'a str,
    pub interpreter: &'a str,
    pub interpreter_args: &'a [String],
    pub body: String,
    pub body_sha256: String,
    pub body_encoding: Option<&'a str>,
    pub native_args: &'a [String],
    pub native_environment: &'a [String],
    pub stdin_contract: Option<&'a str>,
    pub chroot_contract: Option<&'a str>,
    pub timeout_ms: u64,
}

/// Runtime values needed to resolve native package-manager invocation contracts.
pub struct LegacyInvocationRuntime<'a> {
    pub mode: &'a ExecutionMode,
    pub old_version: Option<&'a str>,
    pub new_version: Option<&'a str>,
    pub package_instance_count: Option<u32>,
}

impl ScriptletExecutor {
    /// Preflight a legacy bundle entry before mutation or temporary-file writes.
    pub fn preflight_legacy_entry(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> AnyhowResult<()> {
        self.validate_legacy_execution_contracts(execution, runtime)
    }

    /// Execute a legacy bundle entry and return typed outcome metadata.
    pub fn execute_legacy_entry_with_outcome(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> ScriptletOutcome {
        let requested_sandbox_mode = self.sandbox_mode;
        let effective_sandbox = self.effective_sandbox(false);

        if let Err(error) = self.preflight_legacy_entry(execution, runtime) {
            return self.failure_outcome(
                execution.phase,
                ScriptletFailureKind::SandboxSetupUnavailable,
                requested_sandbox_mode,
                effective_sandbox,
                error.to_string(),
            );
        }

        let script_content = match decode_legacy_body(execution) {
            Ok(script_content) => script_content,
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
        let args = match self.derive_legacy_native_args(execution, runtime) {
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
        let env = match self.legacy_environment(execution) {
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
        let use_sandbox = self.should_use_sandbox(&script_content);
        let effective_sandbox = self.effective_sandbox(use_sandbox);
        let executor = self.clone_with_timeout(Duration::from_millis(execution.timeout_ms));

        let result = if executor.is_live_root() {
            if use_sandbox {
                executor.execute_sandbox_live(
                    execution.phase,
                    execution.interpreter,
                    &script_content,
                    &args,
                    &env_refs,
                )
            } else {
                executor.execute_direct_with_options(
                    execution.phase,
                    execution.interpreter,
                    execution.interpreter_args,
                    &script_content,
                    &args,
                    &env_refs,
                    Duration::from_millis(execution.timeout_ms),
                )
            }
        } else {
            let interpreter_check_path = executor
                .root
                .join(execution.interpreter.trim_start_matches('/'));
            if !interpreter_check_path.exists() {
                warn!(
                    "Interpreter {} not found in target root {}, skipping {} legacy scriptlet",
                    execution.interpreter,
                    executor.root.display(),
                    execution.phase
                );
                return ScriptletOutcome::Skipped {
                    phase: execution.phase.to_string(),
                    requested_sandbox_mode,
                    effective_sandbox,
                };
            }
            executor.execute_in_target(
                execution.phase,
                execution.interpreter,
                execution.interpreter_args,
                &script_content,
                &args,
                &env_refs,
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

    fn validate_legacy_execution_contracts(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> AnyhowResult<()> {
        if execution.timeout_ms < LEGACY_MIN_TIMEOUT_MS
            || execution.timeout_ms > LEGACY_MAX_TIMEOUT_MS
        {
            bail!(
                "TimeoutOutOfRange: legacy entry '{}' timeout_ms {} is outside {}..={}",
                execution.entry_id,
                execution.timeout_ms,
                LEGACY_MIN_TIMEOUT_MS,
                LEGACY_MAX_TIMEOUT_MS
            );
        }

        let script_content = decode_legacy_body(execution)?;
        let use_sandbox = self.should_use_sandbox(&script_content);
        self.validate_legacy_interpreter_args(execution, use_sandbox)?;
        self.derive_legacy_native_args(execution, runtime)?;
        self.legacy_environment(execution)?;
        validate_stdin_contract(execution)?;
        validate_chroot_contract(execution)?;

        let interpreter_check_path = if self.is_live_root() {
            PathBuf::from(execution.interpreter)
        } else {
            self.root
                .join(execution.interpreter.trim_start_matches('/'))
        };

        if !interpreter_check_path.exists() {
            if self.is_live_root() {
                bail!(
                    "SandboxRequirementUnsupported: Interpreter not found: {}. Cannot execute legacy entry '{}'.",
                    execution.interpreter,
                    execution.entry_id
                );
            }
            return Ok(());
        }

        if self.is_live_root() && use_sandbox {
            self.preflight_protected_live_sandbox()
                .map_err(|error| anyhow::anyhow!("SandboxRequirementUnsupported: {error}"))?;
        }

        Ok(())
    }

    fn validate_legacy_interpreter_args(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        use_sandbox: bool,
    ) -> AnyhowResult<()> {
        for arg in execution.interpreter_args {
            if arg.contains('\0') {
                bail!(
                    "NativeArgsContractUnsupported: legacy entry '{}' has an interpreter arg containing NUL",
                    execution.entry_id
                );
            }
        }

        if self.is_live_root() && use_sandbox && !execution.interpreter_args.is_empty() {
            bail!(
                "NativeArgsContractUnsupported: legacy interpreter_args are unsupported with protected live-root sandboxing in Goal 6"
            );
        }

        Ok(())
    }

    fn derive_legacy_native_args(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> AnyhowResult<Vec<String>> {
        if execution.native_args.is_empty() {
            return Ok(self.get_args(runtime.mode, execution.phase));
        }

        let mut args = Vec::with_capacity(execution.native_args.len());
        for contract in execution.native_args {
            if let Some(literal) = contract.strip_prefix("raw:") {
                args.push(literal.to_string());
                continue;
            }

            let Some((position, projection)) = contract.split_once(':') else {
                bail!(
                    "NativeArgsContractUnsupported: malformed legacy native arg contract '{contract}'"
                );
            };
            let parsed_position = position.parse::<usize>().map_err(|_| {
                anyhow::anyhow!(
                    "NativeArgsContractUnsupported: malformed legacy native arg position '{position}'"
                )
            })?;
            if parsed_position == 0 {
                bail!("NativeArgsContractUnsupported: legacy native arg positions are one-based");
            }

            let Some((name, runtime_key)) = projection.split_once('=') else {
                bail!(
                    "NativeArgsContractUnsupported: malformed legacy native arg projection '{projection}'"
                );
            };
            if name != runtime_key {
                bail!(
                    "NativeArgsContractUnsupported: legacy native arg projection '{projection}' is not supported"
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
                            "NativeArgsContractUnsupported: package-instance-count is unavailable for legacy native args"
                        )
                    }),
                other => Err(anyhow::anyhow!(
                    "NativeArgsContractUnsupported: unsupported legacy native arg runtime value '{other}'"
                )),
            }?;
            args.push(value);
        }

        Ok(args)
    }

    fn legacy_environment(
        &self,
        execution: &LegacyScriptletExecution<'_>,
    ) -> AnyhowResult<Vec<(String, String)>> {
        let mut env = vec![
            ("CONARY_PACKAGE_NAME".to_string(), self.package_name.clone()),
            (
                "CONARY_PACKAGE_VERSION".to_string(),
                self.package_version.clone(),
            ),
            ("CONARY_ROOT".to_string(), "/".to_string()),
            ("CONARY_PHASE".to_string(), execution.phase.to_string()),
            ("PATH".to_string(), LEGACY_SAFE_PATH.to_string()),
        ];

        for item in execution.native_environment {
            let Some((key, value)) = item.split_once('=') else {
                bail!(
                    "NativeArgsContractUnsupported: bare native environment key '{}' requires an explicit runtime value",
                    item
                );
            };
            validate_legacy_environment_key(key)?;
            env.push((key.to_string(), value.to_string()));
        }

        Ok(env)
    }
}

fn decode_legacy_body(execution: &LegacyScriptletExecution<'_>) -> AnyhowResult<String> {
    let body_bytes = match execution.body_encoding.unwrap_or("utf-8") {
        "utf-8" => execution.body.as_bytes().to_vec(),
        "base64" => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(&execution.body)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "NativeArgsContractUnsupported: legacy entry '{}' body base64 decode failed: {error}",
                        execution.entry_id
                    )
                })?
        }
        other => bail!(
            "NativeArgsContractUnsupported: legacy entry '{}' body_encoding '{}' is unsupported",
            execution.entry_id,
            other
        ),
    };

    let actual = crate::hash::sha256_prefixed(&body_bytes);
    if !actual.eq_ignore_ascii_case(&execution.body_sha256) {
        bail!(
            "NativeArgsContractUnsupported: legacy entry '{}' body_sha256 mismatch: expected {}, got {}",
            execution.entry_id,
            execution.body_sha256,
            actual
        );
    }

    String::from_utf8(body_bytes).map_err(|error| {
        anyhow::anyhow!(
            "NativeArgsContractUnsupported: legacy entry '{}' body is not UTF-8 executable script text: {error}",
            execution.entry_id
        )
    })
}

fn runtime_old_version(runtime: &LegacyInvocationRuntime<'_>) -> AnyhowResult<String> {
    runtime
        .old_version
        .map(str::to_string)
        .or_else(|| match runtime.mode {
            ExecutionMode::Upgrade { old_version } => Some(old_version.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "NativeArgsContractUnsupported: old-version is unavailable for legacy native args"
            )
        })
}

fn runtime_new_version(
    executor: &ScriptletExecutor,
    runtime: &LegacyInvocationRuntime<'_>,
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

fn validate_stdin_contract(execution: &LegacyScriptletExecution<'_>) -> AnyhowResult<()> {
    match execution.stdin_contract {
        None | Some("none") | Some("null") => Ok(()),
        Some(other) => bail!(
            "NativeArgsContractUnsupported: legacy entry '{}' stdin contract '{}' is unsupported in Goal 6",
            execution.entry_id,
            other
        ),
    }
}

fn validate_chroot_contract(execution: &LegacyScriptletExecution<'_>) -> AnyhowResult<()> {
    match execution.chroot_contract {
        None | Some("install-root") | Some("package-manager-default") => Ok(()),
        Some(other) => bail!(
            "SandboxRequirementUnsupported: legacy entry '{}' chroot contract '{}' is unsupported in Goal 6",
            execution.entry_id,
            other
        ),
    }
}

fn validate_legacy_environment_key(key: &str) -> AnyhowResult<()> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        || key.as_bytes()[0].is_ascii_digit()
    {
        bail!("NativeArgsContractUnsupported: malformed native environment key '{key}'");
    }

    if DANGEROUS_LEGACY_ENV_KEYS.contains(&key) {
        bail!("NativeArgsContractUnsupported: native environment key '{key}' is denied");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{
        ExecutionMode, PackageFormat, SandboxMode, ScriptletExecutor, ScriptletOutcome,
    };
    use super::{LegacyInvocationRuntime, LegacyScriptletExecution};
    use std::path::Path;

    fn legacy_execution_with_contracts(native_args: &[String]) -> LegacyScriptletExecution<'_> {
        let body = "echo legacy\n".to_string();
        LegacyScriptletExecution {
            entry_id: "legacy-entry",
            phase: "post-install",
            interpreter: "/bin/sh",
            interpreter_args: &[],
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            body_encoding: None,
            native_args,
            native_environment: &[],
            stdin_contract: None,
            chroot_contract: None,
            timeout_ms: 30_000,
        }
    }

    fn upgrade_runtime(mode: &ExecutionMode) -> LegacyInvocationRuntime<'_> {
        LegacyInvocationRuntime {
            mode,
            old_version: Some("0.9.0"),
            new_version: Some("1.0.0"),
            package_instance_count: Some(2),
        }
    }

    #[test]
    fn legacy_native_arg_contracts_use_runtime_versions_and_literals() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);
        let contracts = vec![
            "1:old-version=old-version".to_string(),
            "2:new-version=new-version".to_string(),
            "raw:literal".to_string(),
        ];
        let execution = legacy_execution_with_contracts(&contracts);
        let mode = ExecutionMode::Upgrade {
            old_version: "should-not-leak".to_string(),
        };

        let args = executor
            .derive_legacy_native_args(&execution, &upgrade_runtime(&mode))
            .expect("contracts derive");

        assert_eq!(args, vec!["0.9.0", "1.0.0", "literal"]);
    }

    #[test]
    fn legacy_native_arg_contracts_use_runtime_remove_count() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        let contracts = vec!["1:count=count".to_string()];
        let execution = legacy_execution_with_contracts(&contracts);
        let mode = ExecutionMode::Remove;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: Some(0),
        };

        let args = executor
            .derive_legacy_native_args(&execution, &runtime)
            .expect("remove count contract derives");

        assert_eq!(args, vec!["0"]);
    }

    #[test]
    fn legacy_native_arg_contracts_refuse_malformed_or_missing_runtime_values() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);

        for contracts in [
            vec!["old-version=old-version".to_string()],
            vec!["1:unknown=unsupported".to_string()],
            vec!["1:old-version=old-version".to_string()],
        ] {
            let execution = legacy_execution_with_contracts(&contracts);
            let mode = ExecutionMode::Install;
            let runtime = LegacyInvocationRuntime {
                mode: &mode,
                old_version: None,
                new_version: None,
                package_instance_count: None,
            };

            let error = executor
                .derive_legacy_native_args(&execution, &runtime)
                .expect_err("unsupported contract should refuse");
            assert!(
                error.to_string().contains("NativeArgsContractUnsupported"),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn legacy_preflight_refuses_unsupported_invocation_fields() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);
        let mode = ExecutionMode::Remove;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: Some(0),
        };

        let env = vec!["LD_PRELOAD=/tmp/libhack.so".to_string()];
        let path_env = vec!["PATH=/tmp/hijack".to_string()];
        let bare_env = vec!["RPM_INSTALL_PREFIX".to_string()];

        let cases = [
            LegacyScriptletExecution {
                stdin_contract: Some("debconf"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                stdin_contract: Some("paths"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                stdin_contract: Some("unknown"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                chroot_contract: Some("host-root"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                chroot_contract: Some("unknown"),
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                native_environment: &env,
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                native_environment: &path_env,
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                native_environment: &bare_env,
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                timeout_ms: 999,
                ..legacy_execution_with_contracts(&[])
            },
            LegacyScriptletExecution {
                timeout_ms: 300_001,
                ..legacy_execution_with_contracts(&[])
            },
        ];

        for execution in cases {
            let error = executor
                .preflight_legacy_entry(&execution, &runtime)
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
    fn legacy_preflight_rejects_body_hash_mismatch() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);
        let mode = ExecutionMode::Install;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: None,
            new_version: None,
            package_instance_count: None,
        };
        let execution = LegacyScriptletExecution {
            body_sha256: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            ..legacy_execution_with_contracts(&[])
        };

        let error = executor
            .preflight_legacy_entry(&execution, &runtime)
            .expect_err("body hash mismatch should refuse");

        assert!(
            error.to_string().contains("body_sha256 mismatch"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn legacy_execution_uses_safe_path_and_derived_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb)
                .with_sandbox_mode(SandboxMode::None);
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
        let execution = LegacyScriptletExecution {
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            ..legacy_execution_with_contracts(&contracts)
        };
        let mode = ExecutionMode::Upgrade {
            old_version: "should-not-leak".to_string(),
        };

        let outcome =
            executor.execute_legacy_entry_with_outcome(&execution, &upgrade_runtime(&mode));

        assert!(
            matches!(outcome, ScriptletOutcome::Success { .. }),
            "{outcome:?}"
        );
    }

    #[test]
    fn legacy_execution_skips_target_root_when_interpreter_is_absent() {
        let root = tempfile::tempdir().expect("target root");
        let executor = ScriptletExecutor::new(root.path(), "test-pkg", "1.0.0", PackageFormat::Rpm)
            .with_sandbox_mode(SandboxMode::None);
        let mode = ExecutionMode::Remove;
        let runtime = LegacyInvocationRuntime {
            mode: &mode,
            old_version: Some("1.0.0"),
            new_version: None,
            package_instance_count: Some(0),
        };
        let execution = LegacyScriptletExecution {
            phase: "post-remove",
            ..legacy_execution_with_contracts(&[])
        };

        let outcome = executor.execute_legacy_entry_with_outcome(&execution, &runtime);

        assert!(
            matches!(outcome, ScriptletOutcome::Skipped { .. }),
            "{outcome:?}"
        );
    }
}
