#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 6 ]]; then
  echo "Usage: $0 <package> <candidate-version> <version-scheme> <architecture> <source-profile> <db-path>" >&2
  exit 64
fi

package_name="$1"
candidate_version="$2"
version_scheme="$3"
architecture="$4"
source_profile="$5"
db_path="$6"
conary_bin="${CONARY_BIN:-conary}"
work="/tmp/native-pm-security-probe"
repo_name="slice-d-security-unknown"
repo_url="http://127.0.0.1:18088"

case "${db_path}" in
  /*) ;;
  *)
    echo "Database path must be absolute: ${db_path}" >&2
    exit 64
    ;;
esac
if [[ ! -f "${db_path}" ]]; then
  echo "Conary database does not exist: ${db_path}" >&2
  exit 1
fi

if [[ -f "${work}/http.pid" ]]; then
  old_pid="$(<"${work}/http.pid")"
  case "${old_pid}" in
    ''|*[!0-9]*) ;;
    *) kill "${old_pid}" 2>/dev/null || true ;;
  esac
fi
rm -rf "${work}"
mkdir -p "${work}"

python3 - \
  "${work}/metadata.json" \
  "${package_name}" \
  "${candidate_version}" \
  "${version_scheme}" \
  "${architecture}" \
  "${repo_url}" <<'PY'
import json
from pathlib import Path
import sys

(
    metadata_path,
    package_name,
    candidate_version,
    version_scheme,
    architecture,
    repo_url,
) = sys.argv[1:]

metadata = {
    "name": "phase4-security-metadata-unknown",
    "version": "1",
    "security_advisory_source": None,
    "packages": [
        {
            "name": package_name,
            "version": candidate_version,
            "version_scheme": version_scheme,
            "architecture": architecture,
            "description": "Typed candidate for unknown security metadata refusal",
            "checksum": "0" * 64,
            "size": 1,
            "download_url": f"{repo_url}/unreachable-package",
            "requirements": [],
            "relations": [],
            "delta_from": None,
            "security_advisory": None,
        }
    ],
}
Path(metadata_path).write_text(
    json.dumps(metadata, indent=2) + "\n",
    encoding="utf-8",
)
PY

python3 -m http.server 18088 \
  --bind 127.0.0.1 \
  --directory "${work}" \
  >"${work}/http.log" 2>&1 &
http_pid="$!"
echo "${http_pid}" > "${work}/http.pid"
cleanup() {
  kill "${http_pid}" 2>/dev/null || true
}
trap cleanup EXIT

server_ready=false
for _attempt in $(seq 1 50); do
  if python3 - "${repo_url}/metadata.json" <<'PY' >/dev/null 2>&1
import sys
import urllib.request

with urllib.request.urlopen(sys.argv[1], timeout=1) as response:
    if response.status != 200:
        raise SystemExit(1)
PY
  then
    server_ready=true
    break
  fi
  sleep 0.1
done
if [[ "${server_ready}" != true ]]; then
  echo "Unknown-security repository did not become ready" >&2
  exit 1
fi

"${conary_bin}" repo remove "${repo_name}" \
  --db-path "${db_path}" >/dev/null 2>&1 || true
"${conary_bin}" repo add "${repo_name}" "${repo_url}" \
  --package-format json \
  --priority 1000 \
  --security-advisories unknown \
  --source-profile "${source_profile}" \
  --db-path "${db_path}"
"${conary_bin}" repo sync "${repo_name}" \
  --db-path "${db_path}" \
  --force

python3 - \
  "${db_path}" \
  "${repo_name}" \
  "${package_name}" \
  "${candidate_version}" \
  "${version_scheme}" \
  "${architecture}" \
  "${source_profile}" <<'PY'
import sqlite3
import sys

(
    db_path,
    repo_name,
    package_name,
    candidate_version,
    version_scheme,
    architecture,
    source_profile,
) = sys.argv[1:]

connection = sqlite3.connect(db_path)
try:
    repository = connection.execute(
        """
        SELECT id, security_advisory_support, package_format, source_profile
        FROM repositories
        WHERE name = ?
        """,
        (repo_name,),
    ).fetchone()
    if repository is None:
        raise SystemExit("typed unknown-security repository was not persisted")
    repository_id, advisory_support, package_format, persisted_profile = repository
    if (advisory_support, package_format, persisted_profile) != (
        "unknown",
        "json",
        source_profile,
    ):
        raise SystemExit(
            "unknown-security repository persisted an unexpected contract: "
            f"{(advisory_support, package_format, persisted_profile)!r}"
        )

    packages = connection.execute(
        """
        SELECT name, version, version_scheme, architecture, is_security_update
        FROM repository_packages
        WHERE repository_id = ?
        """,
        (repository_id,),
    ).fetchall()
    expected = [
        (
            package_name,
            candidate_version,
            version_scheme,
            architecture,
            0,
        )
    ]
    if packages != expected:
        raise SystemExit(
            f"typed repository sync produced {packages!r}, expected {expected!r}"
        )

    installed = connection.execute(
        "SELECT version FROM troves WHERE name = ?",
        (package_name,),
    ).fetchall()
    if len(installed) != 1:
        raise SystemExit(
            f"expected one installed {package_name!r} trove, found {installed!r}"
        )

finally:
    connection.close()
PY
