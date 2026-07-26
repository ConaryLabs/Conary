// conary-core/src/packages/deb/dpkg_lifecycle/tests/maintscript.rs

use super::*;

fn round_trip(values: &[&[u8]]) -> DpkgMaintscriptHelperInvocation {
    let parsed = DpkgMaintscriptHelperInvocation::parse(&argv(values)).unwrap();
    assert_eq!(
        DpkgMaintscriptHelperInvocation::parse(&parsed.argv()).unwrap(),
        parsed
    );
    parsed
}

#[test]
fn supports_covers_the_complete_public_action_table() {
    for action in MaintscriptHelperAction::ALL {
        let parsed = round_trip(&[b"supports", action.as_bytes()]);
        assert_eq!(parsed, DpkgMaintscriptHelperInvocation::Supports(action));
    }
}

#[test]
fn all_documented_maintainer_phase_shapes_round_trip() {
    let cases: &[&[&str]] = &[
        &["install"],
        &["install", "1", "2"],
        &["upgrade", "2"],
        &["upgrade", "1", "2"],
        &["abort-upgrade", "2"],
        &["abort-upgrade", "1", "2"],
        &["abort-install"],
        &["abort-install", "1", "2"],
        &["configure", ""],
        &["triggered", "interest-a interest-b"],
        &["abort-remove"],
        &["abort-remove", "in-favour", "new-pkg", "2"],
        &["abort-deconfigure", "in-favour", "new-pkg", "2"],
        &[
            "abort-deconfigure",
            "in-favour",
            "new-pkg",
            "2",
            "removing",
            "old-pkg",
            "1",
        ],
        &["remove"],
        &["remove", "in-favour", "new-pkg", "2"],
        &["failed-upgrade", "1", "2"],
        &["deconfigure", "in-favour", "new-pkg", "2"],
        &[
            "deconfigure",
            "in-favour",
            "new-pkg",
            "2",
            "removing",
            "old-pkg",
            "1",
        ],
        &["purge"],
        &["disappear", "overwriter", "3"],
    ];
    for values in cases {
        let values = text_argv(values);
        let parsed = MaintainerScriptPhase::parse(&values).unwrap();
        assert_eq!(
            MaintainerScriptPhase::parse(&parsed.argv()).unwrap(),
            parsed
        );
    }
}

#[test]
fn all_four_operations_and_optional_operand_forms_round_trip() {
    let cases: &[&[&str]] = &[
        &["rm_conffile", "/etc/demo.conf", "--", "upgrade", "1", "2"],
        &[
            "rm_conffile",
            "/etc/demo.conf",
            "2~",
            "--",
            "configure",
            "1",
        ],
        &[
            "rm_conffile",
            "/etc/demo.conf",
            "",
            "demo:amd64",
            "--",
            "purge",
        ],
        &[
            "mv_conffile",
            "/etc/old.conf",
            "/etc/new.conf",
            "2~",
            "demo",
            "--",
            "abort-upgrade",
            "1",
            "2",
        ],
        &[
            "symlink_to_dir",
            "/usr/lib/demo",
            "demo.real",
            "2~",
            "demo",
            "--",
            "install",
            "1",
            "2",
        ],
        &[
            "dir_to_symlink",
            "/usr/lib/demo/",
            "demo.real",
            "--",
            "disappear",
            "overwriter",
            "3",
        ],
    ];
    for values in cases {
        let values = text_argv(values);
        let parsed = DpkgMaintscriptHelperInvocation::parse(&values).unwrap();
        assert_eq!(
            DpkgMaintscriptHelperInvocation::parse(&parsed.argv()).unwrap(),
            parsed
        );
        let DpkgMaintscriptHelperInvocation::Operation(operation) = parsed else {
            panic!("expected operation");
        };
        assert_eq!(operation.action().as_bytes(), values[0]);
    }

    round_trip(&[b"help"]);
    round_trip(&[b"-?"]);
    round_trip(&[b"--help"]);
    round_trip(&[b"--version"]);
}

#[test]
fn operation_phase_and_byte_operands_are_available_without_guessing() {
    let parsed = round_trip(&[
        b"symlink_to_dir",
        b"/usr/lib/\xffdemo",
        b"../\xfetarget",
        b"",
        b"demo:amd64",
        b"--",
        b"abort-install",
        b"1",
        b"2",
    ]);
    let DpkgMaintscriptHelperInvocation::Operation(operation) = parsed else {
        panic!("expected operation");
    };
    assert!(matches!(
        operation.phase(),
        MaintainerScriptPhase::AbortInstall {
            old_and_new_version: Some((old, new))
        } if old == b"1" && new == b"2"
    ));
}

#[test]
fn maintscript_grammar_rejects_private_unknown_and_malformed_forms() {
    for values in [
        &[][..],
        &["future-command"][..],
        &["_internal_pkg_must_own_file", "demo", "/etc/demo"][..],
        &["supports"][..],
        &["supports", "future-command"][..],
        &["supports", "rm_conffile", "extra"][..],
        &["rm_conffile", "/etc/demo", "upgrade", "1", "2"][..],
        &["rm_conffile", "/etc/demo", "--", "upgrade", "--", "2"][..],
        &["rm_conffile", "etc/demo", "--", "purge"][..],
        &["mv_conffile", "/etc/old", "--", "purge"][..],
        &["symlink_to_dir", "/usr/lib/demo", "", "--", "purge"][..],
        &["dir_to_symlink", "usr/lib/demo", "target", "--", "purge"][..],
        &["rm_conffile", "/etc/demo", "--"][..],
        &["rm_conffile", "/etc/demo", "--", "future-phase"][..],
        &["rm_conffile", "/etc/demo", "--", "install", "1"][..],
        &["rm_conffile", "/etc/demo", "--", "configure"][..],
        &["rm_conffile", "/etc/demo", "--", "triggered", ""][..],
        &[
            "rm_conffile",
            "/etc/demo",
            "--",
            "remove",
            "instead-of",
            "pkg",
            "1",
        ][..],
        &["--version", "extra"][..],
    ] {
        assert!(
            DpkgMaintscriptHelperInvocation::parse(&text_argv(values)).is_err(),
            "{values:?}"
        );
    }
}

#[test]
fn supports_result_table_is_exact() {
    for (status, code) in [
        (MaintscriptHelperSupportStatus::Supported, 0),
        (MaintscriptHelperSupportStatus::Unsupported, 1),
    ] {
        assert_eq!(
            MaintscriptHelperSupportStatus::from_exit_code(code).unwrap(),
            status
        );
        assert_eq!(status.exit_code(), code);
    }
    assert!(MaintscriptHelperSupportStatus::from_exit_code(2).is_err());
}
