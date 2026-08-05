// apps/conary/src/commands/try_session/watch/tests.rs
use super::*;
use conary_core::diagnostics::{PackagingCommandStatus, PackagingEventKind, PackagingPhase};

#[test]
fn watch_event_builder_assigns_monotonic_sequences() {
    let mut events = WatchEvents::new("watch-1");
    let first = events.push(
        PackagingPhase::TrySession,
        PackagingEventKind::WatchStarted,
        "Watching .",
    );
    let second = events.push(
        PackagingPhase::Build,
        PackagingEventKind::WatchRefreshStarted,
        "cooking",
    );

    assert_eq!(first.sequence, 1);
    assert_eq!(second.sequence, 2);
    assert_eq!(events.all().len(), 2);
}

#[test]
fn watch_record_output_is_bounded_and_redacted() {
    let mut events = WatchEvents::new("watch-1");
    for index in 0..505 {
        events.push(
            PackagingPhase::Build,
            PackagingEventKind::WatchRefreshFailed,
            format!("API_TOKEN=secret event {index}"),
        );
    }

    let output = events.into_command_output(PackagingCommandStatus::Failed, "done");
    assert_eq!(output.events.len(), 500);
    let rendered = serde_json::to_string(&crate::commands::diagnostics::redacted_packaging_output(
        &output,
    ))
    .unwrap();
    assert!(!rendered.contains("API_TOKEN=secret"), "{rendered}");
    assert!(
        rendered.contains("older watch events were omitted"),
        "{rendered}"
    );
}

#[test]
fn watch_state_suppresses_duplicate_attempt_until_result() {
    let first = WatchIdentity {
        digest: "sha256:first".to_string(),
        file_count: 1,
    };
    let mut state = WatchRefreshState::new(first.clone(), 41);

    assert!(
        !state.should_attempt(&first),
        "initial successful source snapshot should not rebuild without a change"
    );
    let broken = WatchIdentity {
        digest: "sha256:broken".to_string(),
        file_count: 1,
    };
    assert!(state.should_attempt(&broken));
    state.record_attempt(broken.clone());

    assert!(
        !state.should_attempt(&broken),
        "same source snapshot should not start a duplicate in-flight rebuild"
    );
    state.record_failure();
    assert!(state.should_attempt(&broken));
    assert!(
        !state.should_attempt(&first),
        "returning to the last successful source snapshot is already active"
    );
    let changed = WatchIdentity {
        digest: "sha256:changed".to_string(),
        file_count: 1,
    };
    assert!(state.should_attempt(&changed));
}

#[test]
fn watch_state_retries_same_identity_after_failed_attempt() {
    let first = WatchIdentity {
        digest: "sha256:first".to_string(),
        file_count: 1,
    };
    let mut state = WatchRefreshState::new(first, 41);
    let broken = WatchIdentity {
        digest: "sha256:broken".to_string(),
        file_count: 1,
    };

    state.record_attempt(broken.clone());
    state.record_failure();

    assert!(
        state.should_attempt(&broken),
        "failed source snapshots should be retryable after the visible failure"
    );
}

#[test]
fn watched_sources_missing_ignores_recipe_and_auxiliary_files() {
    let temp = tempfile::tempdir().unwrap();
    let source_root = temp.path().join("src");
    std::fs::create_dir_all(&source_root).unwrap();
    let source_set = WatchSourceSet {
        recipe_path: temp.path().join("missing-recipe.toml"),
        local_roots: vec![source_root.clone()],
        local_files: vec![temp.path().join("missing.patch")],
    };

    assert!(
        !watched_sources_missing(&source_set),
        "transient recipe, patch, or additional-file disappearance should be non-destructive"
    );

    std::fs::remove_dir_all(source_root).unwrap();
    assert!(
        watched_sources_missing(&source_set),
        "source root disappearance should still stop the watch session"
    );
}

#[test]
fn refresh_failure_termination_uses_typed_divergence_not_prose() {
    let divergence = anyhow::Error::new(TryRefreshSessionDiverged::new("try-1"))
        .context("diagnostic wording can change independently");
    assert!(refresh_failure_stops_watch(&divergence));

    let diagnostic_prose = anyhow::anyhow!("try watch session changed outside the watcher");
    assert!(!refresh_failure_stops_watch(&diagnostic_prose));
}

#[test]
fn debounce_step_resets_deadline_when_identity_changes_before_ready() {
    let start = Instant::now();
    let mut debounce = DebounceState::new(Duration::from_millis(750));
    let success = WatchIdentity {
        digest: "sha256:success".to_string(),
        file_count: 1,
    };
    let first_change = WatchIdentity {
        digest: "sha256:first-change".to_string(),
        file_count: 1,
    };
    let second_change = WatchIdentity {
        digest: "sha256:second-change".to_string(),
        file_count: 1,
    };
    let mut state = WatchRefreshState::new(success, 41);

    assert_eq!(
        debounce_refresh_identity(&mut state, &mut debounce, first_change, start),
        DebouncedRefresh::Waiting
    );
    assert_eq!(
        debounce.ready_at(),
        Some(start + Duration::from_millis(750))
    );

    assert_eq!(
        debounce_refresh_identity(
            &mut state,
            &mut debounce,
            second_change.clone(),
            start + Duration::from_millis(100)
        ),
        DebouncedRefresh::Waiting
    );
    assert_eq!(
        debounce.ready_at(),
        Some(start + Duration::from_millis(850))
    );
    assert_eq!(
        debounce_refresh_identity(
            &mut state,
            &mut debounce,
            second_change.clone(),
            start + Duration::from_millis(849)
        ),
        DebouncedRefresh::Waiting
    );
    assert_eq!(
        debounce_refresh_identity(
            &mut state,
            &mut debounce,
            second_change,
            start + Duration::from_millis(850)
        ),
        DebouncedRefresh::Ready
    );
}
