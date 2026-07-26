// conary-core/src/packages/deb/dpkg_lifecycle/tests/alternatives.rs

use super::*;

fn round_trip(values: &[&[u8]]) -> AlternativesInvocation {
    let parsed = AlternativesInvocation::parse(&argv(values)).unwrap();
    assert_eq!(
        AlternativesInvocation::parse(&parsed.argv()).unwrap(),
        parsed
    );
    parsed
}

#[test]
fn all_documented_alternatives_actions_round_trip_with_effects() {
    let cases: &[(&[&str], AlternativesEffect)] = &[
        (
            &["--install", "/usr/bin/editor", "editor", "/bin/ed", "10"],
            AlternativesEffect::Mutation,
        ),
        (
            &["--set", "editor", "/bin/ed"],
            AlternativesEffect::Mutation,
        ),
        (
            &["--remove", "editor", "/bin/ed"],
            AlternativesEffect::Mutation,
        ),
        (&["--remove-all", "editor"], AlternativesEffect::Mutation),
        (&["--all"], AlternativesEffect::InteractiveMutation),
        (&["--auto", "editor"], AlternativesEffect::Mutation),
        (&["--display", "editor"], AlternativesEffect::ReadOnly),
        (&["--get-selections"], AlternativesEffect::ReadOnly),
        (&["--set-selections"], AlternativesEffect::Mutation),
        (&["--query", "editor"], AlternativesEffect::ReadOnly),
        (&["--list", "editor"], AlternativesEffect::ReadOnly),
        (
            &["--config", "editor"],
            AlternativesEffect::InteractiveMutation,
        ),
        (&["--help"], AlternativesEffect::ReadOnly),
        (&["--version"], AlternativesEffect::ReadOnly),
    ];
    for (values, effect) in cases {
        let bytes = values
            .iter()
            .map(|value| value.as_bytes())
            .collect::<Vec<_>>();
        let parsed = round_trip(&bytes);
        assert_eq!(parsed.action.effect(), *effect);
    }
    assert_eq!(AlternativesAction::DOCUMENTED_OPTIONS.len(), 14);
}

#[test]
fn install_models_all_slave_operands_and_source_checks() {
    let parsed = round_trip(&[
        b"--install",
        b"/usr/bin/editor",
        b"editor",
        b"/opt/editor",
        b"-100",
        b"--slave",
        b"/usr/bin/view",
        b"view",
        b"/opt/view",
        b"--slave",
        b"/usr/share/man/man1/editor.1.gz",
        b"editor.1.gz",
        b"/opt/editor.1.gz",
    ]);
    let AlternativesAction::Install {
        priority, slaves, ..
    } = parsed.action
    else {
        panic!("expected install");
    };
    assert_eq!(priority, -100);
    assert_eq!(slaves.len(), 2);
    assert_eq!(slaves[0].name, b"view");
}

#[test]
fn all_documented_options_preserve_source_order() {
    let parsed = round_trip(&[
        b"--root",
        b"/target",
        b"--admindir",
        b"/admin",
        b"--altdir",
        b"/etc/alternatives.custom",
        b"--instdir",
        b"/install",
        b"--log",
        b"/log",
        b"--force",
        b"--skip-auto",
        b"--quiet",
        b"--verbose",
        b"--debug",
        b"--display",
        b"editor",
    ]);
    assert_eq!(
        parsed.options.occurrences,
        vec![
            AlternativesOption::Root(b"/target".to_vec()),
            AlternativesOption::AdministrativeDirectory(b"/admin".to_vec()),
            AlternativesOption::AlternativesDirectory(b"/etc/alternatives.custom".to_vec()),
            AlternativesOption::InstallationDirectory(b"/install".to_vec()),
            AlternativesOption::Log(b"/log".to_vec()),
            AlternativesOption::Force,
            AlternativesOption::SkipAutomatic,
            AlternativesOption::Output(AlternativesOutput::Quiet),
            AlternativesOption::Output(AlternativesOutput::Verbose),
            AlternativesOption::Output(AlternativesOutput::Debug),
        ]
    );
    assert_eq!(parsed.options.output(), AlternativesOutput::Debug);
    assert!(parsed.options.force());
    assert!(parsed.options.skip_automatic());
    assert_eq!(
        &parsed.argv()[..4],
        &[
            b"--root".to_vec(),
            b"/target".to_vec(),
            b"--admindir".to_vec(),
            b"/admin".to_vec(),
        ]
    );

    let action_then_option = round_trip(&[b"--all", b"--skip-auto", b"--force"]);
    assert!(action_then_option.options.skip_automatic());
    assert!(action_then_option.options.force());
}

#[test]
fn selection_line_contract_round_trips_auto_manual_and_space_paths() {
    for selection in [
        AlternativeSelection {
            name: b"editor".to_vec(),
            mode: AlternativeSelectionMode::Automatic,
            choice: b"/usr/bin/vim".to_vec(),
        },
        AlternativeSelection {
            name: b"editor".to_vec(),
            mode: AlternativeSelectionMode::Manual,
            choice: b"/path with spaces/editor".to_vec(),
        },
    ] {
        let line = selection.line().unwrap();
        assert_eq!(AlternativeSelection::parse_line(&line).unwrap(), selection);
    }
    assert!(
        AlternativesAction::SetSelections.reads_selection_stdin()
            && !AlternativesAction::GetSelections.reads_selection_stdin()
    );
}

#[test]
fn alternatives_rejects_unknown_conflicts_and_malformed_operands() {
    let cases: &[&[&str]] = &[
        &[],
        &["--unknown"],
        &["-v", "--display", "editor"],
        &["--admindir=/admin", "--display", "editor"],
        &["--root"],
        &["--display"],
        &["--display", "bad/name"],
        &["--display", "bad name"],
        &["--display", "editor", "--query", "editor"],
        &["--slave", "/usr/bin/view", "view", "/opt/view"],
        &["--install", "usr/bin/editor", "editor", "/opt/editor", "1"],
        &["--install", "/usr/bin/editor", "editor", "opt/editor", "1"],
        &[
            "--install",
            "/usr/bin/editor",
            "editor",
            "/usr/bin/editor",
            "1",
        ],
        &[
            "--install",
            "/usr/bin/editor",
            "editor",
            "/opt/editor",
            "1x",
        ],
        &[
            "--install",
            "/usr/bin/editor",
            "editor",
            "/opt/editor",
            "1",
            "--slave",
            "/usr/bin/editor",
            "view",
            "/opt/view",
        ],
        &[
            "--install",
            "/usr/bin/editor",
            "editor",
            "/opt/editor",
            "1",
            "--slave",
            "/usr/bin/view",
            "editor",
            "/opt/view",
        ],
        &["--set", "editor", "opt/editor"],
        &["--remove", "editor"],
        &["--help", "--version"],
    ];
    for values in cases {
        assert!(
            AlternativesInvocation::parse(&text_argv(values)).is_err(),
            "{values:?}"
        );
    }
}

#[test]
fn malformed_selection_records_are_closed_errors() {
    for line in [
        b"editor auto /bin/ed".as_slice(),
        b"editor\n",
        b"editor auto\n",
        b"bad/name auto /bin/ed\n",
        b"editor future /bin/ed\n",
        b"editor auto \n",
        b"editor auto /bin/ed\ntrailing".as_slice(),
        b"editor auto /\0bin/ed\n",
    ] {
        assert!(AlternativeSelection::parse_line(line).is_err(), "{line:?}");
    }
    let too_long = vec![b'x'; 1024];
    assert!(AlternativeSelection::parse_line(&too_long).is_err());
}

#[test]
fn alternatives_result_table_is_exact() {
    for (status, code) in [
        (AlternativesExitStatus::Success, 0),
        (AlternativesExitStatus::Error, 2),
    ] {
        assert_eq!(
            AlternativesExitStatus::from_exit_code(code).unwrap(),
            status
        );
        assert_eq!(status.exit_code(), code);
    }
    assert!(AlternativesExitStatus::from_exit_code(1).is_err());
}
