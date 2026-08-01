#!/bin/sh
# tests/fixtures/supported-host-generation-export/publish-selected-generation.sh
#
# Converge generation publication after model apply. A preceding package
# transaction may already have published once the root became bootable, so
# "already current" and a newly completed publication are equally valid only
# when no debt remains and the selected generation is exact.

set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: publish-selected-generation.sh DB_PATH GENERATION_OUTPUT" >&2
    exit 2
fi

db_path=$1
generation_output=$2
runtime_root=$(dirname "$db_path")
publish_log=/var/tmp/supported-host-publish.log
pending_log=/var/tmp/supported-host-pending.log

set +e
conary system generation publish --yes --db-path "$db_path" >"$publish_log" 2>&1
publish_code=$?
set -e
cat "$publish_log"

if [ "$publish_code" -ne 0 ]; then
    sqlite3 -line "$db_path" \
        "SELECT id, trigger_changeset_id, phase, status, state_number, generation_number, last_error FROM generation_publications WHERE recoverable = 1 ORDER BY id DESC LIMIT 1;"
    exit "$publish_code"
fi

conary system generation pending --db-path "$db_path" >"$pending_log" 2>&1
cat "$pending_log"
grep -Fxq "No pending generation publication debt." "$pending_log"

selected_path=$(readlink -f "$runtime_root/current")
generation=$(basename "$selected_path")
case "$generation" in
    "" | *[!0-9]*)
        echo "selected generation path has no exact numeric identity: $selected_path" >&2
        exit 1
        ;;
esac

expected_path="$runtime_root/generations/$generation"
if [ "$selected_path" != "$expected_path" ]; then
    echo "selected generation escaped the runtime root: $selected_path" >&2
    exit 1
fi

test -f "$expected_path/.conary-artifact.json"
test -f "$expected_path/cas-manifest.json"
test -f "$expected_path/boot-assets/manifest.json"
printf '%s\n' "$generation" >"$generation_output"
