#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

retired_provider_name() {
    printf '%s%s' 'super' 'powers'
}

retired_planning_root() {
    printf 'docs/%s' "$(retired_provider_name)"
}

assistant_history_root() {
    printf 'docs/llms/%s' 'archive'
}

deleted_validator_name() {
    printf '%s%s' 'check-doc-audit-' 'ledger.sh'
}

stage_fixture() {
    git -C "$1" add -A
}

make_good_repo() {
    local root="$1"

    mkdir -p \
        "$root/apps/conary/src/cli" \
        "$root/apps/conary" \
        "$root/apps/conaryd/src/daemon/routes" \
        "$root/apps/conaryd/src/daemon" \
        "$root/crates/conary-core/src/db" \
        "$root/crates/conary-core/src" \
        "$root/docs/guides" \
        "$root/docs/modules" \
        "$root/docs/operations" \
        "$root/docs/roadmaps" \
        "$root/site/src/lib" \
        "$root/site/src/routes/about" \
        "$root/site/src/routes/features" \
        "$root/site/src/routes/install" \
        "$root/web/src/routes/about"

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

Conary is still early. Expect failures.
Use a VM or disposable host first.
Scriptlet-heavy packages are expected to fail while adapter work continues.
If a package install fails, capture the command, distro, package name, Conary version, and refusal text.
Use `conary system adopt --refresh` to refresh adoption tracking.
Install the pinned preview release from v0.10.1.
EOF

    cat > "$root/docs/guides/agent-assisted-tester-loop.md" <<'EOF'
# Agent-Assisted Tester Loop

Use only the pinned v0.10.1 release from
https://github.com/ConaryLabs/Conary/releases/tag/v0.10.1.
Fedora artifact: conary-0.10.1-1.fc44.x86_64.rpm.
EOF

    cat > "$root/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

- conaryd package install/remove/update routes queue package jobs.
EOF

    cat > "$root/ROADMAP.md" <<'EOF'
# Roadmap

The preview remains adoption-led.
Remote Forge validation is paused pending a KVM-capable runner.
The 2026-05-21 Group O QEMU run is dated local evidence.
The 2026-05-21 Group P QEMU run is dated local evidence.
The current milestone is the first external tester loop.
See the [detailed development roadmap](docs/roadmaps/development-roadmap.md).
EOF

    cat > "$root/docs/roadmaps/development-roadmap.md" <<'EOF'
# Development Roadmap

The first external tester milestone is the current product milestone.
Remote Forge validation is paused pending a KVM-capable runner.
The 2026-05-21 Group O QEMU run is dated local evidence.
The 2026-05-21 Group P QEMU run is dated local evidence.
EOF

    cat > "$root/docs/roadmaps/external-tester-milestone.md" <<'EOF'
# External Tester Milestone

The currently pinned preview release is v0.10.1.
EOF

    cat > "$root/docs/operations/external-tester-outreach.md" <<'EOF'
# External Tester Outreach

Do not publish until release readiness is repinned.
The currently pinned preview release is v0.10.1.
EOF

    cat > "$root/docs/INTEGRATION-TESTING.md" <<'EOF'
# Integration Testing

Remote Forge control-plane validation is temporarily paused pending a KVM-capable runner.
Current Group O QEMU export evidence from 2026-05-21 is local evidence.
Current Group P ISO export evidence from 2026-05-21 is local evidence.
EOF

    cat > "$root/docs/operations/release-artifact-matrix.md" <<'EOF'
# Release Artifact Matrix

| Product | Source commit | Binary download or package URL | Required evidence |
| --- | --- | --- | --- |
| `conary` | `v0.10.1` | https://github.com/ConaryLabs/Conary/releases/tag/v0.10.1 | release-build green |
EOF

    cat > "$root/site/src/routes/about/+page.svelte" <<'EOF'
<section>
	<p>SQLite (schema version 69, DB-first runtime state)</p>
	<p>Generation builds produce EROFS images for complete-system rollback.</p>
</section>
EOF

    cat > "$root/site/src/lib/preview-release.ts" <<'EOF'
export const previewRelease = {
    tag: 'v0.10.1',
};
EOF

    cat > "$root/site/src/routes/install/+page.svelte" <<'EOF'
<section>
	<p>Start with the adoption-led limited preview on a VM or non-critical host.</p>
	<p>Remi cold-start conversion can make first package use slower.</p>
	<p>Use the pinned preview release.</p>
</section>
EOF

    cat > "$root/site/src/routes/features/+page.svelte" <<'EOF'
<section>
	<code>conary system generation build --summary test --yes</code>
	<code>conary system generation switch 2 --yes</code>
	<code>conary system generation rollback --yes</code>
	<code>conary system generation gc --keep 3 --yes</code>
	<code>conary model apply --dry-run</code>
	<code>conary model apply --yes</code>
	<p>Federation is outside the reliable limited-preview path.</p>
	<p>Hermetic mode is not a complete reproducibility or containment guarantee.</p>
</section>
EOF

    cat > "$root/web/src/routes/about/+page.svelte" <<'EOF'
<section>
	<p>Package operations and experimental generation artifacts have separate boundaries.</p>
	<code>conary install nginx --dry-run</code>
</section>
EOF

    cat > "$root/apps/conary/src/cli/mod.rs" <<'EOF'
pub enum Commands {
    Install,
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

    cat > "$root/apps/conary/Cargo.toml" <<'EOF'
[package]
name = "conary"
version = "0.10.1"
publish = false
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
        .route("/example", get(list_example_handler).post(create_example_handler))
        .route("/example/{id}", put(update_example_handler).patch(patch_example_handler).delete(delete_example_handler))
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
GET /v1/example | Example list
POST /v1/example | Example create
PUT /v1/example/{id} | Example update
PATCH /v1/example/{id} | Example patch
DELETE /v1/example/{id} | Example delete
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

    git -C "$root" init -q
    stage_fixture "$root"
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
    stage_fixture "$tmp"
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

break_cli_command_reference() {
    printf '\nUse `conary verify` for package verification.\n' >> "$1/README.md"
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
    sed -i 's/Conary is still early. Expect failures./Conary is production ready./' "$1/README.md"
}

break_root_roadmap_link() {
    sed -i '/detailed development roadmap/d' "$1/ROADMAP.md"
}

break_detailed_roadmap_milestone() {
    sed -i '/first external tester milestone/d' "$1/docs/roadmaps/development-roadmap.md"
}

break_tracker_release_version() {
    sed -i 's/v0.10.1/v0.9.2/g' "$1/docs/roadmaps/external-tester-milestone.md"
}

break_outreach_release_version() {
    sed -i 's/v0.10.1/v0.9.2/g' "$1/docs/operations/external-tester-outreach.md"
}

break_detailed_remote_forge_evidence() {
    sed -i '/Remote Forge validation/d' "$1/docs/roadmaps/development-roadmap.md"
}

break_detailed_group_o_evidence() {
    sed -i '/Group O/d' "$1/docs/roadmaps/development-roadmap.md"
}

break_detailed_group_p_evidence() {
    sed -i '/Group P/d' "$1/docs/roadmaps/development-roadmap.md"
}

break_release_doc_version() {
    sed -i 's/v0.10.1/v0.9.2/g' "$1/docs/guides/agent-assisted-tester-loop.md"
}

break_release_artifact_version() {
    sed -i 's/conary-0.10.1/conary-0.9.2/g' "$1/docs/guides/agent-assisted-tester-loop.md"
}

break_system_init_profile() {
    printf '\n```bash\nconary system init\n```\n' >> "$1/README.md"
}

break_site_release_version() {
    sed -i 's/v0.10.1/v0.9.2/g' "$1/site/src/lib/preview-release.ts"
}

break_site_generation_apply_intent() {
    sed -i 's/generation build --summary test --yes/generation build --summary test/' "$1/site/src/routes/features/+page.svelte"
}

break_site_federation_boundary() {
    sed -i 's/Federation is outside the reliable limited-preview path./Federation is ready for onboarding./' "$1/site/src/routes/features/+page.svelte"
}

break_package_index_every_operation_claim() {
    printf '\n<p>Every operation builds an EROFS image mounted with composefs.</p>\n' >> "$1/web/src/routes/about/+page.svelte"
}

break_package_index_install_intent() {
    sed -i 's/conary install nginx --dry-run/conary install nginx/' "$1/web/src/routes/about/+page.svelte"
}

break_package_index_live_install_privilege() {
    sed -i 's/conary install nginx --dry-run/conary install nginx --yes/' "$1/web/src/routes/about/+page.svelte"
}

break_conaryd_501_claim() {
    printf '\nconaryd package install/remove/update routes return `501 Not Implemented`.\n' >> "$1/CHANGELOG.md"
}

break_site_schema_version() {
    sed -i 's/schema version 69/schema version 65/' "$1/site/src/routes/about/+page.svelte"
}

break_every_install_erofs_claim() {
    printf '\n<p>Every install builds a new EROFS image and switches the composefs mount.</p>\n' >> "$1/site/src/routes/about/+page.svelte"
}

break_under_a_minute_claim() {
    printf '\n<p>Install Conary in under a minute.</p>\n' >> "$1/site/src/routes/install/+page.svelte"
}

break_atomic_takeover_claim() {
    printf '\n<p>Conary atomically takes over native packages during adoption.</p>\n' >> "$1/site/src/routes/install/+page.svelte"
}

break_required_scan_path() {
    rm -rf "$1/docs/operations"
}

break_retired_tracked_path() {
    local old_root
    old_root="$(retired_planning_root)"
    mkdir -p "$1/$old_root"
    printf '# Retired planning artifact\n' > "$1/$old_root/old-plan.md"
}

break_live_provider_reference() {
    printf '\nRetired provider brand: %s\n' "$(retired_provider_name)" >> "$1/README.md"
}

break_assistant_history_link() {
    local old_root
    old_root="$(assistant_history_root)"
    printf '\nSee %s/old-notes.md.\n' "$old_root" >> "$1/README.md"
}

break_deleted_validator_reference() {
    printf '\nRun scripts/%s before review.\n' "$(deleted_validator_name)" >> "$1/README.md"
}

break_mandatory_provider_skill() {
    printf '\nYou MUST use the %s:brainstorming skill before editing.\n' \
        "$(retired_provider_name)" >> "$1/README.md"
}

break_planning_archive_directory() {
    mkdir -p "$1/docs/plans/archive"
    printf '# Archived active plan\n' > "$1/docs/plans/archive/old-plan.md"
}

expect_pass
expect_failure "schema drift" break_schema_version 'schema.*68.*SCHEMA_VERSION.*69'
expect_failure "unknown CLI command reference" break_cli_command_reference 'unknown conary root command'
expect_failure "retired command doc" break_retired_command_doc 'retired command'
expect_failure "retired command parser" break_retired_command_parser 'retired command'
expect_failure "PolicyKit overclaim" break_policykit_claim 'PolicyKit'
expect_failure "require_polkit default" break_require_polkit_default 'require_polkit'
expect_failure "missing route doc" break_route_doc 'conaryd route'
expect_failure "missing core publish guard" break_core_publish_guard 'publish = false'
expect_failure "stable core API claim" break_core_api_claim 'stable.*conary-core'
expect_failure "preview status drift" break_preview_status 'early preview warning'
expect_failure "missing detailed roadmap link" break_root_roadmap_link 'detailed.*roadmap'
expect_failure "missing external tester milestone" break_detailed_roadmap_milestone 'first external tester milestone'
expect_failure "tracker release version drift" break_tracker_release_version 'stale conary release reference'
expect_failure "outreach release version drift" break_outreach_release_version 'stale conary release reference'
expect_failure "missing detailed remote Forge evidence" break_detailed_remote_forge_evidence 'remote Forge paused wording'
expect_failure "missing detailed Group O evidence" break_detailed_group_o_evidence 'dated Group O evidence'
expect_failure "missing detailed Group P evidence" break_detailed_group_p_evidence 'dated Group P evidence'
expect_failure "release doc version drift" break_release_doc_version 'stale conary release reference'
expect_failure "release artifact version drift" break_release_artifact_version 'stale conary release reference'
expect_failure "system init exact profile" break_system_init_profile 'system init without an exact --profile'
expect_failure "site release version drift" break_site_release_version 'stale conary release reference'
expect_failure "site generation apply intent" break_site_generation_apply_intent 'generation build apply-intent example'
expect_failure "site federation boundary" break_site_federation_boundary 'federation preview-boundary caveat'
expect_failure "package index every-operation claim" break_package_index_every_operation_claim 'public frontend every-operation generation/integrity claim'
expect_failure "package index install intent" break_package_index_install_intent 'active install without --dry-run or --yes'
expect_failure "package index live install privilege" break_package_index_live_install_privilege 'system-database install without sudo'
expect_failure "conaryd 501 claim" break_conaryd_501_claim 'claims conaryd package execution is still blanket 501'
expect_failure "site schema drift" break_site_schema_version 'schema.*65.*SCHEMA_VERSION.*69'
expect_failure "every install EROFS claim" break_every_install_erofs_claim 'every install builds an EROFS generation'
expect_failure "under a minute claim" break_under_a_minute_claim 'under-a-minute preview claim'
expect_failure "atomic takeover claim" break_atomic_takeover_claim 'atomically absorbed/taken over'
expect_failure "missing required scan path" break_required_scan_path 'required path is missing: docs/operations'
expect_failure "retired tracked path" break_retired_tracked_path 'neutral layout.*retired planning path'
expect_failure "live retired provider reference" break_live_provider_reference 'neutral layout.*retired provider reference'
expect_failure "assistant history archive link" break_assistant_history_link 'neutral layout.*assistant history archive'
expect_failure "deleted validator reference" break_deleted_validator_reference 'neutral layout.*deleted structural validator'
expect_failure "mandatory provider skill" break_mandatory_provider_skill 'neutral layout.*mandatory provider skill directive'
expect_failure "planning archive directory" break_planning_archive_directory 'neutral layout.*planning archive path'

echo "docs truth self-tests passed."
