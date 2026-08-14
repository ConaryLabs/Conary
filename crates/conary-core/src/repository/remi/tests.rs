// conary-core/src/repository/remi/tests.rs

use super::*;

#[test]
fn test_base_url_normalization() {
    // With trailing slash
    let client = RemiClient::new("http://localhost:8080/").unwrap();
    assert_eq!(client.core.base_url, "http://localhost:8080");

    // Without trailing slash
    let client = RemiClient::new("http://localhost:8080").unwrap();
    assert_eq!(client.core.base_url, "http://localhost:8080");
}

#[test]
fn test_conversion_accepted_parsing() {
    let json = r#"{"status":"queued","job_id":"123","poll_url":"/v1/jobs/123","eta_seconds":30}"#;
    let accepted: ConversionAccepted = serde_json::from_str(json).unwrap();
    assert_eq!(accepted.status, "queued");
    assert_eq!(accepted.job_id, "123");
    assert_eq!(accepted.eta_seconds, Some(30));
}

#[test]
fn test_job_status_parsing() {
    let json = r#"{"job_id":"1","status":"ready","distro":"arch","package":"gzip","version":null,"progress":null,"error":null,"manifest":null}"#;
    let status: JobStatus = serde_json::from_str(json).unwrap();
    assert_eq!(status.status, ConversionJobState::Ready);
    assert_eq!(status.package, "gzip");
}

#[test]
fn every_remi_job_state_has_one_shared_poll_decision() {
    let cases = [
        ("pending", protocol::JobPollDecision::Wait),
        ("converting", protocol::JobPollDecision::Wait),
        ("ready", protocol::JobPollDecision::Ready),
        (
            "failed",
            protocol::JobPollDecision::Failed("conversion failed"),
        ),
    ];

    for (wire_state, expected) in cases {
        let json = format!(
            r#"{{"job_id":"1","status":"{wire_state}","distro":"fedora","package":"kernel-core","version":null,"architecture":"x86_64","progress":null,"error":"conversion failed","manifest":null}}"#
        );
        let status: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status.poll_decision(), expected);
    }

    let unsupported = r#"{"job_id":"1","status":"blocked","distro":"fedora","package":"kernel-core","version":null,"architecture":"x86_64","progress":null,"error":null,"manifest":null}"#;
    let error = serde_json::from_str::<JobStatus>(unsupported).unwrap_err();
    assert!(
        error.to_string().contains("unknown variant `blocked`"),
        "{error}"
    );
}

#[test]
fn unexpected_http_error_remains_a_protocol_error() {
    let core = RemiClientCore::new("https://remi.example.test").unwrap();
    let body = r#"{"status":"unsupported-server-contract"}"#;

    let err = core.map_http_error(403, body.to_string(), "kernel-core", "fedora");
    let message = err.to_string();

    assert!(message.contains("HTTP 403"));
    assert!(message.contains("unsupported-server-contract"));
}

#[test]
fn http_client_builder_error_mentions_minimal_chroot_runtime() {
    let message = http_client_builder_error_message("builder error");

    assert!(message.contains("Failed to create HTTP client: builder error"));
    assert!(message.contains("minimal chroot"));
    assert!(message.contains("/etc/resolv.conf"));
    assert!(message.contains("/etc/ssl/certs"));
    assert!(message.contains("ca-certificates"));
}

#[test]
fn test_build_package_url_without_version() {
    let core = RemiClientCore::new("http://remi:8080").unwrap();
    let url = core.package_url("arch", "nginx", None, None, None);
    assert_eq!(url, "http://remi:8080/v1/arch/packages/nginx");
}

#[test]
fn test_build_package_url_with_version() {
    let core = RemiClientCore::new("http://remi:8080").unwrap();
    let url = core.package_url("arch", "nginx", Some("1.24.0"), None, None);
    assert_eq!(
        url,
        "http://remi:8080/v1/arch/packages/nginx?version=1.24.0"
    );
}

#[test]
fn test_build_package_url_with_version_release_and_architecture() {
    let core = RemiClientCore::new("https://remi.example.test").unwrap();
    let url = core.package_url("fedora", "hello", Some("1.0.0"), Some("1"), Some("noarch"));
    assert_eq!(
        url,
        "https://remi.example.test/v1/fedora/packages/hello?version=1.0.0&release=1&arch=noarch"
    );
}

#[test]
fn test_build_download_url_with_version() {
    let core = RemiClientCore::new("http://remi:8080").unwrap();
    let url = core.download_url("arch", "nginx", Some("1.24.0"), None, None);
    assert_eq!(
        url,
        "http://remi:8080/v1/arch/packages/nginx/download?version=1.24.0"
    );
}

#[test]
fn test_build_download_url_with_release() {
    let core = RemiClientCore::new("https://remi.example.test").unwrap();
    let url = core.download_url("fedora", "hello", Some("1.0.0"), Some("1"), Some("noarch"));
    assert_eq!(
        url,
        "https://remi.example.test/v1/fedora/packages/hello/download?version=1.0.0&release=1&arch=noarch"
    );
}

#[test]
fn test_build_download_url_with_version_and_architecture() {
    let core = RemiClientCore::new("http://remi:8080").unwrap();
    let url = core.download_url(
        "fedora",
        "glib2",
        Some("2.86.0-2.fc44"),
        None,
        Some("x86_64"),
    );
    assert_eq!(
        url,
        "http://remi:8080/v1/fedora/packages/glib2/download?version=2.86.0-2.fc44&arch=x86_64"
    );
}

#[test]
fn test_filename_sanitization_fallback() {
    // Path traversal in filename should fall back to safe default
    let malicious = "../../etc/cron.d/evil";
    let safe = sanitize_filename(malicious).unwrap_or_else(|_| format!("{}.ccs", "nginx"));
    assert_eq!(safe, "nginx.ccs");
}

#[test]
fn test_filename_sanitization_normal() {
    // Normal filenames should pass through
    let normal = "nginx-1.24.0.ccs";
    let result = sanitize_filename(normal).unwrap();
    assert_eq!(result, "nginx-1.24.0.ccs");
}

#[test]
fn test_chunk_byte_limit_rejects_excessive_total() {
    let err = check_total_chunk_bytes(MAX_TOTAL_CHUNK_BYTES - 4, 8).unwrap_err();
    assert!(err.to_string().contains("chunk bytes"));
}

#[tokio::test]
async fn fetch_package_retries_when_conversion_queue_is_full() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let attempts = Arc::new(AtomicUsize::new(0));
    let server_attempts = attempts.clone();

    tokio::spawn(async move {
        for n in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            server_attempts.fetch_add(1, Ordering::SeqCst);

            if n == 0 {
                socket
                    .write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\n\
                          Content-Length: 21\r\n\
                          Connection: close\r\n\
                          \r\n\
                          Conversion queue full",
                    )
                    .await
                    .unwrap();
            } else {
                let mut response = b"HTTP/1.1 200 OK\r\n\
                                     Content-Disposition: attachment; filename=\"qemu-img.ccs\"\r\n\
                                     Content-Length: 2\r\n\
                                     Connection: close\r\n\
                                     \r\n"
                    .to_vec();
                response.extend_from_slice(&[0x1f, 0x8b]);
                socket.write_all(&response).await.unwrap();
            }
        }
    });

    let output = tempfile::tempdir().unwrap();
    let client = RemiClient::new(&base_url).unwrap();
    let path = client
        .fetch_package(
            "fedora",
            "qemu-img",
            Some("2:10.1.0-7.fc44"),
            None,
            output.path(),
        )
        .await
        .expect("queue-full response should be retried");

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read(path).unwrap(), [0x1f, 0x8b]);
}

#[tokio::test]
async fn fetch_package_requests_identity_encoding() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0_u8; 1024];
        loop {
            let read = socket.read(&mut buf).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buf[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let mut response = b"HTTP/1.1 200 OK\r\n\
                             Content-Disposition: attachment; filename=\"qemu-img.ccs\"\r\n\
                             Content-Length: 2\r\n\
                             Connection: close\r\n\
                             \r\n"
            .to_vec();
        response.extend_from_slice(&[0x1f, 0x8b]);
        socket.write_all(&response).await.unwrap();

        String::from_utf8(request).unwrap()
    });

    let output = tempfile::tempdir().unwrap();
    let client = RemiClient::new(&base_url).unwrap();
    client
        .fetch_package(
            "fedora",
            "qemu-img",
            Some("2:10.1.0-7.fc44"),
            None,
            output.path(),
        )
        .await
        .expect("download should succeed");

    let request = server.await.unwrap().to_ascii_lowercase();
    assert!(
        request.contains("accept-encoding: identity"),
        "package download request did not force identity encoding:\n{request}"
    );
}

/// Poll cycles the former fixed 300-second cutoff allowed at the 2-second poll
/// interval. An active job must survive more than this many cycles.
const POLLS_BEYOND_FORMER_FIVE_MINUTE_CUTOFF: usize = 152;

const CONTRACT_JOB_ID: &str = "55";
const CONTRACT_PACKAGE: &str = "kernel-modules-core";
const CONTRACT_FAILURE_REASON: &str = "converter rejected payload";

fn job_response(state: &str, error: Option<&str>, manifest: Option<&str>) -> String {
    let error = match error {
        Some(reason) => format!("\"{reason}\""),
        None => "null".to_string(),
    };
    let manifest = manifest.unwrap_or("null");
    format!(
        r#"{{"job_id":"{CONTRACT_JOB_ID}","status":"{state}","distro":"fedora","package":"{CONTRACT_PACKAGE}","version":"6.19.10-300.fc44","architecture":"x86_64","progress":null,"error":{error},"manifest":{manifest}}}"#
    )
}

fn ready_job_response() -> String {
    let manifest = format!(
        r#"{{"name":"{CONTRACT_PACKAGE}","version":"6.19.10-300.fc44","distro":"fedora","chunks":[],"total_size":0,"content_hash":"sha256:test"}}"#
    );
    job_response("ready", None, Some(&manifest))
}

/// Outcome the shared contract requires for a given state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedOutcome {
    ReadyManifest,
    ConversionFailed,
}

/// The poll responses that exercise `state`, and the outcome the client must
/// reach.
///
/// The match is exhaustive, so a new `ConversionJobState` cannot be added
/// without stating its contract here.
fn contract_case(state: ConversionJobState) -> (Vec<String>, ExpectedOutcome) {
    let outlive_former_cutoff = |wire_state: &str| {
        let mut responses =
            vec![job_response(wire_state, None, None); POLLS_BEYOND_FORMER_FIVE_MINUTE_CUTOFF];
        responses.push(ready_job_response());
        (responses, ExpectedOutcome::ReadyManifest)
    };

    match state {
        ConversionJobState::Pending => outlive_former_cutoff("pending"),
        ConversionJobState::Converting => outlive_former_cutoff("converting"),
        ConversionJobState::Ready => (vec![ready_job_response()], ExpectedOutcome::ReadyManifest),
        ConversionJobState::Failed => (
            vec![job_response("failed", Some(CONTRACT_FAILURE_REASON), None)],
            ExpectedOutcome::ConversionFailed,
        ),
    }
}

/// Serve one 202 acceptance followed by `poll_responses` job-status bodies.
async fn spawn_job_poll_server(
    poll_responses: Vec<String>,
) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let accepted = format!(
            r#"{{"status":"queued","job_id":"{CONTRACT_JOB_ID}","poll_url":"/v1/jobs/{CONTRACT_JOB_ID}","eta_seconds":null}}"#
        );
        write_json_response(&listener, "202 Accepted", &accepted).await;

        for body in poll_responses {
            write_json_response(&listener, "200 OK", &body).await;
        }
    });

    (base_url, server)
}

async fn remi_client_outcome(base_url: &str) -> Result<PackageManifest> {
    let mut client = RemiClient::new(base_url).unwrap();
    client.core.poll_interval = Duration::from_millis(1);
    client
        .get_package("fedora", CONTRACT_PACKAGE, None, Some("x86_64"))
        .await
}

fn assert_contract_outcome(
    result: Result<PackageManifest>,
    expected: ExpectedOutcome,
    state: ConversionJobState,
) {
    match expected {
        ExpectedOutcome::ReadyManifest => {
            let manifest = result.unwrap_or_else(|error| {
                panic!("Remi client failed on {state:?}: {error}");
            });
            assert_eq!(manifest.name, CONTRACT_PACKAGE);
        }
        ExpectedOutcome::ConversionFailed => {
            let message = result
                .err()
                .unwrap_or_else(|| panic!("Remi client accepted a failed {state:?} job"))
                .to_string();
            assert!(
                message.contains("Conversion failed"),
                "Remi client on {state:?}: {message}"
            );
            assert!(
                message.contains(CONTRACT_FAILURE_REASON),
                "Remi client dropped the server failure reason on {state:?}: {message}"
            );
        }
    }
}

/// Every state the shared protocol admits reaches its contracted outcome in the
/// Remi client, and no active state is failed on elapsed time.
#[tokio::test]
async fn the_remi_client_honors_the_shared_polling_contract() {
    for state in ConversionJobState::ALL {
        let (responses, expected) = contract_case(state);

        let (base_url, server) = spawn_job_poll_server(responses).await;
        assert_contract_outcome(remi_client_outcome(&base_url).await, expected, state);
        server.await.unwrap();
    }
}

/// A status outside the shared contract is a typed rejection, never silently
/// treated as an active job.
#[tokio::test]
async fn the_remi_client_rejects_a_status_outside_the_shared_contract() {
    let retired = vec![job_response("blocked", None, None)];

    let (base_url, server) = spawn_job_poll_server(retired).await;
    let message = remi_client_outcome(&base_url)
        .await
        .expect_err("Remi client accepted a retired job status")
        .to_string();
    assert!(message.contains("unknown variant `blocked`"), "{message}");
    server.await.unwrap();
}

async fn write_json_response(listener: &tokio::net::TcpListener, status: &str, body: &str) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = [0_u8; 1024];
    let _ = socket.read(&mut request).await.unwrap();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    socket.write_all(response.as_bytes()).await.unwrap();
}

async fn spawn_download_server(
    responses: Vec<Option<Vec<u8>>>,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let attempts = Arc::new(AtomicUsize::new(0));
    let server_attempts = attempts.clone();

    tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            server_attempts.fetch_add(1, Ordering::SeqCst);
            if let Some(response) = response {
                socket.write_all(&response).await.unwrap();
            }
        }
    });

    (base_url, attempts)
}

fn download_response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Disposition: attachment; filename=\"qemu-img.ccs\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn truncated_download_response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Disposition: attachment; filename=\"qemu-img.ccs\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len() + 2
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn output_filenames(output_dir: &Path) -> Vec<String> {
    let mut filenames = std::fs::read_dir(output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    filenames.sort();
    filenames
}

async fn fetch_test_package(base_url: &str, output_dir: &Path) -> Result<PathBuf> {
    RemiClient::new(base_url)?
        .fetch_package(
            "fedora",
            "qemu-img",
            Some("2:10.1.0-7.fc44"),
            None,
            output_dir,
        )
        .await
}

#[tokio::test]
async fn fetch_package_retries_typed_request_transport_failure() {
    use std::sync::atomic::Ordering;

    let (base_url, attempts) =
        spawn_download_server(vec![None, Some(download_response("200 OK", &[0x1f, 0x8b]))]).await;
    let output = tempfile::tempdir().unwrap();

    let path = fetch_test_package(&base_url, output.path())
        .await
        .expect("request transport failure should be retried");

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read(path).unwrap(), [0x1f, 0x8b]);
}

#[tokio::test]
async fn fetch_package_retries_exact_transient_http_status() {
    use std::sync::atomic::Ordering;

    let (base_url, attempts) = spawn_download_server(vec![
        Some(download_response(
            "500 Internal Server Error",
            b"first failure",
        )),
        Some(download_response("200 OK", &[0x1f, 0x8b])),
    ])
    .await;
    let output = tempfile::tempdir().unwrap();

    let path = fetch_test_package(&base_url, output.path())
        .await
        .expect("transient HTTP status should be retried");

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read(path).unwrap(), [0x1f, 0x8b]);
}

#[tokio::test]
async fn fetch_package_does_not_retry_permanent_http_status() {
    use std::sync::atomic::Ordering;

    let (base_url, attempts) =
        spawn_download_server(vec![Some(download_response("403 Forbidden", b"denied"))]).await;
    let output = tempfile::tempdir().unwrap();

    let error = fetch_test_package(&base_url, output.path())
        .await
        .unwrap_err();

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(error.to_string().contains("HTTP 403"), "{error}");
}

#[tokio::test]
async fn fetch_package_does_not_retry_invalid_ccs_body() {
    use std::sync::atomic::Ordering;

    let (base_url, attempts) =
        spawn_download_server(vec![Some(download_response("200 OK", b"not-gzip"))]).await;
    let output = tempfile::tempdir().unwrap();

    let error = fetch_test_package(&base_url, output.path())
        .await
        .unwrap_err();

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(
        error.to_string().contains("not a valid CCS package"),
        "{error}"
    );
}

#[tokio::test]
async fn fetch_package_does_not_retry_local_output_failure() {
    use std::sync::atomic::Ordering;

    let (base_url, attempts) =
        spawn_download_server(vec![Some(download_response("200 OK", &[0x1f, 0x8b]))]).await;
    let output = tempfile::tempdir().unwrap();
    let missing_output_dir = output.path().join("missing");

    let error = fetch_test_package(&base_url, &missing_output_dir)
        .await
        .unwrap_err();

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(
        error.to_string().contains("create staged output file"),
        "{error}"
    );
}

#[tokio::test]
async fn fetch_package_does_not_retry_malformed_accepted_response() {
    use std::sync::atomic::Ordering;

    let (base_url, attempts) = spawn_download_server(vec![Some(download_response(
        "202 Accepted",
        br#"{"status":"queued"}"#,
    ))])
    .await;
    let output = tempfile::tempdir().unwrap();

    let error = fetch_test_package(&base_url, output.path())
        .await
        .unwrap_err();

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(error.to_string().contains("202 response"), "{error}");
}

#[tokio::test]
async fn fetch_package_retries_transient_body_stream_failure() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let attempts = Arc::new(AtomicUsize::new(0));
    let server_attempts = attempts.clone();

    tokio::spawn(async move {
        for n in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            server_attempts.fetch_add(1, Ordering::SeqCst);

            if n == 0 {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\n\
                          Content-Disposition: attachment; filename=\"qemu-img.ccs\"\r\n\
                          Content-Length: 4\r\n\
                          Connection: close\r\n\
                          \r\n\x1f\x8b",
                    )
                    .await
                    .unwrap();
            } else {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\n\
                          Content-Disposition: attachment; filename=\"qemu-img.ccs\"\r\n\
                          Content-Length: 2\r\n\
                          Connection: close\r\n\
                          \r\n\x1f\x8b",
                    )
                    .await
                    .unwrap();
            }
        }
    });

    let output = tempfile::tempdir().unwrap();
    let client = RemiClient::new(&base_url).unwrap();
    let path = client
        .fetch_package(
            "fedora",
            "qemu-img",
            Some("2:10.1.0-7.fc44"),
            None,
            output.path(),
        )
        .await
        .expect("transient body stream failure should be retried");

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read(path).unwrap(), [0x1f, 0x8b]);
    assert_eq!(output_filenames(output.path()), ["qemu-img.ccs"]);
}

#[tokio::test]
async fn exhausted_stream_retries_leave_no_partial_artifact() {
    use std::sync::atomic::Ordering;

    let truncated = truncated_download_response(&[0x1f, 0x8b]);
    let (base_url, attempts) = spawn_download_server(vec![
        Some(truncated.clone()),
        Some(truncated.clone()),
        Some(truncated),
    ])
    .await;
    let output = tempfile::tempdir().unwrap();

    let error = fetch_test_package(&base_url, output.path())
        .await
        .unwrap_err();

    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert!(error.to_string().contains("response stream"), "{error}");
    assert!(output_filenames(output.path()).is_empty());
}

#[tokio::test]
async fn exhausted_stream_retries_preserve_existing_destination() {
    let truncated = truncated_download_response(&[0x1f, 0x8b]);
    let (base_url, _) = spawn_download_server(vec![
        Some(truncated.clone()),
        Some(truncated.clone()),
        Some(truncated),
    ])
    .await;
    let output = tempfile::tempdir().unwrap();
    let destination = output.path().join("qemu-img.ccs");
    std::fs::write(&destination, b"existing verified package").unwrap();

    fetch_test_package(&base_url, output.path())
        .await
        .unwrap_err();

    assert_eq!(
        std::fs::read(destination).unwrap(),
        b"existing verified package"
    );
    assert_eq!(output_filenames(output.path()), ["qemu-img.ccs"]);
}

#[tokio::test]
async fn invalid_body_preserves_existing_destination_and_removes_stage() {
    let (base_url, _) =
        spawn_download_server(vec![Some(download_response("200 OK", b"not-gzip"))]).await;
    let output = tempfile::tempdir().unwrap();
    let destination = output.path().join("qemu-img.ccs");
    std::fs::write(&destination, b"existing verified package").unwrap();

    fetch_test_package(&base_url, output.path())
        .await
        .unwrap_err();

    assert_eq!(
        std::fs::read(destination).unwrap(),
        b"existing verified package"
    );
    assert_eq!(output_filenames(output.path()), ["qemu-img.ccs"]);
}

#[tokio::test]
async fn valid_body_atomically_replaces_existing_destination() {
    let (base_url, _) =
        spawn_download_server(vec![Some(download_response("200 OK", &[0x1f, 0x8b]))]).await;
    let output = tempfile::tempdir().unwrap();
    let destination = output.path().join("qemu-img.ccs");
    std::fs::write(&destination, b"existing verified package").unwrap();

    let path = fetch_test_package(&base_url, output.path())
        .await
        .expect("valid staged package should replace the destination");

    assert_eq!(path, destination);
    assert_eq!(std::fs::read(path).unwrap(), [0x1f, 0x8b]);
    assert_eq!(output_filenames(output.path()), ["qemu-img.ccs"]);
}

#[tokio::test]
async fn health_check_reports_an_unreachable_server_as_unhealthy() {
    let client = RemiClient::new("http://localhost:59999").unwrap();

    let result = client.health_check().await.unwrap();
    assert!(!result);
}

#[test]
fn test_manifest_parsing() {
    let json = r#"{
        "name": "nginx",
        "version": "1.24.0",
        "distro": "arch",
        "chunks": [
            {"hash": "abc123", "size": 1024, "offset": 0},
            {"hash": "def456", "size": 2048, "offset": 1024}
        ],
        "total_size": 3072,
        "content_hash": "xyz789"
    }"#;

    let manifest: PackageManifest = serde_json::from_str(json).unwrap();
    assert_eq!(manifest.name, "nginx");
    assert_eq!(manifest.chunks.len(), 2);
    assert_eq!(manifest.total_size, 3072);
}
