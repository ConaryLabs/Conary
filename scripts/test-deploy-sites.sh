#!/usr/bin/env bash
# scripts/test-deploy-sites.sh -- Exercise the static-site deploy wrapper with fake SSH/rsync.
set -euo pipefail

script="${1:-deploy/deploy-sites.sh}"
test -f "$script" || {
    echo "missing deploy script: $script" >&2
    exit 1
}

tmpdir="$(mktemp -d /tmp/deploy-sites-test.XXXXXX)"
cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

repo="${tmpdir}/repo"
mkdir -p "$repo/deploy" "$repo/site" "$repo/web" "$tmpdir/bin"
cp "$script" "$repo/deploy/deploy-sites.sh"

log="${tmpdir}/commands.log"

cat >"$tmpdir/bin/npm" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'npm %s %s\n' "$(basename "$PWD")" "$*" >>"$DEPLOY_SITES_TEST_LOG"
if [[ "$1" == "run" && "$2" == "check" ]]; then
    exit 0
fi
if [[ "$1" == "run" && "$2" == "build" ]]; then
    mkdir -p build/assets
    printf '<!doctype html>\n' >build/index.html
    printf 'ok\n' >build/assets/app.js
    exit 0
fi
echo "unexpected npm invocation: $*" >&2
exit 9
SH

cat >"$tmpdir/bin/ssh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'ssh %s\n' "$*" >>"$DEPLOY_SITES_TEST_LOG"
[[ "$1" == "-F" && "$2" == "$DEPLOY_SITES_TEST_CONFIG" ]] || {
    echo "missing pinned SSH config" >&2
    exit 9
}
shift 2
host="$1"
shift
[[ "$host" == "deploy-user@example.test" ]] || {
    echo "unexpected host: $host" >&2
    exit 9
}
cmd="$*"
case "$cmd" in
    *"mktemp -d /tmp/conary-site.deploy.XXXXXX"*)
        printf '/tmp/conary-site.deploy.TEST\n'
        ;;
    *"mktemp -d /tmp/conary-web.deploy.XXXXXX"*)
        printf '/tmp/conary-web.deploy.TEST\n'
        ;;
    *"sudo -n /usr/local/sbin/conary-remi-deploy deploy-site site /tmp/conary-site.deploy.TEST"*)
        ;;
    *"sudo -n /usr/local/sbin/conary-remi-deploy deploy-site web /tmp/conary-web.deploy.TEST"*)
        ;;
    *"rm -rf -- /tmp/conary-"*)
        ;;
    *)
        echo "unexpected ssh command: $cmd" >&2
        exit 9
        ;;
esac
SH

cat >"$tmpdir/bin/rsync" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'rsync %s\n' "$*" >>"$DEPLOY_SITES_TEST_LOG"
case "$*" in
    *"-e ssh -F $DEPLOY_SITES_TEST_CONFIG"*"site/build/ deploy-user@example.test:/tmp/conary-site.deploy.TEST/"*) ;;
    *"-e ssh -F $DEPLOY_SITES_TEST_CONFIG"*"web/build/ deploy-user@example.test:/tmp/conary-web.deploy.TEST/"*) ;;
    *)
        echo "unexpected rsync invocation: $*" >&2
        exit 9
        ;;
esac
SH

chmod +x "$tmpdir/bin/npm" "$tmpdir/bin/ssh" "$tmpdir/bin/rsync"

PATH="$tmpdir/bin:$PATH" \
DEPLOY_SITES_TEST_LOG="$log" \
DEPLOY_SITES_TEST_CONFIG="$tmpdir/ssh-config" \
REMI_HOST="deploy-user@example.test" \
REMI_SSH_CONFIG="$tmpdir/ssh-config" \
REMI_DEPLOY_HELPER="/usr/local/sbin/conary-remi-deploy" \
    bash "$repo/deploy/deploy-sites.sh" site >/tmp/deploy-sites-site.out

grep -q 'deploy-site site /tmp/conary-site.deploy.TEST' "$log" ||
    fail "site deploy did not call the deploy helper"
grep -q 'site/build/ deploy-user@example.test:/tmp/conary-site.deploy.TEST/' "$log" ||
    fail "site deploy did not stage build output with rsync"
grep -q '^npm site run check$' "$log" ||
    fail "site deploy did not run the frontend check"
grep -q '^npm site run build$' "$log" ||
    fail "site deploy did not run the frontend build"

: >"$log"

PATH="$tmpdir/bin:$PATH" \
DEPLOY_SITES_TEST_LOG="$log" \
DEPLOY_SITES_TEST_CONFIG="$tmpdir/ssh-config" \
REMI_HOST="deploy-user@example.test" \
REMI_SSH_CONFIG="$tmpdir/ssh-config" \
REMI_DEPLOY_HELPER="/usr/local/sbin/conary-remi-deploy" \
    bash "$repo/deploy/deploy-sites.sh" packages >/tmp/deploy-sites-packages.out

grep -q 'deploy-site web /tmp/conary-web.deploy.TEST' "$log" ||
    fail "packages deploy did not call the deploy helper"
grep -q 'web/build/ deploy-user@example.test:/tmp/conary-web.deploy.TEST/' "$log" ||
    fail "packages deploy did not stage build output with rsync"
grep -q '^npm web run check$' "$log" ||
    fail "packages deploy did not run the frontend check"
grep -q '^npm web run build$' "$log" ||
    fail "packages deploy did not run the frontend build"

echo "deploy sites wrapper smoke passed"
