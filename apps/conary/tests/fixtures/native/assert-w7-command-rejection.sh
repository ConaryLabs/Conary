#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 8 || "$7" != "--" ]]; then
  echo "Usage: $0 <package> <absent-path|-> <db-path> <selected-root> <log> <diagnostic|-> -- <command...>" >&2
  exit 64
fi

package="$1"
absent_path="$2"
db_path="$3"
selected_root="$4"
log="$5"
diagnostic="$6"
shift 7

[[ "$package" =~ ^[a-zA-Z0-9._+-]+$ ]] || {
  echo "Unsafe package identity: $package" >&2
  exit 64
}

authority_digest=/opt/remi-tests/fixtures/native/database-authority-digest.py
before_database_authority="$(python3 "$authority_digest" "$db_path")"
before_current="$(readlink "$selected_root/current")"
before_generation_count="$(find "$selected_root/generations" -mindepth 1 -maxdepth 1 -type d | wc -l)"
before_activation_requests="$(sqlite3 "$db_path" 'SELECT COUNT(*) FROM activation_requests')"
before_activation_intents="$(sqlite3 "$db_path" 'SELECT COUNT(*) FROM generation_activation_intents')"

assert_unchanged() {
  local label="$1"
  local before="$2"
  local after="$3"
  if [[ "$after" != "$before" ]]; then
    printf '%s changed across rejected command: before=%q after=%q\n' \
      "$label" "$before" "$after" >&2
    return 1
  fi
}

if "$@" >"$log" 2>&1; then
  echo "Expected command to reject $package" >&2
  exit 1
fi

if [[ "$diagnostic" != "-" ]]; then
  grep -F "$diagnostic" "$log"
fi
assert_unchanged database_authority "$before_database_authority" \
  "$(python3 "$authority_digest" "$db_path")"
assert_unchanged current_generation "$before_current" \
  "$(readlink "$selected_root/current")"
assert_unchanged generation_count "$before_generation_count" \
  "$(find "$selected_root/generations" -mindepth 1 -maxdepth 1 -type d | wc -l)"
assert_unchanged activation_requests "$before_activation_requests" \
  "$(sqlite3 "$db_path" 'SELECT COUNT(*) FROM activation_requests')"
assert_unchanged activation_intents "$before_activation_intents" \
  "$(sqlite3 "$db_path" 'SELECT COUNT(*) FROM generation_activation_intents')"
assert_unchanged installed_package 0 \
  "$(sqlite3 "$db_path" "SELECT COUNT(*) FROM troves WHERE name = '$package'")"
if [[ "$absent_path" != "-" ]]; then
  test ! -e "$selected_root$absent_path"
fi
