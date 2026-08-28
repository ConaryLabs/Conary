#!/usr/bin/env bash
# scripts/build-static-conary.sh
set -euo pipefail

# Build fully static Conary integration artifacts for x86_64-unknown-linux-musl.
#
# Why this exists: the QEMU suites stage the host-built `conary` into a guest,
# which couples the guest to the build host's glibc and to a matching
# libseccomp.so.2. A statically linked binary removes both couplings, so the
# guest distro and the build host can drift freely.
#
# The only dependency that needs work is libseccomp. The `libseccomp-sys` crate
# links a system library and vendors nothing, so this script builds a static
# libseccomp for musl once and caches it, then points the crate at it through
# the two environment variables its build script reads.

# shellcheck source=scripts/kernel-header-roots.sh
source "$(dirname "${BASH_SOURCE[0]}")/kernel-header-roots.sh"

LIBSECCOMP_VERSION="2.6.0"
LIBSECCOMP_SHA256="83b6085232d1588c379dc9b9cae47bb37407cf262e6e74993c61ba72d2a784dc"
LIBSECCOMP_URL="https://github.com/seccomp/libseccomp/releases/download/v${LIBSECCOMP_VERSION}/libseccomp-${LIBSECCOMP_VERSION}.tar.gz"

TARGET="x86_64-unknown-linux-musl"

usage() {
    cat <<'EOF'
Usage: build-static-conary.sh [OPTIONS]

Build a fully static `conary` for x86_64-unknown-linux-musl, suitable for
staging into any Linux guest regardless of its glibc. The optional protected
matrix mode also builds the host-side integration harness and its library test
executable once for immutable artifact distribution to isolated CI jobs.

Options:
  --cache-dir PATH   Where the static libseccomp is built and cached
                     (default: ${XDG_CACHE_HOME:-$HOME/.cache}/conary-static-deps)
  --rebuild-deps     Rebuild the cached static libseccomp even if present
  --with-test-harness
                     Also build static conary-test CLI and library-test binaries
  --print-path       After building, print the resulting binary path only
  --help             Show this help text

Host requirements: a Rust toolchain with the x86_64-unknown-linux-musl target
(`rustup target add x86_64-unknown-linux-musl`), `musl-gcc`, the Linux kernel
UAPI headers (linux-libc-dev, kernel-headers, or linux-api-headers), plus
curl, tar, make, gperf, file, and jq.

Debug profile only. Conary is pre-alpha and this binary is a test-staging
artifact, not a release artifact.
EOF
}

CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/conary-static-deps"
REBUILD_DEPS="no"
PRINT_PATH="no"
WITH_TEST_HARNESS="no"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --cache-dir)
            CACHE_DIR="$2"
            shift 2
            ;;
        --rebuild-deps)
            REBUILD_DEPS="yes"
            shift
            ;;
        --print-path)
            PRINT_PATH="yes"
            shift
            ;;
        --with-test-harness)
            WITH_TEST_HARNESS="yes"
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

fail() {
    echo "error: $*" >&2
    exit 1
}

log() {
    if [[ "$PRINT_PATH" != "yes" ]]; then
        echo "$*"
    fi
}

for tool in cargo rustc musl-gcc curl tar make gperf sha256sum file jq; do
    command -v "$tool" >/dev/null || fail "host is missing required tool: $tool"
done

rustc --print target-list | grep -qx "$TARGET" || fail "rustc does not know target $TARGET"
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
    fail "target $TARGET is not installed; run: rustup target add $TARGET"
fi

# musl-gcc compiles with musl's headers only, and libseccomp needs the Linux
# UAPI headers, which are not part of musl. Where those headers live, and
# whether they even live together, is distribution-specific; the probe owns
# that knowledge and returns every directory that has to be searched.
mapfile -t KERNEL_HEADER_CANDIDATES < <(kernel_header_candidates)
if ! kernel_header_root_lines="$(kernel_header_roots "${KERNEL_HEADER_CANDIDATES[@]}")"; then
    exit 1
fi
mapfile -t KERNEL_HEADER_ROOTS <<< "$kernel_header_root_lines"
KERNEL_HEADER_FLAGS="$(kernel_header_include_flags "${KERNEL_HEADER_ROOTS[@]}")"
log "kernel UAPI headers: ${KERNEL_HEADER_ROOTS[*]}"

PREFIX="$CACHE_DIR/libseccomp-$LIBSECCOMP_VERSION-musl"
if [[ "$REBUILD_DEPS" == "yes" ]]; then
    rm -rf "$PREFIX"
fi

dependency_started_ns="$(date -u +%s%N)"
dependency_cache_hit="true"
if [[ ! -f "$PREFIX/lib/libseccomp.a" ]]; then
    dependency_cache_hit="false"
    log "building static libseccomp $LIBSECCOMP_VERSION for musl"
    BUILD_DIR="$(mktemp -d)"
    trap 'rm -rf "$BUILD_DIR"' EXIT

    tarball="$CACHE_DIR/libseccomp-$LIBSECCOMP_VERSION.tar.gz"
    mkdir -p "$CACHE_DIR"
    if [[ ! -f "$tarball" ]]; then
        curl -fSL --retry 3 -o "$tarball.part" "$LIBSECCOMP_URL"
        mv "$tarball.part" "$tarball"
    fi
    actual="$(sha256sum "$tarball" | cut -d' ' -f1)"
    if [[ "$actual" != "$LIBSECCOMP_SHA256" ]]; then
        rm -f "$tarball"
        fail "libseccomp tarball checksum mismatch
  expected $LIBSECCOMP_SHA256
  actual   $actual
The cached download has been removed; re-run to fetch it again."
    fi

    tar xzf "$tarball" -C "$BUILD_DIR"
    (
        cd "$BUILD_DIR/libseccomp-$LIBSECCOMP_VERSION"
        CC=musl-gcc CFLAGS="-O2 $KERNEL_HEADER_FLAGS" \
            ./configure --prefix="$PREFIX" --enable-static --disable-shared
        make -j"$(nproc)"
        make install
    )
    [[ -f "$PREFIX/lib/libseccomp.a" ]] || fail "static libseccomp build produced no libseccomp.a"
    rm -rf "$BUILD_DIR"
    trap - EXIT
else
    log "using cached static libseccomp at $PREFIX"
fi
dependency_finished_ns="$(date -u +%s%N)"
dependency_ms=$(( (dependency_finished_ns - dependency_started_ns) / 1000000 ))

repo_root="$(git rev-parse --show-toplevel)"

log "building static integration artifacts for $TARGET"
cargo_packages=(-p conary)
if [[ "$WITH_TEST_HARNESS" == "yes" ]]; then
    cargo_packages+=(-p conary-test)
fi
runtime_started_ns="$(date -u +%s%N)"
(
    cd "$repo_root"
    LIBSECCOMP_LIB_PATH="$PREFIX/lib" \
    LIBSECCOMP_LINK_TYPE=static \
    CC_x86_64_unknown_linux_musl=musl-gcc \
    CFLAGS_x86_64_unknown_linux_musl="$KERNEL_HEADER_FLAGS" \
        cargo build "${cargo_packages[@]}" --target "$TARGET" --locked
)
runtime_finished_ns="$(date -u +%s%N)"
runtime_build_ms=$(( (runtime_finished_ns - runtime_started_ns) / 1000000 ))

test_build_ms=0
if [[ "$WITH_TEST_HARNESS" == "yes" ]]; then
    test_started_ns="$(date -u +%s%N)"
    test_executable="$(
        (
            cd "$repo_root"
            LIBSECCOMP_LIB_PATH="$PREFIX/lib" \
            LIBSECCOMP_LINK_TYPE=static \
            CC_x86_64_unknown_linux_musl=musl-gcc \
            CFLAGS_x86_64_unknown_linux_musl="$KERNEL_HEADER_FLAGS" \
                cargo test -p conary-test --lib --target "$TARGET" --locked \
                    --no-run --message-format=json-render-diagnostics |
                jq -r '
                    select(.reason == "compiler-artifact")
                    | select(.target.name == "conary_test")
                    | select(.profile.test == true)
                    | .executable // empty
                ' |
                tail -n 1
        )
    )"
    [[ -n "$test_executable" && -x "$test_executable" ]] ||
        fail "cargo did not report the conary-test library test executable"
    install -m 0755 "$test_executable" \
        "${CARGO_TARGET_DIR:-$repo_root/target}/$TARGET/debug/conary-test-library-tests"
    test_finished_ns="$(date -u +%s%N)"
    test_build_ms=$(( (test_finished_ns - test_started_ns) / 1000000 ))
fi

if [[ -n "${CONARY_STATIC_BUILD_METRICS_PATH:-}" ]]; then
    metrics_parent="$(dirname "$CONARY_STATIC_BUILD_METRICS_PATH")"
    mkdir -p "$metrics_parent"
    jq -n \
        --argjson dependency_cache_hit "$dependency_cache_hit" \
        --argjson dependency_ms "$dependency_ms" \
        --argjson runtime_build_ms "$runtime_build_ms" \
        --argjson test_build_ms "$test_build_ms" \
        --arg with_test_harness "$WITH_TEST_HARNESS" '
          {
            schema_version: 1,
            static_dependency_cache_hit: $dependency_cache_hit,
            static_dependency_ms: $dependency_ms,
            static_runtime_build_ms: $runtime_build_ms,
            library_test_build_ms: $test_build_ms,
            with_test_harness: ($with_test_harness == "yes")
          }
        ' > "$CONARY_STATIC_BUILD_METRICS_PATH"
fi

target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
binary="$target_dir/$TARGET/debug/conary"
[[ -x "$binary" ]] || fail "expected a binary at $binary"

artifacts=("$binary")
if [[ "$WITH_TEST_HARNESS" == "yes" ]]; then
    artifacts+=(
        "$target_dir/$TARGET/debug/conary-test"
        "$target_dir/$TARGET/debug/conary-test-library-tests"
    )
fi

# A dynamically linked result would silently reintroduce the coupling this
# script exists to remove, so refuse to report success for one.
for artifact in "${artifacts[@]}"; do
    [[ -x "$artifact" ]] || fail "expected a binary at $artifact"
    if ! file "$artifact" | grep -q 'static-pie linked\|statically linked'; then
        fail "built binary is not statically linked: $(file "$artifact")"
    fi
done

if [[ "$PRINT_PATH" == "yes" ]]; then
    printf '%s\n' "$binary"
    exit 0
fi

echo
echo "== result =="
for artifact in "${artifacts[@]}"; do
    file "$artifact"
    ldd "$artifact" 2>&1 || true
done
"$binary" --version
if [[ "$WITH_TEST_HARNESS" == "yes" ]]; then
    "$target_dir/$TARGET/debug/conary-test" --version
    "$target_dir/$TARGET/debug/conary-test-library-tests" --list >/dev/null
fi
echo "path: $binary"
