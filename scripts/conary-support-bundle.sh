#!/usr/bin/env bash
# scripts/conary-support-bundle.sh
set -euo pipefail

target_dir="${1:-target/conary-support-bundle}"
conary_db="${CONARY_DB:-/var/lib/conary/conary.db}"

if [[ -e "$target_dir" && ! -d "$target_dir" ]]; then
    printf 'Refusing to write support bundle: target is not a directory: %s\n' "$target_dir" >&2
    exit 64
fi

if [[ -d "$target_dir" && -n "$(find "$target_dir" -mindepth 1 -print -quit)" ]]; then
    printf 'Refusing to write support bundle into non-empty directory: %s\n' "$target_dir" >&2
    printf 'Choose a fresh directory so reviewed bundle contents cannot be mixed with older files.\n' >&2
    exit 64
fi

mkdir -p "$target_dir"

redact() {
    sed -E \
        -e 's#https://[^/@[:space:]]+:[^/@[:space:]]+@#https://[REDACTED]@#g' \
        -e 's#([?&]access_token=)[^&[:space:]]+#\1[REDACTED]#g' \
        -e 's#([Aa]uthorization:[[:space:]]*).+#\1[REDACTED]#g' \
        -e 's#(X-Remi-Admin-Token:[[:space:]]*).+#\1[REDACTED]#g' \
        -e 's#(Bearer[[:space:]]+)[A-Za-z0-9._~+/-]+=*#\1[REDACTED]#g' \
        -e 's#([Tt]oken=)[^&[:space:]]+#\1[REDACTED]#g' \
        -e 's#([Pp]assword=)[^&[:space:]]+#\1[REDACTED]#g' \
        -e 's#([Ss]ecret=)[^&[:space:]]+#\1[REDACTED]#g' \
        -e 's#/home/[^[:space:]]+/.ssh/[^[:space:]]+#/home/[REDACTED]/.ssh/[REDACTED]#g'
}

write_readme() {
    cat > "$target_dir/README.txt" <<'EOF'
Conary support bundle

Review this local support bundle before attaching it to an issue.

This bundle is allowlist-only. It captures predefined command output and
does not include conary.db, raw logs, environment dumps, shell history,
private keys, SSH keys, /etc/conary/trust, host-local access notes, or package
payloads.

If a maintainer needs deeper database troubleshooting, share the included
integrity/table summaries first. Do not attach a live database file unless a
maintainer explicitly asks for it and you have reviewed the contents.
EOF
}

capture_command() {
    local output="$1"
    local description="$2"
    shift 2

    {
        printf '# %s\n' "$description"
        printf '$'
        printf ' %q' "$@"
        printf '\n\n'

        if command -v "$1" >/dev/null 2>&1; then
            "$@" 2>&1 || printf '\n[command exited with status %s]\n' "$?"
        else
            printf 'command not found: %s\n' "$1"
        fi
    } | redact > "$target_dir/$output"
}

capture_shell() {
    local output="$1"
    local description="$2"
    local command_text="$3"

    {
        printf '# %s\n' "$description"
        printf '$ %s\n\n' "$command_text"
        bash -c "$command_text" 2>&1 || printf '\n[command exited with status %s]\n' "$?"
    } | redact > "$target_dir/$output"
}

capture_db_query() {
    local output="$1"
    local description="$2"
    local sql="$3"

    {
        printf '# %s\n' "$description"
        printf '$ sqlite3 %q %q\n\n' "$conary_db" "$sql"

        if [[ ! -f "$conary_db" ]]; then
            printf 'database not found: %s\n' "$conary_db"
        elif ! command -v sqlite3 >/dev/null 2>&1; then
            printf 'command not found: sqlite3\n'
        else
            sqlite3 "$conary_db" "$sql" 2>&1 || printf '\n[command exited with status %s]\n' "$?"
        fi
    } | redact > "$target_dir/$output"
}

write_readme
capture_command "conary-version.txt" "Conary CLI version" conary --version
capture_command "adoption-status.txt" "Adoption status" conary system adopt --status
capture_command "generation-list.txt" "Generation list" conary system generation list
capture_command "generation-pending.txt" "Pending generation publication debt" conary system generation pending
capture_command "repo-list.txt" "Configured repositories" conary repo list
capture_command "uname.txt" "Kernel and machine summary" uname -a

if command -v lsb_release >/dev/null 2>&1; then
    capture_command "os-release.txt" "Distribution summary" lsb_release -a
elif [[ -f /etc/os-release ]]; then
    capture_shell "os-release.txt" "Distribution summary" "cat /etc/os-release"
else
    printf 'No lsb_release command or /etc/os-release file found.\n' | redact > "$target_dir/os-release.txt"
fi

capture_db_query "db-integrity-check.txt" "SQLite integrity check" "PRAGMA integrity_check;"
capture_db_query "db-tables.txt" "SQLite table inventory" "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;"

printf 'Support bundle written to %s\n' "$target_dir"
printf 'Review %s/README.txt before attaching the bundle to an issue.\n' "$target_dir"
