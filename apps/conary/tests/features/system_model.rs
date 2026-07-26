// conary/tests/features/system_model.rs

use super::*;

// =============================================================================
// SYSTEM MODEL TESTS
// =============================================================================

/// Test system model parsing
#[test]
fn test_system_model_parsing() {
    use conary_core::model::parse_model_file;
    use std::io::Write;

    let temp_dir = tempfile::tempdir().unwrap();
    let model_path = temp_dir.path().join("system.toml");

    // Write a test model file
    let mut file = std::fs::File::create(&model_path).unwrap();
    writeln!(
        file,
        r#"
[model]
version = 1
search = ["fedora@f41:stable"]
install = ["nginx", "postgresql"]
exclude = ["sendmail"]

[pin]
openssl = "3.0.*"

[optional]
packages = ["nginx-module-geoip"]

[[derive]]
name = "nginx-custom"
from = "nginx"
version = "inherit"
patches = []
"#
    )
    .unwrap();

    // Parse the model
    let model = parse_model_file(&model_path).unwrap();

    assert_eq!(model.config.version, 1);
    assert_eq!(model.config.install, vec!["nginx", "postgresql"]);
    assert_eq!(model.config.exclude, vec!["sendmail"]);
    assert_eq!(model.config.search, vec!["fedora@f41:stable"]);
    assert_eq!(model.pin.get("openssl"), Some(&"3.0.*".to_string()));
    assert_eq!(model.optional.packages, vec!["nginx-module-geoip"]);
    assert_eq!(model.derive.len(), 1);
    assert_eq!(model.derive[0].name, "nginx-custom");
    assert_eq!(model.derive[0].from, "nginx");
}

/// Test system model diff computation
#[test]
fn test_system_model_diff() {
    use conary_core::model::parse_model_file;
    use conary_core::model::{DiffAction, SystemState, compute_diff};
    use std::io::Write;

    let temp_dir = tempfile::tempdir().unwrap();
    let model_path = temp_dir.path().join("system.toml");

    // Write a model requesting nginx and redis
    let mut file = std::fs::File::create(&model_path).unwrap();
    writeln!(
        file,
        r#"
[model]
version = 1
install = ["nginx", "redis"]
exclude = ["sendmail"]
"#
    )
    .unwrap();

    let model = parse_model_file(&model_path).unwrap();

    // Create a state with only nginx installed
    let mut state = SystemState::new();
    state.add_package(
        "nginx".to_string(),
        conary_core::model::InstalledPackage {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            architecture: None,
            explicit: true,
            pinned: false,
            label: None,
        },
    );

    // Also have sendmail installed (should be removed)
    state.add_package(
        "sendmail".to_string(),
        conary_core::model::InstalledPackage {
            name: "sendmail".to_string(),
            version: "8.0.0".to_string(),
            architecture: None,
            explicit: true,
            pinned: false,
            label: None,
        },
    );

    // Compute diff
    let diff = compute_diff(&model, &state);

    // Should need to install redis
    assert!(
        diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::Install { package, .. } if package == "redis"
        )),
        "Should need to install redis"
    );

    // Should need to remove sendmail (excluded)
    assert!(
        diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::Remove { package, .. } if package == "sendmail"
        )),
        "Should need to remove sendmail"
    );

    // nginx is already installed, no action needed for it
    assert!(
        !diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::Install { package, .. } if package == "nginx"
        )),
        "Should not need to install nginx again"
    );
}

/// Test system model diff with derived packages
#[test]
fn test_system_model_diff_derived() {
    use conary_core::model::parse_model_file;
    use conary_core::model::{DiffAction, SystemState, compute_diff};
    use std::io::Write;

    let temp_dir = tempfile::tempdir().unwrap();
    let model_path = temp_dir.path().join("system.toml");

    // Write a model with a derived package
    let mut file = std::fs::File::create(&model_path).unwrap();
    writeln!(
        file,
        r#"
[model]
version = 1
install = ["nginx"]

[[derive]]
name = "nginx-custom"
from = "nginx"
version = "inherit"
patches = []
"#
    )
    .unwrap();

    let model = parse_model_file(&model_path).unwrap();

    // State with nginx installed but not the derived package
    let mut state = SystemState::new();
    state.add_package(
        "nginx".to_string(),
        conary_core::model::InstalledPackage {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            architecture: None,
            explicit: true,
            pinned: false,
            label: None,
        },
    );

    // Compute diff
    let diff = compute_diff(&model, &state);

    // Should need to build the derived package
    assert!(
        diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::BuildDerived { name, parent, needs_parent }
            if name == "nginx-custom" && parent == "nginx" && !*needs_parent
        )),
        "Should need to build derived package with parent already installed"
    );
}

/// Test system model state capture
#[test]
fn test_system_model_state_capture() {
    use conary_core::model::capture_current_state;

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Capture current state
    let state = capture_current_state(&conn).unwrap();

    // Should have nginx and openssl from the test db
    assert!(state.is_installed("nginx"), "nginx should be installed");
    assert!(state.is_installed("openssl"), "openssl should be installed");

    // Check nginx details
    let nginx = state.get_package("nginx").unwrap();
    assert_eq!(nginx.version, "1.24.0");
}

/// Test system model snapshot to model conversion
#[test]
fn test_system_model_snapshot() {
    use conary_core::db::models::InstallReason;
    use conary_core::model::{capture_current_state, snapshot_to_model};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Mark nginx as explicit
    db::transaction(&mut conn, |tx| {
        tx.execute(
            "UPDATE troves SET install_reason = ?1 WHERE name = ?2",
            rusqlite::params![InstallReason::Explicit.as_str(), "nginx"],
        )?;
        Ok(())
    })
    .unwrap();

    // Capture state and convert to model
    let state = capture_current_state(&conn).unwrap();
    let model = snapshot_to_model(&state);

    // Model should include explicitly installed packages
    assert!(
        model.config.install.contains(&"nginx".to_string()),
        "Model should include explicitly installed nginx"
    );
}
