// conary-test/src/config/tests/native_corpus.rs

use super::{conary_fixture_path, load_manifest, remi_manifest_path};

#[test]
fn phase4_native_pm_parity_manifest_carries_cross_source_and_daily_driver_contract() {
    let path = remi_manifest_path("phase4-native-pm-parity.toml");
    if !path.exists() {
        return;
    }

    let manifest = load_manifest(&path).unwrap();
    let required_coverage = &manifest
        .suite
        .corpus
        .as_ref()
        .expect("daily-driver semantic coverage authority")
        .required;
    assert_eq!(
        required_coverage,
        &[
            conary_core::corpus::CorpusSemantic::IdentityExactVersion,
            conary_core::corpus::CorpusSemantic::IdentityNativeArchitecture,
            conary_core::corpus::CorpusSemantic::PayloadFiles,
            conary_core::corpus::CorpusSemantic::RelationsVirtualProvide,
            conary_core::corpus::CorpusSemantic::ConfigurationMatchedConfig,
            conary_core::corpus::CorpusSemantic::ConfigurationRemoval,
            conary_core::corpus::CorpusSemantic::LifecycleShell,
        ]
    );
    for unproven in [
        conary_core::corpus::CorpusSemantic::PayloadLargeFiles,
        conary_core::corpus::CorpusSemantic::RelationsConflict,
        conary_core::corpus::CorpusSemantic::LifecycleTrigger,
        conary_core::corpus::CorpusSemantic::RuntimeServiceActivation,
        conary_core::corpus::CorpusSemantic::RuntimeTargetHelper,
    ] {
        assert!(
            !required_coverage.contains(&unproven),
            "daily-driver fixture must not overclaim {unproven:?}"
        );
    }
    let cross_source = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM02X")
        .expect("Phase 4 native PM parity must include the cross-source-format matrix");
    let cross_source_rendered = format!("{cross_source:?}");
    for required in [
        "run-cross-source-lifecycle-matrix.sh",
        "install",
        "rollback",
        "remove",
    ] {
        assert!(
            cross_source_rendered.contains(required),
            "cross-source-format matrix should enforce {required}"
        );
    }
    let matrix_script = std::fs::read_to_string(conary_fixture_path(
        "native/run-cross-source-lifecycle-matrix.sh",
    ))
    .expect("read cross-source lifecycle helper");
    for required in [
        "all_source_formats=(rpm deb arch)",
        "for source_format in \"${source_formats[@]}\"",
        "capture-native-lifecycle-oracle.sh",
        "native-lifecycle-parity",
        "source_profile=\"fedora-44\"",
        "source_profile=\"ubuntu-26.04\"",
        "source_profile=\"arch\"",
        "--from \"${source_profile}\"",
        "expected_trace_digest",
        "assert_trace",
        "forbid-native-pm",
        "native-manager-backups",
        "restore_native_managers",
        "rpmdb",
        "dpkg",
        "pacman",
        "system state rollback",
        "--purge",
        "assert-selected-generation.py",
    ] {
        assert!(
            matrix_script.contains(required),
            "cross-source lifecycle helper should enforce {required}"
        );
    }
    let oracle_script = std::fs::read_to_string(conary_fixture_path(
        "native/capture-native-lifecycle-oracle.sh",
    ))
    .expect("read native lifecycle oracle helper");
    for required in [
        "rpm --install",
        "rpm --upgrade",
        "dpkg --install",
        "pacman --upgrade",
        "capture_and_compare install",
        "capture_and_compare upgrade",
        "capture_and_compare remove",
        "cmp --silent",
        "diff --unified",
    ] {
        assert!(
            oracle_script.contains(required),
            "native lifecycle oracle should enforce {required}"
        );
    }
    for source_format in ["rpm", "deb", "arch"] {
        for operation in ["install", "upgrade", "remove"] {
            let trace_path =
                format!("native-lifecycle-parity/expected/{source_format}/{operation}.trace");
            let trace = std::fs::read_to_string(conary_fixture_path(&trace_path))
                .unwrap_or_else(|error| panic!("read {trace_path}: {error}"));
            assert!(!trace.is_empty(), "{trace_path} must not be empty");
            assert!(
                trace.lines().all(|line| {
                    line.contains("|argc=")
                        && line.contains("|argv=")
                        && line.contains("|stdin=")
                        && line.contains("|payload=")
                }),
                "{trace_path} must contain only normalized lifecycle records"
            );
        }
    }

    let corpus_tests: Vec<_> = manifest
        .test
        .iter()
        .filter(|test| test.group.as_deref() == Some("daily-driver-corpus"))
        .collect();

    assert!(
        corpus_tests.len() >= 5,
        "Phase 4 native PM parity should include a daily-driver corpus group"
    );
    assert!(
        corpus_tests.iter().all(|test| test.skip.is_none()),
        "daily-driver corpus tests must not be manifest-skipped"
    );
    assert!(
        corpus_tests.iter().all(|test| test.flaky != Some(true)),
        "daily-driver corpus tests must not rely on flaky majority voting"
    );

    for test_id in ["TNPM04", "TNPM15"] {
        let metadata_test = manifest
            .test
            .iter()
            .find(|test| test.id == test_id)
            .unwrap_or_else(|| panic!("missing {test_id} native metadata test"));
        let metadata_rendered = format!("{metadata_test:?}");
        assert!(
            metadata_rendered.contains("regular files")
                && metadata_rendered
                    .contains("json_extract(payload_node_json, '$.source.kind.type') = 'regular'",),
            "{test_id} must distinguish typed regular files from directory payload rows"
        );
    }

    let rendered = corpus_tests
        .iter()
        .map(|test| format!("{test:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    for required in [
        "phase4-corpus.service",
        "/etc/phase4-corpus/app.conf",
        "phase4-corpus-alt",
        "phase4-corpus-user",
        "phase4-corpus-group",
        "${native_corpus_dependency_probe}",
        "/var/lib/phase4-corpus/scriptlet.marker",
        "/var/lib/phase4-corpus/remove.marker",
        "phase4-corpus-conflict",
        "large-payload.bin",
        "/usr/lib/kernel/install.d/95-phase4-corpus.install",
        "query whatprovides phase4-corpus-tool",
        "0 config rows",
    ] {
        assert!(
            rendered.contains(required),
            "daily-driver corpus should cover {required}"
        );
    }

    for distro in ["fedora44", "ubuntu-26.04", "arch"] {
        let overrides = manifest
            .distro_overrides
            .get(distro)
            .unwrap_or_else(|| panic!("missing {distro} overrides"));
        for key in [
            "native_corpus_fixture_version",
            "native_corpus_dependency_count",
            "native_corpus_dependency_probe",
            "native_corpus_lifecycle_fidelity",
            "native_corpus_source_format",
            "native_corpus_target_architecture",
        ] {
            assert!(
                overrides.contains_key(key),
                "{distro} overrides should define {key}"
            );
        }
    }

    let install_case = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM16")
        .and_then(|test| test.corpus.as_ref())
        .expect("TNPM16 typed install evidence");
    assert_eq!(install_case.source_profile, "${native_profile}");
    assert_eq!(
        install_case.source_format.as_str(),
        "${native_corpus_source_format}"
    );
    assert_eq!(
        install_case.stages,
        [conary_core::corpus::ConversionStage::Installation]
    );
    assert_eq!(install_case.coverage.len(), 6);
    assert!(
        install_case
            .coverage
            .iter()
            .all(|claim| claim.artifact_roles
                == [conary_core::corpus::SourceArtifactRole::InstallRequest])
    );

    let removal_case = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM18")
        .and_then(|test| test.corpus.as_ref())
        .expect("TNPM18 typed removal evidence");
    assert_eq!(
        removal_case.stages,
        [conary_core::corpus::ConversionStage::Removal]
    );
    assert_eq!(removal_case.coverage.len(), 1);
    assert_eq!(
        removal_case.coverage[0].semantic,
        conary_core::corpus::CorpusSemantic::ConfigurationRemoval
    );

    let build_helper =
        std::fs::read_to_string(conary_fixture_path("native/build-native-fixtures.sh"))
            .expect("read native fixture builder");
    assert!(build_helper.contains("native-fixture-manifest.json"));
    assert!(build_helper.contains("\"schema_version\": 1"));

    let evidence_helper =
        std::fs::read_to_string(conary_fixture_path("native/write-corpus-evidence.py"))
            .expect("read native corpus evidence writer");
    for required in [
        "fixture_manifest",
        "schema_version",
        "sha256(artifact_path)",
        "fixture_build_manifest",
        "os.replace",
    ] {
        assert!(
            evidence_helper.contains(required),
            "native corpus evidence writer should enforce {required}"
        );
    }
}

#[test]
fn focused_native_cross_source_manifest_runs_the_shared_lifecycle_contract() {
    let path = remi_manifest_path("native-cross-source-lifecycle.toml");
    if !path.exists() {
        return;
    }

    let manifest = load_manifest(&path).expect("load focused native lifecycle manifest");
    assert_eq!(manifest.suite.phase, 4);
    assert_eq!(manifest.test.len(), 4);
    assert_eq!(
        manifest
            .suite
            .corpus
            .as_ref()
            .expect("suite coverage authority")
            .required,
        [
            conary_core::corpus::CorpusSemantic::IdentityExactVersion,
            conary_core::corpus::CorpusSemantic::IdentityNativeArchitecture,
            conary_core::corpus::CorpusSemantic::PayloadFiles,
            conary_core::corpus::CorpusSemantic::LifecycleShell,
        ]
    );
    for (test, expected_id, expected_profile, expected_format) in [
        (&manifest.test[0], "TNPMX01R", "fedora-44", "rpm"),
        (&manifest.test[1], "TNPMX01D", "ubuntu-26.04", "deb"),
        (&manifest.test[2], "TNPMX01A", "arch", "alpm"),
    ] {
        assert_eq!(test.id, expected_id);
        assert_eq!(test.fatal, Some(false));
        assert_eq!(test.skip, None);
        assert_eq!(test.flaky, None);

        let corpus = test.corpus.as_ref().expect("typed corpus declaration");
        assert_eq!(corpus.source_profile, expected_profile);
        assert_eq!(corpus.source_format.as_str(), expected_format);
        assert_eq!(
            corpus.digest_source,
            conary_core::corpus::SourceArtifactDigestSource::FixtureBuildManifest
        );
        assert_eq!(corpus.stages.len(), 4);
        assert_eq!(corpus.coverage.len(), 4);
        assert!(corpus.coverage.iter().all(|claim| claim.artifact_roles
            == [
                conary_core::corpus::SourceArtifactRole::InstallRequest,
                conary_core::corpus::SourceArtifactRole::UpdateRequest,
            ]));

        let rendered = format!("{test:?}");
        assert!(
            rendered.contains("run-cross-source-lifecycle-matrix.sh"),
            "focused manifest must execute the shared lifecycle contract"
        );
        assert!(
            rendered.contains("${native_lifecycle_oracle_format}"),
            "focused manifest must pass an explicit typed native-oracle format"
        );
        for operation in ["install", "update", "rollback", "remove"] {
            assert!(
                test.description.contains(operation),
                "focused manifest should name {operation} in its contract"
            );
        }
    }
    let openrc = &manifest.test[3];
    assert_eq!(openrc.id, "TNPMX02O");
    assert_eq!(openrc.fatal, Some(false));
    assert!(openrc.corpus.is_none());
    assert!(
        format!("{openrc:?}").contains("run-openrc-service-lifecycle.sh ${target_init_system}")
    );
    for (distro, expected_format, expected_init) in [
        ("fedora44", "rpm", "systemd"),
        ("ubuntu-26.04", "deb", "systemd"),
        ("arch", "arch", "systemd"),
        ("artix", "arch", "openrc"),
        ("linux-mint-22.3", "deb", "systemd"),
        ("pop-os-24.04", "deb", "systemd"),
    ] {
        let overrides = manifest
            .distro_overrides
            .get(distro)
            .unwrap_or_else(|| panic!("missing {distro} overrides"));
        assert_eq!(
            overrides
                .get("native_lifecycle_oracle_format")
                .map(String::as_str),
            Some(expected_format),
            "{distro} must select its native lifecycle oracle explicitly"
        );
        assert_eq!(
            overrides.get("target_init_system").map(String::as_str),
            Some(expected_init),
            "{distro} must declare its target init authority explicitly"
        );
    }
}
