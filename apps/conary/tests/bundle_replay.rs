// apps/conary/tests/bundle_replay.rs

mod common;

use common::legacy_scriptlet_fixtures::{
    LegacyBundleFixture, build_ccs_package_fixture, synthetic_legacy_bundle,
};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::legacy_scriptlets::{LifecyclePath, ScriptletDecision, ScriptletFidelity};
use conary_core::db;
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
fn ccs_install_dry_run_runs_legacy_bundle_admission_before_returning() {
    let fixture = InstallFixture::new(LegacyBundleFixture::ReviewEntry);
    let output = fixture.run_dry_run(&[]);

    assert_failure(&output);
    assert_contains(&output, "ReviewEntry");
    fixture.assert_no_install_mutation();
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
        let temp = tempfile::tempdir().expect("create test tempdir");
        let db_path = temp.path().join("conary.db");
        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).expect("create install root");
        db::init(&db_path).expect("initialize db");

        let (_package_temp, package_path) =
            build_ccs_package_fixture(case.package_name(), "1.0.0", synthetic_legacy_bundle(case))
                .expect("build CCS fixture");

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

fn is_safe_table_name(table: &str) -> bool {
    table
        .chars()
        .all(|character| character.is_ascii_lowercase() || character == '_')
}
