#!/usr/bin/env bash
# tests/fixtures/adversarial/build-corrupted.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONARY_BIN="${1:-${CONARY_BIN:-$(pwd)/target/debug/conary}}"

build_valid_fixture() {
    local fixture_dir="$1"
    mkdir -p "$fixture_dir/output"
    rm -f "$fixture_dir/output/"*.ccs
    "$CONARY_BIN" ccs build "$fixture_dir/ccs.toml" \
        --source "$fixture_dir/stage" \
        --output "$fixture_dir/output/"
}

mutate_ccs() {
    local source_ccs="$1"
    local output_ccs="$2"
    local mutator="$3"
    local tmpdir
    tmpdir="$(mktemp -d)"
    tar -xzf "$source_ccs" -C "$tmpdir"
    "$mutator" "$tmpdir"
    tar -czf "$output_ccs" -C "$tmpdir" .
    rm -rf "$tmpdir"
}

mutate_bad_checksum() {
    local root="$1"
    python3 - "$root/components/runtime.json" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
data["files"][0]["hash"] = "0" * 64
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

mutate_truncated() {
    local root="$1"
    local object
    object="$(find "$root/objects" -type f | head -1)"
    python3 - "$object" <<'PY'
import os
import sys
path = sys.argv[1]
size = os.path.getsize(path)
with open(path, "r+b") as f:
    f.truncate(max(1, size // 2))
PY
}

mutate_size_lie() {
    local root="$1"
    python3 - "$root/components/runtime.json" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
data["files"][0]["size"] = 1073741824
data["size"] = 1073741824
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

echo "Building valid corrupted-fixture bases..."
for fixture in bad-checksum truncated size-lie; do
    build_valid_fixture "$SCRIPT_DIR/corrupted/$fixture"
done

echo "Corrupting bad-checksum fixture..."
bad_src="$(find "$SCRIPT_DIR/corrupted/bad-checksum/output" -maxdepth 1 -name '*.ccs' | head -1)"
bad_dst="$SCRIPT_DIR/corrupted/bad-checksum/output/bad-checksum-corrupted.ccs"
mutate_ccs "$bad_src" "$bad_dst" mutate_bad_checksum

echo "Corrupting truncated fixture..."
trunc_src="$(find "$SCRIPT_DIR/corrupted/truncated/output" -maxdepth 1 -name '*.ccs' | head -1)"
trunc_dst="$SCRIPT_DIR/corrupted/truncated/output/truncated-corrupted.ccs"
mutate_ccs "$trunc_src" "$trunc_dst" mutate_truncated

echo "Corrupting size-lie fixture..."
size_src="$(find "$SCRIPT_DIR/corrupted/size-lie/output" -maxdepth 1 -name '*.ccs' | head -1)"
size_dst="$SCRIPT_DIR/corrupted/size-lie/output/size-lie-corrupted.ccs"
mutate_ccs "$size_src" "$size_dst" mutate_size_lie

echo "[OK] Corrupted fixtures built:"
printf '  %s\n' "$bad_dst" "$trunc_dst" "$size_dst"
