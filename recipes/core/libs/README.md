# Core Library Recipes

These recipes build the core libraries required by the base system packages.
They must be built before the packages in `recipes/core/base/`.

## Package Categories

### Compression Libraries
| Package | Version | Description |
|---------|---------|-------------|
| zlib | 1.3.1 | Deflate compression library |
| xz | 5.6.4 | LZMA2 compression (liblzma) |
| zstd | 1.5.6 | Fast real-time compression |

### Text/Terminal Libraries
| Package | Version | Description |
|---------|---------|-------------|
| ncurses | 6.6 | Terminal handling library |
| readline | 8.3 | Line editing library |

### Security/Crypto Libraries
| Package | Version | Description |
|---------|---------|-------------|
| openssl | 3.5.4 | Cryptography and TLS (LTS) |
| libcap | 2.73 | POSIX capabilities |
| linux-pam | 1.7.1 | Pluggable Authentication Modules |

### System Libraries
| Package | Version | Description |
|---------|---------|-------------|
| elfutils | 0.194 | ELF handling (libelf, libdw) |
| libmnl | 1.0.5 | Minimalistic Netlink library |
| dbus | 1.16.2 | Message bus system |
| kmod | 34 | Kernel module handling |

## Build Order

Libraries should be built in this order to satisfy dependencies:

```
Stage 1 Toolchain (must be complete)
         │
         ▼
    ┌─────────┐
    │  zlib   │  (no deps, build first)
    └─────────┘
         │
    ┌────┴────┐
    ▼         ▼
┌──────┐  ┌──────────┐
│  xz  │  │ ncurses  │  (parallel)
└──────┘  └──────────┘
    │         │
    ▼         ▼
┌──────┐  ┌──────────┐
│ zstd │  │ readline │  (zstd needs xz; readline needs ncurses)
└──────┘  └──────────┘
    │
    ▼
┌──────────┐
│ openssl  │  (needs zlib)
└──────────┘
    │
    ├─────────────────────────┐
    ▼                         ▼
┌──────────┐              ┌──────────┐
│ elfutils │              │   kmod   │  (both need compression libs)
└──────────┘              └──────────┘
    │
    ▼
┌──────────┐
│  libmnl  │  (minimal deps)
└──────────┘

┌──────────┐
│  libcap  │  (minimal deps, can build early)
└──────────┘

┌────────────┐
│ linux-pam  │  (minimal deps)
└────────────┘

┌──────────┐
│   dbus   │  (needs systemd for full features, can build minimal)
└──────────┘
```

## Building Libraries

```bash
# Build all libraries (orchestrated)
conary bootstrap libs

# Or build individual libraries
conary cook recipes/core/libs/zlib.toml
conary cook recipes/core/libs/xz.toml
conary cook recipes/core/libs/zstd.toml
conary cook recipes/core/libs/ncurses.toml
conary cook recipes/core/libs/readline.toml
conary cook recipes/core/libs/openssl.toml
conary cook recipes/core/libs/libcap.toml
conary cook recipes/core/libs/libmnl.toml
conary cook recipes/core/libs/elfutils.toml
conary cook recipes/core/libs/kmod.toml
conary cook recipes/core/libs/dbus.toml
conary cook recipes/core/libs/linux-pam.toml
```

## Library Notes

### zlib
- Most widely used compression library
- Required by almost everything
- Build this first!

### xz (liblzma)
- Provides excellent compression ratio
- Used for .xz archives and kernel compression
- Move `xz` binary to /bin for initramfs

### zstd
- Modern compression with great speed/ratio
- Used by kernel, systemd, and many tools
- Supports both zlib and lzma as fallbacks

### ncurses
- Wide character support enabled (`libncursesw`)
- Compatibility symlinks for `libncurses`
- Includes terminfo database

### readline
- Linked against ncurses
- Provides history and editing for bash, python, etc.

### openssl
- Version 3.5.x is current LTS (supported until 2030)
- Kernel TLS (kTLS) support enabled
- Certificates go in `/etc/ssl/certs`

### libcap
- POSIX.1e capabilities
- Used by systemd, ping, etc.
- Go bindings disabled

### linux-pam
- Pluggable authentication
- Basic configuration files installed
- SELinux and audit disabled

### elfutils
- Provides libelf and libdw
- Tools prefixed with `eu-` (eu-readelf, eu-strip, etc.)
- Required for kernel and systemd builds

### libmnl
- Netlink communication
- Required by iproute2

### dbus
- Inter-process communication
- Creates `messagebus` system user
- Socket in `/run/dbus/system_bus_socket`

### kmod
- Kernel module management
- Creates symlinks for modprobe, insmod, rmmod, etc.
- Supports xz, zstd, and zlib compressed modules

## Verification

After building, verify libraries are functional:

```bash
# Check zlib
echo test | gzip | gunzip

# Check xz
echo test | xz | unxz

# Check zstd
echo test | zstd | unzstd

# Check ncurses
tput colors

# Check openssl
openssl version

# Check libelf
eu-readelf --version

# Check kmod
kmod --version

# Check dbus
dbus-daemon --version
```

## Development Headers

Each library recipe installs development headers in `/usr/include` and
pkg-config files in `/usr/lib/pkgconfig`. These are needed for building
dependent packages.

## Next Steps

After building all libraries, proceed to build the base system:

```bash
conary bootstrap base
```
