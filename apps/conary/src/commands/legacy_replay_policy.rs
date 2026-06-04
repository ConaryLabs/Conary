// apps/conary/src/commands/legacy_replay_policy.rs
//! Legacy replay policy input construction for CLI mutation paths.

use anyhow::{Context, Result};
use conary_core::ccs::legacy_replay::{HostForeignReplayPolicy, LegacyReplayPolicyInput};
use conary_core::ccs::target_compatibility::{
    CompatibilityPreflightEnvironment, TargetCompatibilityMatrix,
};
use conary_core::db::models::DistroPin;
use conary_core::repository::distro::{ReplayTargetOwned, replay_target_from_distro_id};
use conary_core::scriptlet::SandboxMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacyReplayHostContext {
    pub target: ReplayTargetOwned,
    pub host_policy: HostForeignReplayPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LegacyReplayPolicyOptions {
    pub replay_enabled: bool,
    pub foreign_replay_override: bool,
    pub no_scripts: bool,
    pub requested_sandbox_mode: SandboxMode,
}

pub(crate) fn resolve_legacy_replay_host_context(
    conn: &rusqlite::Connection,
) -> Result<LegacyReplayHostContext> {
    let pin = DistroPin::get_current(conn).context("load current distro pin")?;
    let arch = std::env::consts::ARCH;
    let target = pin
        .as_ref()
        .and_then(|pin| replay_target_from_distro_id(&pin.distro, arch))
        .unwrap_or_else(|| ReplayTargetOwned {
            format: "unknown".to_string(),
            distro: "unknown".to_string(),
            release: "unknown".to_string(),
            arch: arch.to_string(),
        });
    let host_policy = host_foreign_replay_policy_from_pin(pin.as_ref());

    Ok(LegacyReplayHostContext {
        target,
        host_policy,
    })
}

pub(crate) fn host_foreign_replay_policy_from_pin(
    pin: Option<&DistroPin>,
) -> HostForeignReplayPolicy {
    match pin.map(|pin| pin.mixing_policy.as_str()) {
        Some("guarded") => HostForeignReplayPolicy::Guarded,
        Some("permissive") => HostForeignReplayPolicy::Permissive,
        _ => HostForeignReplayPolicy::Strict,
    }
}

pub(crate) fn legacy_replay_policy_input<'a>(
    context: &'a LegacyReplayHostContext,
    options: LegacyReplayPolicyOptions,
) -> Result<LegacyReplayPolicyInput<'a>> {
    Ok(LegacyReplayPolicyInput {
        replay_enabled: options.replay_enabled,
        foreign_replay_override: options.foreign_replay_override,
        no_scripts: options.no_scripts,
        requested_sandbox_mode: options.requested_sandbox_mode,
        host_policy: context.host_policy,
        target: context.target.as_target(),
        compatibility_matrix: compatibility_matrix_for_process()?,
        compatibility_environment: CompatibilityPreflightEnvironment {
            effective_sandbox: options.requested_sandbox_mode,
            ..CompatibilityPreflightEnvironment::default()
        },
    })
}

fn compatibility_matrix_for_process() -> Result<TargetCompatibilityMatrix> {
    #[cfg(debug_assertions)]
    {
        if std::env::var("CONARY_TEST_SKIP_GENERATION_MOUNT").as_deref() == Ok("1")
            && let Ok(raw) = std::env::var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON")
        {
            let matrix: TargetCompatibilityMatrix = serde_json::from_str(&raw)
                .context("parse CONARY_TEST_COMPATIBILITY_MATRIX_JSON")?;
            return TargetCompatibilityMatrix::new(matrix.entries().to_vec())
                .context("validate CONARY_TEST_COMPATIBILITY_MATRIX_JSON");
        }
    }

    Ok(TargetCompatibilityMatrix::production_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::legacy_replay::HostForeignReplayPolicy;
    use conary_core::ccs::target_compatibility::{
        MatrixPreflightRequirements, TargetCompatibilityMatrix, TargetCompatibilityMatrixEntry,
        TargetSelector, TargetSelectorArch, TargetSelectorRelease,
    };
    use conary_core::db;
    use conary_core::db::models::DistroPin;
    use conary_core::repository::distro::ReplayTargetOwned;
    use conary_core::scriptlet::SandboxMode;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn host_context_uses_current_distro_pin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("conary.db");
        db::init(&db_path).expect("init db");
        let conn = db::open(&db_path).expect("open db");
        DistroPin::set(&conn, "fedora-44", "guarded").expect("set pin");

        let context = resolve_legacy_replay_host_context(&conn).expect("resolve context");

        assert_eq!(context.target.to_id(), "rpm/fedora/44/x86_64");
        assert_eq!(context.host_policy, HostForeignReplayPolicy::Guarded);
    }

    #[test]
    fn host_context_falls_back_to_unknown_and_strict_without_pin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("conary.db");
        db::init(&db_path).expect("init db");
        let conn = db::open(&db_path).expect("open db");

        let context = resolve_legacy_replay_host_context(&conn).expect("resolve context");

        assert!(
            context
                .target
                .to_id()
                .starts_with("unknown/unknown/unknown/")
        );
        assert_eq!(context.host_policy, HostForeignReplayPolicy::Strict);
    }

    #[test]
    fn policy_input_uses_host_target_not_source_target() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var("CONARY_TEST_SKIP_GENERATION_MOUNT");
            std::env::remove_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON");
        }
        let context = LegacyReplayHostContext {
            target: ReplayTargetOwned {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: "44".to_string(),
                arch: "x86_64".to_string(),
            },
            host_policy: HostForeignReplayPolicy::Guarded,
        };
        let options = LegacyReplayPolicyOptions {
            replay_enabled: true,
            foreign_replay_override: true,
            no_scripts: false,
            requested_sandbox_mode: SandboxMode::Always,
        };

        let input = legacy_replay_policy_input(&context, options).expect("input");

        assert_eq!(input.target.to_id(), "rpm/fedora/44/x86_64");
        assert_eq!(input.host_policy, HostForeignReplayPolicy::Guarded);
        assert!(input.compatibility_matrix.entries().is_empty());
    }

    #[test]
    fn test_matrix_env_is_ignored_without_test_marker() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let matrix_json = serde_json::to_string(&synthetic_matrix()).expect("matrix json");
        unsafe {
            std::env::remove_var("CONARY_TEST_SKIP_GENERATION_MOUNT");
            std::env::set_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON", matrix_json);
        }

        let matrix = compatibility_matrix_for_process().expect("matrix");

        unsafe {
            std::env::remove_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON");
        }
        assert!(matrix.entries().is_empty());
    }

    #[test]
    fn invalid_test_matrix_json_fails_closed() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::set_var("CONARY_TEST_SKIP_GENERATION_MOUNT", "1");
            std::env::set_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON", "{not-json");
        }

        let error = compatibility_matrix_for_process().expect_err("invalid JSON refuses");

        unsafe {
            std::env::remove_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON");
            std::env::remove_var("CONARY_TEST_SKIP_GENERATION_MOUNT");
        }
        assert!(
            error
                .to_string()
                .contains("parse CONARY_TEST_COMPATIBILITY_MATRIX_JSON")
        );
    }

    #[test]
    fn invalid_test_matrix_entries_fail_closed() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let duplicate_json = r#"{
            "entries": [
                {
                    "id": "duplicate",
                    "source": {
                        "format": "rpm",
                        "distro": "fedora",
                        "release": {"exact": "44"},
                        "arch": {"exact": "x86_64"}
                    },
                    "target": {
                        "format": "rpm",
                        "distro": "fedora",
                        "release": {"exact": "44"},
                        "arch": {"exact": "x86_64"}
                    },
                    "requirements": {},
                    "digest": null,
                    "rationale": "first"
                },
                {
                    "id": "duplicate",
                    "source": {
                        "format": "rpm",
                        "distro": "fedora",
                        "release": {"exact": "44"},
                        "arch": {"exact": "x86_64"}
                    },
                    "target": {
                        "format": "rpm",
                        "distro": "fedora",
                        "release": {"exact": "44"},
                        "arch": {"exact": "x86_64"}
                    },
                    "requirements": {},
                    "digest": null,
                    "rationale": "second"
                }
            ]
        }"#;
        unsafe {
            std::env::set_var("CONARY_TEST_SKIP_GENERATION_MOUNT", "1");
            std::env::set_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON", duplicate_json);
        }

        let error = compatibility_matrix_for_process().expect_err("invalid matrix refuses");

        unsafe {
            std::env::remove_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON");
            std::env::remove_var("CONARY_TEST_SKIP_GENERATION_MOUNT");
        }
        assert!(
            error
                .to_string()
                .contains("validate CONARY_TEST_COMPATIBILITY_MATRIX_JSON")
        );
    }

    fn synthetic_matrix() -> TargetCompatibilityMatrix {
        TargetCompatibilityMatrix::for_testing(vec![TargetCompatibilityMatrixEntry {
            id: "test-fedora44-to-fedora44".to_string(),
            source: TargetSelector {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: TargetSelectorRelease::Exact("44".to_string()),
                arch: TargetSelectorArch::Exact("x86_64".to_string()),
            },
            target: TargetSelector {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: TargetSelectorRelease::Exact("44".to_string()),
                arch: TargetSelectorArch::Exact("x86_64".to_string()),
            },
            requirements: MatrixPreflightRequirements::default(),
            digest: Some("sha256:test".to_string()),
            rationale: "synthetic env fixture".to_string(),
        }])
    }
}
