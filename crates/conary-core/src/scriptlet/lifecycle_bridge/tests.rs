// conary-core/src/scriptlet/lifecycle_bridge/tests.rs

use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Clone)]
struct RecordingHandler {
    requests: Arc<Mutex<Vec<LifecycleBridgeRequest>>>,
    response: LifecycleBridgeResponse,
}

impl LifecycleBridgeHandler for RecordingHandler {
    fn handle(
        &self,
        request: &LifecycleBridgeRequest,
    ) -> std::result::Result<LifecycleBridgeResponse, LifecycleBridgeHandlerError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(self.response.clone())
    }
}

struct ArgumentEchoHandler {
    requests: Arc<Mutex<Vec<LifecycleBridgeRequest>>>,
}

impl LifecycleBridgeHandler for ArgumentEchoHandler {
    fn handle(
        &self,
        request: &LifecycleBridgeRequest,
    ) -> std::result::Result<LifecycleBridgeResponse, LifecycleBridgeHandlerError> {
        self.requests.lock().unwrap().push(request.clone());
        let mut stdout = request.arguments().first().cloned().unwrap_or_default();
        stdout.push(b'\n');
        Ok(LifecycleBridgeResponse::new(stdout, Vec::new(), 0))
    }
}

#[test]
fn protocol_preserves_exact_request_and_response_bytes() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let handler = RecordingHandler {
        requests: Arc::clone(&requests),
        response: LifecycleBridgeResponse::new(
            vec![0, b'\n', 0xff],
            vec![b'e', b'r', b'r', b'\n'],
            23,
        ),
    };
    let (server, mut client) = UnixStream::pair().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        protocol::serve(
            server,
            Arc::new(handler),
            Duration::from_secs(1),
            &server_stop,
        )
    });

    client
        .write_all(b"CONARY-LIFECYCLE-BRIDGE 1 2\n3\nx\n\xff3\na\nb1\n\x80")
        .unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).unwrap();
    assert_eq!(
        response,
        b"CONARY-LIFECYCLE-BRIDGE 1 23 15 20\n\\0000\\0012\\0377\n\\0145\\0162\\0162\\0012\n"
    );
    assert_eq!(server.join().unwrap(), Ok(()));
    assert_eq!(
        *requests.lock().unwrap(),
        [LifecycleBridgeRequest {
            command: b"x\n\xff".to_vec(),
            arguments: vec![b"a\nb".to_vec(), vec![0x80]],
        }]
    );
}

#[test]
fn malformed_header_is_a_transport_failure() {
    let error = serve_invalid_request(b"WRONG 1 0\n", Duration::from_secs(1));
    assert!(error.contains("unsupported lifecycle bridge request header"));
}

#[test]
fn eof_inside_a_value_is_a_transport_failure() {
    let error = serve_invalid_request(
        b"CONARY-LIFECYCLE-BRIDGE 1 0\n4\nab",
        Duration::from_secs(1),
    );
    assert!(error.contains("request body failed"));
    assert!(error.contains("failed to fill whole buffer"));
}

#[test]
fn timeout_inside_a_value_is_a_transport_failure() {
    let handler = RecordingHandler {
        requests: Arc::new(Mutex::new(Vec::new())),
        response: LifecycleBridgeResponse::new(Vec::new(), Vec::new(), 0),
    };
    let (server, mut client) = UnixStream::pair().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        protocol::serve(
            server,
            Arc::new(handler),
            Duration::from_millis(40),
            &server_stop,
        )
    });
    client
        .write_all(b"CONARY-LIFECYCLE-BRIDGE 1 0\n4\nab")
        .unwrap();
    let error = server.join().unwrap().unwrap_err();
    assert!(error.contains("request body failed"));
    assert!(error.contains("timed out") || error.contains("temporarily unavailable"));
}

#[test]
fn inherited_descriptors_return_exact_binary_process_result() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let handler = RecordingHandler {
        requests: Arc::clone(&requests),
        response: LifecycleBridgeResponse::new(
            vec![b'o', 0, b'k', b'\n', 0xff],
            vec![b'e', 0, b'r', b'r'],
            23,
        ),
    };
    let root = tempfile::tempdir().unwrap();
    let mut staged = TargetRootScript::stage(root.path(), b"").unwrap();
    let config = LifecycleBridgeConfig::new(
        vec![LifecycleBridgeEndpoint::new(
            "/usr/bin/conary-test-helper",
            executable_bridge_shim(),
        )],
        handler,
    );
    let mut session = LifecycleBridgeSession::stage(&mut staged, root.path(), &config).unwrap();
    let proxy = session.mounts()[0].source.clone();
    let mut command = Command::new(&proxy);
    command
        .args(["first\nargument", "second argument"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    session.configure_command(&mut command).unwrap();
    let child = command.spawn().unwrap();
    session.child_spawned();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(output.stdout, b"o\0k\n\xff");
    assert_eq!(output.stderr, b"e\0rr");
    session.finish().unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].arguments(),
        [b"first\nargument".to_vec(), b"second argument".to_vec()]
    );
}

#[test]
fn background_helpers_are_serialized_without_crossed_responses() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let handler = ArgumentEchoHandler {
        requests: Arc::clone(&requests),
    };
    let root = tempfile::tempdir().unwrap();
    let mut staged = TargetRootScript::stage(root.path(), b"").unwrap();
    let config = LifecycleBridgeConfig::new(
        vec![LifecycleBridgeEndpoint::new(
            "/usr/bin/conary-test-helper",
            executable_bridge_shim(),
        )],
        handler,
    );
    let mut session = LifecycleBridgeSession::stage(&mut staged, root.path(), &config).unwrap();
    let proxy = session.mounts()[0].source.clone();
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(
            "for value in alpha beta gamma delta epsilon zeta eta theta; do \
             \"$PROXY\" \"$value\" & done; wait",
        )
        .env("PROXY", &proxy)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    session.configure_command(&mut command).unwrap();
    let child = command.spawn().unwrap();
    session.child_spawned();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    session.finish().unwrap();

    let mut lines = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    lines.sort();
    assert_eq!(
        lines,
        [
            "alpha", "beta", "delta", "epsilon", "eta", "gamma", "theta", "zeta"
        ]
    );
    assert_eq!(requests.lock().unwrap().len(), 8);
}

#[test]
fn transient_mount_targets_are_removed_and_existing_targets_are_untouched() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("usr/bin")).unwrap();
    let existing = root.path().join("usr/bin/existing");
    std::fs::write(&existing, b"target-owned").unwrap();
    let mut staged = TargetRootScript::stage(root.path(), b"").unwrap();
    let handler = RecordingHandler {
        requests: Arc::new(Mutex::new(Vec::new())),
        response: LifecycleBridgeResponse::new(Vec::new(), Vec::new(), 0),
    };
    let config = LifecycleBridgeConfig::new(
        vec![
            LifecycleBridgeEndpoint::new("/usr/bin/existing", executable_bridge_shim()),
            LifecycleBridgeEndpoint::new("/usr/share/debconf/confmodule", executable_bridge_shim()),
        ],
        handler,
    );
    let session = LifecycleBridgeSession::stage(&mut staged, root.path(), &config).unwrap();
    assert!(root.path().join("usr/share/debconf/confmodule").is_file());
    session.finish().unwrap();
    assert_eq!(std::fs::read(&existing).unwrap(), b"target-owned");
    assert!(!root.path().join("usr/share/debconf/confmodule").exists());
    assert!(!root.path().join("usr/share").exists());
}

#[test]
fn target_staging_rejects_a_symlink_escape() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("usr")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("usr/bin")).unwrap();
    let mut staged = TargetRootScript::stage(root.path(), b"").unwrap();
    let handler = RecordingHandler {
        requests: Arc::new(Mutex::new(Vec::new())),
        response: LifecycleBridgeResponse::new(Vec::new(), Vec::new(), 0),
    };
    let config = LifecycleBridgeConfig::new(
        vec![LifecycleBridgeEndpoint::new(
            "/usr/bin/conary-test-helper",
            executable_bridge_shim(),
        )],
        handler,
    );
    let error = LifecycleBridgeSession::stage(&mut staged, root.path(), &config)
        .err()
        .expect("an endpoint symlink escape must fail")
        .to_string();
    assert!(error.contains("escapes selected root"), "{error}");
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn executable_shim_uses_the_private_descriptor_contract() {
    let shim = std::str::from_utf8(executable_bridge_shim()).unwrap();
    assert!(shim.starts_with("#!/bin/sh\n"));
    assert!(shim.contains("<&8"));
    assert!(shim.contains(">&7"));
    assert!(shim.contains(">&9"));
}

#[test]
fn selected_root_bind_mount_never_executes_the_target_helper() {
    if !nix::unistd::geteuid().is_root() || !mount_namespace_available() {
        return;
    }

    let root = tempfile::tempdir().unwrap();
    install_host_shell(root.path());
    let target = root.path().join("usr/bin/conary-test-helper");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(
        &target,
        b"#!/bin/sh\ntouch /target-helper-executed\nexit 99\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&target).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o700);
    std::fs::set_permissions(&target, permissions).unwrap();

    let requests = Arc::new(Mutex::new(Vec::new()));
    let handler = RecordingHandler {
        requests: Arc::clone(&requests),
        response: LifecycleBridgeResponse::new(b"bridged\n".to_vec(), Vec::new(), 0),
    };
    let executor = crate::scriptlet::ScriptletExecutor::new(
        root.path(),
        "bridge-fixture",
        "1",
        crate::scriptlet::PackageFormat::Deb,
    )
    .with_lifecycle_bridge(LifecycleBridgeConfig::new(
        vec![LifecycleBridgeEndpoint::new(
            "/usr/bin/conary-test-helper",
            executable_bridge_shim(),
        )],
        handler,
    ));
    let execution = executor.execute_in_target(
        super::super::process::ScriptletProcess {
            phase: "post-install",
            interpreter: "/bin/sh",
            interpreter_args: &[],
            args: &[],
            env: &[],
            stdin: &[],
        },
        b"value=$(/usr/bin/conary-test-helper 'one' 'two words')\n\
          test \"$value\" = bridged\n",
    );
    execution.unwrap();

    assert!(!root.path().join("target-helper-executed").exists());
    assert!(
        std::fs::read(&target)
            .unwrap()
            .starts_with(b"#!/bin/sh\ntouch")
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].command(), b"/usr/bin/conary-test-helper");
    assert_eq!(
        requests[0].arguments(),
        [b"one".to_vec(), b"two words".to_vec()]
    );
}

fn serve_invalid_request(bytes: &[u8], timeout: Duration) -> String {
    let handler = RecordingHandler {
        requests: Arc::new(Mutex::new(Vec::new())),
        response: LifecycleBridgeResponse::new(Vec::new(), Vec::new(), 0),
    };
    let (server, mut client) = UnixStream::pair().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        protocol::serve(server, Arc::new(handler), timeout, &server_stop)
    });
    client.write_all(bytes).unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();
    server.join().unwrap().unwrap_err()
}

fn install_host_shell(root: &Path) {
    copy_into_root(Path::new("/bin/sh"), root, Path::new("/bin/sh"));
    let output = Command::new("ldd").arg("/bin/sh").output().unwrap();
    assert!(output.status.success());
    let ldd_output = String::from_utf8(output.stdout).unwrap();
    let dependencies = ldd_output
        .split_whitespace()
        .filter(|field| field.starts_with('/'))
        .map(|field| field.trim_end_matches(':').to_string())
        .collect::<std::collections::BTreeSet<_>>();
    for dependency in dependencies {
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
    // Safety: the child-only pre-exec probe invokes the same unshare syscall as
    // the lifecycle boundary and does not alter the test process namespace.
    unsafe {
        command.pre_exec(|| {
            nix::sched::unshare(nix::sched::CloneFlags::CLONE_NEWNS).map_err(std::io::Error::from)
        });
    }
    command.status().is_ok()
}
