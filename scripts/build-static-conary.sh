#!/usr/bin/env bash
# scripts/build-static-conary.sh
set -euo pipefail

# Build a fully static `conary` for x86_64-unknown-linux-musl.
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

LIBSECCOMP_VERSION="2.6.0"
LIBSECCOMP_SHA256="83b6085232d1588c379dc9b9cae47bb37407cf262e6e74993c61ba72d2a784dc"
LIBSECCOMP_URL="https://github.com/seccomp/libseccomp/releases/download/v${LIBSECCOMP_VERSION}/libseccomp-${LIBSECCOMP_VERSION}.tar.gz"

TARGET="x86_64-unknown-linux-musl"

usage() {
    cat <<'EOF'
Usage: build-static-conary.sh [OPTIONS]

Build a fully static `conary` for x86_64-unknown-linux-musl, suitable for
staging into any Linux guest regardless of its glibc.

Options:
  --cache-dir PATH   Where the static libseccomp is built and cached
                     (default: ${XDG_CACHE_HOME:-$HOME/.cache}/conary-static-deps)
  --rebuild-deps     Rebuild the cached static libseccomp even if present
  --print-path       After building, print the resulting binary path only
  --help             Show this help text

Host requirements: a Rust toolchain with the x86_64-unknown-linux-musl target
(`rustup target add x86_64-unknown-linux-musl`), `musl-gcc`, Linux kernel
headers under /usr/include, plus curl, tar, make, and gperf.

Debug profile only. Conary is pre-alpha and this binary is a test-staging
artifact, not a release artifact.
EOF
}

CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/conary-static-deps"
REBUILD_DEPS="no"
PRINT_PATH="no"

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

for tool in cargo rustc musl-gcc curl tar make gperf sha256sum; do
    command -v "$tool" >/dev/null || fail "host is missing required tool: $tool"
done

rustc --print target-list | grep -qx "$TARGET" || fail "rustc does not know target $TARGET"
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
    fail "target $TARGET is not installed; run: rustup target add $TARGET"
fi

# musl-gcc compiles with musl's headers only. libseccomp needs the Linux UAPI
# headers (linux/audit.h, asm/unistd.h), which are not part of musl. -idirafter
# adds them at the end of the search path so they are found without shadowing
# any musl header of the same name.
[[ -d /usr/include/linux && -d /usr/include/asm ]] ||
    fail "Linux kernel headers not found under /usr/include; libseccomp cannot be built for musl without them"

PREFIX="$CACHE_DIR/libseccomp-$LIBSECCOMP_VERSION-musl"
if [[ "$REBUILD_DEPS" == "yes" ]]; then
    rm -rf "$PREFIX"
fi

if [[ ! -f "$PREFIX/lib/libseccomp.a" ]]; then
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
        CC=musl-gcc CFLAGS="-O2 -idirafter /usr/include" \
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

repo_root="$(git rev-parse --show-toplevel)"

log "building conary for $TARGET"
(
    cd "$repo_root"
    LIBSECCOMP_LIB_PATH="$PREFIX/lib" \
    LIBSECCOMP_LINK_TYPE=static \
    CC_x86_64_unknown_linux_musl=musl-gcc \
    CFLAGS_x86_64_unknown_linux_musl="-idirafter /usr/include" \
        cargo build -p conary --target "$TARGET"
)

target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
binary="$target_dir/$TARGET/debug/conary"
[[ -x "$binary" ]] || fail "expected a binary at $binary"

# A dynamically linked result would silently reintroduce the coupling this
# script exists to remove, so refuse to report success for one.
if ! file "$binary" | grep -q 'static-pie linked\|statically linked'; then
    fail "built binary is not statically linked: $(file "$binary")"
fi

if [[ "$PRINT_PATH" == "yes" ]]; then
    printf '%s\n' "$binary"
    exit 0
fi

echo
echo "== result =="
file "$binary"
ldd "$binary" 2>&1 || true
"$binary" --version
echo "path: $binary"
