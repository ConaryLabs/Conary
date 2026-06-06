#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

usage() {
    cat >&2 <<'EOF'
Usage: scripts/line-count-report.sh [limit]

Print the largest Rust files under apps/ and crates/.

Arguments:
  limit    Positive integer row limit. Defaults to 60.
EOF
}

limit="${1:-60}"

if [[ $# -gt 1 || "$limit" == "-h" || "$limit" == "--help" ]]; then
    usage
    exit 2
fi

case "$limit" in
    ''|*[!0-9]*)
        usage
        exit 2
        ;;
esac

if (( 10#$limit == 0 )); then
    usage
    exit 2
fi

printf 'lines\tpath\n'

find apps crates -type f -name '*.rs' -exec wc -l {} + \
    | awk '
        $NF == "total" { next }
        {
            line = $0
            sub(/^[[:space:]]+/, "", line)
            count = line
            sub(/[[:space:]].*$/, "", count)
            path = line
            sub(/^[0-9]+[[:space:]]+/, "", path)
            printf "%s\t%s\n", count, path
        }
    ' \
    | sort -rn -k1,1 \
    | awk -v limit="$limit" 'NR <= limit { print }'
