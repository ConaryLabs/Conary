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

chmod 4755 "$SCRIPT_DIR/malicious/setuid/stage/usr/bin/setuid-helper"

echo "Building base malicious fixtures..."
for fixture in path-traversal symlink-attack setuid hostile-scriptlet; do
    build_fixture "$SCRIPT_DIR/malicious/$fixture"
done

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
    "$(find "$SCRIPT_DIR/malicious/hostile-scriptlet/output" -maxdepth 1 -name '*.ccs' | head -1)"
