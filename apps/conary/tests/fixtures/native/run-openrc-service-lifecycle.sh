#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <target-init-system>" >&2
  exit 64
fi
if [[ "$1" != "openrc" ]]; then
  exit 0
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixtures_dir="$(cd "${script_dir}/.." && pwd)"
fixture="${fixtures_dir}/openrc-service"
foreign_fixture="${fixtures_dir}/openrc-foreign-service"
authority="${fixtures_dir}/ccs-test-authority"
conary_bin="${CONARY_BIN:-conary}"
work="/tmp/conary-openrc-service-lifecycle"
root="${work}"
db="${work}/conary.db"
output="${work}/output"
foreign_output="${work}/foreign-output"
home="${work}/home"

rm -rf "${work}"
mkdir -p "${root}" "${output}" "${home}"

HOME="${home}" "${conary_bin}" system init --db-path "${db}"
CONARY_BIN="${conary_bin}" "${script_dir}/prepare-selected-root.sh" "${db}" "${root}"
"${conary_bin}" ccs build "${fixture}" \
  --source "${fixture}/stage" \
  --output "${output}" \
  --target ccs \
  --key "${authority}/fixture-signing-key.private"

mapfile -t packages < <(find "${output}" -maxdepth 1 -type f -name '*.ccs' -print | sort)
if [[ "${#packages[@]}" -ne 1 ]]; then
  echo "Expected one OpenRC lifecycle fixture, found ${#packages[@]}" >&2
  exit 1
fi

CONARY_TEST_SKIP_GENERATION_MOUNT=1 \
  HOME="${home}" \
  "${conary_bin}" ccs install "${packages[0]}" \
    --policy "${authority}/trust-policy.toml" \
    --db-path "${db}" \
    --root "${root}" \
    --sandbox always \
    --no-deps \
    --yes

selected_root="${root}/current"
runlevel_link="${selected_root}/etc/runlevels/default/conary-openrc-fixture"
if [[ ! -L "${runlevel_link}" ]]; then
  echo "Selected OpenRC generation is missing runlevel link ${runlevel_link}" >&2
  find "${selected_root}/etc" -maxdepth 4 -printf '%y %m %p -> %l\n' >&2 || true
  exit 1
fi
runlevel_target="$(readlink "${runlevel_link}")"
if [[ "${runlevel_target}" != "../../init.d/conary-openrc-fixture" ]]; then
  echo "OpenRC runlevel link has target ${runlevel_target}, expected ../../init.d/conary-openrc-fixture" >&2
  exit 1
fi
activation_count="$({
  sqlite3 "${db}" <<'SQL'
SELECT COUNT(*)
FROM activation_requests
WHERE source_kind = 'ccs-service'
  AND invocation_json LIKE '%"kind":"open-rc"%';
SQL
})"
pending_count="$({
  sqlite3 "${db}" <<'SQL'
SELECT COUNT(*)
FROM generation_activation_intents
WHERE status = 'pending';
SQL
})"
if [[ "${activation_count}" != "1" ]]; then
  echo "Expected one direct CCS OpenRC activation request, found ${activation_count}" >&2
  sqlite3 -header -column "${db}" \
    'SELECT source_kind, source_package, source_entry, invocation_json FROM activation_requests;' >&2
  exit 1
fi
if [[ "${pending_count}" != "1" ]]; then
  echo "Expected one pending generation activation intent, found ${pending_count}" >&2
  sqlite3 -header -column "${db}" \
    'SELECT request_id, generation, sequence, status FROM generation_activation_intents;' >&2
  exit 1
fi

CONARY_BIN="${conary_bin}" "${script_dir}/build-native-fixtures.sh" \
  rpm "${foreign_output}" "${foreign_fixture}"
# shellcheck disable=SC1091
source "${foreign_output}/native-fixture.env"
CONARY_TEST_SKIP_GENERATION_MOUNT=1 \
  HOME="${home}" \
  "${conary_bin}" install "${NATIVE_PKG_FILE}" \
    --convert-to-ccs \
    --from fedora-44 \
    --db-path "${db}" \
    --sandbox always \
    --no-deps \
    --yes

foreign_capture_count="$({
  sqlite3 "${db}" <<'SQL'
SELECT COUNT(*)
FROM activation_requests
WHERE source_kind = 'captured-openrc'
  AND source_package = 'conary-openrc-foreign-lifecycle-fixture'
  AND source_entry = 'rpm:%post'
  AND json_extract(invocation_json, '$.kind') = 'open-rc'
  AND json_extract(invocation_json, '$.invocation.action') = 'restart'
  AND json_extract(invocation_json, '$.invocation.service') = 'conary-openrc-foreign-fixture';
SQL
})"
foreign_pending_count="$({
  sqlite3 "${db}" <<'SQL'
SELECT COUNT(*)
FROM generation_activation_intents AS intent
JOIN activation_requests AS request ON request.id = intent.request_id
WHERE intent.status = 'pending'
  AND request.source_kind = 'captured-openrc'
  AND request.source_package = 'conary-openrc-foreign-lifecycle-fixture';
SQL
})"
if [[ "${foreign_capture_count}" != "1" ]]; then
  echo "Expected one captured foreign OpenRC activation request, found ${foreign_capture_count}" >&2
  sqlite3 -header -column "${db}" \
    'SELECT source_kind, source_package, source_entry, invocation_json FROM activation_requests;' >&2
  exit 1
fi
if [[ "${foreign_pending_count}" != "1" ]]; then
  echo "Expected one pending foreign OpenRC generation intent, found ${foreign_pending_count}" >&2
  sqlite3 -header -column "${db}" \
    'SELECT request_id, generation, sequence, status FROM generation_activation_intents;' >&2
  exit 1
fi
