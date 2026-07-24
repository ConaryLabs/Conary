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
    assert_eq!(status.status, "ready");
    assert_eq!(status.package, "gzip");
}

#[test]
fn job_status_parses_publication_refusal_report() {
    let json = r#"{
        "job_id": "35",
        "status": "blocked",
        "distro": "fedora",
        "package": "kernel-core",
        "version": "6.19.10-300.fc44",
        "architecture": "x86_64",
        "progress": null,
        "error": null,
        "manifest": null,
        "publication": {
            "publication_status": "blocked",
            "scriptlet_fidelity": "blocked",
            "target_compatibility": "blocked",
            "summary_valid": true,
            "message": "Converted package is blocked by legacy scriptlet policy",
            "reason_codes": ["blocked-class-selinux", "unknown-command:kernel-install", "selinux"],
            "blocked_reason_codes": ["blocked-class-selinux"],
            "review_reason_codes": [],
            "unknown_command_evidence": [{
                "command": "kernel-install",
                "command_provenance": "literal",
                "argv": ["add", "<kver>", "<path>"],
                "argument_provenance": ["literal", "literal", "literal"],
                "execution_context": "unconditional",
                "phase": "post-install",
                "lifecycle_paths": ["post-install"],
                "source": "shell-ast",
                "environment": []
            }],
            "blocked_classes": ["selinux"],
            "evidence_digest": "sha256:abc",
            "curation_evidence_digest": null,
            "review_artifact_available": true
        }
    }"#;

    let status: JobStatus = serde_json::from_str(json).unwrap();

    let publication = status.publication.expect("publication report");
    assert_eq!(publication.blocked_classes, vec!["selinux"]);
    assert_eq!(
        publication.blocked_reason_codes,
        vec!["blocked-class-selinux"]
    );
}

#[test]
fn terminal_publication_status_becomes_actionable_error() {
    let status = JobStatus {
        job_id: "35".to_string(),
        status: "blocked".to_string(),
        distro: "fedora".to_string(),
        package: "kernel-core".to_string(),
        version: Some("6.19.10-300.fc44".to_string()),
        architecture: Some("x86_64".to_string()),
        progress: None,
        error: None,
        manifest: None,
        publication: Some(PublicationGateReport {
            publication_status: "blocked".to_string(),
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            summary_valid: true,
            message: "Converted package is blocked by legacy scriptlet policy".to_string(),
            reason_codes: vec![
                "blocked-class-initramfs".to_string(),
                "blocked-class-kernel-module".to_string(),
                "kernel-module".to_string(),
                "initramfs".to_string(),
            ],
            blocked_reason_codes: vec![
                "blocked-class-initramfs".to_string(),
                "blocked-class-kernel-module".to_string(),
            ],
            review_reason_codes: vec![],
            unknown_command_evidence: vec![crate::ccs::legacy_scriptlets::UnknownCommandEvidence {
                command: "dracut".to_string(),
                argv: vec!["--force".to_string()],
                phase: Some("post-install".to_string()),
                lifecycle_paths: vec!["post-install".to_string()],
                source: crate::ccs::legacy_scriptlets::CommandEvidenceSource::ShellAst,
                environment: Vec::new(),
                ..crate::ccs::legacy_scriptlets::UnknownCommandEvidence::default()
            }],
            blocked_classes: vec!["initramfs".to_string(), "kernel-module".to_string()],
            boot_security_intents: Vec::new(),
            evidence_digest: Some("sha256:def".to_string()),
            curation_evidence_digest: None,
            review_artifact_available: true,
        }),
    };

    let err = terminal_publication_status_error(&status).expect("terminal error");
    let message = err.to_string();

    assert!(message.contains("Remi refused to serve fedora/kernel-core"));
    assert!(message.contains("blocked classes: initramfs, kernel-module"));
    assert!(message.contains("kernel/initramfs/SELinux scriptlets"));
    assert!(message.contains("public preview"));
    assert!(!message.contains("\"publication_status\""));
}

#[test]
fn direct_publication_refusal_http_error_is_pretty_printed() {
    let core = RemiClientCore::new("https://remi.example.test").unwrap();
    let body = r#"{
        "status": "blocked",
        "message": "Converted package is blocked by legacy scriptlet policy",
        "distro": "fedora",
        "package": "kernel-core",
        "version": "6.19.10-300.fc44",
        "scriptlets": {
            "publication_status": "blocked",
            "scriptlet_fidelity": "blocked",
            "target_compatibility": "blocked",
            "summary_valid": true,
            "message": "Converted package is blocked by legacy scriptlet policy",
            "reason_codes": ["blocked-class-selinux", "selinux"],
            "blocked_reason_codes": ["blocked-class-selinux"],
            "review_reason_codes": [],
            "unknown_command_evidence": [],
            "blocked_classes": ["selinux"],
            "evidence_digest": "sha256:abc",
            "curation_evidence_digest": null,
            "review_artifact_available": true
        }
    }"#;

    let err = core.map_http_error(403, body.to_string(), "kernel-core", "fedora");
    let message = err.to_string();

    assert!(message.contains("Remi refused to serve fedora/kernel-core"));
    assert!(message.contains("blocked classes: selinux"));
    assert!(message.contains("kernel/initramfs/SELinux scriptlets"));
    assert!(!message.contains("\"scriptlets\""));
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

#[tokio::test]
async fn get_package_stops_on_blocked_job_status() {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());

    tokio::spawn(async move {
        let accepted =
            r#"{"status":"queued","job_id":"35","poll_url":"/v1/jobs/35","eta_seconds":1}"#;
        write_json_response(&listener, "202 Accepted", accepted).await;

        let blocked = r#"{
            "job_id": "35",
            "status": "blocked",
            "distro": "fedora",
            "package": "kernel-core",
            "version": "6.19.10-300.fc44",
            "architecture": "x86_64",
            "progress": null,
            "error": null,
            "manifest": null,
            "publication": {
                "publication_status": "blocked",
                "scriptlet_fidelity": "blocked",
                "target_compatibility": "blocked",
                "summary_valid": true,
                "message": "Converted package uses unsupported legacy scriptlet classes for the Remi public preview: selinux",
                "reason_codes": ["blocked-class-selinux", "selinux"],
                "blocked_reason_codes": ["blocked-class-selinux"],
                "review_reason_codes": [],
                "unknown_command_evidence": [],
                "blocked_classes": ["selinux"],
                "evidence_digest": "sha256:abc",
                "curation_evidence_digest": null,
                "review_artifact_available": true
            }
        }"#;
        write_json_response(&listener, "200 OK", blocked).await;
    });

    let client = RemiClient::new(&base_url).unwrap();
    let err = client
        .get_package("fedora", "kernel-core", None, Some("x86_64"))
        .await
        .unwrap_err();
    let message = err.to_string();

    assert!(message.contains("Remi refused to serve fedora/kernel-core"));
    assert!(message.contains("blocked classes: selinux"));
    assert!(!message.contains("Unknown job status"));
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
}

mod async_tests {
    use super::*;

    #[test]
    fn test_async_client_base_url_normalization() {
        let temp_dir = tempfile::tempdir().unwrap();

        // With trailing slash
        let client = AsyncRemiClient::new("http://localhost:8080/", temp_dir.path()).unwrap();
        assert_eq!(client.core.base_url, "http://localhost:8080");

        // Without trailing slash
        let client = AsyncRemiClient::new("http://localhost:8080", temp_dir.path()).unwrap();
        assert_eq!(client.core.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_async_client_with_custom_fetcher() {
        use crate::repository::chunk_fetcher::{CompositeChunkFetcher, LocalCacheFetcher};

        let temp_dir = tempfile::tempdir().unwrap();
        let cache = LocalCacheFetcher::new(temp_dir.path());
        let fetcher = CompositeChunkFetcher::new(vec![Arc::new(cache)]);

        let client = AsyncRemiClient::with_fetcher("http://localhost:8080", fetcher).unwrap();
        assert_eq!(client.core.base_url, "http://localhost:8080");
    }

    #[tokio::test]
    async fn test_async_client_health_check_unreachable() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = AsyncRemiClient::new("http://localhost:59999", temp_dir.path()).unwrap();

        // Should return false for unreachable server
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
}
