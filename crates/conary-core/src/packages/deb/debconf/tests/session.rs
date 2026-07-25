// conary-core/src/packages/deb/debconf/tests/session.rs

use super::super::*;
use super::support::*;

#[test]
fn noninteractive_priority_seen_and_repeated_sessions_are_deterministic() {
    let mut state = DebconfState::default();
    state
        .import_template_records("pkg", &[string_record("pkg/name", "fallback")])
        .unwrap();
    let mut engine = DebconfEngine::new(QueueLoader::default());

    let mut first = DebconfSession::new("pkg", DebconfSessionPolicy::default());
    let input = line(&mut engine, &mut state, &mut first, b"INPUT low pkg/name");
    assert_eq!(input.code, DebconfResultCode::CommandSpecific);
    assert_eq!(input.ret, b"question skipped");
    assert_eq!(first.pending.len(), 1);
    assert!(!first.pending[0].mark_seen);
    assert!(first.busy.contains("pkg/name"));
    assert_code(
        &line(&mut engine, &mut state, &mut first, b"GO"),
        DebconfResultCode::Success,
    );
    assert_eq!(
        state.questions["pkg/name"].value,
        Some(b"fallback".to_vec())
    );
    assert!(!state.questions["pkg/name"].seen);
    assert_eq!(
        engine.process_line(&mut state, &mut first, b"STOP"),
        DebconfExecution::Stopped
    );
    assert!(!state.questions["pkg/name"].seen);

    let policy = DebconfSessionPolicy {
        noninteractive_seen: true,
        ..DebconfSessionPolicy::default()
    };
    let mut second = DebconfSession::new("pkg", policy);
    line(&mut engine, &mut state, &mut second, b"INPUT high pkg/name");
    assert!(second.pending[0].mark_seen);
    line(&mut engine, &mut state, &mut second, b"GO");
    assert!(!state.questions["pkg/name"].seen);
    assert!(second.seen.contains("pkg/name"));
    assert_eq!(
        engine.process_line(&mut state, &mut second, b"STOP"),
        DebconfExecution::Stopped
    );
    assert!(state.questions["pkg/name"].seen);

    let mut third = DebconfSession::new("pkg", policy);
    line(&mut engine, &mut state, &mut third, b"INPUT high pkg/name");
    assert!(!third.pending[0].mark_seen);
    line(&mut engine, &mut state, &mut third, b"CLEAR");
    let fset = line(
        &mut engine,
        &mut state,
        &mut third,
        b"FSET pkg/name seen false",
    );
    assert_eq!(fset.ret, b"false");
    line(&mut engine, &mut state, &mut third, b"INPUT high pkg/name");
    assert!(third.pending[0].mark_seen);

    let session_json = serde_json::to_vec(&third).unwrap();
    assert_eq!(
        serde_json::from_slice::<DebconfSession>(&session_json).unwrap(),
        third
    );
}

#[test]
fn backup_and_progress_cancel_are_explicit_session_outcomes() {
    let mut state = DebconfState::default();
    state
        .import_template_records("pkg", &[string_record("pkg/name", "fallback")])
        .unwrap();
    let mut engine = DebconfEngine::new(QueueLoader::default());
    let mut session = DebconfSession::new("pkg", DebconfSessionPolicy::default());

    assert_eq!(
        session.request_go_outcome(DebconfGoOutcome::Back),
        Err(DebconfSessionRequestError::BackupCapabilityNotNegotiated)
    );
    line(&mut engine, &mut state, &mut session, b"CAPB backup");
    line(
        &mut engine,
        &mut state,
        &mut session,
        b"INPUT high pkg/name",
    );
    session.request_go_outcome(DebconfGoOutcome::Back).unwrap();
    let backed_up = line(&mut engine, &mut state, &mut session, b"GO");
    assert_eq!(backed_up.code, DebconfResultCode::CommandSpecific);
    assert_eq!(backed_up.ret, b"backup");
    assert!(session.backed_up);
    assert!(session.pending.is_empty());
    assert_eq!(state.questions["pkg/name"].value, None);

    line(
        &mut engine,
        &mut state,
        &mut session,
        b"INPUT high pkg/name",
    );
    let invisible_backup = line(&mut engine, &mut state, &mut session, b"GO");
    assert_eq!(invisible_backup.code, DebconfResultCode::CommandSpecific);
    assert_eq!(invisible_backup.ret, b"backup");

    assert_code(
        &line(
            &mut engine,
            &mut state,
            &mut session,
            b"PROGRESS START 0 10 pkg/name",
        ),
        DebconfResultCode::Success,
    );
    session
        .request_progress_outcome(DebconfProgressOutcome::Cancel)
        .unwrap();
    let canceled = line(&mut engine, &mut state, &mut session, b"PROGRESS SET 4");
    assert_eq!(canceled.code, DebconfResultCode::CommandSpecific);
    assert_eq!(canceled.ret, b"CANCELED");
    assert_eq!(session.progress.as_ref().unwrap().current, 0);
    line(&mut engine, &mut state, &mut session, b"PROGRESS SET 4");
    line(&mut engine, &mut state, &mut session, b"PROGRESS STEP 3");
    assert_eq!(session.progress.as_ref().unwrap().current, 7);
    line(
        &mut engine,
        &mut state,
        &mut session,
        b"PROGRESS INFO pkg/name",
    );
    assert_eq!(
        session.progress.as_ref().unwrap().info.as_deref(),
        Some("pkg/name")
    );
    line(&mut engine, &mut state, &mut session, b"PROGRESS STOP");
    assert!(session.progress.is_none());
    assert_eq!(
        line(&mut engine, &mut state, &mut session, b"PROGRESS SET 1",).code,
        DebconfResultCode::InvalidParameters
    );
    assert_eq!(
        line(
            &mut engine,
            &mut state,
            &mut session,
            b"PROGRESS START 3 2 pkg/name",
        )
        .code,
        DebconfResultCode::SyntaxError
    );
}

#[test]
fn substitutions_metaget_flags_reset_and_select_defaults_are_typed() {
    let select = record(
        "pkg/select",
        "select",
        vec![
            field("Default", "not-a-choice", &[]),
            field("Choices", "Localized one, Localized two", &[]),
            field("Choices-C", "coded\\, one, coded2", &[]),
            field("Description", "Hello ${NAME}; literal \\${UNCHANGED}", &[]),
        ],
    );
    let mut state = DebconfState::default();
    state.import_template_records("pkg", &[select]).unwrap();
    let mut engine = DebconfEngine::new(QueueLoader::default());
    let mut session = DebconfSession::new("pkg", DebconfSessionPolicy::default());

    line(
        &mut engine,
        &mut state,
        &mut session,
        b"SUBST pkg/select NAME operator",
    );
    let description = line(
        &mut engine,
        &mut state,
        &mut session,
        b"METAGET pkg/select description",
    );
    assert_eq!(description.ret, b"Hello operator; literal ${UNCHANGED}");
    line(
        &mut engine,
        &mut state,
        &mut session,
        b"INPUT high pkg/select",
    );
    line(&mut engine, &mut state, &mut session, b"GO");
    assert_eq!(
        state.questions["pkg/select"].value,
        Some(b"coded, one".to_vec())
    );

    line(
        &mut engine,
        &mut state,
        &mut session,
        b"FSET pkg/select custom true",
    );
    assert_eq!(
        line(
            &mut engine,
            &mut state,
            &mut session,
            b"FGET pkg/select custom",
        )
        .ret,
        b"true"
    );
    assert_eq!(
        line(
            &mut engine,
            &mut state,
            &mut session,
            b"FGET pkg/select unknown",
        )
        .ret,
        b"false"
    );
    line(
        &mut engine,
        &mut state,
        &mut session,
        b"FSET pkg/select isdefault false",
    );
    assert!(state.questions["pkg/select"].seen);
    line(&mut engine, &mut state, &mut session, b"RESET pkg/select");
    assert_eq!(
        state.questions["pkg/select"].value,
        Some(b"not-a-choice".to_vec())
    );
    assert!(!state.questions["pkg/select"].seen);
}
