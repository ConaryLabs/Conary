// conary-core/src/repository/client/tests.rs

use super::*;

#[test]
fn test_retry_policy_default() {
    let policy = RetryConfig::default();
    assert_eq!(policy.max_attempts, 3);
    assert_eq!(policy.base_delay, Duration::from_secs(1));
    assert_eq!(policy.max_delay, Duration::from_secs(30));
    assert!((policy.jitter_factor - 0.25).abs() < f64::EPSILON);
}

#[test]
fn test_retry_policy_exponential_backoff_no_jitter() {
    let policy = RetryConfig {
        max_attempts: 5,
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(10),
        jitter_factor: 0.0,
    };

    // attempt 1: 100ms * 2^0 = 100ms
    assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(100));
    // attempt 2: 100ms * 2^1 = 200ms
    assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(200));
    // attempt 3: 100ms * 2^2 = 400ms
    assert_eq!(policy.delay_for_attempt(3), Duration::from_millis(400));
    // attempt 4: 100ms * 2^3 = 800ms
    assert_eq!(policy.delay_for_attempt(4), Duration::from_millis(800));
    // attempt 5: 100ms * 2^4 = 1600ms
    assert_eq!(policy.delay_for_attempt(5), Duration::from_millis(1600));
}

#[test]
fn test_retry_policy_max_delay_cap() {
    let policy = RetryConfig {
        max_attempts: 10,
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(5),
        jitter_factor: 0.0,
    };

    // attempt 1: 1s
    assert_eq!(policy.delay_for_attempt(1), Duration::from_secs(1));
    // attempt 2: 2s
    assert_eq!(policy.delay_for_attempt(2), Duration::from_secs(2));
    // attempt 3: 4s
    assert_eq!(policy.delay_for_attempt(3), Duration::from_secs(4));
    // attempt 4: would be 8s, but capped at 5s
    assert_eq!(policy.delay_for_attempt(4), Duration::from_secs(5));
    // attempt 10: still capped at 5s
    assert_eq!(policy.delay_for_attempt(10), Duration::from_secs(5));
}

#[test]
fn test_retry_policy_jitter_within_bounds() {
    let policy = RetryConfig {
        max_attempts: 5,
        base_delay: Duration::from_millis(1000),
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.5,
    };

    // Run multiple times to check jitter stays within bounds
    for _ in 0..100 {
        let delay = policy.delay_for_attempt(1);
        // Base is 1000ms, jitter up to 50% = 500ms, so range is [1000, 1500]
        assert!(delay >= Duration::from_millis(1000));
        assert!(delay <= Duration::from_millis(1500));
    }

    for _ in 0..100 {
        let delay = policy.delay_for_attempt(3);
        // Base is 4000ms, jitter up to 50% = 2000ms, so range is [4000, 6000]
        assert!(delay >= Duration::from_millis(4000));
        assert!(delay <= Duration::from_millis(6000));
    }
}

#[test]
fn test_retry_policy_attempt_zero_saturates() {
    let policy = RetryConfig {
        max_attempts: 3,
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(10),
        jitter_factor: 0.0,
    };

    // attempt 0 should not panic (saturating_sub handles it)
    let delay = policy.delay_for_attempt(0);
    assert_eq!(delay, Duration::from_millis(100));
}

#[test]
fn test_retry_policy_large_attempt_no_overflow() {
    let policy = RetryConfig {
        max_attempts: 100,
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
    };

    // Very large attempt should not panic, just cap at max_delay
    let delay = policy.delay_for_attempt(64);
    assert_eq!(delay, Duration::from_secs(60));

    let delay = policy.delay_for_attempt(100);
    assert_eq!(delay, Duration::from_secs(60));
}

#[test]
fn test_repository_client_with_retry_policy() {
    let policy = RetryConfig {
        max_attempts: 5,
        base_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(15),
        jitter_factor: 0.1,
    };

    let client = RepositoryClient::new()
        .unwrap()
        .with_retry_policy(policy.clone());

    assert_eq!(client.retry_policy.max_attempts, 5);
    assert_eq!(client.retry_policy.base_delay, Duration::from_millis(500));
}

#[test]
fn test_append_limited_chunk_rejects_excessive_total() {
    let mut body = Vec::new();
    let mut total = 0;
    append_limited_chunk(&mut body, &mut total, &[1, 2, 3], 2, "https://example.test")
        .expect_err("chunk should be rejected once it exceeds the limit");
}

#[test]
fn test_byte_download_timeout_uses_download_budget() {
    let timeouts = TimeoutConfig {
        metadata: Duration::from_secs(30),
        download: Duration::from_secs(300),
        connect: Duration::from_secs(5),
    };

    assert_eq!(byte_download_timeout(&timeouts), Duration::from_secs(300));
}

#[tokio::test]
async fn test_download_to_bytes_requests_identity_encoding() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let read = stream.read(&mut buf).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buf[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
        String::from_utf8(request).unwrap()
    });

    let client = RepositoryClient::new().unwrap();
    let bytes = client
        .download_to_bytes(&format!("http://{addr}/metadata"))
        .await
        .unwrap();
    assert_eq!(bytes, b"ok");

    let request = server.await.unwrap().to_ascii_lowercase();
    assert!(
        request.contains("accept-encoding: identity"),
        "request headers did not force identity encoding:\n{request}"
    );
}

#[tokio::test]
async fn test_download_to_bytes_retries_a_transient_transport_failure() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            if attempt == 2 {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await
                    .unwrap();
            }
        }
    });

    let client = RepositoryClient::new()
        .unwrap()
        .with_retry_policy(RetryConfig {
            max_attempts: 2,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            jitter_factor: 0.0,
        });
    let bytes = client
        .download_to_bytes(&format!("http://{addr}/metadata"))
        .await
        .unwrap();

    assert_eq!(bytes, b"ok");
    server.await.unwrap();
}

#[tokio::test]
async fn test_download_file_requests_identity_encoding() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let read = stream.read(&mut buf).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buf[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
        String::from_utf8(request).unwrap()
    });

    let temp_dir = tempfile::tempdir().unwrap();
    let dest_path = temp_dir.path().join("package.ccs");
    let client = RepositoryClient::new().unwrap();
    let identity = client
        .download_file_with_identity(&format!("http://{addr}/package.ccs"), &dest_path)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest_path).unwrap(), b"ok");
    assert_eq!(identity.size, 2);
    assert_eq!(identity.sha256, crate::hash::sha256(b"ok"));

    let request = server.await.unwrap().to_ascii_lowercase();
    assert!(
        request.contains("accept-encoding: identity"),
        "file download request did not force identity encoding:\n{request}"
    );
}
