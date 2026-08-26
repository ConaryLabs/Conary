#!/usr/bin/env bash
# scripts/test-codex-app-server-reconcile.sh

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
reconcile="$repo_root/deploy/hetzner/codex-app-server-reconcile.sh"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT

fake_bin="$test_root/bin"
state_dir="$test_root/state"
mkdir -p "$fake_bin" "$state_dir"

cat >"$fake_bin/ps" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
pid=""
while (($#)); do
    if [[ "$1" == "-p" ]]; then
        pid="$2"
        shift 2
    else
        shift
    fi
done
case "$pid" in
    101) printf 'Mon Aug 24 01:02:03 2026\n' ;;
    202) printf 'Tue Aug 25 04:05:06 2026\n' ;;
    *) exit 1 ;;
esac
EOF
chmod +x "$fake_bin/ps"

run_reconcile() {
    PATH="$fake_bin:$PATH" \
        CODEX_APP_SERVER_DAEMON_STATE_DIR="$state_dir" \
        "$reconcile"
}

write_record() {
    local name="$1"
    local pid="$2"
    local start="$3"
    jq -n --argjson pid "$pid" --arg start "$start" \
        '{pid: $pid, processStartTime: $start}' >"$state_dir/$name.pid"
}

run_reconcile

write_record app-server 101 'Mon Aug 24 01:02:03 2026'
run_reconcile
test -f "$state_dir/app-server.pid"

write_record app-server-updater 303 'Tue Aug 25 04:05:06 2026'
run_reconcile
test ! -e "$state_dir/app-server-updater.pid"
test -f "$state_dir/app-server.pid"

write_record app-server-updater 202 'Mon Aug 24 01:02:03 2026'
run_reconcile
test ! -e "$state_dir/app-server-updater.pid"
test -f "$state_dir/app-server.pid"

printf '{"pid":"invalid"}\n' >"$state_dir/app-server-updater.pid"
if run_reconcile 2>/dev/null; then
    printf 'malformed PID record unexpectedly passed reconciliation\n' >&2
    exit 1
fi
test -f "$state_dir/app-server-updater.pid"

printf 'Codex app-server daemon reconciliation tests passed.\n'
