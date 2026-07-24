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
    "site/src"
)

PRODUCT_DOC_PATHS=(
    "README.md"
    "ROADMAP.md"
    "CHANGELOG.md"
    "docs/ARCHITECTURE.md"
    "docs/conaryopedia-v2.md"
    "docs/modules"
    "docs/operations"
    "docs/roadmaps"
    "site/src"
    "web/src/routes"
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

require_path() {
    local path="$1"
    if [[ ! -e "$path" ]]; then
        report_error "required path is missing: $path"
        return 1
    fi
}

check_required_scan_paths() {
    local path
    local seen="|"
    for path in \
        "${DOCS_TRUTH_SCHEMA_CHECK_PATHS[@]}" \
        "${PRODUCT_DOC_PATHS[@]}" \
        "${POLICYKIT_DOC_PATHS[@]}" \
        "${PARSER_PATHS[@]}"
    do
        if [[ "$seen" == *"|$path|"* ]]; then
            continue
        fi
        seen+="$path|"
        require_path "$path" || true
    done
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

conary_release_version() {
    local manifest="apps/conary/Cargo.toml"
    require_file "$manifest" || return 1

    sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)"/\1/p' "$manifest" | head -n1
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
    local file line_no text found path
    for path in "${DOCS_TRUTH_SCHEMA_CHECK_PATHS[@]}"; do
        if [[ ! -e "$path" ]]; then
            report_error "$path: missing while checking schema version claims"
            continue
        fi

        while IFS=: read -r file line_no text; do
            if [[ "$text" =~ $schema_pattern ]]; then
                found="${BASH_REMATCH[2]}"
                if [[ "$found" != "$schema_version" ]]; then
                    report_error "$file:$line_no mentions schema $found but SCHEMA_VERSION is $schema_version"
                fi
            fi
        done < <(rg -nH -- "$schema_pattern" "$path" || true)
    done
}

check_retired_commands() {
    local retired_pattern='(^|[^A-Za-z0-9_-])(adopt-system|conary[ \t]+adopt|conary-adopt|system-adopt)([^A-Za-z0-9_-]|$)'
    local paths=()
    local path

    while IFS= read -r path; do
        paths+=("$path")
    done < <(existing_paths "${PRODUCT_DOC_PATHS[@]}" "${PARSER_PATHS[@]}")

    if [[ "${#paths[@]}" -eq 0 ]]; then
        report_error "retired command check had no paths to scan"
        return
    fi

    local file line_no text
    while IFS=: read -r file line_no text; do
        case "$file" in
            scripts/check-doc-truth.sh|scripts/test-doc-truth.sh|*/archive/*)
                continue
                ;;
        esac
        report_error "$file:$line_no contains retired command spelling: $text"
    done < <(rg -n -- "$retired_pattern" "${paths[@]}" || true)
}

check_system_init_profiles() {
    local paths=()
    local path

    while IFS= read -r path; do
        paths+=("$path")
    done < <(existing_paths "${PRODUCT_DOC_PATHS[@]}")

    local file line_no text
    while IFS=: read -r file line_no text; do
        if [[ "$text" != *"--profile "* && "$text" != *"--profile="* ]]; then
            report_error "$file:$line_no shows system init without an exact --profile: $text"
        fi
    done < <(rg -nH -- 'conary system init' "${paths[@]}" || true)
}

check_preview_status() {
    require_match "README.md" 'Conary is still early\. Expect failures\.' 'early preview warning'
    require_match "README.md" 'VM or disposable host' 'VM or disposable host warning'
    require_match "README.md" '[Ss]criptlet-heavy packages|package scriptlets' 'scriptlet-heavy package caveat'
    require_match "README.md" 'capture the command, distro, package name' 'tester failure data request'

    require_match "ROADMAP.md" 'adoption-led' 'adoption-led preview wording'
    require_match "ROADMAP.md" '[Ff]irst external tester' 'first external tester milestone wording'
    require_match "ROADMAP.md" 'docs/roadmaps/development-roadmap\.md' 'detailed development roadmap link'
    require_match "docs/roadmaps/development-roadmap.md" '[Ff]irst external tester milestone' 'first external tester milestone wording'

    require_match "docs/roadmaps/development-roadmap.md" 'remote Forge validation is paused pending (a new |a )KVM-capable runner|Remote Forge validation is paused pending (a new |a )KVM-capable runner' 'remote Forge paused wording'
    require_match "docs/INTEGRATION-TESTING.md" 'Remote Forge control-plane validation is temporarily paused pending a KVM-capable runner|Forge-backed.*paused' 'remote Forge paused wording'

    require_match "docs/roadmaps/development-roadmap.md" '2026-05-21.*Group O' 'dated Group O evidence'
    require_match "docs/roadmaps/development-roadmap.md" '2026-05-21.*Group P' 'dated Group P evidence'
    require_match "docs/INTEGRATION-TESTING.md" 'Group O.*2026-05-21' 'dated Group O evidence'
    require_match "docs/INTEGRATION-TESTING.md" 'Group P.*2026-05-21' 'dated Group P evidence'
}

check_release_doc_versions() {
    local version
    version="$(conary_release_version)"
    if [[ -z "$version" ]]; then
        report_error "apps/conary/Cargo.toml: could not parse Conary release version"
        return
    fi

    local current_tag="v${version}"
    local current_artifact_dash="conary-${version}"
    local current_artifact_underscore="conary_${version}"
    local path
    local -a release_docs=(
        "README.md"
        "docs/guides/agent-assisted-tester-loop.md"
        "docs/operations/release-artifact-matrix.md"
        "docs/roadmaps/external-tester-milestone.md"
        "docs/operations/external-tester-outreach.md"
        "site/src"
    )

    for path in "${release_docs[@]}"; do
        [[ -e "$path" ]] || continue
        require_match "$path" "$current_tag" "current Conary release tag ${current_tag}"

        local file line_no text
        while IFS=: read -r file line_no text; do
            if [[ "$text" != *"$current_tag"* ]]; then
                report_error "$file:$line_no has stale conary release reference; expected ${current_tag}: $text"
            fi
        done < <(
            rg -nH -P -- '(?<![A-Za-z0-9_-])v[0-9]+\.[0-9]+\.[0-9]+' "$path" ||
                true
        )

        while IFS=: read -r file line_no text; do
            if [[ "$text" != *"$current_artifact_dash"* && "$text" != *"$current_artifact_underscore"* ]]; then
                report_error "$file:$line_no has stale conary release reference; expected ${version}: $text"
            fi
        done < <(rg -nH -- 'conary[-_][0-9]+\.[0-9]+\.[0-9]+' "$path" || true)
    done
}

check_site_preview_truth() {
    local features="site/src/routes/features/+page.svelte"
    local package_index_about="web/src/routes/about/+page.svelte"
    require_file "$features" || return
    require_file "$package_index_about" || return

    require_match "$features" 'conary system generation build[^<]*--yes' 'generation build apply-intent example'
    require_match "$features" 'conary system generation switch[^<]*--yes' 'generation switch apply-intent example'
    require_match "$features" 'conary system generation rollback[^<]*--yes' 'generation rollback apply-intent example'
    require_match "$features" 'conary system generation gc[^<]*--yes' 'generation garbage-collection apply-intent example'
    require_match "$features" 'conary model apply --dry-run' 'model apply dry-run example'
    require_match "$features" 'conary model apply --yes' 'model apply confirmation example'
    require_match "$features" '[Ff]ederation is outside the reliable limited-preview path' 'federation preview-boundary caveat'
    require_match "$features" 'not a complete reproducibility or containment guarantee' 'hermetic-mode limitation caveat'

    local file line_no text
    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no makes a public frontend every-operation generation/integrity claim: $text"
    done < <(
        rg -n -i -- 'every operation[^.\n]*(EROF|composefs|fs-verity)|fs-verity on every file' \
            "site/src/routes" "web/src/routes" || true
    )

    while IFS=: read -r file line_no text; do
        if [[ "$text" != *"--dry-run"* && "$text" != *"--yes"* ]]; then
            report_error "$file:$line_no shows an active install without --dry-run or --yes: $text"
        fi
    done < <(rg -n -- '<(?:code|span[^>]*)>(?:sudo )?conary install ' "site/src/routes" "web/src/routes" || true)

    while IFS=: read -r file line_no text; do
        if [[ "$text" == *"--yes"* && "$text" != *">sudo conary install "* && "$text" != *"--db-path"* ]]; then
            report_error "$file:$line_no shows a system-database install without sudo: $text"
        fi
    done < <(rg -n -- '<(?:code|span[^>]*)>(?:sudo )?conary install ' "site/src/routes" "web/src/routes" || true)
}

check_preview_claim_drift() {
    local paths=()
    local path

    while IFS= read -r path; do
        paths+=("$path")
    done < <(existing_paths "${PRODUCT_DOC_PATHS[@]}")

    if [[ "${#paths[@]}" -eq 0 ]]; then
        report_error "preview claim drift check had no paths to scan"
        return
    fi

    local file line_no text

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no claims conaryd package execution is still blanket 501: $text"
    done < <(
        rg -n -i -- 'conaryd.*package (install/remove/update|mutation).*501 Not Implemented|package install/remove/update routes return.*501 Not Implemented' "${paths[@]}" || true
    )

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no claims every install builds an EROFS generation: $text"
    done < <(
        rg -n -i -- 'every install[^.\n]*(builds|produces)[^.\n]*EROF|every install, remove, (or |and )?(upgrade|update)[^.\n]*builds[^.\n]*EROF' "${paths[@]}" || true
    )

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no makes an unmeasured under-a-minute preview claim: $text"
    done < <(rg -n -i -- 'under a minute' "${paths[@]}" || true)

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no claims native packages are atomically absorbed/taken over without the explicit takeover boundary: $text"
    done < <(
        rg -n -i -- 'atomically[^.\n]*(absorbs|takes over)|absorbed atomically' "${paths[@]}" || true
    )
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
    local file

    for file in "${files[@]}"; do
        if [[ ! -f "$file" ]]; then
            report_error "required route file is missing: $file"
        fi
    done

    python3 - "${files[@]}" <<'PY'
import re
import sys
from pathlib import Path

KNOWN_METHODS = {"get", "post", "put", "patch", "delete"}
ROUTE_PATTERN = ".route("


def route_calls(source):
    start = 0
    while True:
        route_start = source.find(ROUTE_PATTERN, start)
        if route_start == -1:
            return

        idx = route_start + len(ROUTE_PATTERN)
        depth = 1
        while idx < len(source) and depth:
            char = source[idx]
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
            idx += 1

        if depth != 0:
            raise ValueError("unterminated .route(...) call")

        yield source[route_start + len(ROUTE_PATTERN) : idx - 1]
        start = idx


def extract_path_and_handlers(route_body):
    match = re.match(r'\s*"([^"]+)"\s*,\s*(.*)\s*$', route_body, re.S)
    if not match:
        raise ValueError(f"unsupported .route(...) shape: {route_body.strip()}")
    return match.group(1), match.group(2)


had_error = False
for file_name in sys.argv[1:]:
    path = Path(file_name)
    if not path.is_file():
        continue

    source = path.read_text()
    for route_body in route_calls(source):
        route_path, handlers = extract_path_and_handlers(route_body)
        calls = re.findall(r'(?:(?<=\.)|^|\s)([a-z_][a-z0-9_]*)\s*\(', handlers)
        methods = [call for call in calls if call in KNOWN_METHODS]
        unknown = [call for call in calls if call not in KNOWN_METHODS]

        if not methods or unknown:
            print(
                f"ERROR: unsupported route method shape in {file_name}: {route_body.strip()}",
                file=sys.stderr,
            )
            had_error = True
            continue

        prefix = "" if file_name.endswith("routes/system.rs") and route_path == "/health" else "/v1"
        for method in methods:
            print(f"{method.upper()} {prefix}{route_path}")

if had_error:
    sys.exit(1)
PY
}

extract_doc_routes() {
    local doc="docs/modules/conaryd.md"
    require_file "$doc" || return

    awk '
        /<!-- conaryd-routes:start -->/ { in_routes = 1; next }
        /<!-- conaryd-routes:end -->/ { in_routes = 0; next }
        in_routes && /^(GET|POST|PUT|PATCH|DELETE) \// { print $1 " " $2 }
    ' "$doc"
}

check_conaryd_routes() {
    local code_routes doc_routes
    code_routes="$(mktemp)"
    doc_routes="$(mktemp)"
    trap 'rm -f "$code_routes" "$doc_routes"' RETURN

    if ! extract_code_routes | sort -u > "$code_routes"; then
        report_error "conaryd route extraction failed"
    fi
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

check_conary_cli_command_refs() {
    local cli_file="apps/conary/src/cli/mod.rs"
    require_file "$cli_file" || return

    python3 - "$cli_file" "${PRODUCT_DOC_PATHS[@]}" <<'PY'
import re
import sys
from pathlib import Path

cli_file = Path(sys.argv[1])
scan_roots = [Path(path) for path in sys.argv[2:]]


def kebab_case(name):
    name = re.sub(r"([a-z0-9])([A-Z])", r"\1-\2", name)
    name = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1-\2", name)
    return name.replace("_", "-").lower()


def commands_from_enum(path):
    source = path.read_text()
    enum_start = source.find("pub enum Commands")
    if enum_start == -1:
        raise ValueError(f"{path}: could not find pub enum Commands")

    brace_start = source.find("{", enum_start)
    if brace_start == -1:
        raise ValueError(f"{path}: could not find Commands enum body")

    depth = 1
    idx = brace_start + 1
    while idx < len(source) and depth:
        char = source[idx]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
        idx += 1

    if depth != 0:
        raise ValueError(f"{path}: unterminated Commands enum")

    body = source[brace_start + 1 : idx - 1]
    depth = 1
    pending_name = None
    commands = {"help"}

    for raw_line in body.splitlines():
        stripped = raw_line.strip()
        if depth == 1:
            attr_match = re.search(r'#\[command\([^]]*name\s*=\s*"([^"]+)"', stripped)
            if attr_match:
                pending_name = attr_match.group(1)
            else:
                variant_match = re.match(r"([A-Z][A-Za-z0-9_]*)\b", stripped)
                if variant_match:
                    variant = variant_match.group(1)
                    commands.add(pending_name or kebab_case(variant))
                    pending_name = None

        depth += raw_line.count("{")
        depth -= raw_line.count("}")

    if len(commands) <= 1:
        raise ValueError(f"{path}: no Commands enum variants found")

    return commands


def iter_scan_files(paths):
    extensions = {".md", ".mdx", ".rst", ".adoc", ".svelte"}
    for root in paths:
        if not root.exists():
            continue
        if root.is_file():
            if root.suffix in extensions:
                yield root
            continue
        for path in sorted(root.rglob("*")):
            if not path.is_file() or path.suffix not in extensions:
                continue
            if "archive" in path.parts:
                continue
            yield path


try:
    root_commands = commands_from_enum(cli_file)
except ValueError as error:
    print(f"ERROR: {error}", file=sys.stderr)
    sys.exit(1)

historical_context = re.compile(
    r"\b(retired|removed|deprecated|legacy|historical|archive|no longer|not live|not a live)\b",
    re.IGNORECASE,
)
command_ref = re.compile(r"`conary\s+([A-Za-z0-9][A-Za-z0-9_-]*)(?:\s|`)")

had_error = False
for path in iter_scan_files(scan_roots):
    for line_no, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
        if historical_context.search(line):
            continue
        for match in command_ref.finditer(line):
            root_command = match.group(1)
            if root_command not in root_commands:
                print(
                    f"ERROR: {path}:{line_no} mentions unknown conary root command "
                    f"`conary {root_command}`: {line.strip()}",
                    file=sys.stderr,
                )
                had_error = True

if had_error:
    sys.exit(1)
PY
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

check_neutral_planning_layout() {
    local provider_name
    local old_planning_root
    local old_local_root
    local assistant_history_root
    provider_name="$(printf '%s%s' 'super' 'powers')"
    old_planning_root="docs/${provider_name}"
    old_local_root=".${provider_name}"
    assistant_history_root="docs/llms/$(printf '%s' 'archive')"

    require_file "ROADMAP.md" || true
    require_file "docs/roadmaps/development-roadmap.md" || true

    if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        report_error "neutral layout: tracked-tree check requires a Git worktree"
        return
    fi

    local path
    while IFS= read -r path; do
        [[ -n "$path" ]] || continue
        report_error "neutral layout: retired planning path is tracked: $path"
    done < <(git ls-files -- "${old_planning_root}/**")

    while IFS= read -r path; do
        [[ -n "$path" ]] || continue
        report_error "neutral layout: retired local SDD path is tracked: $path"
    done < <(git ls-files -- "${old_local_root}/**")

    while IFS= read -r path; do
        [[ -n "$path" ]] || continue
        report_error "neutral layout: assistant history archive path is tracked: $path"
    done < <(git ls-files -- "${assistant_history_root}/**")

    while IFS= read -r path; do
        [[ -n "$path" ]] || continue
        report_error "neutral layout: planning archive path is tracked: $path"
    done < <(git ls-files -- "docs/designs/archive/**" "docs/plans/archive/**")

    local mandatory_skill_pattern
    mandatory_skill_pattern="(must|required).{0,80}${provider_name}:[A-Za-z0-9_-]+.{0,40}skill"
    local file line_no text
    while IFS=: read -r file line_no text; do
        report_error "neutral layout: mandatory provider skill directive in $file:$line_no: $text"
    done < <(git grep -n -I -i -E -e "$mandatory_skill_pattern" -- . || true)

    local provider_skill_pattern="${provider_name}:[A-Za-z0-9_-]+"
    while IFS=: read -r file line_no text; do
        report_error "neutral layout: provider-prefixed skill token in $file:$line_no: $text"
    done < <(git grep -n -I -i -E -e "$provider_skill_pattern" -- . || true)

    local deleted_names=(
        "$(printf '%s%s' 'check-doc-audit-' 'ledger')"
        "$(printf '%s%s' 'docs-audit-' 'inventory')"
        "$(printf '%s%s' 'check-coherency-' 'ledger')"
        "$(printf '%s%s' 'check-coherency-' 'wave-scopes')"
        "$(printf '%s%s' 'test-coherency-' 'ledger')"
        "$(printf '%s%s' 'documentation-accuracy-' 'audit')"
        "$(printf '%s%s' 'feature-' 'coherency')"
        "$(printf '%s%s' 'coherency-' 'wave')"
    )
    local deleted_name
    for deleted_name in "${deleted_names[@]}"; do
        while IFS=: read -r file line_no text; do
            report_error "neutral layout: deleted structural validator reference in $file:$line_no: $text"
        done < <(git grep -n -I -i -F -e "$deleted_name" -- . || true)
    done

    while IFS=: read -r file line_no text; do
        report_error "neutral layout: assistant history archive reference in $file:$line_no: $text"
    done < <(git grep -n -I -i -F -e "$assistant_history_root" -- . || true)

    while IFS=: read -r file line_no text; do
        report_error "neutral layout: retired provider reference in $file:$line_no: $text"
    done < <(git grep -n -I -i -F -e "$provider_name" -- . || true)
}

check_neutral_planning_layout
check_required_scan_paths
check_schema_versions
check_retired_commands
check_system_init_profiles
check_preview_status
check_release_doc_versions
check_site_preview_truth
check_preview_claim_drift
check_policykit_truth
check_conaryd_routes
check_conary_cli_command_refs
check_conary_core_surface

if [[ "$errors" -ne 0 ]]; then
    exit 1
fi

echo "Documentation truth checks passed."
