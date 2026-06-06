# conaryd

`conaryd` is the local daemon for query routes, package job queueing, SSE events,
and selected system-operation stubs. It listens on the configured local socket
and applies the same apply-intent boundary as the CLI for package mutation
jobs.

## Authorization

`GET /health` is outside the v1 auth gate so service managers can perform basic
liveness checks. `/v1/*` routes are behind the v1 gate. Query routes are
read-oriented. Package mutation and system operation routes require the daemon
authorization checks and still require explicit apply intent in request bodies
where the operation can mutate the host. New package requests should send
`apply_intent: true`; `allow_live_system_mutation: true` remains accepted as a
compatibility alias for existing clients during the migration window.

PolicyKit authorization is currently fail-closed. Root and the daemon identity
can perform daemon operations. Non-root PolicyKit write authorization is not
implemented until Conary has a real DBus check and policy-file contract.

## Route Reference

The route list below is checked by `scripts/check-doc-truth.sh` against
`apps/conaryd/src/daemon/routes/*.rs`.

<!-- conaryd-routes:start -->
GET /health | Health check outside the v1 auth gate
GET /v1/version | Version and build metadata
GET /v1/metrics | Prometheus-style daemon metrics
GET /v1/system/states | Preview stub: system state listing is not implemented in conaryd
POST /v1/system/rollback | Preview stub: rollback is not implemented in conaryd
POST /v1/system/verify | Preview stub: verification is not implemented in conaryd
POST /v1/system/gc | Preview stub: garbage collection is not implemented in conaryd
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
