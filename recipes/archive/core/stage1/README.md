# Stage 1: Self-Hosted Toolchain Recipes

Stage 1 builds a self-hosted toolchain using the Stage 0 cross-compiler.
After Stage 1 is complete, we have a native toolchain that can build
all remaining packages without relying on the host system.

## Build Order

The toolchain must be built in this exact order due to dependencies:

```
1. linux-headers  (kernel API headers, no compilation)
        │
        ▼
2. binutils       (assembler, linker, nm, ar, etc.)
        │
        ▼
3. gcc-pass1      (minimal C compiler, no threads/shared libs)
        │
        ▼
4. glibc          (C library, needs gcc-pass1)
        │
        ▼
5. gcc-pass2      (full C/C++ compiler with libstdc++)
```

## Package Versions (Jan 2026)

| Package | Version | Notes |
|---------|---------|-------|
| linux-headers | 6.18 | LTS kernel |
| binutils | 2.45.1 | Latest stable |
| gcc | 15.2.0 | Latest stable |
| glibc | 2.42 | Latest stable |
| gmp | 6.3.0 | GCC dependency |
| mpfr | 4.2.1 | GCC dependency |
| mpc | 1.3.1 | GCC dependency |
| isl | 0.27 | GCC dependency (pass2 only) |

## Building Stage 1

```bash
# Ensure Stage 0 is complete
conary bootstrap status

# Build Stage 1 (orchestrated)
conary bootstrap stage1

# Or build individual packages
conary cook recipes/core/stage1/linux-headers.toml --root /conary/sysroot/stage1
conary cook recipes/core/stage1/binutils.toml --root /conary/sysroot/stage1
conary cook recipes/core/stage1/gcc-pass1.toml --root /conary/sysroot/stage1
conary cook recipes/core/stage1/glibc.toml --root /conary/sysroot/stage1
conary cook recipes/core/stage1/gcc-pass2.toml --root /conary/sysroot/stage1
```

## The GCC Bootstrap Problem

GCC needs glibc to compile C programs, but glibc needs GCC to compile.
This chicken-and-egg problem is solved with a two-pass build:

**Pass 1**: Build a minimal GCC with `--without-headers --disable-shared --disable-threads`.
This GCC can compile C code but produces static executables only.

**glibc**: Build glibc using gcc-pass1. This gives us the C library.

**Pass 2**: Rebuild GCC with full C/C++ support, threads, and shared libs.
Now we have a complete, self-hosted toolchain.

## Sysroot Layout

After Stage 1 completes, `/conary/sysroot/stage1` contains:

```
/conary/sysroot/stage1/
├── usr/
│   ├── bin/           # gcc, g++, ld, as, ar, ...
│   ├── lib/           # libc.so, libstdc++.so, libgcc_s.so, ...
│   ├── include/       # C/C++ headers
│   └── libexec/       # GCC internal tools
└── lib64/
    └── ld-linux-x86-64.so.2  # Dynamic linker
```

## Verification

After building, verify the toolchain:

```bash
# Check GCC works
/conary/sysroot/stage1/usr/bin/gcc --version

# Check it can compile
echo 'int main() { return 0; }' | \
    /conary/sysroot/stage1/usr/bin/gcc -x c - -o /tmp/test

# Check binary is linked against our glibc
ldd /tmp/test
# Should show: /conary/sysroot/stage1/lib64/ld-linux-x86-64.so.2

# Check C++ works
echo '#include <iostream>
int main() { std::cout << "Hello\\n"; }' | \
    /conary/sysroot/stage1/usr/bin/g++ -x c++ - -o /tmp/test_cpp
/tmp/test_cpp
# Should output: Hello
```

## Next Steps

After Stage 1 is complete, proceed to build the base system:

```bash
conary bootstrap base
```

This builds the remaining packages needed for a bootable system:
- Kernel (linux)
- Init system (systemd)
- Core utilities (coreutils, util-linux, bash)
- Networking (iproute2, openssh)
