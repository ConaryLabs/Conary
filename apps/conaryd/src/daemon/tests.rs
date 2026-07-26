// apps/conaryd/src/daemon/tests.rs

use super::*;

#[test]
fn test_default_config_uses_canonical_daemon_defaults() {
    let config = DaemonConfig::default();

    assert_eq!(config.db_path, PathBuf::from(DaemonConfig::DEFAULT_DB_PATH));
    assert_eq!(
        config.socket_path,
        PathBuf::from(DaemonConfig::DEFAULT_SOCKET_PATH)
    );
    assert_eq!(config.lock_path, PathBuf::from(SystemLock::DEFAULT_PATH));
    assert_eq!(config.socket_mode, DaemonConfig::DEFAULT_SOCKET_MODE);
    assert_eq!(
        config.tcp_bind.as_deref(),
        Some(DaemonConfig::DEFAULT_TCP_BIND)
    );
    assert!(!config.enable_tcp);
}

#[test]
fn test_daemon_error_serialization() {
    let error = DaemonError::not_found("package nginx");
    let json = serde_json::to_string(&error).unwrap();

    assert!(json.contains("not_found"));
    assert!(json.contains("nginx"));
}

#[test]
fn test_daemon_event_serialization() {
    let event = DaemonEvent::JobProgress {
        job_id: "test-123".to_string(),
        current: 50,
        total: 100,
        message: "Installing nginx".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();

    assert!(json.contains("job_progress"));
    assert!(json.contains("test-123"));
    assert!(json.contains("Installing nginx"));
}

#[test]
fn test_job_status() {
    let status = JobStatus::Running;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"running\"");

    let parsed: JobStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, JobStatus::Running);
}
