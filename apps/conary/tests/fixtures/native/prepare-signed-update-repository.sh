#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "Usage: $0 <rpm|deb|arch> <source-profile> <db-path>" >&2
  exit 64
fi

source_format="$1"
source_profile="$2"
db_path="$3"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixtures_dir="$(cd "${script_dir}/.." && pwd)"
conary_bin="${CONARY_BIN:-conary}"
work="/tmp/native-pm-signed-update"
package_output="${work}/packages"
key_base="${work}/repository-package-key"
repo_name="slice-d-local-update"
repo_url="http://127.0.0.1:18087"

case "${source_format}" in
  rpm)
    expected_scheme="rpm"
    ;;
  deb)
    expected_scheme="debian"
    ;;
  arch)
    expected_scheme="arch"
    ;;
  *)
    echo "Unsupported source format: ${source_format}" >&2
    exit 64
    ;;
esac
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
mkdir -p "${package_output}"

"${conary_bin}" ccs keygen \
  --output "${key_base}" \
  --key-id "phase4-ephemeral-repository-package" \
  --force
"${conary_bin}" ccs build "${fixtures_dir}/phase4-signed-update/${source_format}" \
  --source "${fixtures_dir}/phase4-runtime-fixture-v2/stage" \
  --output "${package_output}" \
  --target ccs \
  --key "${key_base}.private"

mapfile -t packages < <(
  find "${package_output}" -maxdepth 1 -type f -name '*.ccs' -print | sort
)
if [[ "${#packages[@]}" -ne 1 ]]; then
  echo "Expected one signed repository update package, found ${#packages[@]}" >&2
  exit 1
fi
package_path="${packages[0]}"

python3 - \
  "${fixtures_dir}/phase4-signed-update/${source_format}/ccs.toml" \
  "${key_base}.public" \
  "${package_path}" \
  "${package_output}/metadata.json" \
  "${package_output}/trust-policy.toml" \
  "${repo_url}" \
  "${expected_scheme}" <<'PY'
import hashlib
import json
from pathlib import Path
import sys
import tomllib

manifest_path = Path(sys.argv[1])
public_key_path = Path(sys.argv[2])
package_path = Path(sys.argv[3])
metadata_path = Path(sys.argv[4])
policy_path = Path(sys.argv[5])
repo_url = sys.argv[6]
expected_scheme = sys.argv[7]

with manifest_path.open("rb") as stream:
    package = tomllib.load(stream)["package"]
with public_key_path.open("rb") as stream:
    public_key = tomllib.load(stream)

if package["name"] != "phase4-runtime-fixture":
    raise SystemExit("signed update fixture has an unexpected package name")
if package["version"] != "1.0.1-1":
    raise SystemExit("signed update fixture has an unexpected package version")
if package["version_scheme"] != expected_scheme:
    raise SystemExit("signed update fixture has an unexpected version scheme")
architecture = package["platform"]["arch"]
package_bytes = package_path.read_bytes()
checksum = hashlib.sha256(package_bytes).hexdigest()
metadata = {
    "name": "phase4-signed-update",
    "version": "1",
    "security_advisory_source": None,
    "packages": [
        {
            "name": package["name"],
            "version": package["version"],
            "version_scheme": package["version_scheme"],
            "architecture": architecture,
            "description": package["description"],
            "checksum": checksum,
            "size": len(package_bytes),
            "download_url": f"{repo_url}/{package_path.name}",
            "requirements": [],
            "relations": [],
            "delta_from": None,
            "security_advisory": None,
        }
    ],
}
metadata_path.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
policy_path.write_text(
    f'trusted_keys = ["{public_key["key"]}"]\n'
    "require_timestamp = true\n"
    "max_signature_age = 0\n",
    encoding="utf-8",
)
PY

"${conary_bin}" ccs verify "${package_path}" \
  --policy "${package_output}/trust-policy.toml"

python3 -m http.server 18087 \
  --bind 127.0.0.1 \
  --directory "${package_output}" \
  >"${work}/http.log" 2>&1 &
echo "$!" > "${work}/http.pid"

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
  echo "Signed update repository did not become ready" >&2
  exit 1
fi

"${conary_bin}" repo remove "${repo_name}" \
  --db-path "${db_path}" >/dev/null 2>&1 || true
"${conary_bin}" repo add "${repo_name}" "${repo_url}" \
  --package-format json \
  --default-strategy binary \
  --ccs-package-key "${key_base}.public" \
  --source-profile "${source_profile}" \
  --db-path "${db_path}"
"${conary_bin}" repo sync "${repo_name}" \
  --db-path "${db_path}" \
  --force
