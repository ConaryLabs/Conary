# Docs And CI Truth Checks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Plan C from the preview invariant hardening milestone: active docs, command hints, conaryd route references, and public-surface claims must fail CI when they drift from current code.

**Architecture:** Add one cheap always-on docs-truth PR gate. The gate runs existing docs-audit inventory/ledger checks plus a new Bash truth script with self-tests. The truth script encodes narrow invariants for schema version mentions, retired commands, preview status, PolicyKit fail-closed behavior, conaryd route docs, and the internal `conary-core` crate stance.

**Tech Stack:** Bash, ripgrep, awk, GitHub Actions, docs-audit TSV files, Rust crate metadata, Markdown docs.

---

## Scope

This plan implements `docs/superpowers/specs/2026-05-26-docs-ci-truth-checks-design.md`.

Plan C is the final slice of the preview invariant hardening umbrella. It should not add package-manager features. It adds guardrails that make stale docs and public-surface overclaims fail fast.

## Goal-Oriented Execution

Start implementation in a fresh worktree and create a Codex goal before editing:

```text
/goal Implement Plan C docs and CI truth checks from docs/superpowers/plans/2026-05-26-docs-ci-truth-checks.md
```

Use a short objective like:

```text
Implement Plan C docs/CI truth checks, wire them into PR gate, update active docs, archive completed invariant-hardening docs, and leave main ready to merge.
```

The implementation goal is complete only when:

- `scripts/check-doc-truth.sh` exists and passes;
- `scripts/test-doc-truth.sh` exercises passing and failing fixtures;
- `.github/workflows/pr-gate.yml` has an always-on `docs-truth` job;
- conaryd docs and auth wording match current behavior;
- `conary-core` is marked internal and `publish = false`;
- completed Plan A/B/C and umbrella docs are archived;
- docs-audit inventory and ledger pass;
- final workspace verification passes.

## Review-Tightened Decisions

- Use Bash for the truth gate. Do not add a Rust or Python helper for Plan C.
- Schema drift checks are allowlisted to active schema declaration docs, initially `docs/ARCHITECTURE.md` and `docs/conaryopedia-v2.md`.
- Retired command checks cover active docs, active operator-facing strings, and Clap parser source so hidden aliases cannot reintroduce old spellings.
- conaryd route checks compare `METHOD /path` pairs, not bare paths.
- conaryd route extraction must find at least 25 method/path pairs, matching the current route surface.
- `/health` is outside the v1 auth gate; `/v1/*` routes are behind the v1 middleware.
- PolicyKit remains fail-closed; `DaemonConfig::default()` must keep `require_polkit: true`.
- `conary-core` is an internal workspace crate. Set `publish = false` and document that broad module exports are not a stable external API.
- Keep docs-audit inventory strict for every tracked doc path. Do not weaken it for root, app-local, frontend, or planning docs.
- `ripgrep` is an existing repository tool dependency: current scripts such as `scripts/docs-audit-inventory.sh` and `scripts/check-release-matrix.sh` already require it. Plan C should not add a new package-install step to the PR gate, but the truth script should fail clearly if `rg` is unavailable.

## File Structure

- Create `scripts/check-doc-truth.sh`: strict docs/code truth checker.
- Create `scripts/test-doc-truth.sh`: fixture-based self-test runner for the truth checker.
- Modify `.github/workflows/pr-gate.yml`: add always-on `docs-truth` job.
- Create `docs/modules/conaryd.md`: focused conaryd daemon and endpoint reference with machine-checkable route list.
- Modify `README.md`: point conaryd readers at `docs/modules/conaryd.md`.
- Modify `apps/conaryd/src/daemon/auth.rs`: describe PolicyKit as fail-closed and unimplemented.
- Modify `apps/conaryd/src/daemon/mod.rs`: clarify `require_polkit` while keeping the default true.
- Modify `crates/conary-core/Cargo.toml`: add `publish = false`.
- Modify `crates/conary-core/src/lib.rs`: document internal workspace-crate status.
- Modify `docs/ARCHITECTURE.md`: document the internal `conary-core` public-surface decision.
- Modify docs-audit inventory and ledger files.
- Final archive step moves completed Plan A/B/C and umbrella docs to archive paths.

---

### Task 0: Worktree And Goal Setup

**Files:**
- Read: `docs/superpowers/specs/2026-05-26-docs-ci-truth-checks-design.md`
- Read: `docs/superpowers/plans/2026-05-26-docs-ci-truth-checks.md`

- [ ] **Step 1: Create an isolated implementation worktree**

Run from `/home/peter/Conary`:

```bash
git status --short --branch
git worktree add .worktrees/plan-c-docs-truth -b plan-c-docs-truth
cd .worktrees/plan-c-docs-truth
```

Expected:

```text
Preparing worktree (new branch 'plan-c-docs-truth')
```

- [ ] **Step 2: Start the Codex goal**

In Codex, run:

```text
/goal Implement Plan C docs and CI truth checks from docs/superpowers/plans/2026-05-26-docs-ci-truth-checks.md
```

- [ ] **Step 3: Confirm the worktree is clean**

Run:

```bash
git status --short --branch
```

Expected:

```text
## plan-c-docs-truth
```

---

### Task 1: Truth Script And Fixture Tests

**Files:**
- Create: `scripts/check-doc-truth.sh`
- Create: `scripts/test-doc-truth.sh`

- [ ] **Step 1: Add the fixture self-test runner first**

Create `scripts/test-doc-truth.sh` with this complete body:

```bash
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
    trap 'rm -rf "$tmp"' RETURN
    make_good_repo "$tmp"
    run_truth "$tmp" > "$tmp/out" 2>&1 || {
        cat "$tmp/out" >&2
        fail "expected good fixture to pass"
    }
}

expect_failure() {
    local name="$1"
    local mutator="$2"
    local expected="$3"
    local tmp
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' RETURN
    make_good_repo "$tmp"
    "$mutator" "$tmp"
    if run_truth "$tmp" > "$tmp/out" 2>&1; then
        cat "$tmp/out" >&2
        fail "expected $name fixture to fail"
    fi
    grep -Eq "$expected" "$tmp/out" || {
        cat "$tmp/out" >&2
        fail "expected $name failure to match: $expected"
    }
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
```

- [ ] **Step 2: Make the self-test executable**

Run:

```bash
chmod +x scripts/test-doc-truth.sh
```

- [ ] **Step 3: Run the self-test to confirm it fails before the truth script exists**

Run:

```bash
bash scripts/test-doc-truth.sh
```

Expected: failure mentioning `scripts/check-doc-truth.sh` is missing or not executable.

- [ ] **Step 4: Add the truth checker script**

Create `scripts/check-doc-truth.sh` with this complete body:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="${DOCS_TRUTH_ROOT:-}"
if [[ -z "$repo_root" ]]; then
    repo_root="$(git rev-parse --show-toplevel)"
fi
cd "$repo_root"

errors=0

if ! command -v rg >/dev/null 2>&1; then
    echo "ERROR: ripgrep (rg) is required for docs truth checks" >&2
    exit 1
fi

DOCS_TRUTH_SCHEMA_CHECK_PATHS=(
    "docs/ARCHITECTURE.md"
    "docs/conaryopedia-v2.md"
)

PRODUCT_DOC_PATHS=(
    "README.md"
    "ROADMAP.md"
    "docs/ARCHITECTURE.md"
    "docs/conaryopedia-v2.md"
    "docs/modules"
    "docs/operations"
)

POLICYKIT_DOC_PATHS=(
    "README.md"
    "ROADMAP.md"
    "docs/ARCHITECTURE.md"
    "docs/conaryopedia-v2.md"
    "docs/modules"
    "docs/operations"
)

PARSER_PATHS=(
    "apps/conary/src/cli"
    "apps/conary/src/dispatch.rs"
    "apps/conary/src/command_risk.rs"
)

report_error() {
    echo "ERROR: $*" >&2
    errors=1
}

existing_paths() {
    local path
    for path in "$@"; do
        if [[ -e "$path" ]]; then
            printf '%s\n' "$path"
        fi
    done
}

require_file() {
    local path="$1"
    if [[ ! -f "$path" ]]; then
        report_error "required file is missing: $path"
        return 1
    fi
}

require_match() {
    local path="$1"
    local pattern="$2"
    local description="$3"

    if [[ ! -e "$path" ]]; then
        report_error "$path: missing while checking $description"
        return
    fi

    if ! rg -q -- "$pattern" "$path"; then
        report_error "$path: missing $description"
    fi
}

check_schema_versions() {
    local schema_file="crates/conary-core/src/db/schema.rs"
    require_file "$schema_file" || return

    local schema_version
    schema_version="$(sed -nE 's/^pub const SCHEMA_VERSION: i32 = ([0-9]+);/\1/p' "$schema_file")"
    if [[ -z "$schema_version" ]]; then
        report_error "$schema_file: could not parse SCHEMA_VERSION"
        return
    fi

    local schema_pattern='([Ss]chema[ \t]+\(v|[Ss]chema[ \t]+v|currently[ \t]+schema[ \t]+v|schema[ \t]+version[ \t]+)([0-9]+)'
    local path line_no text found
    for path in "${DOCS_TRUTH_SCHEMA_CHECK_PATHS[@]}"; do
        require_file "$path" || continue
        while IFS=: read -r _ line_no text; do
            if [[ "$text" =~ $schema_pattern ]]; then
                found="${BASH_REMATCH[2]}"
                if [[ "$found" != "$schema_version" ]]; then
                    report_error "$path:$line_no mentions schema $found but SCHEMA_VERSION is $schema_version"
                fi
            fi
        done < <(rg -n -- "$schema_pattern" "$path" || true)
    done
}

check_retired_commands() {
    local retired_pattern='adopt-system|conary[ \t]+adopt|conary-adopt|system-adopt'
    local paths=()
    local path

    while IFS= read -r path; do
        paths+=("$path")
    done < <(existing_paths "${PRODUCT_DOC_PATHS[@]}" "${PARSER_PATHS[@]}")

    if [[ "${#paths[@]}" -eq 0 ]]; then
        report_error "retired command check had no paths to scan"
        return
    fi

    local match file line_no text
    while IFS=: read -r file line_no text; do
        case "$file" in
            scripts/check-doc-truth.sh|scripts/test-doc-truth.sh|*/archive/*)
                continue
                ;;
        esac
        report_error "$file:$line_no contains retired command spelling: $text"
    done < <(rg -n -- "$retired_pattern" "${paths[@]}" || true)
}

check_preview_status() {
    require_match "README.md" 'adoption-led' 'adoption-led preview wording'
    require_match "ROADMAP.md" 'adoption-led' 'adoption-led preview wording'

    require_match "README.md" 'Remote Forge validation is paused pending a KVM-capable runner' 'remote Forge paused wording'
    require_match "ROADMAP.md" 'remote Forge validation is paused pending a KVM-capable runner|Remote Forge validation is paused pending a KVM-capable runner' 'remote Forge paused wording'
    require_match "docs/INTEGRATION-TESTING.md" 'Remote Forge control-plane validation is temporarily paused pending a KVM-capable runner|Forge-backed.*paused' 'remote Forge paused wording'

    require_match "README.md" '2026-05-21.*Group O' 'dated Group O evidence'
    require_match "README.md" '2026-05-21.*Group P' 'dated Group P evidence'
    require_match "ROADMAP.md" '2026-05-21.*Group O' 'dated Group O evidence'
    require_match "ROADMAP.md" '2026-05-21.*Group P' 'dated Group P evidence'
    require_match "docs/INTEGRATION-TESTING.md" 'Group O.*2026-05-21' 'dated Group O evidence'
    require_match "docs/INTEGRATION-TESTING.md" 'Group P.*2026-05-21' 'dated Group P evidence'
}

check_policykit_truth() {
    local auth_file="apps/conaryd/src/daemon/auth.rs"
    local daemon_file="apps/conaryd/src/daemon/mod.rs"
    require_file "$auth_file" || return
    require_file "$daemon_file" || return

    local overclaim_pattern='Non-root users can be authorized via PolicyKit|write access requires PolicyKit|PolicyKit authorization works|authorized by PolicyKit'
    local file line_no text
    local policykit_paths=()
    while IFS= read -r file; do
        policykit_paths+=("$file")
    done < <(existing_paths "${POLICYKIT_DOC_PATHS[@]}")

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no claims PolicyKit authorization is available today: $text"
    done < <(rg -n -- "$overclaim_pattern" "$auth_file" "${policykit_paths[@]}" 2>/dev/null || true)

    if ! rg -qi -- 'fail-closed|stubbed|unimplemented|unavailable' "$auth_file"; then
        report_error "$auth_file: must describe PolicyKit authorization as fail-closed, stubbed, unavailable, or unimplemented"
    fi

    if ! rg -q -- 'require_polkit:[ \t]*true' "$daemon_file"; then
        report_error "$daemon_file: DaemonConfig::default() must keep require_polkit: true until auth docs describe a different behavior"
    fi
}

extract_code_routes() {
    local files=(
        "apps/conaryd/src/daemon/routes/system.rs"
        "apps/conaryd/src/daemon/routes/transactions.rs"
        "apps/conaryd/src/daemon/routes/query.rs"
        "apps/conaryd/src/daemon/routes/events.rs"
    )
    local file line path method prefix

    for file in "${files[@]}"; do
        if [[ ! -f "$file" ]]; then
            report_error "required route file is missing: $file"
            continue
        fi

        while IFS= read -r line; do
            if [[ "$line" =~ \.route\(\"([^\"]+)\"[[:space:]]*,[[:space:]]*(get|post|delete)\( ]]; then
                path="${BASH_REMATCH[1]}"
                method="${BASH_REMATCH[2]^^}"
                if [[ "$file" == "apps/conaryd/src/daemon/routes/system.rs" && "$path" == "/health" ]]; then
                    prefix=""
                else
                    prefix="/v1"
                fi
                printf '%s %s%s\n' "$method" "$prefix" "$path"
            fi
        done < "$file"
    done
}

extract_doc_routes() {
    local doc="docs/modules/conaryd.md"
    require_file "$doc" || return

    awk '
        /<!-- conaryd-routes:start -->/ { in_routes = 1; next }
        /<!-- conaryd-routes:end -->/ { in_routes = 0; next }
        in_routes && /^(GET|POST|DELETE) \// { print $1 " " $2 }
    ' "$doc"
}

check_conaryd_routes() {
    local code_routes doc_routes
    code_routes="$(mktemp)"
    doc_routes="$(mktemp)"
    trap 'rm -f "$code_routes" "$doc_routes"' RETURN

    extract_code_routes | sort -u > "$code_routes"
    extract_doc_routes | sort -u > "$doc_routes"

    local route_count
    route_count="$(wc -l < "$code_routes" | tr -d ' ')"
    if [[ "$route_count" -lt 25 ]]; then
        report_error "conaryd route extraction found $route_count method/path pairs; expected at least 25"
    fi

    if ! diff -u "$code_routes" "$doc_routes" >&2; then
        report_error "conaryd route docs differ from apps/conaryd/src/daemon/routes"
    fi

    require_match "docs/modules/conaryd.md" '/health.*outside the v1 auth gate|/health.*outside.*auth' '/health auth-boundary wording'
    require_match "docs/modules/conaryd.md" '/v1/\*.*behind the v1 gate|/v1/\*.*auth' '/v1 auth-boundary wording'
    require_match "docs/modules/conaryd.md" 'Preview stub|preview-stubbed|not implemented' 'preview-stubbed system route wording'
}

check_conary_core_surface() {
    require_file "crates/conary-core/Cargo.toml" || return
    require_file "crates/conary-core/src/lib.rs" || return

    if ! rg -q -- '^publish[ \t]*=[ \t]*false$' "crates/conary-core/Cargo.toml"; then
        report_error "crates/conary-core/Cargo.toml must set publish = false while conary-core is internal"
    fi

    require_match "crates/conary-core/src/lib.rs" 'Internal workspace crate|internal workspace crate' 'internal crate documentation'

    local active_paths=()
    local path
    while IFS= read -r path; do
        active_paths+=("$path")
    done < <(existing_paths "README.md" "ROADMAP.md" "docs/ARCHITECTURE.md" "docs/conaryopedia-v2.md" "docs/modules" "docs/operations")

    local pattern='conary-core.*(stable public API|stable SDK|external library contract)|(stable public API|stable SDK|external library contract).*conary-core'
    local file line_no text
    while IFS=: read -r file line_no text; do
        if [[ ! "$text" =~ ([Ii]nternal|[Uu]nstable|not[[:space:]]+stable) ]]; then
            report_error "$file:$line_no makes a stable conary-core API claim without internal/unstable wording: $text"
        fi
    done < <(rg -n -i -- "$pattern" "${active_paths[@]}" || true)
}

check_schema_versions
check_retired_commands
check_preview_status
check_policykit_truth
check_conaryd_routes
check_conary_core_surface

if [[ "$errors" -ne 0 ]]; then
    exit 1
fi

echo "Documentation truth checks passed."
```

- [ ] **Step 5: Make the truth checker executable**

Run:

```bash
chmod +x scripts/check-doc-truth.sh
```

- [ ] **Step 6: Run the self-tests**

Run:

```bash
bash scripts/test-doc-truth.sh
```

Expected:

```text
docs truth self-tests passed.
```

- [ ] **Step 7: Run the truth script against the real repo and record the expected failures**

Run:

```bash
bash scripts/check-doc-truth.sh
```

Expected before the docs cleanup tasks: non-zero exit with failures for current PolicyKit wording, missing `docs/modules/conaryd.md`, missing `publish = false`, and possibly README's old conaryd endpoint pointer. Do not weaken the script to make this pass; the later tasks fix the repo.

- [ ] **Step 8: Commit Task 1**

Run:

```bash
git add scripts/check-doc-truth.sh scripts/test-doc-truth.sh
git commit -m "test(docs): add docs truth checker fixtures"
```

---

### Task 2: CI Wiring

**Files:**
- Modify: `.github/workflows/pr-gate.yml`

- [ ] **Step 1: Add the docs-truth job**

In `.github/workflows/pr-gate.yml`, add this job after `release-matrix-policy`:

```yaml
  docs-truth:
    name: docs-truth
    runs-on: ubuntu-latest
    env:
      LC_ALL: C
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
      - name: Check required shell tools
        run: command -v rg >/dev/null
      - name: Check docs audit ledger
        run: bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
      - name: Check docs audit inventory
        shell: bash
        run: diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
      - name: Check docs truth invariants
        run: bash scripts/check-doc-truth.sh
```

- [ ] **Step 2: Run the YAML policy checks**

Run:

```bash
bash scripts/check-github-action-runtimes.sh
```

Expected:

```text
GitHub Actions runtime pins look Node 24-ready.
```

- [ ] **Step 3: Confirm the new job is present**

Run:

```bash
rg -n "docs-truth|check-doc-truth|check-doc-audit-ledger|docs-audit-inventory" .github/workflows/pr-gate.yml
```

Expected: output contains `docs-truth`, `scripts/check-doc-truth.sh`, the ledger check, and the inventory diff.

- [ ] **Step 4: Commit Task 2**

Run:

```bash
git add .github/workflows/pr-gate.yml
git commit -m "ci: add docs truth gate"
```

---

### Task 3: conaryd Docs And PolicyKit Truth

**Files:**
- Create: `docs/modules/conaryd.md`
- Modify: `README.md`
- Modify: `apps/conaryd/src/daemon/auth.rs`
- Modify: `apps/conaryd/src/daemon/mod.rs`

- [ ] **Step 1: Add the canonical conaryd module doc**

Create `docs/modules/conaryd.md`:

```markdown
# conaryd

`conaryd` is the local daemon for query routes, package job queueing, SSE events,
and selected system-operation stubs. It listens on the configured local socket
and applies the same live-host mutation acknowledgement boundary as the CLI for
package mutation jobs.

## Authorization

`GET /health` is outside the v1 auth gate so service managers can perform basic
liveness checks. `/v1/*` routes are behind the v1 gate. Query routes are
read-oriented. Package mutation and system operation routes require the daemon
authorization checks and still require explicit live-host mutation
acknowledgement in request bodies where the operation can mutate the host.

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
```

- [ ] **Step 2: Update README's conaryd endpoint pointer**

In `README.md`, replace:

```markdown
See the [Conaryopedia](docs/conaryopedia-v2.md) for the full REST endpoint list.
```

with:

```markdown
See [docs/modules/conaryd.md](docs/modules/conaryd.md) for the maintained daemon endpoint list.
```

- [ ] **Step 3: Fix conaryd auth module docs**

At the top of `apps/conaryd/src/daemon/auth.rs`, replace the existing module docs through the PolicyKit action list with:

```rust
//! Authentication and authorization for the daemon
//!
//! Provides:
//! - Peer credential extraction (SO_PEERCRED)
//! - Permission checking (root and daemon identity)
//! - Fail-closed PolicyKit authorization stub
//! - Audit logging
//!
//! # Security Model
//!
//! The daemon enforces the following security model:
//!
//! - **Root users** (UID 0): Full access to all operations
//! - **Daemon identity**: Access to daemon-owned local API operations
//! - **Other users**: Read-only access by default; write access is denied while
//!   PolicyKit remains an unimplemented fail-closed stub
//!
//! # PolicyKit
//!
//! PolicyKit authorization is not implemented yet. Both the `polkit` feature
//! path and the non-`polkit` build path deny write authorization for non-root
//! users until Conary has a real DBus authorization check and installed policy
//! file contract.
//!
//! Reserved future policy actions:
//! - `com.conary.daemon.install` - Install packages
//! - `com.conary.daemon.remove` - Remove packages
//! - `com.conary.daemon.update` - Update packages
//! - `com.conary.daemon.rollback` - System rollback
```

- [ ] **Step 4: Clarify the daemon config field**

In `apps/conaryd/src/daemon/mod.rs`, change the field comment:

```rust
/// Require PolicyKit for non-root users
pub require_polkit: bool,
```

to:

```rust
/// Keep non-root write authorization fail-closed behind the PolicyKit stub
pub require_polkit: bool,
```

Keep `require_polkit: true` in `DaemonConfig::default()`.

- [ ] **Step 5: Run the truth script and inspect remaining failures**

Run:

```bash
bash scripts/check-doc-truth.sh
```

Expected after this task: PolicyKit and conaryd route failures are gone. Remaining failures should be limited to `conary-core` internal crate metadata/docs and any docs-audit inventory changes from the new module doc.

- [ ] **Step 6: Commit Task 3**

Run:

```bash
git add docs/modules/conaryd.md README.md apps/conaryd/src/daemon/auth.rs apps/conaryd/src/daemon/mod.rs
git commit -m "docs(conaryd): add checked daemon endpoint reference"
```

---

### Task 4: conary-core Internal Crate Guard

**Files:**
- Modify: `crates/conary-core/Cargo.toml`
- Modify: `crates/conary-core/src/lib.rs`
- Modify: `docs/ARCHITECTURE.md`

- [ ] **Step 1: Mark conary-core as non-publishable**

In `crates/conary-core/Cargo.toml`, add `publish = false` to the `[package]` section:

```toml
[package]
name = "conary-core"
version = "0.8.0"
edition = "2024"
rust-version = "1.94"
authors = ["Conary Contributors"]
description = "Core library for the Conary package manager"
license = "MIT OR Apache-2.0"
publish = false
```

- [ ] **Step 2: Clarify the crate-level docs**

In `crates/conary-core/src/lib.rs`, replace the current crate docs with:

```rust
//! Conary Core Library
//!
//! Internal workspace crate used by Conary applications for shared package,
//! database, transaction, generation, and filesystem logic.
//!
//! The broad module exports are for workspace convenience and integration-test
//! reuse. They are not a stable external public API or SDK contract.
```

- [ ] **Step 3: Update the architecture module map**

In `docs/ARCHITECTURE.md`, change:

```text
    +-- lib.rs           Public API surface
```

to:

```text
    +-- lib.rs           Internal workspace crate surface, not a stable external API
```

Then add this paragraph after the module map code block:

```markdown
`conary-core` is currently an internal workspace crate. Its broad module exports
exist for workspace app reuse and integration tests, not as a stable external
API or SDK promise. The crate is marked `publish = false`; a curated public
facade would need its own design if Conary later supports external library
consumers.
```

Also update the schema section heading:

```markdown
## Database Schema (v67)
```

to:

```markdown
## Database Schema (v69)
```

- [ ] **Step 4: Run focused truth and Rust metadata checks**

Run:

```bash
bash scripts/check-doc-truth.sh
cargo metadata --no-deps --format-version 1 >/tmp/conary-cargo-metadata.json
```

Expected:

```text
Documentation truth checks passed.
```

`cargo metadata` should exit 0.

- [ ] **Step 5: Commit Task 4**

Run:

```bash
git add crates/conary-core/Cargo.toml crates/conary-core/src/lib.rs docs/ARCHITECTURE.md
git commit -m "docs(core): mark conary-core internal"
```

---

### Task 5: Docs-Audit Metadata And Current Plan Status

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Refresh the inventory after adding docs/modules/conaryd.md and this plan**

Run:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 2: Add or update ledger rows for new active docs**

Add a row for `docs/modules/conaryd.md`:

```tsv
docs/modules/conaryd.md	docs/modules/conaryd.md	canonical	contributor	conaryd; endpoint-reference; auth; preview-stubs	apps/conaryd/src/daemon/routes/system.rs; apps/conaryd/src/daemon/routes/transactions.rs; apps/conaryd/src/daemon/routes/query.rs; apps/conaryd/src/daemon/routes/events.rs; apps/conaryd/src/daemon/auth.rs; scripts/check-doc-truth.sh	verified	corrected	Added a maintained conaryd module reference with method-aware route inventory, v1 auth-boundary wording, preview-stubbed system route status, and fail-closed PolicyKit authorization wording checked by the docs truth gate.
```

The ledger row for `docs/superpowers/plans/2026-05-26-docs-ci-truth-checks.md`
is created by the plan-writing commit. Keep it as a planning row until the
final archive task moves the plan.

- [ ] **Step 3: Run docs audit checks**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-truth.sh
```

Expected:

```text
Documentation audit ledger check passed (--require-complete).
Documentation truth checks passed.
```

The inventory diff should print nothing and exit 0.

- [ ] **Step 4: Commit Task 5**

Run:

```bash
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: register conaryd truth reference"
```

---

### Task 6: Close The Umbrella And Archive Completed Hardening Docs

**Files:**
- Move: `docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md`
- Move: `docs/superpowers/specs/2026-05-26-generation-publication-durability-design.md`
- Move: `docs/superpowers/specs/2026-05-26-docs-ci-truth-checks-design.md`
- Move: `docs/superpowers/plans/2026-05-25-adoption-safety-and-integrity.md`
- Move: `docs/superpowers/plans/2026-05-26-generation-publication-durability.md`
- Move: `docs/superpowers/plans/2026-05-26-docs-ci-truth-checks.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Move completed docs to archive paths**

Run:

```bash
git mv docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md docs/superpowers/specs/archive/2026-05-25-preview-invariant-hardening-design.md
git mv docs/superpowers/specs/2026-05-26-generation-publication-durability-design.md docs/superpowers/specs/archive/2026-05-26-generation-publication-durability-design.md
git mv docs/superpowers/specs/2026-05-26-docs-ci-truth-checks-design.md docs/superpowers/specs/archive/2026-05-26-docs-ci-truth-checks-design.md
git mv docs/superpowers/plans/2026-05-25-adoption-safety-and-integrity.md docs/superpowers/plans/archive/2026-05-25-adoption-safety-and-integrity.md
git mv docs/superpowers/plans/2026-05-26-generation-publication-durability.md docs/superpowers/plans/archive/2026-05-26-generation-publication-durability.md
git mv docs/superpowers/plans/2026-05-26-docs-ci-truth-checks.md docs/superpowers/plans/archive/2026-05-26-docs-ci-truth-checks.md
```

- [ ] **Step 2: Refresh the inventory**

Run:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 3: Update ledger rows for archived hardening docs**

Update the six moved rows so each sets both `origin_path` and `path` to the new archive location, has `family` and `audience` set to `historical`, and has `disposition` set to `archived`. The ledger checker requires `origin_path` to exist in the current committed inventory, so the old active path must move into the notes instead of staying in `origin_path`.

Use these notes:

```text
Archived from <old active path> after the preview invariant hardening milestone completed: Plan A adoption safety and integrity, Plan B generation publication durability, and Plan C docs/CI truth checks all landed with CI-backed verification.
```

The moved rows should use this old-path to new-path mapping:

```text
docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md -> docs/superpowers/specs/archive/2026-05-25-preview-invariant-hardening-design.md
docs/superpowers/specs/2026-05-26-generation-publication-durability-design.md -> docs/superpowers/specs/archive/2026-05-26-generation-publication-durability-design.md
docs/superpowers/specs/2026-05-26-docs-ci-truth-checks-design.md -> docs/superpowers/specs/archive/2026-05-26-docs-ci-truth-checks-design.md
docs/superpowers/plans/2026-05-25-adoption-safety-and-integrity.md -> docs/superpowers/plans/archive/2026-05-25-adoption-safety-and-integrity.md
docs/superpowers/plans/2026-05-26-generation-publication-durability.md -> docs/superpowers/plans/archive/2026-05-26-generation-publication-durability.md
docs/superpowers/plans/2026-05-26-docs-ci-truth-checks.md -> docs/superpowers/plans/archive/2026-05-26-docs-ci-truth-checks.md
```

- [ ] **Step 4: Update the audit summary**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, add a short dated note near the existing invariant-hardening/Plan B summary:

```markdown
### 2026-05-26 Plan C Docs Truth Gate

Plan C closed the preview invariant hardening umbrella by adding an always-on
docs-truth PR gate, strict docs-audit CI checks, a maintained conaryd endpoint
reference, fail-closed PolicyKit wording, retired-command checks, schema-version
truth checks, and the internal `conary-core` crate guard. The completed Plan A,
Plan B, Plan C, and umbrella docs were moved to archive paths after landing.
```

- [ ] **Step 5: Verify the active plan/spec roots are clean**

Run:

```bash
find docs/superpowers/plans -maxdepth 1 -type f -print
find docs/superpowers/specs -maxdepth 1 -type f -print
```

Expected: no output.

- [ ] **Step 6: Run docs gates after archive**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-truth.sh
git diff --check
```

Expected:

```text
Documentation audit ledger check passed (--require-complete).
Documentation truth checks passed.
```

The inventory diff and `git diff --check` should print nothing and exit 0.

- [ ] **Step 7: Commit Task 6**

Run:

```bash
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/specs/archive/2026-05-25-preview-invariant-hardening-design.md docs/superpowers/specs/archive/2026-05-26-generation-publication-durability-design.md docs/superpowers/specs/archive/2026-05-26-docs-ci-truth-checks-design.md docs/superpowers/plans/archive/2026-05-25-adoption-safety-and-integrity.md docs/superpowers/plans/archive/2026-05-26-generation-publication-durability.md docs/superpowers/plans/archive/2026-05-26-docs-ci-truth-checks.md
git commit -m "docs: archive completed invariant hardening plans"
```

---

### Task 7: Final Verification

**Files:**
- Verify repository state only.

- [ ] **Step 1: Run focused docs checks**

Run:

```bash
bash scripts/test-doc-truth.sh
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
```

Expected:

```text
docs truth self-tests passed.
Documentation truth checks passed.
Documentation audit ledger check passed (--require-complete).
```

The inventory diff should print nothing and exit 0.

- [ ] **Step 2: Run workspace verification**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
git diff --check
```

Expected:

- `cargo fmt --check` exits 0.
- `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- `cargo run -p conary-test -- list` exits 0 and lists the suite inventory.
- `git diff --check` exits 0.

- [ ] **Step 3: Confirm the branch diff**

Run:

```bash
git status --short --branch
git log --oneline main..HEAD
git diff --name-only main..HEAD
```

Expected:

- Branch is clean except expected committed work.
- Commit list contains the Plan C task commits.
- Diff paths are limited to scripts, PR gate, conaryd/core docs and metadata, docs-audit files, and archived hardening docs.

- [ ] **Step 4: Complete the Codex goal**

After verification passes and there is no uncommitted work, mark the Codex goal complete. Report the final `/goal` token usage from the tool result in the user-facing summary.

---

## Final Gate

Before merging or pushing, run:

```bash
bash scripts/test-doc-truth.sh
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
git diff --check
```

## Expected Final Commits

The implementation branch should contain a small commit series like:

```text
test(docs): add docs truth checker fixtures
ci: add docs truth gate
docs(conaryd): add checked daemon endpoint reference
docs(core): mark conary-core internal
docs: register docs truth implementation plan
docs: archive completed invariant hardening plans
```

Exact subjects may vary, but each commit should stage exact files only. Do not use broad commands such as `git add docs scripts crates apps`.
