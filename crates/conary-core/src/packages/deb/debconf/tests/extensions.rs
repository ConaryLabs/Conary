// conary-core/src/packages/deb/debconf/tests/extensions.rs

use super::super::*;
use super::support::*;

#[test]
fn data_title_info_blocks_and_deprecated_queries_have_explicit_state() {
    let mut state = DebconfState::default();
    let mut engine = DebconfEngine::new(QueueLoader::default());
    let mut session = DebconfSession::new("pkg", DebconfSessionPolicy::default());

    assert_code(
        &line(
            &mut engine,
            &mut state,
            &mut session,
            b"DATA dynamic/name type string",
        ),
        DebconfResultCode::Success,
    );
    line(
        &mut engine,
        &mut state,
        &mut session,
        b"DATA dynamic/name description First\\nSecond",
    );
    assert_eq!(
        state.templates["dynamic/name"].fields["description"],
        b"First\nSecond"
    );
    assert_eq!(
        line(
            &mut engine,
            &mut state,
            &mut session,
            b"DATA dynamic/name type boolean",
        )
        .code,
        DebconfResultCode::InvalidParameters
    );
    line(
        &mut engine,
        &mut state,
        &mut session,
        b"SETTITLE dynamic/name",
    );
    assert_eq!(session.title, b"First\nSecond");
    let title = response(engine.process_argv(
        &mut state,
        &mut session,
        b"TITLE",
        &[b"exact  title\nbytes".to_vec()],
    ));
    assert_code(&title, DebconfResultCode::Success);
    assert_eq!(session.title, b"exact  title\nbytes");

    line(&mut engine, &mut state, &mut session, b"BEGINBLOCK");
    line(&mut engine, &mut state, &mut session, b"BEGINBLOCK");
    line(
        &mut engine,
        &mut state,
        &mut session,
        b"INPUT high dynamic/name",
    );
    assert_eq!(session.pending[0].block_path, [1, 2]);
    line(&mut engine, &mut state, &mut session, b"ENDBLOCK");
    line(&mut engine, &mut state, &mut session, b"ENDBLOCK");
    assert_eq!(
        line(&mut engine, &mut state, &mut session, b"ENDBLOCK",).code,
        DebconfResultCode::SyntaxError
    );

    line(&mut engine, &mut state, &mut session, b"INFO dynamic/name");
    assert_eq!(session.info.as_deref(), Some("dynamic/name"));
    line(&mut engine, &mut state, &mut session, b"INFO");
    assert_eq!(session.info, None);
    assert_eq!(
        line(&mut engine, &mut state, &mut session, b"EXIST dynamic/name",).ret,
        b"true"
    );
    assert_eq!(
        line(&mut engine, &mut state, &mut session, b"EXIST missing/name",).ret,
        b"false"
    );
    assert_eq!(
        line(
            &mut engine,
            &mut state,
            &mut session,
            b"VISIBLE high dynamic/name",
        )
        .ret,
        b"false"
    );
}

#[test]
fn typed_loader_maps_open_parse_and_owner_override_without_io() {
    let loaded = string_record("loaded/name", "loaded");
    let mut loader = QueueLoader::default();
    loader
        .results
        .push_back(Err(DebconfTemplateLoadError::open(b"denied".to_vec())));
    loader
        .results
        .push_back(Err(DebconfTemplateLoadError::parse(
            b"bad\nrecord".to_vec(),
        )));
    loader.results.push_back(Ok(vec![loaded]));
    let mut engine = DebconfEngine::new(loader);
    let mut state = DebconfState::default();
    let mut session = DebconfSession::new("pkg-default", DebconfSessionPolicy::default());

    let open = response(engine.process_argv(
        &mut state,
        &mut session,
        b"X_LOADTEMPLATEFILE",
        &[b"/path with spaces".to_vec()],
    ));
    assert_eq!(open.code, DebconfResultCode::InvalidParameters);
    assert_eq!(open.ret, b"failed to open /path with spaces: denied");

    let parse = response(engine.process_argv(
        &mut state,
        &mut session,
        b"X_LOADTEMPLATEFILE",
        &[b"/parse".to_vec()],
    ));
    assert_eq!(parse.code, DebconfResultCode::InternalError);
    assert_eq!(parse.ret, b"bad\\nrecord");

    let loaded = response(engine.process_argv(
        &mut state,
        &mut session,
        b"X_LOADTEMPLATEFILE",
        &[b"/success".to_vec(), b"pkg-override".to_vec()],
    ));
    assert_code(&loaded, DebconfResultCode::Success);
    assert_eq!(
        state.questions["loaded/name"]
            .owners
            .iter()
            .collect::<Vec<_>>(),
        ["pkg-override"]
    );
    assert_eq!(
        engine.loader().paths,
        [
            b"/path with spaces".to_vec(),
            b"/parse".to_vec(),
            b"/success".to_vec()
        ]
    );
}

#[test]
fn stop_persists_session_seen_and_has_no_reply() {
    let mut state = DebconfState::default();
    state
        .import_template_records("pkg", &[string_record("pkg/name", "default")])
        .unwrap();
    let mut engine = DebconfEngine::new(QueueLoader::default());
    let policy = DebconfSessionPolicy {
        noninteractive_seen: true,
        ..DebconfSessionPolicy::default()
    };
    let mut session = DebconfSession::new("pkg", policy);
    line(
        &mut engine,
        &mut state,
        &mut session,
        b"INPUT high pkg/name",
    );
    line(&mut engine, &mut state, &mut session, b"GO");
    assert!(!state.questions["pkg/name"].seen);
    assert_eq!(
        engine.process_line(&mut state, &mut session, b"STOP"),
        DebconfExecution::Stopped
    );
    assert!(session.stopped);
    assert!(state.questions["pkg/name"].seen);
    assert_eq!(
        response(engine.process_line(&mut state, &mut session, b"GET pkg/name")).code,
        DebconfResultCode::InternalError
    );
}
