#!/usr/bin/env bash
# tests/fixtures/native/build-pinned-binary-fixture.sh
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "Usage: $0 <rpm|deb|arch> <output-dir> [repository-url]" >&2
  exit 64
fi

target="$1"
output_dir="$2"
repository_url="${3:-http://127.0.0.1:18083}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixture_root="${script_dir}/../phase4-pinned-repository"
authority_root="${script_dir}/../ccs-test-authority"
conary_bin="${CONARY_BIN:-conary}"
package_name="phase4-repository-fixture"
version="1.0.0"
release="1"

case "$target" in
  rpm)
    version_scheme="rpm"
    architecture="x86_64"
    ;;
  deb)
    version_scheme="debian"
    architecture="amd64"
    ;;
  arch)
    version_scheme="arch"
    architecture="x86_64"
    ;;
  *)
    echo "Unsupported pinned repository target: ${target}" >&2
    exit 64
    ;;
esac

mkdir -p "$output_dir"
rm -f "${output_dir}"/*.ccs "${output_dir}/metadata.json"

"$conary_bin" ccs build "${fixture_root}/${target}/ccs.toml" \
  --source "${fixture_root}/stage" \
  --output "$output_dir" \
  --key "${authority_root}/fixture-signing-key.private"

artifact="${output_dir}/${package_name}-${version}-${release}.ccs"
test -s "$artifact"

python3 - \
  "${output_dir}/metadata.json" \
  "${output_dir}/pinned-binary-fixture-manifest.json" \
  "$version_scheme" \
  "$package_name" \
  "$version" \
  "$release" \
  "$architecture" \
  "$artifact" \
  "$repository_url" <<'PY'
import hashlib
import json
import os
import sys
from pathlib import Path

(
    metadata_path,
    fixture_manifest_path,
    version_scheme,
    package_name,
    version,
    release,
    architecture,
    artifact_path,
    repository_url,
) = sys.argv[1:]
artifact = Path(artifact_path)
artifact_bytes = artifact.read_bytes()
document = {
    "name": "phase4-pinned-binary-repository",
    "version": "1",
    "security_advisory_source": None,
    "packages": [
        {
            "name": package_name,
            "version": version,
            "release": release,
            "version_scheme": version_scheme,
            "architecture": architecture,
            "description": "Pinned signed CCS repository fixture",
            "checksum": hashlib.sha256(artifact_bytes).hexdigest(),
            "size": len(artifact_bytes),
            "download_url": f"{repository_url}/{artifact.name}",
            "requirements": [],
            "relations": [],
            "delta_from": None,
            "security_advisory": None,
        }
    ],
}
path = Path(metadata_path)
temporary_path = path.with_suffix(path.suffix + ".tmp")
temporary_path.write_text(
    json.dumps(document, separators=(",", ":")) + "\n", encoding="utf-8"
)
os.replace(temporary_path, path)

fixture_manifest = {
    "schema_version": 1,
    "artifact_path": str(artifact),
    "builder_target": version_scheme,
    "sha256": hashlib.sha256(artifact_bytes).hexdigest(),
}
manifest_path = Path(fixture_manifest_path)
temporary_manifest_path = manifest_path.with_suffix(manifest_path.suffix + ".tmp")
temporary_manifest_path.write_text(
    json.dumps(fixture_manifest, separators=(",", ":")) + "\n", encoding="utf-8"
)
os.replace(temporary_manifest_path, manifest_path)
PY

printf 'PINNED_BINARY_ARTIFACT=%q\n' "$artifact" > "${output_dir}/pinned-binary-fixture.env"
printf 'PINNED_REPOSITORY_METADATA=%q\n' "${output_dir}/metadata.json" >> "${output_dir}/pinned-binary-fixture.env"
printf 'PINNED_BINARY_FIXTURE_MANIFEST=%q\n' "${output_dir}/pinned-binary-fixture-manifest.json" >> "${output_dir}/pinned-binary-fixture.env"
