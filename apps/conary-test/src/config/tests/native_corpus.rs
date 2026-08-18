// conary-test/src/config/tests/native_corpus.rs

use super::{conary_fixture_path, load_manifest, remi_manifest_path};

mod daily_driver;
mod evidence;
mod version_rewrite;

#[test]
fn phase4_native_pm_parity_manifest_carries_cross_source_contract() {
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
