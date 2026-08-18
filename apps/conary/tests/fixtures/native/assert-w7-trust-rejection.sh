#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 5 ]]; then
  echo "Usage: $0 <package> <diagnostic> <db-path> <selected-root> <conary-bin>" >&2
  exit 64
fi

package="$1"
diagnostic="$2"
db_path="$3"
selected_root="$4"
conary_bin="$5"
log="/tmp/${package}-trust-rejection.log"

before_database="$(sqlite3 "${db_path}" .dump | sha256sum | awk '{print $1}')"
before_current="$(readlink "${selected_root}/current")"
before_generation_count="$(find "${selected_root}/generations" -mindepth 1 -maxdepth 1 -type d | wc -l)"
before_activation_requests="$(sqlite3 "${db_path}" 'SELECT COUNT(*) FROM activation_requests')"
before_activation_intents="$(sqlite3 "${db_path}" 'SELECT COUNT(*) FROM generation_activation_intents')"

if CONARY_TEST_SKIP_GENERATION_MOUNT=1 "${conary_bin}" install "${package}" \
  --convert-to-ccs \
  --repo w7trust \
  --db-path "${db_path}" \
  --root "${selected_root}" \
  --no-deps \
  --yes \
  --sandbox always >"${log}" 2>&1
then
  echo "Expected ${package} to fail its package-signature trust contract" >&2
  exit 1
fi

grep -F "${diagnostic}" "${log}"
test "$(sqlite3 "${db_path}" .dump | sha256sum | awk '{print $1}')" = "${before_database}"
test "$(readlink "${selected_root}/current")" = "${before_current}"
test "$(find "${selected_root}/generations" -mindepth 1 -maxdepth 1 -type d | wc -l)" = "${before_generation_count}"
test "$(sqlite3 "${db_path}" 'SELECT COUNT(*) FROM activation_requests')" = "${before_activation_requests}"
test "$(sqlite3 "${db_path}" 'SELECT COUNT(*) FROM generation_activation_intents')" = "${before_activation_intents}"
test "$(sqlite3 "${db_path}" "SELECT COUNT(*) FROM troves WHERE name = '${package}'")" = 0
test ! -e "${selected_root}/usr/share/${package}/proof.txt"
