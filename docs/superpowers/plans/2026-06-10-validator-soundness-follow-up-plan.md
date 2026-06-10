# Validator Soundness Follow-Up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close remaining fail-open edges in docs and ledger validators after higher-stakes evidence and release tracks land.

**Architecture:** Add failing fixture tests first, then tighten one validator behavior at a time. Keep every rule narrow, explainable, and tied to a documented invariant.

**Tech Stack:** Bash validators, shell fixtures, docs-truth, coherency ledger checks.

---

## Design Source

- `docs/superpowers/specs/2026-06-10-validator-soundness-follow-up-design.md`

## File Map

| Path | Purpose |
| --- | --- |
| `scripts/check-doc-truth.sh` | Route extraction and constrained CLI doc checks. |
| `scripts/test-doc-truth.sh` | Negative fixtures for docs-truth. |
| `scripts/check-coherency-ledger.sh` | Remaining ledger integrity edges if new fixtures expose them. |
| `scripts/test-coherency-ledger.sh` | Ledger validator fixture coverage. |
| `scripts/check-coherency-wave-scopes.sh` | Scope registry validation. |
| `scripts/check-release-matrix.sh` | Release matrix status semantics not covered by deploy-mode checks. |
| `scripts/test-release-matrix.sh` | Matrix-policy fixture tests for status semantics. |

## Task 0: Baseline

- [ ] Run:

```bash
bash scripts/test-doc-truth.sh
bash scripts/check-doc-truth.sh
bash scripts/test-coherency-ledger.sh
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
bash scripts/test-release-matrix.sh
bash scripts/check-release-matrix.sh
```

Expected: all pass before edits.

## Task 1: Tighten conaryd Route Extraction

- [ ] In `scripts/test-doc-truth.sh`, add a fixture route file that includes:

```rust
Router::new()
    .route("/v1/example", get(list).post(create))
    .route("/v1/example/{id}", put(update).patch(patch).delete(delete));
```

- [ ] Add matching docs inside the fixture route block:

```text
GET /v1/example
POST /v1/example
PUT /v1/example/{id}
PATCH /v1/example/{id}
DELETE /v1/example/{id}
```

- [ ] Verify the test fails before editing `scripts/check-doc-truth.sh`.
- [ ] Update extraction to support `GET`, `POST`, `PUT`, `PATCH`, and `DELETE` in separate or chained Axum route handlers.
- [ ] If extraction sees `.route(` with an unknown method shape, fail with a clear unsupported route-pattern message.
- [ ] Run `bash scripts/test-doc-truth.sh` and `bash scripts/check-doc-truth.sh`.

## Task 2: Prevent Silent Scan-Scope Shrinkage

- [ ] Add a test fixture where a required docs-truth path is missing.
- [ ] Update `scripts/check-doc-truth.sh` so required paths fail loudly unless they are explicitly optional in a local array.
- [ ] Keep archive or ignored local paths out of required checks.
- [ ] Run `bash scripts/test-doc-truth.sh`.

## Task 3: Add Constrained CLI Command Reference Check

- [ ] In `scripts/check-doc-truth.sh`, extract backtick command references matching:

```text
`conary <token>`
```

from `README.md`, `docs/conaryopedia-v2.md`, `docs/modules/*.md`, and `docs/operations/*.md`.
- [ ] Compare `<token>` to root help output from:

```bash
cargo run -p conary -- --help
```

or to a static allowlist generated from the Clap enum if running Cargo is too heavy for docs-truth.
- [ ] Add an allowlist for shell stand-ins, options, and examples that are not root command names.
- [ ] Add a negative fixture proving a retired command such as `` `conary verify` `` fails unless it is explicitly described as retired.
- [ ] Run `bash scripts/test-doc-truth.sh`.

## Task 4: Recheck Coherency Validators

- [ ] Add fixtures to `scripts/test-coherency-ledger.sh` only for gaps that still reproduce on current `main`.
- [ ] Before changing the checker, verify each new fixture fails against the current script.
- [ ] Candidate fixtures to recheck:
  - header-only ledger;
  - trailing raw tab;
  - malformed scope registry header;
  - command pointer containing a semicolon.
- [ ] Patch `scripts/check-coherency-ledger.sh` or `scripts/check-coherency-wave-scopes.sh` only for reproduced gaps.
- [ ] Run the coherency validator tests and live ledger checks.

## Task 5: Strengthen Release Matrix Semantics

- [ ] Track 3 owns deploy-mode consistency checks for products with `deploy_mode=none` and live deploy jobs. Do not duplicate that work here.
- [ ] Add tests in `scripts/test-release-matrix.sh` that prove release matrix checks fail when:
  - an expected product status row is missing from `docs/operations/release-artifact-matrix.md`;
  - a workflow route has keyword presence but an unsupported product/mode pair.
- [ ] Update `scripts/check-release-matrix.sh` to inspect the structure needed for those cases.
- [ ] Run `bash scripts/test-release-matrix.sh` and `bash scripts/check-release-matrix.sh`.

## Task 6: Final Verification And Commit

- [ ] Run:

```bash
bash scripts/test-doc-truth.sh
bash scripts/check-doc-truth.sh
bash scripts/test-coherency-ledger.sh
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
bash scripts/test-release-matrix.sh
bash scripts/check-release-matrix.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

- [ ] Commit:

```bash
git add scripts/check-doc-truth.sh scripts/test-doc-truth.sh scripts/check-coherency-ledger.sh scripts/test-coherency-ledger.sh scripts/check-coherency-wave-scopes.sh scripts/check-release-matrix.sh scripts/test-release-matrix.sh
git commit -m "test: harden validator soundness gates"
```

Only stage paths that changed.
