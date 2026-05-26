#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

make_good_repo() {
    local root="$1"

    mkdir -p \
        "$root/apps/conary/src/cli" \
        "$root/apps/conaryd/src/daemon/routes" \
        "$root/apps/conaryd/src/daemon" \
        "$root/crates/conary-core/src/db" \
        "$root/crates/conary-core/src" \
        "$root/docs/modules" \
        "$root/docs/operations"

    cat > "$root/crates/conary-core/src/db/schema.rs" <<'EOF'
/// Current schema version
pub const SCHEMA_VERSION: i32 = 69;
EOF

    cat > "$root/docs/ARCHITECTURE.md" <<'EOF'
# Architecture

The database layer uses Schema v69.

`conary-core` is an internal workspace crate, not a stable external API.
EOF

    cat > "$root/docs/conaryopedia-v2.md" <<'EOF'
# Conaryopedia

The local SQLite database is currently schema v69.
EOF

    cat > "$root/README.md" <<'EOF'
# Conary

Conary is being prepared as an adoption-led limited public preview.
Remote Forge validation is paused pending a KVM-capable runner.
The 2026-05-21 Group O local QEMU evidence is recorded.
The 2026-05-21 Group P local QEMU evidence is recorded.
EOF

    cat > "$root/ROADMAP.md" <<'EOF'
# Roadmap

The preview remains adoption-led.
Remote Forge validation is paused pending a KVM-capable runner.
The 2026-05-21 Group O QEMU run is dated local evidence.
The 2026-05-21 Group P QEMU run is dated local evidence.
EOF

    cat > "$root/docs/INTEGRATION-TESTING.md" <<'EOF'
# Integration Testing

Remote Forge control-plane validation is temporarily paused pending a KVM-capable runner.
Current Group O QEMU export evidence from 2026-05-21 is local evidence.
Current Group P ISO export evidence from 2026-05-21 is local evidence.
EOF

    cat > "$root/apps/conary/src/cli/mod.rs" <<'EOF'
pub enum Commands {
    System,
}
EOF

    cat > "$root/apps/conary/src/dispatch.rs" <<'EOF'
pub fn dispatch() {}
EOF

    cat > "$root/apps/conary/src/command_risk.rs" <<'EOF'
pub fn classify() {}
EOF

    cat > "$root/apps/conaryd/src/daemon/auth.rs" <<'EOF'
//! Authentication and authorization for the daemon.
//!
//! PolicyKit authorization is currently an unimplemented fail-closed stub.
//! Non-root write operations are denied until a real DBus check and policy-file
//! contract exist.
EOF

    cat > "$root/apps/conaryd/src/daemon/mod.rs" <<'EOF'
pub struct DaemonConfig {
    pub require_polkit: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            require_polkit: true,
        }
    }
}
EOF

    cat > "$root/crates/conary-core/Cargo.toml" <<'EOF'
[package]
name = "conary-core"
version = "0.8.0"
publish = false
EOF

    cat > "$root/crates/conary-core/src/lib.rs" <<'EOF'
//! Conary Core Library
//!
//! Internal workspace crate. The broad module exports are not a stable external
//! public API.
EOF

    cat > "$root/apps/conaryd/src/daemon/routes/system.rs" <<'EOF'
pub(super) fn root_router() -> Router<SharedState> {
    Router::new().route("/health", get(health_handler))
}

pub(super) fn v1_router() -> Router<SharedState> {
    Router::new()
        .route("/version", get(version_handler))
        .route("/metrics", get(metrics_handler))
        .route("/system/states", get(list_states_handler))
        .route("/system/rollback", post(rollback_handler))
        .route("/system/verify", post(verify_handler))
        .route("/system/gc", post(gc_handler))
}
EOF

    cat > "$root/apps/conaryd/src/daemon/routes/transactions.rs" <<'EOF'
pub(super) fn router() -> Router<SharedState> {
    Router::new()
        .route("/transactions", get(list_transactions_handler))
        .route("/transactions", post(create_transaction_handler))
        .route("/transactions/dry-run", post(dry_run_handler))
        .route("/transactions/{id}", get(get_transaction_handler))
        .route("/transactions/{id}", delete(cancel_transaction_handler))
        .route("/transactions/{id}/stream", get(transaction_stream_handler))
        .route("/packages/install", post(install_packages_handler))
        .route("/packages/remove", post(remove_packages_handler))
        .route("/packages/update", post(update_packages_handler))
        .route("/enhance", post(enhance_handler))
}
EOF

    cat > "$root/apps/conaryd/src/daemon/routes/query.rs" <<'EOF'
pub(super) fn router() -> Router<SharedState> {
    Router::new()
        .route("/packages", get(list_packages_handler))
        .route("/packages/{name}", get(get_package_handler))
        .route("/packages/{name}/files", get(get_package_files_handler))
        .route("/search", get(search_handler))
        .route("/depends/{name}", get(depends_handler))
        .route("/rdepends/{name}", get(rdepends_handler))
        .route("/history", get(history_handler))
}
EOF

    cat > "$root/apps/conaryd/src/daemon/routes/events.rs" <<'EOF'
pub(super) fn router() -> Router<SharedState> {
    Router::new().route("/events", get(events_handler))
}
EOF

    cat > "$root/docs/modules/conaryd.md" <<'EOF'
# conaryd

`/health` is outside the v1 auth gate. `/v1/*` routes are behind the v1 gate.

<!-- conaryd-routes:start -->
GET /health | Health check
GET /v1/version | Version info
GET /v1/metrics | Metrics
GET /v1/system/states | Preview stub
POST /v1/system/rollback | Preview stub
POST /v1/system/verify | Preview stub
POST /v1/system/gc | Preview stub
GET /v1/transactions | List jobs
POST /v1/transactions | Create job
POST /v1/transactions/dry-run | Dry-run job
GET /v1/transactions/{id} | Get job
DELETE /v1/transactions/{id} | Cancel job
GET /v1/transactions/{id}/stream | Stream job
POST /v1/packages/install | Queue install
POST /v1/packages/remove | Queue remove
POST /v1/packages/update | Queue update
POST /v1/enhance | Queue enhance
GET /v1/packages | List packages
GET /v1/packages/{name} | Package detail
GET /v1/packages/{name}/files | Package files
GET /v1/search | Search packages
GET /v1/depends/{name} | Dependencies
GET /v1/rdepends/{name} | Reverse dependencies
GET /v1/history | Changeset history
GET /v1/events | SSE events
<!-- conaryd-routes:end -->
EOF
}

run_truth() {
    local root="$1"
    DOCS_TRUTH_ROOT="$root" bash "$repo_root/scripts/check-doc-truth.sh"
}

expect_pass() {
    local tmp
    tmp="$(mktemp -d)"
    make_good_repo "$tmp"
    run_truth "$tmp" > "$tmp/out" 2>&1 || {
        cat "$tmp/out" >&2
        rm -rf "$tmp"
        fail "expected good fixture to pass"
    }
    rm -rf "$tmp"
}

expect_failure() {
    local name="$1"
    local mutator="$2"
    local expected="$3"
    local tmp
    tmp="$(mktemp -d)"
    make_good_repo "$tmp"
    "$mutator" "$tmp"
    if run_truth "$tmp" > "$tmp/out" 2>&1; then
        cat "$tmp/out" >&2
        rm -rf "$tmp"
        fail "expected $name fixture to fail"
    fi
    grep -Eq "$expected" "$tmp/out" || {
        cat "$tmp/out" >&2
        rm -rf "$tmp"
        fail "expected $name failure to match: $expected"
    }
    rm -rf "$tmp"
}

break_schema_version() {
    printf '# Architecture\n\nThe database layer uses Schema v68.\n' > "$1/docs/ARCHITECTURE.md"
}

break_retired_command_doc() {
    printf '\nRun conary adopt nginx\n' >> "$1/README.md"
}

break_retired_command_parser() {
    printf '#[command(alias = "adopt-system")]\npub struct Adopt;\n' > "$1/apps/conary/src/cli/mod.rs"
}

break_policykit_claim() {
    cat > "$1/apps/conaryd/src/daemon/auth.rs" <<'EOF'
//! Non-root users can be authorized via PolicyKit for specific operations.
EOF
}

break_require_polkit_default() {
    sed -i 's/require_polkit: true/require_polkit: false/' "$1/apps/conaryd/src/daemon/mod.rs"
}

break_route_doc() {
    grep -v 'GET /v1/events' "$1/docs/modules/conaryd.md" > "$1/docs/modules/conaryd.md.tmp"
    mv "$1/docs/modules/conaryd.md.tmp" "$1/docs/modules/conaryd.md"
}

break_core_publish_guard() {
    grep -v '^publish = false$' "$1/crates/conary-core/Cargo.toml" > "$1/crates/conary-core/Cargo.toml.tmp"
    mv "$1/crates/conary-core/Cargo.toml.tmp" "$1/crates/conary-core/Cargo.toml"
}

break_core_api_claim() {
    printf '\nconary-core provides a stable public API for external integrations.\n' >> "$1/README.md"
}

break_preview_status() {
    sed -i 's/adoption-led/feature-rich/' "$1/README.md"
}

expect_pass
expect_failure "schema drift" break_schema_version 'schema.*68.*SCHEMA_VERSION.*69'
expect_failure "retired command doc" break_retired_command_doc 'retired command'
expect_failure "retired command parser" break_retired_command_parser 'retired command'
expect_failure "PolicyKit overclaim" break_policykit_claim 'PolicyKit'
expect_failure "require_polkit default" break_require_polkit_default 'require_polkit'
expect_failure "missing route doc" break_route_doc 'conaryd route'
expect_failure "missing core publish guard" break_core_publish_guard 'publish = false'
expect_failure "stable core API claim" break_core_api_claim 'stable.*conary-core'
expect_failure "preview status drift" break_preview_status 'adoption-led'

echo "docs truth self-tests passed."
