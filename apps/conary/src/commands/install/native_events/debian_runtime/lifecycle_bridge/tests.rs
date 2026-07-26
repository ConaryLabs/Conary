// apps/conary/src/commands/install/native_events/debian_runtime/lifecycle_bridge/tests.rs

use super::*;
use conary_core::packages::deb::debconf::{DebconfResultCode, DebconfTemplateLoadErrorKind};
use conary_core::scriptlet::{
    ExecutionMode, NativeInvocationRuntime, NativeLifecycleExecution, PackageFormat, SandboxMode,
    ScriptletExecutor, ScriptletOutcome,
};
use std::io::Write as _;
use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};

const PACKAGE_TEMPLATES: &[u8] = b"\
Template: demo/answer\n\
Type: string\n\
Default: initial\n\
Description: Exact answer\n";

fn bridge(root: &Path, owner: &str, templates: &[u8]) -> DebianLifecycleBridge {
    let records = parse(templates).unwrap().records;
    DebianLifecycleBridge::new(root, owner, DebconfState::default(), &records).unwrap()
}

fn confmodule(bridge: &DebianLifecycleBridge, argv: &[&[u8]]) -> LifecycleBridgeResponse {
    bridge
        .handler
        .handle_confmodule(&argv.iter().map(|arg| arg.to_vec()).collect::<Vec<_>>())
        .unwrap()
}

#[test]
fn every_debian_endpoint_is_conary_owned() {
    let root = tempfile::tempdir().unwrap();
    let config = bridge(root.path(), "demo", PACKAGE_TEMPLATES).config();
    let targets = config
        .endpoints()
        .iter()
        .map(|endpoint| endpoint.target().to_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        [
            "/usr/bin/dpkg-maintscript-helper",
            "/usr/bin/dpkg-query",
            "/usr/bin/dpkg",
            "/usr/bin/update-alternatives",
            "/usr/bin/deb-systemd-helper",
            "/usr/bin/deb-systemd-invoke",
            "/usr/sbin/invoke-rc.d",
            "/usr/sbin/update-rc.d",
            "/usr/sbin/service",
            "/usr/share/debconf/confmodule",
        ]
    );
    assert!(confmodule_shim().starts_with(b"#!/bin/sh\n"));
}

#[test]
fn exact_argv_and_caller_result_mutate_one_event_state() {
    let root = tempfile::tempdir().unwrap();
    let bridge = bridge(root.path(), "demo", PACKAGE_TEMPLATES);

    let set = confmodule(
        &bridge,
        &[b"SET", b"demo/answer", b"value  with\twhitespace"],
    );
    assert_eq!(set.exit_code(), DebconfResultCode::Success.as_u16() as u8);
    assert_eq!(set.stdout(), b"value set");
    let get = confmodule(&bridge, &[b"GET", b"demo/answer"]);
    assert_eq!(get.exit_code(), 0);
    assert_eq!(get.stdout(), b"value  with\twhitespace");
    assert_eq!(
        bridge.state().unwrap().questions["demo/answer"].value,
        Some(b"value  with\twhitespace".to_vec())
    );
}

#[test]
fn stop_has_a_private_success_frame_and_finishes_session() {
    let root = tempfile::tempdir().unwrap();
    let bridge = bridge(root.path(), "demo", PACKAGE_TEMPLATES);

    let stopped = confmodule(&bridge, &[b"STOP"]);
    assert_eq!(stopped.exit_code(), 0);
    assert!(stopped.stdout().is_empty());
    assert!(stopped.stderr().is_empty());
    let after = confmodule(&bridge, &[b"GET", b"demo/answer"]);
    assert_eq!(
        after.exit_code(),
        DebconfResultCode::InternalError.as_u16() as u8
    );
    assert_eq!(after.stdout(), b"confmodule session has stopped");
}

#[test]
fn package_preload_preserves_shared_owners_and_existing_answer() {
    let root = tempfile::tempdir().unwrap();
    let records = parse(PACKAGE_TEMPLATES).unwrap().records;
    let first = DebianLifecycleBridge::new(root.path(), "first", DebconfState::default(), &records)
        .unwrap();
    let set = confmodule(&first, &[b"SET", b"demo/answer", b"retained"]);
    assert_eq!(set.exit_code(), 0);

    let second =
        DebianLifecycleBridge::new(root.path(), "second", first.state().unwrap(), &records)
            .unwrap();
    let state = second.state().unwrap();
    assert_eq!(
        state.questions["demo/answer"].owners,
        ["first".to_string(), "second".to_string()]
            .into_iter()
            .collect()
    );
    assert_eq!(
        state.questions["demo/answer"].value,
        Some(b"retained".to_vec())
    );
}

#[test]
fn normalized_store_settles_shared_state_after_success_and_failure() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    let root = tempfile::tempdir().unwrap();
    let records = parse(PACKAGE_TEMPLATES).unwrap().records;

    let first = DebianLifecycleBridge::new(root.path(), "first", DebconfState::default(), &records)
        .unwrap();
    assert_eq!(
        confmodule(&first, &[b"SET", b"demo/answer", b"successful"]).exit_code(),
        0
    );
    let success =
        crate::commands::install::native_events::execution::settle_debian_lifecycle_event(
            &conn,
            Some(&first),
            Ok(()),
        )
        .unwrap();
    assert!(success.is_ok());

    let persisted = conary_core::db::models::DebianDebconfState::load(&conn).unwrap();
    let second = DebianLifecycleBridge::new(root.path(), "second", persisted, &records).unwrap();
    assert_eq!(
        confmodule(&second, &[b"SET", b"demo/answer", b"failed-but-stored"]).exit_code(),
        0
    );
    let failure =
        crate::commands::install::native_events::execution::settle_debian_lifecycle_event(
            &conn,
            Some(&second),
            Err(anyhow::anyhow!("maintainer script failed")),
        )
        .unwrap()
        .unwrap_err();
    assert_eq!(failure.to_string(), "maintainer script failed");

    let persisted = conary_core::db::models::DebianDebconfState::load(&conn).unwrap();
    assert_eq!(
        persisted.questions["demo/answer"].owners,
        ["first".to_string(), "second".to_string()]
            .into_iter()
            .collect()
    );
    assert_eq!(
        persisted.questions["demo/answer"].value,
        Some(b"failed-but-stored".to_vec())
    );
}

#[test]
fn selected_root_loader_accepts_regular_in_root_templates() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("tmp")).unwrap();
    std::fs::write(root.path().join("tmp/templates"), PACKAGE_TEMPLATES).unwrap();
    symlink("/tmp/templates", root.path().join("templates-link")).unwrap();
    let mut loader = SelectedRootTemplateLoader::new(root.path()).unwrap();

    let records = loader.load(b"/tmp/templates").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].template, "demo/answer");
    assert_eq!(
        loader.load(b"/templates-link").unwrap()[0].template,
        "demo/answer"
    );
}

#[test]
fn x_loadtemplatefile_uses_the_selected_root_loader_and_exact_owner() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("additional"),
        b"Template: shared/additional\nType: boolean\nDescription: Additional\n",
    )
    .unwrap();
    let bridge = bridge(root.path(), "demo", PACKAGE_TEMPLATES);

    let response = confmodule(
        &bridge,
        &[b"X_LOADTEMPLATEFILE", b"/additional", b"shared-owner"],
    );
    assert_eq!(response.exit_code(), 0);
    assert_eq!(
        bridge.state().unwrap().questions["shared/additional"].owners,
        ["shared-owner".to_string()].into_iter().collect()
    );
}

#[test]
fn selected_root_loader_rejects_symlink_escape_and_non_regular_file() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("templates"), PACKAGE_TEMPLATES).unwrap();
    symlink(
        outside.path().join("templates"),
        root.path().join("escaped"),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("directory")).unwrap();
    let mut loader = SelectedRootTemplateLoader::new(root.path()).unwrap();

    for path in [b"/escaped".as_slice(), b"/directory".as_slice()] {
        let error = loader.load(path).unwrap_err();
        assert_eq!(error.kind, DebconfTemplateLoadErrorKind::Open);
    }
}

#[test]
fn selected_root_loader_distinguishes_parse_failure() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("templates"), b"not a templates file\n").unwrap();
    let mut loader = SelectedRootTemplateLoader::new(root.path()).unwrap();

    let error = loader.load(b"/templates").unwrap_err();
    assert_eq!(error.kind, DebconfTemplateLoadErrorKind::Parse);
}

#[test]
fn selected_root_loader_rejects_the_sixteen_mib_plus_one_boundary() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("templates");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_DEBCONF_TEMPLATES_SIZE + 1).unwrap();
    let mut loader = SelectedRootTemplateLoader::new(root.path()).unwrap();

    let error = loader.load(b"/templates").unwrap_err();
    assert_eq!(error.kind, DebconfTemplateLoadErrorKind::Open);
    assert!(String::from_utf8_lossy(&error.message).contains("maximum"));
}

#[test]
fn confmodule_never_executes_a_target_frontend() {
    let shim = String::from_utf8(confmodule_shim()).unwrap();
    assert!(!shim.contains("/usr/share/debconf/frontend"));
    assert!(!shim.contains("/usr/lib/cdebconf"));
    assert!(shim.contains("DEBIAN_HAS_FRONTEND=1"));
    assert!(shim.contains("exec 3>&1"));
}

#[test]
fn confmodule_is_valid_posix_shell_syntax() {
    let mut child = Command::new("/bin/sh")
        .args(["-n", "-s"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&confmodule_shim())
        .unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn config_and_postinst_source_the_real_bridge_and_share_persisted_state() {
    if !nix::unistd::geteuid().is_root() || !mount_namespace_available() {
        return;
    }

    let root = tempfile::tempdir().unwrap();
    install_host_shell(root.path());
    let records = parse(PACKAGE_TEMPLATES).unwrap().records;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();

    let config =
        DebianLifecycleBridge::new(root.path(), "demo", DebconfState::default(), &records).unwrap();
    let config_outcome = run_shell_event(
        root.path(),
        &config,
        "deb:config",
        "config",
        "\
. /usr/share/debconf/confmodule\n\
db_set demo/answer 'configured  exactly'\n\
test \"$RET\" = 'value set'\n\
db_stop\n",
    );
    assert!(
        matches!(&config_outcome, ScriptletOutcome::Success { .. }),
        "{config_outcome:?}"
    );
    crate::commands::install::native_events::execution::settle_debian_lifecycle_event(
        &conn,
        Some(&config),
        config_outcome.into_result().map_err(anyhow::Error::from),
    )
    .unwrap()
    .unwrap();

    let state = conary_core::db::models::DebianDebconfState::load(&conn).unwrap();
    let postinst = DebianLifecycleBridge::new(root.path(), "demo", state, &records).unwrap();
    let postinst_outcome = run_shell_event(
        root.path(),
        &postinst,
        "deb:postinst",
        "post-install",
        "\
. /usr/share/debconf/confmodule\n\
db_get demo/answer\n\
test \"$RET\" = 'configured  exactly'\n\
db_stop\n",
    );
    assert!(
        matches!(postinst_outcome, ScriptletOutcome::Success { .. }),
        "{postinst_outcome:?}"
    );
}

fn run_shell_event(
    root: &Path,
    bridge: &DebianLifecycleBridge,
    entry_id: &str,
    phase: &str,
    body: &str,
) -> ScriptletOutcome {
    let executor = ScriptletExecutor::new(root, "demo", "1", PackageFormat::Deb)
        .with_sandbox_mode(SandboxMode::Always)
        .with_lifecycle_bridge(bridge.config());
    let native_args = Vec::new();
    let native_environment = Vec::new();
    let resolved_args = Vec::new();
    let execution = NativeLifecycleExecution {
        entry_id,
        phase,
        interpreter: "/bin/sh",
        interpreter_args: &[],
        body: body.to_string(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body_encoding: None,
        shell_entrypoint: None,
        native_args: &native_args,
        native_environment: &native_environment,
        stdin_contract: Some("none"),
        chroot_contract: Some("package-manager-default"),
        timeout_ms: 10_000,
    };
    let mode = ExecutionMode::Install;
    let runtime = NativeInvocationRuntime {
        mode: &mode,
        old_version: None,
        new_version: Some("1"),
        package_instance_count: Some(1),
        resolved_native_args: Some(&resolved_args),
        stdin: &[],
    };
    executor.execute_native_lifecycle_entry_with_outcome(&execution, &runtime)
}

fn install_host_shell(root: &Path) {
    copy_into_root(Path::new("/bin/sh"), root, Path::new("/bin/sh"));
    let output = Command::new("ldd").arg("/bin/sh").output().unwrap();
    assert!(output.status.success());
    for dependency in String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .filter(|field| field.starts_with('/'))
        .map(|field| field.trim_end_matches(':').to_string())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let path = Path::new(&dependency);
        copy_into_root(path, root, path);
    }
}

fn copy_into_root(source: &Path, root: &Path, destination: &Path) {
    let source = std::fs::canonicalize(source).unwrap();
    let destination = root.join(destination.strip_prefix("/").unwrap());
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::copy(source, destination).unwrap();
}

fn mount_namespace_available() -> bool {
    let mut command = Command::new("/bin/true");
    // Safety: this child-only probe uses the lifecycle executor's mount
    // namespace syscall and cannot alter the test process namespace.
    unsafe {
        command.pre_exec(|| {
            nix::sched::unshare(nix::sched::CloneFlags::CLONE_NEWNS).map_err(std::io::Error::from)
        });
    }
    command.status().is_ok()
}
