// conary-core/src/packages/deb/lifecycle_helpers/tests.rs

use super::*;

fn argv(arguments: &[&str]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| (*argument).to_string())
        .collect()
}

fn assert_round_trip(command: &str, arguments: &[&str]) -> DebianLifecycleHelperInvocation {
    let parsed = DebianLifecycleHelperInvocation::parse(command, &argv(arguments)).unwrap();
    let rendered = parsed.argv();
    assert_eq!(
        DebianLifecycleHelperInvocation::parse(parsed.command(), &rendered).unwrap(),
        parsed
    );
    parsed
}

#[test]
fn source_pin_names_every_helper_once_with_immutable_identity() {
    assert_eq!(PINNED_HELPER_SOURCES.len(), 5);
    assert_eq!(
        PINNED_HELPER_SOURCES.map(|source| source.command),
        [
            "deb-systemd-helper",
            "deb-systemd-invoke",
            "invoke-rc.d",
            "update-rc.d",
            "service",
        ]
    );
    for source in PINNED_HELPER_SOURCES {
        assert_eq!(source.version, INIT_SYSTEM_HELPERS_VERSION);
        assert!(
            source
                .source_url
                .starts_with(INIT_SYSTEM_HELPERS_SOURCE_BASE)
        );
        assert_eq!(source.sha256.len(), 64);
        assert!(source.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}

#[test]
fn top_level_authority_round_trips_every_helper_variant() {
    for (command, arguments) in [
        ("deb-systemd-helper", &["enable", "demo.service"][..]),
        ("deb-systemd-invoke", &["restart", "demo.service"][..]),
        ("invoke-rc.d", &["demo", "restart"][..]),
        ("update-rc.d", &["demo", "defaults"][..]),
        ("service", &["demo", "restart"][..]),
    ] {
        assert_round_trip(command, arguments);
    }
    assert!(matches!(
        DebianLifecycleHelperInvocation::parse("not-a-helper", &[]),
        Err(HelperGrammarError::UnknownHelper(_))
    ));
}

#[test]
fn every_documented_option_spelling_is_in_the_round_trip_table() {
    for (command, arguments) in [
        (
            "deb-systemd-helper",
            &["--quiet", "enable", "demo.service"][..],
        ),
        (
            "deb-systemd-helper",
            &["--user", "enable", "demo.service"][..],
        ),
        (
            "deb-systemd-helper",
            &["--system", "enable", "demo.service"][..],
        ),
        (
            "deb-systemd-invoke",
            &["--user", "restart", "demo.service"][..],
        ),
        (
            "deb-systemd-invoke",
            &["--system", "restart", "demo.service"][..],
        ),
        ("deb-systemd-invoke", &["--no-dbus", "daemon-reload"][..]),
        ("invoke-rc.d", &["--quiet", "demo", "restart"][..]),
        ("invoke-rc.d", &["--force", "demo", "restart"][..]),
        ("invoke-rc.d", &["--try-anyway", "demo", "restart"][..]),
        ("invoke-rc.d", &["--disclose-deny", "demo", "restart"][..]),
        ("invoke-rc.d", &["--query", "demo", "restart"][..]),
        ("invoke-rc.d", &["--no-fallback", "demo", "restart"][..]),
        (
            "invoke-rc.d",
            &["--skip-systemd-native", "demo", "restart"][..],
        ),
        ("invoke-rc.d", &["--help"][..]),
        ("update-rc.d", &["-f", "demo", "defaults"][..]),
        ("update-rc.d", &["-h"][..]),
        ("update-rc.d", &["--help"][..]),
        ("service", &["-h"][..]),
        ("service", &["--help"][..]),
        ("service", &["-V"][..]),
        ("service", &["--version"][..]),
        ("service", &["--status-all"][..]),
    ] {
        assert_round_trip(command, arguments);
    }
}

#[test]
fn deb_systemd_helper_action_table_is_exhaustive_and_round_trips() {
    for action in DebSystemdHelperAction::ALL {
        let parsed = assert_round_trip(
            "deb-systemd-helper",
            &[action.as_str(), "demo.service", "demo.socket"],
        );
        let DebianLifecycleHelperInvocation::DebSystemdHelper(parsed) = parsed else {
            panic!("wrong helper variant");
        };
        assert_eq!(parsed.action, action);
        assert_eq!(parsed.units, ["demo.service", "demo.socket"]);
    }
}

#[test]
fn deb_systemd_helper_models_getopt_permutation_scope_and_terminator() {
    let parsed = DebSystemdHelperInvocation::parse(&argv(&[
        "enable",
        "--quiet",
        "--user",
        "--system",
        "--user",
        "demo.service",
    ]))
    .unwrap();
    assert_eq!(
        parsed.options,
        DebSystemdHelperOptions {
            quiet: true,
            instance: UnitInstance::User,
        }
    );
    assert_eq!(
        DebSystemdHelperInvocation::parse(&parsed.argv()).unwrap(),
        parsed
    );

    let hyphen_unit =
        DebSystemdHelperInvocation::parse(&argv(&["--", "enable", "-literal.service"])).unwrap();
    assert!(hyphen_unit.argv().contains(&"--".to_string()));
    assert_eq!(
        DebSystemdHelperInvocation::parse(&hyphen_unit.argv()).unwrap(),
        hyphen_unit
    );
}

#[test]
fn deb_systemd_helper_rejects_unknown_or_incomplete_input() {
    for arguments in [
        &["--quiet", "--mystery", "enable", "demo.service"][..],
        &["future-action", "demo.service"][..],
        &["enable"][..],
        &["enable", ""][..],
    ] {
        assert!(DebSystemdHelperInvocation::parse(&argv(arguments)).is_err());
    }
    for (status, code) in [
        (DebSystemdHelperQueryStatus::Satisfied, 0),
        (DebSystemdHelperQueryStatus::Unsatisfied, 1),
    ] {
        assert_eq!(
            DebSystemdHelperQueryStatus::from_exit_code(code).unwrap(),
            status
        );
        assert_eq!(status.exit_code(), code);
    }
    assert!(DebSystemdHelperQueryStatus::from_exit_code(2).is_err());
}

#[test]
fn deb_systemd_invoke_action_table_enforces_both_documented_shapes() {
    for action in DebSystemdInvokeAction::ALL {
        let arguments = if action.takes_units() {
            vec![action.as_str(), "demo.service"]
        } else {
            vec![action.as_str()]
        };
        let parsed = assert_round_trip("deb-systemd-invoke", &arguments);
        let DebianLifecycleHelperInvocation::DebSystemdInvoke(parsed) = parsed else {
            panic!("wrong helper variant");
        };
        assert_eq!(parsed.action, action);
    }

    assert!(DebSystemdInvokeInvocation::parse(&argv(&["start"])).is_err());
    assert!(DebSystemdInvokeInvocation::parse(&argv(&["daemon-reload", "demo.service"])).is_err());
    assert!(
        DebSystemdInvokeInvocation::parse(&argv(&["--no-dbus", "restart", "demo.service"]))
            .is_err()
    );
    assert!(
        DebSystemdInvokeInvocation::parse(&argv(&["--unknown", "start", "demo.service"])).is_err()
    );
}

#[test]
fn deb_systemd_invoke_models_every_policy_status_branch() {
    assert_eq!(
        DebSystemdInvokePolicyStatus::from_exit_code(0),
        DebSystemdInvokePolicyStatus::Run
    );
    assert_eq!(
        DebSystemdInvokePolicyStatus::from_exit_code(104),
        DebSystemdInvokePolicyStatus::Run
    );
    assert_eq!(
        DebSystemdInvokePolicyStatus::from_exit_code(101),
        DebSystemdInvokePolicyStatus::Deny
    );
    assert_eq!(
        DebSystemdInvokePolicyStatus::from_exit_code(42),
        DebSystemdInvokePolicyStatus::IgnoredUnsupported(42)
    );
}

#[test]
fn invoke_rc_d_round_trips_all_standard_and_custom_actions() {
    for action in InitScriptAction::STANDARD {
        let parsed = assert_round_trip("invoke-rc.d", &["demo", action.as_str(), "--literal"]);
        let DebianLifecycleHelperInvocation::InvokeRcD(InvokeRcDInvocation::Run {
            action: parsed_action,
            extra_parameters,
            ..
        }) = parsed
        else {
            panic!("wrong helper variant");
        };
        assert_eq!(parsed_action, action);
        assert_eq!(extra_parameters, ["--literal"]);
    }

    let parsed = assert_round_trip("invoke-rc.d", &["demo", "rotate-secrets", "now"]);
    let DebianLifecycleHelperInvocation::InvokeRcD(InvokeRcDInvocation::Run { action, .. }) =
        parsed
    else {
        panic!("wrong helper variant");
    };
    assert_eq!(
        action,
        InitScriptAction::Custom("rotate-secrets".to_string())
    );
}

#[test]
fn invoke_rc_d_models_option_effects_without_reclassifying_extra_parameters() {
    let parsed = InvokeRcDInvocation::parse(&argv(&[
        "--quiet",
        "demo",
        "--force",
        "--query",
        "--no-fallback",
        "--skip-systemd-native",
        "restart",
        "--query",
    ]))
    .unwrap();
    let InvokeRcDInvocation::Run {
        options,
        extra_parameters,
        ..
    } = &parsed
    else {
        panic!("expected run invocation");
    };
    assert!(options.force);
    assert!(options.try_anyway);
    assert!(options.query);
    assert!(options.effectively_discloses_denial());
    assert!(options.effectively_disables_fallback());
    assert_eq!(extra_parameters, &["--query"]);
    assert_eq!(InvokeRcDInvocation::parse(&parsed.argv()).unwrap(), parsed);

    assert!(matches!(
        InvokeRcDInvocation::parse(&argv(&["--help"])).unwrap(),
        InvokeRcDInvocation::Help
    ));
    assert!(InvokeRcDInvocation::parse(&argv(&["--unknown", "demo", "start"])).is_err());
    assert!(InvokeRcDInvocation::parse(&argv(&["demo"])).is_err());
    assert!(InvokeRcDInvocation::parse(&argv(&["bad name", "start"])).is_err());
}

#[test]
fn invoke_rc_d_status_and_policy_tables_are_total_for_documented_codes() {
    for code in 0..=106 {
        let status = InvokeRcDStatus::from_exit_code(code).unwrap();
        assert_eq!(status.exit_code(), code);
    }
    assert!(InvokeRcDStatus::from_exit_code(107).is_err());

    for (code, expected) in [
        (0, InvokeRcDPolicyStatus::Allow),
        (104, InvokeRcDPolicyStatus::Allow),
        (1, InvokeRcDPolicyStatus::Undefined),
        (105, InvokeRcDPolicyStatus::Undefined),
        (101, InvokeRcDPolicyStatus::Deny),
        (
            102,
            InvokeRcDPolicyStatus::InvokeStatus(InvokeRcDStatus::SubsystemError),
        ),
        (42, InvokeRcDPolicyStatus::Unsupported(42)),
    ] {
        assert_eq!(
            InvokeRcDPolicyStatus::from_helper_result(code, "").unwrap(),
            expected
        );
    }
    assert_eq!(
        InvokeRcDPolicyStatus::from_helper_result(106, "restart rotate-secrets").unwrap(),
        InvokeRcDPolicyStatus::Fallback(vec![
            InitScriptAction::Restart,
            InitScriptAction::Custom("rotate-secrets".to_string()),
        ])
    );
    assert!(InvokeRcDPolicyStatus::from_helper_result(106, "").is_err());
}

#[test]
fn service_models_informational_status_full_restart_and_custom_branches() {
    for arguments in [
        &["--help"][..],
        &["--version"][..],
        &["--status-all"][..],
        &["demo", "--full-restart"][..],
        &["demo", "rotate-secrets", "now"][..],
    ] {
        assert_round_trip("service", arguments);
    }
    for action in InitScriptAction::STANDARD {
        assert_round_trip("service", &["demo", action.as_str()]);
    }
    assert!(ServiceInvocation::parse(&argv(&["demo", "--full-restart", "unexpected"])).is_err());
    assert!(ServiceInvocation::parse(&argv(&["--unknown"])).is_err());
    assert!(ServiceInvocation::parse(&[]).is_err());

    for status in ServiceStatus::ALL {
        assert_eq!(ServiceStatus::from_marker(status.marker()).unwrap(), status);
    }
    assert!(ServiceStatus::from_marker("!").is_err());
}

#[test]
fn update_rc_d_fixed_action_and_toggle_tables_round_trip() {
    for arguments in [
        &["demo", "remove"][..],
        &["demo", "defaults"][..],
        &["demo", "defaults-disabled"][..],
        &["-f", "demo", "defaults"][..],
        &["demo", "enable"][..],
        &["demo", "disable"][..],
    ] {
        assert_round_trip("update-rc.d", arguments);
    }
    for runlevel in SysvRunlevel::TOGGLE {
        assert_round_trip("update-rc.d", &["demo", "enable", runlevel.as_str()]);
        assert_round_trip("update-rc.d", &["demo", "disable", runlevel.as_str()]);
    }
    assert!(UpdateRcDInvocation::parse(&argv(&["demo", "enable", "1"])).is_err());
    assert!(UpdateRcDInvocation::parse(&argv(&["demo", "remove", "ignored"])).is_err());
    assert!(UpdateRcDInvocation::parse(&argv(&["--unknown", "demo", "defaults"])).is_err());
    assert!(UpdateRcDInvocation::parse(&argv(&["demo", "future-action"])).is_err());
}

#[test]
fn update_rc_d_legacy_source_branch_has_a_strict_typed_sequence() {
    let parsed = assert_round_trip(
        "update-rc.d",
        &[
            "demo", "start", "20", "2", "3", "4", "5", ".", "stop", "80", "0", "1", "6", ".",
        ],
    );
    let DebianLifecycleHelperInvocation::UpdateRcD(UpdateRcDInvocation::Apply {
        operation: UpdateRcDOperation::LegacyDefaults { action, clauses },
        ..
    }) = parsed
    else {
        panic!("wrong helper variant");
    };
    assert_eq!(action, LegacySequenceKind::Start);
    assert_eq!(clauses.len(), 2);
    assert_eq!(clauses[0].sequence, 20);
    assert_eq!(clauses[1].kind, LegacySequenceKind::Stop);
    assert_eq!(clauses[1].runlevels.len(), 3);

    assert_round_trip("update-rc.d", &["demo", "start"]);
    for malformed in [
        &["demo", "start", "2", "2", "."][..],
        &["demo", "start", "20", "."][..],
        &["demo", "start", "20", "2"][..],
        &["demo", "start", "20", "2", ".", "80", "0", "."][..],
    ] {
        assert!(UpdateRcDInvocation::parse(&argv(malformed)).is_err());
    }
}
