// conary-core/src/repository/substituter/tests.rs

use super::*;
use crate::db::schema;
use crate::hash::sha256;
use rusqlite::Connection;
use std::fs;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Helper to write a chunk into a local cache directory using the CAS layout:
/// `{cache_dir}/objects/{hash[0:2]}/{hash[2:]}`
fn write_chunk_to_cache(cache_dir: &Path, hash: &str, data: &[u8]) {
    let (prefix, rest) = hash.split_at(2);
    let dir = cache_dir.join("objects").join(prefix);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(rest), data).unwrap();
}

fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .unwrap();
    schema::ensure_current(&conn).unwrap();
    conn
}

async fn spawn_chunk_server(routes: HashMap<String, Vec<u8>>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen_paths = Arc::new(Mutex::new(Vec::new()));
    let seen_paths_task = Arc::clone(&seen_paths);
    let routes = Arc::new(routes);

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = [0_u8; 4096];
            let bytes_read = match stream.read(&mut buf).await {
                Ok(bytes_read) => bytes_read,
                Err(_) => continue,
            };
            if bytes_read == 0 {
                continue;
            }

            let request = String::from_utf8_lossy(&buf[..bytes_read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            seen_paths_task.lock().unwrap().push(path.clone());

            let response = if let Some(hash) = path.strip_prefix("/v1/chunks/") {
                if let Some(body) = routes.get(hash) {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes()
                    .into_iter()
                    .chain(body.iter().copied())
                    .collect::<Vec<_>>()
                } else {
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                }
            } else {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            };

            let _ = stream.write_all(&response).await;
        }
    });

    (format!("http://{addr}"), seen_paths)
}

async fn spawn_durable_failure_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buffer = [0_u8; 4096];
            if stream.read(&mut buffer).await.unwrap_or(0) == 0 {
                continue;
            }
            let _ = stream
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nX-Conary-Error: durable-chunk-unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
    });
    format!("http://{addr}")
}

#[test]
fn test_chain_ordering() {
    let chain = SubstituterChain::new(vec![
        SubstituterSource::LocalCache {
            cache_dir: PathBuf::from("/tmp/cache"),
        },
        SubstituterSource::Federation {
            tier: "cell_hub".to_string(),
        },
        SubstituterSource::Remi {
            endpoint: "https://remi.example.com".to_string(),
            distro: "fedora-41".to_string(),
        },
    ]);

    assert_eq!(chain.len(), 3);
    assert_eq!(chain.sources()[0].name(), "local-cache");
    assert_eq!(chain.sources()[1].name(), "federation");
    assert_eq!(chain.sources()[2].name(), "remi");
}

#[test]
fn test_add_source() {
    let mut chain = SubstituterChain::new(Vec::new());
    assert!(chain.is_empty());
    assert_eq!(chain.len(), 0);

    chain.add_source(SubstituterSource::Remi {
        endpoint: "https://remi.example.com".to_string(),
        distro: "fedora-41".to_string(),
    });
    assert!(!chain.is_empty());
    assert_eq!(chain.len(), 1);
    assert_eq!(chain.sources().len(), 1);
}

#[tokio::test]
async fn test_local_cache_resolve_async() {
    let tmp_dir = TempDir::new().unwrap();
    let cache_dir = tmp_dir.path();

    let hash = sha256(b"test chunk content");
    let data = b"test chunk content";
    write_chunk_to_cache(cache_dir, &hash, data);

    let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
        cache_dir: cache_dir.to_path_buf(),
    }]);

    let (resolved_data, result) = chain.resolve_chunk(&hash, None).await.unwrap();
    assert_eq!(resolved_data, data);
    assert_eq!(result.source_name, "local-cache");
    assert_eq!(result.source_index, 0);
    assert!(result.peer_metrics.is_empty());
}

#[tokio::test]
async fn test_local_cache_miss() {
    let tmp_dir = TempDir::new().unwrap();

    let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
        cache_dir: tmp_dir.path().to_path_buf(),
    }]);

    let result = chain
        .resolve_chunk(
            "deadbeef00112233deadbeef00112233deadbeef00112233deadbeef00112233",
            None,
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_resolve_chunks_batch_async() {
    let tmp_dir = TempDir::new().unwrap();
    let cache_dir = tmp_dir.path();

    let hash1 = sha256(b"data-one");
    let hash2 = sha256(b"data-two");

    write_chunk_to_cache(cache_dir, &hash1, b"data-one");
    write_chunk_to_cache(cache_dir, &hash2, b"data-two");

    let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
        cache_dir: cache_dir.to_path_buf(),
    }]);

    let result = chain
        .resolve_chunks(&[hash1.clone(), hash2.clone()], None)
        .await
        .unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(result.chunks[&hash1], b"data-one");
    assert_eq!(result.chunks[&hash2], b"data-two");
    assert!(result.peer_metrics.is_empty());
}

#[tokio::test]
async fn test_resolve_chunks_partial() {
    let tmp_dir = TempDir::new().unwrap();
    let cache_dir = tmp_dir.path();

    let hash1 = sha256(b"only-this-one");
    let hash2 = sha256(b"missing-one");

    write_chunk_to_cache(cache_dir, &hash1, b"only-this-one");

    let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
        cache_dir: cache_dir.to_path_buf(),
    }]);

    let result = chain
        .resolve_chunks(&[hash1.clone(), hash2.clone()], None)
        .await
        .unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result.chunks[&hash1], b"only-this-one");
    assert!(!result.chunks.contains_key(&hash2));
}

#[tokio::test]
async fn test_resolve_chunks_falls_through_sources() {
    let empty_cache = TempDir::new().unwrap();
    let populated_cache = TempDir::new().unwrap();

    let hash = sha256(b"found-in-second");
    write_chunk_to_cache(populated_cache.path(), &hash, b"found-in-second");

    let chain = SubstituterChain::new(vec![
        SubstituterSource::LocalCache {
            cache_dir: empty_cache.path().to_path_buf(),
        },
        SubstituterSource::LocalCache {
            cache_dir: populated_cache.path().to_path_buf(),
        },
    ]);

    let result = chain
        .resolve_chunks(std::slice::from_ref(&hash), None)
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result.chunks[&hash], b"found-in-second");
}

#[tokio::test]
async fn durable_remi_failure_stops_single_source_fallback() {
    let populated_cache = TempDir::new().unwrap();
    let hash = sha256(b"must-not-fallback");
    write_chunk_to_cache(populated_cache.path(), &hash, b"must-not-fallback");
    let chain = SubstituterChain::new(vec![
        SubstituterSource::Remi {
            endpoint: spawn_durable_failure_server().await,
            distro: "fedora-44".to_string(),
        },
        SubstituterSource::LocalCache {
            cache_dir: populated_cache.path().to_path_buf(),
        },
    ]);

    let error = chain.resolve_chunk(&hash, None).await.unwrap_err();
    assert!(matches!(error, Error::DurableChunkUnavailable { .. }));
}

#[tokio::test]
async fn durable_remi_failure_stops_batch_source_fallback() {
    let populated_cache = TempDir::new().unwrap();
    let hash = sha256(b"must-not-batch-fallback");
    write_chunk_to_cache(populated_cache.path(), &hash, b"must-not-batch-fallback");
    let chain = SubstituterChain::new(vec![
        SubstituterSource::Remi {
            endpoint: spawn_durable_failure_server().await,
            distro: "fedora-44".to_string(),
        },
        SubstituterSource::LocalCache {
            cache_dir: populated_cache.path().to_path_buf(),
        },
    ]);

    let error = chain
        .resolve_chunks(std::slice::from_ref(&hash), None)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::DurableChunkUnavailable { .. }));
}

#[test]
fn test_source_name() {
    assert_eq!(
        SubstituterSource::LocalCache {
            cache_dir: PathBuf::from("/tmp")
        }
        .name(),
        "local-cache"
    );
    assert_eq!(
        SubstituterSource::Federation {
            tier: "region_hub".to_string()
        }
        .name(),
        "federation"
    );
    assert_eq!(
        SubstituterSource::Remi {
            endpoint: "https://remi.example.com".to_string(),
            distro: "fedora".to_string(),
        }
        .name(),
        "remi"
    );
}

#[test]
fn test_serde_roundtrip() {
    let sources = vec![
        SubstituterSource::LocalCache {
            cache_dir: PathBuf::from("/var/cache/conary"),
        },
        SubstituterSource::Federation {
            tier: "cell_hub".to_string(),
        },
        SubstituterSource::Remi {
            endpoint: "https://remi.example.com".to_string(),
            distro: "fedora-41".to_string(),
        },
    ];

    let json = serde_json::to_string(&sources).unwrap();
    let deserialized: Vec<SubstituterSource> = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.len(), 3);
    assert!(matches!(
        &deserialized[0],
        SubstituterSource::LocalCache { cache_dir } if cache_dir == Path::new("/var/cache/conary")
    ));
    assert!(matches!(
        &deserialized[1],
        SubstituterSource::Federation { tier } if tier == "cell_hub"
    ));
    assert!(matches!(
        &deserialized[2],
        SubstituterSource::Remi { endpoint, distro }
            if endpoint == "https://remi.example.com" && distro == "fedora-41"
    ));
}

#[tokio::test]
async fn test_empty_chain_async() {
    let chain = SubstituterChain::new(Vec::new());
    assert!(chain.is_empty());
    assert_eq!(chain.len(), 0);

    let result = chain.resolve_chunk("abcdef1234567890", None).await;
    assert!(result.is_err());

    let result = chain.resolve_chunks(&[], None).await.unwrap();
    assert!(result.is_empty());

    let result = chain
        .resolve_chunks(&["abcdef1234567890".to_string()], None)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_resolve_chunks_empty_request() {
    let chain = SubstituterChain::new(vec![SubstituterSource::LocalCache {
        cache_dir: PathBuf::from("/nonexistent"),
    }]);
    let result = chain.resolve_chunks(&[], None).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_remi_source_fetches_chunk_and_populates_cache() {
    let tmp_dir = TempDir::new().unwrap();
    let data = b"remi-data".to_vec();
    let hash = sha256(&data);
    let (endpoint, seen_paths) =
        spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;

    let chain = SubstituterChain::new(vec![
        SubstituterSource::LocalCache {
            cache_dir: tmp_dir.path().to_path_buf(),
        },
        SubstituterSource::Remi {
            endpoint,
            distro: "fedora-42".to_string(),
        },
    ]);

    let (resolved, result) = chain.resolve_chunk(&hash, None).await.unwrap();
    assert_eq!(resolved, data);
    assert_eq!(result.source_name, "remi");

    let cached = chain.resolve_chunk(&hash, None).await.unwrap();
    assert_eq!(cached.1.source_name, "local-cache");
    assert_eq!(seen_paths.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_resolve_chunk_uses_cached_copy_after_remi_hit() {
    let tmp_dir = TempDir::new().unwrap();
    let data = b"cached-after-remi".to_vec();
    let hash = sha256(&data);
    let (endpoint, seen_paths) =
        spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;

    let chain = SubstituterChain::new(vec![
        SubstituterSource::LocalCache {
            cache_dir: tmp_dir.path().to_path_buf(),
        },
        SubstituterSource::Remi {
            endpoint,
            distro: "fedora-42".to_string(),
        },
    ]);

    let first = chain.resolve_chunk(&hash, None).await.unwrap();
    assert_eq!(first.1.source_name, "remi");
    assert_eq!(seen_paths.lock().unwrap().len(), 1);

    let second = chain.resolve_chunk(&hash, None).await.unwrap();
    assert_eq!(second.1.source_name, "local-cache");
    assert_eq!(seen_paths.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_federation_source_skips_disabled_and_open_circuit_peers() {
    let conn = test_conn();
    let data = b"federation-data".to_vec();
    let hash = sha256(&data);
    let (healthy_endpoint, seen_paths) =
        spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;

    federation_peer::insert(
        &conn,
        "peer-disabled",
        "http://127.0.0.1:1",
        Some("Disabled"),
        "leaf",
    )
    .unwrap();
    federation_peer::insert(
        &conn,
        "peer-open-circuit",
        "http://127.0.0.1:2",
        Some("Open Circuit"),
        "leaf",
    )
    .unwrap();
    federation_peer::insert(
        &conn,
        "peer-healthy",
        &healthy_endpoint,
        Some("Healthy"),
        "leaf",
    )
    .unwrap();
    conn.execute(
        "UPDATE federation_peers SET is_enabled = 0 WHERE id = 'peer-disabled'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE federation_peers SET consecutive_failures = 6 WHERE id = 'peer-open-circuit'",
        [],
    )
    .unwrap();

    let chain = SubstituterChain::new(vec![SubstituterSource::Federation {
        tier: "leaf".to_string(),
    }]);
    let prepared = chain.prepare_federation_peers(&conn).unwrap();

    let (resolved, result) = chain.resolve_chunk(&hash, Some(&prepared)).await.unwrap();
    assert_eq!(resolved, data);
    assert_eq!(result.source_name, "federation");
    assert_eq!(result.peer_metrics.len(), 1);
    assert_eq!(result.peer_metrics[0].peer_id, "peer-healthy");
    assert!(result.peer_metrics[0].succeeded);
    assert_eq!(seen_paths.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_federation_source_falls_through_after_failed_peer() {
    let conn = test_conn();
    let data = b"healthy-peer-data".to_vec();
    let hash = sha256(&data);
    let (healthy_endpoint, _healthy_seen) =
        spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;
    let (failing_endpoint, _failing_seen) = spawn_chunk_server(HashMap::new()).await;

    federation_peer::insert(
        &conn,
        "peer-fail",
        &failing_endpoint,
        Some("Failing"),
        "leaf",
    )
    .unwrap();
    federation_peer::insert(&conn, "peer-ok", &healthy_endpoint, Some("Healthy"), "leaf").unwrap();
    conn.execute(
        "UPDATE federation_peers SET latency_ms = 5 WHERE id = 'peer-fail'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE federation_peers SET latency_ms = 50 WHERE id = 'peer-ok'",
        [],
    )
    .unwrap();

    let chain = SubstituterChain::new(vec![SubstituterSource::Federation {
        tier: "leaf".to_string(),
    }]);
    let prepared = chain.prepare_federation_peers(&conn).unwrap();

    let (resolved, result) = chain.resolve_chunk(&hash, Some(&prepared)).await.unwrap();
    assert_eq!(resolved, data);
    assert_eq!(result.peer_metrics.len(), 2);
    assert_eq!(result.peer_metrics[0].peer_id, "peer-fail");
    assert!(!result.peer_metrics[0].succeeded);
    assert_eq!(result.peer_metrics[1].peer_id, "peer-ok");
    assert!(result.peer_metrics[1].succeeded);

    SubstituterChain::apply_peer_metrics(&conn, &result.peer_metrics).unwrap();
    let failed = federation_peer::find_by_id(&conn, "peer-fail")
        .unwrap()
        .unwrap();
    let healthy = federation_peer::find_by_id(&conn, "peer-ok")
        .unwrap()
        .unwrap();
    assert_eq!(failed.failure_count, 1);
    assert_eq!(healthy.success_count, 1);
}

#[tokio::test]
async fn test_federation_source_records_success_metrics_and_caches_chunk() {
    let conn = test_conn();
    let tmp_dir = TempDir::new().unwrap();
    let data = b"cached-federation-hit".to_vec();
    let hash = sha256(&data);
    let (healthy_endpoint, seen_paths) =
        spawn_chunk_server(HashMap::from([(hash.clone(), data.clone())])).await;

    federation_peer::insert(
        &conn,
        "peer-cache",
        &healthy_endpoint,
        Some("Cache Peer"),
        "leaf",
    )
    .unwrap();

    let chain = SubstituterChain::new(vec![
        SubstituterSource::LocalCache {
            cache_dir: tmp_dir.path().to_path_buf(),
        },
        SubstituterSource::Federation {
            tier: "leaf".to_string(),
        },
    ]);
    let prepared = chain.prepare_federation_peers(&conn).unwrap();

    let first = chain.resolve_chunk(&hash, Some(&prepared)).await.unwrap();
    assert_eq!(first.1.source_name, "federation");
    SubstituterChain::apply_peer_metrics(&conn, &first.1.peer_metrics).unwrap();

    let peer = federation_peer::find_by_id(&conn, "peer-cache")
        .unwrap()
        .unwrap();
    assert_eq!(peer.success_count, 1);

    let second = chain.resolve_chunk(&hash, None).await.unwrap();
    assert_eq!(second.1.source_name, "local-cache");
    assert_eq!(seen_paths.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_federation_source_without_prepared_peers_is_skipped() {
    let hash = sha256(b"never-fetched");
    let chain = SubstituterChain::new(vec![SubstituterSource::Federation {
        tier: "leaf".to_string(),
    }]);

    let err = chain.resolve_chunk(&hash, None).await.unwrap_err();
    assert!(format!("{err}").contains("prepared peer data"));
}
