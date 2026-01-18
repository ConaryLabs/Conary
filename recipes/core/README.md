# Conary Core Bootstrap Recipes

This directory contains the complete recipe set for bootstrapping a
self-hosting Conary Linux system from scratch.

## Overview

These 52 packages provide everything needed to build a bootable Linux
system capable of compiling itself. The recipes follow a staged approach,
starting with cross-compilation tools and progressing to a fully native
system.

## Package Categories

| Category | Count | Description |
|----------|-------|-------------|
| [stage1/](stage1/) | 5 | Cross-compiler toolchain bootstrap |
| [base/](base/) | 7 | Core system (kernel, shell, init, networking) |
| [libs/](libs/) | 12 | Essential libraries (compression, crypto, terminal) |
| [dev/](dev/) | 14 | Development tools (compilers, build systems) |
| [text/](text/) | 8 | Text processing utilities |
| [archive/](archive/) | 4 | Compression and archiving tools |
| [net/](net/) | 3 | Networking tools (curl, wget, certificates) |
| [vcs/](vcs/) | 1 | Version control (git) |
| [sys/](sys/) | 4 | System administration utilities |
| [editors/](editors/) | 2 | Text editors (vim, nano) |
| [boot/](boot/) | 5 | Bootloader and EFI tools |

**Total: 52 packages**

## Complete Package List

### Stage 1: Toolchain Bootstrap
| Package | Version | Description |
|---------|---------|-------------|
| linux-headers | 6.18 | Linux kernel headers for cross-compilation |
| binutils | 2.45.1 | GNU assembler, linker, and binary utilities |
| gcc-pass1 | 15.2.0 | GCC Pass 1 - minimal C compiler |
| glibc | 2.42 | GNU C Library |
| gcc-pass2 | 15.2.0 | GCC Pass 2 - full C/C++ compiler |

### Base System
| Package | Version | Description |
|---------|---------|-------------|
| linux | 6.18 | Linux kernel and headers |
| coreutils | 9.9 | GNU core utilities (ls, cp, mv, etc.) |
| bash | 5.3 | GNU Bourne-Again Shell |
| util-linux | 2.41.3 | System utilities (mount, fdisk, etc.) |
| systemd | 257.9 | System and service manager |
| iproute2 | 6.15.0 | Network configuration tools |
| openssh | 10.1p1 | Secure shell client and server |

### Libraries
| Package | Version | Description |
|---------|---------|-------------|
| zlib | 1.3.1 | Compression library |
| xz | 5.6.4 | XZ/LZMA compression |
| zstd | 1.5.6 | Zstandard compression |
| ncurses | 6.6 | Terminal handling library |
| readline | 8.3 | Command line editing library |
| openssl | 3.5.4 | TLS/SSL and cryptography |
| libcap | 2.73 | POSIX capabilities library |
| libmnl | 1.0.5 | Netlink library |
| elfutils | 0.194 | ELF handling utilities |
| kmod | 34 | Kernel module tools |
| dbus | 1.16.2 | Message bus system |
| linux-pam | 1.7.1 | Pluggable Authentication Modules |

### Development Tools
| Package | Version | Description |
|---------|---------|-------------|
| make | 4.4.1 | GNU Make build tool |
| m4 | 1.4.20 | GNU M4 macro processor |
| autoconf | 2.72 | Configure script generator |
| automake | 1.18.1 | Makefile generator |
| libtool | 2.5.4 | Generic library support |
| pkgconf | 2.3.0 | Package compiler/linker flag tool |
| bison | 3.8.2 | Parser generator |
| flex | 2.6.4 | Lexical analyzer generator |
| gettext | 0.26 | Internationalization tools |
| perl | 5.42.0 | Perl programming language |
| python | 3.14.2 | Python programming language |
| cmake | 4.2.1 | Cross-platform build system |
| ninja | 1.13.1 | Small build system |
| meson | 1.7.0 | High-level build system |

### Text Processing
| Package | Version | Description |
|---------|---------|-------------|
| grep | 3.12 | Pattern matching utility |
| sed | 4.9 | Stream editor |
| gawk | 5.3.2 | GNU AWK |
| less | 685 | Terminal pager |
| diffutils | 3.12 | File comparison utilities |
| patch | 2.8 | Apply patches to files |
| findutils | 4.10.0 | File finding utilities |
| file | 5.46 | File type detection |

### Compression/Archiving
| Package | Version | Description |
|---------|---------|-------------|
| tar | 1.35 | Tape archiver |
| gzip | 1.13 | GNU zip compression |
| bzip2 | 1.0.8 | Block-sorting compression |
| cpio | 2.15 | Copy in/out archiver |

### Networking
| Package | Version | Description |
|---------|---------|-------------|
| curl | 8.18.0 | Data transfer tool and library |
| wget2 | 2.2.1 | Network file retriever |
| ca-certificates | 2026.01 | Mozilla CA certificate bundle |

### Version Control
| Package | Version | Description |
|---------|---------|-------------|
| git | 2.53.0 | Distributed version control |

### System Utilities
| Package | Version | Description |
|---------|---------|-------------|
| procps-ng | 4.1.0 | Process monitoring (ps, top, free) |
| psmisc | 23.7 | Process utilities (killall, pstree) |
| shadow | 4.16.0 | User management (useradd, passwd) |
| sudo | 1.9.18 | Privilege escalation |

### Editors
| Package | Version | Description |
|---------|---------|-------------|
| vim | 9.2 | Vi Improved text editor |
| nano | 8.0 | Simple text editor |

### Boot Tools
| Package | Version | Description |
|---------|---------|-------------|
| grub | 2.14 | GRand Unified Bootloader |
| efibootmgr | 18 | EFI Boot Manager |
| efivar | 39 | EFI variable tools |
| popt | 1.19 | Option parsing library |
| dosfstools | 4.2 | FAT filesystem utilities |

## Bootstrap Build Order

The complete bootstrap follows this dependency order:

```
┌─────────────────────────────────────────────────────────────────────┐
│                     STAGE 0: Host Tools                             │
│  (Use existing system compiler to build cross-compiler)             │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│                  STAGE 1: Cross-Compiler Toolchain                  │
│                                                                     │
│  binutils ──► gcc-pass1 ──► glibc ──► gcc-pass2                    │
│                                                                     │
│  Result: Native compiler that doesn't depend on host               │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│                    STAGE 2: Core Libraries                          │
│                                                                     │
│  zlib, xz, zstd          (compression)                             │
│  ncurses, readline       (terminal)                                │
│  openssl                 (crypto)                                  │
│  libcap, linux-pam       (security)                                │
│  elfutils, kmod          (kernel support)                          │
│  libmnl, dbus            (IPC)                                     │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│                    STAGE 3: Development Tools                       │
│                                                                     │
│  make, m4                (basic build)                             │
│  autoconf, automake      (autotools)                               │
│  libtool, pkgconf        (library tools)                           │
│  bison, flex             (parsers)                                 │
│  gettext                 (i18n)                                    │
│  perl, python            (scripting)                               │
│  cmake, ninja, meson     (modern build systems)                    │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      STAGE 4: Base System                           │
│                                                                     │
│  linux (kernel)          coreutils, bash, util-linux               │
│  systemd                 iproute2, openssh                         │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│                     STAGE 5: Userland Tools                         │
│                                                                     │
│  Text:    grep, sed, gawk, less, diffutils, patch, findutils, file │
│  Archive: tar, gzip, bzip2, cpio                                   │
│  Net:     ca-certificates, curl, wget2                             │
│  VCS:     git                                                      │
│  Sys:     procps-ng, psmisc, shadow, sudo                          │
│  Editors: vim, nano                                                │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│                       STAGE 6: Boot Tools                           │
│                                                                     │
│  popt ──► efivar ──► efibootmgr                                    │
│  dosfstools                                                        │
│  grub (BIOS + UEFI)                                                │
└─────────────────────────────────────────────────────────────────────┘
```

## Building

### Full Bootstrap

```bash
# Complete bootstrap from scratch
conary bootstrap full

# Or stage by stage
conary bootstrap stage1
conary bootstrap libs
conary bootstrap dev
conary bootstrap base
conary bootstrap text
conary bootstrap archive
conary bootstrap net
conary bootstrap vcs
conary bootstrap sys
conary bootstrap editors
conary bootstrap boot
```

### Individual Packages

```bash
# Build a single package
conary cook recipes/core/libs/zlib.toml

# Build with verbose output
conary cook -v recipes/core/base/bash.toml

# Build for specific target
conary cook --target=x86_64-conary-linux-gnu recipes/core/dev/gcc.toml
```

## Recipe Format

All recipes use TOML format with these sections:

```toml
[package]
name = "example"
version = "1.0.0"
release = "1"
summary = "Short description"
description = """
Long description of the package.
"""
license = "GPL-3.0-or-later"
homepage = "https://example.org"

[source]
archive = "https://example.org/example-%(version)s.tar.xz"
checksum = "sha256:..."
signature = "https://example.org/example-%(version)s.tar.xz.sig"  # optional

[build]
requires = ["glibc", "zlib"]           # Runtime dependencies
makedepends = ["gcc", "make"]          # Build-time dependencies

configure = """
./configure --prefix=/usr
"""

make = "make -j%(jobs)s"

install = """
make DESTDIR=%(destdir)s install
"""

[variables]
jobs = "$(nproc)"
```

### Variable Substitution

| Variable | Description |
|----------|-------------|
| `%(version)s` | Package version |
| `%(release)s` | Package release |
| `%(destdir)s` | Installation destination |
| `%(srcdir)s` | Source directory |
| `%(jobs)s` | Parallel job count |

## Version Management

All package versions are centralized in [versions.toml](versions.toml):

```toml
[toolchain]
gcc = "15.2.0"
glibc = "2.42"
binutils = "2.45.1"

[libs]
zlib = "1.3.1"
openssl = "3.5.4"
# ...
```

To update a package version:
1. Edit `versions.toml`
2. Update the checksum in the package recipe
3. Test build the package

## Directory Structure

```
recipes/core/
├── README.md           # This file
├── versions.toml       # Central version configuration
├── stage1/             # Toolchain bootstrap
│   ├── README.md
│   ├── binutils.toml
│   ├── gcc-pass1.toml
│   ├── glibc.toml
│   └── gcc-pass2.toml
├── base/               # Core system
├── libs/               # Libraries
├── dev/                # Development tools
├── text/               # Text processing
├── archive/            # Compression tools
├── net/                # Networking
├── vcs/                # Version control
├── sys/                # System utilities
├── editors/            # Text editors
└── boot/               # Bootloader
```

## Checksums

All recipes include SHA-256 checksums for source verification.
Checksums marked `FIXME_ACTUAL_CHECKSUM` need to be updated
before building.

To get a checksum:
```bash
curl -sL https://example.org/package-1.0.tar.xz | sha256sum
```

## Contributing

When adding new recipes:

1. Follow the existing format and naming conventions
2. Add the version to `versions.toml`
3. Include proper dependencies in `requires` and `makedepends`
4. Test the build before committing
5. Update the category README with build order information

## License

These recipes are part of the Conary project and are provided under
the same license as Conary itself. Individual packages have their
own licenses as noted in each recipe.
