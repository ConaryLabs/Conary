# Remi Local-Only Test-Serving Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align Remi non-public admin/test serving with the approved design by explicitly treating valid `local-only` converted rows as test-servable while keeping public routes gated.

**Architecture:** Keep the existing admin-only non-public serving lane in `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`. Add focused regression coverage for `local-only` manifest and download behavior, then update Remi/scriptlet docs and audit metadata so the documented policy matches the design and code. Public package, chunk, OCI, sparse, search, and detail routes must continue to use the public-ready predicate.

**Tech Stack:** Rust, axum route tests, rusqlite test fixtures, Remi docs-audit checks.

## Global Constraints

- `non_public_test_serving.enabled` remains default-off.
- Admin test serving remains admin-scoped and must not rewrite `publication_status`.
- Public package, index, sparse, OCI, search, detail, and chunk routes must keep filtering by public-ready state.
- Test-serving metadata must never serialize raw `review_artifact_path` values.
- Malformed and stale converted rows are not test-served.
- Valid `local-only` rows are non-public rows and may be served only through the enabled admin/test lane.
- No new public URLs, unauthenticated routes, or public chunk-gate bypasses.
- Documentation changes must keep `docs/modules/remi.md`, `docs/SCRIPTLET_SECURITY.md`, the documentation accuracy ledger, and the inventory aligned.

---

## File Structure

- Modify `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`
  - Add a `local_only_summary()` test helper.
  - Add a regression test proving valid `local-only` rows return sanitized metadata and stream through `test-download` when the admin lane is enabled.
- Modify `apps/remi/src/server/publication.rs`
  - Extend publication status and chunk/listing tests so `local-only` remains review-required and non-public on public surfaces.
- Modify `docs/modules/remi.md`
  - Clarify that non-public test serving covers valid non-public rows, including `local-only`.
- Modify `docs/SCRIPTLET_SECURITY.md`
  - Clarify that maintainers may inspect blocked, private-review, or local-only converted CCS files through the admin/test lane.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Update the Remi non-public test-serving design row to mention local-only alignment.
  - Keep this implementation plan row aligned with the final touched files and proof scope.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Regenerate after staging this plan and doc changes.

## Task 1: Pin Local-Only Admin Test Serving

**Files:**
- Modify: `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`
- Modify: `apps/remi/src/server/publication.rs`

**Interfaces:**
- Consumes: `seed_non_public_test_row_with_summary(db_path, architecture, ccs_name, summary, summary_valid)`
- Consumes: existing `lookup_non_public_test_package` admin lookup behavior
- Produces: `local_only_summary() -> ScriptletBundleSummary` test helper
- Produces: `non_public_test_serving_manifest_and_download_allow_local_only_rows` regression test
- Produces: `publication_policy_maps_statuses_to_decisions` coverage for `local-only`
- Produces: `publication_golden_outcomes_filter_public_listing_and_chunks` coverage for `local-only`

- [ ] **Step 1: Extend publication classification coverage**

In `apps/remi/src/server/publication.rs`, add this assertion to
`publication_policy_maps_statuses_to_decisions` after the existing
`private-review` assertion:

```rust
assert!(matches!(
    classify_summary(ScriptletSummaryForPublication {
        summary: summary("local-only"),
        valid: true,
    }),
    PublicationDecision::ReviewRequired(_)
));
```

- [ ] **Step 2: Extend public listing and chunk-gate coverage**

In `publication_golden_outcomes_filter_public_listing_and_chunks`, add this
summary after `review_required` and before `blocked`:

```rust
let local_only = golden_summary("local-only", "local-only", "local-only");
```

Add this case to the `cases` array before the blocked case:

```rust
(
    "goal8a-local-only",
    "local-only-chunk",
    local_only,
    false,
),
```

The existing assertions in the test must then prove the local-only row is
absent from the public-ready name set and has
`ChunkPublicationState::NonPublicOnly`.

- [ ] **Step 3: Add the local-only summary helper**

In the test module in `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`, add this helper after `blocked_summary`:

```rust
fn local_only_summary() -> ScriptletBundleSummary {
    ScriptletBundleSummary {
        publication_status: "local-only".to_string(),
        scriptlet_fidelity: "local-only".to_string(),
        target_compatibility: "local-only".to_string(),
        review_artifact_path: Some("/tmp/private-review-secret.json".to_string()),
        ..ScriptletBundleSummary::default()
    }
}
```

- [ ] **Step 4: Add the admin-route regression test**

Add this test near the other non-public test-serving route tests:

```rust
#[tokio::test]
async fn non_public_test_serving_manifest_and_download_allow_local_only_rows() {
    let (app, db_path) = test_app_with_non_public_test_serving(true).await;
    seed_non_public_test_row_with_summary(
        &db_path,
        "x86_64",
        "pkg-local-only.ccs",
        local_only_summary(),
        true,
    );

    let manifest_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(manifest_response.status(), StatusCode::OK);
    let body = to_bytes(manifest_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();

    assert!(body.contains("\"status\":\"non-public-test-serving\""));
    assert!(body.contains("\"publication_status\":\"local-only\""));
    assert!(body.contains("\"review_artifact_available\":true"));
    assert!(!body.contains("review_artifact_path"));
    assert!(!body.contains("private-review-secret"));

    let download_response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/admin/packages/fedora/pkg/test-download?version=1.0&arch=x86_64")
                .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(download_response.status(), StatusCode::OK);
    assert_eq!(
        download_response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let body = to_bytes(download_response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"non-public ccs");
}
```

- [ ] **Step 5: Run the focused tests**

Run:

```bash
cargo test -p remi publication_policy_maps_statuses_to_decisions
cargo test -p remi publication_golden_outcomes_filter_public_listing_and_chunks
cargo test -p remi non_public_test_serving_manifest_and_download_allow_local_only_rows
```

Expected: PASS. This is a regression pin for behavior that the approved Remi design already intends and the current classification path may already support.

- [ ] **Step 6: Run the route family and publication tests**

Run:

```bash
cargo test -p remi non_public_test_serving
cargo test -p remi publication
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add apps/remi/src/server/handlers/admin/non_public_test_serving.rs apps/remi/src/server/publication.rs
git commit -m "test: pin local-only non-public test serving"
```

## Task 2: Align Remi And Scriptlet Docs

**Files:**
- Modify: `docs/modules/remi.md`
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Read: `docs/modules/test-fixtures.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

**Interfaces:**
- Consumes: `GET /v1/admin/packages/{distro}/{package}/test-manifest`
- Consumes: `GET /v1/admin/packages/{distro}/{package}/test-download`
- Produces: docs that explicitly include `local-only` in the admin/test lane and keep public publication separate

- [ ] **Step 1: Update Remi docs**

In `docs/modules/remi.md`, replace the first paragraph under `### Non-Public Test Serving` with:

```markdown
Remi also has a default-off admin/test lane for valid non-public converted
artifacts, including rows classified as `blocked`, `private-review`, or
`local-only`. When `non_public_test_serving.enabled = true`, an admin can
request `/v1/admin/packages/{distro}/{package}/test-manifest` or
`/v1/admin/packages/{distro}/{package}/test-download` with an exact version and
optional architecture (`arch`, or `architecture` as an alias). This does not
change public publication status, public indexes, OCI tags, sparse indexes,
search results, or public chunk serving. Malformed or stale rows are not
test-served; they need reconversion or metadata repair first.
```

- [ ] **Step 2: Update scriptlet security docs**

In `docs/SCRIPTLET_SECURITY.md`, replace the Remi non-public test-serving paragraph with:

```markdown
Maintainers may use Remi's non-public admin/test serving lane to fetch blocked,
private-review, or local-only converted CCS files for inspection. That lane is
disabled by default, requires admin access, and preserves the original
scriptlet publication status. It is not public publication authority and does
not permit raw legacy scriptlet replay.
```

- [ ] **Step 3: Update docs-audit ledger**

In `docs/superpowers/documentation-accuracy-audit-ledger.tsv`, update the row for `docs/superpowers/specs/2026-07-08-remi-non-public-test-serving-design.md` so the description mentions `local-only` rows as eligible admin/test artifacts.

Confirm the existing row for
`docs/superpowers/plans/2026-07-08-remi-local-only-test-serving-alignment-plan.md`
still names the implementation files touched by this slice:
`apps/remi/src/server/handlers/admin/non_public_test_serving.rs` and
`apps/remi/src/server/publication.rs`.

Keep each edited ledger row at exactly 9 tab-separated fields; do not replace
literal tabs with spaces.

- [ ] **Step 4: Check fixture-family docs**

Read `docs/modules/test-fixtures.md` and confirm the
`remi-scriptlet-publication-gate` fixture family still accurately describes the
fast proof after adding `local-only` coverage to `cargo test -p remi
publication`. Update the file only if the existing fixture-family description
has become inaccurate.

- [ ] **Step 5: Regenerate inventory**

Run:

```bash
git add docs/superpowers/plans/2026-07-08-remi-local-only-test-serving-alignment-plan.md docs/modules/remi.md docs/SCRIPTLET_SECURITY.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh > /tmp/remi-local-only-inventory.tsv
cp /tmp/remi-local-only-inventory.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 6: Run docs gates**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add docs/modules/remi.md docs/SCRIPTLET_SECURITY.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/plans/2026-07-08-remi-local-only-test-serving-alignment-plan.md
git commit -m "docs: align local-only test-serving policy"
```

## Task 3: Final Verification And Review

**Files:**
- Read: `docs/superpowers/plans/2026-07-08-remi-local-only-test-serving-alignment-plan.md`
- Read: `.superpowers/sdd/progress.md`

**Interfaces:**
- Consumes: Task 1 and Task 2 commits
- Produces: final review package and clean verification record

- [ ] **Step 1: Run focused Remi proof**

Run:

```bash
cargo test -p remi non_public_test_serving
cargo test -p remi publication
```

Expected: PASS.

- [ ] **Step 2: Run docs and hygiene proof**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

- [ ] **Step 3: Request final code review**

Generate a review package from this slice base commit through `HEAD`, dispatch a final reviewer with the roadmap design, Remi non-public test-serving design, and this plan as required context, and fix Critical/Important findings before considering the slice complete.

- [ ] **Step 4: Record completion**

Append a line to `.superpowers/sdd/progress.md` naming the concrete 7-character
base and head commit abbreviations for this slice and recording the final
review result.

Do not commit `.superpowers/sdd/progress.md`.
