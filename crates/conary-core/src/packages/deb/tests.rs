// conary-core/src/packages/deb/tests.rs

use super::*;
use crate::packages::traits::{
    DebMaintainerMode, DebTriggerAwaitMode, DebTriggerDirective, NativeArgumentValue,
    NativeLifecyclePath, NativeScriptletKind, NativeScriptletMetadata, NativeStdinContract,
};
use std::io::Cursor;

#[test]
fn declared_member_copy_rejects_shorter_and_longer_bodies() {
    let mut output = Vec::new();
    let shorter =
        copy_declared_entry(&mut Cursor::new(b"ab"), &mut output, 3, "test member").unwrap_err();
    assert!(matches!(&shorter, Error::ParseError(_)));
    assert!(
        shorter
            .to_string()
            .contains("declares 3 bytes but yields 2")
    );

    output.clear();
    let longer =
        copy_declared_entry(&mut Cursor::new(b"abcd"), &mut output, 3, "test member").unwrap_err();
    assert!(matches!(&longer, Error::ParseError(_)));
    assert!(
        longer
            .to_string()
            .contains("declares 3 bytes but yields at least 4")
    );
}

#[test]
fn test_control_parsing() {
    let content = r#"Package: test-package
Version: 1.0.0-1
Architecture: amd64
Multi-Arch: foreign
Description: A test package
 This is a longer description
 that spans multiple lines.
Maintainer: Test User <test@example.com>
Section: utils
Priority: optional
Homepage: https://example.com
Installed-Size: 1024
Depends: libc6 (>= 2.34), zlib1g
Recommends: python3
"#;

    let control = DebPackage::parse_control(content).unwrap();
    assert_eq!(control.name, Some("test-package".to_string()));
    assert_eq!(control.version, Some("1.0.0-1".to_string()));
    assert_eq!(control.architecture, Some("amd64".to_string()));
    assert_eq!(control.multi_arch, DebianMultiArch::Foreign);
    assert_eq!(control.description, Some("A test package".to_string()));
    assert_eq!(
        control.maintainer,
        Some("Test User <test@example.com>".to_string())
    );
    assert_eq!(control.section, Some("utils".to_string()));
    assert_eq!(control.priority, Some("optional".to_string()));
    assert_eq!(control.homepage, Some("https://example.com".to_string()));
    assert_eq!(control.installed_size, Some(1024));
    assert_eq!(control.dependencies.len(), 2);
    assert_eq!(control.recommends.len(), 1);
}

#[test]
fn control_parser_rejects_malformed_numeric_fields() {
    let installed_size =
        DebPackage::parse_control("Package: test\nInstalled-Size: not-a-number\n").unwrap_err();
    assert!(
        installed_size.to_string().contains("Installed-Size"),
        "{installed_size}"
    );

    let epoch = DebPackage::parse_control("Package: test\nEpoch: not-a-number\n").unwrap_err();
    assert!(epoch.to_string().contains("Epoch"), "{epoch}");
}

#[test]
fn control_parser_rejects_unknown_multi_arch_value() {
    let error = DebPackage::parse_control("Package: test\nMulti-Arch: sometimes\n").unwrap_err();
    assert!(error.to_string().contains("Multi-Arch"), "{error}");
}

#[test]
fn control_parser_preserves_debian_all_architecture_token() {
    let control =
        DebPackage::parse_control("Package: portable-data\nVersion: 1\nArchitecture: all\n")
            .unwrap();
    assert_eq!(control.architecture.as_deref(), Some("all"));
}

#[test]
fn control_relations_preserve_conflicts_breaks_and_replaces_as_typed_authority() {
    let control = DebPackage::parse_control(
        "Package: newpkg\n\
         Version: 2\n\
         Architecture: amd64\n\
         Conflicts: old-conflict (<< 2)\n\
         Breaks: old-breaks (<= 1)\n\
         Replaces: old-owner (= 1)\n",
    )
    .unwrap();

    let mut relations =
        DebPackage::convert_relations(&control.conflicts, RepositoryRequirementKind::Conflict)
            .unwrap();
    relations.extend(
        DebPackage::convert_relations(&control.breaks, RepositoryRequirementKind::Breaks).unwrap(),
    );
    relations.extend(
        DebPackage::convert_relations(&control.replaces, RepositoryRequirementKind::Replace)
            .unwrap(),
    );

    assert_eq!(
        relations
            .iter()
            .map(|relation| relation.kind)
            .collect::<Vec<_>>(),
        vec![
            RepositoryRequirementKind::Conflict,
            RepositoryRequirementKind::Breaks,
            RepositoryRequirementKind::Replace,
        ]
    );
    assert_eq!(
        relations
            .iter()
            .map(|relation| relation.native_text.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("old-conflict (<< 2)"),
            Some("old-breaks (<= 1)"),
            Some("old-owner (= 1)"),
        ]
    );
}

#[test]
fn control_relation_rejects_malformed_exact_constraint() {
    let error = DebPackage::convert_relations(
        &["oldpkg (>=)".to_string()],
        RepositoryRequirementKind::Conflict,
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("missing Debian relation version")
    );
}

#[test]
fn test_dependency_list_parsing() {
    let deps = "libc6 (>= 2.34), zlib1g, python3 | python2";
    let parsed = DebPackage::parse_dependency_list(deps).unwrap();
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0], "libc6 (>= 2.34)");
    assert_eq!(parsed[1], "zlib1g");
    assert_eq!(parsed[2], "python3 | python2");
}

#[test]
fn dependency_list_rejects_empty_comma_atoms() {
    for malformed in ["", ",libc6", "libc6,,zlib1g", "libc6,"] {
        let error = DebPackage::parse_dependency_list(malformed).unwrap_err();
        assert!(error.to_string().contains("empty atom"), "{error}");
    }
}

#[test]
fn control_parser_rejects_malformed_lines_and_duplicate_fields() {
    for malformed in [
        " continuation-without-field\n",
        "Package: first\nthis is not a field\n",
        "Package: first\nPackage: second\n",
        "Package: first\n\nVersion: 1\n",
    ] {
        assert!(
            DebPackage::parse_control(malformed).is_err(),
            "malformed control metadata was accepted: {malformed:?}"
        );
    }
}

#[test]
fn deb_predepends_and_alternatives_preserve_exact_typed_groups() {
    let control = DebPackage::parse_control(
        "Package: consumer\n\
         Version: 1\n\
         Architecture: amd64\n\
         Pre-Depends: init-system (>= 2)\n\
         Depends: default-mta | mail-transport-agent\n",
    )
    .unwrap();
    let pre = DebPackage::convert_requirements(
        &control.pre_dependencies,
        RepositoryRequirementKind::PreDepends,
    )
    .unwrap();
    let depends =
        DebPackage::convert_requirements(&control.dependencies, RepositoryRequirementKind::Depends)
            .unwrap();

    assert_eq!(pre[0].kind, RepositoryRequirementKind::PreDepends);
    assert_eq!(
        pre[0].alternatives[0].version_constraint.as_deref(),
        Some(">= 2")
    );
    assert!(matches!(
        depends[0].expression,
        crate::repository::dependency_model::RepositoryRequirementExpression::Or(_)
    ));
    assert_eq!(
        depends[0]
            .alternatives
            .iter()
            .map(|clause| clause.name.as_str())
            .collect::<Vec<_>>(),
        vec!["default-mta", "mail-transport-agent"]
    );
}

#[test]
fn deb_shebang_split_preserves_exact_native_interpreter_argv() {
    let body = b"#!/usr/bin/perl -w\nprint qq(ok\\n);\n";
    let native = DebPackage::native_abi_from_control_member("postinst", body)
        .unwrap()
        .expect("native postinst");

    assert_eq!(native.interpreter.as_deref(), Some("/usr/bin/perl"));
    assert_eq!(native.interpreter_args, vec!["-w".to_string()]);
}

#[test]
fn deb_shebang_preserves_the_kernel_optional_argument_as_one_value() {
    let native = DebPackage::native_abi_from_control_member(
        "postinst",
        b"#!/usr/bin/env -S bash -eu\nprintf ok\n",
    )
    .unwrap()
    .expect("native postinst");

    assert_eq!(native.interpreter.as_deref(), Some("/usr/bin/env"));
    assert_eq!(native.interpreter_args, vec!["-S bash -eu".to_string()]);
}

#[test]
fn deb_maintainer_scripts_reject_missing_or_malformed_shebangs() {
    for body in [
        b"echo guessed-shell\n".as_slice(),
        b"#!bin/sh\nexit 0\n".as_slice(),
        b"#!/bin/sh\r\nexit 0\n".as_slice(),
        b"#!\xff\nexit 0\n".as_slice(),
    ] {
        assert!(
            DebPackage::native_abi_from_control_member("postinst", body).is_err(),
            "{body:?}"
        );
    }
}

#[test]
fn deb_binary_script_body_preserves_its_declared_interpreter() {
    let native =
        DebPackage::native_abi_from_control_member("postinst", b"#!/bin/sh\nprintf '\\377'\n\xff")
            .unwrap()
            .expect("native postinst");

    assert_eq!(native.interpreter.as_deref(), Some("/bin/sh"));
    assert_eq!(
        native.body.encoding,
        crate::packages::traits::NativeScriptletBodyEncoding::Binary
    );
}

#[test]
fn deb_native_abi_includes_config_as_native_only() {
    let entry = DebPackage::native_abi_from_control_member(
        "config",
        b"#!/bin/sh\ndb_input high pkg/question\n",
    )
    .unwrap()
    .expect("config entry");

    assert_eq!(entry.native_slot, "config");
    assert_eq!(entry.primary_lifecycle, NativeLifecyclePath::Config);
    assert_eq!(entry.invocation.stdin, NativeStdinContract::Debconf);

    let NativeScriptletMetadata::Deb(meta) = &entry.metadata else {
        panic!("expected deb metadata");
    };
    assert!(
        meta.maintainer_modes
            .iter()
            .any(|mode| mode.mode == DebMaintainerMode::Configure)
    );
    assert!(
        meta.maintainer_modes
            .iter()
            .any(|mode| mode.mode == DebMaintainerMode::Reconfigure)
    );
}

#[test]
fn deb_conffiles_preserve_remove_on_upgrade_as_typed_metadata() {
    let entries =
        DebPackage::parse_conffiles("/etc/demo.conf\nremove-on-upgrade /etc/obsolete.conf\n")
            .unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path, "/etc/demo.conf");
    assert!(!entries[0].remove_on_upgrade);
    assert_eq!(entries[1].path, "/etc/obsolete.conf");
    assert!(entries[1].remove_on_upgrade);
    assert!(entries.iter().all(|entry| entry.noreplace && !entry.ghost));
}

#[test]
fn deb_conffiles_reject_unknown_flags_and_malformed_entries() {
    for invalid in [
        "obsolete /etc/demo.conf\n",
        "remove-on-upgrade relative.conf\n",
        "remove-on-upgrade /etc/../passwd\n",
        "\n",
        "/etc/missing-final-newline",
    ] {
        assert!(DebPackage::parse_conffiles(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn deb_triggers_file_is_control_artifact_with_await_mode() {
    let body = b"interest-noawait update-icon-caches\nactivate-await ldconfig\n";
    let entry = DebPackage::native_abi_from_triggers_file(body).unwrap();

    assert_eq!(entry.kind, NativeScriptletKind::ControlArtifact);
    assert_eq!(entry.native_slot, "triggers");
    assert_eq!(entry.interpreter, None);
    assert_eq!(entry.body.bytes, body);

    let NativeScriptletMetadata::Deb(meta) = &entry.metadata else {
        panic!("expected deb metadata");
    };
    assert_eq!(meta.trigger_declarations.len(), 2);
    assert_eq!(
        meta.trigger_declarations[0].directive,
        DebTriggerDirective::Interest
    );
    assert_eq!(
        meta.trigger_declarations[0].await_mode,
        DebTriggerAwaitMode::NoAwait
    );
    assert_eq!(
        meta.trigger_declarations[1].directive,
        DebTriggerDirective::Activate
    );
    assert_eq!(
        meta.trigger_declarations[1].await_mode,
        DebTriggerAwaitMode::Await
    );
}

#[test]
fn deb_maintainer_invocation_table_preserves_mode_specific_arguments() {
    let preinst = DebPackage::native_abi_from_control_member("preinst", b"#!/bin/sh\n:")
        .unwrap()
        .expect("preinst entry");
    let NativeScriptletMetadata::Deb(preinst_meta) = &preinst.metadata else {
        panic!("expected deb metadata");
    };
    let upgrade = preinst_meta
        .maintainer_modes
        .iter()
        .find(|mode| mode.mode == DebMaintainerMode::Upgrade)
        .expect("preinst upgrade mode");
    assert_eq!(upgrade.args[0].name, "action");
    assert_eq!(upgrade.args[1].name, "old-version");
    assert_eq!(upgrade.args[1].index, 2);
    assert_eq!(upgrade.args[2].name, "new-version");
    assert_eq!(upgrade.args[2].index, 3);

    let postinst = DebPackage::native_abi_from_control_member("postinst", b"#!/bin/sh\n:")
        .unwrap()
        .expect("postinst entry");
    let NativeScriptletMetadata::Deb(postinst_meta) = &postinst.metadata else {
        panic!("expected deb metadata");
    };
    let triggered = postinst_meta
        .maintainer_modes
        .iter()
        .find(|mode| mode.mode == DebMaintainerMode::Triggered)
        .expect("postinst triggered mode");
    assert_eq!(triggered.args[1].name, "trigger-names");
    assert_eq!(triggered.args[1].value, NativeArgumentValue::TriggerNames);

    let abort_deconfigure = postinst_meta
        .maintainer_modes
        .iter()
        .find(|mode| mode.mode == DebMaintainerMode::AbortDeconfigure)
        .expect("postinst abort-deconfigure mode");
    assert_eq!(abort_deconfigure.args[1].name, "in-favour");
    assert_eq!(abort_deconfigure.args[2].name, "failed-install-package");
    assert_eq!(abort_deconfigure.args[3].index, 4);
    assert_eq!(abort_deconfigure.args[5].name, "conflicting-package");
    assert_eq!(abort_deconfigure.args[6].name, "conflicting-version");
    assert_eq!(
        abort_deconfigure
            .args
            .iter()
            .skip(2)
            .map(|argument| (&argument.value, argument.required))
            .collect::<Vec<_>>(),
        vec![
            (&NativeArgumentValue::InstallingPackageName, true),
            (&NativeArgumentValue::InstallingPackageVersion, true),
            (&NativeArgumentValue::ConflictingPackageMarker, false),
            (&NativeArgumentValue::ConflictingPackageName, false),
            (&NativeArgumentValue::ConflictingPackageVersion, false),
        ]
    );

    let prerm = DebPackage::native_abi_from_control_member("prerm", b"#!/bin/sh\n:")
        .unwrap()
        .expect("prerm entry");
    let NativeScriptletMetadata::Deb(prerm_meta) = &prerm.metadata else {
        panic!("expected deb metadata");
    };
    let deconfigure = prerm_meta
        .maintainer_modes
        .iter()
        .find(|mode| mode.mode == DebMaintainerMode::Deconfigure)
        .expect("prerm deconfigure mode");
    assert_eq!(deconfigure.args[1].name, "in-favour");
    assert_eq!(deconfigure.args[2].name, "package-being-installed");
    assert_eq!(deconfigure.args[3].index, 4);
    assert_eq!(deconfigure.args[5].name, "conflicting-package");
    assert_eq!(deconfigure.args[6].name, "conflicting-version");
    assert_eq!(
        deconfigure
            .args
            .iter()
            .skip(2)
            .map(|argument| (&argument.value, argument.required))
            .collect::<Vec<_>>(),
        vec![
            (&NativeArgumentValue::InstallingPackageName, true),
            (&NativeArgumentValue::InstallingPackageVersion, true),
            (&NativeArgumentValue::ConflictingPackageMarker, false),
            (&NativeArgumentValue::ConflictingPackageName, false),
            (&NativeArgumentValue::ConflictingPackageVersion, false),
        ]
    );

    let postrm = DebPackage::native_abi_from_control_member("postrm", b"#!/bin/sh\n:")
        .unwrap()
        .expect("postrm entry");
    let NativeScriptletMetadata::Deb(postrm_meta) = &postrm.metadata else {
        panic!("expected deb metadata");
    };
    let disappear = postrm_meta
        .maintainer_modes
        .iter()
        .find(|mode| mode.mode == DebMaintainerMode::Disappear)
        .expect("postrm disappear mode");
    assert_eq!(disappear.args[1].name, "overwriter-package");
    assert_eq!(disappear.args[2].name, "overwriter-version");
}
