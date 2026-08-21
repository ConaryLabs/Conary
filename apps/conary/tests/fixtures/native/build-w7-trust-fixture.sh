#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <output-directory>" >&2
  exit 64
fi

output="$1"
fixtures_root="${CONARY_FIXTURES_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
encoded="${fixtures_root}/phase4-w7-trust-corpus/w7-trust-corpus.tar.gz.b64"
expected_bundle_sha256="d9ce969aa229e45030316973da6267d1ca25a4c7631cbfc002eff78de15e85b8"

mkdir -p "${output}"
archive="$(mktemp "${output}/.w7-trust-corpus.XXXXXX.tar.gz")"
cleanup() {
  rm -f "${archive}"
}
trap cleanup EXIT

base64 --decode "${encoded}" > "${archive}"
actual_bundle_sha256="$(sha256sum "${archive}" | awk '{print $1}')"
if [[ "${actual_bundle_sha256}" != "${expected_bundle_sha256}" ]]; then
  echo "W7 trust corpus bundle digest mismatch: ${actual_bundle_sha256}" >&2
  exit 1
fi

tar --extract --gzip --file "${archive}" --directory "${output}"
python3 - "${output}" <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
manifest = json.loads((root / "fixture-manifest.json").read_text())
for name, expected in manifest.items():
    path = root / name
    data = path.read_bytes()
    actual = {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
    if actual != expected:
        raise SystemExit(f"W7 trust fixture mismatch for {name}: {actual} != {expected}")

for case in ("valid-a", "valid-b", "expired", "revoked", "unknown"):
    artifact = root / f"w7-trust-{case}-1.0.0-1-any.pkg.tar.gz"
    evidence = {
        "schema_version": 1,
        "artifact_path": str(artifact),
        "builder_target": "arch",
        "sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
    }
    evidence_path = root / f"w7-trust-{case}-fixture-manifest.json"
    temporary = evidence_path.with_suffix(evidence_path.suffix + ".tmp")
    temporary.write_text(json.dumps(evidence, separators=(",", ":")) + "\n")
    temporary.replace(evidence_path)
PY

fingerprint="$(sed -n '1p' "${output}/master-fingerprint")"
if [[ ! "${fingerprint}" =~ ^[0-9A-F]{40}$ ]]; then
  echo "W7 trust master fingerprint is not canonical: ${fingerprint}" >&2
  exit 1
fi

printf 'W7_TRUST_ROOT=%q\n' "${output}" > "${output}/w7-trust-fixture.env"
printf 'W7_TRUST_MASTER_FINGERPRINT=%q\n' "${fingerprint}" >> "${output}/w7-trust-fixture.env"
printf 'W7_TRUST_BUNDLE_SHA256=%q\n' "${expected_bundle_sha256}" >> "${output}/w7-trust-fixture.env"
