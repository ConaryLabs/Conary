// apps/conary/tests/bundle_replay.rs

mod common;

use common::legacy_scriptlet_fixtures::{
    LegacyBundleFixture, build_ccs_package_fixture, synthetic_legacy_bundle,
};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::builder::{CcsBuilder, write_ccs_package};
use conary_core::ccs::legacy_scriptlets::{LifecyclePath, ScriptletDecision, ScriptletFidelity};
use conary_core::ccs::manifest::{CcsManifest, ScriptHook};
use conary_core::db;
use conary_core::db::models::{InstalledLegacyScriptletBundle, ScriptletEntry};
use conary_core::packages::PackageFormat;
use std::process::{Command, Output};

#[test]
fn synthetic_legacy_bundle_fixtures_cover_task5_matrix() {
    let cases = [
        LegacyBundleFixture::NoBundle,
        LegacyBundleFixture::NativeFree,
        LegacyBundleFixture::ReplacedOnly,
        LegacyBundleFixture::ReviewEntry,
        LegacyBundleFixture::BlockedEntry,
        LegacyBundleFixture::UnknownDecision,
        LegacyBundleFixture::SameSourceLegacyPostInstall,
        LegacyBundleFixture::FutureLegacyPostRemove,
        LegacyBundleFixture::FutureLegacyPreAndPostRemove,
        LegacyBundleFixture::UpgradeOldPreAndPostRemove,
        LegacyBundleFixture::UpgradeNewPreAndPost,
        LegacyBundleFixture::RawTriggerLegacy,
        LegacyBundleFixture::UnsupportedNativeInvocation,
    ];

    for case in cases {
        let bundle = synthetic_legacy_bundle(case);
        let (_temp, package_path) =
            build_ccs_package_fixture(case.package_name(), "1.0.0", bundle.clone())
                .expect("build CCS fixture");
        let parsed = CcsPackage::parse(package_path.to_str().expect("utf-8 package path"))
            .expect("parse CCS fixture");

        assert_eq!(
            parsed.manifest().legacy_scriptlets.is_some(),
            bundle.is_some(),
            "{case:?}"
        );

        if let Some(bundle) = bundle {
            bundle.validate().expect("fixture bundle validates");
            let decisions: Vec<_> = bundle
                .entries
                .iter()
                .map(|entry| entry.decision.clone())
                .collect();

            match case {
                LegacyBundleFixture::NativeFree => {
                    assert!(bundle.entries.is_empty());
                    assert_eq!(bundle.scriptlet_fidelity, ScriptletFidelity::NativeFree);
                }
                LegacyBundleFixture::ReplacedOnly => {
                    assert_eq!(decisions, vec![ScriptletDecision::Replaced]);
                }
                LegacyBundleFixture::ReviewEntry => {
                    assert_eq!(decisions, vec![ScriptletDecision::Review]);
                }
                LegacyBundleFixture::BlockedEntry => {
                    assert_eq!(decisions, vec![ScriptletDecision::Blocked]);
                }
                LegacyBundleFixture::UnknownDecision => {
                    assert!(matches!(
                        decisions.as_slice(),
                        [ScriptletDecision::Unknown(_)]
                    ));
                }
                LegacyBundleFixture::SameSourceLegacyPostInstall => {
                    assert_eq!(decisions, vec![ScriptletDecision::Legacy]);
                    assert_eq!(bundle.entries[0].phase, LifecyclePath::PostInstall);
                }
                LegacyBundleFixture::FutureLegacyPostRemove => {
                    assert_eq!(decisions, vec![ScriptletDecision::Legacy]);
                    assert_eq!(bundle.entries[0].phase, LifecyclePath::PostRemove);
                }
                LegacyBundleFixture::FutureLegacyPreAndPostRemove => {
                    assert_eq!(
                        decisions,
                        vec![ScriptletDecision::Legacy, ScriptletDecision::Legacy]
                    );
                    assert_eq!(bundle.entries[0].phase, LifecyclePath::PreRemove);
                    assert_eq!(bundle.entries[1].phase, LifecyclePath::PostRemove);
                }
                LegacyBundleFixture::UpgradeOldPreAndPostRemove => {
                    assert_eq!(
                        decisions,
                        vec![ScriptletDecision::Legacy, ScriptletDecision::Legacy]
                    );
                    assert_eq!(bundle.entries[0].phase, LifecyclePath::PreRemove);
                    assert_eq!(bundle.entries[1].phase, LifecyclePath::PostRemove);
                }
                LegacyBundleFixture::UpgradeNewPreAndPost => {
                    assert_eq!(
                        decisions,
                        vec![ScriptletDecision::Legacy, ScriptletDecision::Legacy]
                    );
                    assert_eq!(bundle.entries[0].phase, LifecyclePath::PreUpgrade);
                    assert_eq!(bundle.entries[1].phase, LifecyclePath::PostUpgrade);
                }
                LegacyBundleFixture::RawTriggerLegacy => {
                    assert_eq!(decisions, vec![ScriptletDecision::Legacy]);
                    assert_eq!(bundle.entries[0].phase, LifecyclePath::Trigger);
                    assert!(bundle.entries[0].rpm_trigger.is_some());
                }
                LegacyBundleFixture::UnsupportedNativeInvocation => {
                    assert_eq!(decisions, vec![ScriptletDecision::Legacy]);
                    assert!(bundle.entries[0].native_invocation.stdin.is_some());
                }
                LegacyBundleFixture::NoBundle => unreachable!("handled by None branch"),
            }
        }
    }
}

#[test]
fn ccs_install_refuses_unsafe_legacy_bundles_before_db_mutation() {
    let cases = [
        (LegacyBundleFixture::ReviewEntry, "ReviewEntry"),
        (LegacyBundleFixture::BlockedEntry, "BlockedEntry"),
        (LegacyBundleFixture::UnknownDecision, "UnknownDecision"),
        (
            LegacyBundleFixture::RawTriggerLegacy,
            "TriggerReplayUnsupported",
        ),
        (
            LegacyBundleFixture::SameSourceLegacyPostInstall,
            "LegacyReplayFeatureDisabled",
        ),
    ];

    for (case, expected_text) in cases {
        let fixture = InstallFixture::new(case);
        let output = fixture.run_install(&[]);

        assert_failure(&output);
        assert_contains(&output, expected_text);
        fixture.assert_no_install_mutation();
    }
}

#[test]
fn ccs_install_no_scripts_refuses_selected_legacy_replay_before_db_mutation() {
    let fixture = InstallFixture::new(LegacyBundleFixture::SameSourceLegacyPostInstall);
    let output = fixture.run_install(&["--allow-legacy-replay", "--no-scripts"]);

    assert_failure(&output);
    assert_contains(&output, "NoScriptsWouldSkipRequiredReplay");
    fixture.assert_no_install_mutation();
}

#[test]
fn ccs_install_no_bundle_with_no_scripts_keeps_existing_behavior() {
    let fixture = InstallFixture::new(LegacyBundleFixture::NoBundle);
    let output = fixture.run_install(&["--no-scripts"]);

    assert_success(&output);
    let conn = db::open(&fixture.db_path).expect("open db");
    assert_eq!(table_count(&conn, "troves"), 1);
    assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 0);
    let metadata = single_changeset_metadata(&conn);
    assert!(
        metadata.get("legacy_scriptlet_replay").is_none(),
        "no-bundle install should not add legacy replay audit metadata"
    );
}

#[test]
fn ccs_install_native_free_bundle_with_no_scripts_is_allowed_and_persisted() {
    let fixture = InstallFixture::new(LegacyBundleFixture::NativeFree);
    let output = fixture.run_install(&["--no-scripts"]);

    assert_success(&output);
    let conn = db::open(&fixture.db_path).expect("open db");
    let trove_id = single_trove_id(&conn);
    let installed = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
        .expect("load installed legacy bundle")
        .expect("native-free bundle should still be persisted");
    let decoded = installed.bundle().expect("decode installed bundle");
    assert!(decoded.entries.is_empty());
    assert_eq!(decoded.scriptlet_fidelity, ScriptletFidelity::NativeFree);
}

#[test]
fn ccs_install_replaced_only_bundle_with_no_scripts_suppresses_ccs_hooks() {
    let fixture = InstallFixture::new_replaced_post_install_with_hook("exit 42\n");
    let output = fixture.run_install(&["--no-scripts"]);

    assert_success(&output);
    assert!(
        !output_text(&output).contains("Post-install hooks failed"),
        "--no-scripts should suppress the failing CCS post-install hook"
    );
    let conn = db::open(&fixture.db_path).expect("open db");
    let trove_id = single_trove_id(&conn);
    let installed = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
        .expect("load installed legacy bundle")
        .expect("replaced-only bundle should be persisted");
    let decoded = installed.bundle().expect("decode installed bundle");
    assert_eq!(decoded.entries[0].decision, ScriptletDecision::Replaced);
}

#[test]
fn ccs_install_no_scripts_persists_future_legacy_bundle_for_later_lifecycle() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPostRemove);
    let output = fixture.run_install(&["--no-scripts"]);

    assert_success(&output);
    assert!(!output_text(&output).contains("NoScriptsWouldSkipRequiredReplay"));

    let conn = db::open(&fixture.db_path).expect("open db");
    let trove_id = single_trove_id(&conn);
    let installed = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
        .expect("load installed legacy bundle")
        .expect("future-lifecycle bundle should be persisted");
    let decoded = installed.bundle().expect("decode installed bundle");
    assert_eq!(decoded.entries.len(), 1);
    assert_eq!(decoded.entries[0].phase, LifecyclePath::PostRemove);
    assert_eq!(decoded.entries[0].decision, ScriptletDecision::Legacy);
}

#[test]
fn ccs_install_allowed_legacy_pre_entry_rejects_unsupported_native_contract_before_db_mutation() {
    let fixture = InstallFixture::new(LegacyBundleFixture::UnsupportedNativeInvocation);
    let output = fixture.run_install(&["--allow-legacy-replay"]);

    assert_failure(&output);
    assert_contains(&output, "NativeArgsContractUnsupported");
    fixture.assert_no_install_mutation();
}

#[test]
fn ccs_install_dry_run_runs_legacy_bundle_admission_before_returning() {
    let fixture = InstallFixture::new(LegacyBundleFixture::ReviewEntry);
    let output = fixture.run_dry_run(&[]);

    assert_failure(&output);
    assert_contains(&output, "ReviewEntry");
    fixture.assert_no_install_mutation();
}

#[test]
fn ccs_install_persists_future_legacy_bundle_for_later_lifecycle() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPostRemove);
    let output = fixture.run_install(&[]);

    assert_success(&output);
    assert!(!output_text(&output).contains("replay-post-remove"));

    let conn = db::open(&fixture.db_path).expect("open db");
    let trove_id = single_trove_id(&conn);
    let installed = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
        .expect("load installed legacy bundle")
        .expect("installed bundle row exists");

    assert_eq!(installed.source_package, "legacy-fixture-remove");
    assert_eq!(installed.source_version, "1.0.0-1.fc44");
    assert_eq!(installed.target_id, "rpm/fedora/44/x86_64");
    assert_eq!(installed.target_compatibility, "source-native");
    assert_eq!(installed.foreign_replay_policy, "deny");
    assert_eq!(installed.scriptlet_fidelity, "legacy-replay");
    assert_eq!(installed.publication_status, "public");
    assert_eq!(installed.replay_policy, "goal6-safe-replay");
    assert!(!installed.replay_enabled);
    assert_eq!(installed.installed_changeset_id, Some(1));

    let decoded = installed.bundle().expect("decode installed bundle");
    assert_eq!(decoded.entries.len(), 1);
    assert_eq!(decoded.entries[0].phase, LifecyclePath::PostRemove);
    assert_eq!(decoded.entries[0].decision, ScriptletDecision::Legacy);
}

#[test]
fn ccs_install_dry_run_with_accepted_future_bundle_persists_no_installed_bundle() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPostRemove);
    let output = fixture.run_dry_run(&[]);

    assert_success(&output);
    fixture.assert_no_install_mutation();
}

#[test]
fn ccs_install_with_legacy_bundle_does_not_persist_flattened_pre_remove_hook() {
    let fixture = InstallFixture::new_replaced_pre_remove_with_hook();
    let output = fixture.run_install(&[]);

    assert_success(&output);

    let conn = db::open(&fixture.db_path).expect("open db");
    let trove_id = single_trove_id(&conn);
    let installed = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
        .expect("load installed legacy bundle");
    assert!(installed.is_some(), "installed bundle row should exist");

    let scriptlets =
        ScriptletEntry::find_by_trove(&conn, trove_id).expect("load flattened scriptlet entries");
    assert!(
        scriptlets.is_empty(),
        "bundle-covered CCS hooks must not be flattened into scriptlets: {scriptlets:?}"
    );
}

#[test]
fn ccs_install_with_future_legacy_bundle_records_changeset_audit_metadata() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPostRemove);
    let output = fixture.run_install(&[]);

    assert_success(&output);

    let conn = db::open(&fixture.db_path).expect("open db");
    let metadata = single_changeset_metadata(&conn);
    let audit = metadata
        .get("legacy_scriptlet_replay")
        .expect("legacy replay audit metadata");

    assert_eq!(metadata["schema"], "conary.changeset.metadata.v1");
    assert_eq!(audit["bundle_present"], true);
    assert_eq!(audit["target_id"], "rpm/fedora/44/x86_64");
    assert_eq!(audit["source_target_id"], "rpm/fedora/44/x86_64");
    assert_eq!(audit["target_compatibility"], "source-native");
    assert_eq!(audit["foreign_replay_policy"], "deny");
    assert_eq!(audit["host_policy"], "strict");
    assert_eq!(audit["feature_gate"], "disabled");
    assert_eq!(audit["foreign_override"], false);
    assert_eq!(
        audit["evidence_digest"],
        conary_core::hash::sha256_prefixed(b"legacy-fixture-remove-evidence")
    );

    let planned_entries = audit["planned_entries"]
        .as_array()
        .expect("planned entries array");
    assert!(
        planned_entries.is_empty(),
        "future lifecycle entries should be preserved in the installed bundle but not planned for fresh install"
    );
}

#[test]
fn remove_refuses_installed_legacy_bundle_without_replay_flag_before_mutation() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPostRemove);
    let output = fixture.run_install(&[]);
    assert_success(&output);

    let output = fixture.run_remove("legacy-fixture-remove", &[]);

    assert_failure(&output);
    assert_contains(&output, "LegacyReplayFeatureDisabled");
    let conn = db::open(&fixture.db_path).expect("open db");
    assert_eq!(table_count(&conn, "changesets"), 1);
    assert_eq!(table_count(&conn, "troves"), 1);
    assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 1);
}

#[test]
fn remove_no_scripts_refuses_required_installed_replay_before_mutation() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPostRemove);
    let output = fixture.run_install(&[]);
    assert_success(&output);

    let output = fixture.run_remove(
        "legacy-fixture-remove",
        &["--allow-legacy-replay", "--no-scripts"],
    );

    assert_failure(&output);
    assert_contains(&output, "NoScriptsWouldSkipRequiredReplay");
    let conn = db::open(&fixture.db_path).expect("open db");
    assert_eq!(table_count(&conn, "changesets"), 1);
    assert_eq!(table_count(&conn, "troves"), 1);
    assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 1);
}

#[test]
fn remove_with_replay_flag_executes_post_remove_plan_from_memory() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPostRemove);
    let output = fixture.run_install(&[]);
    assert_success(&output);

    let output = fixture.run_remove("legacy-fixture-remove", &["--allow-legacy-replay"]);

    assert_success(&output);
    let conn = db::open(&fixture.db_path).expect("open db");
    assert_eq!(table_count(&conn, "changesets"), 2);
    assert_eq!(table_count(&conn, "troves"), 0);
    assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 0);

    let metadata = changeset_metadata(&conn, 2);
    let audit = metadata
        .get("legacy_scriptlet_replay")
        .expect("remove changeset has legacy replay audit");
    let planned_entries = audit["planned_entries"]
        .as_array()
        .expect("planned entries array");
    assert_eq!(planned_entries.len(), 1);
    assert_eq!(planned_entries[0]["entry_id"], "rpm:%postun");
    assert_eq!(planned_entries[0]["phase"], "post-remove");
    assert_eq!(planned_entries[0]["raw_replay_required"], true);
    assert_eq!(planned_entries[0]["outcome"]["phase"], "post-remove");
    assert_eq!(planned_entries[0]["outcome"]["status"], "skipped");
}

#[test]
fn remove_with_replay_flag_does_not_need_original_ccs_archive() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPostRemove);
    let output = fixture.run_install(&[]);
    assert_success(&output);
    fixture.delete_package_archive();

    let output = fixture.run_remove("legacy-fixture-remove", &["--allow-legacy-replay"]);

    assert_success(&output);
    let conn = db::open(&fixture.db_path).expect("open db");
    assert_eq!(table_count(&conn, "troves"), 0);
    assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 0);
}

#[test]
fn remove_replays_pre_and_post_remove_plans_once_in_audit() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPreAndPostRemove);
    let output = fixture.run_install(&[]);
    assert_success(&output);

    let output = fixture.run_remove("legacy-fixture-remove-both", &["--allow-legacy-replay"]);

    assert_success(&output);
    let conn = db::open(&fixture.db_path).expect("open db");
    let metadata = changeset_metadata(&conn, 2);
    let planned_entries = metadata["legacy_scriptlet_replay"]["planned_entries"]
        .as_array()
        .expect("planned entries array");
    assert_eq!(planned_entries.len(), 2);
    assert_eq!(planned_entries[0]["entry_id"], "rpm:%preun");
    assert_eq!(planned_entries[0]["phase"], "pre-remove");
    assert_eq!(planned_entries[0]["outcome"]["phase"], "pre-remove");
    assert_eq!(planned_entries[0]["outcome"]["status"], "skipped");
    assert_eq!(planned_entries[1]["entry_id"], "rpm:%postun");
    assert_eq!(planned_entries[1]["phase"], "post-remove");
    assert_eq!(planned_entries[1]["outcome"]["phase"], "post-remove");
    assert_eq!(planned_entries[1]["outcome"]["status"], "skipped");
}

#[test]
fn remove_malformed_installed_bundle_fails_before_mutation() {
    let fixture = InstallFixture::new(LegacyBundleFixture::FutureLegacyPostRemove);
    let output = fixture.run_install(&[]);
    assert_success(&output);
    fixture.corrupt_installed_bundle_toml();

    let output = fixture.run_remove("legacy-fixture-remove", &["--allow-legacy-replay"]);

    assert_failure(&output);
    assert_contains(&output, "installed legacy scriptlet bundle is malformed");
    let conn = db::open(&fixture.db_path).expect("open db");
    assert_eq!(table_count(&conn, "changesets"), 1);
    assert_eq!(table_count(&conn, "troves"), 1);
    assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 1);
}

#[test]
fn upgrade_refuses_old_installed_legacy_bundle_without_flag_before_mutation() {
    let fixture = UpgradeFixture::new(
        LegacyBundleFixture::UpgradeOldPreAndPostRemove,
        LegacyBundleFixture::NativeFree,
    );
    let output = fixture.run_old_install(&[]);
    assert_success(&output);

    let output = fixture.run_upgrade(&[]);

    assert_failure(&output);
    assert_contains(&output, "LegacyReplayFeatureDisabled");
    let conn = db::open(&fixture.db_path).expect("open db");
    assert_eq!(table_count(&conn, "changesets"), 1);
    assert_eq!(table_count(&conn, "troves"), 1);
    assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 1);
    assert_eq!(installed_versions(&conn), vec!["1.0.0".to_string()]);
}

#[test]
fn upgrade_replays_old_and_new_legacy_plans_and_replaces_installed_bundle() {
    let fixture = UpgradeFixture::new(
        LegacyBundleFixture::UpgradeOldPreAndPostRemove,
        LegacyBundleFixture::UpgradeNewPreAndPost,
    );
    let output = fixture.run_old_install(&[]);
    assert_success(&output);

    let output = fixture.run_upgrade(&["--allow-legacy-replay"]);

    assert_success(&output);
    let conn = db::open(&fixture.db_path).expect("open db");
    assert_eq!(table_count(&conn, "changesets"), 2);
    assert_eq!(table_count(&conn, "troves"), 1);
    assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 1);
    assert_eq!(installed_versions(&conn), vec!["2.0.0".to_string()]);

    let trove_id = single_trove_id(&conn);
    let installed = InstalledLegacyScriptletBundle::find_by_trove(&conn, trove_id)
        .expect("load installed legacy bundle")
        .expect("new installed bundle row exists");
    let decoded = installed.bundle().expect("decode installed bundle");
    assert_eq!(decoded.entries[0].phase, LifecyclePath::PreUpgrade);
    assert_eq!(decoded.entries[1].phase, LifecyclePath::PostUpgrade);

    let metadata = changeset_metadata(&conn, 2);
    let planned_entries = metadata["legacy_scriptlet_replay"]["planned_entries"]
        .as_array()
        .expect("planned entries array");
    let phases: Vec<_> = planned_entries
        .iter()
        .map(|entry| {
            (
                entry["entry_id"].as_str().expect("entry id"),
                entry["phase"].as_str().expect("phase"),
                entry["outcome"]["status"].as_str().expect("outcome status"),
            )
        })
        .collect();
    assert_eq!(
        phases,
        vec![
            ("rpm:%preun", "pre-remove", "skipped"),
            ("rpm:%pre", "pre-upgrade", "skipped"),
            ("rpm:%postun", "post-remove", "skipped"),
            ("rpm:%post", "post-upgrade", "skipped"),
        ]
    );
}

#[test]
fn upgrade_transaction_failure_preserves_old_installed_bundle() {
    let fixture = UpgradeFixture::new(
        LegacyBundleFixture::UpgradeOldPreAndPostRemove,
        LegacyBundleFixture::UpgradeNewPreAndPost,
    );
    let output = fixture.run_old_install(&[]);
    assert_success(&output);
    fixture.force_upgrade_transaction_failure();

    let output = fixture.run_upgrade(&["--allow-legacy-replay"]);

    assert_failure(&output);
    assert_contains(&output, "forced upgrade transaction failure");
    let conn = db::open(&fixture.db_path).expect("open db");
    assert_eq!(table_count(&conn, "changesets"), 1);
    assert_eq!(table_count(&conn, "troves"), 1);
    assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 1);
    assert_eq!(installed_versions(&conn), vec!["1.0.0".to_string()]);
}

struct InstallFixture {
    _temp: tempfile::TempDir,
    _package_temp: tempfile::TempDir,
    package_path: std::path::PathBuf,
    db_path: std::path::PathBuf,
    root: std::path::PathBuf,
}

impl InstallFixture {
    fn new(case: LegacyBundleFixture) -> Self {
        let (_package_temp, package_path) =
            build_ccs_package_fixture(case.package_name(), "1.0.0", synthetic_legacy_bundle(case))
                .expect("build CCS fixture");
        Self::from_package(_package_temp, package_path)
    }

    fn new_replaced_pre_remove_with_hook() -> Self {
        let mut bundle =
            synthetic_legacy_bundle(LegacyBundleFixture::ReplacedOnly).expect("replaced bundle");
        bundle.source_package = "legacy-fixture-replaced-remove".to_string();
        if let Some(entry) = bundle.entries.first_mut() {
            entry.id = "rpm:%preun".to_string();
            entry.native_slot = "%preun".to_string();
            entry.phase = LifecyclePath::PreRemove;
            entry.lifecycle_paths = vec!["remove:pre".to_string()];
            entry.transaction_order.position = "before-payload".to_string();
        }
        bundle.validate().expect("mutated fixture bundle validates");

        let (_package_temp, package_path) = build_ccs_package_fixture_with_pre_remove_hook(
            "legacy-fixture-replaced-remove",
            "1.0.0",
            Some(bundle),
            "echo replaced pre-remove hook\n",
        )
        .expect("build CCS fixture with pre-remove hook");
        Self::from_package(_package_temp, package_path)
    }

    fn new_replaced_post_install_with_hook(post_install_script: &str) -> Self {
        let mut bundle =
            synthetic_legacy_bundle(LegacyBundleFixture::ReplacedOnly).expect("replaced bundle");
        if let Some(entry) = bundle.entries.first_mut() {
            entry.id = "rpm:%post".to_string();
            entry.native_slot = "%post".to_string();
            entry.phase = LifecyclePath::PostInstall;
            entry.lifecycle_paths = vec!["install:post".to_string()];
            entry.transaction_order.position = "after-payload".to_string();
        }
        bundle.validate().expect("mutated fixture bundle validates");

        let (_package_temp, package_path) = build_ccs_package_fixture_with_post_install_hook(
            LegacyBundleFixture::ReplacedOnly.package_name(),
            "1.0.0",
            Some(bundle),
            post_install_script,
        )
        .expect("build CCS fixture with post-install hook");
        Self::from_package(_package_temp, package_path)
    }

    fn from_package(_package_temp: tempfile::TempDir, package_path: std::path::PathBuf) -> Self {
        let temp = tempfile::tempdir().expect("create test tempdir");
        let db_path = temp.path().join("conary.db");
        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).expect("create install root");
        db::init(&db_path).expect("initialize db");

        Self {
            _temp: temp,
            _package_temp,
            package_path,
            db_path,
            root,
        }
    }

    fn run_install(&self, extra_args: &[&str]) -> Output {
        let mut args = vec![
            "--allow-live-system-mutation",
            "ccs",
            "install",
            self.package_path.to_str().expect("utf-8 package path"),
            "--allow-unsigned",
            "--sandbox",
            "never",
            "--db-path",
            self.db_path.to_str().expect("utf-8 db path"),
            "--root",
            self.root.to_str().expect("utf-8 root path"),
        ];
        args.extend_from_slice(extra_args);
        run_conary(&args)
    }

    fn run_dry_run(&self, extra_args: &[&str]) -> Output {
        let mut args = vec![
            "ccs",
            "install",
            self.package_path.to_str().expect("utf-8 package path"),
            "--dry-run",
            "--allow-unsigned",
            "--sandbox",
            "never",
            "--db-path",
            self.db_path.to_str().expect("utf-8 db path"),
            "--root",
            self.root.to_str().expect("utf-8 root path"),
        ];
        args.extend_from_slice(extra_args);
        run_conary(&args)
    }

    fn run_remove(&self, package_name: &str, extra_args: &[&str]) -> Output {
        let mut args = vec![
            "--allow-live-system-mutation",
            "remove",
            package_name,
            "--sandbox",
            "never",
            "--db-path",
            self.db_path.to_str().expect("utf-8 db path"),
            "--root",
            self.root.to_str().expect("utf-8 root path"),
        ];
        args.extend_from_slice(extra_args);
        run_conary(&args)
    }

    fn delete_package_archive(&self) {
        std::fs::remove_file(&self.package_path).expect("remove package archive");
    }

    fn corrupt_installed_bundle_toml(&self) {
        let conn = db::open(&self.db_path).expect("open db");
        conn.execute(
            "UPDATE installed_legacy_scriptlet_bundles SET bundle_toml = ?1",
            ["this is not valid = ["],
        )
        .expect("corrupt bundle toml");
    }

    fn assert_no_install_mutation(&self) {
        let conn = db::open(&self.db_path).expect("open db");
        for table in [
            "troves",
            "changesets",
            "files",
            "scriptlets",
            "installed_legacy_scriptlet_bundles",
        ] {
            assert_eq!(
                table_count(&conn, table),
                0,
                "expected no rows in {table} after refused install"
            );
        }
    }
}

struct UpgradeFixture {
    _temp: tempfile::TempDir,
    _old_package_temp: tempfile::TempDir,
    _new_package_temp: tempfile::TempDir,
    old_package_path: std::path::PathBuf,
    new_package_path: std::path::PathBuf,
    db_path: std::path::PathBuf,
    root: std::path::PathBuf,
}

impl UpgradeFixture {
    fn new(old_case: LegacyBundleFixture, new_case: LegacyBundleFixture) -> Self {
        let package_name = old_case.package_name();
        let (_old_package_temp, old_package_path) =
            build_ccs_package_fixture(package_name, "1.0.0", synthetic_legacy_bundle(old_case))
                .expect("build old CCS fixture");
        let (_new_package_temp, new_package_path) =
            build_ccs_package_fixture(package_name, "2.0.0", synthetic_legacy_bundle(new_case))
                .expect("build new CCS fixture");
        let temp = tempfile::tempdir().expect("create test tempdir");
        let db_path = temp.path().join("conary.db");
        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).expect("create install root");
        db::init(&db_path).expect("initialize db");

        Self {
            _temp: temp,
            _old_package_temp,
            _new_package_temp,
            old_package_path,
            new_package_path,
            db_path,
            root,
        }
    }

    fn run_old_install(&self, extra_args: &[&str]) -> Output {
        self.run_install_path(&self.old_package_path, extra_args)
    }

    fn run_upgrade(&self, extra_args: &[&str]) -> Output {
        self.run_install_path(&self.new_package_path, extra_args)
    }

    fn run_install_path(&self, package_path: &std::path::Path, extra_args: &[&str]) -> Output {
        let mut args = vec![
            "--allow-live-system-mutation",
            "ccs",
            "install",
            package_path.to_str().expect("utf-8 package path"),
            "--allow-unsigned",
            "--sandbox",
            "never",
            "--db-path",
            self.db_path.to_str().expect("utf-8 db path"),
            "--root",
            self.root.to_str().expect("utf-8 root path"),
        ];
        args.extend_from_slice(extra_args);
        run_conary(&args)
    }

    fn force_upgrade_transaction_failure(&self) {
        let conn = db::open(&self.db_path).expect("open db");
        conn.execute_batch(
            "CREATE TRIGGER fail_upgrade_file_history
             BEFORE INSERT ON file_history
             WHEN NEW.action = 'modify'
             BEGIN
                 SELECT RAISE(FAIL, 'forced upgrade transaction failure');
             END;",
        )
        .expect("install upgrade failure trigger");
    }
}

fn build_ccs_package_fixture_with_pre_remove_hook(
    name: &str,
    version: &str,
    bundle: Option<conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle>,
    pre_remove_script: &str,
) -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let temp = tempfile::tempdir()?;
    let source_dir = temp.path().join("src");
    std::fs::create_dir_all(source_dir.join("usr/bin"))?;
    std::fs::write(source_dir.join("usr/bin/fixture"), b"fixture\n")?;

    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.legacy_scriptlets = bundle;
    manifest.hooks.pre_remove = Some(ScriptHook {
        script: pre_remove_script.to_string(),
    });

    let result = CcsBuilder::new(manifest, &source_dir).build()?;
    let package_path = temp.path().join(format!("{name}.ccs"));
    write_ccs_package(&result, &package_path)?;
    Ok((temp, package_path))
}

fn build_ccs_package_fixture_with_post_install_hook(
    name: &str,
    version: &str,
    bundle: Option<conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle>,
    post_install_script: &str,
) -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let temp = tempfile::tempdir()?;
    let source_dir = temp.path().join("src");
    std::fs::create_dir_all(source_dir.join("usr/bin"))?;
    std::fs::write(source_dir.join("usr/bin/fixture"), b"fixture\n")?;

    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.legacy_scriptlets = bundle;
    manifest.hooks.post_install = Some(ScriptHook {
        script: post_install_script.to_string(),
    });

    let result = CcsBuilder::new(manifest, &source_dir).build()?;
    let package_path = temp.path().join(format!("{name}.ccs"));
    write_ccs_package(&result, &package_path)?;
    Ok((temp, package_path))
}

fn run_conary(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .args(args)
        .output()
        .expect("run conary")
}

fn assert_failure(output: &Output) {
    assert!(
        !output.status.success(),
        "expected failure, got success\n{}",
        output_text(output)
    );
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success, got failure\n{}",
        output_text(output)
    );
}

fn assert_contains(output: &Output, expected: &str) {
    let text = output_text(output);
    assert!(
        text.contains(expected),
        "expected output to contain {expected:?}\n{text}"
    );
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn table_count(conn: &rusqlite::Connection, table: &str) -> i64 {
    assert!(is_safe_table_name(table));
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .expect("count table rows")
}

fn single_trove_id(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT id FROM troves", [], |row| row.get(0))
        .expect("single installed trove")
}

fn installed_versions(conn: &rusqlite::Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT version FROM troves ORDER BY version")
        .expect("prepare installed versions query");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query installed versions")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect installed versions")
}

fn single_changeset_metadata(conn: &rusqlite::Connection) -> serde_json::Value {
    changeset_metadata(conn, 1)
}

fn changeset_metadata(conn: &rusqlite::Connection, changeset_id: i64) -> serde_json::Value {
    let raw: Option<String> = conn
        .query_row(
            "SELECT metadata FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .expect("changeset metadata");
    let raw = raw.expect("changeset metadata should be present");
    serde_json::from_str(&raw).expect("changeset metadata is JSON")
}

fn is_safe_table_name(table: &str) -> bool {
    table
        .chars()
        .all(|character| character.is_ascii_lowercase() || character == '_')
}
