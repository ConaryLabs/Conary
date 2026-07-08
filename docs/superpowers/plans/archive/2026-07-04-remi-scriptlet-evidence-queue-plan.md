# Remi Scriptlet Evidence Queue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the admin/operator-only Remi scriptlet evidence queue from the 2026-07-03 design. Remi should aggregate blocked and review-required scriptlet conversion evidence into stable adapter-candidate clusters, let maintainers triage them, and export sanitized adapter packets without changing publication authority.

**Architecture:** Keep publication safety in `apps/remi/src/server/publication.rs` and conversion persistence in `apps/remi/src/server/conversion/persistence.rs`. Add a Remi-owned `scriptlet_evidence_queue` module for normalization, aggregation, state, backfill, and packet export. Add only schema and row-model plumbing to `conary-core` because Remi uses the shared SQLite migration path. Admin handlers expose the queue behind existing bearer-token admin auth.

**Tech Stack:** Rust, Axum, rusqlite, serde JSON, SQLite migrations, existing Remi admin auth/audit middleware, existing legacy scriptlet summaries, cargo test, docs audit scripts.

---

## Non-Negotiable Boundaries

- The queue is not publication authority. `publication_status`, `classify_converted_package`, support-matrix proof, and adapter-backed conversion remain the only path to public-ready converted artifacts.
- Do not expose raw scriptlet bodies, private local paths, environment values, or maintainer-only notes through public routes.
- Do not run LLM calls in this plan. LLM summaries need a child design for provider config, key storage, billing limits, egress controls, retries, and storage policy.
- Do not add a public tracker in this plan. The only public-shaped output is an explicit sanitized packet mode on an admin endpoint.
- Do not run a full evidence backfill at Remi startup.
- If incremental queue recording fails after a conversion row is inserted, log it and rely on backfill for repair. Queue failures must not make a blocked package public or turn a public package blocked.

## Current Baseline

Already landed repo facts this plan relies on:

- `apps/remi/src/server/publication.rs` builds `PublicationGateReport` and private `ScriptletReviewArtifact` files under `scriptlet-review`.
- `apps/remi/src/server/conversion/persistence.rs` stores passive scriptlet metadata and review artifact paths on `converted_packages`.
- `apps/remi/src/server/handlers/admin/packages.rs` exposes an admin-only review artifact lookup and validates that artifact paths stay under the review root.
- `crates/conary-core/src/ccs/convert/effects.rs` carries sanitized command evidence for review/blocked classifications.
- `crates/conary-core/src/ccs/legacy_scriptlets.rs` and `ScriptletBundleSummary` carry `boot_security_intents`, which are the stable v1 command-shape evidence for kernel/initramfs/bootloader/SELinux classes.
- `apps/remi/src/server/scriptlet_corpus.rs` is scan-only planning evidence and must not become stable cluster authority in this plan.
- `crates/conary-core/src/ccs/convert/support_matrix.rs` can provide current class/adapter rows for packet context.

## File Structure

- Modify `crates/conary-core/src/db/schema.rs`
  - Bump `SCHEMA_VERSION` from 74 to 75.
  - Route migration version 75 to `migrations::migrate_v75`.
- Modify `crates/conary-core/src/db/migrations/v41_current.rs`
  - Add queue tables, indexes, and migration tests.
- Add `crates/conary-core/src/db/models/scriptlet_evidence.rs`
  - Own database row structs and small SQL helpers for clusters, samples, state events, notes, and backfill runs.
- Modify `crates/conary-core/src/db/models/mod.rs`
  - Export the scriptlet evidence model module.
- Add `apps/remi/src/server/scriptlet_evidence_queue/mod.rs`
  - Public Remi-owned queue facade.
- Add `apps/remi/src/server/scriptlet_evidence_queue/types.rs`
  - Request/response DTOs, cluster keys, packet DTOs, and state enum.
- Add `apps/remi/src/server/scriptlet_evidence_queue/normalization.rs`
  - Stable cluster-key tuple generation and v1 command-shape normalization.
- Add `apps/remi/src/server/scriptlet_evidence_queue/aggregation.rs`
  - Convert `ConvertedPackage` rows into one or more queue samples.
- Add `apps/remi/src/server/scriptlet_evidence_queue/backfill.rs`
  - Operator-triggered, batched, retryable backfill.
- Add `apps/remi/src/server/scriptlet_evidence_queue/storage.rs`
  - Remi-level DB orchestration over the core row helpers.
- Add `apps/remi/src/server/scriptlet_evidence_queue/packet.rs`
  - Adapter packet export with private and public-sanitized modes.
- Modify `apps/remi/src/server/mod.rs`
  - Register the new module.
- Add `apps/remi/src/server/handlers/admin/scriptlet_evidence.rs`
  - Admin list/detail/backfill/state/notes/packet handlers.
- Modify `apps/remi/src/server/handlers/admin/mod.rs`
  - Export the new admin handlers.
- Modify `apps/remi/src/server/routes/admin.rs`
  - Add external admin routes behind existing bearer-token middleware.
- Modify `apps/remi/src/server/audit.rs`
  - Map `/v1/admin/scriptlet-evidence/*` to `scriptlet.evidence.*` audit actions.
- Modify `apps/remi/src/server/conversion/persistence.rs`
  - Best-effort incremental queue recording after new converted rows are inserted.
- Modify `apps/remi/src/server/handlers/openapi.rs`
  - Add admin route documentation for the new endpoints.
- Modify `docs/modules/remi.md`
  - Document the queue as admin-only adapter planning evidence.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Register this implementation plan and later implementation doc changes.
- Modify `docs/superpowers/feature-coherency-ledger.tsv` during implementation
  - Add a row for the new admin evidence routes once routes exist.

## Data Model

Migration 75 should create these tables:

```sql
CREATE TABLE scriptlet_evidence_clusters (
    cluster_key TEXT PRIMARY KEY,
    schema_version INTEGER NOT NULL,
    distro TEXT NOT NULL,
    target_profile TEXT NOT NULL,
    blocked_class TEXT NOT NULL,
    command TEXT NOT NULL,
    normalized_command_shape TEXT NOT NULL,
    normalized_command_shape_hash TEXT NOT NULL,
    lifecycle_phase TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'needs-triage'
        CHECK (state IN (
            'needs-triage',
            'adapter-candidate',
            'in-design',
            'in-implementation',
            'covered-partial',
            'covered-public-ready',
            'wont-support'
        )),
    first_seen TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_seen TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE scriptlet_evidence_cluster_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cluster_key TEXT NOT NULL REFERENCES scriptlet_evidence_clusters(cluster_key) ON DELETE CASCADE,
    converted_package_id INTEGER REFERENCES converted_packages(id) ON DELETE SET NULL,
    original_checksum TEXT NOT NULL,
    distro TEXT NOT NULL,
    package_name TEXT NOT NULL,
    package_version TEXT NOT NULL,
    package_architecture TEXT,
    publication_status TEXT NOT NULL,
    scriptlet_fidelity TEXT NOT NULL,
    target_compatibility TEXT NOT NULL,
    reason_codes_json TEXT NOT NULL,
    blocked_classes_json TEXT NOT NULL,
    boot_security_intents_json TEXT NOT NULL,
    review_artifact_path TEXT,
    review_artifact_stale INTEGER NOT NULL DEFAULT 0,
    evidence_digest TEXT,
    curation_evidence_digest TEXT,
    observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE scriptlet_evidence_state_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cluster_key TEXT NOT NULL REFERENCES scriptlet_evidence_clusters(cluster_key) ON DELETE CASCADE,
    from_state TEXT,
    to_state TEXT NOT NULL,
    actor TEXT NOT NULL,
    reason TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE scriptlet_evidence_notes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cluster_key TEXT NOT NULL REFERENCES scriptlet_evidence_clusters(cluster_key) ON DELETE CASCADE,
    actor TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE scriptlet_evidence_backfill_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    status TEXT NOT NULL CHECK (status IN ('running', 'complete', 'failed')),
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    completed_at TEXT,
    last_converted_package_id INTEGER NOT NULL DEFAULT 0,
    scanned_count INTEGER NOT NULL DEFAULT 0,
    clustered_count INTEGER NOT NULL DEFAULT 0,
    error_message TEXT
);
```

Use indexes:

- `idx_scriptlet_evidence_clusters_state_last_seen` on `(state, last_seen DESC)`.
- `idx_scriptlet_evidence_clusters_class` on `(blocked_class, command)`.
- `idx_scriptlet_evidence_samples_cluster` on `(cluster_key, observed_at DESC)`.
- `idx_scriptlet_evidence_samples_package` on `(distro, package_name, package_version, package_architecture)`.
- unique expression index `idx_scriptlet_evidence_samples_unique_observation` on `(cluster_key, original_checksum, package_name, package_version, COALESCE(package_architecture, ''))`.
- `idx_scriptlet_evidence_backfill_status` on `(status, updated_at DESC)`.

Create the sample uniqueness rule as an expression index:

```sql
CREATE UNIQUE INDEX idx_scriptlet_evidence_samples_unique_observation
    ON scriptlet_evidence_cluster_samples(
        cluster_key,
        original_checksum,
        package_name,
        package_version,
        COALESCE(package_architecture, '')
    );
```

## Stable Cluster Key

Use schema version `1` for cluster-key semantics, independent from database schema version 75.

The v1 key tuple is exactly:

```text
schema_version
distro
target_profile_or_unknown
blocked_class
command
normalized_command_shape_hash
lifecycle_phase_or_unknown
```

Derive `target_profile_or_unknown` from `supported_profiles::route_by_slug(distro)` only when the route maps to exactly one public profile. Today that produces `fedora-44`, `ubuntu-26.04`, or `arch` for the supported Remi route slugs. Otherwise use `unknown`.

Compute:

```text
normalized_command_shape = "<command> <normalized argv>"
normalized_command_shape_hash = sha256_hex(normalized_command_shape)
cluster_key = "s1-" + sha256_hex(canonical_json_tuple)
```

The key must be URL-safe because it is a path parameter. Do not use `sha256:` prefixes in `cluster_key`.

## V1 Evidence Extraction

For each non-public or malformed `ConvertedPackage` row, create zero or more cluster samples:

1. For each `boot_security_intents` entry, create a command-shape cluster with:
   - `blocked_class = intent.class_id`
   - `command = intent.command`
   - `normalized_command_shape = command + normalized intent.argv`
   - `lifecycle_phase = intent.phase.unwrap_or("unknown")`
2. For each `unknown_commands` entry when there are no boot/security intents for that row, create a fallback cluster with:
   - `blocked_class = "unknown-command"`
   - `command = unknown command`
   - `normalized_command_shape = unknown command`
   - `lifecycle_phase = "unknown"`
3. For blocked/review classes without command-shape evidence, create a deterministic class-level fallback cluster with:
   - `blocked_class = class id`
   - `command = "unknown"`
   - `normalized_command_shape = "<class>:<reason-codes>"`
   - `lifecycle_phase = "unknown"`
4. For malformed summary JSON with no usable class data, create a malformed-metadata cluster with:
   - `blocked_class = "malformed-scriptlet-summary"`
   - `command = "unknown"`
   - `normalized_command_shape = "malformed-scriptlet-summary"`
   - `lifecycle_phase = "unknown"`

This means v1 is immediately useful for kernel/initramfs/bootloader/SELinux evidence because those intents already have sanitized command shapes. It still keeps other blocked/review rows visible as class-level work items until a later design projects generic sanitized command evidence for every class.

## Normalization Rules

`normalization.rs` must re-sanitize every command shape before hashing or exporting, even if the input came from `boot_security_intents`.

Rules:

- Replace kernel-version-like path segments with `<kver>`.
- Replace `/boot/` prefixes with `<boot>/`.
- Replace `$VAR` and `${VAR}` tokens with `<env>`.
- Drop raw environment assignment values such as `FOO=/private/path`; keep only an `<env-assignment>` marker when the assignment affects command shape.
- Preserve command names and option names.
- Preserve approved boot/security paths:
  - `<boot>/<path>`
  - `/lib/modules/<kver>/<path>`
  - `/usr/lib/modules/<kver>/<path>`
  - `/etc/selinux/<path>`
  - `/usr/share/selinux/<path>`
- Replace other absolute paths with `<path>` unless they are already known package payload paths from a later, explicit evidence source.
- Collapse repeated whitespace.
- Sort reason-code fallbacks before building class-level fallback shapes.

## Admin Routes

Add these routes to the external admin router only, behind the existing bearer-token auth middleware. Do not mount them on the public router. Do not treat localhost by itself as authorization for this queue.

- `GET /v1/admin/scriptlet-evidence/clusters`
  - Query: `state`, `distro`, `blocked_class`, `command`, `package`, `limit`, `offset`.
  - Requires `Scope::Admin` for v1.
  - Returns clusters sorted by `last_seen DESC`, with attempt count, unique package count, architecture set, and stale sample count computed from samples.
- `GET /v1/admin/scriptlet-evidence/clusters/{cluster_key}`
  - Requires `Scope::Admin`.
  - Returns cluster detail, sample summaries, state events, and notes.
- `POST /v1/admin/scriptlet-evidence/backfill`
  - Requires `Scope::Admin`.
  - Body: `{ "limit": 500 }`, default limit 500, max limit 5000.
  - Runs one batch and returns run progress. Operators can repeat until status is `complete`.
- `PUT /v1/admin/scriptlet-evidence/clusters/{cluster_key}/state`
  - Requires `Scope::Admin`.
  - Body: `{ "state": "adapter-candidate", "reason": "repeated dracut shape" }`.
  - Updates cluster state and appends a state event.
- `POST /v1/admin/scriptlet-evidence/clusters/{cluster_key}/notes`
  - Requires `Scope::Admin`.
  - Body: `{ "body": "Check Fedora kernel package fixture first." }`.
  - Stores maintainer-only notes.
- `GET /v1/admin/scriptlet-evidence/clusters/{cluster_key}/packet`
  - Query: `visibility=private` or `visibility=public-sanitized`; default `private`.
  - Returns an adapter packet. Private mode may include validated review artifact identifiers. Public-sanitized mode omits review artifact paths/ids and maintainer notes.

Use the existing `check_scope`, `validate_path_param`, `TokenScopes`, and `TokenName` patterns. Since v1 stays operator-only, do not add new token scopes in this plan.

## Task 1: Add Schema And Core Row Helpers

**Files:**
- Modify `crates/conary-core/src/db/schema.rs`
- Modify `crates/conary-core/src/db/migrations/v41_current.rs`
- Add `crates/conary-core/src/db/models/scriptlet_evidence.rs`
- Modify `crates/conary-core/src/db/models/mod.rs`

- [ ] **Step 1: Write failing migration tests**

Add tests near the v74 migration tests in `v41_current.rs`:

```rust
#[test]
fn test_migrate_v75_adds_scriptlet_evidence_queue_tables() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();

    for table in [
        "scriptlet_evidence_clusters",
        "scriptlet_evidence_cluster_samples",
        "scriptlet_evidence_state_events",
        "scriptlet_evidence_notes",
        "scriptlet_evidence_backfill_runs",
    ] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "missing table {table}");
    }
}

#[test]
fn test_scriptlet_evidence_cluster_state_is_constrained() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();

    conn.execute(
        "INSERT INTO scriptlet_evidence_clusters (
            cluster_key, schema_version, distro, target_profile, blocked_class,
            command, normalized_command_shape, normalized_command_shape_hash,
            lifecycle_phase, state
        ) VALUES (
            's1-good', 1, 'fedora', 'fedora-44', 'initramfs',
            'dracut', 'dracut --force <boot>/initramfs.img', 'abc',
            'postinstall', 'needs-triage'
        )",
        [],
    )
    .unwrap();

    let bad = conn.execute(
        "INSERT INTO scriptlet_evidence_clusters (
            cluster_key, schema_version, distro, target_profile, blocked_class,
            command, normalized_command_shape, normalized_command_shape_hash,
            lifecycle_phase, state
        ) VALUES (
            's1-bad', 1, 'fedora', 'fedora-44', 'initramfs',
            'dracut', 'dracut', 'def', 'postinstall', 'public-ready'
        )",
        [],
    );
    assert!(bad.is_err());
}
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core test_migrate_v75_adds_scriptlet_evidence_queue_tables
cargo test -p conary-core test_scriptlet_evidence_cluster_state_is_constrained
```

Expected: FAIL until schema version 75 and the migration exist.

- [ ] **Step 3: Implement migration 75**

Update `SCHEMA_VERSION`, the migration match arm, and `migrate_v75`. Use `strftime('%Y-%m-%dT%H:%M:%SZ', 'now')` for new queue timestamps to match recent Remi schema style.

- [ ] **Step 4: Add small row helpers**

In `scriptlet_evidence.rs`, add:

```rust
pub const CLUSTER_KEY_PREFIX: &str = "s1-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceCluster { /* columns */ }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceSample { /* columns */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptletEvidenceState {
    NeedsTriage,
    AdapterCandidate,
    InDesign,
    InImplementation,
    CoveredPartial,
    CoveredPublicReady,
    WontSupport,
}
```

Helpers should cover:

- upsert cluster without overwriting existing `state`;
- upsert sample by unique observation key;
- list clusters with computed aggregate counts;
- get cluster detail;
- update state and append event in one transaction;
- insert note;
- create/update/complete/fail backfill run.

- [ ] **Step 5: Prove schema and helper behavior**

Run:

```bash
cargo test -p conary-core scriptlet_evidence
cargo test -p conary-core test_migrate_v75
```

Expected: PASS.

## Task 2: Add Normalization And Cluster-Key Generation

**Files:**
- Add `apps/remi/src/server/scriptlet_evidence_queue/mod.rs`
- Add `apps/remi/src/server/scriptlet_evidence_queue/types.rs`
- Add `apps/remi/src/server/scriptlet_evidence_queue/normalization.rs`
- Modify `apps/remi/src/server/mod.rs`

- [ ] **Step 1: Write failing normalizer tests**

Add unit tests in `normalization.rs`:

```rust
#[test]
fn normalizes_kernel_boot_and_env_values() {
    let shape = normalize_command_shape(
        "dracut",
        &[
            "--force".to_string(),
            "/boot/initramfs-6.10.12-200.fc40.x86_64.img".to_string(),
            "$KERNEL_VERSION".to_string(),
            "SECRET=/home/remi/private".to_string(),
        ],
    );

    assert_eq!(
        shape,
        "dracut --force <boot>/initramfs-<kver>.img <env> <env-assignment>"
    );
    assert!(!shape.contains("remi"));
    assert!(!shape.contains("6.10.12-200"));
}

#[test]
fn cluster_key_ignores_architecture_and_package_identity() {
    let base = ClusterKeyInput {
        schema_version: 1,
        distro: "fedora".to_string(),
        target_profile: "fedora-44".to_string(),
        blocked_class: "initramfs".to_string(),
        command: "dracut".to_string(),
        normalized_command_shape: "dracut --force <boot>/initramfs-<kver>.img".to_string(),
        lifecycle_phase: "postinstall".to_string(),
    };

    let first = stable_cluster_key(&base);
    let second = stable_cluster_key(&ClusterKeyInput { ..base });

    assert_eq!(first.cluster_key, second.cluster_key);
    assert!(first.cluster_key.starts_with("s1-"));
    assert!(!first.cluster_key.contains(':'));
}
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p remi scriptlet_evidence_queue::normalization
```

Expected: FAIL until the module exists.

- [ ] **Step 3: Implement normalization**

Implement:

- `normalize_command_shape(command, argv) -> String`
- `normalize_token(token) -> Option<String>`
- `target_profile_for_distro(distro) -> String`
- `stable_cluster_key(input) -> StableClusterKey`

Use `conary_core::hash::sha256` for hex hashes. Build canonical JSON for the key tuple with `serde_json::json!` and a fixed field order.

- [ ] **Step 4: Prove normalization**

Run:

```bash
cargo test -p remi scriptlet_evidence_queue::normalization
```

Expected: PASS.

## Task 3: Aggregate Converted Rows Into Queue Samples

**Files:**
- Add `apps/remi/src/server/scriptlet_evidence_queue/aggregation.rs`
- Add `apps/remi/src/server/scriptlet_evidence_queue/storage.rs`
- Modify `apps/remi/src/server/scriptlet_evidence_queue/mod.rs`

- [ ] **Step 1: Write failing aggregation tests**

Create tests that build `ConvertedPackage` rows directly in an in-memory migrated DB.

Required cases:

- blocked initramfs row with `boot_security_intents` creates a `dracut` cluster;
- review-required row without command evidence creates a class-level fallback cluster;
- malformed summary JSON creates a `malformed-scriptlet-summary` cluster;
- public-ready row creates no cluster;
- missing review artifact path marks the sample stale without failing.

Example assertion shape:

```rust
#[test]
fn blocked_boot_security_summary_creates_command_cluster() {
    let (_temp, conn) = crate::server::conversion::test_support::create_test_db();
    let mut converted = converted_package_with_summary(blocked_initramfs_summary());
    converted.insert(&conn).unwrap();

    let samples = evidence_samples_from_converted(&converted, Path::new("/tmp/cache")).unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].blocked_class, "initramfs");
    assert_eq!(samples[0].command, "dracut");
    assert!(samples[0].normalized_command_shape.contains("<boot>"));
    assert!(samples[0].normalized_command_shape.contains("<kver>"));
}
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p remi scriptlet_evidence_queue::aggregation
```

Expected: FAIL until aggregation exists.

- [ ] **Step 3: Implement sample extraction**

Implement:

- `evidence_samples_from_converted(converted, cache_dir) -> anyhow::Result<Vec<PendingEvidenceSample>>`
- `record_converted_package(conn, cache_dir, converted) -> anyhow::Result<RecordSummary>`
- review artifact staleness detection using `publication::validate_review_artifact_path` when the file exists;
- safe fallback when the review root or file is absent.

Stale artifact semantics:

- Missing `review_artifact_path`: sample has `review_artifact_stale = true`.
- Path outside review root: sample has `review_artifact_stale = true`, and packet/detail must never expose that path.
- File missing under review root: sample has `review_artifact_stale = true`.

- [ ] **Step 4: Prove aggregation**

Run:

```bash
cargo test -p remi scriptlet_evidence_queue::aggregation
cargo test -p remi publication
```

Expected: PASS. Publication tests prove the queue did not weaken existing gates.

## Task 4: Add Operator-Triggered Backfill

**Files:**
- Add `apps/remi/src/server/scriptlet_evidence_queue/backfill.rs`
- Modify `apps/remi/src/server/scriptlet_evidence_queue/storage.rs`
- Add `apps/remi/src/server/handlers/admin/scriptlet_evidence.rs`
- Modify `apps/remi/src/server/handlers/admin/mod.rs`
- Modify `apps/remi/src/server/routes/admin.rs`

- [ ] **Step 1: Write failing backfill tests**

Add tests for:

- a pre-existing blocked row appears after one backfill batch;
- repeated backfill does not duplicate samples;
- limit is honored and progress resumes after `last_converted_package_id`;
- public rows are skipped;
- malformed summary rows are included.

Example route-level test:

```rust
#[tokio::test]
async fn admin_backfill_requires_admin_scope_and_materializes_existing_rows() {
    let (app, db_path) = super::test_helpers::test_app().await;
    seed_blocked_converted_package(&db_path);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/admin/scriptlet-evidence/backfill")
                .header("authorization", "Bearer test-admin-token-12345")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"limit": 100}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p remi scriptlet_evidence_backfill
```

Expected: FAIL until the backfill module and route exist.

- [ ] **Step 3: Implement backfill batch**

Implement:

- `run_backfill_batch(db_path, cache_dir, limit) -> anyhow::Result<BackfillBatchResult>`
- query `converted_packages` by ascending `id` where:
  - `publication_status != 'public'`, or
  - `scriptlet_summary_json` is malformed enough that `scriptlet_summary_for_publication().valid == false`.
- store a `scriptlet_evidence_backfill_runs` row per batch.
- return `complete = true` when fewer than `limit` rows remain.

Backfill must use `spawn_blocking` from handlers and must not run automatically in `start_server`.

- [ ] **Step 4: Prove backfill**

Run:

```bash
cargo test -p remi scriptlet_evidence_backfill
cargo test -p remi publication
```

Expected: PASS.

## Task 5: Add Incremental Recording On New Conversions

**Files:**
- Modify `apps/remi/src/server/conversion/persistence.rs`
- Modify `apps/remi/src/server/scriptlet_evidence_queue/mod.rs`
- Modify or add tests in `apps/remi/src/server/conversion/persistence.rs`

- [ ] **Step 1: Write failing conversion persistence tests**

Add tests for:

- a blocked conversion writes a review artifact, inserts a `converted_packages` row, and records a queue sample;
- a public conversion inserts no queue sample;
- queue recording failure does not change `ServerConversionOutcome`.

Use existing `make_conversion_result`, `goal8a_scriptlet_summary`, and temporary CCS path helpers already in `persistence.rs`.

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p remi persist_conversion_records_scriptlet_evidence
```

Expected: FAIL until persistence calls the queue module.

- [ ] **Step 3: Implement best-effort recording**

After `converted.insert(&conn)?`, call:

```rust
if let Err(error) = crate::server::scriptlet_evidence_queue::record_converted_package(
    &conn,
    &self.cache_dir,
    &converted,
) {
    tracing::warn!(
        "failed to record scriptlet evidence queue sample for {} {}: {error}",
        metadata.name,
        metadata.version
    );
}
```

Keep the call after the converted row insert so the sample can reference `converted_package_id`. Do not include queue writes in the path that decides `ServerConversionOutcome`.

- [ ] **Step 4: Prove conversion behavior**

Run:

```bash
cargo test -p remi persist_conversion_records_scriptlet_evidence
cargo test -p remi --lib conversion
cargo test -p remi publication
```

Expected: PASS.

## Task 6: Add Admin List And Detail Endpoints

**Files:**
- Modify `apps/remi/src/server/handlers/admin/scriptlet_evidence.rs`
- Modify `apps/remi/src/server/routes/admin.rs`
- Modify `apps/remi/src/server/audit.rs`
- Modify `apps/remi/src/server/handlers/openapi.rs`

- [ ] **Step 1: Write failing endpoint tests**

Tests should cover:

- no bearer token returns `401`;
- bearer token with insufficient scope returns `403`;
- admin token returns cluster list;
- invalid `cluster_key` returns `400`;
- missing cluster returns `404`;
- detail response does not include raw `review_artifact_path`;
- stale artifact count is visible to admins.

Seed a non-admin token with `repos:read` using `admin_token::create` to prove scope rejection.

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p remi scriptlet_evidence_admin
```

Expected: FAIL until handlers and routes exist.

- [ ] **Step 3: Implement handlers**

Handler response DTOs should include:

- cluster key, class, command, normalized shape, state;
- distro and target profile;
- attempt count, unique package count, architecture set;
- first/last seen;
- stale sample count;
- sample summaries with package/version/arch/status/reason codes;
- review artifact availability as a boolean or route-shaped reference, never a raw local path.

Use `spawn_blocking` for DB work. Validate `limit <= 1000` for list requests.

- [ ] **Step 4: Update audit and OpenAPI**

In `audit.rs`, map `scriptlet-evidence` paths to `scriptlet.evidence`. Add route docs to `openapi.rs` with admin-only wording.

- [ ] **Step 5: Prove endpoint behavior**

Run:

```bash
cargo test -p remi scriptlet_evidence_admin
cargo test -p remi audit
cargo test -p remi openapi
```

Expected: PASS.

## Task 7: Add Candidate State, Events, And Notes

**Files:**
- Modify `apps/remi/src/server/handlers/admin/scriptlet_evidence.rs`
- Modify `apps/remi/src/server/scriptlet_evidence_queue/storage.rs`
- Modify `crates/conary-core/src/db/models/scriptlet_evidence.rs`

- [ ] **Step 1: Write failing state and notes tests**

Cover:

- `needs-triage -> adapter-candidate` persists;
- each transition appends a state event;
- invalid state returns `400`;
- note body is required and length-limited;
- notes are returned by admin detail;
- marking a cluster `covered-public-ready` does not update any `converted_packages.publication_status`.

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p remi scriptlet_evidence_state
```

Expected: FAIL until the mutation handlers exist.

- [ ] **Step 3: Implement state and notes**

Use the `TokenName` extension for `actor` when available. Use `"unknown-admin"` only as a defensive fallback; normal external admin requests should always carry a token name.

State updates must run in a transaction:

1. read current state;
2. validate requested state;
3. update cluster state and `updated_at`;
4. insert `scriptlet_evidence_state_events`.

Keep notes maintainer-only. Do not include notes in public-sanitized packet mode.

- [ ] **Step 4: Prove state behavior**

Run:

```bash
cargo test -p remi scriptlet_evidence_state
cargo test -p remi publication
```

Expected: PASS.

## Task 8: Add Adapter Packet Export

**Files:**
- Add `apps/remi/src/server/scriptlet_evidence_queue/packet.rs`
- Modify `apps/remi/src/server/handlers/admin/scriptlet_evidence.rs`
- Modify `apps/remi/src/server/routes/admin.rs`

- [ ] **Step 1: Write failing packet tests**

Cover:

- private packet includes cluster key, state, class, command shape, affected packages, counts, timestamps, reason codes, sanitized boot/security intents, suggested fixture names, support matrix row, notes, and safe review artifact references;
- public-sanitized packet omits review artifact identifiers and notes;
- packet does not contain raw `/home/`, `/tmp/`, review root paths, raw kernel versions, or environment values;
- unknown cluster returns `404`.

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p remi scriptlet_evidence_packet
```

Expected: FAIL until packet export exists.

- [ ] **Step 3: Implement packet export**

Build a packet from cluster detail plus support matrix context:

- find a `SupportMatrixEntry` by `class_id == cluster.blocked_class`;
- include `fixture_names` from the support matrix when present;
- suggest a fixture name using `blocked_class`, `command`, and the first affected package when no row exists;
- include `review_artifact` references only in private mode and only when the sample path validates under `review_artifact_root`.

Use JSON field names that are stable enough for future design-plan seeding:

```json
{
  "schema": "conary.remi.scriptlet-evidence-packet.v1",
  "visibility": "private",
  "cluster": {},
  "impact": {},
  "evidence": {},
  "support_matrix": {},
  "maintainer_notes": [],
  "adapter_work": {}
}
```

- [ ] **Step 4: Prove packet behavior**

Run:

```bash
cargo test -p remi scriptlet_evidence_packet
```

Expected: PASS.

## Task 9: Add Docs, Coherency Rows, And Final Verification

**Files:**
- Modify `docs/modules/remi.md`
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify `docs/superpowers/feature-coherency-ledger.tsv`

- [ ] **Step 1: Update Remi docs**

Add an admin-only "Scriptlet Evidence Queue" subsection near passive scriptlet metadata. It should say:

- the queue aggregates non-public conversion evidence into adapter planning clusters;
- it is admin/operator-only;
- it does not make blocked packages public;
- scan-only corpus evidence remains planning-only;
- public tracking and LLM assistance are deferred to separate designs.

- [ ] **Step 2: Add coherency row for route behavior**

Add a feature-coherency row for the admin evidence routes covering:

- claims: admin-only evidence queue routes list/detail/backfill/state/notes/packet;
- implementation paths: routes, handlers, queue module, DB model/migration;
- verification: `cargo test -p remi scriptlet_evidence_admin`, `cargo test -p remi scriptlet_evidence_packet`, `cargo test -p remi publication`;
- gate: rerun before changing queue routes or exposing evidence publicly.

- [ ] **Step 3: Update docs audit ledger and inventory**

Register `docs/modules/remi.md` changes and this implementation plan. Regenerate inventory after staging new tracked docs:

```bash
git add docs/superpowers/plans/archive/2026-07-04-remi-scriptlet-evidence-queue-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 4: Run final verification**

Run:

```bash
cargo fmt --check
cargo test -p conary-core scriptlet_evidence
cargo test -p conary-core test_migrate_v75
cargo test -p remi scriptlet_evidence_queue
cargo test -p remi scriptlet_evidence_backfill
cargo test -p remi scriptlet_evidence_admin
cargo test -p remi scriptlet_evidence_state
cargo test -p remi scriptlet_evidence_packet
cargo test -p remi publication
cargo test -p remi --lib conversion
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected: PASS.

## Deferred Follow-Ups

- Generic command-shape projection for every blocked/review class, beyond the boot/security intents available in v1.
- Public tracker projection with privacy wording, status vocabulary, and analytics bucketing.
- Advisory LLM summary design and implementation.
- Narrow `scriptlet-evidence:read` and `scriptlet-evidence:write` token scopes after the operator workflow stabilizes.
- Reconciliation tooling that compares `covered-public-ready` clusters against actual support-matrix and reconversion outcomes.

## Execution Order

1. Schema and core row helpers.
2. Normalization and stable key generation.
3. Aggregation from converted rows.
4. Backfill.
5. Incremental recording.
6. Admin list/detail endpoints.
7. State/events/notes.
8. Packet export.
9. Docs/coherency/final verification.

## Handoff

Recommended execution mode: **Subagent-Driven Development**. Tasks 1-4 and 6-8 have clean enough boundaries for parallel implementation after the schema and normalization foundations land. Keep Task 5 after Task 3 so conversion persistence calls a stable queue facade.
