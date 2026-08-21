#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C
export TZ=UTC
umask 022

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <output-dir>" >&2
  exit 64
fi

output_dir="$1"
fixtures_root="${CONARY_FIXTURES_ROOT:-/opt/remi-tests/fixtures}"
work="${output_dir}/work"
package_root="${work}/arch-root"
archive="${work}/phase4-w7-semantic-corpus.pkg.tar"
artifact="${output_dir}/phase4-w7-semantic-corpus-2:1.0.0-3-any.pkg.tar.gz"

rm -rf "${work}"
mkdir -p \
  "${package_root}/usr/bin" \
  "${package_root}/usr/share/phase4-w7-semantic-corpus"

install -m 0755 \
  "${fixtures_root}/phase4-w7-semantic-corpus/stage/usr/bin/phase4-w7-semantic" \
  "${package_root}/usr/bin/phase4-w7-semantic"

python3 - "${package_root}" <<'PY'
import os
import sys
from pathlib import Path

root = Path(sys.argv[1])
payload = root / "usr/share/phase4-w7-semantic-corpus"

large = payload / "large.bin"
block = bytes(range(256)) * 256
with large.open("wb") as handle:
    for _ in range(512):
        handle.write(block)

sparse = payload / "sparse.bin"
with sparse.open("wb") as handle:
    handle.write(b"W7-SPARSE-BEGIN\n")
    handle.seek(64 * 1024 * 1024 - len(b"W7-SPARSE-END\n"))
    handle.write(b"W7-SPARSE-END\n")

tool = root / "usr/bin/phase4-w7-semantic"
os.setxattr(tool, "user.conary", b"exact-w7-xattr")

stat = sparse.stat()
if stat.st_size != 64 * 1024 * 1024 or stat.st_blocks * 512 >= stat.st_size // 8:
    raise SystemExit("fixture source is not a 64 MiB sparse file")
PY

cat > "${package_root}/.PKGINFO" <<'EOF'
pkgname = phase4-w7-semantic-corpus
pkgbase = phase4-w7-semantic-corpus
pkgver = 2:1.0.0-3
pkgdesc = Pinned W7 identity, payload, and metadata semantic fixture
url = https://conary.io/fixtures/phase4-w7-semantic-corpus
builddate = 1787011200
packager = Conary Fixture Maintainers <fixtures@conary.io>
size = 100663348
arch = any
license = MIT
provides = phase4-w7-semantic-corpus=2:1.0.0
EOF

tar \
  --format=pax \
  --sparse \
  --xattrs \
  --sort=name \
  --mtime=@1787011200 \
  --pax-option=delete=atime,delete=ctime \
  --numeric-owner \
  --owner=0 \
  --group=0 \
  -C "${package_root}" \
  -cf "${archive}" \
  .PKGINFO usr
gzip -n -9 -c "${archive}" > "${artifact}"

checksum="$(sha256sum "${artifact}" | awk '{print $1}')"
python3 - "${output_dir}/w7-semantic-fixture-manifest.json" "${artifact}" "${checksum}" <<'PY'
import json
import os
import sys
from pathlib import Path

manifest_path = Path(sys.argv[1])
manifest = {
    "schema_version": 1,
    "artifact_path": sys.argv[2],
    "builder_target": "arch",
    "sha256": sys.argv[3],
}
temporary_path = manifest_path.with_suffix(manifest_path.suffix + ".tmp")
temporary_path.write_text(
    json.dumps(manifest, separators=(",", ":")) + "\n", encoding="utf-8"
)
os.replace(temporary_path, manifest_path)
PY

printf '%s\n' \
  "W7_SEMANTIC_PACKAGE='${artifact}'" \
  "W7_SEMANTIC_SHA256='${checksum}'" \
  > "${output_dir}/w7-semantic-fixture.env"
