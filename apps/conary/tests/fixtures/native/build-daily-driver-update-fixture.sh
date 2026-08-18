#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "Usage: $0 <rpm|deb|arch> <v1-source-dir> <v2-source-dir> <output-dir>" >&2
  exit 64
fi

target="$1"
v1_source_dir="$2"
v2_source_dir="$3"
output_dir="$4"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "${target}" in
  rpm|deb|arch) ;;
  *)
    echo "Unsupported daily-driver update target: ${target}" >&2
    exit 64
    ;;
esac
if [[ "${v1_source_dir}" != "/tmp/native-pm-corpus-src" \
  || "${v2_source_dir}" != "/tmp/native-pm-corpus-v2-src" \
  || "${output_dir}" != "/tmp/native-pm-corpus-v2" ]]; then
  echo "Daily-driver update paths must match the focused suite scratch contract" >&2
  exit 64
fi
if [[ ! -f "${v1_source_dir}/ccs.toml" || ! -d "${v1_source_dir}/stage" ]]; then
  echo "Daily-driver v1 source is incomplete: ${v1_source_dir}" >&2
  exit 64
fi

rm -rf "${v2_source_dir}" "${output_dir}"
cp -a "${v1_source_dir}" "${v2_source_dir}"
mkdir -p "${output_dir}"

python3 "${script_dir}/rewrite-package-version.py" \
  "${v2_source_dir}/ccs.toml" \
  1.0.0 \
  1.0.1

python3 - "${v2_source_dir}" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1])
(source / "stage/etc/phase4-corpus/app.conf").write_text(
    'mode = "daily-driver"\n'
    'service = "phase4-corpus"\n'
    'revision = "v2"\n',
    encoding="utf-8",
)
(source / "stage/etc/phase4-corpus/app-local.conf").write_text(
    'local = "driver-v2"\n',
    encoding="utf-8",
)
(source / "stage/etc/phase4-corpus/app-deleted.conf").write_text(
    'deleted = "driver-v2"\n',
    encoding="utf-8",
)
(source / "stage/usr/bin/phase4-corpus").write_text(
    "#!/bin/sh\nprintf 'phase4-daily-driver-corpus-v2\\n'\n",
    encoding="utf-8",
)
PY

python3 - "${v2_source_dir}/ccs.toml" <<'PY'
import copy
import os
from pathlib import Path
import sys
import tempfile
import tomllib

manifest_path = Path(sys.argv[1])
manifest = manifest_path.read_text(encoding="utf-8")
parsed = tomllib.loads(manifest)
if any(
    declaration.get("path") == "/etc/phase4-corpus/app-unmatched.conf"
    for declaration in parsed["config"]["files"]
):
    raise SystemExit("v2 manifest must newly introduce the unmatched config declaration")

declaration_block = (
    "[[config.files]]\n"
    'source = "ccs"\n'
    'path = "/etc/phase4-corpus/app-unmatched.conf"\n'
    "noreplace = true\n"
    'payload = "absent"\n'
)
lines = manifest.splitlines(keepends=True)
hooks_index = next(
    index for index, line in enumerate(lines) if line.strip() == "[hooks]"
)
lines.insert(hooks_index, declaration_block)
rewritten = "".join(lines)
after = tomllib.loads(rewritten)
expected = copy.deepcopy(parsed)
expected["config"]["files"].append(
    {
        "source": "ccs",
        "path": "/etc/phase4-corpus/app-unmatched.conf",
        "noreplace": True,
        "payload": "absent",
    }
)
if after != expected:
    raise SystemExit("unmatched introduction changed fields outside its declaration")

mode = manifest_path.stat().st_mode & 0o777
temporary_path = None
try:
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=manifest_path.parent,
        prefix=f".{manifest_path.name}.",
        delete=False,
    ) as stream:
        temporary_path = Path(stream.name)
        stream.write(rewritten)
        stream.flush()
        os.fsync(stream.fileno())
    os.chmod(temporary_path, mode)
    os.replace(temporary_path, manifest_path)
except BaseException:
    if temporary_path is not None:
        temporary_path.unlink(missing_ok=True)
    raise
PY

"${script_dir}/build-native-fixtures.sh" \
  "${target}" \
  "${output_dir}" \
  "${v2_source_dir}"
test -s "${output_dir}/native-fixture-manifest.json"
