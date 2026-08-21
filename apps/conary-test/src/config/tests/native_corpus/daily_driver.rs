// conary-test/src/config/tests/native_corpus/daily_driver.rs

use super::{conary_fixture_path, load_manifest, remi_manifest_path};
use conary_core::packages::{PackageFormat, traits::NativeLifecyclePath};

#[test]
fn phase4_daily_driver_corpus_manifest_proves_remaining_configuration_states() {
    let path = remi_manifest_path("phase4-native-daily-driver-corpus.toml");
    if !path.exists() {
        return;
    }
    let manifest = load_manifest(&path).expect("load focused daily-driver manifest");
    let parity_path = remi_manifest_path("phase4-native-pm-parity.toml");
    if !parity_path.exists() {
        return;
    }
    let parity_manifest = load_manifest(&parity_path).expect("load native parity manifest");
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
            conary_core::corpus::CorpusSemantic::IdentityEpochRelease,
            conary_core::corpus::CorpusSemantic::IdentityArchitectureIndependent,
            conary_core::corpus::CorpusSemantic::IdentityNativeArchitecture,
            conary_core::corpus::CorpusSemantic::PayloadFiles,
            conary_core::corpus::CorpusSemantic::PayloadDirectories,
            conary_core::corpus::CorpusSemantic::PayloadSymlinks,
            conary_core::corpus::CorpusSemantic::PayloadHardlinks,
            conary_core::corpus::CorpusSemantic::PayloadSparseFiles,
            conary_core::corpus::CorpusSemantic::PayloadLargeFiles,
            conary_core::corpus::CorpusSemantic::MetadataXattrs,
            conary_core::corpus::CorpusSemantic::MetadataCapabilities,
            conary_core::corpus::CorpusSemantic::MetadataOwnership,
            conary_core::corpus::CorpusSemantic::MetadataTimestamps,
            conary_core::corpus::CorpusSemantic::RelationsVersionedDependency,
            conary_core::corpus::CorpusSemantic::RelationsVirtualProvide,
            conary_core::corpus::CorpusSemantic::RelationsSameNameCompatibilityProvide,
            conary_core::corpus::CorpusSemantic::RelationsConflict,
            conary_core::corpus::CorpusSemantic::RelationsReplacement,
            conary_core::corpus::CorpusSemantic::ConfigurationMatchedConfig,
            conary_core::corpus::CorpusSemantic::ConfigurationUnmatchedDeclaration,
            conary_core::corpus::CorpusSemantic::ConfigurationLocalModification,
            conary_core::corpus::CorpusSemantic::ConfigurationDeletion,
            conary_core::corpus::CorpusSemantic::ConfigurationUpgrade,
            conary_core::corpus::CorpusSemantic::ConfigurationRemoval,
            conary_core::corpus::CorpusSemantic::LifecycleNone,
            conary_core::corpus::CorpusSemantic::LifecycleShell,
            conary_core::corpus::CorpusSemantic::LifecycleNonShellInterpreter,
            conary_core::corpus::CorpusSemantic::LifecycleTrigger,
            conary_core::corpus::CorpusSemantic::LifecycleTransactionHook,
            conary_core::corpus::CorpusSemantic::TrustMultipleValidSigningKeys,
            conary_core::corpus::CorpusSemantic::TrustSigningSubkeys,
            conary_core::corpus::CorpusSemantic::TrustExpiredKey,
            conary_core::corpus::CorpusSemantic::TrustRevokedKey,
            conary_core::corpus::CorpusSemantic::TrustUnknownKey,
            conary_core::corpus::CorpusSemantic::RuntimeServiceActivation,
            conary_core::corpus::CorpusSemantic::RuntimeTargetHelper,
            conary_core::corpus::CorpusSemantic::RuntimeDeferredGenerationWork,
            conary_core::corpus::CorpusSemantic::FailureInterruptedDownload,
            conary_core::corpus::CorpusSemantic::FailureConversion,
            conary_core::corpus::CorpusSemantic::FailurePayloadMutation,
            conary_core::corpus::CorpusSemantic::FailureLifecycle,
            conary_core::corpus::CorpusSemantic::FailurePublication,
            conary_core::corpus::CorpusSemantic::FailureActivation,
        ]
    );

    let corpus_tests: Vec<_> = manifest
        .test
        .iter()
        .filter(|test| test.group.as_deref() == Some("daily-driver-corpus"))
        .collect();

    assert!(
        corpus_tests.len() == 20,
        "focused daily-driver manifest should own exactly TNPM13 through TNPM32"
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
        "/etc/phase4-corpus/app-local.conf",
        "/etc/phase4-corpus/app-deleted.conf",
        "/etc/phase4-corpus/app-unmatched.conf",
        "9 regular files",
        "phase4-corpus-alt",
        "phase4-corpus-user",
        "phase4-corpus-group",
        "${native_corpus_dependency_probe}",
        "phase4-repository-fixture|= 1.0.0",
        "|repository|dependency",
        "/usr/share/phase4-repository-fixture/probe.txt",
        "/var/lib/phase4-corpus/scriptlet.marker",
        "captured-systemctl|phase4-daily-driver-corpus|systemd|start|phase4-corpus.service",
        "captured-boot-runtime|phase4-daily-driver-corpus|boot-runtime|depmod|-a",
        "pending|2",
        "RuntimeServiceActivation",
        "RuntimeTargetHelper",
        "FailureInterruptedDownload",
        "FailureConversion",
        "FailurePayloadMutation",
        "FailureLifecycle",
        "FailurePublication",
        "FailureActivation",
        "RuntimeDeferredGenerationWork",
        "assert-w7-command-rejection.sh",
        "CONARY_TEST_PROC_CMDLINE_PATH",
        "w7-publication-failure",
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

    let trust_cases = manifest
        .test
        .iter()
        .filter(|test| {
            matches!(
                test.id.as_str(),
                "TNPM23" | "TNPM24" | "TNPM25" | "TNPM26" | "TNPM27"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(trust_cases.len(), 5);
    let trust_semantics = trust_cases
        .iter()
        .flat_map(|test| test.corpus.as_ref().unwrap().coverage.iter())
        .map(|claim| claim.semantic)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        trust_semantics,
        std::collections::BTreeSet::from([
            conary_core::corpus::CorpusSemantic::TrustMultipleValidSigningKeys,
            conary_core::corpus::CorpusSemantic::TrustSigningSubkeys,
            conary_core::corpus::CorpusSemantic::TrustExpiredKey,
            conary_core::corpus::CorpusSemantic::TrustRevokedKey,
            conary_core::corpus::CorpusSemantic::TrustUnknownKey,
        ])
    );
    let rendered_trust = format!("{manifest:?}");
    for required in [
        "w7trust.gpg",
        "arch-keyring-format open-pgp",
        "w7-trust-valid-a",
        "w7-trust-valid-b",
        "is expired at the trust-snapshot time",
        "KeyRevoked",
        "no authenticated keyring certificate owns signing key",
        "assert-w7-trust-rejection.sh",
    ] {
        assert!(
            rendered_trust.contains(required),
            "W7 trust corpus should prove {required}"
        );
    }

    let rpm_semantics_test = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM22")
        .expect("TNPM22 RPM relation and lifecycle test");
    let rpm_semantics_case = rpm_semantics_test
        .corpus
        .as_ref()
        .expect("TNPM22 typed relation and lifecycle evidence");
    assert_eq!(
        rpm_semantics_case
            .coverage
            .iter()
            .map(|claim| claim.semantic)
            .collect::<Vec<_>>(),
        [
            conary_core::corpus::CorpusSemantic::RelationsConflict,
            conary_core::corpus::CorpusSemantic::RelationsReplacement,
            conary_core::corpus::CorpusSemantic::LifecycleNonShellInterpreter,
            conary_core::corpus::CorpusSemantic::LifecycleTrigger,
            conary_core::corpus::CorpusSemantic::LifecycleTransactionHook,
        ]
    );
    assert!(rpm_semantics_case.coverage.iter().all(|claim| {
        claim.artifact_roles == [conary_core::corpus::SourceArtifactRole::InstallRequest]
    }));
    let rendered_rpm_semantics = format!("{rpm_semantics_test:?}");
    for required in [
        "phase4-w7-rpm-semantics",
        "package_requirement_groups",
        "conflict|phase4-w7-conflict|< 2",
        "obsolete|phase4-w7-replaced|<= 1",
        "native_slot = \\\"%post\\\"",
        "native_slot = \\\"%pretrans\\\"",
        "native_slot = \\\"%filetriggerin\\\"",
        "interpreter = \\\"/usr/bin/python3\\\"",
    ] {
        assert!(
            rendered_rpm_semantics.contains(required),
            "TNPM22 should prove {required}"
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
    assert_eq!(install_case.coverage.len(), 15);
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
    assert_eq!(update_case.coverage.len(), 4);
    for (semantic, roles) in [
        (
            conary_core::corpus::CorpusSemantic::ConfigurationUnmatchedDeclaration,
            [
                conary_core::corpus::SourceArtifactRole::InstallRequest,
                conary_core::corpus::SourceArtifactRole::UpdateRequest,
            ],
        ),
        (
            conary_core::corpus::CorpusSemantic::ConfigurationLocalModification,
            [
                conary_core::corpus::SourceArtifactRole::InstallRequest,
                conary_core::corpus::SourceArtifactRole::UpdateRequest,
            ],
        ),
        (
            conary_core::corpus::CorpusSemantic::ConfigurationDeletion,
            [
                conary_core::corpus::SourceArtifactRole::InstallRequest,
                conary_core::corpus::SourceArtifactRole::UpdateRequest,
            ],
        ),
        (
            conary_core::corpus::CorpusSemantic::ConfigurationUpgrade,
            [
                conary_core::corpus::SourceArtifactRole::InstallRequest,
                conary_core::corpus::SourceArtifactRole::UpdateRequest,
            ],
        ),
    ] {
        let claim = update_case
            .coverage
            .iter()
            .find(|claim| claim.semantic == semantic)
            .unwrap_or_else(|| panic!("missing {semantic:?} coverage claim"));
        assert_eq!(
            claim.artifact_roles, roles,
            "{semantic:?} must bind install and update artifact roles"
        );
    }
    let update_test = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM18")
        .expect("TNPM18 update test");
    let rendered_update = format!("{update_test:?}");
    for required in [
        "app-local.conf",
        "app-deleted.conf",
        "app-unmatched.conf",
        "locally-edited",
        "user-edited",
        "/dev/shm/conary-config-upper-",
        "mount --bind",
        "mknod",
        "app-local.conf.rpmnew",
        "app-local.conf.dpkg-dist",
        "app-local.conf.pacnew",
        "app-unmatched.conf=09f55aa386292b9525a334ba15a6a5e1922d08f3013772ae3b61c009b057b510",
        "app-unmatched.conf||09f55aa386292b9525a334ba15a6a5e1922d08f3013772ae3b61c009b057b510|1|modified|deb|0|0|1",
        "materialized || '|' || ghost",
        "remove_on_upgrade",
    ] {
        assert!(
            rendered_update.contains(required),
            "daily-driver update should prove {required}"
        );
    }

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

    let semantic_case = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM20")
        .and_then(|test| test.corpus.as_ref())
        .expect("TNPM20 typed identity, payload, xattr, and no-lifecycle evidence");
    assert_eq!(
        semantic_case
            .coverage
            .iter()
            .map(|claim| claim.semantic)
            .collect::<Vec<_>>(),
        [
            conary_core::corpus::CorpusSemantic::IdentityEpochRelease,
            conary_core::corpus::CorpusSemantic::IdentityArchitectureIndependent,
            conary_core::corpus::CorpusSemantic::PayloadSparseFiles,
            conary_core::corpus::CorpusSemantic::PayloadLargeFiles,
            conary_core::corpus::CorpusSemantic::MetadataXattrs,
            conary_core::corpus::CorpusSemantic::LifecycleNone,
        ]
    );
    assert!(semantic_case.coverage.iter().all(|claim| {
        claim.artifact_roles == [conary_core::corpus::SourceArtifactRole::InstallRequest]
    }));
    let semantic_test = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM20")
        .expect("TNPM20 semantic test");
    let rendered_semantic = format!("{semantic_test:?}");
    for required in [
        "pkgver = 2:1.0.0-3",
        "arch = any",
        "st_blocks",
        "32*1024*1024",
        "64*1024*1024",
        "user.conary",
        "installed_native_lifecycle_bundles",
        "native-free",
    ] {
        assert!(
            rendered_semantic.contains(required),
            "TNPM20 should prove {required}"
        );
    }

    let capability_test = manifest
        .test
        .iter()
        .find(|test| test.id == "TNPM21")
        .expect("TNPM21 capability test");
    let capability_case = capability_test
        .corpus
        .as_ref()
        .expect("TNPM21 typed capability evidence");
    assert_eq!(capability_case.coverage.len(), 1);
    assert_eq!(
        capability_case.coverage[0].semantic,
        conary_core::corpus::CorpusSemantic::MetadataCapabilities
    );
    assert_eq!(
        capability_case.coverage[0].artifact_roles,
        [conary_core::corpus::SourceArtifactRole::InstallRequest]
    );
    let rendered_capability = format!("{capability_test:?}");
    for required in [
        "phase4-w7-capability-corpus",
        "NATIVE_PKG_SHA256",
        "installed_file_capabilities",
        "security.capability",
        "0100000200040000000000000000000000000000",
    ] {
        assert!(
            rendered_capability.contains(required),
            "TNPM21 should prove {required}"
        );
    }

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
        "/usr/sbin/depmod -a",
    ] {
        assert!(
            primary_fixture.contains(required),
            "daily-driver native export must preserve {required}"
        );
    }
    for config_path in [
        "/etc/phase4-corpus/app.conf",
        "/etc/phase4-corpus/app-local.conf",
        "/etc/phase4-corpus/app-deleted.conf",
    ] {
        assert!(
            primary_fixture.contains(&format!("path = \"{config_path}\"")),
            "daily-driver fixture must declare {config_path}"
        );
    }
    assert!(
        primary_fixture.contains("payload = \"matched\""),
        "the v1 fixture ships every declared config path"
    );
    assert!(
        !primary_fixture.contains("/etc/phase4-corpus/app-unmatched.conf"),
        "the v1 fixture must not declare the unmatched path before the update"
    );
    for stage_file in [
        "stage/etc/phase4-corpus/app-local.conf",
        "stage/etc/phase4-corpus/app-deleted.conf",
    ] {
        assert!(
            conary_fixture_path(&format!("phase4-daily-driver-corpus/{stage_file}")).is_file(),
            "daily-driver fixture must ship {stage_file}"
        );
    }
    assert!(
        !conary_fixture_path(
            "phase4-daily-driver-corpus/stage/etc/phase4-corpus/app-unmatched.conf"
        )
        .exists(),
        "the v1 fixture must not ship the unmatched path"
    );

    let selected_root_helper =
        std::fs::read_to_string(conary_fixture_path("native/prepare-selected-root.sh"))
            .expect("read selected-root preparation helper");
    for required in [
        "install_path=\"$(command -v install)\"",
        "copy_elf_closure \"${install_path}\"",
        "if ($1 ~ /^\\//) { print $1 }",
        "print $3",
        "copy_elf_closure /bin/false",
        "python_path=/usr/bin/python3",
        "copy_elf_closure \"${python_path}\"",
        "python_stdlib=\"$(${python_path} -I -S -c",
        "python_pathlib=\"$(${python_path} -I -S -c",
        "copy_runtime_tree \"${python_stdlib}\"",
        "--present \"${python_path}\"",
        "--present \"${python_pathlib}\"",
        "systemctl_path=/usr/bin/systemctl",
        "Selected-root systemctl fixture is missing",
        "rm -f \"${stage}/usr/bin/systemctl\"",
        "depmod_path=/usr/sbin/depmod",
        "Selected-root depmod fixture is missing",
        "--present /bin/false",
        "--present \"${systemctl_path:-/usr/bin/true}\"",
        "--present \"${depmod_path}\"",
        "--present \"${install_path}\"",
    ] {
        assert!(
            selected_root_helper.contains(required),
            "daily-driver lifecycle runtime should include {required}"
        );
    }
    assert!(
        conary_fixture_path("native-selected-root-layout/stage/usr/bin/systemctl").is_file(),
        "selected-root activation capture must ship its deterministic systemctl boundary"
    );
    assert!(
        conary_fixture_path("native-selected-root-layout/stage/usr/sbin/depmod").is_file(),
        "selected-root boot-runtime capture must ship its deterministic depmod boundary"
    );
    assert!(
        !conary_fixture_path("phase4-daily-driver-corpus/stage/usr/sbin/depmod").exists(),
        "the source package must invoke the typed depmod boundary without replacing it"
    );

    let selected_generation_helper =
        std::fs::read_to_string(conary_fixture_path("native/assert-selected-generation.py"))
            .expect("read selected-generation assertion helper");
    for required in [
        "--expect-hardlink",
        "--expect-node-metadata",
        "--expect-xattr-hex",
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
        "app-local.conf",
        "app-deleted.conf",
        "app-unmatched.conf",
        "\"driver-v2\"",
        "payload = \"absent\"",
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
fn pinned_w7_rpm_fixture_carries_typed_relations_and_lifecycle_abi() {
    let output = tempfile::tempdir().expect("create W7 RPM fixture output");
    let fixtures_root = conary_fixture_path("");
    let status = std::process::Command::new(conary_fixture_path(
        "native/build-w7-rpm-semantics-fixture.sh",
    ))
    .arg(output.path())
    .env("CONARY_FIXTURES_ROOT", &fixtures_root)
    .status()
    .expect("decode pinned W7 RPM fixture");
    assert!(status.success());

    let artifact = output
        .path()
        .join("phase4-w7-rpm-semantics-1.0.0-1.x86_64.rpm");
    assert_eq!(
        conary_core::hash::sha256(&std::fs::read(&artifact).unwrap()),
        "0b412f9d56da06d94bff3f7f3ba58b080633edf90e5bcc6beba9fe40544bc0ca"
    );
    let package = conary_core::packages::rpm::RpmPackage::parse(
        artifact.to_str().expect("UTF-8 RPM fixture path"),
    )
    .expect("parse pinned W7 RPM fixture");
    assert_eq!(
        package
            .relations()
            .iter()
            .map(|relation| relation.kind)
            .collect::<Vec<_>>(),
        [
            conary_core::repository::dependency_model::RepositoryRequirementKind::Conflict,
            conary_core::repository::dependency_model::RepositoryRequirementKind::Obsolete,
        ]
    );

    let scriptlets = package.native_scriptlet_abi();
    for (slot, lifecycle, interpreter) in [
        (
            "%post",
            NativeLifecyclePath::PostInstall,
            "/usr/bin/python3",
        ),
        (
            "%pretrans",
            NativeLifecyclePath::PreTransaction,
            "/usr/bin/python3",
        ),
        (
            "%filetriggerin",
            NativeLifecyclePath::FileTrigger,
            "/usr/bin/python3",
        ),
    ] {
        let scriptlet = scriptlets
            .iter()
            .find(|entry| entry.native_slot == slot)
            .unwrap_or_else(|| panic!("missing {slot}"));
        assert_eq!(scriptlet.primary_lifecycle, lifecycle);
        assert_eq!(scriptlet.interpreter.as_deref(), Some(interpreter));
    }
}
