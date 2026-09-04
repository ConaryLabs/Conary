// apps/conary-test/src/engine/executor/tests.rs

use super::*;
use crate::config::manifest::{FileChecksum, TestStep};

#[test]
fn from_step_run_produces_action() {
    let step = TestStep {
        run: Some("echo ${GREETING}".to_string()),
        ..TestStep::default()
    };
    let mut vars = HashMap::new();
    vars.insert("GREETING".to_string(), "hello".to_string());

    let action = StepAction::from_step(&step, &vars).unwrap();
    match action {
        StepAction::Run(cmd) => assert_eq!(cmd, "echo hello"),
        other => panic!("expected Run, got {other:?}"),
    }
}

#[test]
fn from_step_file_exists_produces_action() {
    let step = TestStep {
        file_exists: Some("/usr/bin/${TOOL}".to_string()),
        ..TestStep::default()
    };
    let mut vars = HashMap::new();
    vars.insert("TOOL".to_string(), "conary".to_string());

    let action = StepAction::from_step(&step, &vars).unwrap();
    match action {
        StepAction::FileExists(path) => {
            assert_eq!(path, PathBuf::from("/usr/bin/conary"));
        }
        other => panic!("expected FileExists, got {other:?}"),
    }
}

#[test]
fn from_step_none_for_empty() {
    let step = TestStep::default();
    assert!(StepAction::from_step(&step, &HashMap::new()).is_none());
}

#[test]
fn from_step_conary_expands_vars() {
    let step = TestStep {
        conary: Some("install ${PKG}".to_string()),
        ..TestStep::default()
    };
    let mut vars = HashMap::new();
    vars.insert("PKG".to_string(), "tree".to_string());

    let action = StepAction::from_step(&step, &vars).unwrap();
    match action {
        StepAction::Conary(args) => assert_eq!(args, "install tree"),
        other => panic!("expected Conary, got {other:?}"),
    }
}

#[test]
fn from_step_file_checksum_expands_both_fields() {
    let step = TestStep {
        file_checksum: Some(FileChecksum {
            path: "/tmp/${FILE}".to_string(),
            sha256: "${HASH}".to_string(),
        }),
        ..TestStep::default()
    };
    let mut vars = HashMap::new();
    vars.insert("FILE".to_string(), "hello.txt".to_string());
    vars.insert("HASH".to_string(), "abc123".to_string());

    let action = StepAction::from_step(&step, &vars).unwrap();
    match action {
        StepAction::FileChecksum { path, sha256 } => {
            assert_eq!(path, PathBuf::from("/tmp/hello.txt"));
            assert_eq!(sha256, "abc123");
        }
        other => panic!("expected FileChecksum, got {other:?}"),
    }
}

#[test]
fn from_step_sleep_passes_through() {
    let step = TestStep {
        sleep: Some(5),
        ..TestStep::default()
    };
    let action = StepAction::from_step(&step, &HashMap::new()).unwrap();
    match action {
        StepAction::Sleep(s) => assert_eq!(s, 5),
        other => panic!("expected Sleep, got {other:?}"),
    }
}

#[test]
fn from_step_kill_after_log_expands_conary_field() {
    let step = TestStep {
        kill_after_log: Some(KillAfterLog {
            conary: "ccs install ${PKG}".to_string(),
            pattern: "Deploying".to_string(),
            timeout_seconds: 10,
        }),
        ..TestStep::default()
    };
    let mut vars = HashMap::new();
    vars.insert("PKG".to_string(), "pkg.ccs".to_string());

    let action = StepAction::from_step(&step, &vars).unwrap();
    match action {
        StepAction::KillAfterLog(config) => {
            assert_eq!(config.conary, "ccs install pkg.ccs");
            assert_eq!(config.pattern, "Deploying");
        }
        other => panic!("expected KillAfterLog, got {other:?}"),
    }
}

#[test]
fn exhaustive_match_compiles() {
    // This test exists to verify that all StepAction variants are handled.
    // If a new variant is added, the match below will fail to compile.
    let action = StepAction::Sleep(0);
    match &action {
        StepAction::Run(_) => {}
        StepAction::Conary(_) => {}
        StepAction::FileExists(_) => {}
        StepAction::FileNotExists(_) => {}
        StepAction::FileExecutable(_) => {}
        StepAction::DirExists(_) => {}
        StepAction::FileChecksum { .. } => {}
        StepAction::Sleep(_) => {}
        StepAction::KillAfterLog(_) => {}
        StepAction::QemuBoot(_) => {}
    }
}

#[test]
fn build_kill_command_plain() {
    let cmd = build_kill_after_log_command("/usr/bin/conary", "ccs install foo.ccs");
    assert!(cmd.contains("exec /usr/bin/conary ccs install foo.ccs"));
    assert!(cmd.contains("__CONARY_TEST_PID__"));
}

#[test]
fn build_kill_command_with_env() {
    let cmd =
        build_kill_after_log_command("/usr/bin/conary", "env HOLD_MS=1500 ccs install foo.ccs");
    assert!(cmd.contains("exec env HOLD_MS=1500 /usr/bin/conary ccs install foo.ccs"));
}

#[test]
fn step_result_from_exec_no_failure() {
    let exec = ExecResult {
        exit_code: 0,
        stdout: "ok".to_string(),
        stderr: String::new(),
    };
    let result = StepResult::from_exec(&exec, Duration::from_millis(42));
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "ok");
    assert!(result.failure.is_none());
}

#[test]
fn step_result_failed_carries_message() {
    let exec = ExecResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: "err".to_string(),
    };
    let result = StepResult::failed(&exec, Duration::from_millis(10), "boom".to_string());
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.failure, Some("boom".to_string()));
}

// ---- from_step tests for FileNotExists, FileExecutable, DirExists ----

#[test]
fn from_step_file_not_exists_produces_action() {
    let step = TestStep {
        file_not_exists: Some("/tmp/${NAME}".to_string()),
        ..TestStep::default()
    };
    let mut vars = HashMap::new();
    vars.insert("NAME".to_string(), "gone.txt".to_string());

    let action = StepAction::from_step(&step, &vars).unwrap();
    match action {
        StepAction::FileNotExists(path) => {
            assert_eq!(path, PathBuf::from("/tmp/gone.txt"));
        }
        other => panic!("expected FileNotExists, got {other:?}"),
    }
}

#[test]
fn from_step_file_executable_produces_action() {
    let step = TestStep {
        file_executable: Some("/usr/bin/${BIN}".to_string()),
        ..TestStep::default()
    };
    let mut vars = HashMap::new();
    vars.insert("BIN".to_string(), "conary".to_string());

    let action = StepAction::from_step(&step, &vars).unwrap();
    match action {
        StepAction::FileExecutable(path) => {
            assert_eq!(path, PathBuf::from("/usr/bin/conary"));
        }
        other => panic!("expected FileExecutable, got {other:?}"),
    }
}

#[test]
fn from_step_dir_exists_produces_action() {
    let step = TestStep {
        dir_exists: Some("/var/${DIR}".to_string()),
        ..TestStep::default()
    };
    let mut vars = HashMap::new();
    vars.insert("DIR".to_string(), "lib".to_string());

    let action = StepAction::from_step(&step, &vars).unwrap();
    match action {
        StepAction::DirExists(path) => {
            assert_eq!(path, PathBuf::from("/var/lib"));
        }
        other => panic!("expected DirExists, got {other:?}"),
    }
}

// ---- execute_step unit tests ----

use crate::container::mock::MockBackend;

fn test_ctx() -> ExecutionContext<'static> {
    ExecutionContext {
        conary_bin: "/usr/bin/conary",
        db_path: "/var/lib/conary/db",
    }
}

#[tokio::test]
async fn execute_step_run() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: "hello world\n".to_string(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::Run("echo hello world".to_string());
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "hello world\n");
    assert!(result.failure.is_none());
    let calls = mock.exec_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], vec!["sh", "-c", "echo hello world"]);
}

#[tokio::test]
async fn execute_step_conary() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: "installed tree\n".to_string(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::Conary("install tree".to_string());
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "installed tree\n");
    assert!(result.failure.is_none());
    let calls = mock.exec_calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0][2].contains("/usr/bin/conary install tree --db-path"));
}

#[tokio::test]
async fn execute_step_file_exists_success() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::FileExists(PathBuf::from("/usr/bin/conary"));
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.failure.is_none());
    let calls = mock.exec_calls();
    assert_eq!(calls[0], vec!["test", "-e", "/usr/bin/conary"]);
}

#[tokio::test]
async fn execute_step_file_exists_failure() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::FileExists(PathBuf::from("/missing/file"));
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 1);
    assert!(result.failure.is_some());
    assert!(
        result
            .failure
            .as_ref()
            .unwrap()
            .contains("file does not exist")
    );
}

#[tokio::test]
async fn execute_step_file_not_exists_success() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::FileNotExists(PathBuf::from("/tmp/gone.txt"));
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.failure.is_none());
    let calls = mock.exec_calls();
    assert_eq!(calls[0], vec!["test", "!", "-e", "/tmp/gone.txt"]);
}

#[tokio::test]
async fn execute_step_file_not_exists_failure() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::FileNotExists(PathBuf::from("/tmp/exists.txt"));
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 1);
    assert!(
        result
            .failure
            .as_ref()
            .unwrap()
            .contains("file unexpectedly exists")
    );
}

#[tokio::test]
async fn execute_step_file_executable_success() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::FileExecutable(PathBuf::from("/usr/bin/conary"));
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.failure.is_none());
    let calls = mock.exec_calls();
    assert_eq!(calls[0], vec!["test", "-x", "/usr/bin/conary"]);
}

#[tokio::test]
async fn execute_step_file_executable_failure() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::FileExecutable(PathBuf::from("/tmp/script.sh"));
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 1);
    assert!(
        result
            .failure
            .as_ref()
            .unwrap()
            .contains("file is not executable")
    );
}

#[tokio::test]
async fn execute_step_dir_exists_success() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::DirExists(PathBuf::from("/var/lib"));
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.failure.is_none());
    let calls = mock.exec_calls();
    assert_eq!(calls[0], vec!["test", "-d", "/var/lib"]);
}

#[tokio::test]
async fn execute_step_dir_exists_failure() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::DirExists(PathBuf::from("/nonexistent"));
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 1);
    assert!(
        result
            .failure
            .as_ref()
            .unwrap()
            .contains("directory does not exist")
    );
}

#[tokio::test]
async fn execute_step_file_checksum_match() {
    let hash = "abc123def456";
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: format!("{hash}  /tmp/file.txt\n"),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::FileChecksum {
        path: PathBuf::from("/tmp/file.txt"),
        sha256: hash.to_string(),
    };
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.failure.is_none());
}

#[tokio::test]
async fn execute_step_file_checksum_mismatch() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 0,
        stdout: "wronghash  /tmp/file.txt\n".to_string(),
        stderr: String::new(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::FileChecksum {
        path: PathBuf::from("/tmp/file.txt"),
        sha256: "expectedhash".to_string(),
    };
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert!(result.failure.is_some());
    assert!(
        result
            .failure
            .as_ref()
            .unwrap()
            .contains("checksum mismatch")
    );
}

#[tokio::test]
async fn execute_step_file_checksum_sha256sum_fails() {
    let mock = MockBackend::new(vec![ExecResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: "No such file\n".to_string(),
    }]);
    let ctx = test_ctx();
    let action = StepAction::FileChecksum {
        path: PathBuf::from("/tmp/missing.txt"),
        sha256: "abc".to_string(),
    };
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert!(result.failure.is_some());
    assert!(
        result
            .failure
            .as_ref()
            .unwrap()
            .contains("sha256sum failed")
    );
}

#[tokio::test]
async fn execute_step_sleep() {
    let mock = MockBackend::new(vec![]);
    let ctx = test_ctx();
    let action = StepAction::Sleep(0);
    let result = execute_step(
        &action,
        &mock,
        &"ctr-1".to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.failure.is_none());
    assert!(result.stdout.is_empty());
    // No exec calls should have been made.
    assert!(mock.exec_calls().is_empty());
}
