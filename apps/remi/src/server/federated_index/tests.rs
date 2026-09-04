// apps/remi/src/server/federated_index/tests.rs

use super::*;

const REVISION_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const REVISION_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn make_version(ver: &str, converted: bool) -> SparseVersionEntry {
    SparseVersionEntry {
        version: ver.to_string(),
        release: None,
        provides: Vec::new(),
        requirement_groups: Vec::new(),
        architecture: Some("x86_64".to_string()),
        size: 1024,
        converted,
        content_hash: if converted {
            Some(format!("sha256:{ver}"))
        } else {
            None
        },
    }
}

fn make_entry(name: &str, distro: &str, versions: Vec<SparseVersionEntry>) -> SparseIndexEntry {
    SparseIndexEntry {
        name: name.to_string(),
        distro: distro.to_string(),
        versions,
    }
}

#[test]
fn test_merge_empty() {
    let merged = merge_sparse_entries("fedora", "nginx", vec![]).unwrap();
    assert_eq!(merged.name, "nginx");
    assert_eq!(merged.distro, "fedora");
    assert!(merged.versions.is_empty());
}

#[test]
fn test_merge_single_entry() {
    let entry = make_entry(
        "nginx",
        "fedora",
        vec![make_version("1.0", true), make_version("2.0", false)],
    );

    let merged = merge_sparse_entries("fedora", "nginx", vec![entry]).unwrap();
    assert_eq!(merged.name, "nginx");
    assert_eq!(merged.distro, "fedora");
    assert_eq!(merged.versions.len(), 2);
}

#[test]
fn test_merge_no_overlap() {
    let entry1 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);
    let entry2 = make_entry("nginx", "fedora", vec![make_version("2.0", true)]);

    let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2]).unwrap();
    assert_eq!(merged.versions.len(), 2);
    assert_eq!(merged.versions[0].version, "1.0");
    assert_eq!(merged.versions[1].version, "2.0");
}

#[test]
fn test_merge_overlapping_prefer_converted() {
    // Source 1: v1.0 converted, v2.0 not converted
    let entry1 = make_entry(
        "nginx",
        "fedora",
        vec![make_version("1.0", true), make_version("2.0", false)],
    );

    // Source 2: v2.0 converted, v3.0 not converted
    let entry2 = make_entry(
        "nginx",
        "fedora",
        vec![make_version("2.0", true), make_version("3.0", false)],
    );

    let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2]).unwrap();
    assert_eq!(merged.versions.len(), 3);

    let v2 = merged.versions.iter().find(|v| v.version == "2.0").unwrap();
    assert!(v2.converted, "Should prefer converted=true for v2.0");
    assert!(v2.content_hash.is_some());
}

#[test]
fn test_merge_keeps_first_when_both_converted() {
    let entry1 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);
    let entry2 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);

    let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2]).unwrap();
    assert_eq!(merged.versions.len(), 1);
    assert!(merged.versions[0].converted);
}

#[test]
fn test_merge_keeps_first_when_both_unconverted() {
    let entry1 = make_entry("nginx", "fedora", vec![make_version("1.0", false)]);
    let entry2 = make_entry("nginx", "fedora", vec![make_version("1.0", false)]);

    let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2]).unwrap();
    assert_eq!(merged.versions.len(), 1);
    assert!(!merged.versions[0].converted);
}

#[test]
fn test_merge_sorted_output() {
    let entry1 = make_entry("nginx", "fedora", vec![make_version("3.0", true)]);
    let entry2 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);
    let entry3 = make_entry("nginx", "fedora", vec![make_version("2.0", true)]);

    let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2, entry3]).unwrap();
    assert_eq!(merged.versions.len(), 3);
    assert_eq!(merged.versions[0].version, "1.0");
    assert_eq!(merged.versions[1].version, "2.0");
    assert_eq!(merged.versions[2].version, "3.0");
}

#[test]
fn merge_preserves_sibling_releases_and_architectures() {
    let mut first = make_version("1.0", false);
    first.release = Some("1".to_string());
    let mut second = make_version("1.0", false);
    second.release = Some("2".to_string());
    let mut third = make_version("1.0", false);
    third.release = Some("2".to_string());
    third.architecture = Some("aarch64".to_string());

    let merged = merge_sparse_entries(
        "fedora",
        "demo",
        vec![
            make_entry("demo", "fedora", vec![first]),
            make_entry("demo", "fedora", vec![second, third]),
        ],
    )
    .unwrap();

    assert_eq!(merged.versions.len(), 3);
    assert_eq!(merged.versions[0].release.as_deref(), Some("1"));
    assert_eq!(merged.versions[1].architecture.as_deref(), Some("aarch64"));
    assert_eq!(merged.versions[2].architecture.as_deref(), Some("x86_64"));
}

#[test]
fn merge_rejects_response_identity_mismatch() {
    let error = merge_sparse_entries(
        "fedora",
        "nginx",
        vec![make_entry(
            "curl",
            "fedora",
            vec![make_version("1.0", false)],
        )],
    )
    .expect_err("wrong package identity must fail closed");
    assert!(error.to_string().contains("requested fedora/nginx"));
}

#[test]
fn merge_rejects_conflicting_resolution_or_content_identity() {
    let first = make_version("1.0", true);
    let mut conflicting_metadata = first.clone();
    conflicting_metadata.size += 1;
    let error = merge_sparse_entries(
        "fedora",
        "nginx",
        vec![
            make_entry("nginx", "fedora", vec![first.clone()]),
            make_entry("nginx", "fedora", vec![conflicting_metadata]),
        ],
    )
    .expect_err("resolution metadata conflict must fail closed");
    assert!(error.to_string().contains("resolution metadata"));

    let mut conflicting_content = first.clone();
    conflicting_content.content_hash = Some("sha256:different".to_string());
    let error = merge_sparse_entries(
        "fedora",
        "nginx",
        vec![
            make_entry("nginx", "fedora", vec![first]),
            make_entry("nginx", "fedora", vec![conflicting_content]),
        ],
    )
    .expect_err("content identity conflict must fail closed");
    assert!(error.to_string().contains("content identity"));
}

#[tokio::test]
async fn remote_fetch_rejects_body_identity_that_disagrees_with_url() {
    let app = axum::Router::new().route(
        "/v1/index/fedora/nginx",
        axum::routing::get(|| async {
            (
                [(
                    crate::server::handlers::public_read::PROFILE_REVISION_HEADER,
                    REVISION_A,
                )],
                axum::Json(make_entry(
                    "curl",
                    "fedora",
                    vec![make_version("1.0", false)],
                )),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await });

    let error = fetch_remote_sparse_entry(
        &reqwest::Client::new(),
        &format!("http://{address}"),
        "fedora",
        "nginx",
        REVISION_A,
    )
    .await
    .expect_err("upstream body identity must match its requested URL");
    server.abort();

    assert!(error.to_string().contains("requested fedora/nginx"));
}

#[tokio::test]
async fn test_cache_put_and_get() {
    let cache = FederatedIndexCache::new();
    let entry = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);

    cache
        .put(REVISION_A, "fedora", "nginx", entry.clone())
        .await;

    let cached = cache
        .get(REVISION_A, "fedora", "nginx", Duration::from_secs(60))
        .await;
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().versions.len(), 1);
}

#[tokio::test]
async fn test_cache_miss_no_entry() {
    let cache = FederatedIndexCache::new();

    let cached = cache
        .get(REVISION_A, "fedora", "nginx", Duration::from_secs(60))
        .await;
    assert!(cached.is_none());
}

#[tokio::test]
async fn test_cache_ttl_expiry() {
    let cache = FederatedIndexCache::new();
    let entry = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);

    cache.put(REVISION_A, "fedora", "nginx", entry).await;

    // With a zero TTL, the entry should be considered expired immediately
    let cached = cache
        .get(REVISION_A, "fedora", "nginx", Duration::from_secs(0))
        .await;
    assert!(cached.is_none());
}

#[tokio::test]
async fn test_cache_cleanup() {
    let cache = FederatedIndexCache::new();

    cache
        .put(
            REVISION_A,
            "fedora",
            "nginx",
            make_entry("nginx", "fedora", vec![make_version("1.0", true)]),
        )
        .await;
    cache
        .put(
            REVISION_A,
            "fedora",
            "curl",
            make_entry("curl", "fedora", vec![make_version("8.0", true)]),
        )
        .await;

    assert_eq!(cache.len().await, 2);

    // Cleanup with zero TTL should remove everything
    let removed = cache.cleanup(Duration::from_secs(0)).await;
    assert_eq!(removed, 2);
    assert_eq!(cache.len().await, 0);
}

#[tokio::test]
async fn test_cache_cleanup_preserves_fresh() {
    let cache = FederatedIndexCache::new();

    cache
        .put(
            REVISION_A,
            "fedora",
            "nginx",
            make_entry("nginx", "fedora", vec![make_version("1.0", true)]),
        )
        .await;

    // Cleanup with long TTL should preserve entry
    let removed = cache.cleanup(Duration::from_secs(3600)).await;
    assert_eq!(removed, 0);
    assert_eq!(cache.len().await, 1);
}

#[tokio::test]
async fn test_cache_different_keys() {
    let cache = FederatedIndexCache::new();

    cache
        .put(
            REVISION_A,
            "fedora",
            "nginx",
            make_entry("nginx", "fedora", vec![make_version("1.0", true)]),
        )
        .await;
    cache
        .put(
            REVISION_A,
            "arch",
            "nginx",
            make_entry("nginx", "arch", vec![make_version("2.0", true)]),
        )
        .await;

    let fedora = cache
        .get(REVISION_A, "fedora", "nginx", Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(fedora.distro, "fedora");

    let arch = cache
        .get(REVISION_A, "arch", "nginx", Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(arch.distro, "arch");
}

#[tokio::test]
async fn cache_never_reuses_an_entry_across_profile_revisions() {
    let cache = FederatedIndexCache::new();
    cache
        .put(
            REVISION_A,
            "fedora",
            "nginx",
            make_entry("nginx", "fedora", vec![make_version("1.0", false)]),
        )
        .await;

    assert!(
        cache
            .get(REVISION_B, "fedora", "nginx", Duration::from_secs(60))
            .await
            .is_none()
    );
}
