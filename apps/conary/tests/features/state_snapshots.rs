// conary/tests/features/state_snapshots.rs

use super::*;

// =============================================================================
// STATE SNAPSHOT TESTS
// =============================================================================

/// Test state snapshot operations (equivalent to cmd_state_*)
#[test]
fn test_state_snapshot_operations() {
    use conary_core::db::models::{StateEngine, SystemState};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    let engine = StateEngine::new(&conn);

    // Create a snapshot
    let state = engine.create_snapshot("Test snapshot", None, None).unwrap();
    assert!(state.id.is_some());
    assert_eq!(state.summary, "Test snapshot");
    // First state is numbered 0 (state_number starts at 0, not 1)
    assert_eq!(state.state_number, 0);

    // List snapshots using SystemState::list_all
    let states = SystemState::list_all(&conn).unwrap();
    assert!(!states.is_empty(), "Should have at least one state");

    // Create another snapshot
    let state2 = engine
        .create_snapshot("Second snapshot", None, None)
        .unwrap();
    assert!(state2.state_number > state.state_number);

    // Get latest state (highest state_number from list)
    let states = SystemState::list_all(&conn).unwrap();
    assert!(states.len() >= 2);
    // list_all returns DESC order, so first is latest
    assert_eq!(states[0].summary, "Second snapshot");
}
