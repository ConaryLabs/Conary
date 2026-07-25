// conary-core/src/scriptlet/native_lifecycle/arch_install.rs

//! Exact libalpm-style execution of Arch `.INSTALL` function libraries.

use super::NativeLifecycleExecution;
use crate::error::{Error, Result, ScriptletFailureKind};
use crate::scriptlet::{PackageFormat, ScriptletExecutor};
use anyhow::{Result as AnyhowResult, bail};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use tempfile::{Builder as TempBuilder, TempDir};

pub(super) fn validate(
    executor: &ScriptletExecutor,
    execution: &NativeLifecycleExecution<'_>,
    args: &[String],
    stdin: &[u8],
) -> AnyhowResult<()> {
    if executor.package_format != PackageFormat::Arch {
        bail!(
            "NativeArgsContractUnsupported: shell entrypoint '{}' is reserved for Arch .INSTALL entries",
            execution.shell_entrypoint.unwrap_or_default()
        );
    }
    if !execution.interpreter_args.is_empty() {
        bail!(
            "NativeArgsContractUnsupported: Arch .INSTALL entry '{}' declares interpreter arguments; libalpm invokes only the configured shell with -c",
            execution.entry_id
        );
    }
    if !stdin.is_empty() {
        bail!(
            "NativeArgsContractUnsupported: Arch .INSTALL entry '{}' declares non-empty stdin",
            execution.entry_id
        );
    }
    executor
        .preflight_native_command(&[execution.interpreter.to_string(), "-c".to_string()])
        .map_err(|error| anyhow::anyhow!("SandboxRequirementUnsupported: {error}"))?;

    let entrypoint = validated_entrypoint(execution)?;
    let expected_args = match entrypoint {
        "pre_install" | "post_install" | "pre_remove" | "post_remove" => 1,
        "pre_upgrade" | "post_upgrade" => 2,
        _ => unreachable!("validated_entrypoint accepts only the six libalpm functions"),
    };
    if args.len() != expected_args {
        bail!(
            "NativeArgsContractUnsupported: Arch .INSTALL function '{entrypoint}' requires {expected_args} version argument(s), got {}",
            args.len()
        );
    }
    for version in args {
        validate_full_package_version(version)?;
    }
    Ok(())
}

pub(super) fn execute(
    executor: &ScriptletExecutor,
    execution: &NativeLifecycleExecution<'_>,
    source: &[u8],
    args: &[String],
    environment: &[(&str, &str)],
    stdin: &[u8],
) -> Result<()> {
    executor.require_target_root()?;
    let entrypoint = execution
        .shell_entrypoint
        .expect("Arch .INSTALL execution follows successful validation");
    let source_file = PreparedInstallSource::create(&executor.root, source)?;
    let command = source_and_call_command(&source_file.guest_file, entrypoint, args);
    let shell_argv = vec![execution.interpreter.to_string(), "-c".to_string(), command];

    let result = executor.execute_native_command_with_environment(
        execution.phase,
        &shell_argv,
        stdin,
        environment,
    );

    source_file.cleanup(result)
}

fn validated_entrypoint<'a>(execution: &'a NativeLifecycleExecution<'_>) -> AnyhowResult<&'a str> {
    let entrypoint = execution.shell_entrypoint.ok_or_else(|| {
        anyhow::anyhow!(
            "NativeArgsContractUnsupported: Arch .INSTALL entry '{}' has no function",
            execution.entry_id
        )
    })?;
    match entrypoint {
        "pre_install" | "post_install" | "pre_upgrade" | "post_upgrade" | "pre_remove"
        | "post_remove" => Ok(entrypoint),
        _ => bail!(
            "NativeArgsContractUnsupported: Arch .INSTALL entry '{}' has unknown function '{}'",
            execution.entry_id,
            entrypoint
        ),
    }
}

/// Validate the full package version grammar passed to `.INSTALL`.
///
/// PKGBUILD(5) defines `pkgver-pkgrel` or `epoch:pkgver-pkgrel`. `pkgver` is
/// non-empty ASCII without colon, slash, hyphen, or whitespace; `pkgrel` is a
/// positive integer with at most one positive integer subrelease; a serialized
/// epoch is a positive integer.
fn validate_full_package_version(version: &str) -> AnyhowResult<()> {
    if version.is_empty() || !version.is_ascii() {
        bail!(
            "NativeArgsContractUnsupported: Arch lifecycle version '{version}' is not a non-empty ASCII full package version"
        );
    }

    let version_release = if let Some((epoch, remainder)) = version.split_once(':') {
        if remainder.contains(':') || !is_positive_integer(epoch) {
            bail!(
                "NativeArgsContractUnsupported: Arch lifecycle version '{version}' has an invalid epoch"
            );
        }
        remainder
    } else {
        version
    };

    let Some((pkgver, pkgrel)) = version_release.rsplit_once('-') else {
        bail!(
            "NativeArgsContractUnsupported: Arch lifecycle version '{version}' is missing pkgrel"
        );
    };
    if pkgver.is_empty()
        || pkgver
            .bytes()
            .any(|byte| byte == b':' || byte == b'/' || byte == b'-' || byte.is_ascii_whitespace())
    {
        bail!(
            "NativeArgsContractUnsupported: Arch lifecycle version '{version}' has an invalid pkgver"
        );
    }

    let mut release_parts = pkgrel.split('.');
    let release = release_parts.next().unwrap_or_default();
    let subrelease = release_parts.next();
    if !is_positive_integer(release)
        || subrelease.is_some_and(|part| !is_positive_integer(part))
        || release_parts.next().is_some()
    {
        bail!(
            "NativeArgsContractUnsupported: Arch lifecycle version '{version}' has an invalid pkgrel"
        );
    }
    Ok(())
}

fn is_positive_integer(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.bytes().any(|byte| byte != b'0')
}

fn source_and_call_command(source: &Path, entrypoint: &str, args: &[String]) -> String {
    let mut command = format!(". {}; {entrypoint}", shell_quote(&source.to_string_lossy()));
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

struct PreparedInstallSource {
    temp_dir: TempDir,
    guest_file: PathBuf,
}

impl PreparedInstallSource {
    fn create(root: &Path, source: &[u8]) -> Result<Self> {
        let target_tmp = root.join("tmp");
        if !target_tmp.exists() {
            let mut builder = DirBuilder::new();
            builder.recursive(true).mode(0o1777);
            builder.create(&target_tmp)?;
        }

        let temp_dir = TempBuilder::new().prefix("alpm_").tempdir_in(&target_tmp)?;
        let relative_dir = temp_dir.path().strip_prefix(root).map_err(|error| {
            Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                format!(
                    "Arch .INSTALL temporary directory escaped target root {}: {error}",
                    root.display()
                ),
            )
        })?;
        let guest_dir = Path::new("/").join(relative_dir);
        let guest_file = guest_dir.join(".INSTALL");
        fs::write(temp_dir.path().join(".INSTALL"), source)?;
        Ok(Self {
            temp_dir,
            guest_file,
        })
    }

    fn cleanup(self, result: Result<()>) -> Result<()> {
        match (result, self.temp_dir.close()) {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(error)) => Err(Error::scriptlet(
                ScriptletFailureKind::ProcessSetupFailed,
                format!("Arch .INSTALL temporary file cleanup failed: {error}"),
            )),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup_error)) => {
                tracing::warn!("Arch .INSTALL temporary file cleanup also failed: {cleanup_error}");
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{NativeInvocationRuntime, NativeLifecycleExecution};
    use super::{shell_quote, source_and_call_command, validate_full_package_version};
    use crate::scriptlet::{ExecutionMode, PackageFormat, ScriptletExecutor, ScriptletOutcome};
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    #[test]
    fn full_package_version_uses_documented_arch_grammar() {
        for version in ["1.0-1", "2:1.0+git'next-3.4"] {
            validate_full_package_version(version).expect(version);
        }
        for version in [
            "",
            "1.0",
            "0:1.0-1",
            "1:1.0-0",
            "1:1.0-1.2.3",
            "1:1.0 bad-1",
            "1:1.0/next-1",
            "1:1.0-next-1",
        ] {
            assert!(
                validate_full_package_version(version).is_err(),
                "{version} should be invalid"
            );
        }
    }

    #[test]
    fn source_and_call_quotes_exact_versions_without_positional_shell_args() {
        let args = vec!["2:1.0'next-3.4".to_string(), "0.9+$old-2".to_string()];
        assert_eq!(
            source_and_call_command(
                Path::new("/tmp/alpm_fixture/.INSTALL"),
                "post_upgrade",
                &args
            ),
            ". '/tmp/alpm_fixture/.INSTALL'; post_upgrade '2:1.0'\\''next-3.4' '0.9+$old-2'"
        );
        assert_eq!(shell_quote("plain"), "'plain'");
    }

    #[test]
    fn runtime_sources_with_empty_positional_args_then_calls_exact_function() {
        let fixture = tempfile::tempdir().expect("fixture");
        let configured_shell = fixture.path().join("configured-arch-shell");
        symlink("/usr/bin/bash", &configured_shell).expect("configured shell symlink");
        let output = fixture.path().join("result");
        let environment = vec![format!("ARCH_RUNTIME_RESULT={}", output.display())];
        let body = r#"
printf 'source|argc=%s|zero=%s|all=%s|file=%s\n' \
    "$#" "$0" "$*" "${BASH_SOURCE[0]}" > "$ARCH_RUNTIME_RESULT"
post_upgrade() {
    printf 'function|argc=%s|one=%s|two=%s\n' \
        "$#" "$1" "$2" >> "$ARCH_RUNTIME_RESULT"
}
"#
        .to_string();
        let args = vec!["2:1.0'next-3.4".to_string(), "0.9+$old-2".to_string()];
        let execution = NativeLifecycleExecution {
            entry_id: "arch:post_upgrade",
            phase: "post-upgrade",
            interpreter: configured_shell.to_str().expect("UTF-8 shell path"),
            interpreter_args: &[],
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            body_encoding: None,
            shell_entrypoint: Some("post_upgrade"),
            native_args: &[],
            native_environment: &environment,
            stdin_contract: Some("none"),
            chroot_contract: Some("install-root"),
            timeout_ms: 30_000,
        };
        let mode = ExecutionMode::Upgrade {
            old_version: args[1].clone(),
        };
        let runtime = NativeInvocationRuntime {
            mode: &mode,
            old_version: Some(&args[1]),
            new_version: Some(&args[0]),
            package_instance_count: None,
            resolved_native_args: Some(&args),
            stdin: &[],
        };
        let executor = ScriptletExecutor::new(
            Path::new("/"),
            "arch-runtime-fixture",
            &args[0],
            PackageFormat::Arch,
        );

        let outcome = executor.execute_native_lifecycle_entry_with_outcome(&execution, &runtime);
        assert!(
            matches!(outcome, ScriptletOutcome::Success { .. }),
            "{outcome:?}"
        );

        let result = std::fs::read_to_string(&output).expect("runtime output");
        let mut lines = result.lines();
        let source = lines.next().expect("source line");
        let source_prefix = format!(
            "source|argc=0|zero={}|all=|file=",
            configured_shell.display()
        );
        assert!(
            source.starts_with(&source_prefix),
            "unexpected source context: {source}"
        );
        assert_eq!(
            lines.next(),
            Some("function|argc=2|one=2:1.0'next-3.4|two=0.9+$old-2")
        );
        assert_eq!(lines.next(), None);

        let source_path = PathBuf::from(source.strip_prefix(&source_prefix).expect("source path"));
        assert!(
            source_path.ends_with(".INSTALL"),
            "{}",
            source_path.display()
        );
        assert!(
            !source_path.exists(),
            "temporary source survived at {}",
            source_path.display()
        );
    }
}
