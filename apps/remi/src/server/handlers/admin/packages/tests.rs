// remi/src/server/handlers/admin/packages/tests.rs

use crate::server::handlers::admin::test_helpers::{rebuild_app, test_app};
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};
use flate2::Compression;
use flate2::write::GzEncoder;
use tar::Builder;
use tower::ServiceExt;

fn minimal_ccs(name: &str, version: &str) -> Vec<u8> {
    let manifest = conary_core::ccs::manifest::CcsManifest::new_minimal(name, version);
    let manifest_toml = toml::to_string(&manifest).expect("serialize manifest");
    let component = serde_json::json!({
        "name": "runtime",
        "files": [],
        "hash": "empty",
        "size": 0
    })
    .to_string();

    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = Builder::new(encoder);

    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(manifest_toml.len() as u64);
    header.set_cksum();
    tar.append_data(&mut header, "MANIFEST.toml", manifest_toml.as_bytes())
        .expect("write manifest");

    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(component.len() as u64);
    header.set_cksum();
    tar.append_data(&mut header, "components/runtime.json", component.as_bytes())
        .expect("write component");

    tar.into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip")
}

async fn assert_status(response: axum::response::Response, expected: StatusCode) {
    assert_status_named("response", response, expected).await;
}

async fn assert_status_named(
    label: &str,
    response: axum::response::Response,
    expected: StatusCode,
) {
    let status = response.status();
    if status != expected {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        panic!(
            "{label}: unexpected status {status}, expected {expected}; body: {}",
            String::from_utf8_lossy(&body)
        );
    }
}

#[tokio::test]
async fn test_upload_package_registers_converted_record() {
    let (app, db_path) = test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/fedora")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::from(minimal_ccs("fixture-demo", "1.0.0")))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_status_named("fedora upload", response, StatusCode::CREATED).await;

    let conn = conary_core::db::open(&db_path).unwrap();
    let found = conary_core::db::models::ConvertedPackage::find_by_package_identity(
        &conn,
        "fedora",
        "fixture-demo",
        Some("1.0.0"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(found.original_format, "ccs");
    assert_eq!(
        found.total_size,
        Some(minimal_ccs("fixture-demo", "1.0.0").len() as i64)
    );
}

#[tokio::test]
async fn test_upload_package_allows_same_fixture_for_multiple_distros() {
    let (app, db_path) = test_app().await;
    let body = minimal_ccs("fixture-demo", "1.0.0");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/fedora")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_status_named("fedora upload", response, StatusCode::CREATED).await;

    let app2 = rebuild_app(&db_path);
    let response = app2
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/ubuntu")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_status_named("ubuntu upload", response, StatusCode::CREATED).await;

    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(
        conary_core::db::models::ConvertedPackage::find_by_package_identity(
            &conn,
            "fedora",
            "fixture-demo",
            Some("1.0.0"),
        )
        .unwrap()
        .is_some()
    );
    assert!(
        conary_core::db::models::ConvertedPackage::find_by_package_identity(
            &conn,
            "ubuntu",
            "fixture-demo",
            Some("1.0.0"),
        )
        .unwrap()
        .is_some()
    );
}

#[tokio::test]
async fn test_upload_package_rejects_unauthenticated() {
    let (app, _db_path) = test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/fedora")
                .body(Body::from(minimal_ccs("fixture-demo", "1.0.0")))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_package_upload_rejects_unsupported_distro_before_cache_paths() {
    let (app, db_path) = test_app().await;
    let cache_dir = db_path.parent().unwrap().join("cache");
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/debian")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        !cache_dir.join("packages").join("debian").exists(),
        "unsupported distro route must not create cache/packages/debian"
    );
}

#[tokio::test]
async fn admin_review_artifact_requires_admin_scope() {
    let (app, _db_path) = test_app().await;

    let response = tower::ServiceExt::oneshot(
        app,
        Request::builder()
            .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_review_artifact_rejects_paths_outside_review_root() {
    let (app, db_path) = test_app().await;
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &["abc".to_string()],
        3,
        "sha256:content".to_string(),
        "/tmp/pkg.ccs".to_string(),
    );
    let mut summary = ScriptletBundleSummary {
        publication_status: "private-review".to_string(),
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        review_reason_codes: vec!["review-class-debconf".to_string()],
        ..Default::default()
    };
    summary.review_artifact_path = Some("/etc/passwd".to_string());
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.insert(&conn).unwrap();

    let response = tower::ServiceExt::oneshot(
        app,
        Request::builder()
            .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0")
            .header("Authorization", "Bearer test-admin-token-12345")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_upload_with_blocked_bundle_stores_non_public_metadata() {
    let (app, db_path) = test_app().await;
    let archive = blocked_scriptlet_ccs_fixture();

    let response = tower::ServiceExt::oneshot(
        app,
        Request::builder()
            .method(Method::POST)
            .uri("/v1/admin/packages/fedora")
            .header("Authorization", "Bearer test-admin-token-12345")
            .body(Body::from(archive))
            .unwrap(),
    )
    .await
    .unwrap();

    assert_status(response, StatusCode::CREATED).await;

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let converted = ConvertedPackage::find_by_package_identity(
        &conn,
        "fedora",
        "blocked-scriptlet-fixture",
        Some("1.0"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(converted.publication_status, "blocked");
    assert!(converted.review_artifact_path.is_some());
    let expected_digest = conary_core::hash::sha256_prefixed(b"fixture-evidence");
    assert_eq!(
        converted.evidence_digest.as_deref(),
        Some(expected_digest.as_str())
    );
    let artifact_path = std::path::PathBuf::from(converted.review_artifact_path.unwrap());
    let artifact: serde_json::Value =
        serde_json::from_slice(&std::fs::read(artifact_path).unwrap()).unwrap();
    assert_eq!(artifact["schema"], "conary.remi.scriptlet-review.v2");
    assert_eq!(
        artifact["publication"]["evidence_digest"].as_str(),
        Some(expected_digest.as_str())
    );
    assert!(
        !serde_json::to_string(&artifact)
            .unwrap()
            .contains("review_artifact_path")
    );
}

#[tokio::test]
async fn admin_upload_recomputes_stale_public_high_risk_file_capability_bundle() {
    let (app, db_path) = test_app().await;
    let archive = stale_public_high_risk_file_capability_ccs_fixture();

    let response = tower::ServiceExt::oneshot(
        app,
        Request::builder()
            .method(Method::POST)
            .uri("/v1/admin/packages/fedora")
            .header("Authorization", "Bearer test-admin-token-12345")
            .body(Body::from(archive))
            .unwrap(),
    )
    .await
    .unwrap();

    assert_status(response, StatusCode::CREATED).await;

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let converted = ConvertedPackage::find_by_package_identity(
        &conn,
        "fedora",
        "stale-public-file-capability-fixture",
        Some("1.0"),
    )
    .unwrap()
    .unwrap();

    assert_eq!(converted.scriptlet_fidelity, "fully-replaced");
    assert_eq!(converted.target_compatibility, "conary-portable");
    assert_eq!(converted.publication_status, "private-review");
    assert!(converted.review_artifact_path.is_some());

    let summary = converted.scriptlet_summary();
    assert_eq!(
        summary.review_reason_codes,
        vec!["public-policy-file-capability-private-review".to_string()]
    );

    let artifact_path = std::path::PathBuf::from(converted.review_artifact_path.unwrap());
    let artifact: serde_json::Value =
        serde_json::from_slice(&std::fs::read(artifact_path).unwrap()).unwrap();
    assert_eq!(
        artifact["publication"]["publication_status"].as_str(),
        Some("private-review")
    );
    assert_eq!(
        artifact["publication"]["review_reason_codes"]
            .as_array()
            .and_then(|codes| codes.first())
            .and_then(serde_json::Value::as_str),
        Some("public-policy-file-capability-private-review")
    );
}

#[tokio::test]
async fn admin_review_artifact_lookup_is_arch_specific_and_reports_stale_rows() {
    let (app, db_path) = test_app().await;
    seed_review_artifact_row(
        &db_path,
        "pkg",
        "1.0",
        Some("x86_64"),
        "current.json",
        false,
    );
    seed_review_artifact_row(&db_path, "pkg", "1.0", Some("aarch64"), "stale.json", true);

    let current = tower::ServiceExt::oneshot(
        app.clone(),
        Request::builder()
            .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0&arch=x86_64")
            .header("Authorization", "Bearer test-admin-token-12345")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(current.status(), StatusCode::OK);

    let stale = tower::ServiceExt::oneshot(
        app,
        Request::builder()
            .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0&arch=aarch64")
            .header("Authorization", "Bearer test-admin-token-12345")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
}

fn blocked_scriptlet_ccs_fixture() -> Vec<u8> {
    use conary_core::ccs::builder::{CcsBuilder, write_ccs_package};
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, ForeignReplayPolicy, LegacyScriptletBundle, PublicationPolicy,
        PublicationStatus, ScriptletFidelity, SourceFormat, TargetCompatibility, VersionScheme,
    };

    let temp = tempfile::tempdir().unwrap();
    let mut manifest =
        conary_core::ccs::manifest::CcsManifest::new_minimal("blocked-scriptlet-fixture", "1.0");
    manifest.legacy_scriptlets = Some(LegacyScriptletBundle {
        schema: conary_core::ccs::legacy_scriptlets::LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 2,
        source_format: SourceFormat::Rpm,
        source_family: "rpm".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: None,
        source_arch: Some("x86_64".to_string()),
        source_package: "blocked-scriptlet-fixture".to_string(),
        source_version: "1.0".to_string(),
        source_checksum: Some(conary_core::hash::sha256_prefixed(b"fixture-source")),
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "test".to_string(),
        conversion_policy: "publication-gate-test".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: Some(conary_core::hash::sha256_prefixed(b"fixture-evidence")),
        target_compatibility: TargetCompatibility::Blocked,
        allowed_targets: Vec::new(),
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::Blocked,
        publication_status: PublicationStatus::Blocked,
        scriptlet_fidelity: ScriptletFidelity::Blocked,
        decision_counts: DecisionCounts::default(),
        unsupported_class_counts: std::collections::BTreeMap::new(),
        security_policy_intents: Vec::new(),
        entries: Vec::new(),
        extra: std::collections::BTreeMap::new(),
    });

    std::fs::write(temp.path().join("payload.txt"), b"fixture").unwrap();
    let path = temp.path().join("blocked.ccs");
    let result = CcsBuilder::new(manifest, temp.path())
        .build()
        .expect("fixture build");
    write_ccs_package(&result, &path).expect("fixture CCS package");
    std::fs::read(path).expect("fixture bytes")
}

fn stale_public_high_risk_file_capability_ccs_fixture() -> Vec<u8> {
    use conary_core::ccs::builder::{CcsBuilder, write_ccs_package};
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, EffectConfidence, EffectReplacement, EffectSource, ForeignReplayPolicy,
        LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle, LegacyScriptletEntry, LifecyclePath,
        NativeInvocation, PublicationPolicy, PublicationStatus, ScriptletDecision, ScriptletEffect,
        ScriptletFidelity, SourceFormat, TargetCompatibility, TransactionOrder, VersionScheme,
    };
    use std::collections::BTreeMap;

    let temp = tempfile::tempdir().unwrap();
    let capability = "cap_sys_admin";
    let body = format!("setcap {capability}=+ep /usr/bin/test\n");
    let mut effect_extra = BTreeMap::new();
    effect_extra.insert(
        "capabilities".to_string(),
        toml::Value::Array(vec![toml::Value::String(capability.to_string())]),
    );

    let mut manifest = conary_core::ccs::manifest::CcsManifest::new_minimal(
        "stale-public-file-capability-fixture",
        "1.0",
    );
    manifest.legacy_scriptlets = Some(LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 2,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: "stale-public-file-capability-fixture".to_string(),
        source_version: "1.0".to_string(),
        source_checksum: Some(conary_core::hash::sha256_prefixed(
            b"stale-public-file-capability-source",
        )),
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "stale-converter".to_string(),
        conversion_tool_version: "0.0.0".to_string(),
        conversion_policy: "stale-public-policy".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            b"stale-public-file-capability-evidence",
        )),
        target_compatibility: TargetCompatibility::ConaryPortable,
        allowed_targets: Vec::new(),
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::PublicIfNoBlocked,
        publication_status: PublicationStatus::Public,
        scriptlet_fidelity: ScriptletFidelity::FullyReplaced,
        decision_counts: DecisionCounts {
            replaced: 1,
            ..DecisionCounts::default()
        },
        unsupported_class_counts: BTreeMap::new(),
        security_policy_intents: Vec::new(),
        entries: vec![LegacyScriptletEntry {
            id: "scriptlet:0:post-install".to_string(),
            native_slot: "%post".to_string(),
            phase: LifecyclePath::PostInstall,
            lifecycle_paths: vec!["post-install".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
            body,
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                ..TransactionOrder::default()
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            decision: ScriptletDecision::Replaced,
            reason_code: "helper-complete-file-capability".to_string(),
            human_reason: None,
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                b"stale-public-file-capability-entry",
            )),
            source_evidence_refs: Vec::new(),
            effects: vec![ScriptletEffect {
                kind: "file-capability".to_string(),
                source: EffectSource::ShellAst,
                confidence: EffectConfidence::Declared,
                replacement: EffectReplacement::Complete,
                adapter_id: Some("file-capability/v1".to_string()),
                adapter_digest: None,
                command: Some("setcap".to_string()),
                args: vec![format!("{capability}=+ep"), "/usr/bin/test".to_string()],
                path: Some("/usr/bin/test".to_string()),
                reason_code: Some("helper-complete-file-capability".to_string()),
                extra: effect_extra,
            }],
            unknown_command_evidence: Vec::new(),
            blocked_classes: Vec::new(),
            boot_security_intents: Vec::new(),
            security_policy_intents: Vec::new(),
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }],
        extra: BTreeMap::new(),
    });

    std::fs::write(temp.path().join("payload.txt"), b"fixture").unwrap();
    let path = temp.path().join("stale-public-file-capability.ccs");
    let result = CcsBuilder::new(manifest, temp.path())
        .build()
        .expect("fixture build");
    write_ccs_package(&result, &path).expect("fixture CCS package");
    std::fs::read(path).expect("fixture bytes")
}

fn seed_review_artifact_row(
    db_path: &std::path::Path,
    package: &str,
    version: &str,
    architecture: Option<&str>,
    artifact_name: &str,
    stale: bool,
) {
    let cache_dir = db_path.parent().unwrap().join("cache");
    let artifact_dir = crate::server::publication::review_artifact_root(&cache_dir)
        .join("fedora")
        .join(package)
        .join(version)
        .join(architecture.unwrap_or("noarch"));
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let artifact_path = artifact_dir.join(artifact_name);
    std::fs::write(
        &artifact_path,
        serde_json::json!({
            "schema": "conary.remi.scriptlet-review.v2",
            "package": package,
            "version": version,
            "architecture": architecture,
        })
        .to_string(),
    )
    .unwrap();

    let conn = rusqlite::Connection::open(db_path).unwrap();
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        package.to_string(),
        version.to_string(),
        "rpm".to_string(),
        format!("sha256:source-{package}-{version}-{artifact_name}"),
        &["abc".to_string()],
        3,
        format!("sha256:content-{package}-{version}-{artifact_name}"),
        format!("/tmp/{package}-{version}-{artifact_name}.ccs"),
    );
    converted.package_architecture = architecture.map(str::to_string);
    if stale {
        converted.conversion_version = CONVERSION_VERSION - 1;
    }
    let mut summary = ScriptletBundleSummary {
        publication_status: "private-review".to_string(),
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        review_reason_codes: vec!["review-class-debconf".to_string()],
        ..Default::default()
    };
    summary.review_artifact_path = Some(artifact_path.to_string_lossy().to_string());
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.insert(&conn).unwrap();
}
