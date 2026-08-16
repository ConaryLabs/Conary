#!/usr/bin/env bash
# tests/fixtures/native/build-pinned-remi-fixture.sh
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 <rpm|deb|arch> <output-dir>" >&2
  exit 64
fi

target="$1"
output_dir="$2"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixture_root="${script_dir}/../phase4-pinned-repository"
authority_root="${script_dir}/../ccs-test-authority"
conary_bin="${CONARY_BIN:-conary}"
package_name="phase4-repository-fixture"
version="1.0.0"
release="1"

case "$target" in
  rpm)
    route="fedora"
    source_profile="fedora-44"
    architecture="x86_64"
    ;;
  deb)
    route="ubuntu"
    source_profile="ubuntu-26.04"
    architecture="amd64"
    ;;
  arch)
    route="arch"
    source_profile="arch"
    architecture="x86_64"
    ;;
  *)
    echo "Unsupported pinned repository target: ${target}" >&2
    exit 64
    ;;
esac

mkdir -p "$output_dir"
rm -f "${output_dir}"/*.ccs "${output_dir}"/index-*.json

"$conary_bin" ccs build "${fixture_root}/${target}/ccs.toml" \
  --source "${fixture_root}/stage" \
  --output "$output_dir" \
  --key "${authority_root}/fixture-signing-key.private"

artifact="${output_dir}/${package_name}-${version}-${release}.ccs"
test -s "$artifact"
artifact_size="$(stat -c %s "$artifact")"

python3 - \
  "${output_dir}/index-${route}.json" \
  "$route" \
  "$source_profile" \
  "$package_name" \
  "$version" \
  "$release" \
  "$architecture" \
  "$artifact_size" <<'PY'
import json
import os
import sys
from pathlib import Path

(
    index_path,
    route,
    source_profile,
    package_name,
    version,
    release,
    architecture,
    artifact_size,
) = sys.argv[1:]
document = {
    "distro": route,
    "source_profile": source_profile,
    "packages": [
        {
            "name": package_name,
            "distro": route,
            "versions": [
                {
                    "version": version,
                    "release": release,
                    "provides": [],
                    "requirement_groups": [],
                    "architecture": architecture,
                    "size": int(artifact_size),
                }
            ],
        }
    ],
    "total": 1,
    "page": 1,
    "per_page": 128,
}
path = Path(index_path)
temporary_path = path.with_suffix(path.suffix + ".tmp")
temporary_path.write_text(
    json.dumps(document, separators=(",", ":")) + "\n", encoding="utf-8"
)
os.replace(temporary_path, path)
PY

printf 'PINNED_REMI_ARTIFACT=%q\n' "$artifact" > "${output_dir}/pinned-remi-fixture.env"
printf 'PINNED_REMI_INDEX=%q\n' "${output_dir}/index-${route}.json" >> "${output_dir}/pinned-remi-fixture.env"
