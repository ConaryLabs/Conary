# Limited Preview Honesty First Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the first limited-preview honesty slice concrete by fixing misleading CLI/docs examples, conaryd package-operation stubs, Phase 4 automation drift, and Remi write-route classification.

**Architecture:** This pass favors small truth corrections over broad refactors. Public CLI/docs fixes stay in the CLI and living docs; conaryd package-operation surfaces consistently return explicit 501 preview guidance; Remi route ownership is classified before route movement; review coverage is tracked in a ledger so line-by-line work has a durable signoff trail.

**Tech Stack:** Rust workspace, clap CLI definitions, axum service routes, TOML conary-test manifests, Markdown docs, TSV audit ledgers.

---

## Scope

This plan implements the first slice of
`docs/superpowers/specs/2026-05-14-limited-preview-codebase-honesty-and-cleanup-design.md`.

In scope:

- README live-mutation example honesty.
- SBOM example correction.
- Generation/bootstrap ISO wording and parser/help clarity.
- Automation docs and Phase 4 Group D manifest truth.
- conaryd package-operation route honesty for generic transactions, dry-run, and system states.
- Remi public write-route classification and comments.
- conary-test fallback response-shape classification, without mechanical deduplication.
- Review coverage ledger and doc audit updates.

Out of scope for this plan:

- install/remove/update transaction-lifecycle helper extraction.
- Remi CAS/chunk helper consolidation.
- conary-test HTTP/MCP fallback deduplication.
- broad large-file decomposition.
- route movement for Remi public write endpoints.

## Files

- Create: `docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv`
- Modify: `README.md`
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `crates/conary-core/src/generation/export.rs`
- Modify: `apps/conary/src/cli/bootstrap.rs`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/modules/bootstrap.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `apps/conaryd/src/daemon/routes.rs`
- Modify: `apps/conaryd/src/daemon/routes/system.rs`
- Modify: `apps/conaryd/src/daemon/routes/transactions.rs`
- Modify: `apps/remi/src/server/handlers/derivations.rs`
- Modify: `apps/remi/src/server/handlers/seeds.rs`
- Modify: `apps/remi/src/server/handlers/profiles.rs`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

---

### Task 1: Seed The Review Coverage Ledger

**Files:**
- Create: `docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv`

- [ ] **Step 1: Create the ledger with seed rows**

Create the file with this exact header and seed content:

```tsv
path	slice	reviewer	status	finding_category	decision	verification
README.md	1	agent	pending	documentation-drift	review live-mutation and SBOM examples	cargo run -p conary -- --help
apps/conary/src/cli/generation.rs	1,3	agent	pending	misleading-public-surface	review generation export help	cargo run -p conary -- system generation export --help
crates/conary-core/src/generation/export.rs	3	agent	pending	misleading-public-surface	review generation export parser/error wording	cargo test -p conary-core generation::export
apps/conary/src/cli/bootstrap.rs	1,3	agent	pending	misleading-public-surface	review bootstrap image help	cargo run -p conary -- bootstrap image --help
docs/ARCHITECTURE.md	3,7	agent	pending	documentation-drift	review ISO and resolver-map claims	rg -n "iso|ISO|graph.rs|engine.rs" docs/ARCHITECTURE.md
docs/modules/bootstrap.md	3,7	agent	pending	documentation-drift	review bootstrap ISO wording	rg -n "iso|ISO|boot media|output format" docs/modules/bootstrap.md
docs/conaryopedia-v2.md	1,3,7	agent	pending	documentation-drift	review automation and ISO claims	rg -n "automation history|daemon mode|persisting configuration|ISO|USB|optical" docs/conaryopedia-v2.md
apps/conary/tests/integration/remi/manifests/phase4-group-d.toml	1,6	agent	pending	documentation-drift	review automation history expectation	cargo run -p conary-test -- run --suite phase4-group-d --distro fedora44 --phase 4
apps/conaryd/src/daemon/routes.rs	2	agent	pending	misleading-public-surface	review conaryd route tests	cargo test -p conaryd daemon::routes
apps/conaryd/src/daemon/routes/system.rs	2	agent	pending	misleading-public-surface	review system state route honesty	cargo test -p conaryd daemon::routes
apps/conaryd/src/daemon/routes/transactions.rs	2	agent	pending	misleading-public-surface	review generic transaction and dry-run honesty	cargo test -p conaryd daemon::routes
apps/remi/src/server/handlers/derivations.rs	4	agent	pending	release-blocker	review public write auth/rate-limit comment	cargo test -p remi
apps/remi/src/server/handlers/seeds.rs	4	agent	pending	release-blocker	review public write auth/rate-limit comment	cargo test -p remi
apps/remi/src/server/handlers/profiles.rs	4	agent	pending	release-blocker	review public write auth/rate-limit comment	cargo test -p remi
apps/conary-test/src/server/handlers.rs	6	agent	pending	duplication-consolidation	record HTTP fallback ordering decision	cargo test -p conary-test
apps/conary-test/src/server/mcp.rs	6	agent	pending	duplication-consolidation	record MCP fallback response-shape decision	cargo test -p conary-test
apps/conary-test/src/server/service.rs	6	agent	pending	duplication-consolidation	record service fallback ordering decision	cargo test -p conary-test
```

- [ ] **Step 2: Verify the ledger is parseable as TSV**

Run:

```bash
awk -F '\t' 'NR == 1 { cols = NF; next } NF != cols { print FNR ":" $0; bad = 1 } END { exit bad }' docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv
```

Expected: exit 0 with no output.

- [ ] **Step 3: Commit the ledger seed**

Run:

```bash
git add docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv
git commit -m "docs: seed limited preview honesty review ledger"
```

---

### Task 2: Fix CLI And Documentation Honesty

**Files:**
- Modify: `README.md`
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `crates/conary-core/src/generation/export.rs`
- Modify: `apps/conary/src/cli/bootstrap.rs`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/modules/bootstrap.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv`

- [ ] **Step 1: Confirm the current command-help mismatches**

Run:

```bash
cargo run -p conary -- system generation export --help
cargo run -p conary -- bootstrap image --help
cargo run -p conary -- system sbom --help
cargo run -p conary -- provenance export --help
```

Expected before edits:

- generation export help lists `iso` as an ordinary format.
- bootstrap image help says `Generate bootable image` while listing `iso`.
- system SBOM help describes CycloneDX but accepts an arbitrary `--format` string.
- provenance export help lists SPDX support.

- [ ] **Step 2: Update README live-mutation examples**

In `README.md`, add this sentence before the first non-dry-run mutation example in the quick start:

```markdown
Commands that mutate the active host require the explicit `--allow-live-system-mutation` acknowledgement; dry-run commands remain the safest first pass.
```

Change these examples:

```diff
-./target/debug/conary install nginx             # Apply atomically
+./target/debug/conary --allow-live-system-mutation install nginx  # Apply atomically
-./target/debug/conary system generation build --summary "Initial setup"
+./target/debug/conary --allow-live-system-mutation system generation build --summary "Initial setup"
-./target/debug/conary system generation switch 1
+./target/debug/conary --allow-live-system-mutation system generation switch 1
-conary system generation build --summary "Post-update"
+conary --allow-live-system-mutation system generation build --summary "Post-update"
-conary system generation switch 3    # Select generation 3 for next boot
+conary --allow-live-system-mutation system generation switch 3    # Select generation 3 for next boot
-conary system generation rollback    # Select previous generation for next boot
+conary --allow-live-system-mutation system generation rollback    # Select previous generation for next boot
-conary system generation gc --keep 3 # Keep only the 3 most recent
+conary --allow-live-system-mutation system generation gc --keep 3 # Keep only the 3 most recent
-conary system adopt --system --full  # Bulk adoption with CAS backing
+conary --allow-live-system-mutation system adopt --system --full  # Bulk adoption with CAS backing
-conary system takeover --up-to generation --yes
+conary --allow-live-system-mutation system takeover --up-to generation --yes
-conary system generation switch 1    # Select the prepared generation for next boot
+conary --allow-live-system-mutation system generation switch 1    # Select the prepared generation for next boot
-conary install nginx postgresql redis
+conary --allow-live-system-mutation install nginx postgresql redis
-conary system state revert 5      # Revert to snapshot 5
+conary --allow-live-system-mutation system state revert 5      # Revert to snapshot 5
```

- [ ] **Step 3: Fix README SBOM examples**

Replace:

```bash
conary system sbom nginx --format spdx  # Generate SBOM
```

with:

```bash
conary system sbom nginx --format cyclonedx        # Generate runtime SBOM
conary provenance export nginx --format spdx       # Export provenance SBOM
```

- [ ] **Step 4: Update generation export help and parser wording**

In `apps/conary/src/cli/generation.rs`, replace:

```rust
        /// Output format: raw, qcow2, or iso.
```

with:

```rust
        /// Output format: raw or qcow2. ISO is reserved and returns a preview NotImplemented error.
```

In `crates/conary-core/src/generation/export.rs`, keep parsing `iso` so the explicit `NotImplemented` error still works, but change the invalid-format message to:

```rust
"invalid generation export format {other}; expected raw, qcow2, or reserved iso"
```

- [ ] **Step 5: Update bootstrap image help**

In `apps/conary/src/cli/bootstrap.rs`, replace:

```rust
    /// Generate bootable image
```

with:

```rust
    /// Generate a bootstrap image; ISO output is non-bootable preview scaffolding
```

Replace:

```rust
        /// Image format (raw, qcow2, iso, erofs)
```

with:

```rust
        /// Image format (raw, qcow2, erofs, or non-bootable preview iso)
```

- [ ] **Step 6: Update active bootstrap and ISO docs**

Update these active-doc claims so ISO is described as reserved or non-bootable preview scaffolding:

```bash
rg -n "iso|ISO|USB|optical|boot media|output format" docs/ARCHITECTURE.md docs/modules/bootstrap.md docs/conaryopedia-v2.md
```

Required end state:

- `docs/ARCHITECTURE.md` no longer presents bootstrap ISO as a normal bootable output.
- `docs/modules/bootstrap.md` no longer presents ISO as release-ready boot media.
- `docs/conaryopedia-v2.md` no longer labels current ISO output as normal USB/optical boot media.

- [ ] **Step 7: Update automation docs and manifest expectation**

In `docs/conaryopedia-v2.md`, replace the stale automation paragraph around `automation history`, daemon mode, and config persistence with:

```markdown
`automation history` reads records written by `conary automation apply` and prints `No automation history.` when none are present. `automation daemon` runs the scheduler in the foreground for preview use; use systemd or another supervisor for background operation. `automation configure` persists settings to the active model/config file path and prints the file it changed.
```

In `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`, change T254:

```diff
-description = "Explain that automation history is not implemented until actions are recorded"
+description = "Show empty automation history before actions are recorded"
```

and:

```diff
-stdout_contains_all = ["automation history is not yet implemented", "conary automation apply"]
+stdout_contains_all = ["No automation history."]
```

In `docs/INTEGRATION-TESTING.md`, update any current Phase 4 wording that still uses automation history as a not-implemented example.

- [ ] **Step 8: Update the review ledger rows for Task 2 files**

For each Task 2 file in `docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv`, change `pending` to `changed` and set the decision to the concrete correction made.

- [ ] **Step 9: Verify CLI help and manifest parsing**

Run:

```bash
cargo run -p conary -- system generation export --help
cargo run -p conary -- bootstrap image --help
cargo run -p conary -- system sbom --help
cargo run -p conary -- provenance export --help
cargo run -p conary-test -- list
```

Expected:

- generation export help says raw/qcow2 are the normal formats and ISO is reserved.
- bootstrap image help does not imply ISO is bootable.
- system/provenance SBOM help distinguishes CycloneDX and SPDX support.
- conary-test manifest inventory loads successfully.

- [ ] **Step 10: Run the owning Phase 4 group**

Run:

```bash
cargo run -p conary-test -- run --suite phase4-group-d --distro fedora44 --phase 4
```

Expected: Group D completes, including T254 with `No automation history.`

- [ ] **Step 11: Commit CLI and docs honesty fixes**

Run:

```bash
git add README.md apps/conary/src/cli/generation.rs crates/conary-core/src/generation/export.rs apps/conary/src/cli/bootstrap.rs docs/ARCHITECTURE.md docs/modules/bootstrap.md docs/conaryopedia-v2.md apps/conary/tests/integration/remi/manifests/phase4-group-d.toml docs/INTEGRATION-TESTING.md docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv
git commit -m "docs: clarify limited preview CLI honesty"
```

---

### Task 3: Make conaryd Package-Operation Routes Honest

**Files:**
- Modify: `apps/conaryd/src/daemon/routes.rs`
- Modify: `apps/conaryd/src/daemon/routes/system.rs`
- Modify: `apps/conaryd/src/daemon/routes/transactions.rs`
- Modify: `docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv`

- [ ] **Step 1: Change conaryd route tests first**

In `apps/conaryd/src/daemon/routes.rs`, change `test_handler_create_transaction_valid` to expect 501 for package operations. Use this replacement test body:

```rust
    #[tokio::test]
    async fn test_handler_create_package_transaction_not_implemented() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["nginx"]
                }
            ]
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
        assert_eq!(
            json["detail"],
            "Daemon package install jobs are not implemented yet. Use the CLI directly."
        );
    }
```

- [ ] **Step 2: Change the dry-run test first**

Replace `test_handler_dry_run_valid` with:

```rust
    #[tokio::test]
    async fn test_handler_dry_run_package_transaction_not_implemented() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["nginx", "curl"]
                }
            ]
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions/dry-run")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
        assert_eq!(
            json["detail"],
            "Daemon package install jobs are not implemented yet. Use the CLI directly."
        );
    }
```

- [ ] **Step 3: Change the system-states test first**

Replace `test_handler_list_states_empty` with:

```rust
    #[tokio::test]
    async fn test_handler_list_states_not_implemented() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = axum::http::Request::builder()
            .uri("/v1/system/states")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
        assert_eq!(
            json["detail"],
            "System state listing is not implemented in conaryd preview. Use the CLI directly."
        );
    }
```

- [ ] **Step 4: Run the tests to verify they fail before implementation**

Run:

```bash
cargo test -p conaryd daemon::routes::tests::test_handler_create_package_transaction_not_implemented daemon::routes::tests::test_handler_dry_run_package_transaction_not_implemented daemon::routes::tests::test_handler_list_states_not_implemented
```

Expected: fail because the current implementation returns 400 or 200 for at least one route.

- [ ] **Step 5: Update generic transaction creation**

In `apps/conaryd/src/daemon/routes/transactions.rs`, replace the bad-request block:

```rust
    if !matches!(job_kind, crate::daemon::JobKind::Enhance) {
        return Err(ApiError(Box::new(DaemonError::bad_request(&format!(
            "Job kind '{}' is not yet supported by the daemon. \
             Use the CLI directly for install/remove/update operations.",
            job_kind.as_str()
        )))));
    }
```

with:

```rust
    if !matches!(job_kind, crate::daemon::JobKind::Enhance) {
        return Err(package_jobs_not_implemented(job_kind.as_str()));
    }
```

- [ ] **Step 6: Update transaction dry-run**

In `dry_run_handler`, after the empty-operations check and before the synthetic summary construction, add:

```rust
    let job_kind = determine_job_kind(&request.operations);
    if !matches!(job_kind, crate::daemon::JobKind::Enhance) {
        return Err(package_jobs_not_implemented(job_kind.as_str()));
    }
```

- [ ] **Step 7: Update system state listing**

In `apps/conaryd/src/daemon/routes/system.rs`, replace:

```rust
async fn list_states_handler(State(_state): State<SharedState>) -> ApiResult<Json<Vec<()>>> {
    Ok(Json(vec![]))
}
```

with:

```rust
async fn list_states_handler(State(_state): State<SharedState>) -> ApiResult<Json<Vec<()>>> {
    Err(not_implemented_error(
        "System state listing is not implemented in conaryd preview. Use the CLI directly.",
    ))
}
```

- [ ] **Step 8: Run conaryd tests**

Run:

```bash
cargo test -p conaryd
```

Expected: pass.

- [ ] **Step 9: Update the review ledger rows for conaryd**

For `apps/conaryd/src/daemon/routes.rs`, `apps/conaryd/src/daemon/routes/system.rs`, and `apps/conaryd/src/daemon/routes/transactions.rs`, change `pending` to `changed` and record the 501 route decision.

- [ ] **Step 10: Commit conaryd route honesty**

Run:

```bash
git add apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes/system.rs apps/conaryd/src/daemon/routes/transactions.rs docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv
git commit -m "fix(conaryd): make deferred transaction routes explicit"
```

---

### Task 4: Classify Remi Public Write-Route Controls

**Files:**
- Modify: `apps/remi/src/server/handlers/derivations.rs`
- Modify: `apps/remi/src/server/handlers/seeds.rs`
- Modify: `apps/remi/src/server/handlers/profiles.rs`
- Modify: `docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv`

- [ ] **Step 1: Review the current public and admin router layers**

Run:

```bash
rg -n "put_derivation|put_seed|put_profile|rate_limit_middleware|audit_log_middleware|auth_middleware|require_admin_token" apps/remi/src/server/routes apps/remi/src/server/handlers apps/remi/src/server/auth.rs apps/remi/src/server/rate_limit.rs
```

Expected:

- derivation/seed/profile PUT endpoints are mounted on the public router.
- each endpoint calls `require_admin_token`.
- public router rate-limit, ban, body-limit, and audit middleware apply according to Remi config.
- admin-router governor/auth/audit layering is separate.

- [ ] **Step 2: Replace misleading write-endpoint comments**

In each of these files:

- `apps/remi/src/server/handlers/derivations.rs`
- `apps/remi/src/server/handlers/seeds.rs`
- `apps/remi/src/server/handlers/profiles.rs`

Replace the note that says admin rate limiters do not apply with this wording:

```rust
/// NOTE: Auth is checked inline via `require_admin_token` because this write
/// route lives on the public content-addressed API path for preview clients.
/// The public router's rate-limit, ban, body-limit, and audit middleware apply
/// when enabled in Remi config; the separate admin-router governor does not.
```

- [ ] **Step 3: Record the route decision in the review ledger**

For the three Remi handler rows in the review ledger, change `pending` to `changed` and set the decision to:

```text
kept public content-addressed write path; documented inline admin-token auth and public-router controls; admin-router movement remains outside this first pass
```

- [ ] **Step 4: Run Remi tests**

Run:

```bash
cargo test -p remi
```

Expected: pass.

- [ ] **Step 5: Commit Remi classification**

Run:

```bash
git add apps/remi/src/server/handlers/derivations.rs apps/remi/src/server/handlers/seeds.rs apps/remi/src/server/handlers/profiles.rs docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv
git commit -m "docs(remi): classify public write route controls"
```

---

### Task 5: Record conary-test Fallback Shape Decisions

**Files:**
- Modify: `docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv`

- [ ] **Step 1: Verify the fallback paths differ**

Run:

```bash
rg -n "sort_by_key|sorted by run ID descending|to_json_report|service::get_run|service::list_runs|Remi proxy failed" apps/conary-test/src/server/handlers.rs apps/conary-test/src/server/mcp.rs apps/conary-test/src/server/service.rs
```

Expected:

- HTTP list fallback sorts ascending for compatibility.
- service list fallback sorts descending.
- MCP list fallback delegates to the service result.
- MCP get fallback returns a JSON report string from the in-memory run entry.
- HTTP get fallback delegates through `service::get_run`.

- [ ] **Step 2: Update the review ledger**

For these rows:

- `apps/conary-test/src/server/handlers.rs`
- `apps/conary-test/src/server/mcp.rs`
- `apps/conary-test/src/server/service.rs`

Change `pending` to `deferred` and set the decision to:

```text
not equivalent; preserve current HTTP compatibility ordering and MCP response shape in this plan; dedup requires a separate API-shape decision
```

- [ ] **Step 3: Run conary-test unit checks**

Run:

```bash
cargo test -p conary-test
cargo run -p conary-test -- list
```

Expected: both pass.

- [ ] **Step 4: Commit conary-test fallback classification**

Run:

```bash
git add docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv
git commit -m "docs: record conary-test fallback review decision"
```

---

### Task 6: Finish Documentation Audit And Workspace Verification

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv`

- [ ] **Step 1: Add audit rows for the new plan and review ledger**

Add this row to `docs/superpowers/documentation-accuracy-audit-inventory.tsv`:

```tsv
docs/superpowers/plans/2026-05-14-limited-preview-honesty-first-pass-plan.md	planning	maintainer
```

The review ledger is a TSV execution artifact and is not emitted by `scripts/docs-audit-inventory.sh`; do not add it to the inventory unless the audit script is expanded to include TSV execution artifacts.

Add this row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

```tsv
docs/superpowers/plans/2026-05-14-limited-preview-honesty-first-pass-plan.md	docs/superpowers/plans/2026-05-14-limited-preview-honesty-first-pass-plan.md	planning	maintainer	codebase-honesty; implementation-plan; public-preview	docs/superpowers/specs/2026-05-14-limited-preview-codebase-honesty-and-cleanup-design.md; README.md; apps/conaryd/src/daemon/routes/transactions.rs; apps/remi/src/server/routes/public.rs	verified	corrected	Added the first implementation plan for limited-preview CLI/docs honesty, conaryd route truth, Remi write-route classification, Phase 4 automation drift, and review-ledger coverage.
```

- [ ] **Step 2: Run doc audit checks**

Run:

```bash
bash scripts/docs-audit-inventory.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected: both pass with the new plan represented.

- [ ] **Step 3: Run targeted stale-claim sweeps**

Run:

```bash
rg -n "automation history is not yet implemented|USB/optical|boot media|format spdx|raw, qcow2, or iso|graph.rs|engine.rs" README.md docs apps/conary/src apps/conary/tests/integration/remi/manifests
```

Expected:

- no README `system sbom ... --format spdx` example remains.
- no active automation history stub claim remains.
- no active bootstrap ISO wording presents current ISO output as release-ready boot media.
- any remaining `graph.rs` or `engine.rs` hit is historical or outside the resolver map corrected in this pass.

- [ ] **Step 4: Run baseline workspace gates**

Run:

```bash
cargo fmt --check
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: all pass.

- [ ] **Step 5: Commit audit and verification cleanup**

Run:

```bash
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/plans/archive/2026-05-14-limited-preview-honesty-review-ledger.tsv
git commit -m "docs: record limited preview honesty audit coverage"
```

---

## Completion Criteria

- The review coverage ledger exists and records a disposition for every file touched by this plan.
- README examples either dry-run or include the live-mutation acknowledgement.
- README SBOM examples match actual format support.
- Generation/bootstrap ISO help and active docs no longer imply ISO is release-ready.
- Automation history docs and Phase 4 Group D expectation match current behavior.
- conaryd package-operation transaction surfaces return explicit 501 preview guidance.
- Remi public-path write controls are documented precisely in code comments and the review ledger.
- conary-test fallback deduplication is explicitly deferred because response shapes are not equivalent.
- Documentation inventory and ledger checks pass.
- Baseline fast checks pass.

## Self-Review Notes

- Spec coverage: covers the first implementation plan in the revised design spec, including the agentic-review amendments.
- Risk ordering: puts public/doc honesty and conaryd route truth before transaction helper extraction or Remi route movement.
- Verification: includes owning package tests, Phase 4 Group D, stale-claim sweeps, doc audit checks, and workspace fast gates.
