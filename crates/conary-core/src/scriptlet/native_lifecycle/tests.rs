// conary-core/src/scriptlet/native_lifecycle/tests.rs

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
    let executor = ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);
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
    let executor = ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
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
    let executor = ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
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
    let executor = ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);

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
    let executor = ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
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

    let outcome =
        executor.execute_native_lifecycle_entry_with_outcome(&execution, &upgrade_runtime(&mode));

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
