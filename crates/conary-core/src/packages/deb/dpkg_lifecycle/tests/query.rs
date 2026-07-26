// conary-core/src/packages/deb/dpkg_lifecycle/tests/query.rs

use super::*;

fn round_trip(values: &[&[u8]]) -> DpkgQueryInvocation {
    let parsed = DpkgQueryInvocation::parse(&argv(values)).unwrap();
    assert_eq!(DpkgQueryInvocation::parse(&parsed.argv()).unwrap(), parsed);
    parsed
}

#[test]
fn all_documented_query_actions_and_aliases_round_trip() {
    for values in [
        &["--list"][..],
        &["-l", "lib*"][..],
        &["--show"][..],
        &["-W", "dpkg"][..],
        &["--status"][..],
        &["-s", "dpkg"][..],
        &["--listfiles", "dpkg"][..],
        &["-L", "dpkg", "base-files"][..],
        &["--control-list", "dpkg"][..],
        &["--control-show", "dpkg", "postinst"][..],
        &["--control-path", "dpkg"][..],
        &["-c", "dpkg", "postinst"][..],
        &["--search", "/usr/bin/dpkg"][..],
        &["-S", "*.service"][..],
        &["--print-avail"][..],
        &["-p", "dpkg"][..],
        &["--help"][..],
        &["-?"][..],
        &["--version"][..],
    ] {
        let bytes = values
            .iter()
            .map(|value| value.as_bytes())
            .collect::<Vec<_>>();
        round_trip(&bytes);
    }
    assert_eq!(DpkgQueryAction::DOCUMENTED_NAMES.len(), 11);
}

#[test]
fn every_documented_query_option_spelling_is_typed() {
    for values in [
        &["--admindir=/state", "--show"][..],
        &["--admindir", "/state", "--show"][..],
        &["--root=/target", "--show"][..],
        &["--root", "/target", "--show"][..],
        &["--load-avail", "--list"][..],
        &["--no-pager", "--status"][..],
        &["-f${Package}", "--show"][..],
        &["-f=${Package}", "--show"][..],
        &["-f", "${Package}", "--show"][..],
        &["--showformat=${Package}", "--show"][..],
        &["--showformat", "${Package}", "--show"][..],
    ] {
        let bytes = values
            .iter()
            .map(|value| value.as_bytes())
            .collect::<Vec<_>>();
        round_trip(&bytes);
    }
}

#[test]
fn query_option_occurrences_preserve_root_admindir_precedence() {
    let parsed = round_trip(&[
        b"--root=/target",
        b"--admindir=/custom",
        b"--root=/last",
        b"--showformat=${Version}",
        b"-f=${Package}",
        b"--load-avail",
        b"--load-avail",
        b"--no-pager",
        b"--show",
        b"dpkg",
    ]);
    assert_eq!(
        parsed.options.occurrences,
        vec![
            DpkgQueryOption::Root(b"/target".to_vec()),
            DpkgQueryOption::AdministrativeDirectory(b"/custom".to_vec()),
            DpkgQueryOption::Root(b"/last".to_vec()),
            DpkgQueryOption::ShowFormat(b"${Version}".to_vec()),
            DpkgQueryOption::ShowFormat(b"${Package}".to_vec()),
            DpkgQueryOption::LoadAvailable,
            DpkgQueryOption::LoadAvailable,
            DpkgQueryOption::NoPager,
        ]
    );
    assert_eq!(parsed.options.show_format(), Some(b"${Package}".as_slice()));
    assert!(parsed.options.load_available());
    assert!(parsed.options.no_pager());
    assert_eq!(
        &parsed.argv()[..3],
        &[
            b"--root=/target".to_vec(),
            b"--admindir=/custom".to_vec(),
            b"--root=/last".to_vec(),
        ]
    );

    let reverse = round_trip(&[b"--admindir=/custom", b"--root=/target", b"--show"]);
    assert_ne!(parsed.options.occurrences, reverse.options.occurrences);
}

#[test]
fn query_delimiter_and_operands_remain_byte_exact() {
    let parsed = round_trip(&[
        b"--showformat=\xff${Package}",
        b"--show",
        b"--",
        b"-literal",
        b"\xfe",
    ]);
    assert_eq!(
        parsed.options.show_format(),
        Some(b"\xff${Package}".as_slice())
    );
    assert_eq!(
        parsed.action,
        DpkgQueryAction::Show {
            patterns: vec![b"-literal".to_vec(), b"\xfe".to_vec()]
        }
    );

    let trailing_option = round_trip(&[b"--list", b"first", b"--root=/operand"]);
    assert!(matches!(
        trailing_option.action,
        DpkgQueryAction::List { ref patterns }
            if patterns == &text_argv(&["first", "--root=/operand"])
    ));
}

#[test]
fn query_rejects_unknown_conflicting_missing_and_bad_arity_forms() {
    for values in [
        &[][..],
        &["--unknown"][..],
        &["--root"][..],
        &["--show", "--status"][..],
        &["--listfiles"][..],
        &["--control-list"][..],
        &["--control-list", "dpkg", "extra"][..],
        &["--control-show", "dpkg"][..],
        &["--control-show", "dpkg", "postinst", "extra"][..],
        &["--control-path"][..],
        &["--control-path", "dpkg", "postinst", "extra"][..],
        &["--search"][..],
        &["--help", "extra"][..],
        &["--version", "extra"][..],
    ] {
        assert!(
            DpkgQueryInvocation::parse(&text_argv(values)).is_err(),
            "{values:?}"
        );
    }
}

#[test]
fn query_result_table_is_total_for_documented_codes() {
    for (status, code) in [
        (DpkgQueryExitStatus::Success, 0),
        (DpkgQueryExitStatus::NoMatch, 1),
        (DpkgQueryExitStatus::Error, 2),
    ] {
        assert_eq!(DpkgQueryExitStatus::from_exit_code(code).unwrap(), status);
        assert_eq!(status.exit_code(), code);
    }
    assert!(DpkgQueryExitStatus::from_exit_code(3).is_err());
}
