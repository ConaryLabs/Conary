// conary-core/src/packages/deb/debconf/tests/protocol.rs

use super::super::*;
use super::support::*;

#[test]
fn parses_the_complete_typed_command_union() {
    assert!(matches!(
        parse_command(b"VERSION 2.1", false).unwrap(),
        DebconfCommand::Version {
            version: Some(DebconfProtocolVersion {
                major: 2,
                minor: Some(1)
            })
        }
    ));
    assert!(matches!(
        parse_command(b"CAPB backup escape", false).unwrap(),
        DebconfCommand::Capb { capabilities } if capabilities == [b"backup", b"escape"]
    ));
    assert!(matches!(
        parse_command(b"TITLE Package setup", false).unwrap(),
        DebconfCommand::Title { title } if title == b"Package setup"
    ));
    assert!(matches!(
        parse_command(b"SETTITLE pkg/title", false).unwrap(),
        DebconfCommand::SetTitle { .. }
    ));
    assert!(matches!(
        parse_command(b"INPUT critical pkg/name", false).unwrap(),
        DebconfCommand::Input {
            priority: DebconfPriority::Critical,
            ..
        }
    ));
    assert_eq!(parse_command(b"GO", false).unwrap(), DebconfCommand::Go);
    assert_eq!(
        parse_command(b"CLEAR", false).unwrap(),
        DebconfCommand::Clear
    );
    assert_eq!(
        parse_command(b"BEGINBLOCK", false).unwrap(),
        DebconfCommand::BeginBlock
    );
    assert_eq!(
        parse_command(b"ENDBLOCK", false).unwrap(),
        DebconfCommand::EndBlock
    );
    assert_eq!(parse_command(b"STOP", false).unwrap(), DebconfCommand::Stop);
    assert!(matches!(
        parse_command(b"GET pkg/name", false).unwrap(),
        DebconfCommand::Get { .. }
    ));
    assert!(matches!(
        parse_command(b"SET pkg/name value with spaces", false).unwrap(),
        DebconfCommand::Set { value, .. } if value == b"value with spaces"
    ));
    assert!(matches!(
        parse_command(b"RESET pkg/name", false).unwrap(),
        DebconfCommand::Reset { .. }
    ));
    assert!(matches!(
        parse_command(b"SUBST pkg/name TOKEN replacement value", false).unwrap(),
        DebconfCommand::Subst { value, .. } if value == b"replacement value"
    ));
    assert!(matches!(
        parse_command(b"FGET pkg/name seen", false).unwrap(),
        DebconfCommand::FGet { .. }
    ));
    assert!(matches!(
        parse_command(b"FSET pkg/name seen true", false).unwrap(),
        DebconfCommand::FSet {
            value: DebconfFlagValue::True,
            ..
        }
    ));
    assert!(matches!(
        parse_command(b"METAGET pkg/name description", false).unwrap(),
        DebconfCommand::MetaGet { .. }
    ));
    assert!(matches!(
        parse_command(b"REGISTER pkg/name pkg/alias", false).unwrap(),
        DebconfCommand::Register { .. }
    ));
    assert!(matches!(
        parse_command(b"UNREGISTER pkg/alias", false).unwrap(),
        DebconfCommand::Unregister { .. }
    ));
    assert_eq!(
        parse_command(b"PURGE", false).unwrap(),
        DebconfCommand::Purge
    );
    assert!(matches!(
        parse_command(b"INFO", false).unwrap(),
        DebconfCommand::Info { question: None }
    ));
    assert!(matches!(
        parse_command(b"INFO pkg/note", false).unwrap(),
        DebconfCommand::Info { question: Some(_) }
    ));
    assert!(matches!(
        parse_command(b"DATA pkg/name description one\\ntwo", false).unwrap(),
        DebconfCommand::Data { value, .. } if value == b"one\ntwo"
    ));
    assert!(matches!(
        parse_command(b"PROGRESS START 0 100 pkg/progress", false).unwrap(),
        DebconfCommand::Progress(DebconfProgressCommand::Start {
            minimum: 0,
            maximum: 100,
            ..
        })
    ));
    assert!(matches!(
        parse_command(b"PROGRESS SET 30", false).unwrap(),
        DebconfCommand::Progress(DebconfProgressCommand::Set { value: 30 })
    ));
    assert!(matches!(
        parse_command(b"PROGRESS STEP -2", false).unwrap(),
        DebconfCommand::Progress(DebconfProgressCommand::Step { increment: -2 })
    ));
    assert!(matches!(
        parse_command(b"PROGRESS INFO pkg/progress", false).unwrap(),
        DebconfCommand::Progress(DebconfProgressCommand::Info { .. })
    ));
    assert_eq!(
        parse_command(b"PROGRESS STOP", false).unwrap(),
        DebconfCommand::Progress(DebconfProgressCommand::Stop)
    );
    assert!(matches!(
        parse_command(b"X_LOADTEMPLATEFILE /tmp/templates pkg", false).unwrap(),
        DebconfCommand::XLoadTemplateFile { owner: Some(_), .. }
    ));
    assert!(matches!(
        parse_command(b"VISIBLE high pkg/name", false).unwrap(),
        DebconfCommand::Visible { .. }
    ));
    assert!(matches!(
        parse_command(b"EXIST pkg/name", false).unwrap(),
        DebconfCommand::Exist { .. }
    ));
}

#[test]
fn wire_and_argv_parsers_are_strict_without_losing_argument_bytes() {
    let escaped = parse_command(b"SET pkg/name hello\\ world\\\\tail\\nsecond", true).unwrap();
    assert!(matches!(
        escaped,
        DebconfCommand::Set { value, .. } if value == b"hello world\\tail\nsecond"
    ));

    let argv = parse_argv(
        b"SET",
        &[
            b"pkg/name".to_vec(),
            b"repeated  whitespace\nand bytes".to_vec(),
            Vec::new(),
        ],
    )
    .unwrap();
    assert!(matches!(
        argv,
        DebconfCommand::Set { value, .. }
            if value == b"repeated  whitespace\nand bytes "
    ));
    assert!(matches!(
        parse_argv(
            b"CAPB",
            &[b"escape".to_vec(), b"vendor capability".to_vec()]
        )
        .unwrap(),
        DebconfCommand::Capb { capabilities }
            if capabilities
                == vec![b"escape".to_vec(), b"vendor capability".to_vec()]
    ));

    for malformed in [
        b"".as_slice(),
        b"UNKNOWN thing",
        b"GET",
        b"GO extra",
        b"STOP extra",
        b"BEGINBLOCK extra",
        b"FSET pkg/name seen maybe",
        b"PROGRESS START 0 nope pkg/name",
        b"X_LOADTEMPLATEFILE",
    ] {
        let error = parse_command(malformed, false).expect_err("must reject malformed command");
        assert_eq!(
            error.response().code,
            DebconfResultCode::SyntaxError,
            "{malformed:?}"
        );
    }
    assert!(parse_argv(b"BAD COMMAND", &[]).is_err());
}

#[test]
fn return_codes_wire_bytes_and_caller_ret_are_exact() {
    assert_eq!(DebconfResultCode::Success.as_u16(), 0);
    assert_eq!(DebconfResultCode::EscapedData.as_u16(), 1);
    assert_eq!(DebconfResultCode::InvalidParameters.as_u16(), 10);
    assert_eq!(DebconfResultCode::SyntaxError.as_u16(), 20);
    assert_eq!(DebconfResultCode::CommandSpecific.as_u16(), 30);
    assert_eq!(DebconfResultCode::InternalError.as_u16(), 100);

    let mut state = DebconfState::default();
    state
        .import_template_records("pkg", &[string_record("pkg/name", "default")])
        .unwrap();
    let mut engine = DebconfEngine::new(QueueLoader::default());
    let mut escaped_session = DebconfSession::new("pkg", DebconfSessionPolicy::default());
    assert_code(
        &line(
            &mut engine,
            &mut state,
            &mut escaped_session,
            b"CAPB escape",
        ),
        DebconfResultCode::Success,
    );
    let exact = b"first\nsecond\\tail\xff".to_vec();
    let set = response(engine.process_argv(
        &mut state,
        &mut escaped_session,
        b"SET",
        &[b"pkg/name".to_vec(), exact.clone()],
    ));
    assert_code(&set, DebconfResultCode::Success);
    let get = line(
        &mut engine,
        &mut state,
        &mut escaped_session,
        b"GET pkg/name",
    );
    assert_eq!(get.code, DebconfResultCode::EscapedData);
    assert_eq!(get.ret, b"first\\nsecond\\\\tail\xff");
    assert_eq!(get.wire_bytes(), b"1 first\\nsecond\\\\tail\xff\n");
    assert_eq!(
        get.caller_result(),
        DebconfCallerResult {
            status: DebconfResultCode::Success,
            ret: exact,
        }
    );

    let mut plain_session = DebconfSession::new("pkg", DebconfSessionPolicy::default());
    let plain = line(&mut engine, &mut state, &mut plain_session, b"GET pkg/name");
    assert_eq!(plain.code, DebconfResultCode::Success);
    assert_eq!(plain.ret, b"first");
}
