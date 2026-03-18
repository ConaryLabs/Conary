# LFS 13 Bootstrap Alignment — Design Spec

## Overview

Realign the Conary bootstrap pipeline with Linux From Scratch 13 (systemd version) as the authoritative reference. The current bootstrap builds ~60 packages with ad-hoc configure flags, uses crosstool-ng for cross-compilation, has ~30 missing packages that LFS considers essential, and falls back to host system binaries when things are missing. This redesign produces a fully from-source bootable system where every binary in the image was built by the pipeline.

**Supersedes:** `2026-03-16-bootable-conaryos-image-design.md`

## Goals

- Bootstrap pipeline follows LFS 13 systemd build order and configure flags
- Every binary in the bootable image is built from source (no host fallbacks)
- Toolchain (glibc, binutils, GCC) rebuilt as final-system packages
- Bootable qcow2 boots to login prompt in QEMU
- Tier 2 adds SSH, Conary, and networking for test use
- Every deviation from LFS is documented with rationale

## Non-Goals

- Graphical desktop / GUI
- Multi-architecture (x86_64 only)
- Secure boot signing
- Running LFS test suites during build (Tcl/Expect/DejaGNU skipped)
- BIOS boot support
- initramfs / dracut

## Design Decisions

| Decision | Choice | Alternatives Considered |
|----------|--------|------------------------|
| LFS alignment level | Authoritative guide with documented deviations | Exact mirror; architecture-only reference |
| Cross-compilation | Direct from host GCC (LFS Ch5) | crosstool-ng (current, dropped) |
| Build structure | Two-tier: LFS base + BLFS extras | Single build; minimal boot first |
| Test tooling | Skip (Tcl, Expect, DejaGNU) | Include for build-time test suites |
| Documentation packages | Include (man-pages, groff, man-db, texinfo) | Skip for minimal system |
| Kernel config | defconfig + LFS required + QEMU virtio | QEMU-only minimal; full allmodconfig |
| Bootloader | systemd-boot (deviation from LFS GRUB) | GRUB 2.14 (LFS default) |
| Root filesystem | ext4 + virtio-blk compiled into kernel | initramfs with dracut |
| Conary stage | Tier 2 (after LFS base boots) | End of Tier 1; post-boot install |
| Execution approach | Bottom-up, LFS chapter order | Middle-out (fix existing); top-down (boot target first) |

## Pipeline Architecture

Six phases mapping directly to LFS chapters:

```
Phase 1: Cross-Toolchain (LFS Ch5)
  Input:  Host system compiler
  Build:  5 packages (binutils-pass1, gcc-pass1, linux-headers, glibc, libstdc++)
  Output: Cross-compiler at $LFS/tools/ targeting x86_64-conary-linux-gnu
  Verify: x86_64-conary-linux-gnu-gcc compiles and links a hello.c

Phase 2: Temporary Tools (LFS Ch6-7)
  Input:  Phase 1 cross-compiler
  Build:  23 packages (17 cross-compiled in Ch6, 6 built in chroot in Ch7)
  Output: Minimal system that can be chrooted into for building
  Verify: chroot into $LFS, run bash, make, gcc

Phase 3: Final System (LFS Ch8)
  Input:  Phase 2 chroot environment
  Build:  77 packages in LFS order (81 Ch8 packages minus GRUB, Tcl, Expect, DejaGNU)
  Output: Complete self-hosting Linux system with rebuilt toolchain
  Verify: chroot, run gcc --version, python3 -c "import sqlite3", systemctl --version

Phase 4: System Configuration (LFS Ch9)
  Input:  Phase 3 system
  Work:   /etc files, network, locale, hostname, systemd wiring
  Output: Configured conaryOS system
  Verify: /etc/os-release, /etc/fstab, /etc/hostname, systemd symlinks present

Phase 5: Bootable Image (LFS Ch10, systemd-boot)
  Input:  Phase 4 system
  Build:  Linux kernel (LFS Ch10.3, recipe in system/linux.toml)
  Work:   systemd-boot config, BLS entry (no initrd line), systemd-repart image
  Output: conaryos-base.qcow2 — Tier 1 complete
  Verify: QEMU boots to login prompt, uname -r correct, systemctl status running

Phase 6: Tier 2 (BLFS + Conary)
  Input:  Phase 4 chroot (continues from Phase 3/4 sysroot)
  Build:  8 packages (linux-pam, openssh, ca-certs, curl, sudo, nano, Rust, Conary)
  Work:   Second image creation step
  Output: conaryos.qcow2 — final conaryOS test image
  Verify: SSH into VM works, conary --version, curl to Remi succeeds
```

## Package Inventory

### Phase 1: Cross-Toolchain (5 packages)

All recipes rewritten to match LFS Ch5 configure flags. No crosstool-ng.

| # | Package | Version | LFS Section |
|---|---------|---------|-------------|
| 1 | Binutils Pass 1 | 2.46.0 | 5.2 |
| 2 | GCC Pass 1 | 15.2.0 | 5.3 |
| 3 | Linux API Headers | 6.19.8 | 5.4 |
| 4 | Glibc | 2.43 | 5.5 |
| 5 | Libstdc++ (from GCC) | 15.2.0 | 5.6 |

### Phase 2: Temporary Tools (23 packages)

Cross-compiled with Phase 1 toolchain (Ch6) or built in chroot (Ch7). Minimal configure flags — these are throwaway builds replaced in Phase 3.

| # | Package | Version | LFS Section | Ch |
|---|---------|---------|-------------|----|
| 1 | M4 | 1.4.21 | 6.2 | 6 |
| 2 | Ncurses | 6.6 | 6.3 | 6 |
| 3 | Bash | 5.3 | 6.4 | 6 |
| 4 | Coreutils | 9.10 | 6.5 | 6 |
| 5 | Diffutils | 3.12 | 6.6 | 6 |
| 6 | File | 5.47 | 6.7 | 6 |
| 7 | Findutils | 4.10.0 | 6.8 | 6 |
| 8 | Gawk | 5.4.0 | 6.9 | 6 |
| 9 | Grep | 3.12 | 6.10 | 6 |
| 10 | Gzip | 1.14 | 6.11 | 6 |
| 11 | Make | 4.4.1 | 6.12 | 6 |
| 12 | Patch | 2.8 | 6.13 | 6 |
| 13 | Sed | 4.9 | 6.14 | 6 |
| 14 | Tar | 1.35 | 6.15 | 6 |
| 15 | Xz | 5.8.2 | 6.16 | 6 |
| 16 | Binutils Pass 2 | 2.46.0 | 6.17 | 6 |
| 17 | GCC Pass 2 | 15.2.0 | 6.18 | 6 |
| 18 | Gettext | 1.0 | 7.7 | 7 |
| 19 | Bison | 3.8.2 | 7.8 | 7 |
| 20 | Perl | 5.42.1 | 7.9 | 7 |
| 21 | Python | 3.14.3 | 7.10 | 7 |
| 22 | Texinfo | 7.3 | 7.11 | 7 |
| 23 | Util-linux | 2.41.3 | 7.12 | 7 |

### Phase 3: Final System (77 packages)

Built inside chroot in LFS Ch8 order. Toolchain rebuilt as final-system packages.

| # | Package | Version | LFS Section | Status |
|---|---------|---------|-------------|--------|
| 1 | Man-pages | 6.17 | 8.3 | New |
| 2 | Iana-Etc | 20260306 | 8.4 | New |
| 3 | Glibc | 2.43 | 8.5 | Rewrite |
| 4 | Zlib | 1.3.2 | 8.6 | Update |
| 5 | Bzip2 | 1.0.8 | 8.7 | Update |
| 6 | Xz | 5.8.2 | 8.8 | Update |
| 7 | Lz4 | 1.10.0 | 8.9 | New |
| 8 | Zstd | 1.5.7 | 8.10 | Update |
| 9 | File | 5.47 | 8.11 | Update |
| 10 | Readline | 8.3 | 8.12 | Update |
| 11 | Pcre2 | 10.47 | 8.13 | New |
| 12 | M4 | 1.4.21 | 8.14 | Update |
| 13 | Bc | 7.0.3 | 8.15 | New |
| 14 | Flex | 2.6.4 | 8.16 | Update |
| 15 | Pkgconf | 2.5.1 | 8.17 | Update |
| 16 | Binutils | 2.46.0 | 8.18 | Rewrite |
| 17 | GMP | 6.3.0 | 8.19 | New |
| 18 | MPFR | 4.2.2 | 8.20 | New |
| 19 | MPC | 1.3.1 | 8.21 | New |
| 20 | Attr | 2.5.2 | 8.22 | New |
| 21 | Acl | 2.3.2 | 8.23 | New |
| 22 | Libcap | 2.77 | 8.24 | Update |
| 23 | Libxcrypt | 4.5.2 | 8.25 | New |
| 24 | Shadow | 4.19.4 | 8.26 | Update |
| 25 | GCC | 15.2.0 | 8.27 | Rewrite |
| 26 | Ncurses | 6.6 | 8.28 | Update |
| 27 | Sed | 4.9 | 8.29 | Update |
| 28 | Psmisc | 23.7 | 8.30 | Update |
| 29 | Gettext | 1.0 | 8.31 | Update |
| 30 | Bison | 3.8.2 | 8.32 | Update |
| 31 | Grep | 3.12 | 8.33 | Update |
| 32 | Bash | 5.3 | 8.34 | Update |
| 33 | Libtool | 2.5.4 | 8.35 | Update |
| 34 | GDBM | 1.26 | 8.36 | New |
| 35 | Gperf | 3.3 | 8.37 | New |
| 36 | Expat | 2.7.4 | 8.38 | New |
| 37 | Inetutils | 2.7 | 8.39 | New |
| 38 | Less | 692 | 8.40 | Update |
| 39 | Perl | 5.42.1 | 8.41 | Update |
| 40 | XML::Parser | 2.47 | 8.42 | New |
| 41 | Intltool | 0.51.0 | 8.43 | New |
| 42 | Autoconf | 2.72 | 8.44 | Update |
| 43 | Automake | 1.18.1 | 8.45 | Update |
| 44 | OpenSSL | 3.6.1 | 8.46 | Update |
| 45 | Libelf (Elfutils) | 0.194 | 8.47 | Update |
| 46 | Libffi | 3.5.2 | 8.48 | New |
| 47 | Sqlite | 3510300 | 8.49 | New |
| 48 | Python | 3.14.3 | 8.50 | Update |
| 49 | Flit-Core | 3.12.0 | 8.51 | New |
| 50 | Packaging | 26.0 | 8.52 | New |
| 51 | Wheel | 0.46.3 | 8.53 | New |
| 52 | Setuptools | 82.0.1 | 8.54 | New |
| 53 | Ninja | 1.13.2 | 8.55 | Update |
| 54 | Meson | 1.10.1 | 8.56 | Update |
| 55 | Kmod | 34.2 | 8.57 | Update |
| 56 | Coreutils | 9.10 | 8.58 | Update |
| 57 | Diffutils | 3.12 | 8.59 | Update |
| 58 | Gawk | 5.4.0 | 8.60 | Update |
| 59 | Findutils | 4.10.0 | 8.61 | Update |
| 60 | Groff | 1.24.0 | 8.62 | New |
| 61 | Gzip | 1.14 | 8.63 | Update |
| 62 | IPRoute2 | 6.19.0 | 8.64 | Update |
| 63 | Kbd | 2.9.0 | 8.65 | New |
| 64 | Libpipeline | 1.5.8 | 8.66 | New |
| 65 | Make | 4.4.1 | 8.67 | Update |
| 66 | Patch | 2.8 | 8.68 | Update |
| 67 | Tar | 1.35 | 8.69 | Update |
| 68 | Texinfo | 7.3 | 8.70 | New |
| 69 | Vim | 9.2.0161 | 8.71 | Update |
| 70 | MarkupSafe | 3.0.3 | 8.72 | New |
| 71 | Jinja2 | 3.1.6 | 8.73 | New |
| 72 | Systemd | 259.5 | 8.74 | Update |
| 73 | D-Bus | 1.16.2 | 8.75 | Update |
| 74 | Man-DB | 2.13.1 | 8.76 | New |
| 75 | Procps-ng | 4.0.6 | 8.77 | Update |
| 76 | Util-linux | 2.41.3 | 8.78 | Update |
| 77 | E2fsprogs | 1.47.4 | 8.79 | New |

**Totals: 31 new, 3 rewrite (glibc/binutils/gcc final), 43 update/verify.**

### Phase 6: Tier 2 (8 packages)

| # | Package | Source |
|---|---------|--------|
| 1 | linux-pam | BLFS |
| 2 | openssh | BLFS |
| 3 | ca-certificates | BLFS |
| 4 | curl | BLFS |
| 5 | sudo | BLFS |
| 6 | nano | BLFS |
| 7 | Rust (bootstrap binary) | rustup.rs |
| 8 | Conary | conaryOS-specific |

### Packages Dropped

No longer built (systemd-boot eliminates them from Tier 1):
- grub, efivar, efibootmgr, popt, dosfstools (ESP FAT32 creation is handled by `systemd-repart` on the host during image build; the sysroot itself never needs `mkfs.fat`)

Moved from current base to Tier 2:
- openssh, linux-pam, ca-certificates, curl, sudo, nano

Removed entirely (not needed):
- wget2, git, cmake, cpio, libmnl, dracut

## Recipe Structure

```
recipes/
  cross-tools/           # Phase 1 (LFS Ch5) — 5 recipes
    binutils-pass1.toml
    gcc-pass1.toml
    linux-headers.toml
    glibc.toml
    libstdcxx.toml

  temp-tools/             # Phase 2 (LFS Ch6-7) — 23 recipes
    m4.toml
    ncurses.toml
    bash.toml
    coreutils.toml
    diffutils.toml
    file.toml
    findutils.toml
    gawk.toml
    grep.toml
    gzip.toml
    make.toml
    patch.toml
    sed.toml
    tar.toml
    xz.toml
    binutils-pass2.toml
    gcc-pass2.toml
    gettext.toml
    bison.toml
    perl.toml
    python.toml
    texinfo.toml
    util-linux.toml

  system/                 # Phase 3 + kernel (LFS Ch8 + Ch10.3) — 78 recipes
    man-pages.toml
    iana-etc.toml
    glibc.toml
    ...
    e2fsprogs.toml
    linux.toml              # Kernel, built in Phase 5 (LFS Ch10.3)

  tier2/                  # Phase 6 (BLFS + Conary) — 8 recipes
    linux-pam.toml
    openssh.toml
    ca-certificates.toml
    curl.toml
    sudo.toml
    nano.toml
    rust.toml
    conary.toml
```

Build order for Phase 3 is defined by the Rust code (`final_system.rs`), not by filenames. The LFS section reference in each recipe header provides ordering context. Each recipe includes an LFS section reference comment and, if flags deviate from LFS, a `[deviations]` table documenting each change with rationale. The kernel recipe (`system/linux.toml`) lives in `system/` but is built by Phase 5 (`image.rs`), not Phase 3.

Phases 4 and 5 have no recipe directories — they are Rust-code-only phases. Phase 4 (`system_config.rs`) writes `/etc` files and systemd configuration. Phase 5 (`image.rs`) builds the kernel from `system/linux.toml`, configures the bootloader, and creates the disk image.

Old recipe directories (`recipes/core/`, `recipes/base/`, `recipes/stage1/`, `recipes/conary/`) are deleted.

## Rust Code Changes

### Module Map

| Current Module | Action | New Module |
|----------------|--------|------------|
| `stage0.rs` | Delete | — |
| `stage1.rs` | Refactor | `cross_tools.rs` (Phase 1) |
| `stage2.rs` | Delete | — (optional purity rebuild, superseded by LFS Ch8 toolchain rebuild) |
| `base.rs` | Split | `temp_tools.rs` (Phase 2) + `final_system.rs` (Phase 3) |
| `base.rs` (populate/finalize_sysroot) | Extract | `system_config.rs` (Phase 4) |
| `conary_stage.rs` | Move | `tier2.rs` (Phase 6) |
| `image.rs` | Update | `image.rs` (Phase 5: kernel build, systemd-boot, image creation) |
| `repart.rs` | Keep | `repart.rs` (systemd-repart integration, used by Phase 5) |
| `build_runner.rs` | Update | `build_runner.rs` (cross-compilation context) |
| `build_helpers.rs` | Keep | `build_helpers.rs` (utility functions) |
| `config.rs` | Update | `config.rs` (new phase structure) |
| `stages.rs` | Update | `stages.rs` (new stage enum) |
| `toolchain.rs` | Update | `toolchain.rs` (LFS toolchain paths) |
| `mod.rs` | Update | `mod.rs` (remove `ct-ng` from `Prerequisites::check()`, update exports) |

### Key Behavioral Changes

- **No host fallbacks.** `copy_efi_binary()` no longer falls back to host. `finalize_sysroot()` does not call dracut. `generate_ssh_host_keys()` uses the sysroot's ssh-keygen (Tier 2) or skips (Tier 1). If something is missing, it is a build error.
- **Two build contexts.** `build_runner.rs` handles cross-compilation (Phase 1-2, where `--host` and `--build` differ) and native chroot builds (Phase 3+).
- **Linear build order.** Phase 3 replaces the 5-phase categorization (Libraries/DevTools/CoreSystem/Userland/Boot) with a single ordered list matching LFS Ch8. No dependency graph needed — LFS already solved the ordering.

## Documented LFS Deviations

| # | Deviation | LFS 13 does | conaryOS does | Rationale |
|---|-----------|-------------|---------------|-----------|
| 1 | Bootloader | GRUB 2.14 | systemd-boot | Built as part of systemd (`-Dbootloader=true`). Drops 5 packages. UEFI-only is fine for QEMU and modern hardware. |
| 2 | Test tooling | Tcl, Expect, DejaGNU | Skipped | Only needed for build-time test suites. Can add later. |
| 3 | Target triple | `x86_64-lfs-linux-gnu` | `x86_64-conary-linux-gnu` | Cosmetic. Same mechanism. |
| 4 | Kernel config | `make menuconfig` | `defconfig` + `scripts/config` | Automated equivalent. Recipes need deterministic builds. |
| 5 | Version bumps | Pinned to LFS 13 versions | May bump point releases | Start with LFS versions, bump incrementally with documented rationale. |

## Image Pipeline (Phase 5)

1. Build kernel from `system/linux.toml` recipe (LFS Ch10.3). Install to `/boot/vmlinuz-6.19.8` with modules at `/usr/lib/modules/6.19.8/`.
2. No initramfs — ext4 driver and virtio-blk built into kernel as `=y` (not `=m`)
3. Write `loader.conf` + BLS entry. The BLS entry has **no `initrd` line** (unlike the current code which references `initramfs-{ver}.img`):
   ```
   title   conaryOS
   linux   /vmlinuz-6.19.8
   options root=LABEL=CONARY_ROOT ro console=ttyS0,115200
   ```
4. Copy `systemd-bootx64.efi` from sysroot's `usr/lib/systemd/boot/efi/` to `/boot/EFI/BOOT/BOOTX64.EFI`. Hard error if missing (no host fallback).
5. `systemd-repart` creates GPT: 512MB ESP (FAT32, label CONARY_ESP) + remaining ext4 (label CONARY_ROOT)
6. `qemu-img convert` to `conaryos-base.qcow2`

Host tools for image creation: `systemd-repart`, `qemu-img` only. Everything inside the image is from source.

## Tier 2 Build Mechanism (Phase 6)

Phase 6 continues in the **same chroot** from Phases 3/4 — the sysroot is still mounted and available. Tier 2 packages (openssh, curl, Rust, Conary, etc.) are built and installed into the sysroot using the same `build_runner.rs` recipe execution engine. After all Tier 2 packages are installed, a **second image creation step** produces `conaryos.qcow2`.

This means two image artifacts exist:
- `conaryos-base.qcow2` — Tier 1 (pure LFS, boots to login prompt, no SSH)
- `conaryos.qcow2` — Tier 2 (full conaryOS with SSH, Conary, networking)

Both are independently bootable. Tier 1 is the "did LFS work?" verification. Tier 2 is the actual test platform.

## Verification

| Phase | Check | Pass Criteria |
|-------|-------|---------------|
| 1 | Cross-compiler test | `x86_64-conary-linux-gnu-gcc hello.c -o hello && file hello` shows ELF x86-64, `./hello` prints "Hello" |
| 2 | Chroot test | `chroot $LFS /bin/bash -c "gcc --version && make --version"` succeeds |
| 3 | System completeness | `python3 -c "import sqlite3"`, `systemctl --version`, `mke2fs -V` in chroot |
| 4 | Config presence | `/etc/os-release`, `/etc/fstab`, `/etc/hostname`, systemd symlinks exist |
| 5 | QEMU boot | Boots to login prompt on serial console, `uname -r` correct, `systemctl status` running |
| 6 | Tier 2 functional | SSH key-based login works, `conary --version` returns, `curl https://packages.conary.io/v1/health` succeeds |

Phase 5 maps to existing T156 QEMU boot test (updated for new image name). Phase 6 is what the previous session was working toward.

## Kernel Configuration

Start from `defconfig`, then apply LFS required options and QEMU support:

**LFS required (Ch10):**
- `CONFIG_CGROUPS=y`, `CONFIG_MEMCG=y` (systemd)
- `CONFIG_DEVTMPFS=y`, `CONFIG_DEVTMPFS_MOUNT=y` (udev)
- `CONFIG_TMPFS=y`, `CONFIG_TMPFS_POSIX_ACL=y`
- `CONFIG_INOTIFY_USER=y`
- `CONFIG_NET=y`, `CONFIG_INET=y`, `CONFIG_IPV6=y`

**conaryOS additions (namespaces, containers):**
- `CONFIG_NAMESPACES=y`, `CONFIG_USER_NS=y`, `CONFIG_PID_NS=y`, `CONFIG_NET_NS=y`
- `CONFIG_OVERLAY_FS=y`
- `CONFIG_SECCOMP=y`, `CONFIG_SECCOMP_FILTER=y`

**Built-in (not modules) for no-initramfs boot:**
- `CONFIG_EXT4_FS=y` (root filesystem)
- `CONFIG_VIRTIO=y`, `CONFIG_VIRTIO_PCI=y`, `CONFIG_VIRTIO_BLK=y` (QEMU storage)
- `CONFIG_VIRTIO_NET=y` (QEMU networking)
- `CONFIG_VFAT_FS=y` (ESP partition)

## Scope Summary

- **114 recipe files** across 4 directories (5 cross-tools + 23 temp-tools + 78 system + 8 tier2)
- **31 new recipes**, 3 rewrites, ~43 updates/verifications (Phase 3), plus 5 new cross-tools, 23 temp-tools (mix of new/rewritten), 8 tier2
- **13 Rust modules** touched: 2 deleted, 3 refactored into 5 new modules, 6 updated, 2 kept
- **5 documented deviations** from LFS
- **2 image artifacts** (conaryos-base.qcow2, conaryos.qcow2)
- **0 host fallbacks** in either image
