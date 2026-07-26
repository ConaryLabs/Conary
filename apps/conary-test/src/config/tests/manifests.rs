// conary-test/src/config/tests/manifests.rs

use super::*;

#[test]
fn test_load_phase1_core_manifest() {
    let path = remi_manifest_path("phase1-core.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert!(
            manifest.test.len() >= 10,
            "Expected at least 10 tests, got {}",
            manifest.test.len()
        );

        // Verify suite metadata
        assert_eq!(manifest.suite.phase, 1);
        assert!(!manifest.suite.setup.is_empty(), "Expected setup steps");

        // Verify T01 health check
        let t01 = &manifest.test[0];
        assert_eq!(t01.id, "T01");
        assert_eq!(t01.name, "health_check");
        assert_eq!(t01.fatal, Some(true));

        // Verify T04 repo sync is fatal
        let t04 = manifest.test.iter().find(|t| t.id == "T04").unwrap();
        assert_eq!(t04.fatal, Some(true));
        assert_eq!(t04.timeout, 300);

        // Verify T08 has file_executable step
        let t08 = manifest.test.iter().find(|t| t.id == "T08").unwrap();
        assert!(
            t08.step.iter().any(|s| s.file_executable.is_some()),
            "T08 should have a file_executable step"
        );

        // Verify T10 has exit_code_not assertion
        let t10 = manifest.test.iter().find(|t| t.id == "T10").unwrap();
        let assertion = t10.step[0].assert.as_ref().unwrap();
        assert_eq!(assertion.exit_code_not, Some(0));
    }
}

#[test]
fn test_load_phase2_group_a_manifest() {
    let path = remi_manifest_path("phase2-group-a.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 2);
        assert!(
            manifest.test.len() >= 13,
            "Expected at least 13 tests (T38-T50), got {}",
            manifest.test.len()
        );

        // Verify T38 is fatal
        let t38 = manifest.test.iter().find(|t| t.id == "T38").unwrap();
        assert_eq!(t38.name, "install_fixture_v1_with_deps");
        assert_eq!(t38.fatal, Some(true));
        assert_eq!(t38.group.as_deref(), Some("A"));

        // Verify T39 has dir_exists step
        let t39 = manifest.test.iter().find(|t| t.id == "T39").unwrap();
        assert!(
            t39.step.iter().any(|s| s.dir_exists.is_some()),
            "T39 should have a dir_exists step"
        );

        // Verify T40 has file_checksum step
        let t40 = manifest.test.iter().find(|t| t.id == "T40").unwrap();
        assert!(
            t40.step.iter().any(|s| s.file_checksum.is_some()),
            "T40 should have a file_checksum step"
        );

        // Verify T42 has file_not_exists steps
        let t42 = manifest.test.iter().find(|t| t.id == "T42").unwrap();
        let not_exists_count = t42
            .step
            .iter()
            .filter(|s| s.file_not_exists.is_some())
            .count();
        assert_eq!(
            not_exists_count, 2,
            "T42 should have 2 file_not_exists steps"
        );

        // Verify T48 depends on T47
        let t48 = manifest.test.iter().find(|t| t.id == "T48").unwrap();
        assert_eq!(t48.depends_on.as_ref().unwrap(), &["T47"]);
    }
}

#[test]
fn test_load_phase2_group_b_manifest() {
    let path = remi_manifest_path("phase2-group-b.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 2);
        assert!(
            manifest.test.len() >= 7,
            "Expected at least 7 tests (T51-T57), got {}",
            manifest.test.len()
        );

        // Verify setup step installs fixture
        assert!(!manifest.suite.setup.is_empty(), "Expected setup steps");

        // Verify T51 is fatal and uses run (no_db)
        let t51 = manifest.test.iter().find(|t| t.id == "T51").unwrap();
        assert_eq!(t51.fatal, Some(true));
        assert_eq!(t51.group.as_deref(), Some("B"));
        assert!(t51.step[0].run.is_some(), "T51 should use run (no_db)");

        // Verify T57 exercises takeover through ready-to-activate state.
        let t57 = manifest.test.iter().find(|t| t.id == "T57").unwrap();
        let a57 = t57.step[0].assert.as_ref().unwrap();
        assert_eq!(a57.exit_code, Some(0));
        assert!(
            t57.step[0]
                .conary
                .as_deref()
                .is_some_and(|cmd| cmd.contains("system takeover --up-to generation"))
        );
        let all57 = a57.stdout_contains_all.as_ref().unwrap();
        assert!(
            all57.contains(&"ready to activate".to_string())
                && all57.contains(&"conary system generation switch".to_string())
        );
    }
}

#[test]
fn test_load_phase2_group_c_manifest() {
    let path = remi_manifest_path("phase2-group-c.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 2);
        assert!(
            manifest.test.len() >= 4,
            "Expected at least 4 tests (T58-T61), got {}",
            manifest.test.len()
        );

        // Verify setup creates directories
        assert!(!manifest.suite.setup.is_empty(), "Expected setup steps");

        // Verify T58 uses stdout_contains_if_success
        let t58 = manifest.test.iter().find(|t| t.id == "T58").unwrap();
        assert_eq!(t58.group.as_deref(), Some("C"));
        let a58 = t58.step[0].assert.as_ref().unwrap();
        assert!(
            a58.stdout_contains_if_success.is_some(),
            "T58 should use stdout_contains_if_success"
        );

        // Verify T61 uses run (shell command with timeout)
        let t61 = manifest.test.iter().find(|t| t.id == "T61").unwrap();
        assert!(t61.step[0].run.is_some(), "T61 should use run");
    }
}

#[test]
fn test_load_phase2_group_d_manifest() {
    let path = remi_manifest_path("phase2-group-d.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 2);
        for (id, name) in [
            ("T62", "cook_toml_recipe"),
            ("T63", "ccs_output_valid"),
            ("T66", "hermetic_build"),
        ] {
            let test = manifest
                .test
                .iter()
                .find(|test| test.id == id)
                .unwrap_or_else(|| panic!("phase2 group D should retain durable test {id}"));
            assert_eq!(test.name, name, "{id} should retain its durable identity");
            assert_eq!(test.group.as_deref(), Some("D"));
        }

        // Verify T62 is fatal
        let t62 = manifest.test.iter().find(|t| t.id == "T62").unwrap();
        assert_eq!(t62.fatal, Some(true));
        let cook = t62.step[0]
            .run
            .as_deref()
            .expect("T62 should invoke the recipe cook path");
        assert!(
            cook.contains("recipes/simple-hello/recipe.toml")
                && cook.contains("--source-cache")
                && cook.contains("--output"),
            "T62 should cook the HTTP-backed fixture into an explicit output directory"
        );

        let t63 = manifest.test.iter().find(|t| t.id == "T63").unwrap();
        assert!(matches!(t63.depends_on.as_deref(), Some([dependency]) if dependency == "T62"));
        let output_check = t63.step[0]
            .run
            .as_deref()
            .expect("T63 should check cook output");
        assert!(
            output_check.contains("/tmp/conary-recipe-output/*.ccs"),
            "T63 should prove T62 emitted a CCS artifact"
        );

        let mock_server = manifest
            .suite
            .mock_server
            .as_ref()
            .expect("phase2 group D should serve recipe fixture sources over HTTP");
        assert!(
            mock_server.routes.iter().any(|route| {
                route.path == "/source.tar.gz"
                    && route.body_file.as_deref()
                        == Some("/opt/remi-tests/fixtures/recipes/simple-hello/source.tar.gz")
            }),
            "phase2 group D should expose source.tar.gz via mock HTTP server"
        );

        let recipe =
            std::fs::read_to_string(conary_fixture_path("recipes/simple-hello/recipe.toml"))
                .expect("read simple hello recipe fixture");
        assert!(
            recipe.contains("archive = \"http://127.0.0.1:18083/source.tar.gz\""),
            "simple hello recipe should use the phase2 group D mock HTTP source"
        );
        assert!(
            !recipe.contains("archive = \"file://"),
            "simple hello recipe should not use file:// sources"
        );

        // Verify T66 uses exit_code_not and stdout_contains_any
        let t66 = manifest.test.iter().find(|t| t.id == "T66").unwrap();
        assert!(
            t66.step[0]
                .run
                .as_deref()
                .is_some_and(|command| command.contains("--isolated")),
            "T66 should retain the hermetic isolation contract"
        );
        let a66 = t66.step[0].assert.as_ref().unwrap();
        assert_eq!(
            a66.exit_code_not,
            Some(0),
            "T66 should require non-zero exit"
        );
        assert!(
            a66.stdout_contains_any.is_some(),
            "T66 should use stdout_contains_any"
        );
        let any66 = a66.stdout_contains_any.as_ref().unwrap();
        assert!(any66.len() >= 5, "T66 should check for at least 5 keywords");
    }
}

#[test]
fn test_load_phase2_group_e_manifest() {
    let path = remi_manifest_path("phase2-group-e.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 2);
        assert!(
            manifest.test.len() >= 5,
            "Expected at least 5 tests (T67-T71), got {}",
            manifest.test.len()
        );

        // Verify T67 uses run (curl command)
        let t67 = manifest.test.iter().find(|t| t.id == "T67").unwrap();
        assert_eq!(t67.group.as_deref(), Some("E"));
        assert!(t67.step[0].run.is_some(), "T67 should use run (curl)");

        // Verify T68 has file_exists step
        let t68 = manifest.test.iter().find(|t| t.id == "T68").unwrap();
        assert!(
            t68.step.iter().any(|s| s.file_exists.is_some()),
            "T68 should verify file exists after install"
        );
        let install_step = t68
            .step
            .iter()
            .filter_map(|step| step.conary.as_deref())
            .find(|command| command.starts_with("install "))
            .expect("T68 should install a package");
        assert!(
            install_step.contains("${TEST_PACKAGE_3}"),
            "T68 should install a distro-backed Remi package"
        );
        assert!(
            !install_step.contains("${FIXTURE_PKG_NAME}"),
            "T68 should not install the local CCS fixture by Remi package name"
        );
        let file_check = t68
            .step
            .iter()
            .find_map(|step| step.file_exists.as_deref())
            .expect("T68 should verify an installed file");
        assert_eq!(
            file_check, "${TEST_BINARY_3}",
            "T68 should verify the selected distro package binary"
        );

        // Verify T69 uses stdout_contains_any for HTTP code check
        let t69 = manifest.test.iter().find(|t| t.id == "T69").unwrap();
        let a69 = t69.step[0].assert.as_ref().unwrap();
        assert!(
            a69.stdout_contains_any.is_some(),
            "T69 should use stdout_contains_any"
        );

        // Verify T71 checks for "packages"
        let t71 = manifest.test.iter().find(|t| t.id == "T71").unwrap();
        let a71 = t71.step[0].assert.as_ref().unwrap();
        assert_eq!(a71.stdout_contains.as_deref(), Some("packages"));
    }
}

#[test]
fn test_load_phase2_group_f_manifest() {
    let path = remi_manifest_path("phase2-group-f.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 2);
        assert!(
            manifest.test.len() >= 5,
            "Expected at least 5 tests (T72-T76), got {}",
            manifest.test.len()
        );

        // Verify T72 uses stdout_contains_all
        let t72 = manifest.test.iter().find(|t| t.id == "T72").unwrap();
        assert_eq!(t72.group.as_deref(), Some("F"));
        let a72 = t72.step[0].assert.as_ref().unwrap();
        let all72 = a72.stdout_contains_all.as_ref().unwrap();
        assert_eq!(all72.len(), 2);
        assert!(all72.contains(&"remi.conary.io".to_string()));
        assert!(all72.contains(&"(default)".to_string()));

        // Verify T73 sets custom channel and checks for it
        let t73 = manifest.test.iter().find(|t| t.id == "T73").unwrap();
        assert_eq!(t73.step.len(), 2, "T73 should have 2 steps (set + verify)");

        // Verify T75 checks for version info
        let t75 = manifest.test.iter().find(|t| t.id == "T75").unwrap();
        let a75 = t75.step[0].assert.as_ref().unwrap();
        let all75 = a75.stdout_contains_all.as_ref().unwrap();
        assert!(all75.contains(&"Current version:".to_string()));
        assert!(all75.contains(&"Update channel:".to_string()));

        // Verify T76 exercises the HTTPS-only update-channel guard.
        let t76 = manifest.test.iter().find(|t| t.id == "T76").unwrap();
        let t76_command = t76.step[0]
            .conary
            .as_deref()
            .expect("T76 should use a conary command");
        assert!(
            t76_command.contains("system update-channel set http://"),
            "T76 should attempt a plain HTTP update channel"
        );
        let a76 = t76.step[0].assert.as_ref().unwrap();
        assert_eq!(
            a76.exit_code_not,
            Some(0),
            "T76 should require update-channel rejection"
        );
        assert_eq!(
            a76.stderr_contains.as_deref(),
            Some("Update channel URL must use https://")
        );
    }
}

#[test]
fn test_load_phase3_group_g_manifest_data_integrity_expectations() {
    let path = remi_manifest_path("phase3-group-g.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 3);

        let t81 = manifest.test.iter().find(|test| test.id == "T81").unwrap();
        let native_assertion = t81.step[0].assert.as_ref().unwrap();
        assert_eq!(
            native_assertion.stderr_contains.as_deref(),
            Some("Failed"),
            "T81 should assert the corrupted native package is rejected without depending on parser-specific wording"
        );

        let t83 = manifest.test.iter().find(|test| test.id == "T83").unwrap();
        let corrupt_step = t83.step[1]
            .run
            .as_deref()
            .expect("T83 should manipulate a CAS object directly");
        assert!(
            corrupt_step.contains("rm -f"),
            "T83 should remove a CAS object because system verify checks object presence"
        );
        assert!(
            corrupt_step.contains("sqlite3 ${DB_PATH}")
                && corrupt_step.contains("/usr/share/conary-test/hello.txt"),
            "T83 should remove the CAS object referenced by the installed fixture file"
        );
        assert!(
            !corrupt_step.contains("find /var/lib/conary/objects"),
            "T83 should not delete an arbitrary first CAS object"
        );
        assert!(
            !corrupt_step.contains("printf 'corrupt'"),
            "T83 should not overwrite CAS object contents when verify only checks presence"
        );
        let verify_assertion = t83.step[2].assert.as_ref().unwrap();
        assert_eq!(
            verify_assertion.stdout_contains.as_deref(),
            Some("MISSING from CAS")
        );

        let t84 = manifest.test.iter().find(|test| test.id == "T84").unwrap();
        let assertion = t84.step[0].assert.as_ref().unwrap();
        assert_eq!(
            assertion.stderr_contains.as_deref(),
            Some("authority"),
            "T84 should accept the current signed-authority mismatch rejection"
        );
    }
}

#[test]
fn test_load_phase3_group_h_manifest_post_install_failure_expectations() {
    let path = remi_manifest_path("phase3-group-h.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 3);

        let t93 = manifest.test.iter().find(|test| test.id == "T93").unwrap();
        let install_assertion = t93.step[0].assert.as_ref().unwrap();
        assert_eq!(
            install_assertion.exit_code_not,
            Some(0),
            "T93 should expect a post-install script failure to fail the install"
        );
        assert_eq!(
            install_assertion.stderr_contains.as_deref(),
            Some("post-install hooks failed")
        );

        let status_step = t93.step[1]
            .run
            .as_deref()
            .expect("T93 should query package state after the failed install");
        assert!(
            status_step.contains("sqlite3 ${DB_PATH}")
                && status_step.contains("FROM troves")
                && status_step.contains("failing-scriptlet"),
            "T93 should verify package state is absent from the DB"
        );
        let status_assertion = t93.step[1].assert.as_ref().unwrap();
        assert_eq!(
            status_assertion.stdout_contains.as_deref(),
            Some("state-absent")
        );

        assert_eq!(
            t93.step[2].file_not_exists.as_deref(),
            Some("/usr/bin/failing-scriptlet"),
            "T93 should verify the package payload was rolled back"
        );
    }
}

#[test]
fn test_load_phase3_group_i_manifest_security_boundary_expectations() {
    let path = remi_manifest_path("phase3-group-i.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 3);

        for test_id in ["T97", "T98"] {
            let test = manifest
                .test
                .iter()
                .find(|test| test.id == test_id)
                .unwrap();
            let assertion = test.step[0].assert.as_ref().unwrap();
            assert_eq!(
                assertion.stderr_contains.as_deref(),
                Some("authority"),
                "{test_id} should accept early signed-authority validation rejection"
            );
        }

        for (test_id, package_name, payload_path) in [
            ("T100", "proc-environ", "/usr/bin/proc-environ"),
            (
                "T101",
                "adversarial-hostile-scriptlet",
                "/usr/bin/hostile-scriptlet",
            ),
            ("T102", "outside-root-write", "/usr/bin/outside-root-write"),
        ] {
            let test = manifest
                .test
                .iter()
                .find(|test| test.id == test_id)
                .unwrap();
            let install_assertion = test.step[0].assert.as_ref().unwrap();
            assert_eq!(
                install_assertion.exit_code_not,
                Some(0),
                "{test_id} should expect a sandbox scriptlet failure to fail the install"
            );
            assert_eq!(
                install_assertion.stderr_contains.as_deref(),
                Some("post-install hooks failed")
            );

            let status_step = test.step[1]
                .run
                .as_deref()
                .expect("sandbox scriptlet tests should query package state");
            assert!(
                status_step.contains("sqlite3 ${DB_PATH}")
                    && status_step.contains("FROM troves")
                    && status_step.contains(package_name),
                "{test_id} should verify package state is absent from the DB"
            );
            let status_assertion = test.step[1].assert.as_ref().unwrap();
            assert_eq!(
                status_assertion.stdout_contains.as_deref(),
                Some("state-absent")
            );
            assert!(
                test.step
                    .iter()
                    .any(|step| step.file_not_exists.as_deref() == Some(payload_path)),
                "{test_id} should verify the package payload was rolled back"
            );
        }
    }
}

#[test]
fn test_load_phase3_group_l_manifest_self_update_http_mocks_seed_db_directly() {
    let path = remi_manifest_path("phase3-group-l.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 3);

        for id in ["T128", "T129", "T130", "T130a"] {
            let test = manifest
                .test
                .iter()
                .find(|test| test.id == id)
                .expect("expected self-update lifecycle test");
            let script = test.step[0]
                .run
                .as_deref()
                .expect("self-update lifecycle tests should use shell orchestration");
            assert!(
                script.contains("INSERT INTO settings")
                    && script.contains("'update-channel'")
                    && script.contains("http://127.0.0.1:"),
                "{id} should seed the mock HTTP update channel directly into the test DB"
            );
            assert!(
                !script.contains("system update-channel set http://"),
                "{id} should not ask the HTTPS-only CLI to accept a plain HTTP channel"
            );
            assert!(
                !script.contains("self-update --no-verify"),
                "{id} must not retain the removed self-update trust bypass"
            );
        }
    }
}

#[test]
fn test_load_phase3_group_m_manifest_installs_local_fixture_ccs() {
    let path = remi_manifest_path("phase3-group-m.toml");
    if path.exists() {
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.suite.phase, 3);

        let t138 = manifest.test.iter().find(|test| test.id == "T138").unwrap();
        let install_step = t138
            .step
            .iter()
            .filter_map(|step| step.conary.as_deref())
            .find(|command| command.contains("install "))
            .expect("T138 should install the local fixture CCS");
        assert!(
            install_step.contains("ccs install ${FIXTURE_V1_CCS}"),
            "T138 should install the generated local CCS fixture"
        );
        assert!(
            !install_step.contains("${FIXTURE_PKG_NAME} --repo"),
            "T138 should not ask Remi metadata for the local fixture package"
        );

        assert_eq!(
            manifest
                .distro_overrides
                .get("arch")
                .and_then(|overrides| overrides.get("dep_heavy_package"))
                .map(String::as_str),
            Some("jq"),
            "Arch should use a smaller dependency-heavy package than vim to keep Forge runtime bounded"
        );
        assert_eq!(
            manifest
                .distro_overrides
                .get("ubuntu-26.04")
                .and_then(|overrides| overrides.get("small_package"))
                .map(String::as_str),
            Some("patch"),
            "Ubuntu should use a package present in its Remi preview lane"
        );
        assert_eq!(
            manifest
                .distro_overrides
                .get("ubuntu-26.04")
                .and_then(|overrides| overrides.get("small_binary"))
                .map(String::as_str),
            Some("/usr/bin/patch"),
            "Ubuntu should verify the binary for its selected small package"
        );

        for id in ["T141", "T144", "T147", "T148", "T149"] {
            let test = manifest.test.iter().find(|test| test.id == id).unwrap();
            let cleanup_step = test.step[0]
                .run
                .as_deref()
                .unwrap_or_else(|| panic!("{id} should remove any fixture left by earlier tests"));
            assert!(
                cleanup_step.contains("remove ${FIXTURE_PKG_NAME}")
                    && cleanup_step.contains("--db-path ${DB_PATH}"),
                "{id} should start from a clean fixture install state"
            );
        }

        for id in ["T144", "T147", "T149"] {
            let test = manifest.test.iter().find(|test| test.id == id).unwrap();
            let install_step = test
                .step
                .iter()
                .filter_map(|step| step.conary.as_deref())
                .find(|command| command.contains("install "))
                .expect("fixture lifecycle test should install the fixture");
            assert!(
                install_step.contains("ccs install ${FIXTURE_V1_CCS}"),
                "{id} should install the local CCS fixture instead of resolving fixture metadata from Remi"
            );
            assert!(
                !install_step.contains("${FIXTURE_PKG_NAME} --repo"),
                "{id} should not ask Remi metadata for the local fixture package"
            );
        }

        let uses_small_binary = manifest
            .test
            .iter()
            .flat_map(|test| &test.step)
            .any(|step| {
                step.file_exists.as_deref() == Some("${small_binary}")
                    || step.file_not_exists.as_deref() == Some("${small_binary}")
                    || step
                        .run
                        .as_deref()
                        .is_some_and(|script| script.contains("${small_binary}"))
            });
        assert!(
            uses_small_binary,
            "Group M should verify distro-specific binaries via small_binary rather than hard-coded /usr/bin/tree"
        );

        for assertion in manifest
            .test
            .iter()
            .flat_map(|test| &test.step)
            .filter_map(|step| step.assert.as_ref())
        {
            assert_ne!(
                assertion.stdout_not_contains.as_deref(),
                Some("panic"),
                "Group M should avoid bare panic checks that match package filenames such as pvpanic.h"
            );
        }
    }
}
