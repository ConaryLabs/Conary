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

add_fixture_doc_frontmatter() {
    local root="$1"
    local file tmp

    while IFS= read -r file; do
        [[ "$(sed -n '1p' "$file")" == "---" ]] && continue
        tmp="${file}.frontmatter"
        {
            printf '%s\n' \
                '---' \
                'last_updated: 2026-07-25' \
                'revision: 1' \
                'summary: Fixture document for documentation-truth self-tests' \
                '---' \
                ''
            sed -n '1,$p' "$file"
        } > "$tmp"
        mv "$tmp" "$file"
    done < <(find "$root/docs" -type f -name '*.md' | sort)
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
        "$root/docs/llms" \
        "$root/docs/modules" \
        "$root/docs/operations" \
        "$root/docs/roadmaps" \
        "$root/site/src/lib" \
        "$root/site/src/routes/about" \
        "$root/site/src/routes/features" \
        "$root/site/src/routes/install" \
        "$root/web/src/routes/about"

    cat > "$root/Cargo.toml" <<'EOF'
[workspace]
members = ["apps/conary", "crates/conary-core"]

[workspace.package]
version = "0.10.1"
publish = false
EOF

    cat > "$root/crates/conary-core/src/db/schema.rs" <<'EOF'
/// Current schema version
pub const SCHEMA_VERSION: i32 = 69;
EOF

    cat > "$root/docs/ARCHITECTURE.md" <<'EOF'
# Architecture

The database layer uses Schema v69.

`conary-core` is an internal workspace crate, not a stable external API.
EOF

    cat > "$root/README.md" <<'EOF'
# Conary

[![Latest release](https://img.shields.io/github/v/release/ConaryLabs/Conary?label=release)](https://github.com/ConaryLabs/Conary/releases/latest)

Conary is still early. Expect failures.
Use a VM or disposable host first.
The primary adoption path is cross-distro package installation.
The source package format defines the package ABI.
If a package install fails, capture the command, distro, package name, Conary version, and refusal text.
Use `conary system adopt --refresh` to refresh adoption tracking.
Inspect the latest immutable GitHub release.

## Release Channels

| Channel | Current state | Authority |
| --- | --- | --- |
| Development head | Root [`Cargo.toml`](Cargo.toml) `[workspace.package]` version | Repository source authority |
| Latest published, artifact-verified release | [Latest immutable GitHub release](https://github.com/ConaryLabs/Conary/releases/latest) | [Release artifact matrix](docs/operations/release-artifact-matrix.md) |
| Current external tester pin | **None** | Paused until the corpus gate completes |
EOF

    cat > "$root/SECURITY.md" <<'EOF'
# Security Policy

## Supported Versions

| Version | Supported |
| --- | --- |
| [Latest immutable preview release](https://github.com/ConaryLabs/Conary/releases/latest) | Yes |
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

The cross-distro package-installation preview is active.
Remote Forge validation and conary-test deployment are decommissioned.
The 2026-07-31 Group O QEMU run is dated local evidence.
The 2026-07-31 Group P QEMU run is dated local evidence.
The current milestone is the first external tester loop.
See the [detailed development roadmap](docs/roadmaps/development-roadmap.md).
EOF

    cat > "$root/docs/roadmaps/development-roadmap.md" <<'EOF'
# Development Roadmap

The first external tester milestone is the current product milestone.
Remote Forge validation and conary-test deployment are decommissioned.
The 2026-07-31 Group O QEMU run is dated local evidence.
The 2026-07-31 Group P QEMU run is dated local evidence.
EOF

    cat > "$root/docs/roadmaps/external-tester-milestone.md" <<'EOF'
# External Tester Milestone

The publication gate for synchronized suite `v0.10.1` is complete.
EOF

    cat > "$root/docs/operations/external-tester-outreach.md" <<'EOF'
---
last_updated: 2026-07-25
revision: 1
status: postponed
target_release: v0.11.0
summary: Fixture postponed launch packet
---

# External Tester Outreach

Do not publish until release readiness is repinned.
The historical safety baseline is v0.10.1.
The intended release is v0.11.0.
https://github.com/ConaryLabs/Conary/releases/tag/v0.11.0
- [ ] Publish immutable `v0.11.0`.
EOF

    cat > "$root/docs/INTEGRATION-TESTING.md" <<'EOF'
# Integration Testing

Remote Forge control-plane validation and conary-test deployment are decommissioned; there is no replacement Forge rollout path.
Current Group O QEMU export evidence from 2026-07-31 is local evidence.
Current Group P ISO export evidence from 2026-07-31 is local evidence.
EOF

    cat > "$root/docs/operations/release-artifact-matrix.md" <<'EOF'
# Release Artifact Matrix

Version `0.10.1` is the current immutable release authority.

| Product | Source commit | Binary download or package URL | Required evidence |
| --- | --- | --- | --- |
| `conary` | `v0.10.1` | https://github.com/ConaryLabs/Conary/releases/tag/v0.10.1 | release-build green |
| `remi` | `remi-v0.7.0` | https://github.com/ConaryLabs/Conary/releases/tag/remi-v0.7.0 | independent service release |
EOF

    cat > "$root/site/src/routes/about/+page.svelte" <<'EOF'
<section>
	<p>SQLite (schema version 69, DB-first runtime state)</p>
	<p>Generation builds produce EROFS images for complete-system rollback.</p>
</section>
EOF

    cat > "$root/site/src/lib/preview-release.ts" <<'EOF'
const version = '0.10.1';

export const previewRelease = {
    version,
    tag: `v${version}`,
    asset: `conary-${version}.ccs`,
};
EOF

    cat > "$root/site/src/routes/install/+page.svelte" <<'EOF'
<section>
	<p>Start with the cross-distro limited preview on a VM or non-critical host.</p>
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
//! Root users, the daemon service identity, and the configured Unix socket group
//! have full access. Every other peer is denied.
EOF

    cat > "$root/apps/conaryd/src/daemon/config.rs" <<'EOF'
pub struct DaemonConfig {
    pub socket_mode: u32,
    pub socket_group: Option<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_mode: Self::DEFAULT_SOCKET_MODE,
            socket_group: None,
        }
    }
}

impl DaemonConfig {
    pub const DEFAULT_SOCKET_MODE: u32 = 0o660;
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
publish.workspace = true
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
Unimplemented system-operation routes are absent rather than exposed as placeholders.

Root, the daemon identity, and members of the exact group passed through
`--socket-group` can perform daemon operations. There is no PolicyKit placeholder.

<!-- conaryd-routes:start -->
GET /health | Health check
GET /v1/version | Version info
GET /v1/metrics | Metrics
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

    cat > "$root/AGENTS.md" <<'EOF'
# Repository Guidelines

Current roadmap state lives under `docs/roadmaps/`.
EOF

    cat > "$root/CONTRIBUTING.md" <<'EOF'
# Contributing

Stable contracts live under `docs/specs/`.
EOF

    cat > "$root/docs/llms/README.md" <<'EOF'
# Assistant Map

Subsystem routing lives under `docs/modules/`.
EOF

    cat > "$root/docs/llms/subsystem-map.md" <<'EOF'
# Subsystem Map

Canonical specifications live under `docs/specs/`.
EOF

    cat > "$root/docs/modules/feature-ownership.md" <<'EOF'
# Feature Ownership

Roadmap ordering lives under `docs/roadmaps/`.
EOF

    mkdir -p "$root/docs/specs"
    add_fixture_doc_frontmatter "$root"
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

expect_unassigned_outreach_pass() {
    local tmp
    tmp="$(mktemp -d)"
    make_good_repo "$tmp"
    sed -i \
        -e 's/target_release: v0.11.0/target_release: unassigned/' \
        -e 's/v0.11.0/v0.10.1/g' \
        "$tmp/docs/operations/external-tester-outreach.md"
    printf '\nNo new release is assigned.\n' >> "$tmp/docs/operations/external-tester-outreach.md"
    stage_fixture "$tmp"
    run_truth "$tmp" > "$tmp/out" 2>&1 || {
        cat "$tmp/out" >&2
        rm -rf "$tmp"
        fail "expected unassigned outreach fixture to pass"
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

break_socket_group_default() {
    sed -i 's/socket_group: None/socket_group: Some("wheel".to_string())/' "$1/apps/conaryd/src/daemon/config.rs"
}

break_route_doc() {
    grep -v 'GET /v1/events' "$1/docs/modules/conaryd.md" > "$1/docs/modules/conaryd.md.tmp"
    mv "$1/docs/modules/conaryd.md.tmp" "$1/docs/modules/conaryd.md"
}

break_core_publish_guard() {
    grep -v '^publish\.workspace = true$' "$1/crates/conary-core/Cargo.toml" > "$1/crates/conary-core/Cargo.toml.tmp"
    mv "$1/crates/conary-core/Cargo.toml.tmp" "$1/crates/conary-core/Cargo.toml"
}

break_workspace_publish_guard() {
    sed -i 's/^publish = false$/publish = true/' "$1/Cargo.toml"
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

break_outreach_target_state() {
    sed -i 's/target_release: v0.11.0/target_release: later/' "$1/docs/operations/external-tester-outreach.md"
}

break_unassigned_outreach_candidate_version() {
    sed -i 's/target_release: v0.11.0/target_release: unassigned/' "$1/docs/operations/external-tester-outreach.md"
    printf '\nNo new release is assigned.\n' >> "$1/docs/operations/external-tester-outreach.md"
}

break_detailed_forge_retirement_evidence() {
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

break_readme_release_channel() {
    sed -i 's#github.com/ConaryLabs/Conary/releases/latest#github.com/ConaryLabs/Conary/releases/tag/v0.9.2#g' "$1/README.md"
}

break_security_release_version() {
    sed -i 's#github.com/ConaryLabs/Conary/releases/latest#github.com/ConaryLabs/Conary/releases/tag/v0.9.2#' "$1/SECURITY.md"
}

break_release_artifact_version() {
    sed -i 's/conary-0.10.1/conary-0.9.2/g' "$1/docs/guides/agent-assisted-tester-loop.md"
}

break_system_init_profile() {
    printf '\n```bash\nconary system init --profile fedora-44\n```\n' >> "$1/README.md"
}

break_site_release_version() {
    sed -i "s/const version = '0.10.1'/const version = '0.9.2'/" "$1/site/src/lib/preview-release.ts"
}

break_site_release_version_duplication() {
    sed -i 's/tag: `v${version}`/tag: '\''v0.10.1'\''/' "$1/site/src/lib/preview-release.ts"
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

break_retired_plan_directory() {
    mkdir -p "$1/docs/plans"
    printf '# Retired active plan location\n' > "$1/docs/plans/example-plan.md"
}

break_retired_design_directory() {
    mkdir -p "$1/docs/designs"
    printf '# Retired active design location\n' > "$1/docs/designs/example-design.md"
}

break_live_doc_location_claim() {
    printf '\nActive designs live under `docs/missing-live-location/`.\n' >> "$1/AGENTS.md"
}

break_frontmatter_revision() {
    sed -i '/^revision:/d' "$1/docs/ARCHITECTURE.md"
}

expect_pass
expect_unassigned_outreach_pass
expect_failure "schema drift" break_schema_version 'schema.*68.*SCHEMA_VERSION.*69'
expect_failure "unknown CLI command reference" break_cli_command_reference 'unknown conary root command'
expect_failure "retired command doc" break_retired_command_doc 'retired command'
expect_failure "retired command parser" break_retired_command_parser 'retired command'
expect_failure "PolicyKit overclaim" break_policykit_claim 'PolicyKit'
expect_failure "socket-group default" break_socket_group_default 'root/daemon-only default'
expect_failure "missing route doc" break_route_doc 'conaryd route'
expect_failure "missing core publish guard" break_core_publish_guard 'inherit.*publication policy'
expect_failure "publishable workspace root" break_workspace_publish_guard 'disable registry publication'
expect_failure "stable core API claim" break_core_api_claim 'stable.*conary-core'
expect_failure "preview status drift" break_preview_status 'early preview warning'
expect_failure "missing detailed roadmap link" break_root_roadmap_link 'detailed.*roadmap'
expect_failure "missing external tester milestone" break_detailed_roadmap_milestone 'first external tester milestone'
expect_failure "tracker release version drift" break_tracker_release_version 'stale conary release reference|completed publication gate'
expect_failure "outreach release version drift" break_outreach_release_version 'current-release baseline|outside current/target contract'
expect_failure "outreach target state" break_outreach_target_state 'exact vMAJOR.MINOR.PATCH tag or unassigned'
expect_failure "unassigned outreach candidate version" break_unassigned_outreach_candidate_version 'outside current/target contract|outside retained current release'
expect_failure "missing Forge deployment retirement evidence" break_detailed_forge_retirement_evidence 'Forge deployment retirement wording'
expect_failure "missing detailed Group O evidence" break_detailed_group_o_evidence 'dated Group O evidence'
expect_failure "missing detailed Group P evidence" break_detailed_group_p_evidence 'dated Group P evidence'
expect_failure "release doc version drift" break_release_doc_version 'stale conary release reference'
expect_failure "README release channel drift" break_readme_release_channel 'hard-codes a public release version|derived published-release channel'
expect_failure "security release version drift" break_security_release_version 'hard-codes a public release version|derived supported-release row'
expect_failure "release artifact version drift" break_release_artifact_version 'stale conary release reference'
expect_failure "system init source independence" break_system_init_profile 'system init depend on a host distro profile'
expect_failure "site release version drift" break_site_release_version 'stale conary release reference'
expect_failure "site release version duplication" break_site_release_version_duplication 'duplicates the site published-release version'
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
expect_failure "retired plan directory" break_retired_plan_directory 'neutral layout.*retired design/plan path'
expect_failure "retired design directory" break_retired_design_directory 'neutral layout.*retired design/plan path'
expect_failure "missing live documentation directory" break_live_doc_location_claim 'names missing live documentation directory'
expect_failure "missing frontmatter revision" break_frontmatter_revision 'frontmatter requires a positive integer revision'

echo "docs truth self-tests passed."
