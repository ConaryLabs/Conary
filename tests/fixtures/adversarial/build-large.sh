#!/usr/bin/env bash
# tests/fixtures/adversarial/build-large.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONARY_BIN="${1:-${CONARY_BIN:-$(pwd)/target/debug/conary}}"
LARGE_DIR="$SCRIPT_DIR/large"

LARGE_PACKAGE_MB="${LARGE_PACKAGE_MB:-128}"
LARGE_PACKAGE_FILE_COUNT="${LARGE_PACKAGE_FILE_COUNT:-64}"
TEN_K_FILE_COUNT="${TEN_K_FILE_COUNT:-10000}"
DEEP_TREE_DEPTH="${DEEP_TREE_DEPTH:-128}"
TEMP_ROOT=""

cleanup() {
    if [ -n "${TEMP_ROOT:-}" ] && [ -d "$TEMP_ROOT" ]; then
        rm -rf "$TEMP_ROOT"
    fi
}

trap cleanup EXIT

build_fixture() {
    local fixture_name="$1"
    local source_dir="$2"
    local final_name="$3"
    local package_name="$4"
    local description="$5"

    local work_dir output_dir manifest_path built_ccs
    work_dir="$(mktemp -d)"
    output_dir="$work_dir/output"
    manifest_path="$work_dir/ccs.toml"
    mkdir -p "$output_dir"

    cat > "$manifest_path" <<EOF
[package]
name = "$package_name"
version = "1.0.0"
description = "$description"
license = "MIT"

[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"

[provides]
capabilities = ["$package_name"]
binaries = []

[requires]
capabilities = []
packages = []

[components]
default = ["runtime"]

[hooks]
EOF

    "$CONARY_BIN" ccs build "$manifest_path" \
        --source "$source_dir" \
        --output "$output_dir/"

    built_ccs="$(find "$output_dir" -maxdepth 1 -name '*.ccs' | head -1)"
    if [ -z "$built_ccs" ]; then
        echo "FATAL: failed to build $fixture_name" >&2
        rm -rf "$work_dir"
        exit 1
    fi

    mv "$built_ccs" "$LARGE_DIR/$final_name"
    rm -rf "$work_dir"
}

generate_large_package_stage() {
    local stage_dir="$1"
    local mib_per_file remainder_mib file_mib
    mib_per_file=$(( LARGE_PACKAGE_MB / LARGE_PACKAGE_FILE_COUNT ))
    remainder_mib=$(( LARGE_PACKAGE_MB % LARGE_PACKAGE_FILE_COUNT ))

    mkdir -p "$stage_dir/usr/share/large-package"
    for i in $(seq 1 "$LARGE_PACKAGE_FILE_COUNT"); do
        file_mib="$mib_per_file"
        if [ "$i" -le "$remainder_mib" ]; then
            file_mib=$(( file_mib + 1 ))
        fi
        dd if=/dev/zero \
            of="$stage_dir/usr/share/large-package/blob-$(printf '%03d' "$i").bin" \
            bs=1M \
            count="$file_mib" \
            status=none
    done
}

generate_ten_k_stage() {
    local stage_dir="$1"
    python3 - "$stage_dir" "$TEN_K_FILE_COUNT" <<'PY'
import pathlib
import sys

stage_dir = pathlib.Path(sys.argv[1])
count = int(sys.argv[2])

for i in range(1, count + 1):
    shard = f"{i % 100:02d}"
    file_path = stage_dir / "usr/share/ten-k-files" / shard / f"file-{i:05d}.txt"
    file_path.parent.mkdir(parents=True, exist_ok=True)
    file_path.write_text(f"fixture-file-{i:05d}\n", encoding="utf-8")
PY
}

generate_deep_tree_stage() {
    local stage_dir="$1"
    python3 - "$stage_dir" "$DEEP_TREE_DEPTH" <<'PY'
import pathlib
import sys

current_dir = pathlib.Path(sys.argv[1])
depth = int(sys.argv[2])

for level in range(1, depth + 1):
    current_dir = current_dir / f"level-{level:03d}"
    current_dir.mkdir(parents=True, exist_ok=True)
    (current_dir / "node.txt").write_text(f"depth={level:03d}\n", encoding="utf-8")
PY
}

main() {
    local temp_root large_stage ten_k_stage deep_stage

    mkdir -p "$LARGE_DIR"
    rm -f "$LARGE_DIR"/large-package.ccs "$LARGE_DIR"/10k-files.ccs "$LARGE_DIR"/deep-tree.ccs

    temp_root="$(mktemp -d)"
    TEMP_ROOT="$temp_root"

    large_stage="$temp_root/large-package-stage"
    ten_k_stage="$temp_root/ten-k-stage"
    deep_stage="$temp_root/deep-tree-stage"

    echo "Generating large package stage (${LARGE_PACKAGE_MB} MiB across ${LARGE_PACKAGE_FILE_COUNT} files)..."
    generate_large_package_stage "$large_stage"
    build_fixture \
        "large-package" \
        "$large_stage" \
        "large-package.ccs" \
        "large-package" \
        "Large package stress fixture for disk-full and interrupted install tests"

    echo "Generating 10k-files stage (${TEN_K_FILE_COUNT} files)..."
    generate_ten_k_stage "$ten_k_stage"
    build_fixture \
        "10k-files" \
        "$ten_k_stage" \
        "10k-files.ccs" \
        "ten-k-files" \
        "Metadata-heavy fixture with many small files"

    echo "Generating deep-tree stage (${DEEP_TREE_DEPTH} levels)..."
    generate_deep_tree_stage "$deep_stage"
    build_fixture \
        "deep-tree" \
        "$deep_stage" \
        "deep-tree.ccs" \
        "deep-tree" \
        "Deeply nested directory fixture for path traversal and tree-walk stress"

    echo "[OK] Large fixtures built:"
    printf '  %s\n' \
        "$LARGE_DIR/large-package.ccs" \
        "$LARGE_DIR/10k-files.ccs" \
        "$LARGE_DIR/deep-tree.ccs"
    TEMP_ROOT=""
    rm -rf "$temp_root"
}

main "$@"
