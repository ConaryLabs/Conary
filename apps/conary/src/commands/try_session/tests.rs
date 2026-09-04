// apps/conary/src/commands/try_session/tests.rs

use std::path::PathBuf;

use conary_core::db::models::TrySessionMode;

use super::install::{build_try_install_plan, build_try_transaction_config};
use super::test_support::*;

#[test]
#[should_panic(
    expected = "use composefs_ops::test_mount_skip_guard for the shared mount-skip environment"
)]
fn generic_env_guard_rejects_mount_skip_mutation_authority() {
    let _guard = EnvVarGuard::set(crate::test_hooks::names::SKIP_GENERATION_MOUNT, "1");
}

#[test]
fn try_transaction_config_override_keeps_live_runtime_paths_for_copied_db() {
    let fixture = TryRuntimeFixture::new();
    let work_dir = fixture.root.join("try/session-a");
    let copied_db = work_dir.join("conary.db");

    let config = build_try_transaction_config(&fixture.runtime_root(), copied_db.clone());

    assert_eq!(config.db_path, copied_db);
    assert_eq!(config.root, fixture.root);
    assert_eq!(config.objects_dir, fixture.root.join("objects"));
    assert_eq!(config.generations_dir, fixture.root.join("generations"));
    assert_eq!(config.etc_state_dir, fixture.root.join("etc-state"));
    assert_eq!(config.mount_point, fixture.root.join("mnt"));
}

#[test]
fn namespace_try_install_plan_uses_selected_root_and_shared_runtime() {
    let fixture = TryRuntimeFixture::new();
    let work_dir = fixture.root.join("try/session-a");
    let copied_db = work_dir.join("conary.db");

    let plan = build_try_install_plan(
        &fixture.runtime_root(),
        &work_dir,
        copied_db.clone(),
        TrySessionMode::Namespace,
    );

    assert_eq!(
        plan.install_root,
        work_dir.join("selected-root-session/root")
    );
    assert_ne!(plan.install_root, PathBuf::from("/"));
    assert_eq!(plan.runtime_root, fixture.runtime_root());
    assert_eq!(plan.copied_db_path, copied_db);
}
