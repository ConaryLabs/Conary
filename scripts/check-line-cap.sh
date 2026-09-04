#!/usr/bin/env bash
# scripts/check-line-cap.sh
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/check-line-cap.sh [--root <path>] [--allowlist <path>]

Fail when a Rust source file has more than 1,000 non-test lines without a
checked-in, issue-linked exception. Non-test lines are the lines before the
first inline #[cfg(test)] module, or the whole file when none exists.
EOF
}

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

repo_root="$(git rev-parse --show-toplevel)"
scan_root="$repo_root"
allowlist="$repo_root/scripts/line-cap-allowlist.txt"

while (($#)); do
    case "$1" in
        --root)
            (($# >= 2)) || fail "--root requires a path"
            scan_root="$2"
            shift 2
            ;;
        --allowlist)
            (($# >= 2)) || fail "--allowlist requires a path"
            allowlist="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown argument: $1"
            ;;
    esac
done

scan_root="$(cd "$scan_root" && pwd -P)"
[[ -f "$allowlist" ]] || fail "line-cap allowlist not found: $allowlist"

declare -A allowed_issues=()
declare -A used_allowlist_entries=()

while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" =~ ^[[:space:]]*# ]] && continue
    read -r path issue extra <<<"$line"
    [[ -n "${path:-}" && "${issue:-}" =~ ^#[0-9]+$ && -z "${extra:-}" ]] \
        || fail "invalid allowlist entry (expected '<path> #<issue>'): $line"
    [[ -z "${allowed_issues[$path]+set}" ]] || fail "duplicate allowlist entry: $path"
    allowed_issues["$path"]="$issue"
done < "$allowlist"

non_test_lines() {
    awk '
        function inline_test_module(line) {
            return line ~ /^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[[:space:]]*\{/
        }

        /^[[:space:]]*#\[cfg\(test\)\][[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[[:space:]]*\{/ {
            print NR - 1
            found = 1
            exit
        }

        /^[[:space:]]*#\[cfg\(test\)\][[:space:]]*$/ {
            pending = NR
            next
        }

        pending && inline_test_module($0) {
            print pending - 1
            found = 1
            exit
        }

        pending && $0 !~ /^[[:space:]]*($|#\[|\/\/)/ {
            pending = 0
        }

        END {
            if (!found) {
                print NR
            }
        }
    ' "$1"
}

scan_dirs=()
for candidate in "$scan_root/apps" "$scan_root/crates"; do
    [[ -d "$candidate" ]] && scan_dirs+=("$candidate")
done
((${#scan_dirs[@]})) || fail "no apps/ or crates/ source roots below $scan_root"

errors=0
while IFS= read -r -d '' file; do
    relative="${file#"$scan_root"/}"
    case "$relative" in
        tests.rs|*/tests.rs|tests/*.rs|*/tests/*.rs|*/target/*)
            continue
            ;;
    esac

    lines="$(non_test_lines "$file")"
    ((lines > 1000)) || continue

    if [[ -n "${allowed_issues[$relative]+set}" ]]; then
        used_allowlist_entries["$relative"]=1
        continue
    fi

    echo "ERROR: $relative has $lines non-test lines (limit: 1000)" >&2
    errors=$((errors + 1))
done < <(find "${scan_dirs[@]}" -type f -name '*.rs' -print0 | sort -z)

for path in "${!allowed_issues[@]}"; do
    if [[ -z "${used_allowlist_entries[$path]+set}" ]]; then
        echo "ERROR: stale line-cap allowlist entry: $path ${allowed_issues[$path]}" >&2
        errors=$((errors + 1))
    fi
done

((errors == 0)) || exit 1
echo "Rust non-test line cap passed."
