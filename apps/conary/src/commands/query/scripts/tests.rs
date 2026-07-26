// apps/conary/src/commands/query/scripts/tests.rs

#[cfg(test)]
mod query_scripts {
    use super::super::*;
    use conary_core::ccs::native_lifecycle::{
        LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1,
        NativeInvocation, NativeLifecycleBundle, NativeLifecycleEntry, NativeLifecycleEntryKind,
        ScriptletFidelity, SourceFormat, TransactionOrder, VersionScheme,
    };

    fn bundle_fixture() -> NativeLifecycleBundle {
        let post_install_body = "systemctl daemon-reload\n";
        let pre_remove_body = "ldconfig\n";
        NativeLifecycleBundle {
            schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
            schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_profile: Some("fedora-44".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "nginx".to_string(),
            source_version: "1.28.0-1.fc44".to_string(),
            source_checksum: Some(
                "sha256:3333333333333333333333333333333333333333333333333333333333333333"
                    .to_string(),
            ),
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "remi".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "typed-native-lifecycle".to_string(),
            evidence_digest: Some(
                "sha256:5555555555555555555555555555555555555555555555555555555555555555"
                    .to_string(),
            ),
            scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
            entries: vec![
                entry_fixture("rpm:%preun", pre_remove_body),
                entry_fixture("rpm:%post", post_install_body),
            ],
        }
    }

    fn entry_fixture(id: &str, body: &str) -> NativeLifecycleEntry {
        NativeLifecycleEntry {
            id: id.to_string(),
            native_slot: id.split(':').nth(1).unwrap_or("%post").to_string(),
            kind: NativeLifecycleEntryKind::Executable,
            phase: if id.ends_with("%preun") {
                LifecyclePath::PreRemove
            } else {
                LifecyclePath::PostInstall
            },
            lifecycle_paths: vec!["install:first".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: vec!["-e".to_string()],
            body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation {
                args: vec!["1".to_string()],
                environment: vec!["RPM_INSTALL_PREFIX=/".to_string()],
                stdin: Some("none".to_string()),
                chroot: Some("install-root".to_string()),
            },
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                before: vec![],
                after: vec!["payload".to_string()],
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: vec!["ldconfig".to_string()],
            evidence_digest: Some(
                "sha256:6666666666666666666666666666666666666666666666666666666666666666"
                    .to_string(),
            ),
            source_evidence_refs: vec!["capture:rpm:%post".to_string()],
            rpm_trigger: None,
            rpm_runtime: Some(conary_core::ccs::native_lifecycle::RpmRuntimeMetadata {
                program: conary_core::ccs::native_lifecycle::RpmProgram::External,
                body_transforms: Vec::new(),
                critical: false,
                criticality: conary_core::ccs::native_lifecycle::RpmCriticality::WarningOnly,
                raw_flags: 0,
                unknown_flags: 0,
                install_prefixes: Vec::new(),
                macro_context: Default::default(),
                header_context: Default::default(),
                package_rpm_version: None,
            }),
            rpm_sysusers: None,
            deb_maintainer: None,
            arch_install: None,
            arch_hook: None,
            residual_lifecycle: None,
        }
    }

    fn package_identity() -> PackageQueryIdentity {
        PackageQueryIdentity {
            name: "nginx".to_string(),
            version: "1.28.0".to_string(),
        }
    }

    fn signed_ccs_fixture(
        temp: &tempfile::TempDir,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        conary_core::ccs::SigningKeyPair,
    ) {
        let source = temp.path().join("source");
        std::fs::create_dir_all(source.join("usr/bin")).unwrap();
        std::fs::write(source.join("usr/bin/query-demo"), b"demo\n").unwrap();
        let result = conary_core::ccs::CcsBuilder::new(
            conary_core::ccs::CcsManifest::new_minimal("query-demo", "1.0.0"),
            &source,
        )
        .unwrap()
        .build()
        .unwrap();
        let package_path = temp.path().join("query-demo.ccs");
        let policy_path = temp.path().join("policy.toml");
        let signer =
            conary_core::ccs::SigningKeyPair::generate().with_key_id("query-test-authority");
        conary_core::ccs::builder::write_signed_current_ccs_package(
            &result,
            &package_path,
            &signer,
            false,
        )
        .unwrap();
        std::fs::write(
            &policy_path,
            format!("trusted_keys = [\"{}\"]\n", signer.public_key_base64()),
        )
        .unwrap();
        (package_path, policy_path, signer)
    }

    #[test]
    fn script_query_summary_renders_bundle_counts() {
        let output = render_ccs_bundle_text(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions::default(),
        )
        .expect("render summary");

        assert!(output.contains("Package: nginx 1.28.0"));
        assert!(output.contains("Native lifecycle bundle: conary.native-lifecycles.v1"));
        assert!(output.contains("Native entries: 2"));
        assert!(output.contains("rpm:%post"));
        assert!(!output.contains("systemctl daemon-reload"));
    }

    #[test]
    fn script_query_verbose_renders_entry_details() {
        let output = render_ccs_bundle_text(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions {
                verbose: true,
                ..ScriptQueryOptions::default()
            },
        )
        .expect("render verbose");

        assert!(output.contains("Interpreter: /bin/sh"));
        assert!(output.contains("Timeout: 30000ms"));
        assert!(output.contains("body_sha256="));
        assert!(!output.contains("systemctl daemon-reload"));
    }

    #[test]
    fn script_query_entry_filter_renders_one_entry() {
        let output = render_ccs_bundle_text(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions {
                entry: Some("rpm:%post".to_string()),
                ..ScriptQueryOptions::default()
            },
        )
        .expect("render entry");

        assert!(output.contains("rpm:%post"));
        assert!(!output.contains("rpm:%preun"));
    }

    #[test]
    fn script_query_json_omits_raw_bodies_by_default() {
        let output = render_ccs_bundle_json(
            &package_identity(),
            Some(&bundle_fixture()),
            &ScriptQueryOptions::default(),
        )
        .expect("render json");
        let json: serde_json::Value = serde_json::from_str(&output).expect("valid json");

        assert_eq!(json["bundle_present"], true);
        assert!(output.contains("body_sha256"));
        assert!(!output.contains("systemctl daemon-reload"));
        assert!(json["entries"][0].get("body").is_none());
    }

    #[test]
    fn script_query_json_reports_no_bundle_without_entries() {
        let output =
            render_ccs_bundle_json(&package_identity(), None, &ScriptQueryOptions::default())
                .expect("render json");
        let json: serde_json::Value = serde_json::from_str(&output).expect("valid json");

        assert_eq!(json["bundle_present"], false);
        assert!(json["bundle"].is_null());
        assert!(
            json["entries"]
                .as_array()
                .expect("entries array")
                .is_empty()
        );
    }

    #[test]
    fn script_query_json_reports_zero_entry_bundle() {
        let mut bundle = bundle_fixture();
        bundle.entries.clear();
        bundle.scriptlet_fidelity = ScriptletFidelity::NativeFree;

        let output = render_ccs_bundle_json(
            &package_identity(),
            Some(&bundle),
            &ScriptQueryOptions::default(),
        )
        .expect("render json");
        let json: serde_json::Value = serde_json::from_str(&output).expect("valid json");

        assert_eq!(json["bundle_present"], true);
        assert!(
            json["entries"]
                .as_array()
                .expect("entries array")
                .is_empty()
        );
    }

    #[test]
    fn ccs_file_query_requires_explicit_trust_policy() {
        let temp = tempfile::tempdir().unwrap();
        let (package_path, policy_path, _) = signed_ccs_fixture(&temp);
        let package_path = package_path.to_string_lossy();

        let missing_policy =
            load_verified_ccs_package(&package_path, &ScriptQueryOptions::default()).unwrap_err();
        assert!(missing_policy.to_string().contains("requires --policy"));

        let package = load_verified_ccs_package(
            &package_path,
            &ScriptQueryOptions {
                policy_path: Some(policy_path.to_string_lossy().into_owned()),
                ..ScriptQueryOptions::default()
            },
        )
        .unwrap();
        assert_eq!(package.manifest().package.name, "query-demo");
    }

    #[test]
    fn ccs_file_query_rejects_untrusted_signer() {
        let temp = tempfile::tempdir().unwrap();
        let (package_path, policy_path, _) = signed_ccs_fixture(&temp);
        let untrusted = conary_core::ccs::SigningKeyPair::generate();
        std::fs::write(
            &policy_path,
            format!("trusted_keys = [\"{}\"]\n", untrusted.public_key_base64()),
        )
        .unwrap();

        let error = load_verified_ccs_package(
            &package_path.to_string_lossy(),
            &ScriptQueryOptions {
                policy_path: Some(policy_path.to_string_lossy().into_owned()),
                ..ScriptQueryOptions::default()
            },
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("not trusted"));
    }
}
