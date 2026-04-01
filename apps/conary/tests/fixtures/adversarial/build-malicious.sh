#!/usr/bin/env bash
# tests/fixtures/adversarial/build-malicious.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONARY_BIN="${1:-${CONARY_BIN:-$(pwd)/target/debug/conary}}"

build_fixture() {
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

mutate_ccs_with_payload() {
    local source_ccs="$1"
    local output_ccs="$2"
    local payload="$3"
    local tmpdir
    tmpdir="$(mktemp -d)"
    tar -xzf "$source_ccs" -C "$tmpdir"
    mutate_decompression_bomb "$tmpdir" "$payload"
    tar -czf "$output_ccs" -C "$tmpdir" .
    rm -rf "$tmpdir"
}

make_executable() {
    local path="$1"
    if [ -f "$path" ]; then
        chmod 0755 "$path"
    fi
}

mutate_path_traversal() {
    local root="$1"
    python3 - "$root/components/runtime.json" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
data["files"][0]["path"] = "../../etc/shadow"
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

mutate_symlink_attack() {
    local root="$1"
    python3 - "$root/components/runtime.json" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
for entry in data["files"]:
    if entry.get("type") == "regular":
        entry["path"] = "/usr/share/passwd-link"
        break
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

mutate_signature_timestamp() {
    local root="$1"
    python3 - "$root/MANIFEST.sig" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
data["timestamp"] = "2020-01-01T00:00:00Z"
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

build_signed_fixture() {
    local fixture_dir="$1"
    local output_name="$2"
    local key_path="$3"
    local built_ccs
    build_fixture "$fixture_dir"
    built_ccs="$(find "$fixture_dir/output" -maxdepth 1 -name '*.ccs' | head -1)"
    "$CONARY_BIN" ccs sign "$built_ccs" --key "$key_path" --output "$fixture_dir/output/$output_name"
}

mutate_decompression_bomb() {
    local root="$1"
    local payload="$2"
    python3 - "$root" "$payload" <<'PY'
import hashlib
import json
import os
import sys

root = sys.argv[1]
payload = sys.argv[2]
objects_dir = os.path.join(root, "objects")
runtime_path = os.path.join(root, "components", "runtime.json")

with open(runtime_path, "r", encoding="utf-8") as f:
    data = json.load(f)
with open(payload, "rb") as f:
    content = f.read()

digest = hashlib.sha256(content).hexdigest()
os.makedirs(os.path.join(objects_dir, digest[:2]), exist_ok=True)
with open(os.path.join(objects_dir, digest[:2], digest[2:]), "wb") as f:
    f.write(content)

entry = data["files"][0]
entry["size"] = len(content)
entry["sha256"] = digest

with open(runtime_path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

build_decompression_bomb() {
    local fixture_dir="$1"
    local output_path="$fixture_dir/output/decompression-bomb.ccs"
    local tmpdir
    tmpdir="$(mktemp -d)"
    mkdir -p "$fixture_dir/output"
    rm -f "$fixture_dir/output/"*.ccs
    python3 - "$tmpdir/huge-zero.bin" <<'PY'
import os
import sys
with open(sys.argv[1], "wb") as f:
    f.truncate(600 * 1024 * 1024)
PY
    "$CONARY_BIN" ccs build "$fixture_dir/ccs.toml" \
        --source "$fixture_dir/stage" \
        --output "$fixture_dir/output/"
    local base_ccs
    base_ccs="$(find "$fixture_dir/output" -maxdepth 1 -name '*.ccs' | head -1)"
    rm -f "$output_path"
    mutate_ccs_with_payload "$base_ccs" "$output_path" "$tmpdir/huge-zero.bin"
    rm -rf "$tmpdir"
}

chmod 4755 "$SCRIPT_DIR/malicious/setuid/stage/usr/bin/setuid-helper"
make_executable "$SCRIPT_DIR/malicious/path-traversal/stage/usr/bin/traversal-check"
make_executable "$SCRIPT_DIR/malicious/hostile-scriptlet/stage/usr/bin/hostile-scriptlet"
make_executable "$SCRIPT_DIR/malicious/proc-environ/stage/usr/bin/proc-environ"
make_executable "$SCRIPT_DIR/malicious/outside-root-write/stage/usr/bin/outside-root-write"
make_executable "$SCRIPT_DIR/malicious/cap-net-raw/stage/usr/bin/cap-net-raw"
make_executable "$SCRIPT_DIR/malicious/capability-overflow/stage/usr/bin/capability-overflow"
make_executable "$SCRIPT_DIR/malicious/decompression-bomb/stage/usr/bin/decompression-bomb"
make_executable "$SCRIPT_DIR/malicious/expired-signature/stage/usr/bin/expired-signature"

echo "Building base malicious fixtures..."
for fixture in path-traversal symlink-attack setuid hostile-scriptlet proc-environ outside-root-write cap-net-raw capability-overflow; do
    build_fixture "$SCRIPT_DIR/malicious/$fixture"
done

key_base="$SCRIPT_DIR/malicious/expired-signature/output/expired-signature-key"
mkdir -p "$SCRIPT_DIR/malicious/expired-signature/output"
if [ ! -f "${key_base}.private" ]; then
    "$CONARY_BIN" ccs keygen --output "$key_base" --key-id "expired-signature" --force
fi
public_key="$(python3 - <<PY
import pathlib
import tomllib
path = pathlib.Path("${key_base}.public")
with path.open("rb") as f:
    data = tomllib.load(f)
print(data["key"])
PY
)"
cat > "$SCRIPT_DIR/malicious/expired-signature/trust-policy.toml" <<EOF
trusted_keys = ["$public_key"]
allow_unsigned = false
require_timestamp = true
max_signature_age = 86400
EOF
build_signed_fixture \
    "$SCRIPT_DIR/malicious/expired-signature" \
    "expired-signature.ccs" \
    "${key_base}.private"

echo "Mutating expired signature timestamp..."
signed_src="$SCRIPT_DIR/malicious/expired-signature/output/expired-signature.ccs"
expired_dst="$SCRIPT_DIR/malicious/expired-signature/output/expired-signature-expired.ccs"
mutate_ccs "$signed_src" "$expired_dst" mutate_signature_timestamp
mv "$expired_dst" "$signed_src"

echo "Building decompression bomb fixture..."
build_decompression_bomb "$SCRIPT_DIR/malicious/decompression-bomb"

cp \
    "$(find "$SCRIPT_DIR/malicious/proc-environ/output" -maxdepth 1 -name 'proc-environ-*.ccs' | head -1)" \
    "$SCRIPT_DIR/malicious/proc-environ/output/proc-environ.ccs"
cp \
    "$(find "$SCRIPT_DIR/malicious/outside-root-write/output" -maxdepth 1 -name 'outside-root-write-*.ccs' | head -1)" \
    "$SCRIPT_DIR/malicious/outside-root-write/output/outside-root-write.ccs"
cp \
    "$(find "$SCRIPT_DIR/malicious/cap-net-raw/output" -maxdepth 1 -name 'cap-net-raw-*.ccs' | head -1)" \
    "$SCRIPT_DIR/malicious/cap-net-raw/output/cap-net-raw.ccs"
cp \
    "$(find "$SCRIPT_DIR/malicious/capability-overflow/output" -maxdepth 1 -name 'capability-overflow-*.ccs' | head -1)" \
    "$SCRIPT_DIR/malicious/capability-overflow/output/capability-overflow.ccs"

echo "Mutating path traversal fixture..."
path_src="$(find "$SCRIPT_DIR/malicious/path-traversal/output" -maxdepth 1 -name '*.ccs' | head -1)"
path_dst="$SCRIPT_DIR/malicious/path-traversal/output/path-traversal-malicious.ccs"
mutate_ccs "$path_src" "$path_dst" mutate_path_traversal

echo "Mutating symlink attack fixture..."
symlink_src="$(find "$SCRIPT_DIR/malicious/symlink-attack/output" -maxdepth 1 -name '*.ccs' | head -1)"
symlink_dst="$SCRIPT_DIR/malicious/symlink-attack/output/symlink-attack-malicious.ccs"
mutate_ccs "$symlink_src" "$symlink_dst" mutate_symlink_attack

echo "[OK] Malicious fixtures built:"
printf '  %s\n' \
    "$path_dst" \
    "$symlink_dst" \
    "$(find "$SCRIPT_DIR/malicious/setuid/output" -maxdepth 1 -name '*.ccs' | head -1)" \
    "$(find "$SCRIPT_DIR/malicious/hostile-scriptlet/output" -maxdepth 1 -name '*.ccs' | head -1)" \
    "$(find "$SCRIPT_DIR/malicious/proc-environ/output" -maxdepth 1 -name '*.ccs' | head -1)" \
    "$(find "$SCRIPT_DIR/malicious/outside-root-write/output" -maxdepth 1 -name '*.ccs' | head -1)" \
    "$(find "$SCRIPT_DIR/malicious/cap-net-raw/output" -maxdepth 1 -name '*.ccs' | head -1)" \
    "$(find "$SCRIPT_DIR/malicious/capability-overflow/output" -maxdepth 1 -name '*.ccs' | head -1)" \
    "$(find "$SCRIPT_DIR/malicious/expired-signature/output" -maxdepth 1 -name 'expired-signature.ccs' | head -1)" \
    "$(find "$SCRIPT_DIR/malicious/decompression-bomb/output" -maxdepth 1 -name 'decompression-bomb.ccs' | head -1)"
