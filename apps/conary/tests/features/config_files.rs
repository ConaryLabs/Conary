// conary/tests/features/config_files.rs

use super::*;

// =============================================================================
// CONFIG FILE TESTS
// =============================================================================

/// Test config file tracking (equivalent to config management commands)
#[test]
fn test_config_file_tracking() {
    use conary_core::db::models::{ConfigFile, ConfigSource, ConfigStatus, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Get nginx trove id
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap().pop().unwrap();
    let nginx_id = nginx.id.unwrap();

    // Create config file entries
    let mut config1 = ConfigFile::new(
        "/etc/nginx/nginx.conf".to_string(),
        nginx_id,
        "abc123hash".to_string(),
    );
    config1.source = ConfigSource::Auto;
    config1.insert(&conn).unwrap();

    let mut config2 = ConfigFile::new_noreplace(
        "/etc/nginx/sites-enabled/default".to_string(),
        nginx_id,
        "def456hash".to_string(),
    );
    config2.source = ConfigSource::Rpm;
    config2.insert(&conn).unwrap();

    // List all configs for nginx
    let configs = ConfigFile::find_by_trove(&conn, nginx_id).unwrap();
    assert_eq!(configs.len(), 2);

    // Find by path
    let found = ConfigFile::find_by_path(&conn, "/etc/nginx/nginx.conf")
        .unwrap()
        .unwrap();
    assert_eq!(found.original_hash.as_deref(), Some("abc123hash"));
    assert_eq!(found.status, ConfigStatus::Pristine);

    // Mark as modified (simulating user edit)
    found.mark_modified(&conn, "modified_hash_123").unwrap();

    // Find modified configs
    let modified = ConfigFile::find_modified(&conn).unwrap();
    assert_eq!(modified.len(), 1);
    assert_eq!(modified[0].path, "/etc/nginx/nginx.conf");

    // Verify noreplace flag
    let sites_config = ConfigFile::find_by_path(&conn, "/etc/nginx/sites-enabled/default")
        .unwrap()
        .unwrap();
    assert!(sites_config.noreplace);
    assert_eq!(sites_config.source, ConfigSource::Rpm);

    // Mark as missing
    sites_config.mark_missing(&conn).unwrap();
    let missing = ConfigFile::find_by_status(&conn, ConfigStatus::Missing).unwrap();
    assert_eq!(missing.len(), 1);
}
