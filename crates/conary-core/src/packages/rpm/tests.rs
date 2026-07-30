// conary-core/src/packages/rpm/tests.rs

use super::*;
use crate::packages::traits::{
    NativeArgumentValue, NativeLifecyclePath, NativeScriptletMetadata, NativeStdinContract,
    RpmTriggerAction,
};

#[test]
fn test_to_trove_conversion() {
    // Create a minimal RpmPackage for testing
    let rpm = RpmPackage {
        meta: PackageMetadata::new(
            PathBuf::from("/fake/path.rpm"),
            "test-package".to_string(),
            "1.0.0".to_string(),
            VersionScheme::Rpm,
        ),
        source_rpm: Some("test-package-1.0.0.src.rpm".to_string()),
        build_host: Some("buildhost.example.com".to_string()),
        vendor: Some("Test Vendor".to_string()),
        license: Some("MIT".to_string()),
        url: Some("https://example.com".to_string()),
        payload: PackagePayload::default(),
    };

    let trove = rpm.to_trove();

    assert_eq!(trove.name, "test-package");
    assert_eq!(trove.version, "1.0.0");
}

#[test]
fn test_provenance_accessors() {
    let rpm = RpmPackage {
        meta: PackageMetadata::new(
            PathBuf::from("/fake/test.rpm"),
            "test".to_string(),
            "1.0".to_string(),
            VersionScheme::Rpm,
        ),
        source_rpm: Some("test-1.0.src.rpm".to_string()),
        build_host: Some("builder".to_string()),
        vendor: Some("Vendor".to_string()),
        license: Some("GPL".to_string()),
        url: Some("https://test.com".to_string()),
        payload: PackagePayload::default(),
    };

    assert_eq!(rpm.source_rpm(), Some("test-1.0.src.rpm"));
    assert_eq!(rpm.build_host(), Some("builder"));
    assert_eq!(rpm.vendor(), Some("Vendor"));
    assert_eq!(rpm.license(), Some("GPL"));
    assert_eq!(rpm.url(), Some("https://test.com"));
}

#[test]
fn test_parse_nonexistent_file() {
    // Test that parsing a nonexistent file returns an error
    let result = RpmPackage::parse("/nonexistent/file.rpm");
    assert!(result.is_err());
}

#[test]
fn rpm_artifact_parser_preserves_nonzero_epoch_in_version_identity() {
    let temp_dir = tempfile::tempdir().unwrap();
    let package_path = temp_dir.path().join("epoch-fixture.rpm");
    let mut builder = rpm::PackageBuilder::new(
        "epoch-fixture",
        "2.0",
        "MIT",
        "x86_64",
        "RPM epoch identity fixture",
    );
    builder.epoch(7).release("3.fc44");
    builder.build().unwrap().write_file(&package_path).unwrap();

    let parsed = RpmPackage::parse(package_path.to_str().unwrap()).unwrap();

    assert_eq!(parsed.version(), "7:2.0-3.fc44");
}

#[test]
fn rpm_artifact_requirement_canonicalizes_empty_epoch_at_typed_boundary() {
    let mut builder = rpm::PackageBuilder::new(
        "empty-epoch-consumer",
        "1",
        "MIT",
        "x86_64",
        "RPM empty requirement epoch fixture",
    );
    builder.requires(rpm::Dependency {
        name: "device-mapper-libs".to_string(),
        flags: DependencyFlags::EQUAL,
        version: ":1.02.212-2.fc44".to_string(),
    });

    let requirements = RpmPackage::extract_requirements(&builder.build().unwrap()).unwrap();

    assert_eq!(
        requirements[0].native_text.as_deref(),
        Some("device-mapper-libs = 1.02.212-2.fc44")
    );
}

#[test]
fn rpm_header_relations_preserve_conflicts_and_obsoletes_as_typed_authority() {
    let mut builder =
        rpm::PackageBuilder::new("newpkg", "2", "MIT", "x86_64", "package relation fixture");
    builder
        .conflicts(rpm::Dependency::less("old-conflict", "2"))
        .obsoletes(rpm::Dependency::less_eq("old-owner", "1"));
    let package = builder.build().unwrap();

    let relations = RpmPackage::extract_relations(&package).unwrap();

    assert_eq!(
        relations
            .iter()
            .map(|relation| relation.kind)
            .collect::<Vec<_>>(),
        vec![
            RepositoryRequirementKind::Conflict,
            RepositoryRequirementKind::Obsolete,
        ]
    );
    assert_eq!(
        relations
            .iter()
            .map(|relation| relation.native_text.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("old-conflict < 2"), Some("old-owner <= 1")]
    );
}

#[test]
fn rpm_header_relation_rejects_comparison_without_version() {
    let mut builder =
        rpm::PackageBuilder::new("newpkg", "2", "MIT", "x86_64", "malformed relation fixture");
    builder.conflicts(rpm::Dependency {
        name: "oldpkg".to_string(),
        flags: DependencyFlags::GREATER | DependencyFlags::EQUAL,
        version: String::new(),
    });
    let package = builder.build().unwrap();

    let error = RpmPackage::extract_relations(&package).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("comparison flags but no version")
    );
}

#[test]
fn rpm_runtime_features_are_validated_without_becoming_package_requirements() {
    let mut builder =
        rpm::PackageBuilder::new("feature-consumer", "1", "MIT", "x86_64", "feature fixture");
    builder
        .requires(rpm::Dependency {
            name: "rpmlib(CompressedFileNames)".to_string(),
            flags: DependencyFlags::RPMLIB | DependencyFlags::LESS | DependencyFlags::EQUAL,
            version: "3.0.4-1".to_string(),
        })
        .requires(rpm::Dependency {
            name: "config(feature-consumer)".to_string(),
            flags: DependencyFlags::CONFIG | DependencyFlags::EQUAL,
            version: "1-1".to_string(),
        });
    let package = builder.build().unwrap();

    let requirements = RpmPackage::extract_requirements(&package).unwrap();
    assert_eq!(requirements.len(), 1);
    assert_eq!(
        requirements[0].native_text.as_deref(),
        Some("config(feature-consumer) = 1-1")
    );
    assert!(
        requirements
            .iter()
            .flat_map(|group| group.expression.atoms())
            .all(|clause| !clause.name.starts_with("rpmlib("))
    );
}

#[test]
fn rpm_artifact_rejects_unimplemented_runtime_feature() {
    let mut builder = rpm::PackageBuilder::new(
        "feature-consumer",
        "1",
        "MIT",
        "x86_64",
        "unsupported feature fixture",
    );
    builder.requires(rpm::Dependency {
        name: "rpmlib(ConcurrentAccess)".to_string(),
        flags: DependencyFlags::RPMLIB | DependencyFlags::LESS | DependencyFlags::EQUAL,
        version: "4.1-1".to_string(),
    });

    let error = RpmPackage::extract_requirements(&builder.build().unwrap()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not implement RPM runtime capability"),
        "{error}"
    );
}

#[test]
fn rpm_artifact_rejects_short_circuited_build_marker() {
    let mut builder = rpm::PackageBuilder::new(
        "short-circuited",
        "1",
        "MIT",
        "x86_64",
        "invalid short-circuit build fixture",
    );
    builder.requires(rpm::Dependency {
        name: "rpmlib(ShortCircuited)".to_string(),
        flags: DependencyFlags::RPMLIB | DependencyFlags::LESS | DependencyFlags::EQUAL,
        version: "4.9.0-1".to_string(),
    });

    let error = RpmPackage::extract_requirements(&builder.build().unwrap()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("packaged without completing an RPM build phase"),
        "{error}"
    );
}

#[test]
fn rpm_runtime_true_short_circuits_a_mixed_rich_or_requirement() {
    let mut builder = rpm::PackageBuilder::new(
        "feature-consumer",
        "1",
        "MIT",
        "x86_64",
        "mixed rich runtime fixture",
    );
    builder.requires(rpm::Dependency::any(
        "(missing-provider or rpmlib(CompressedFileNames) <= 3.0.4-1)",
    ));

    let requirements = RpmPackage::extract_requirements(&builder.build().unwrap()).unwrap();

    assert!(requirements.is_empty());
}

#[test]
fn rpm_config_dependency_and_provide_remain_package_capability_authority() {
    let mut builder =
        rpm::PackageBuilder::new("config-owner", "1", "MIT", "x86_64", "config fixture");
    builder
        .requires(rpm::Dependency::config("config-owner", "1-1"))
        .provides(rpm::Dependency::config("config-owner", "1-1"));
    let package = builder.build().unwrap();

    let requirements = RpmPackage::extract_requirements(&package).unwrap();
    let provides = RpmPackage::extract_provides(&package).unwrap();

    assert!(requirements.iter().any(|requirement| {
        requirement.native_text.as_deref() == Some("config(config-owner) = 1-1")
    }));
    assert!(provides.iter().any(|provide| {
        provide.name == "config(config-owner)" && provide.version.as_deref() == Some("1-1")
    }));
}

#[test]
fn rpm_package_cannot_forge_package_manager_runtime_provides() {
    let mut builder =
        rpm::PackageBuilder::new("forger", "1", "MIT", "x86_64", "runtime forgery fixture");
    builder.provides(rpm::Dependency::rpmlib("CompressedFileNames", "3.0.4-1"));

    let error = RpmPackage::extract_provides(&builder.build().unwrap()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("typed runtime ledger is the sole authority"),
        "{error}"
    );
}

#[test]
fn rpm_reserved_sense_bits_require_their_exact_namespaces() {
    let mut bad_runtime =
        rpm::PackageBuilder::new("bad-runtime", "1", "MIT", "x86_64", "bad runtime sense");
    bad_runtime.requires(rpm::Dependency {
        name: "ordinary-package".to_string(),
        flags: DependencyFlags::RPMLIB,
        version: String::new(),
    });
    assert!(
        RpmPackage::extract_requirements(&bad_runtime.build().unwrap())
            .unwrap_err()
            .to_string()
            .contains("outside the rpmlib(Feature) grammar")
    );

    let mut bad_config =
        rpm::PackageBuilder::new("bad-config", "1", "MIT", "x86_64", "bad config sense");
    bad_config.provides(rpm::Dependency {
        name: "ordinary-capability".to_string(),
        flags: DependencyFlags::CONFIG,
        version: String::new(),
    });
    assert!(
        RpmPackage::extract_provides(&bad_config.build().unwrap())
            .unwrap_err()
            .to_string()
            .contains("outside the config(Name) grammar")
    );
}

#[test]
fn rpm_file_requirement_remains_an_exact_capability() {
    let mut builder = rpm::PackageBuilder::new(
        "shell-user",
        "1",
        "MIT",
        "x86_64",
        "file requirement fixture",
    );
    builder.requires(rpm::Dependency::any("/bin/sh"));
    let package = builder.build().unwrap();

    let requirements = RpmPackage::extract_requirements(&package).unwrap();
    assert_eq!(requirements.len(), 1);
    assert_eq!(requirements[0].alternatives[0].name, "/bin/sh");
}

#[test]
fn rpm_rich_dependency_uses_the_formal_requirement_contract() {
    let mut builder = rpm::PackageBuilder::new(
        "rich-user",
        "1",
        "MIT",
        "x86_64",
        "rich requirement fixture",
    );
    builder.requires(rpm::Dependency::any("(foo if bar)"));
    let package = builder.build().unwrap();

    let requirements = RpmPackage::extract_requirements(&package).unwrap();
    assert!(matches!(
        requirements[0].expression,
        crate::repository::dependency_model::RepositoryRequirementExpression::If { .. }
    ));

    let mut malformed =
        rpm::PackageBuilder::new("bad-rich", "1", "MIT", "x86_64", "bad rich fixture");
    malformed.requires(rpm::Dependency::any("(foo if bar"));
    assert!(RpmPackage::extract_requirements(&malformed.build().unwrap()).is_err());
}

#[test]
fn rpm_program_vector_splits_interpreter_and_args() {
    let scriptlet =
        rpm::Scriptlet::new("echo posttrans").prog(vec!["/usr/lib/rpm/lua", "--", "script.lua"]);

    let (interpreter, args) = RpmPackage::rpm_scriptlet_program(&scriptlet);

    assert_eq!(interpreter, "/usr/lib/rpm/lua");
    assert_eq!(args, vec!["--".to_string(), "script.lua".to_string()]);
}

#[test]
fn rpm_scriptlet_flags_preserve_names_and_bits() {
    let flags = RpmPackage::rpm_scriptlet_flags_metadata(rpm::ScriptletFlags::EXPAND, false, false);

    assert!(flags.names.contains(&"EXPAND".to_string()));
    assert_eq!(flags.raw_bits, rpm::ScriptletFlags::EXPAND.bits());
    assert!(flags.expand);
    assert!(!flags.query_format);
    assert!(!flags.critical);
    assert_eq!(flags.criticality, RpmScriptletCriticality::WarningOnly);
}

#[test]
fn rpm_effective_criticality_uses_slot_defaults_and_file_trigger_override() {
    let defaulted = RpmPackage::rpm_scriptlet_flags_metadata(
        rpm::ScriptletFlags::from_bits_retain(0),
        true,
        false,
    );
    assert!(defaulted.critical);
    assert_eq!(defaulted.criticality, RpmScriptletCriticality::SlotDefault);

    let forced_warning =
        RpmPackage::rpm_scriptlet_flags_metadata(rpm::ScriptletFlags::CRITICAL, true, true);
    assert!(!forced_warning.critical);
    assert_eq!(
        forced_warning.criticality,
        RpmScriptletCriticality::ForcedWarningOnly
    );
}

#[test]
fn rpm_sysusers_provides_become_typed_pre_payload_entries() {
    let mut builder = rpm::PackageBuilder::new(
        "sysusers-fixture",
        "1.0.0",
        "MIT",
        "x86_64",
        "sysusers fixture",
    );
    builder
        .with_file_contents(
            "u! dnsmasq - \"Dnsmasq service\" /var/lib/dnsmasq\n\
             g input -\n\
             m dnsmasq input\n",
            rpm::FileOptions::new("/usr/lib/sysusers.d/sysusers-fixture.conf"),
        )
        .unwrap();
    let package = builder.build().unwrap();

    let groups = RpmPackage::extract_rpm_sysusers(&package).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].source_path.as_deref(),
        Some("/usr/lib/sysusers.d/sysusers-fixture.conf")
    );
    assert_eq!(groups[0].directives.len(), 3);
    assert!(matches!(
        groups[0]
            .directives
            .iter()
            .find(|directive| matches!(directive, RpmSysusersDirective::User { .. }))
            .expect("user directive"),
        RpmSysusersDirective::User {
            name,
            locked: true,
            ..
        } if name == "dnsmasq"
    ));

    let entries = RpmPackage::extract_native_scriptlet_abi(&package).unwrap();
    let sysusers = entries
        .iter()
        .find(|entry| entry.native_slot == "%sysusers")
        .expect("typed sysusers entry");
    assert_eq!(sysusers.primary_lifecycle, NativeLifecyclePath::Sysusers);
    assert_eq!(sysusers.invocation.stdin, NativeStdinContract::Sysusers);
    let NativeScriptletMetadata::Rpm(metadata) = &sysusers.metadata else {
        panic!("RPM metadata");
    };
    assert_eq!(metadata.runtime.program, RpmScriptletProgram::Sysusers);
    assert!(metadata.runtime.flags.critical);
}

#[test]
fn rpm_trigger_action_and_comparison_are_split_from_raw_flags() {
    let flags = rpm::DependencyFlags::TRIGGERPOSTUN | rpm::DependencyFlags::LE;

    assert_eq!(
        RpmPackage::rpm_trigger_action(flags),
        RpmTriggerAction::PostUninstall
    );
    assert_eq!(
        RpmPackage::rpm_trigger_comparison(flags),
        Some("<=".to_string())
    );

    let unknown = RpmPackage::rpm_trigger_action(rpm::DependencyFlags::from_bits_retain(0));
    assert_eq!(
        unknown,
        RpmTriggerAction::Unknown {
            raw_flags: rpm::DependencyFlags::from_bits_retain(0).bits(),
        }
    );
}

#[test]
fn rpm_native_abi_preserves_untransaction_verify_and_all_trigger_actions() {
    let mut builder = rpm::PackageBuilder::new(
        "native-abi-fixture",
        "1.0.0",
        "MIT",
        "x86_64",
        "native abi fixture",
    );
    builder
        .pre_install_script("echo pre")
        .post_install_script("echo post")
        .pre_uninstall_script("echo preun")
        .post_uninstall_script("echo postun")
        .pre_trans_script("echo pretrans")
        .post_trans_script("echo posttrans")
        .pre_untrans_script("echo preuntrans")
        .post_untrans_script("echo postuntrans")
        .verify_script("echo verify")
        .trigger_prein("bash", None, "echo triggerprein")
        .trigger_in(
            "bash",
            Some((rpm::DependencyFlags::GREATER, "5.0")),
            "echo triggerin",
        )
        .trigger_un("bash", None, "echo triggerun")
        .trigger_postun("bash", None, "echo triggerpostun")
        .file_trigger_in("/usr/lib", None, "echo filetriggerin")
        .file_trigger_un("/usr/lib", None, "echo filetriggerun")
        .file_trigger_postun("/usr/lib", None, "echo filetriggerpostun")
        .trans_file_trigger_in("/usr/bin", None, "echo transfiletriggerin")
        .trans_file_trigger_un("/usr/bin", None, "echo transfiletriggerun")
        .trans_file_trigger_postun("/usr/bin", None, "echo transfiletriggerpostun");

    let package = builder.build().expect("fixture rpm package");
    let entries = RpmPackage::extract_native_scriptlet_abi(&package).unwrap();
    let slots: Vec<_> = entries
        .iter()
        .map(|entry| entry.native_slot.as_str())
        .collect();

    for slot in [
        "%pre",
        "%post",
        "%preun",
        "%postun",
        "%pretrans",
        "%posttrans",
        "%preuntrans",
        "%postuntrans",
        "%verify",
        "%triggerprein",
        "%triggerin",
        "%triggerun",
        "%triggerpostun",
        "%filetriggerin",
        "%filetriggerun",
        "%filetriggerpostun",
        "%transfiletriggerin",
        "%transfiletriggerun",
        "%transfiletriggerpostun",
    ] {
        assert!(slots.contains(&slot), "missing native slot {slot}");
    }

    let verify = entries
        .iter()
        .find(|entry| entry.native_slot == "%verify")
        .expect("verify entry");
    assert_eq!(verify.primary_lifecycle, NativeLifecyclePath::Verify);
    assert_eq!(verify.support.reason_code(), None);
    let NativeScriptletMetadata::Rpm(verify_metadata) = &verify.metadata else {
        panic!("expected rpm metadata");
    };
    assert!(verify_metadata.runtime.flags.critical);

    let pre = entries
        .iter()
        .find(|entry| entry.native_slot == "%pre")
        .expect("pre entry");
    assert_eq!(
        pre.lifecycle_paths,
        vec![
            NativeLifecyclePath::PreInstall,
            NativeLifecyclePath::PreUpgrade
        ]
    );
    assert_eq!(
        pre.invocation.args[0].value,
        NativeArgumentValue::PackageInstanceCount
    );

    let preun = entries
        .iter()
        .find(|entry| entry.native_slot == "%preun")
        .expect("preun entry");
    assert_eq!(
        preun.lifecycle_paths,
        vec![
            NativeLifecyclePath::PreRemove,
            NativeLifecyclePath::PreUpgrade
        ]
    );

    let file_trigger = entries
        .iter()
        .find(|entry| entry.native_slot == "%filetriggerin")
        .expect("file trigger entry");
    assert_eq!(file_trigger.invocation.stdin, NativeStdinContract::Paths);
    let NativeScriptletMetadata::Rpm(meta) = &file_trigger.metadata else {
        panic!("expected rpm metadata");
    };
    let trigger = meta.trigger.as_ref().expect("trigger metadata");
    assert_eq!(trigger.path_prefixes, vec!["/usr/lib".to_string()]);
    assert!(!meta.runtime.flags.critical);
    assert_eq!(
        meta.runtime.flags.criticality,
        RpmScriptletCriticality::ForcedWarningOnly
    );

    let trans_postun = entries
        .iter()
        .find(|entry| entry.native_slot == "%transfiletriggerpostun")
        .expect("transaction file trigger postun entry");
    assert_eq!(trans_postun.invocation.stdin, NativeStdinContract::None);
    assert_eq!(trans_postun.invocation.args.len(), 1);
    let NativeScriptletMetadata::Rpm(meta) = &trans_postun.metadata else {
        panic!("expected rpm metadata");
    };
    assert_eq!(
        meta.trigger
            .as_ref()
            .expect("trigger metadata")
            .path_prefixes,
        vec!["/usr/bin".to_string()]
    );
}

#[test]
fn rpm_runtime_preserves_typed_transform_and_embedded_lua_context() {
    let mut builder = rpm::PackageBuilder::new(
        "runtime-context",
        "2.3.4",
        "MIT",
        "x86_64",
        "runtime context fixture",
    );
    builder
        .with_file_contents(
            "fixture\n",
            rpm::FileOptions::new("/usr/share/runtime-context/data"),
        )
        .expect("fixture file");
    builder
        .post_install_script(
            rpm::Scriptlet::new("%%{NAME} %{name}")
                .flags(rpm::ScriptletFlags::EXPAND | rpm::ScriptletFlags::QFORMAT),
        )
        .pre_install_script(rpm::Scriptlet::new("assert(arg[2] == 1)").prog(vec!["<lua>"]));

    let package = builder.build().expect("fixture RPM");
    let entries = RpmPackage::extract_native_scriptlet_abi(&package).expect("native ABI");

    let post = entries
        .iter()
        .find(|entry| entry.native_slot == "%post")
        .expect("post entry");
    let NativeScriptletMetadata::Rpm(post) = &post.metadata else {
        panic!("RPM metadata");
    };
    assert_eq!(post.runtime.program, RpmScriptletProgram::External);
    assert!(post.runtime.flags.expand);
    assert!(post.runtime.flags.query_format);
    assert!(post.runtime.package_rpm_version.is_some());
    assert!(
        post.runtime
            .macro_context
            .definitions
            .iter()
            .any(|definition| definition.name == "name" && definition.body == "runtime-context")
    );
    assert!(post.runtime.header_context.facts.iter().any(|fact| {
        fact.name.as_deref() == Some("NAME")
            && fact.value == RpmHeaderValueMetadata::String("runtime-context".to_string())
    }));
    assert!(post.runtime.header_context.facts.iter().any(|fact| {
        fact.name.as_deref() == Some("FILENAMES")
            && fact.value
                == RpmHeaderValueMetadata::StringArray(vec![
                    "/usr/share/runtime-context/data".to_string(),
                ])
    }));
    assert!(post.runtime.header_context.facts.iter().any(|fact| {
        fact.name.as_deref() == Some("NEVRA")
            && fact.value
                == RpmHeaderValueMetadata::String("runtime-context-2.3.4-1.x86_64".to_string())
    }));

    let pre = entries
        .iter()
        .find(|entry| entry.native_slot == "%pre")
        .expect("pre entry");
    let NativeScriptletMetadata::Rpm(pre) = &pre.metadata else {
        panic!("RPM metadata");
    };
    assert_eq!(pre.runtime.program, RpmScriptletProgram::EmbeddedLua);
    assert!(!pre.runtime.macro_context.definitions.is_empty());
    assert!(pre.runtime.header_context.facts.is_empty());
}
