#!/usr/bin/env bash
# scripts/check-line-cap.sh
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/check-line-cap.sh [--root <path>] [--allowlist <path>]

Fail when a Rust source file has more than 1,000 non-test lines or an inline
#[cfg(test)] module has more than 300 lines without a checked-in, issue-linked
exception. Non-test lines are all lines outside inline #[cfg(test)] modules.
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

source_metrics() {
    awk '
        function repeat_hashes(count,    result, position) {
            result = ""
            for (position = 0; position < count; position++) {
                result = result "#"
            }
            return result
        }

        function char_literal_end(line, start,    escape, position) {
            if (substr(line, start + 1, 1) != "\\") {
                return substr(line, start + 2, 1) == "\047" ? start + 2 : 0
            }

            escape = substr(line, start + 2, 1)
            if (escape == "x") {
                return substr(line, start + 5, 1) == "\047" ? start + 5 : 0
            }
            if (escape == "u" && substr(line, start + 3, 1) == "{") {
                for (position = start + 4; position <= length(line); position++) {
                    if (substr(line, position, 1) == "}") {
                        return substr(line, position + 1, 1) == "\047" ? position + 1 : 0
                    }
                }
                return 0
            }
            return substr(line, start + 3, 1) == "\047" ? start + 3 : 0
        }

        function brace_delta(line,    c, delta, hashes, position, next_two, raw_end) {
            delta = 0
            position = 1
            while (position <= length(line)) {
                c = substr(line, position, 1)
                next_two = substr(line, position, 2)

                if (block_comment_depth > 0) {
                    if (next_two == "/*") {
                        block_comment_depth++
                        position += 2
                    } else if (next_two == "*/") {
                        block_comment_depth--
                        position += 2
                    } else {
                        position++
                    }
                    continue
                }

                if (raw_string) {
                    if (c == "\"" && (raw_hashes == 0 || substr(line, position + 1, raw_hashes) == repeat_hashes(raw_hashes))) {
                        position += raw_hashes + 1
                        raw_string = 0
                    } else {
                        position++
                    }
                    continue
                }

                if (quoted_string) {
                    if (c == "\\") {
                        position += 2
                    } else {
                        if (c == "\"") {
                            quoted_string = 0
                        }
                        position++
                    }
                    continue
                }

                if (next_two == "//") {
                    break
                }
                if (next_two == "/*") {
                    block_comment_depth++
                    position += 2
                    continue
                }

                if (c == "r") {
                    raw_end = position + 1
                    hashes = 0
                    while (substr(line, raw_end, 1) == "#") {
                        hashes++
                        raw_end++
                    }
                    if (substr(line, raw_end, 1) == "\"") {
                        raw_string = 1
                        raw_hashes = hashes
                        position = raw_end + 1
                        continue
                    }
                }

                if (c == "\"") {
                    quoted_string = 1
                    position++
                    continue
                }
                if (c == "\047") {
                    raw_end = char_literal_end(line, position)
                    if (raw_end > 0) {
                        position = raw_end + 1
                        continue
                    }
                }
                if (c == "{") {
                    delta++
                } else if (c == "}") {
                    delta--
                }
                position++
            }
            return delta
        }

        function inline_test_module(line) {
            return line ~ /^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[[:space:]]*\{/
        }

        function finish_test_module() {
            if (test_lines > largest_test_lines) {
                largest_test_lines = test_lines
                largest_test_start = test_start
            }
            in_test = 0
            test_depth = 0
            test_lines = 0
            test_start = 0
        }

        {
            delta = brace_delta($0)

            if (in_test) {
                test_lines++
                test_depth += delta
                if (test_depth == 0) {
                    finish_test_module()
                }
                next
            }

            if (pending_test) {
                if (inline_test_module($0)) {
                    in_test = 1
                    test_start = pending_start
                    test_lines = pending_lines + 1
                    test_depth = delta
                    pending_test = 0
                    pending_lines = 0
                    if (test_depth == 0) {
                        finish_test_module()
                    }
                    next
                }
                if ($0 ~ /^[[:space:]]*($|#\[|\/\/)/) {
                    pending_lines++
                    next
                }
                non_test += pending_lines
                pending_test = 0
                pending_lines = 0
            }

            if ($0 ~ /^[[:space:]]*#\[cfg\(test\)\]/) {
                remainder = $0
                sub(/^[[:space:]]*#\[cfg\(test\)\][[:space:]]*/, "", remainder)
                if (inline_test_module(remainder)) {
                    in_test = 1
                    test_start = NR
                    test_lines = 1
                    test_depth = delta
                    if (test_depth == 0) {
                        finish_test_module()
                    }
                } else {
                    pending_test = 1
                    pending_start = NR
                    pending_lines = 1
                }
                next
            }

            non_test++
        }

        END {
            if (pending_test) {
                non_test += pending_lines
            }
            if (in_test) {
                finish_test_module()
            }
            printf "%d\t%d\t%d\n", non_test, largest_test_lines, largest_test_start
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

    IFS=$'\t' read -r lines inline_test_lines inline_test_start < <(source_metrics "$file")
    if ((lines <= 1000 && inline_test_lines <= 300)); then
        continue
    fi

    if [[ -n "${allowed_issues[$relative]+set}" ]]; then
        used_allowlist_entries["$relative"]=1
        continue
    fi

    if ((lines > 1000)); then
        echo "ERROR: $relative has $lines non-test lines (limit: 1000)" >&2
        errors=$((errors + 1))
    fi
    if ((inline_test_lines > 300)); then
        echo "ERROR: $relative has an inline #[cfg(test)] module at line $inline_test_start with $inline_test_lines lines (limit: 300)" >&2
        errors=$((errors + 1))
    fi
done < <(find "${scan_dirs[@]}" -type f -name '*.rs' -print0 | sort -z)

for path in "${!allowed_issues[@]}"; do
    if [[ -z "${used_allowlist_entries[$path]+set}" ]]; then
        echo "ERROR: stale line-cap allowlist entry: $path ${allowed_issues[$path]}" >&2
        errors=$((errors + 1))
    fi
done

((errors == 0)) || exit 1
echo "Rust source line caps passed."
