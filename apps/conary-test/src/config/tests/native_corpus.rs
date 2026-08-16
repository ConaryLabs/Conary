// conary-test/src/config/tests/native_corpus.rs

use super::{conary_fixture_path, load_manifest, remi_manifest_path};

mod evidence;
mod version_rewrite;

#[test]
fn phase4_native_pm_parity_manifest_carries_cross_source_and_daily_driver_contract() {
    let path = remi_manifest_path("phase4-native-pm-parity.toml");
    if !path.exists() {
        return;
    }

    let parity_manifest = load_manifest(&path).unwrap();
    assert!(
        parity_manifest.suite.corpus.is_none(),
        "the legacy parity suite must not own focused W7 coverage"
    );
    assert!(
        parity_manifest
            .test
            .iter()
            .all(|test| test.group.as_deref() != Some("daily-driver-corpus")),
        "daily-driver cases must have one focused manifest owner"
    );
    let cross_source = parity_manifest
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

    let provider_contract = [
        (
            "fedora44",
            "7",
            "3",
            "/etc/phase4-runtime-fixture/app.conf,/usr/bin/phase4-runtime-fixture,/usr/include/phase4-runtime-fixture/api.h",
        ),
        (
            "ubuntu-26.04",
            "4",
            "3",
            "/etc/phase4-runtime-fixture/app.conf,/usr/bin/phase4-runtime-fixture,/usr/include/phase4-runtime-fixture/api.h",
        ),
        (
            "arch",
            "4",
            "3",
            "/etc/phase4-runtime-fixture/app.conf,/usr/bin/phase4-runtime-fixture,/usr/include/phase4-runtime-fixture/api.h",
        ),
    ];
    for (distro, provider_count, file_provider_count, file_provider_set) in provider_contract {
        let overrides = parity_manifest
            .distro_overrides
            .get(distro)
            .unwrap_or_else(|| panic!("missing {distro} overrides"));
        for (key, expected) in [
            ("native_provider_count", provider_count),
            ("native_file_provider_count", file_provider_count),
            ("native_file_provider_set", file_provider_set),
        ] {
            assert_eq!(
                overrides.get(key).map(String::as_str),
                Some(expected),
                "{distro} must declare its exact {key}"
            );
        }
    }
    let metadata_test = parity_manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM04")
        .expect("Phase 4 native PM parity must include TNPM04");
    let metadata_rendered = format!("{metadata_test:?}");
    for required in [
        "${native_provider_count} provides",
        "${native_file_provider_count} file provides",
        "phase4-runtime-fixture|${native_fixture_version}|package",
        "${native_file_provider_set}",
    ] {
        assert!(
            metadata_rendered.contains(required),
            "TNPM04 must enforce the source-format and payload-owned provider contract {required}"
        );
    }
    let deferred_follow_up = parity_manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM09")
        .expect("Phase 4 native PM parity must include TNPM09");
    let deferred_rendered = format!("{deferred_follow_up:?}");
    assert!(
        deferred_rendered.contains("generation_publication")
            && deferred_rendered.contains("generation publication is pending")
            && deferred_rendered.contains("generation_publications")
            && deferred_rendered.contains("last_error")
            && deferred_rendered
                .contains("forced generation rebuild failure for test: slice-d-forced"),
        "TNPM09 must separate canonical deferred authority from exact publication failure evidence"
    );

    let mock_server = parity_manifest
        .suite
        .mock_server
        .as_ref()
        .expect("native parity must own a pinned loopback repository server");
    assert_eq!(mock_server.port, 18083);
    let route_contract = mock_server
        .routes
        .iter()
        .map(|route| {
            (
                route.path.as_str(),
                route.body_file.as_deref(),
                route.delay_ms,
            )
        })
        .collect::<Vec<_>>();
    for required in ["/metadata.json", "/phase4-repository-fixture-1.0.0-1.ccs"] {
        assert!(
            route_contract.iter().any(|(path, body_file, delay)| {
                *path == required
                    && body_file.is_some_and(|path| path.starts_with("/tmp/native-pm-pinned-repo/"))
                    && delay.is_none()
            }),
            "native parity must serve the exact bounded route {required}"
        );
    }
    let setup = format!("{:?}", parity_manifest.suite.setup);
    for required in [
        "build-pinned-binary-fixture.sh ${native_target}",
        "http://127.0.0.1:18083",
        "--default-strategy binary",
        "--ccs-package-key ${FIXTURE_CCS_PUBLIC_KEY}",
        "--source-profile ${native_profile}",
    ] {
        assert!(
            setup.contains(required),
            "native parity setup must enforce {required}"
        );
    }
    assert!(
        !setup.contains("${REMI_ENDPOINT}"),
        "native parity must not consume the live Remi endpoint"
    );
    for distro in ["fedora44", "ubuntu-26.04", "arch"] {
        let overrides = parity_manifest
            .distro_overrides
            .get(distro)
            .unwrap_or_else(|| panic!("missing {distro} overrides"));
        assert_eq!(
            overrides.get("repo_install_pkg").map(String::as_str),
            Some("phase4-repository-fixture")
        );
        assert_eq!(
            overrides.get("repo_install_path").map(String::as_str),
            Some("/usr/share/phase4-repository-fixture/probe.txt")
        );
    }
    for (target, expected_scheme, expected_architecture) in [
        ("rpm", "rpm", "x86_64"),
        ("deb", "debian", "amd64"),
        ("arch", "arch", "x86_64"),
    ] {
        let fixture = conary_fixture_path(&format!("phase4-pinned-repository/{target}/ccs.toml"));
        let manifest = conary_core::ccs::manifest::CcsManifest::from_file(&fixture)
            .unwrap_or_else(|error| panic!("load {}: {error}", fixture.display()));
        assert_eq!(manifest.package.name, "phase4-repository-fixture");
        assert_eq!(manifest.package.version, "1.0.0");
        assert_eq!(manifest.package.release, "1");
        assert_eq!(manifest.package.version_scheme.as_str(), expected_scheme);
        assert_eq!(
            manifest
                .package
                .platform
                .as_ref()
                .expect("pinned repository fixture must declare a platform")
                .arch
                .as_deref(),
            Some(expected_architecture)
        );
    }
    let pinned_builder =
        std::fs::read_to_string(conary_fixture_path("native/build-pinned-binary-fixture.sh"))
            .expect("read pinned binary fixture builder");
    for required in [
        "fixture-signing-key.private",
        "hashlib.sha256(artifact_bytes).hexdigest()",
        "\"security_advisory_source\": None",
        "\"download_url\": f\"{repository_url}/{artifact.name}\"",
        "\"release\": release",
        "\"requirements\": []",
        "\"relations\": []",
        "pinned-binary-fixture-manifest.json",
        "\"schema_version\": 1",
        "os.replace(temporary_path, path)",
    ] {
        assert!(
            pinned_builder.contains(required),
            "pinned binary builder must enforce {required}"
        );
    }
    let security_probe = std::fs::read_to_string(conary_fixture_path(
        "native/prepare-unknown-security-repository.sh",
    ))
    .expect("read unknown-security repository helper");
    for required in [
        "--security-advisories unknown",
        "--source-profile \"${source_profile}\"",
        "security_advisory_support, package_format, source_profile",
    ] {
        assert!(
            security_probe.contains(required),
            "unknown-security repository helper must enforce {required}"
        );
    }
    assert!(
        !security_probe.contains("UPDATE troves"),
        "unknown-security proof must not rewrite installed provenance through raw SQLite"
    );
    let autoremove_test = parity_manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM12")
        .expect("Phase 4 native PM parity must include TNPM12");
    let autoremove_rendered = format!("{autoremove_test:?}");
    for required in [
        "model apply",
        "--strict",
        "--no-autoremove",
        "native-matrix-root-layout",
        "phase4-runtime-fixture",
        "Marked '${repo_install_pkg}' as dependency",
    ] {
        assert!(
            autoremove_rendered.contains(required),
            "TNPM12 must establish its orphan through model authority {required}"
        );
    }
    assert!(
        !autoremove_rendered.contains("UPDATE troves"),
        "TNPM12 must not rewrite install reason through raw SQLite"
    );
    let container_root = path
        .parent()
        .and_then(std::path::Path::parent)
        .expect("native parity manifest must live below the Remi integration root")
        .join("containers");
    for containerfile in [
        "Containerfile.fedora44",
        "Containerfile.ubuntu-26.04",
        "Containerfile.arch",
        "Containerfile.artix",
        "Containerfile.cachyos",
        "Containerfile.opensuse-tumbleweed",
        "Containerfile.debian-derivative",
    ] {
        let contents = std::fs::read_to_string(container_root.join(containerfile))
            .unwrap_or_else(|error| panic!("read {containerfile}: {error}"));
        assert!(
            contents.contains("command -v setsid"),
            "{containerfile} must prove the synchronous exec supervisor capability"
        );
    }

    let daily_driver_path = remi_manifest_path("phase4-native-daily-driver-corpus.toml");
    let manifest = load_manifest(&daily_driver_path).expect("load focused daily-driver manifest");
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
            conary_core::corpus::CorpusSemantic::PayloadDirectories,
            conary_core::corpus::CorpusSemantic::PayloadSymlinks,
            conary_core::corpus::CorpusSemantic::PayloadHardlinks,
            conary_core::corpus::CorpusSemantic::MetadataOwnership,
            conary_core::corpus::CorpusSemantic::MetadataTimestamps,
            conary_core::corpus::CorpusSemantic::RelationsVersionedDependency,
            conary_core::corpus::CorpusSemantic::RelationsVirtualProvide,
            conary_core::corpus::CorpusSemantic::RelationsSameNameCompatibilityProvide,
            conary_core::corpus::CorpusSemantic::ConfigurationMatchedConfig,
            conary_core::corpus::CorpusSemantic::ConfigurationUpgrade,
            conary_core::corpus::CorpusSemantic::ConfigurationRemoval,
            conary_core::corpus::CorpusSemantic::LifecycleShell,
        ]
    );
    for unproven in [
        conary_core::corpus::CorpusSemantic::PayloadLargeFiles,
        conary_core::corpus::CorpusSemantic::MetadataXattrs,
        conary_core::corpus::CorpusSemantic::MetadataCapabilities,
        conary_core::corpus::CorpusSemantic::RelationsConflict,
        conary_core::corpus::CorpusSemantic::ConfigurationUnmatchedDeclaration,
        conary_core::corpus::CorpusSemantic::ConfigurationLocalModification,
        conary_core::corpus::CorpusSemantic::ConfigurationDeletion,
        conary_core::corpus::CorpusSemantic::LifecycleTrigger,
        conary_core::corpus::CorpusSemantic::RuntimeServiceActivation,
        conary_core::corpus::CorpusSemantic::RuntimeTargetHelper,
    ] {
        assert!(
            !required_coverage.contains(&unproven),
            "daily-driver fixture must not overclaim {unproven:?}"
        );
    }

    let corpus_tests: Vec<_> = manifest
        .test
        .iter()
        .filter(|test| test.group.as_deref() == Some("daily-driver-corpus"))
        .collect();

    assert!(
        corpus_tests.len() == 7,
        "focused daily-driver manifest should own exactly TNPM13 through TNPM19"
    );
    assert!(
        corpus_tests.iter().all(|test| test.skip.is_none()),
        "daily-driver corpus tests must not be manifest-skipped"
    );
    assert!(
        corpus_tests.iter().all(|test| test.flaky != Some(true)),
        "daily-driver corpus tests must not rely on flaky majority voting"
    );

    for (test_id, owner) in [("TNPM04", &parity_manifest), ("TNPM15", &manifest)] {
        let metadata_test = owner
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
        "phase4-repository-fixture|= 1.0.0",
        "|repository|dependency",
        "/usr/share/phase4-repository-fixture/probe.txt",
        "/var/lib/phase4-corpus/scriptlet.marker",
        "/var/lib/phase4-corpus/remove.marker",
        "phase4-corpus-conflict",
        "/usr/lib/phase4-corpus/state|directory|0750",
        "/opt|directory|0750",
        "/usr/bin/phase4-corpus-link|symlink|phase4-corpus",
        "/usr/lib/phase4-corpus/hardlink-anchor",
        "/usr/lib/phase4-corpus/hardlink-copy",
        "touch -d @1700000000",
        "--expect-directory /usr/lib/phase4-corpus/state=0750",
        "--expect-directory /opt=0750",
        "--expect-symlink /usr/bin/phase4-corpus-link=phase4-corpus",
        "--expect-hardlink /usr/lib/phase4-corpus/hardlink-anchor=/usr/lib/phase4-corpus/hardlink-copy",
        "--expect-node-metadata /usr/lib/phase4-corpus/hardlink-anchor=${native_corpus_payload_user},${native_corpus_payload_group},1700000000,0",
        "--expect-node-metadata /usr/lib/phase4-corpus/hardlink-copy=${native_corpus_payload_user},${native_corpus_payload_group},1700000000,0",
        "assert-native-hardlink.sh",
        "hardlink-copy 0 0 1700000000",
        "FILEMODES",
        "/opt|16872|root|root",
        "printf %s conflicting-corpus-payload",
        "is incompatible with package phase4-daily-driver-corpus",
        "large-payload.bin",
        "/usr/lib/kernel/install.d/95-phase4-corpus.install",
        "--from ${native_profile}",
        "query whatprovides 'virtual(phase4-corpus-tool)'",
        "query whatprovides phase4-daily-driver-corpus",
        "phase4-daily-driver-corpus|1.0|eq|package",
        "source-declared",
        "provides version: 1.0",
        "provides version: ${native_corpus_fixture_version}",
        "Total: 1 provider(s)",
        "0 config rows",
        "--dependency-fixture-manifest",
        "build-daily-driver-update-fixture.sh",
        "prepare-native-update-repository.sh",
        "--update-fixture-manifest",
        "daily_driver_corpus_configuration_upgrade",
        "installation",
        "update",
        "source_checksum",
        "w7-native-update",
        "resolution",
    ] {
        assert!(
            rendered.contains(required),
            "daily-driver corpus should cover {required}"
        );
    }
    let daily_driver_setup = format!("{:?}", manifest.suite.setup);
    for required in [
        "build-pinned-binary-fixture.sh ${native_target}",
        "http://127.0.0.1:18084",
        "--default-strategy binary",
        "--ccs-package-key ${FIXTURE_CCS_PUBLIC_KEY}",
        "repo sync ${REPO_NAME} --force",
    ] {
        assert!(
            daily_driver_setup.contains(required),
            "daily-driver dependency setup must enforce {required}"
        );
    }
    assert!(
        !daily_driver_setup.contains("${REMI_ENDPOINT}"),
        "daily-driver dependency proof must not consume the live Remi endpoint"
    );
    let mock_server = manifest
        .suite
        .mock_server
        .as_ref()
        .expect("daily-driver dependency proof needs a pinned loopback repository");
    assert_eq!(mock_server.port, 18084);
    for required in ["/metadata.json", "/phase4-repository-fixture-1.0.0-1.ccs"] {
        assert!(
            mock_server
                .routes
                .iter()
                .any(|route| route.path == required),
            "daily-driver dependency proof must serve {required}"
        );
    }

    for (distro, expected_user, expected_group) in [
        ("fedora44", "numeric:0", "numeric:0"),
        ("ubuntu-26.04", "numeric:0", "numeric:0"),
        ("arch", "numeric:0", "numeric:0"),
    ] {
        let overrides = manifest
            .distro_overrides
            .get(distro)
            .unwrap_or_else(|| panic!("missing {distro} overrides"));
        for key in [
            "native_corpus_fixture_version",
            "native_corpus_update_version",
            "native_corpus_dependency_count",
            "native_corpus_dependency_probe",
            "native_corpus_lifecycle_fidelity",
            "native_corpus_source_format",
            "native_corpus_capability_format",
            "native_corpus_target_architecture",
            "native_corpus_payload_user",
            "native_corpus_payload_group",
        ] {
            assert!(
                overrides.contains_key(key),
                "{distro} overrides should define {key}"
            );
        }
        assert_eq!(
            overrides
                .get("native_corpus_payload_user")
                .map(String::as_str),
            Some(expected_user),
            "{distro} must preserve the source format's typed user identity"
        );
        assert_eq!(
            overrides
                .get("native_corpus_payload_group")
                .map(String::as_str),
            Some(expected_group),
            "{distro} must preserve the source format's typed group identity"
        );
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
        [
            conary_core::corpus::ConversionStage::Resolution,
            conary_core::corpus::ConversionStage::Installation,
        ]
    );
    assert_eq!(install_case.coverage.len(), 13);
    for semantic in [
        conary_core::corpus::CorpusSemantic::MetadataOwnership,
        conary_core::corpus::CorpusSemantic::MetadataTimestamps,
    ] {
        let claim = install_case
            .coverage
            .iter()
            .find(|claim| claim.semantic == semantic)
            .unwrap_or_else(|| panic!("missing {semantic:?} coverage claim"));
        assert_eq!(
            claim.artifact_roles,
            [conary_core::corpus::SourceArtifactRole::InstallRequest]
        );
    }
    let dependency_claim = install_case
        .coverage
        .iter()
        .find(|claim| {
            claim.semantic == conary_core::corpus::CorpusSemantic::RelationsVersionedDependency
        })
        .expect("versioned dependency coverage claim");
    assert_eq!(
        dependency_claim.artifact_roles,
        [
            conary_core::corpus::SourceArtifactRole::InstallRequest,
            conary_core::corpus::SourceArtifactRole::InstallDependency,
        ]
    );
    assert!(
        install_case
            .coverage
            .iter()
            .filter(|claim| {
                claim.semantic != conary_core::corpus::CorpusSemantic::RelationsVersionedDependency
            })
            .all(|claim| claim.artifact_roles
                == [conary_core::corpus::SourceArtifactRole::InstallRequest])
    );

    let update_case = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM18")
        .and_then(|test| test.corpus.as_ref())
        .expect("TNPM18 typed configuration-upgrade evidence");
    assert_eq!(
        update_case.stages,
        [
            conary_core::corpus::ConversionStage::Installation,
            conary_core::corpus::ConversionStage::Update,
        ]
    );
    assert_eq!(update_case.coverage.len(), 1);
    assert_eq!(
        update_case.coverage[0].semantic,
        conary_core::corpus::CorpusSemantic::ConfigurationUpgrade
    );
    assert_eq!(
        update_case.coverage[0].artifact_roles,
        [
            conary_core::corpus::SourceArtifactRole::InstallRequest,
            conary_core::corpus::SourceArtifactRole::UpdateRequest,
        ]
    );

    let removal_test = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM19")
        .expect("TNPM19 removal test");
    let removal_case = removal_test
        .corpus
        .as_ref()
        .expect("TNPM19 typed removal evidence");
    assert_eq!(
        removal_case.stages,
        [conary_core::corpus::ConversionStage::Removal]
    );
    assert_eq!(removal_case.coverage.len(), 1);
    assert_eq!(
        removal_case.coverage[0].semantic,
        conary_core::corpus::CorpusSemantic::ConfigurationRemoval
    );
    assert_eq!(
        removal_case.coverage[0].artifact_roles,
        [conary_core::corpus::SourceArtifactRole::UpdateRequest]
    );
    assert!(
        format!("{removal_test:?}").contains("--purge"),
        "configuration removal evidence must cross the explicit purge boundary"
    );

    let build_helper =
        std::fs::read_to_string(conary_fixture_path("native/build-native-fixtures.sh"))
            .expect("read native fixture builder");
    assert!(build_helper.contains("native-fixture-manifest.json"));
    assert!(build_helper.contains("\"schema_version\": 1"));

    let hardlink_helper =
        std::fs::read_to_string(conary_fixture_path("native/assert-native-hardlink.sh"))
            .expect("read native hardlink oracle");
    for required in [
        "rpm --root",
        "dpkg-deb --extract",
        "bsdtar --extract",
        "stat --format '%d:%i:%h'",
        "stat --format '%u:%g:%Y'",
        "expected_metadata",
    ] {
        assert!(
            hardlink_helper.contains(required),
            "native hardlink oracle should enforce {required}"
        );
    }

    for (fixture, homepage) in [
        (
            "phase4-daily-driver-corpus/ccs.toml",
            "https://conary.io/fixtures/phase4-daily-driver-corpus",
        ),
        (
            "phase4-daily-driver-corpus-conflict/ccs.toml",
            "https://conary.io/fixtures/phase4-daily-driver-corpus-conflict",
        ),
    ] {
        let manifest = std::fs::read_to_string(conary_fixture_path(fixture))
            .unwrap_or_else(|error| panic!("read {fixture}: {error}"));
        assert!(
            manifest.contains(&format!("homepage = \"{homepage}\"")),
            "{fixture} must carry exact metadata required by every native exporter"
        );
        assert!(
            manifest
                .contains("maintainers = [\"Conary Fixture Maintainers <fixtures@conary.io>\"]"),
            "{fixture} must carry the exact Arch packager authority"
        );
    }

    let primary_fixture =
        std::fs::read_to_string(conary_fixture_path("phase4-daily-driver-corpus/ccs.toml"))
            .expect("read primary daily-driver fixture");
    for required in [
        "[native_export.rpm]",
        "requires = [{ name = \"phase4-repository-fixture\", relation = \"equal\", version = \"1.0.0\" }]",
        "{ name = \"phase4-corpus-tool\", relation = \"any\" }",
        "{ name = \"phase4-daily-driver-corpus\", relation = \"equal\", version = \"1.0\" }",
        "[native_export.deb]",
        "depends = [\"phase4-repository-fixture (= 1.0.0)\"]",
        "provides = [\"phase4-corpus-tool\", \"phase4-daily-driver-corpus (= 1.0)\"]",
        "[native_export.arch]",
        "depends = [\"phase4-repository-fixture=1.0.0\"]",
        "provides = [\"phase4-corpus-tool\", \"phase4-daily-driver-corpus=1.0\"]",
    ] {
        assert!(
            primary_fixture.contains(required),
            "daily-driver native export must preserve {required}"
        );
    }

    let selected_root_helper =
        std::fs::read_to_string(conary_fixture_path("native/prepare-selected-root.sh"))
            .expect("read selected-root preparation helper");
    for required in [
        "install_path=\"$(command -v install)\"",
        "copy_elf_closure \"${install_path}\"",
        "if ($1 ~ /^\\//) { print $1 }",
        "print $3",
        "copy_elf_closure /bin/false",
        "--present /bin/false",
        "--present \"${install_path}\"",
    ] {
        assert!(
            selected_root_helper.contains(required),
            "daily-driver lifecycle runtime should include {required}"
        );
    }

    let selected_generation_helper =
        std::fs::read_to_string(conary_fixture_path("native/assert-selected-generation.py"))
            .expect("read selected-generation assertion helper");
    for required in [
        "--expect-hardlink",
        "--expect-node-metadata",
        "hardlink_identity",
        "content authority is not anchor-only",
        "expected_payload_identity",
        "expected_mtime",
    ] {
        assert!(
            selected_generation_helper.contains(required),
            "selected-generation hardlink proof should enforce {required}"
        );
    }

    let evidence_helper =
        std::fs::read_to_string(conary_fixture_path("native/write-corpus-evidence.py"))
            .expect("read native corpus evidence writer");
    for required in [
        "fixture_manifest",
        "schema_version",
        "sha256(artifact_path)",
        "fixture_build_manifest",
        "install_dependency",
        "update_request",
        "dependency artifact identity requires all dependency arguments",
        "update artifact identity requires all update arguments",
        "os.replace",
    ] {
        assert!(
            evidence_helper.contains(required),
            "native corpus evidence writer should enforce {required}"
        );
    }

    let update_builder = std::fs::read_to_string(conary_fixture_path(
        "native/build-daily-driver-update-fixture.sh",
    ))
    .expect("read daily-driver update builder");
    for required in [
        "rewrite-package-version.py",
        "1.0.0",
        "1.0.1",
        "revision = \"v2\"",
        "build-native-fixtures.sh",
        "native-fixture-manifest.json",
    ] {
        assert!(
            update_builder.contains(required),
            "daily-driver update builder should enforce {required}"
        );
    }

    let update_repository = std::fs::read_to_string(conary_fixture_path(
        "native/prepare-native-update-repository.sh",
    ))
    .expect("read native update repository helper");
    for required in [
        "native update artifact digest contradicts its build manifest",
        "cook \"${native_artifact}\"",
        "--source-profile \"${source_profile}\"",
        "ccs verify",
        "repository-fixture-manifest.json",
        "--default-strategy binary",
        "--ccs-package-key",
        "repo sync",
    ] {
        assert!(
            update_repository.contains(required),
            "native update repository should enforce {required}"
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
