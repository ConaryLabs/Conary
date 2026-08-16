#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 8 ]]; then
  echo "Usage: $0 <native-fixture-manifest> <source-format> <source-profile> <db-path> <package-name> <version> <version-scheme> <architecture>" >&2
  exit 64
fi

fixture_manifest="$1"
source_format="$2"
source_profile="$3"
db_path="$4"
package_name="$5"
package_version="$6"
version_scheme="$7"
architecture="$8"
conary_bin="${CONARY_BIN:-conary}"
work="/tmp/native-update-repository"
package_output="${work}/packages"
repo_name="w7-native-update"
repo_url="http://127.0.0.1:18087"

case "${source_format}:${version_scheme}" in
  rpm:rpm|deb:debian|arch:arch) ;;
  *)
    echo "Native update source format and version scheme disagree: ${source_format}:${version_scheme}" >&2
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
if [[ ! -f "${fixture_manifest}" || ! -f "${db_path}" ]]; then
  echo "Native fixture manifest or Conary database is absent" >&2
  exit 64
fi

if [[ -f "${work}/http.pid" ]]; then
  old_pid="$(<"${work}/http.pid")"
  case "${old_pid}" in
    ''|*[!0-9]*) ;;
    *) kill "${old_pid}" 2>/dev/null || true ;;
  esac
fi
rm -rf "${work}"
mkdir -p "${package_output}" "${work}/home"

mapfile -t native_identity < <(python3 - "${fixture_manifest}" "${source_format}" <<'PY'
import hashlib
import json
from pathlib import Path
import sys

manifest = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
if manifest.get("schema_version") != 1:
    raise SystemExit("unsupported native fixture manifest schema")
if manifest.get("builder_target") != sys.argv[2]:
    raise SystemExit("native fixture target contradicts the update source format")
artifact = Path(manifest["artifact_path"])
digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
if digest != manifest.get("sha256"):
    raise SystemExit("native update artifact digest contradicts its build manifest")
print(artifact)
print(digest)
PY
)
if [[ "${#native_identity[@]}" -ne 2 ]]; then
  echo "Native update fixture did not resolve to one exact artifact identity" >&2
  exit 1
fi
native_artifact="${native_identity[0]}"
native_digest="${native_identity[1]}"

XDG_DATA_HOME="${work}/xdg-data" HOME="${work}/home" \
  "${conary_bin}" cook "${native_artifact}" \
  --source-profile "${source_profile}" \
  --output "${package_output}"

mapfile -t packages < <(
  find "${package_output}" -maxdepth 1 -type f -name '*.ccs' -print | sort
)
if [[ "${#packages[@]}" -ne 1 ]]; then
  echo "Expected one converted native update package, found ${#packages[@]}" >&2
  exit 1
fi
package_path="${packages[0]}"
public_key="${work}/xdg-data/conary/ccs/local-dev/local-dev-key.public.toml"
test -s "${public_key}"

"${conary_bin}" ccs inspect "${package_path}" --format json > "${work}/inspection.json"
python3 - \
  "${work}/inspection.json" \
  "${work}/metadata.json" \
  "${work}/repository-fixture-manifest.json" \
  "${work}/trust-policy.toml" \
  "${public_key}" \
  "${package_path}" \
  "${native_artifact}" \
  "${native_digest}" \
  "${package_name}" \
  "${package_version}" \
  "${version_scheme}" \
  "${architecture}" \
  "${repo_url}" <<'PY'
import hashlib
import json
import os
from pathlib import Path
import sys
import tomllib

(
    inspection_path,
    metadata_path,
    repository_manifest_path,
    policy_path,
    public_key_path,
    package_path,
    native_artifact_path,
    native_digest,
    package_name,
    package_version,
    version_scheme,
    architecture,
    repo_url,
) = sys.argv[1:]

inspection = json.loads(Path(inspection_path).read_text(encoding="utf-8"))
if inspection.get("name") != package_name or inspection.get("version") != package_version:
    raise SystemExit("converted update identity contradicts the native fixture declaration")

package = Path(package_path)
package_bytes = package.read_bytes()
package_digest = hashlib.sha256(package_bytes).hexdigest()
metadata = {
    "name": "w7-native-update",
    "version": "1",
    "security_advisory_source": None,
    "packages": [
        {
            "name": package_name,
            "version": package_version,
            "version_scheme": version_scheme,
            "architecture": architecture,
            "description": "W7 native-derived configuration update fixture",
            "checksum": package_digest,
            "size": len(package_bytes),
            "download_url": f"{repo_url}/packages/{package.name}",
            "requirements": [],
            "relations": [],
            "delta_from": None,
            "security_advisory": None,
        }
    ],
}
repository_manifest = {
    "schema_version": 1,
    "source_artifact": {
        "artifact_path": native_artifact_path,
        "sha256": native_digest,
    },
    "repository_artifact": {
        "artifact_path": str(package),
        "sha256": package_digest,
    },
}
with Path(public_key_path).open("rb") as stream:
    public_key = tomllib.load(stream)

for path, document in [
    (Path(metadata_path), metadata),
    (Path(repository_manifest_path), repository_manifest),
]:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(document, separators=(",", ":")) + "\n", encoding="utf-8"
    )
    os.replace(temporary, path)
Path(policy_path).write_text(
    f'trusted_keys = ["{public_key["key"]}"]\nrequire_timestamp = false\n',
    encoding="utf-8",
)
PY

"${conary_bin}" ccs verify "${package_path}" --policy "${work}/trust-policy.toml"

python3 -m http.server 18087 \
  --bind 127.0.0.1 \
  --directory "${work}" \
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
  echo "Native update repository did not become ready" >&2
  exit 1
fi

"${conary_bin}" repo remove "${repo_name}" \
  --db-path "${db_path}" >/dev/null 2>&1 || true
"${conary_bin}" repo add "${repo_name}" "${repo_url}" \
  --package-format json \
  --default-strategy binary \
  --ccs-package-key "${public_key}" \
  --source-profile "${source_profile}" \
  --db-path "${db_path}"
"${conary_bin}" repo sync "${repo_name}" \
  --db-path "${db_path}" \
  --force

repository_digest="$(sha256sum "${package_path}" | awk '{print $1}')"
printf 'NATIVE_UPDATE_SOURCE_ARTIFACT=%q\n' "${native_artifact}" > "${work}/native-update-repository.env"
printf 'NATIVE_UPDATE_SOURCE_SHA256=%q\n' "${native_digest}" >> "${work}/native-update-repository.env"
printf 'NATIVE_UPDATE_CCS_ARTIFACT=%q\n' "${package_path}" >> "${work}/native-update-repository.env"
printf 'NATIVE_UPDATE_CCS_SHA256=%q\n' "${repository_digest}" >> "${work}/native-update-repository.env"
printf 'NATIVE_UPDATE_REPOSITORY=%q\n' "${repo_name}" >> "${work}/native-update-repository.env"
