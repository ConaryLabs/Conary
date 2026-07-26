---
last_updated: 2026-07-25
revision: 5
summary: Document conaryd authorization, exact-generation package jobs, routes, and daemon boundaries
---

# conaryd

`conaryd` is the local daemon for query routes, package job queueing, and SSE
events. It listens on the configured local socket and applies the same
apply-intent boundary as the CLI for package mutation jobs. Unimplemented
system-operation routes are absent rather than exposed as placeholders.

## Authorization

`GET /health` is outside the v1 auth gate so service managers can perform basic
liveness checks. `/v1/*` routes are behind the v1 gate. Query routes are
read-oriented. Package mutation and system operation routes require the daemon
authorization checks and still require explicit apply intent in request bodies
where the operation can mutate the host. Package requests send
`apply_intent: true`; removed acknowledgement aliases are rejected as unknown
request fields.

Root, the daemon identity, and members of the exact group passed through
`--socket-group` can perform daemon operations. At startup, conaryd resolves
that group once and fails if it does not exist; the same group owns the
mode-`0660` Unix socket and is checked against `SO_PEERCRED` plus the live
process supplementary-group list. With no configured group, the API is
root/daemon-only. There is no PolicyKit placeholder, broad authenticated-user
fallback, or implicit `sudo`/`wheel` distribution-group selection.

## Package Job Execution Boundary

Daemon install, remove, and update jobs are queued and tracked by `conaryd`, but
the package operation executor in `apps/conaryd/src/daemon/package_ops.rs`
currently calls the CLI command functions from the `conary` crate
(`cmd_install`, `cmd_remove`, and `cmd_update`). Changes to package-job behavior
therefore need both daemon-route/job proof and the owning CLI package-command
proof; `package_ops.rs` is the adapter boundary, not an independent package
manager implementation.

A mutating package operation is complete only when its exact selected-root
generation is published. If the package database commit succeeds but generation
publication leaves recoverable debt, the daemon reports the job as failed with
the persisted publication phase, failure detail, and exact retry command. It
must not label a database-only mutation as a completed package job. Package
publication never mutates the ambient root passed to the daemon; the new
generation is the execution result.

## Route Reference

The route list below is checked by `scripts/check-doc-truth.sh` against
`apps/conaryd/src/daemon/routes/{system,transactions,query,events}.rs`.

<!-- conaryd-routes:start -->
GET /health | Health check outside the v1 auth gate
GET /v1/version | Version and build metadata
GET /v1/metrics | Prometheus-style daemon metrics
GET /v1/transactions | List visible daemon jobs
POST /v1/transactions | Queue a daemon transaction job
POST /v1/transactions/dry-run | Preview a daemon transaction request
GET /v1/transactions/{id} | Get a visible daemon job
DELETE /v1/transactions/{id} | Cancel a visible daemon job
GET /v1/transactions/{id}/stream | Stream visible daemon job events
POST /v1/packages/install | Queue package install work
POST /v1/packages/remove | Queue package remove work
POST /v1/packages/update | Queue package update work
POST /v1/enhance | Queue enhancement work
GET /v1/packages | List packages
GET /v1/packages/{name} | Get package details
GET /v1/packages/{name}/files | List package files
GET /v1/search | Search package names
GET /v1/depends/{name} | List direct package dependencies
GET /v1/rdepends/{name} | List reverse package dependencies
GET /v1/history | List changeset history with publication status
GET /v1/events | Stream daemon events
<!-- conaryd-routes:end -->

Route implementation ownership: `apps/conaryd/src/daemon/routes.rs` is the
route hub; `daemon/config.rs` owns runtime configuration and canonical defaults;
`routes/router.rs` owns Axum assembly; `routes/types.rs` owns API DTOs;
`routes/errors.rs` owns API error conversion; `routes/auth.rs` owns route-level
auth and job/event visibility gates; `routes/db.rs` owns blocking DB query
plumbing; `routes/sse.rs` owns SSE connection guarding; and
`routes/{system,query,transactions,events}.rs` own endpoint declarations and
handlers.
