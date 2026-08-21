// conary-core/src/repository/remi/tests.rs

use super::*;

#[test]
fn base_url_is_normalized() {
    let client = RemiClient::new("http://localhost:8080/").unwrap();
    assert_eq!(client.core.base_url, "http://localhost:8080");
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
        (
            "cancelled",
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
    assert!(serde_json::from_str::<JobStatus>(
        r#"{"job_id":"1","status":"blocked","distro":"fedora","package":"kernel-core","version":null,"architecture":"x86_64","progress":null,"error":null,"manifest":null}"#
    )
    .unwrap_err()
    .to_string()
    .contains("unknown variant `blocked`"));
}

#[test]
fn package_url_preserves_version_release_and_architecture() {
    let core = RemiClientCore::new("https://remi.example.test").unwrap();
    assert_eq!(
        core.package_url("fedora", "hello", Some("1.0.0"), Some("1"), Some("noarch")),
        "https://remi.example.test/v1/fedora/packages/hello?version=1.0.0&release=1&arch=noarch"
    );
}

const CONTRACT_JOB_ID: &str = "55";
const CONTRACT_PACKAGE: &str = "kernel-modules-core";

fn poll_manifest() -> PackageManifest {
    PackageManifest {
        name: CONTRACT_PACKAGE.to_string(),
        version: "6.19.10-300.fc44".to_string(),
        distro: "fedora".to_string(),
        transport: crate::ccs::transport::test_transport(&[]),
        total_size: 0,
        content_hash: "sha256:test".to_string(),
    }
}

fn job_response(state: &str, error: Option<&str>, manifest: Option<&PackageManifest>) -> String {
    serde_json::json!({
        "job_id": CONTRACT_JOB_ID,
        "status": state,
        "distro": "fedora",
        "package": CONTRACT_PACKAGE,
        "version": "6.19.10-300.fc44",
        "architecture": "x86_64",
        "progress": null,
        "error": error,
        "manifest": manifest,
    })
    .to_string()
}

async fn write_json_response(listener: &tokio::net::TcpListener, status: &str, body: &str) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = [0_u8; 2048];
    let _ = socket.read(&mut request).await.unwrap();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    socket.write_all(response.as_bytes()).await.unwrap();
}

#[tokio::test]
async fn active_job_states_wait_until_the_manifest_is_ready() {
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let accepted = serde_json::json!({
            "status": "queued",
            "job_id": CONTRACT_JOB_ID,
            "poll_url": format!("/v1/jobs/{CONTRACT_JOB_ID}"),
            "eta_seconds": null,
        })
        .to_string();
        write_json_response(&listener, "202 Accepted", &accepted).await;
        write_json_response(&listener, "200 OK", &job_response("pending", None, None)).await;
        write_json_response(&listener, "200 OK", &job_response("converting", None, None)).await;
        write_json_response(
            &listener,
            "200 OK",
            &job_response("ready", None, Some(&poll_manifest())),
        )
        .await;
    });

    let mut client = RemiClient::new(&base_url).unwrap();
    client.core.poll_interval = Duration::from_millis(1);
    let manifest = client
        .get_package("fedora", CONTRACT_PACKAGE, None, Some("x86_64"))
        .await
        .unwrap();
    assert_eq!(manifest.name, CONTRACT_PACKAGE);
    server.await.unwrap();
}

#[tokio::test]
async fn cold_fetch_downloads_signed_objects_and_warm_fetch_reuses_the_cas() {
    use crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests;
    use crate::ccs::signing::SigningKeyPair;
    use crate::ccs::verify::{TrustPolicy, verify_package, verify_package_into_cas};
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let fixture_dir = tempfile::tempdir().unwrap();
    let package_path = fixture_dir.path().join("fixture.ccs");
    let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("transport");
    let payload_bytes = (0..(crate::ccs::chunking::MAX_CHUNK_SIZE as usize * 8))
        .map(|index| ((index * 31 + index / 251) % 256) as u8)
        .collect::<Vec<_>>();
    let crate::ccs::v3::PackageKindV3::Package(package) = &mut authority.kind else {
        unreachable!()
    };
    package.files[0].content = Some(crate::payload::PayloadContentAuthority {
        sha256: crate::hash::sha256(&payload_bytes),
        size: payload_bytes.len() as u64,
    });
    package.files[0].content_layout = crate::ccs::v3::FileContentLayoutV3::FastCdcV2020 {
        min_size: crate::ccs::chunking::MIN_CHUNK_SIZE,
        average_size: crate::ccs::chunking::AVG_CHUNK_SIZE,
        max_size: crate::ccs::chunking::MAX_CHUNK_SIZE,
        chunks: crate::ccs::chunking::Chunker::new()
            .chunk_bytes(&payload_bytes)
            .iter()
            .map(crate::ccs::chunking::Chunk::reference)
            .collect(),
    };
    authority.components.get_mut("main").unwrap().total_size = payload_bytes.len() as u64;
    let payloads = BTreeMap::from([("/usr/bin/hello".to_string(), payload_bytes)]);
    let signer = SigningKeyPair::generate();
    write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        None,
        None,
    )
    .unwrap();
    let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
    let verified = verify_package(&package_path, &policy).unwrap();
    let transport = crate::ccs::CcsTransportEnvelopeV1::from_verified_archive(&verified).unwrap();
    let manifest = PackageManifest {
        name: authority.identity.name.clone(),
        version: authority.identity.version.clone(),
        distro: "fedora".to_string(),
        transport,
        total_size: package_path.metadata().unwrap().len(),
        content_hash: crate::hash::sha256(&std::fs::read(&package_path).unwrap()),
    };
    let manifest_body = serde_json::to_vec(&manifest).unwrap();
    let mut object_bodies = BTreeMap::new();
    for (hash, source) in verified.object_sources() {
        let mut body = Vec::new();
        source.open().unwrap().read_to_end(&mut body).unwrap();
        object_bodies.insert(hash.clone(), body);
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let object_requests = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&object_requests);
    let expected_connections = object_bodies.len() + 2;
    let server = tokio::spawn(async move {
        for _ in 0..expected_connections {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                if read == 0 || request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let first_line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap()
                .to_string();
            let body = if first_line.starts_with("GET /v1/chunks/") {
                observed.fetch_add(1, Ordering::SeqCst);
                let hash = first_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .trim_start_matches("/v1/chunks/");
                object_bodies.get(hash).unwrap().clone()
            } else {
                manifest_body.clone()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        }
    });

    let output = tempfile::tempdir().unwrap();
    let cas_dir = tempfile::tempdir().unwrap();
    let cas = crate::filesystem::CasStore::new(cas_dir.path()).unwrap();
    let client = RemiClient::new(&base_url).unwrap();
    let cold = client
        .fetch_package_with_metrics(RemiFetchRequest {
            distro: "fedora",
            name: &authority.identity.name,
            version: Some(&authority.identity.version),
            architecture: None,
            output_dir: output.path(),
            cas: &cas,
            trust_policy: &policy,
        })
        .await
        .unwrap();
    let warm = client
        .fetch_package_with_metrics(RemiFetchRequest {
            distro: "fedora",
            name: &authority.identity.name,
            version: Some(&authority.identity.version),
            architecture: None,
            output_dir: output.path(),
            cas: &cas,
            trust_policy: &policy,
        })
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(cold.metrics.objects_downloaded, cold.metrics.objects_total);
    assert_eq!(cold.metrics.cas_hits, 0);
    assert_eq!(warm.metrics.objects_downloaded, 0);
    assert_eq!(warm.metrics.cas_hits, warm.metrics.objects_total);
    assert_eq!(
        object_requests.load(Ordering::SeqCst),
        cold.metrics.objects_total as usize
    );
    let install_ready = verify_package_into_cas(&warm.path, &policy, &cas).unwrap();
    let install_metrics = install_ready.verified_object_metrics().unwrap();
    assert_eq!(install_metrics.hits, warm.metrics.objects_total);
    assert_eq!(install_metrics.misses, 0);
    assert_eq!(install_metrics.persistent_bytes_written, 0);
    assert_eq!(install_metrics.canonical_bytes_reread, 0);
    println!(
        "ISSUE400_COLD_OBJECTS={} ISSUE400_COLD_BYTES={} ISSUE400_WARM_OBJECTS={} ISSUE400_WARM_BYTES={}",
        cold.metrics.objects_downloaded,
        cold.metrics.bytes_downloaded,
        warm.metrics.objects_downloaded,
        warm.metrics.bytes_downloaded
    );
}
