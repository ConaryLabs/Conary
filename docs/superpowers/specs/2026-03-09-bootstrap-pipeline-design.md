# Bootstrap Pipeline Design

## Overview

A tiered bootstrap pipeline that builds a self-hosting Conary Linux system from scratch on Remi, validated by QEMU at each stage.

## Pipeline Architecture

```
Stage 0 (cross-toolchain)
  ↓
Stage 1 (self-hosted toolchain: linux-headers, binutils, gcc, glibc)
  ↓
Tier A: Minimal Boot (16 packages)
  → raw image → QEMU boot test → login prompt
  ↓
Tier B: Full Base (~60 packages)
  → updated image → GRUB boot + SSH test
  ↓
Tier C: Self-Hosting (Rust + Conary)
  → final image → self-rebuild test
```

Stage 2 (reproducibility rebuild) is skipped for now. Each tier checkpoints and produces a bootable image. The sysroot is additive -- Tier B extends Tier A, Tier C extends Tier B.

**Stage 1 carry-forward:** Stage 1 builds glibc, gcc, and binutils into the sysroot. These are reused directly by all subsequent tiers -- glibc is not rebuilt.

## Target Environment

- **Host:** Remi (12 cores, 64GB RAM, 2TB disk)
- **Architecture:** x86_64 only
- **Bootstrap root:** `/conary/bootstrap/`
- **Validation:** QEMU (no hardware boot required)

## Directory Layout

```
/conary/bootstrap/
  bootstrap-state.json    # Pipeline state (current tier, completed packages)
  sources/                # Downloaded source tarballs (cached across runs)
  tools/                  # Stage 0 cross-toolchain
  stage1/                 # Stage 1 self-hosted toolchain
  sysroot/                # Target root filesystem (grows across tiers)
  build/                  # Per-package build directories (cleaned after success)
  logs/                   # Per-package build logs + pipeline log
  images/                 # Output images (one per tier)
    tier-a.img
    tier-b.img
    tier-c.img
```

## Tier A: Minimal Boot

Goal: smallest package set that boots to a login prompt under QEMU.

### Package Set (16 packages, dependency-ordered)

| Order | Package | Why |
|-------|---------|-----|
| 1 | zlib | Compression library, needed by almost everything |
| 2 | xz | Decompression for kernel modules |
| 3 | zstd | Compression for kernel/initramfs |
| 4 | openssl | Crypto library, needed by kmod and systemd |
| 5 | ncurses | Terminal handling, needed by bash |
| 6 | readline | Line editing, needed by bash |
| 7 | libcap | Linux capabilities, needed by systemd |
| 8 | kmod | Module loading, needed by systemd + kernel |
| 9 | elfutils | ELF utilities, needed by systemd |
| 10 | dbus | IPC bus, needed by systemd |
| 11 | linux-pam | Auth, needed by login/systemd |
| 12 | util-linux | mount, login, agetty, fdisk |
| 13 | coreutils | ls, cp, cat, chmod -- the basics |
| 14 | bash | Shell |
| 15 | systemd | Init system, PID 1 |
| 16 | linux | Kernel + modules |

Note: glibc is carried forward from Stage 1 and is not rebuilt. `iproute2` and `libmnl` are deferred to Tier B (networking is not needed for Tier A's boot-to-login-prompt goal), despite `iproute2` being in `base.rs`'s `CORE_PACKAGES` constant. The code will need a minor adjustment to support the tier split.

### Sysroot Population

Before imaging, the orchestrator creates essential system files in the sysroot:

- `/etc/passwd` -- root entry with empty password
- `/etc/group` -- root, wheel, tty groups
- `/etc/shadow` -- root with no password hash (permits passwordless login)
- `/etc/hostname` -- `conary`
- `/etc/os-release` -- Conary Linux identity (needed by systemd)
- `/etc/machine-id` -- empty file (systemd generates on first boot)
- `/etc/fstab` -- root partition mount

### Boot Strategy

Direct-boot via QEMU `-kernel` flag, no bootloader needed:

```bash
qemu-system-x86_64 \
  -kernel sysroot/boot/vmlinuz \
  -initrd images/initramfs.img \
  -append "root=/dev/vda2 console=ttyS0 init=/lib/systemd/systemd" \
  -drive file=images/tier-a.img,format=raw \
  -m 1024 -nographic -no-reboot
```

### Initramfs

Minimal cpio archive with static busybox. The busybox binary is downloaded from the official busybox.net static builds (x86_64, musl-linked, checksum-verified).

```
/init                    # Shell script
/bin/busybox             # Static binary (~1MB, from busybox.net)
/dev/console             # Device node
/proc/                   # Mount point
/sys/                    # Mount point
/mnt/root/               # Root mount point
```

`/init` script:

```sh
#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev
mount /dev/vda2 /mnt/root
exec switch_root /mnt/root /lib/systemd/systemd
```

The initramfs is installed into the sysroot at `/boot/initramfs.img` so both QEMU direct-boot and GRUB (Tier B) can reference the same file.

## Tier B: Full Base

Extends Tier A sysroot with remaining ~45 packages from existing recipes:
- Networking: libmnl, iproute2, openssh, curl, wget2, ca-certificates
- Dev tools: make, autoconf, cmake, ninja, meson, perl, python, etc.
- Text tools: grep, sed, gawk, less, diffutils, patch, findutils, file
- Archive tools: tar, gzip, bzip2, cpio
- System: procps-ng, psmisc, shadow, sudo
- Editors: vim, nano
- VCS: git
- Boot: grub, efibootmgr, efivar, dosfstools, popt

Tier B image boots via GRUB (installed into the image) instead of QEMU direct-boot.

## Tier C: Self-Hosting

Extends Tier B sysroot with:
1. **Rust** -- download official Rust 1.94.0 bootstrap binary, build in sysroot
2. **Conary** -- `cargo build` conary inside the sysroot

Recipes live at `recipes/conary/` (not `recipes/core/conary/`). The recipe directory path must account for this.

Validates by cloning conary source inside the VM and running `cargo build`.

## Orchestrator Script

`scripts/bootstrap-remi.sh` -- to be created. Bash script that runs on Remi.

### Usage

```bash
./scripts/bootstrap-remi.sh --tier all     # Full pipeline
./scripts/bootstrap-remi.sh --tier a       # Single tier
./scripts/bootstrap-remi.sh --resume       # Resume after failure
./scripts/bootstrap-remi.sh --clean        # Clean start
```

### Responsibilities

1. Check prerequisites (Rust toolchain, QEMU, parted, mkfs)
2. Build conary from source if not present
3. Run `conary bootstrap stage0` then `conary bootstrap stage1`
4. For each tier, iterate packages in order via `conary bootstrap base --package <name>`
5. After each tier, populate sysroot (etc files), generate image, run QEMU smoke test
6. Log everything to `logs/pipeline.log` + per-package logs
7. On failure: save state, print failure details + log path, exit non-zero

### Resume Logic

Reads `bootstrap-state.json` on start. Skips completed packages via `StageManager::completed_packages`. Resumes from the exact package that failed.

## QEMU Validation Tests

### Tier A -- Boot to Login

1. Boot with `-kernel` direct boot (timeout 90s)
2. Wait for "login:" on serial console
3. Log in as root (no password)
4. Run: `uname -r` (verify kernel version)
5. Run: `systemctl is-system-running` (verify systemd started)
6. Poweroff

### Tier B -- Full Base with SSH

1. Boot from disk image via GRUB (timeout 120s)
2. Wait for login prompt
3. SSH in via forwarded port (`-net user,hostfwd=tcp::2222-:22`)
4. Run commands over SSH: `ls`, `grep`, `python3 --version`, `git --version`
5. Verify networking inside VM
6. Poweroff

### Tier C -- Self-Hosting

1. Boot from final image, SSH in
2. Verify: `rustc --version`, `cargo --version`, `conary --version`
3. `conary query list` (verify DB access)
4. Clone conary source, `cargo build` (can it rebuild itself?)
5. Poweroff

## Rust Code Changes

### Working as-is
- Stage 0 (seed download or ct-ng build)
- Stage 1 (5-package toolchain build)
- Stage/checkpoint persistence (`bootstrap-state.json` via `StageManager`)
- Recipe parsing and dependency graph
- CLI commands

### Needs implementation

| Change | File(s) | Description |
|--------|---------|-------------|
| Per-package CLI mode | `src/cli/` (arg defs) + `src/commands/bootstrap/mod.rs` (handler) | Add `--package <name>` and `--tier a/b/c` flags to `conary bootstrap base` |
| Tier-aware base builder | `base.rs` | The current code builds all packages in 5 phases (Libraries/DevTools/CoreSystem/Userland/Boot). Needs refactoring to support tier-based subsets. The `BootstrapStage` enum may need `TierA/TierB/TierC` variants or sub-stage tracking alongside the existing `BaseSystem` stage. |
| Recipe-driven base builds | `base.rs` | Use recipe TOML `configure`/`make`/`install` steps instead of hardcoded Stage 1 logic |
| Initramfs generation | `image.rs` | Download static busybox, generate minimal cpio archive, install to `/boot/initramfs.img` |
| Raw image creation | `image.rs` | Wire up end-to-end: create image, partition (GPT: ESP + root), mkfs, copy sysroot, install kernel |
| `build_rust()` | `conary_stage.rs` | Download Rust 1.94.0 bootstrap binary, build in sysroot |
| `build_conary()` | `conary_stage.rs` | `cargo build` conary inside sysroot |

### Not changing
- Recipe TOML format
- Stage 0/1 code
- Checkpoint/state system (already works)
- CLI structure (only adding flags)

## Out of Scope

- Stage 2 reproducibility rebuild
- ISO output format
- CI workflow for bootstrap
- aarch64/riscv64 support
- Dracut-based initramfs
