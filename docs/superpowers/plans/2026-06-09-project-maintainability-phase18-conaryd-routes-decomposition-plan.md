# Phase 18 conaryd Routes Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose `apps/conaryd/src/daemon/routes.rs` from a 2,345-line hotspot into a focused route hub plus route-owned child modules, without changing daemon API behavior, public route paths, authorization behavior, SSE behavior, response shapes, or test coverage.

**Architecture:** Keep `routes.rs` as the stable `daemon::routes` module and public re-export point for `build_router` and the existing public route DTO surface. Move shared API DTOs, error conversion, route-level auth and visibility gates, blocking DB query plumbing, SSE connection-limit state, router assembly, and route-local tests into focused child modules under `apps/conaryd/src/daemon/routes/`. Preserve the existing endpoint-owner files (`events.rs`, `query.rs`, `system.rs`, `transactions.rs`) because `scripts/check-doc-truth.sh` hard-codes those four files for route extraction.

**Tech Stack:** Rust 2024, Axum, Tokio, Serde, rusqlite-backed Conary DB models, Tower test helpers, existing conaryd daemon state/job/event infrastructure.

## Current Repo Facts To Preserve

- `apps/conaryd/src/daemon/routes.rs` is 2,345 lines and is currently the top maintainability hotspot.
- Existing route child modules:
  - `apps/conaryd/src/daemon/routes/events.rs`
  - `apps/conaryd/src/daemon/routes/query.rs`
  - `apps/conaryd/src/daemon/routes/system.rs`
  - `apps/conaryd/src/daemon/routes/transactions.rs`
- `scripts/check-doc-truth.sh` extracts documented route methods and paths from `routes/{system,transactions,query,events}.rs` specifically. Do not move endpoint declarations out of these four owner files.
- Baseline route test inventory:
  - `cargo test -p conaryd --lib daemon::routes::tests -- --list` lists exactly 55 tests.
  - `cargo test -p conaryd --lib daemon::routes -- --list` lists exactly 55 route tests before decomposition.
  - `cargo test -p conaryd --lib daemon::routes::transactions -- --list` lists 0 tests before decomposition.
- Baseline docs-audit inventory before locking this plan:
  - `LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l` returns `161`.
  - Ledger counts are `archived 73`, `corrected 61`, `retained-historical 14`, `verified-no-change 13`.
- After locking in this plan file, the docs-audit inventory must be `162` tracked doc-like files and the ledger must have `62` `corrected` rows.

## Desired End State

```text
apps/conaryd/src/daemon/routes.rs
apps/conaryd/src/daemon/routes/auth.rs
apps/conaryd/src/daemon/routes/db.rs
apps/conaryd/src/daemon/routes/errors.rs
apps/conaryd/src/daemon/routes/events.rs
apps/conaryd/src/daemon/routes/query.rs
apps/conaryd/src/daemon/routes/router.rs
apps/conaryd/src/daemon/routes/sse.rs
apps/conaryd/src/daemon/routes/system.rs
apps/conaryd/src/daemon/routes/test_support.rs
apps/conaryd/src/daemon/routes/transactions.rs
apps/conaryd/src/daemon/routes/types.rs
```

Final `routes.rs` should contain only:

- the path comment,
- child module declarations,
- public re-export of `build_router`,
- public re-exports for the existing `daemon::routes::*` DTO/error surface,
- route-module-local internal imports only if truly needed by the hub,
- no route handlers,
- no DTO definitions,
- no `#[cfg(test)] mod tests`.

Sketch:

```rust
// apps/conaryd/src/daemon/routes.rs

mod auth;
mod db;
mod errors;
mod events;
mod query;
mod router;
mod sse;
mod system;
#[cfg(test)]
pub(super) mod test_support;
mod transactions;
mod types;

pub use errors::{ApiError, ApiResult};
pub use router::build_router;
pub use types::{
    CreateTransactionRequest, CreateTransactionResponse, DependencyInfo, DryRunResponse,
    DryRunSummary, HealthResponse, HistoryEntry, PackageDetails, PackageOperationOptions,
    PackageOperationRequest, PackageSummary, SearchQuery, SharedState, TransactionDetails,
    TransactionListQuery, TransactionOperation, TransactionSummary, VersionResponse,
};
```

## Visibility Contract

- `build_router` remains public through `daemon::routes::build_router`.
- Route response/request DTOs that are currently public in `routes.rs` remain `pub` in `types.rs` and are re-exported from `routes.rs` to avoid accidental API shrinkage.
- `TransactionOperation` must remain reachable as `crate::daemon::routes::TransactionOperation`; `apps/conaryd/src/daemon/package_ops.rs` imports that path today.
- `ApiError` and `ApiResult` remain `pub` in `errors.rs` and are re-exported from `routes.rs`.
- `ApiError` must expose route-internal construction either with `pub struct ApiError(pub(super) Box<DaemonError>);` or an equivalent `pub(super)` constructor. The least invasive move is the `pub(super)` tuple field because `auth.rs`, `db.rs`, `sse.rs`, and `transactions.rs` currently construct `ApiError(Box::new(...))`.
- Cross-module helpers should be `pub(super)` rather than `pub(crate)` unless an existing caller outside `daemon::routes` requires broader visibility.
- `TransactionDetails::from_job` and `HistoryEntry::from_changeset_with_publication` must be `pub(super)` because sibling endpoint modules call them.
- `SseConnectionGuard` must be `pub(super)` because `transactions.rs` names it in `JobSseStream`; `acquire_sse_connection` must be `pub(super)`.
- Test support must live behind `#[cfg(test)] pub(super) mod test_support;`; helper functions inside it should be `pub(super)` so sibling test modules can import them as `super::super::test_support::...`.
- Rust privacy reminder: child modules can access items visible in their ancestor module, but private items in one sibling are not visible to another sibling. Use explicit `pub(super)` for shared route helpers.

## Non-Goals

- Do not change HTTP route paths, methods, request formats, response formats, or status codes.
- Do not change auth semantics, Unix-socket identity checks, per-UID event/job visibility, idempotency handling, or SSE connection limits.
- Do not change DB schema, migrations, daemon state shape, or job queue behavior.
- Do not add new endpoints or retire documented endpoints.
- Do not move endpoint route declarations out of `events.rs`, `query.rs`, `system.rs`, or `transactions.rs`.
- Keep documented endpoint declarations in single-line `.route("...", get|post|delete(...))` form unless `scripts/check-doc-truth.sh` is updated in the same slice.
- Do not rewrite tests beyond path/import relocation and helper extraction.

## Task 0: Lock In This Plan

**Files:**

- `docs/superpowers/plans/2026-06-09-project-maintainability-phase18-conaryd-routes-decomposition-plan.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

**Steps:**

- [ ] Stage this plan file.
- [ ] Add a `corrected` ledger row for this plan file with exactly 9 tab-separated columns.
- [ ] Stage the ledger update after adding the row.
- [ ] Regenerate the tracked docs-audit inventory:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] Update `docs/superpowers/documentation-accuracy-audit-summary.md` so the latest maintainability planning note includes Phase 18 and the counts move to `162` tracked files / `62` corrected rows.
- [ ] Stage the inventory and summary updates.
- [ ] Use this evidence source set in the ledger row:

```text
apps/conaryd/src/daemon/routes.rs; apps/conaryd/src/daemon/routes/events.rs; apps/conaryd/src/daemon/routes/query.rs; apps/conaryd/src/daemon/routes/system.rs; apps/conaryd/src/daemon/routes/transactions.rs; apps/conaryd/src/daemon/package_ops.rs; scripts/check-doc-truth.sh; docs/modules/conaryd.md; docs/modules/feature-ownership.md; docs/llms/subsystem-map.md; docs/ARCHITECTURE.md
```

- [ ] Suggested ledger tags:

```text
maintainability; phase18; conaryd; daemon-routes; hotspot-decomposition
```

- [ ] Suggested ledger note:

```text
Added the Phase 18 conaryd routes decomposition plan for turning routes.rs into a focused route hub while extracting shared API types, error helpers, auth/visibility gates, DB helpers, SSE guards, router assembly, and route-local tests into child modules without changing daemon route behavior.
```

- [ ] Run:

```bash
git diff --check
git diff --cached --check
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected after staging this plan: inventory `162`; corrected count `62`; no malformed TSV rows.

- [ ] Commit with:

```bash
git commit -m "docs: plan conaryd route decomposition"
```

## Task 1: Extract Shared API Types

**Files:**

- Create `apps/conaryd/src/daemon/routes/types.rs`
- Update `apps/conaryd/src/daemon/routes.rs`
- Update existing route child modules as needed

**Move from `routes.rs` to `types.rs`:**

- `SharedState`
- `HealthResponse`
- `VersionResponse`
- `PackageSummary`
- `PackageDetails`
- `DependencyInfo`
- `HistoryEntry`
- `SearchQuery`
- `TransactionSummary`
- `TransactionDetails`
- `TransactionListQuery`
- `TransactionOperation`
- `CreateTransactionRequest`
- `PackageOperationRequest`
- `PackageOperationOptions`
- `CreateTransactionResponse`
- `DryRunResponse`
- `DryRunSummary`
- `publication_status_for_changeset`
- `is_false`

**`types.rs` import surface:**

```rust
// apps/conaryd/src/daemon/routes/types.rs

use crate::daemon::{DaemonError, DaemonJob, DaemonState};
use conary_core::db::models::{Changeset, DependencyEntry, GenerationPublication, Trove};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
```

**Visibility requirements:**

- Keep DTO structs/enums `pub`.
- Keep DTO fields public where they are public today.
- Keep `SharedState` as `pub type SharedState = Arc<DaemonState>;`.
- Make `HistoryEntry::from_changeset_with_publication` `pub(super)`.
- Make `TransactionDetails::from_job` `pub(super)`.
- Keep `publication_status_for_changeset` private if only used inside `types.rs`; make it `pub(super)` only if query code calls it directly after the move.
- Keep `is_false` private unless Serde path resolution requires a public path. Since it is used by `#[serde(default, skip_serializing_if = "is_false")]` in the same module, private is sufficient.

**Path updates:**

- In `routes.rs`, add public re-exports for the moved type surface:

```rust
pub use types::{
    CreateTransactionRequest, CreateTransactionResponse, DependencyInfo, DryRunResponse,
    DryRunSummary, HealthResponse, HistoryEntry, PackageDetails, PackageOperationOptions,
    PackageOperationRequest, PackageSummary, SearchQuery, SharedState, TransactionDetails,
    TransactionListQuery, TransactionOperation, TransactionSummary, VersionResponse,
};
```

- Do not break `apps/conaryd/src/daemon/package_ops.rs`; it currently imports `crate::daemon::routes::TransactionOperation`.
- In `query.rs`, import:

```rust
use super::types::{DependencyInfo, HistoryEntry, PackageDetails, PackageSummary, SearchQuery, SharedState};
```

- In `system.rs`, import:

```rust
use super::types::{HealthResponse, SharedState, VersionResponse};
```

- In `transactions.rs`, import:

```rust
use super::types::{
    CreateTransactionRequest, CreateTransactionResponse, DryRunResponse, DryRunSummary,
    PackageOperationOptions, PackageOperationRequest, SharedState, TransactionDetails,
    TransactionListQuery, TransactionOperation, TransactionSummary,
};
```

- In `events.rs`, import:

```rust
use super::types::SharedState;
```

**Move these tests to `types.rs` `#[cfg(test)] mod tests`:**

- `test_health_response_serialization`
- `test_version_response_serialization`
- `history_publication_status_matches_changeset_debt`
- `daemon_job_transaction_summary_does_not_claim_publication_status`

**`types.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::DaemonJob;
    use conary_core::db::models::{
        GenerationPublication, GenerationPublicationPhase, GenerationPublicationStatus,
    };
}
```

Adjust the test body to use the imported publication types or keep fully qualified paths and remove unused imports. Do not keep unused imports.

**Verification:**

```bash
cargo fmt
cargo check -p conaryd
cargo test -p conaryd --lib daemon::routes::types::tests -- --list
cargo test -p conaryd --lib daemon::routes::types::tests
```

Commit:

```bash
git add apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes/types.rs apps/conaryd/src/daemon/routes/*.rs
git commit -m "refactor(conaryd): extract route api types"
```

## Task 2: Extract Error Conversion And DB Query Plumbing

**Files:**

- Create `apps/conaryd/src/daemon/routes/errors.rs`
- Create `apps/conaryd/src/daemon/routes/db.rs`
- Update `apps/conaryd/src/daemon/routes.rs`
- Update route child modules

**Move from `routes.rs` to `errors.rs`:**

- `INTERNAL_ERROR_DETAIL`
- `ApiError`
- `ApiResult`
- `impl IntoResponse for ApiError`
- `not_found_error`
- `bad_request_error`
- `not_implemented_error`
- `internal_error`
- `internal_error_with`
- `internal_api_error`

**`errors.rs` import surface:**

```rust
// apps/conaryd/src/daemon/routes/errors.rs

use crate::daemon::DaemonError;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use std::fmt::Display;
```

**Visibility requirements:**

- `ApiError` remains `pub`.
- Define it as `pub struct ApiError(pub(super) Box<DaemonError>);` unless all sibling direct tuple constructions are rewritten to a `pub(super)` constructor. This preserves existing route-internal construction from `auth.rs`, `db.rs`, `sse.rs`, and `transactions.rs` after the split.
- `ApiResult<T>` remains `pub`.
- Helper constructors should be `pub(super)` unless an external caller already uses them.
- `internal_error`, `internal_error_with`, and `internal_api_error` must be `pub(super)` because `db.rs` and `transactions.rs` call them after the split.
- `INTERNAL_ERROR_DETAIL` should be `pub(super)` because `query::tests::test_internal_errors_are_sanitized_for_clients` imports it after the test moves.

In `routes.rs`, add public re-exports:

```rust
pub use errors::{ApiError, ApiResult};
```

**Move from `routes.rs` to `db.rs`:**

- `run_db_query`

**`db.rs` import surface:**

```rust
// apps/conaryd/src/daemon/routes/db.rs

use super::errors::{ApiError, internal_api_error, internal_error_with};
use super::types::SharedState;
```

`run_db_query` must remain `pub(super)` and continue to use `tokio::task::spawn_blocking` internally. If `ApiError` does not use a `pub(super)` tuple field, update the final `map_err` from direct tuple construction to the route-internal constructor or `ApiError::from`.

**Path updates:**

- Endpoint modules should import `ApiResult` and error helpers from `super::errors`.
- Endpoint modules should import `run_db_query` from `super::db`.
- Avoid re-exporting all error helpers from `routes.rs`; import them directly where used.

**Move this test to `errors.rs` `#[cfg(test)] mod tests`:**

- `test_api_error_response`

**`errors.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
}
```

**Verification:**

```bash
cargo fmt
cargo check -p conaryd
cargo test -p conaryd --lib daemon::routes::errors::tests -- --list
cargo test -p conaryd --lib daemon::routes::errors::tests
```

Commit:

```bash
git add apps/conaryd/src/daemon/routes/errors.rs apps/conaryd/src/daemon/routes/db.rs apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes/*.rs
git commit -m "refactor(conaryd): extract route errors and db helper"
```

## Task 3: Extract Route Auth, SSE Guarding, And Router Assembly

**Files:**

- Create `apps/conaryd/src/daemon/routes/auth.rs`
- Create `apps/conaryd/src/daemon/routes/sse.rs`
- Create `apps/conaryd/src/daemon/routes/router.rs`
- Update `apps/conaryd/src/daemon/routes.rs`
- Update `events.rs`, `system.rs`, and `transactions.rs`

### Auth

**Move from `routes.rs` to `auth.rs`:**

- `require_auth`
- `require_socket_identity`
- `auth_gate_middleware`
- `job_visible_to_requester`
- `ensure_job_visible`
- `event_visible_to_requester`

**`auth.rs` import surface:**

```rust
// apps/conaryd/src/daemon/routes/auth.rs

use super::errors::{ApiError, not_found_error};
use super::types::SharedState;
use crate::daemon::auth::{Action, AuthChecker, PeerCredentials};
use crate::daemon::{DaemonError, DaemonEvent, DaemonJob};
use axum::{
    extract::{Extension, Request, State},
    middleware,
    response::Response,
};
use std::collections::HashMap;
```

Preserve the existing behavior of every auth helper. `auth_gate_middleware` must continue requiring live root-or-daemon Unix socket identity for all `/v1` requests; per-handler `require_auth` calls keep mapping operations to `Action` values. Do not add method-to-action mapping inside the v1 gate.

### SSE

**Move from `routes.rs` to `sse.rs`:**

- `MAX_DAEMON_SSE_CONNECTIONS`
- `SseConnectionGuard`
- `impl Drop for SseConnectionGuard`
- `acquire_sse_connection`

**`sse.rs` import surface:**

```rust
// apps/conaryd/src/daemon/routes/sse.rs

use super::errors::ApiError;
use super::types::SharedState;
use crate::daemon::{DaemonError, DaemonState};
use std::sync::Arc;
use std::sync::atomic::Ordering;
```

**Visibility requirements:**

- `SseConnectionGuard` must be `pub(super)` because `transactions.rs` names it in `JobSseStream`.
- `acquire_sse_connection` must be `pub(super)`.
- `MAX_DAEMON_SSE_CONNECTIONS` can be private unless tests import it directly. If tests assert the explicit limit, prefer importing the constant as `pub(super)` rather than duplicating the literal.

### Router

**Move from `routes.rs` to `router.rs`:**

- `DAEMON_BODY_LIMIT_BYTES`
- `build_router`
- `build_v1_router`

**`router.rs` import surface:**

```rust
// apps/conaryd/src/daemon/routes/router.rs

use super::auth::auth_gate_middleware;
use super::types::SharedState;
use super::{events, query, system, transactions};
use axum::{Router, extract::DefaultBodyLimit, middleware};
```

**Visibility requirements:**

- `build_router` remains `pub`.
- `build_v1_router` can stay private.
- `DAEMON_BODY_LIMIT_BYTES` should be `pub(super)` because the router test verifies the boundary.
- In `routes.rs`, re-export:

```rust
pub use router::build_router;
```

**Path updates:**

- In `events.rs`, replace `use super::*;` with:

```rust
use super::auth::event_visible_to_requester;
use super::errors::ApiError;
use super::sse::acquire_sse_connection;
use super::types::SharedState;
use crate::daemon::auth::PeerCredentials;
use axum::{
    Router,
    extract::{Extension, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::get,
};
use futures::stream::{self, Stream};
use std::{collections::HashMap, convert::Infallible, time::Duration};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
```

- In `query.rs`, replace `use super::*;` with:

```rust
use super::db::run_db_query;
use super::errors::{ApiResult, not_found_error};
use super::types::{
    DependencyInfo, HistoryEntry, PackageDetails, PackageSummary, SearchQuery, SharedState,
};
use axum::{
    Router,
    extract::{Path, Query, State},
    response::Json,
    routing::get,
};
use conary_core::db::models::{Changeset, DependencyEntry, GenerationPublication, Trove};
```

Shorten `conary_core::db::models::GenerationPublication::pending_recoverable` to `GenerationPublication::pending_recoverable`.

- In `system.rs`, replace `use super::*;` with:

```rust
use super::auth::require_auth;
use super::errors::{ApiResult, not_implemented_error};
use super::types::{HealthResponse, SharedState, VersionResponse};
use crate::daemon::auth::{Action, PeerCredentials};
use axum::{
    Router,
    extract::{Extension, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use std::sync::atomic::Ordering;
```

- In `transactions.rs`, replace `use super::*;` with:

```rust
use super::auth::{ensure_job_visible, job_visible_to_requester, require_auth};
use super::db::run_db_query;
use super::errors::{
    ApiError, ApiResult, bad_request_error, internal_api_error, internal_error,
    internal_error_with, not_found_error,
};
use super::sse::{SseConnectionGuard, acquire_sse_connection};
use super::types::{
    CreateTransactionRequest, CreateTransactionResponse, DryRunResponse, DryRunSummary,
    PackageOperationOptions, PackageOperationRequest, SharedState, TransactionDetails,
    TransactionListQuery, TransactionOperation, TransactionSummary,
};
use crate::daemon::auth::{Action, PeerCredentials};
use crate::daemon::{DaemonError, DaemonEvent, DaemonJob, JobStatus};
use axum::{
    Router,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{
        Json,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
};
use futures::stream::{self, Stream};
use std::{convert::Infallible, sync::atomic::Ordering, time::Duration};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
```

Keep these paths fully qualified unless adding imports deliberately:

- `serde_json::to_value`, `serde_json::to_string`, and `serde_json::json!`
- `crate::daemon::EnhanceJobSpec` in `enhance_handler`
- `conary_core::Error::Database` in `insert_or_dedup`

`insert_or_dedup`, `get_idempotency_key`, `idempotent_job_response`, `enqueue_transaction_request`, `determine_job_kind`, `require_auth_for_operations`, and `validate_transaction_operations` remain in `transactions.rs`.

**Move auth/router/SSE tests:**

Move these tests to `auth.rs`:

- `test_require_auth_current_process_allowed`
- `test_require_auth_admin_group_allowed`
- `test_require_auth_regular_user_denied`
- `test_require_auth_no_creds_denied`
- `test_auth_gate_blocks_put_without_credentials`
- `test_auth_gate_blocks_delete_without_credentials`
- `test_auth_gate_blocks_get_without_credentials`
- `test_auth_gate_blocks_get_for_non_daemon_user`
- `test_auth_gate_revalidates_live_peer_identity`
- `test_event_visibility_filters_by_requesting_uid`

Move these tests to `router.rs`:

- `test_v1_router_rejects_request_bodies_over_2mb`
- `test_handler_nonexistent_route`

Move this test to `events.rs`:

- `test_handler_events_rejects_when_sse_limit_reached`

Suggested `auth.rs` test imports:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::{
        body_json, create_test_state, current_process_creds, test_router,
    };
    use crate::daemon::auth::{Action, AuthChecker, PeerCredentials};
    use crate::daemon::{DaemonEvent, DaemonJob};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use tower::ServiceExt;
}
```

Suggested `router.rs` test imports:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::{create_test_state, current_process_creds, test_router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;
}
```

**Test support dependency:**

These tests currently rely on helper functions that still live in the parent `routes.rs` test module. During this task, either move the needed helpers first into `test_support.rs` as described in Task 4, or keep the parent test module temporarily until Task 4. Do not leave duplicated helper definitions in multiple test modules.

Recommended: create `test_support.rs` in this task before moving auth/router/SSE tests. If you do that, treat Task 4 as a verification and cleanup checkpoint rather than creating the file a second time.

**Verification:**

```bash
cargo fmt
cargo check -p conaryd
cargo test -p conaryd --lib daemon::routes::auth::tests -- --list
cargo test -p conaryd --lib daemon::routes::router::tests -- --list
cargo test -p conaryd --lib daemon::routes::events::tests -- --list
cargo test -p conaryd --lib daemon::routes::auth::tests
cargo test -p conaryd --lib daemon::routes::router::tests
cargo test -p conaryd --lib daemon::routes::events::tests
bash scripts/check-doc-truth.sh
```

Commit:

```bash
git add apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes/*.rs
git commit -m "refactor(conaryd): extract route auth and router plumbing"
```

## Task 4: Extract Shared Route Test Support

**Files:**

- Create `apps/conaryd/src/daemon/routes/test_support.rs` if it was not already created in Task 3
- Update all child test modules
- Shrink the parent `routes.rs` test module until it is empty, then delete it in Task 6

**Move from `routes.rs` tests to `test_support.rs`:**

- `create_test_state`
- `create_test_state_with_db_path`
- `current_process_creds`
- `test_router`
- `body_bytes`
- `body_json`

**`test_support.rs` import surface:**

```rust
// apps/conaryd/src/daemon/routes/test_support.rs

use super::router::build_router;
use super::types::SharedState;
use crate::daemon::auth::PeerCredentials;
use crate::daemon::{DaemonConfig, DaemonState, SystemLock};
use axum::{Extension, Router, response::Response};
use http_body_util::BodyExt;
use std::sync::Arc;
```

**Visibility requirements:**

- Declare the module from `routes.rs` as:

```rust
#[cfg(test)]
pub(super) mod test_support;
```

- Make helper functions `pub(super)` so child tests can import:

```rust
use super::super::test_support::{body_json, create_test_state, test_router};
```

**Important import notes for child tests:**

- Each test module that calls `.oneshot(...)` must import `tower::ServiceExt`.
- Each test module that constructs `Request` must import `axum::http::Request` and `axum::body::Body` as needed.
- Each test module that uses `StatusCode` must import `axum::http::StatusCode`.
- Each test module that serializes JSON request bodies must import `serde_json::json` or use `serde_json::json!` fully qualified.
- Avoid defining duplicate state/router/body helpers in child modules.

**Verification:**

```bash
cargo fmt
cargo check -p conaryd
cargo test -p conaryd --lib daemon::routes -- --list
```

The list may still include some `daemon::routes::tests::*` entries until Task 6. That is acceptable only if unmoved tests remain in the parent module.

Commit:

```bash
git add apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes/test_support.rs apps/conaryd/src/daemon/routes/*.rs
git commit -m "refactor(conaryd): extract route test support"
```

## Task 5: Move System, Query, Events, And Transaction Tests To Owner Modules

**Files:**

- `apps/conaryd/src/daemon/routes/system.rs`
- `apps/conaryd/src/daemon/routes/query.rs`
- `apps/conaryd/src/daemon/routes/events.rs`
- `apps/conaryd/src/daemon/routes/transactions.rs`
- `apps/conaryd/src/daemon/routes.rs`

### System tests

Move these tests to `system.rs`:

- `test_handler_health_returns_200`
- `test_handler_version_returns_info`
- `test_handler_metrics_returns_prometheus_format`
- `test_handler_list_states_not_implemented`
- `test_handler_rollback_not_implemented`
- `test_handler_verify_not_implemented`
- `test_handler_gc_not_implemented`
- `test_handler_system_endpoints_require_auth`

Suggested `system.rs` test imports:

```rust
#[cfg(test)]
mod tests {
    use super::super::test_support::{
        body_bytes, body_json, create_test_state, current_process_creds, test_router,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
}
```

### Query tests

Move these tests to `query.rs`:

- `test_handler_list_packages_empty_db`
- `test_handler_get_package_not_found`
- `test_handler_get_package_files_not_found`
- `test_handler_search_empty_results`
- `test_handler_search_no_query_param`
- `test_handler_depends_not_found`
- `test_handler_rdepends_empty`
- `test_handler_history_empty`
- `test_internal_errors_are_sanitized_for_clients`

Suggested `query.rs` test imports:

```rust
#[cfg(test)]
mod tests {
    use super::super::errors::INTERNAL_ERROR_DETAIL;
    use super::super::test_support::{
        body_bytes, body_json, create_test_state, create_test_state_with_db_path,
        current_process_creds, test_router,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
}
```

Adjust for exact moved bodies and avoid unused imports.

### Events tests

If not already moved in Task 3, move this test to `events.rs`:

- `test_handler_events_rejects_when_sse_limit_reached`

Suggested `events.rs` test imports:

```rust
#[cfg(test)]
mod tests {
    use super::super::test_support::{create_test_state, current_process_creds, test_router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::atomic::Ordering;
    use tower::ServiceExt;
}
```

### Transaction tests

Move these tests to `transactions.rs`:

- `test_handler_list_transactions_empty`
- `test_handler_get_transaction_not_found`
- `test_handler_create_transaction_queues_package_jobs`
- `test_handler_create_transaction_empty_operations`
- `test_handler_create_transaction_rejects_mixed_package_kinds`
- `test_handler_create_transaction_invalid_json`
- `test_handler_create_transaction_forbidden`
- `test_handler_create_transaction_idempotency`
- `test_handler_enhance_idempotency`
- `test_handler_create_transaction_rejects_existing_enhance_idempotency_key`
- `test_handler_get_transaction_after_creation`
- `test_handler_list_transactions_with_status_filter`
- `test_handler_list_transactions_filters_by_requesting_uid`
- `test_handler_get_transaction_hides_foreign_job`
- `test_handler_transaction_stream_hides_foreign_job`
- `test_handler_cancel_transaction_not_found`
- `test_handler_cancel_transaction_hides_foreign_job`
- `test_package_routes_queue_package_jobs`
- `test_handler_dry_run_returns_package_summary`
- `test_handler_dry_run_empty_operations`

Suggested `transactions.rs` test imports:

```rust
#[cfg(test)]
mod tests {
    use super::super::test_support::{
        body_json, create_test_state, current_process_creds, test_router,
    };
    use crate::daemon::{DaemonJob, JobStatus};
    use crate::daemon::auth::PeerCredentials;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
}
```

The current transaction test bodies use `serde_json::json!`, `serde_json::to_string`, and `serde_json::to_value` fully qualified. Keep those fully qualified, or if you shorten the macro to `json!`, add `use serde_json::json;` and update every corresponding call so the import is used.

Adjust for exact moved bodies and avoid unused imports.

### Production import cleanup

- Replace `use super::*;` in `events.rs`, `query.rs`, `system.rs`, and `transactions.rs` with explicit imports.
- Keep route declarations and handlers in their existing endpoint owner modules.
- Do not move `router()` functions from endpoint owner modules.
- Do not change route path strings.

**Verification:**

```bash
cargo fmt
cargo check -p conaryd
cargo test -p conaryd --lib daemon::routes::system::tests -- --list
cargo test -p conaryd --lib daemon::routes::query::tests -- --list
cargo test -p conaryd --lib daemon::routes::events::tests -- --list
cargo test -p conaryd --lib daemon::routes::transactions::tests -- --list
cargo test -p conaryd --lib daemon::routes::system::tests
cargo test -p conaryd --lib daemon::routes::query::tests
cargo test -p conaryd --lib daemon::routes::events::tests
cargo test -p conaryd --lib daemon::routes::transactions::tests
bash scripts/check-doc-truth.sh
```

Commit:

```bash
git add apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes/*.rs
git commit -m "refactor(conaryd): move route tests to owner modules"
```

## Task 6: Clean The Hub And Validate Route Boundaries

**Files:**

- `apps/conaryd/src/daemon/routes.rs`
- all `apps/conaryd/src/daemon/routes/*.rs`

**Steps:**

- [ ] Delete the now-empty parent `#[cfg(test)] mod tests` from `routes.rs`.
- [ ] Delete unused imports from `routes.rs`.
- [ ] Confirm `routes.rs` only contains module declarations plus public re-exports for `build_router`, `ApiError`/`ApiResult`, and the existing route DTO surface.
- [ ] Confirm new files all start with path comments.
- [ ] Confirm endpoint route declarations are still in:
  - `events.rs`
  - `query.rs`
  - `system.rs`
  - `transactions.rs`
- [ ] Confirm no child route module still uses broad production `use super::*;`.

**Suggested boundary checks:**

```bash
rg -n "^\s*use super::\*;" apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes
rg -n "^(pub |pub\(|fn |async fn|struct |enum |type |const |impl |#\[cfg\(test\)\])" apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes/*.rs
cargo test -p conaryd --lib daemon::routes -- --list
cargo test -p conaryd --lib daemon::routes::tests -- --list
```

Expected:

- `daemon::routes -- --list` still lists 55 route tests.
- `daemon::routes::tests -- --list` should list 0 tests because the parent route test module is gone.
- Child owner modules should contain the 55 tests.

**Verification:**

```bash
cargo fmt
cargo check -p conaryd
cargo test -p conaryd --lib daemon::routes
bash scripts/check-doc-truth.sh
```

Commit:

```bash
git add apps/conaryd/src/daemon/routes.rs apps/conaryd/src/daemon/routes/*.rs
git commit -m "refactor(conaryd): slim route hub"
```

## Task 7: Update Documentation Routing

**Files:**

- `docs/modules/conaryd.md`
- `docs/modules/feature-ownership.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

**Docs updates:**

- In `docs/modules/conaryd.md`, keep the documented route reference table unchanged unless route paths actually changed. Replace any broad `routes/*.rs` route-check wording with the exact hard-coded owner-file set from `scripts/check-doc-truth.sh`: `routes/{system,transactions,query,events}.rs`. Add or update a short implementation-ownership note:

```markdown
Route implementation ownership: `apps/conaryd/src/daemon/routes.rs` is the route hub; `routes/router.rs` owns Axum assembly; `routes/types.rs` owns API DTOs; `routes/errors.rs` owns API error conversion; `routes/auth.rs` owns route-level auth and job/event visibility gates; `routes/db.rs` owns blocking DB query plumbing; `routes/sse.rs` owns SSE connection guarding; and `routes/{system,query,transactions,events}.rs` own endpoint declarations and handlers.
```

- In `docs/modules/feature-ownership.md`, expand the conaryd `Start here` routing paths so the split modules are discoverable:

```markdown
`apps/conaryd/src/daemon/routes.rs`; `apps/conaryd/src/daemon/routes/router.rs`;
`apps/conaryd/src/daemon/routes/auth.rs`; `apps/conaryd/src/daemon/routes/types.rs`;
`apps/conaryd/src/daemon/routes/errors.rs`; `apps/conaryd/src/daemon/routes/db.rs`;
`apps/conaryd/src/daemon/routes/sse.rs`; `apps/conaryd/src/daemon/routes/transactions.rs`;
`apps/conaryd/src/daemon/routes/query.rs`; `apps/conaryd/src/daemon/routes/system.rs`;
`apps/conaryd/src/daemon/routes/events.rs`;
```

Preserve existing references to `apps/conaryd/src/daemon/mod.rs`, `apps/conaryd/src/daemon/jobs.rs`, and `docs/modules/conaryd.md`.

- In `docs/llms/subsystem-map.md`, update the conaryd route/auth boundary entry to mention:
  - `apps/conaryd/src/daemon/routes.rs`
  - `apps/conaryd/src/daemon/routes/router.rs`
  - `apps/conaryd/src/daemon/routes/auth.rs`
  - `apps/conaryd/src/daemon/routes/types.rs`
  - `apps/conaryd/src/daemon/routes/errors.rs`
  - `apps/conaryd/src/daemon/routes/db.rs`
  - `apps/conaryd/src/daemon/routes/sse.rs`
  - existing endpoint owner modules under `apps/conaryd/src/daemon/routes/`
- In `docs/ARCHITECTURE.md`, update the conaryd tree description from a monolithic `routes.rs REST API endpoints` entry to a route hub plus route modules entry.
- Update existing ledger rows for changed docs and update the Phase 18 plan row evidence after the new split files exist. Do not add new rows for existing docs.
- Sweep active docs and audit files for stale `Phase 17`, `161`, `corrected 61`, and `routes/*.rs` wording introduced or made stale by this phase.

**Verification:**

```bash
git diff --check
bash scripts/check-doc-truth.sh
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected inventory remains `162` and corrected count remains `62` after Task 0 lock-in. Implementation tasks should not add new doc-like files.

Commit:

```bash
git add docs/modules/conaryd.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md docs/ARCHITECTURE.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: update conaryd route ownership"
```

## Task 8: Final Verification

Run all final gates from a clean working tree except for intentional staged changes.

```bash
cargo fmt --check
cargo check -p conaryd
cargo test -p conaryd --lib daemon::routes -- --list
cargo test -p conaryd --lib daemon::routes
cargo test -p conaryd daemon
cargo test -p conaryd
bash scripts/check-doc-truth.sh
cargo clippy -p conaryd --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/maintainability-drift-report.sh
scripts/line-count-report.sh 30
git diff --check
git status --short --branch
```

Expected outcomes:

- Formatting passes.
- `cargo check -p conaryd` passes.
- All conaryd route tests pass.
- `cargo test -p conaryd` passes.
- Route doc-truth check passes.
- Clippy passes with `-D warnings`.
- Docs-audit inventory is `162`.
- Docs-audit corrected rows are `62`.
- `routes.rs` is no longer a large hotspot; route logic is distributed across focused child modules.

If workspace-wide clippy finds unrelated pre-existing warnings, stop and record the exact output before deciding whether to fix or report the unrelated blocker.

## Test Mapping Checklist

Move each current parent route test exactly once:

**types.rs**

- [ ] `test_health_response_serialization`
- [ ] `test_version_response_serialization`
- [ ] `history_publication_status_matches_changeset_debt`
- [ ] `daemon_job_transaction_summary_does_not_claim_publication_status`

**errors.rs**

- [ ] `test_api_error_response`

**auth.rs**

- [ ] `test_require_auth_current_process_allowed`
- [ ] `test_require_auth_admin_group_allowed`
- [ ] `test_require_auth_regular_user_denied`
- [ ] `test_require_auth_no_creds_denied`
- [ ] `test_auth_gate_blocks_put_without_credentials`
- [ ] `test_auth_gate_blocks_delete_without_credentials`
- [ ] `test_auth_gate_blocks_get_without_credentials`
- [ ] `test_auth_gate_blocks_get_for_non_daemon_user`
- [ ] `test_auth_gate_revalidates_live_peer_identity`
- [ ] `test_event_visibility_filters_by_requesting_uid`

**router.rs**

- [ ] `test_v1_router_rejects_request_bodies_over_2mb`
- [ ] `test_handler_nonexistent_route`

**system.rs**

- [ ] `test_handler_health_returns_200`
- [ ] `test_handler_version_returns_info`
- [ ] `test_handler_metrics_returns_prometheus_format`
- [ ] `test_handler_list_states_not_implemented`
- [ ] `test_handler_rollback_not_implemented`
- [ ] `test_handler_verify_not_implemented`
- [ ] `test_handler_gc_not_implemented`
- [ ] `test_handler_system_endpoints_require_auth`

**query.rs**

- [ ] `test_handler_list_packages_empty_db`
- [ ] `test_handler_get_package_not_found`
- [ ] `test_handler_get_package_files_not_found`
- [ ] `test_handler_search_empty_results`
- [ ] `test_handler_search_no_query_param`
- [ ] `test_handler_depends_not_found`
- [ ] `test_handler_rdepends_empty`
- [ ] `test_handler_history_empty`
- [ ] `test_internal_errors_are_sanitized_for_clients`

**events.rs**

- [ ] `test_handler_events_rejects_when_sse_limit_reached`

**transactions.rs**

- [ ] `test_handler_list_transactions_empty`
- [ ] `test_handler_get_transaction_not_found`
- [ ] `test_handler_create_transaction_queues_package_jobs`
- [ ] `test_handler_create_transaction_empty_operations`
- [ ] `test_handler_create_transaction_rejects_mixed_package_kinds`
- [ ] `test_handler_create_transaction_invalid_json`
- [ ] `test_handler_create_transaction_forbidden`
- [ ] `test_handler_create_transaction_idempotency`
- [ ] `test_handler_enhance_idempotency`
- [ ] `test_handler_create_transaction_rejects_existing_enhance_idempotency_key`
- [ ] `test_handler_get_transaction_after_creation`
- [ ] `test_handler_list_transactions_with_status_filter`
- [ ] `test_handler_list_transactions_filters_by_requesting_uid`
- [ ] `test_handler_get_transaction_hides_foreign_job`
- [ ] `test_handler_transaction_stream_hides_foreign_job`
- [ ] `test_handler_cancel_transaction_not_found`
- [ ] `test_handler_cancel_transaction_hides_foreign_job`
- [ ] `test_package_routes_queue_package_jobs`
- [ ] `test_handler_dry_run_returns_package_summary`
- [ ] `test_handler_dry_run_empty_operations`

Total: 55 tests.

## Review Prompts

Use this prompt for Gemini/DeepSeek/local agentic review before lock-in:

```text
You are reviewing a repository-grounded Rust maintainability plan for Conary.

Repo: /home/peter/Conary
Plan file: docs/superpowers/plans/2026-06-09-project-maintainability-phase18-conaryd-routes-decomposition-plan.md
Target file: apps/conaryd/src/daemon/routes.rs

Please perform a critical review against the actual filesystem and code. Do not assume the plan is correct. Check Rust module resolution, visibility, import surfaces, test relocation, Axum route behavior, docs-audit math, and verification gates.

Important context:
- routes.rs is currently 2,345 lines and the top hotspot.
- Existing endpoint owner modules are apps/conaryd/src/daemon/routes/{events,query,system,transactions}.rs.
- scripts/check-doc-truth.sh extracts documented route declarations from apps/conaryd/src/daemon/routes/{system,transactions,query,events}.rs specifically, so endpoint declarations should stay in those owner modules and keep single-line .route("...", get|post|delete(...)) declarations unless the script is updated too.
- Baseline route tests: cargo test -p conaryd --lib daemon::routes::tests -- --list shows 55 tests.
- Baseline docs-audit count is 161 tracked files / 61 corrected rows before locking the plan; after adding the plan row it should become 162 / 62.

Please return:
1. Summary verdict: Ready, Ready with fixes, or Not ready.
2. Critical findings: compile failures, behavior regressions, broken public routes, auth/security regressions, or docs-audit failures.
3. Important findings: likely clippy/test/import/visibility issues or sequencing hazards.
4. Minor findings: clarity or polish.
5. Missing concerns the plan should cover.
6. Suggested exact edits to the plan.
7. Verification commands you ran and results.
8. Claims verified against code.
9. Claims not verified and why.

Focus especially on:
- Whether moving SharedState/DTOs/errors/auth/db/sse/router helpers into child modules keeps every existing child module compiling.
- Whether SseConnectionGuard visibility is sufficient for transactions.rs.
- Whether TransactionDetails::from_job and HistoryEntry::from_changeset_with_publication visibility is sufficient.
- Whether all 55 parent tests are assigned exactly once and receive the right imports after moving.
- Whether test_support.rs can be accessed from all child test modules under #[cfg(test)].
- Whether check-doc-truth remains valid after the split.
```

## Self-Review Checklist

- [ ] `routes.rs` stays a file module and keeps submodules under `routes/`; do not rename it to `routes/mod.rs`.
- [ ] `pub use router::build_router;` preserves public route construction.
- [ ] `TransactionOperation` remains reachable as `crate::daemon::routes::TransactionOperation`.
- [ ] Existing public route DTOs and `ApiError`/`ApiResult` are re-exported from `routes.rs`.
- [ ] Endpoint route declarations stay in `events.rs`, `query.rs`, `system.rs`, and `transactions.rs`.
- [ ] `scripts/check-doc-truth.sh` passes after every task that touches route declarations or docs.
- [ ] `SseConnectionGuard` is visible anywhere it is named.
- [ ] Route auth helpers preserve existing peer-credential and visibility semantics.
- [ ] `run_db_query` still uses `spawn_blocking` and still sanitizes internal errors for clients.
- [ ] No production child module keeps broad `use super::*;`.
- [ ] All 55 tests move exactly once.
- [ ] No parent `daemon::routes::tests::*` tests remain after Task 6.
- [ ] Docs-audit counts move from 161/61 to 162/62 when the plan is committed and remain 162/62 through implementation.
