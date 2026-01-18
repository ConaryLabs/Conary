# Stage 0: Cross-Compilation Toolchain

Stage 0 produces a static cross-compiler that runs on any Linux host and targets
`x86_64-conary-linux-gnu`. This toolchain is used to build Stage 1 (the self-hosted
toolchain).

## Components

| Component | Version | Purpose |
|-----------|---------|---------|
| GCC | 13.3.0 | C/C++ compiler |
| glibc | 2.39 | C library |
| binutils | 2.42 | Assembler, linker, etc. |
| Linux headers | 6.6.70 | Kernel API headers |

## Prerequisites

Install crosstool-ng:

```bash
# Fedora/RHEL
sudo dnf install crosstool-ng

# Arch
sudo pacman -S crosstool-ng

# Or build from source
git clone https://github.com/crosstool-ng/crosstool-ng.git
cd crosstool-ng
./bootstrap && ./configure --prefix=/usr/local
make && sudo make install
```

## Building

```bash
cd bootstrap/stage0

# Build the toolchain (takes 30-60 minutes)
ct-ng build

# Result is installed to /tools/x86_64-conary-linux-gnu/
ls /tools/bin/x86_64-conary-linux-gnu-gcc
```

## Using the Toolchain

Set environment variables:

```bash
export PATH="/tools/bin:$PATH"
export CC="x86_64-conary-linux-gnu-gcc"
export CXX="x86_64-conary-linux-gnu-g++"
export AR="x86_64-conary-linux-gnu-ar"
export RANLIB="x86_64-conary-linux-gnu-ranlib"
```

## Verification

```bash
# Check toolchain is static (no shared lib deps)
ldd /tools/bin/x86_64-conary-linux-gnu-gcc
# Should output: "not a dynamic executable"

# Check target
/tools/bin/x86_64-conary-linux-gnu-gcc -dumpmachine
# Should output: x86_64-conary-linux-gnu

# Test compile
echo 'int main() { return 0; }' | /tools/bin/x86_64-conary-linux-gnu-gcc -x c - -o /tmp/test
file /tmp/test
# Should show: ELF 64-bit LSB executable, x86-64
```

## Configuration Details

Key settings in `crosstool.config`:

- `CT_STATIC_TOOLCHAIN=y` - Toolchain is fully static, runs anywhere
- `CT_TARGET_VENDOR="conary"` - Identifies our custom toolchain
- `CT_GLIBC_OLDEST_ABI="2.17"` - Binaries run on RHEL 7+ / glibc 2.17+
- `CT_ARCH_ARCH="x86-64-v2"` - Modern x86_64 baseline (SSE4.2, POPCNT)

## Rebuilding

If you need to modify the configuration:

```bash
# Interactive configuration
ct-ng menuconfig

# Save changes (overwrites crosstool.config)
ct-ng savedefconfig DEFCONFIG=crosstool.config

# Clean and rebuild
ct-ng distclean
ct-ng build
```

## Caching

The build downloads source tarballs to `tarballs/`. These can be cached:

```bash
# Pre-populate tarballs
ct-ng source

# Tarballs are in bootstrap/stage0/tarballs/
```

## Next Steps

After Stage 0 is built, use it to build Stage 1:

```bash
conary bootstrap stage1
```

Stage 1 rebuilds gcc, glibc, and binutils using the Stage 0 toolchain,
producing a fully native (self-hosted) toolchain.
