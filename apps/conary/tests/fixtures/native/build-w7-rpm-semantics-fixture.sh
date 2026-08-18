#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <output-dir>" >&2
  exit 64
fi

output_dir="$1"
fixtures_root="${CONARY_FIXTURES_ROOT:-/opt/remi-tests/fixtures}"
encoded="${fixtures_root}/phase4-w7-rpm-semantics/phase4-w7-rpm-semantics-1.0.0-1.x86_64.rpm.b64"
artifact="${output_dir}/phase4-w7-rpm-semantics-1.0.0-1.x86_64.rpm"
temporary="${artifact}.tmp"
expected_sha256="0b412f9d56da06d94bff3f7f3ba58b080633edf90e5bcc6beba9fe40544bc0ca"

mkdir -p "${output_dir}"
base64 --decode "${encoded}" > "${temporary}"
echo "${expected_sha256}  ${temporary}" | sha256sum --check --status
mv "${temporary}" "${artifact}"

python3 - "${output_dir}/native-fixture-manifest.json" "${artifact}" "${expected_sha256}" <<'PY'
import json
import os
import sys
from pathlib import Path

manifest_path = Path(sys.argv[1])
manifest = {
    "schema_version": 1,
    "artifact_path": sys.argv[2],
    "builder_target": "rpm",
    "sha256": sys.argv[3],
}
temporary_path = manifest_path.with_suffix(manifest_path.suffix + ".tmp")
temporary_path.write_text(
    json.dumps(manifest, separators=(",", ":")) + "\n", encoding="utf-8"
)
os.replace(temporary_path, manifest_path)
PY

printf '%s\n' \
  "W7_RPM_SEMANTICS_PACKAGE='${artifact}'" \
  "W7_RPM_SEMANTICS_SHA256='${expected_sha256}'" \
  > "${output_dir}/w7-rpm-semantics-fixture.env"
