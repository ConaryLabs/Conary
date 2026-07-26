// conary-core/src/packages/deb/debconf/tests/state.rs

use super::super::*;
use super::support::*;

#[test]
fn template_import_matches_upstream_folding_and_is_transactional() {
    let source = record(
        "pkg/name",
        "string",
        vec![
            field("Default", "fallback  ", &[]),
            field(
                "Description",
                "Short description  ",
                &[" first continuation", " .", "  * literal list item  "],
            ),
            field("Description-pt_BR.UTF-8", "Escolha", &[]),
            field("X-Vendor.Field", "retained", &[]),
        ],
    );
    let mut state = DebconfState::default();
    state
        .import_template_records("pkg", std::slice::from_ref(&source))
        .unwrap();

    let template = &state.templates["pkg/name"];
    assert_eq!(template.template_type, "string");
    assert_eq!(template.fields["default"], b"fallback");
    assert_eq!(template.fields["description"], b"Short description");
    assert_eq!(
        template.fields["extended_description"],
        b"first continuation\n\n * literal list item"
    );
    assert_eq!(
        template.fields["description-pt_br.utf-8"],
        "Escolha".as_bytes()
    );
    assert_eq!(template.fields["x-vendor.field"], b"retained");
    assert_eq!(template.owners.iter().collect::<Vec<_>>(), ["pkg/name"]);
    assert_eq!(
        state.questions["pkg/name"]
            .owners
            .iter()
            .collect::<Vec<_>>(),
        ["pkg"]
    );
    assert_eq!(state.effective_value("pkg/name").unwrap(), b"fallback");

    let serialized = serde_json::to_vec(&state).unwrap();
    assert_eq!(
        serde_json::from_slice::<DebconfState>(&serialized).unwrap(),
        state
    );

    let before = state.clone();
    let mut invalid = source;
    invalid.fields.push(field("DESCRIPTION", "duplicate", &[]));
    assert!(state.import_template_records("other", &[invalid]).is_err());
    assert_eq!(state, before);

    let mut mismatched = string_record("pkg/other", "x");
    mismatched.template = "pkg/not-the-field".to_string();
    assert!(
        state
            .import_template_records("other", &[mismatched])
            .is_err()
    );
    assert_eq!(state, before);
}

#[test]
fn shared_question_and_template_ownership_survives_unregister_and_purge() {
    let template = string_record("shared/name", "value");
    let mut state = DebconfState::default();
    state
        .import_template_records("pkg-a", std::slice::from_ref(&template))
        .unwrap();
    state
        .import_template_records("pkg-b", std::slice::from_ref(&template))
        .unwrap();
    assert_eq!(
        state.questions["shared/name"]
            .owners
            .iter()
            .collect::<Vec<_>>(),
        ["pkg-a", "pkg-b"]
    );

    let mut engine = DebconfEngine::new(QueueLoader::default());
    let mut session_a = DebconfSession::new("pkg-a", DebconfSessionPolicy::default());
    let mut session_b = DebconfSession::new("pkg-b", DebconfSessionPolicy::default());
    assert_code(
        &line(
            &mut engine,
            &mut state,
            &mut session_a,
            b"REGISTER shared/name shared/alias",
        ),
        DebconfResultCode::Success,
    );
    assert_code(
        &line(
            &mut engine,
            &mut state,
            &mut session_b,
            b"REGISTER shared/name shared/alias",
        ),
        DebconfResultCode::Success,
    );
    assert_eq!(
        state.questions["shared/alias"]
            .owners
            .iter()
            .collect::<Vec<_>>(),
        ["pkg-a", "pkg-b"]
    );
    assert_eq!(
        state.templates["shared/name"]
            .owners
            .iter()
            .collect::<Vec<_>>(),
        ["shared/alias", "shared/name"]
    );

    assert_code(
        &line(
            &mut engine,
            &mut state,
            &mut session_a,
            b"INPUT high shared/alias",
        ),
        DebconfResultCode::CommandSpecific,
    );
    let busy = line(
        &mut engine,
        &mut state,
        &mut session_a,
        b"UNREGISTER shared/alias",
    );
    assert_code(&busy, DebconfResultCode::InvalidParameters);
    line(&mut engine, &mut state, &mut session_a, b"CLEAR");
    assert_code(
        &line(
            &mut engine,
            &mut state,
            &mut session_a,
            b"UNREGISTER shared/alias",
        ),
        DebconfResultCode::Success,
    );
    assert_eq!(
        state.questions["shared/alias"]
            .owners
            .iter()
            .collect::<Vec<_>>(),
        ["pkg-b"]
    );

    assert_code(
        &line(&mut engine, &mut state, &mut session_b, b"PURGE"),
        DebconfResultCode::Success,
    );
    assert!(!state.questions.contains_key("shared/alias"));
    assert_eq!(
        state.questions["shared/name"]
            .owners
            .iter()
            .collect::<Vec<_>>(),
        ["pkg-a"]
    );
    assert!(state.templates.contains_key("shared/name"));

    assert_code(
        &line(&mut engine, &mut state, &mut session_a, b"PURGE"),
        DebconfResultCode::Success,
    );
    assert!(state.questions.is_empty());
    assert!(state.templates.is_empty());
}
