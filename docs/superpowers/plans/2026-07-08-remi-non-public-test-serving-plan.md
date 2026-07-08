# Remi Non-Public Test Serving Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a default-off Remi admin/test lane that serves blocked and review-required converted CCS artifacts without changing public publication semantics.

**Architecture:** Keep public publication decisions in `apps/remi/src/server/publication.rs` unchanged. Add a runtime config flag and focused admin-only metadata/download handlers in `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`; route them through both admin router assemblies, guarded by the existing admin auth/loopback layers. Use whole-file CCS download for the first slice so `/v1/chunks/{hash}` stays public-gated.

**Tech Stack:** Rust, axum, serde, rusqlite, cargo test, Remi docs-audit checks.

## Global Constraints

- `non_public_test_serving.enabled` defaults to false.
- The test lane must not rewrite `publication_status`.
- Public package, index, sparse, OCI, search, and chunk routes must keep filtering by public-ready state.
- Test-serving metadata must never serialize raw `review_artifact_path`.
- Malformed scriptlet summaries are not test-served in this slice.
- No normal public download analytics for non-public test downloads.

---

## File Structure

- Modify `apps/remi/src/server/config.rs`
  - Add `NonPublicTestServingSection`.
  - Add `non_public_test_serving` to `RemiConfig`.
  - Map the section into `ServerConfig`.
- Modify `apps/remi/src/server/mod.rs`
  - Add `non_public_test_serving` to runtime `ServerConfig`.
- Create `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`
  - Add test-serving query DTO, response DTO, lookup enum, DB lookup helper, metadata handler, download handler, and focused route-level tests.
- Modify `apps/remi/src/server/handlers/admin/mod.rs`
  - Register and re-export the focused non-public test-serving handler module.
- Modify `apps/remi/src/server/routes/admin.rs`
  - Add `test-manifest` and `test-download` routes to internal and external admin routers.
- Modify `docs/modules/remi.md`
  - Document the admin/test lane while preserving public-gate language.
- Modify `docs/SCRIPTLET_SECURITY.md`
  - Document that blocked/review converted artifacts may be test-served through admin Remi only.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Register this spec and plan.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Regenerate after staging the new docs.

## Task 1: Add Default-Off Config Policy

**Files:**
- Modify: `apps/remi/src/server/config.rs`
- Modify: `apps/remi/src/server/mod.rs`

**Interfaces:**
- Produces: `NonPublicTestServingSection { enabled: bool }`
- Produces: `ServerConfig.non_public_test_serving: NonPublicTestServingSection`

- [ ] **Step 1: Write failing config tests**

Add these tests in `apps/remi/src/server/config.rs` near the existing config tests:

```rust
#[test]
fn non_public_test_serving_defaults_disabled() {
    let runtime = RemiConfig::default().to_server_config().unwrap();

    assert!(!runtime.non_public_test_serving.enabled);
}

#[test]
fn non_public_test_serving_can_be_enabled_from_toml() {
    let config: RemiConfig = toml::from_str(
        r#"
        [non_public_test_serving]
        enabled = true
        "#,
    )
    .unwrap();

    let runtime = config.to_server_config().unwrap();

    assert!(runtime.non_public_test_serving.enabled);
}
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p remi non_public_test_serving
```

Expected: FAIL because the runtime config has no `non_public_test_serving` field yet.

- [ ] **Step 3: Add the config section**

In `apps/remi/src/server/config.rs`, add:

```rust
/// Admin/test access for converted artifacts that are not public-ready.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct NonPublicTestServingSection {
    /// Allow admin/test endpoints to serve blocked or review-required conversions.
    #[serde(default)]
    pub enabled: bool,
}
```

Add this field to `RemiConfig`:

```rust
/// Non-public conversion test-serving settings
#[serde(default)]
pub non_public_test_serving: NonPublicTestServingSection,
```

Add this field to `ServerConfig` in `apps/remi/src/server/mod.rs`:

```rust
/// Default-off admin/test access for non-public converted artifacts.
pub non_public_test_serving: crate::server::config::NonPublicTestServingSection,
```

Add this mapping in `RemiConfig::to_server_config()`:

```rust
non_public_test_serving: self.non_public_test_serving.clone(),
```

- [ ] **Step 4: Run the config tests**

Run:

```bash
cargo test -p remi non_public_test_serving
```

Expected: PASS.

## Task 2: Add Admin Test-Serving Lookup And Metadata

**Files:**
- Create: `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`
- Modify: `apps/remi/src/server/handlers/admin/mod.rs`

**Interfaces:**
- Consumes: `ServerConfig.non_public_test_serving.enabled`
- Produces: `get_non_public_test_manifest(...) -> Response`
- Produces: `NonPublicTestManifestResponse`

- [ ] **Step 1: Write failing handler tests**

Add tests in `apps/remi/src/server/handlers/admin/non_public_test_serving.rs` with names containing `non_public_test_serving`:

```rust
#[tokio::test]
async fn non_public_test_serving_manifest_is_disabled_by_default() {
    let (app, db_path) = test_app().await;
    seed_non_public_test_row(&db_path, "blocked", true);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("NON_PUBLIC_TEST_SERVING_DISABLED"));
}

#[tokio::test]
async fn non_public_test_serving_manifest_returns_sanitized_blocked_metadata() {
    let (app, db_path) = test_app_with_non_public_test_serving(true).await;
    seed_non_public_test_row(&db_path, "blocked", true);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();

    assert!(body.contains("\"status\":\"non-public-test-serving\""));
    assert!(body.contains("\"publication_status\":\"blocked\""));
    assert!(body.contains("\"blocked-class-network\""));
    assert!(body.contains("\"review_artifact_available\":true"));
    assert!(!body.contains("review_artifact_path"));
    assert!(!body.contains("private-review-secret"));
}

#[tokio::test]
async fn non_public_test_serving_manifest_rejects_public_rows() {
    let (app, db_path) = test_app_with_non_public_test_serving(true).await;
    seed_non_public_test_row(&db_path, "public", true);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn non_public_test_serving_manifest_rejects_malformed_summary() {
    let (app, db_path) = test_app_with_non_public_test_serving(true).await;
    seed_non_public_test_row(&db_path, "blocked", false);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("MALFORMED_SCRIPTLET_SUMMARY"));
}
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p remi non_public_test_serving_manifest
```

Expected: FAIL because the helper, route, and handler do not exist.

- [ ] **Step 3: Add test helpers**

Add helper functions in the test module:

```rust
fn test_app_with_non_public_test_serving(enabled: bool) -> (axum::Router, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    conary_core::db::init(&db_path).unwrap();

    let config = crate::server::ServerConfig {
        db_path: db_path.clone(),
        chunk_dir: tmp.path().join("chunks"),
        cache_dir: tmp.path().join("cache"),
        non_public_test_serving: crate::server::config::NonPublicTestServingSection { enabled },
        ..Default::default()
    };
    std::fs::create_dir_all(&config.chunk_dir).unwrap();
    std::fs::create_dir_all(&config.cache_dir).unwrap();

    let state = Arc::new(RwLock::new(
        crate::server::ServerState::new(config).expect("test server state"),
    ));
    let app = crate::server::routes::create_external_admin_router(state, None);

    let hash = crate::server::auth::hash_token("test-admin-token-12345");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::models::admin_token::create(&conn, "test-admin", &hash, "admin")
            .unwrap();
    }

    std::mem::forget(tmp);
    (app, db_path)
}

fn seed_non_public_test_row(db_path: &std::path::Path, status: &str, summary_valid: bool) {
    let ccs_path = db_path.parent().unwrap().join("pkg.ccs");
    std::fs::write(&ccs_path, b"non-public ccs").unwrap();
    let conn = crate::server::open_runtime_db(db_path).unwrap();
    let mut converted = conary_core::db::models::ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
        &["abc".to_string()],
        14,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
    );
    converted.package_architecture = Some("x86_64".to_string());
    let mut summary = ScriptletBundleSummary {
        publication_status: status.to_string(),
        scriptlet_fidelity: status.to_string(),
        target_compatibility: status.to_string(),
        blocked_reason_codes: if status == "blocked" {
            vec!["blocked-class-network".to_string()]
        } else {
            Vec::new()
        },
        review_artifact_path: Some("/tmp/private-review-secret.json".to_string()),
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();
    if !summary_valid {
        summary.publication_status = "public".to_string();
        converted.scriptlet_summary_json = serde_json::to_string(&summary).unwrap();
    }
    converted.insert(&conn).unwrap();
}
```

- [ ] **Step 4: Add the handler DTOs and lookup**

Add the query, response, and lookup enum:

```rust
#[derive(Debug, Deserialize)]
pub struct NonPublicTestServingQuery {
    pub version: String,
    pub arch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NonPublicTestManifestResponse {
    pub status: &'static str,
    pub distro: String,
    pub package: String,
    pub version: String,
    pub arch: Option<String>,
    pub public_ready: bool,
    pub ccs: NonPublicTestCcsInfo,
    pub scriptlets: crate::server::publication::PublicationGateReport,
}

#[derive(Debug, Serialize)]
pub struct NonPublicTestCcsInfo {
    pub content_hash: Option<String>,
    pub total_size: u64,
}

enum NonPublicTestLookup {
    Eligible {
        manifest: NonPublicTestManifestResponse,
        ccs_path: PathBuf,
    },
    Disabled,
    PublicReady,
    Malformed(crate::server::publication::PublicationGateReport),
    Stale,
    AmbiguousArchitecture,
    Missing,
}
```

Implement `lookup_non_public_test_package(...)` with these rules:

- disabled config returns `Disabled`;
- empty version returns route-level `INVALID_PARAMETER`;
- stale records return `Stale`;
- no record or missing CCS path/file returns `Missing`;
- invalid summary shape returns `Malformed(report)`;
- public-ready records return `PublicReady`;
- blocked/review records return `Eligible`.

- [ ] **Step 5: Add `get_non_public_test_manifest`**

The handler checks admin scope, supported distro, package path parameter, and
non-empty version, then maps lookup variants to JSON errors:

```rust
pub async fn get_non_public_test_manifest(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, package)): Path<(String, String)>,
    Query(query): Query<NonPublicTestServingQuery>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response
```

Use existing `json_error` for errors:

- `403 NON_PUBLIC_TEST_SERVING_DISABLED`
- `409 ALREADY_PUBLIC`
- `409 STALE_CONVERSION`
- `409 MALFORMED_SCRIPTLET_SUMMARY`
- `409 AMBIGUOUS_ARCHITECTURE`
- `404 NOT_FOUND`

- [ ] **Step 6: Add routes**

In `apps/remi/src/server/routes/admin.rs`, add the route to both admin router
builders:

```rust
.route(
    "/v1/admin/packages/{distro}/{package}/test-manifest",
    get(admin_handlers::get_non_public_test_manifest),
)
```

- [ ] **Step 7: Run the metadata tests**

Run:

```bash
cargo test -p remi non_public_test_serving_manifest
```

Expected: PASS.

## Task 3: Add Admin Whole-File Download

**Files:**
- Modify: `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`
- Modify: `apps/remi/src/server/routes/admin.rs`

**Interfaces:**
- Consumes: `lookup_non_public_test_package(...)`
- Produces: `download_non_public_test_package(...) -> Response`

- [ ] **Step 1: Write the failing download test**

Add:

```rust
#[tokio::test]
async fn non_public_test_serving_download_streams_blocked_ccs_bytes() {
    let (app, db_path) = test_app_with_non_public_test_serving(true).await;
    seed_non_public_test_row(&db_path, "blocked", true);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/admin/packages/fedora/pkg/test-download?version=1.0&arch=x86_64")
                .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"non-public ccs");
}
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p remi non_public_test_serving_download_streams_blocked_ccs_bytes
```

Expected: FAIL because the download route is missing.

- [ ] **Step 3: Add the download handler**

Add:

```rust
pub async fn download_non_public_test_package(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, package)): Path<(String, String)>,
    Query(query): Query<NonPublicTestServingQuery>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response
```

Reuse the metadata lookup. For `Eligible`, stream the `ccs_path` with
`Content-Type: application/octet-stream`, sanitized `Content-Disposition`, and
`Cache-Control: no-store`. Do not call `AnalyticsRecorder::record`.

- [ ] **Step 4: Add the route**

In both admin router builders:

```rust
.route(
    "/v1/admin/packages/{distro}/{package}/test-download",
    get(admin_handlers::download_non_public_test_package),
)
```

- [ ] **Step 5: Run the download test**

Run:

```bash
cargo test -p remi non_public_test_serving_download
```

Expected: PASS.

## Task 4: Pin Public Gate Preservation

**Files:**
- Modify: `apps/remi/src/server/handlers/packages.rs`
- Modify: `apps/remi/src/server/handlers/chunks.rs` only if a targeted test needs a clearer name; do not change behavior.

**Interfaces:**
- Consumes: existing public `converted_ccs_path_for_download(...)`
- Consumes: existing public chunk gate

- [ ] **Step 1: Run existing public refusal tests before code changes**

Run:

```bash
cargo test -p remi converted_download_lookup_refuses_blocked_rows
cargo test -p remi get_chunk_returns_not_found_for_non_public_only_hash
```

Expected: PASS before and after this task. If names changed, use `cargo test -p remi publication` plus the exact chunk test name from `handlers/chunks.rs`.

- [ ] **Step 2: Add no production behavior here**

This task is a guard task. If Task 2 or 3 changed public package or chunk code,
move that change back to the admin handler.

- [ ] **Step 3: Run publication proof**

Run:

```bash
cargo test -p remi publication
```

Expected: PASS.

## Task 5: Document The Test Lane

**Files:**
- Modify: `docs/modules/remi.md`
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

**Interfaces:**
- Consumes: routes `/v1/admin/packages/{distro}/{package}/test-manifest` and `/test-download`
- Produces: docs that distinguish public publication from admin/test serving

- [ ] **Step 1: Update Remi docs**

Add a short section to `docs/modules/remi.md` near the publication-gate text:

```markdown
### Non-Public Test Serving

Remi also has a default-off admin/test lane for converted artifacts that are
blocked or private-review. When `non_public_test_serving.enabled = true`, an
admin can request `/v1/admin/packages/{distro}/{package}/test-manifest` or
`/v1/admin/packages/{distro}/{package}/test-download` with an exact version and
optional architecture (`arch`, or `architecture` as an alias). This does not
change public publication status, public
indexes, OCI tags, sparse indexes, or public chunk serving. Malformed rows are
not test-served; they need reconversion or metadata repair first.
```

- [ ] **Step 2: Update scriptlet security docs**

Add one paragraph to `docs/SCRIPTLET_SECURITY.md`:

```markdown
Maintainers may use Remi's non-public admin/test serving lane to fetch blocked
or review-required converted CCS files for inspection. That lane is disabled by
default, requires admin access, and preserves the original scriptlet publication
status. It is not public publication authority and does not permit raw legacy
scriptlet replay.
```

- [ ] **Step 3: Register docs**

Append ledger rows for:

- `docs/superpowers/specs/2026-07-08-remi-non-public-test-serving-design.md`
- `docs/superpowers/plans/2026-07-08-remi-non-public-test-serving-plan.md`

Regenerate inventory after staging the new files:

```bash
git add docs/superpowers/specs/2026-07-08-remi-non-public-test-serving-design.md docs/superpowers/plans/2026-07-08-remi-non-public-test-serving-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
```

Expected: any diff only adds the two new planning rows. Apply that diff to the tracked inventory.

## Task 6: Verification

**Files:**
- No new source files.

**Interfaces:**
- Consumes: all task outputs.

- [ ] **Step 1: Run focused Remi tests**

Run:

```bash
cargo test -p remi non_public_test_serving
cargo test -p remi publication
```

Expected: PASS.

- [ ] **Step 2: Run package/chunk public gate proof**

Run:

```bash
cargo test -p remi converted_download_lookup_refuses_blocked_rows
cargo test -p remi get_chunk_returns_not_found_for_non_public_only_hash
```

Expected: PASS.

- [ ] **Step 3: Run docs proof**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.
