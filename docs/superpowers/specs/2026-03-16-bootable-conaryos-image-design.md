# Bootable conaryOS Image — Design Spec

## Overview

Fix the Conary bootstrap pipeline so that `conary bootstrap base && conary bootstrap image --format qcow2` produces a fully bootable conaryOS image. The image boots via UEFI using systemd-boot, reaches a multi-user systemd target, provides an interactive serial console login, and runs an SSH daemon for automated test access.

## Goals

- `conary bootstrap base && conary bootstrap image --format qcow2` produces a bootable image
- Image boots via UEFI in QEMU (and VirtualBox, bare metal with EFI)
- Interactive login works on serial console and virtual terminal
- SSH works for automated test access (T156 QEMU test passes)
- Image is useful for manual exploration (shell, common tools, networking)
- Branded as **conaryOS** (conaryos.com) — the OS distribution built with the Conary package manager

## Non-Goals

- Graphical desktop / GUI
- Network-based package installation during boot (image is self-contained)
- Multi-architecture (x86_64 only this iteration)
- Secure boot signing
- Legacy BIOS boot (UEFI only)

## Current State

The bootstrap pipeline builds 60 packages (systemd, openssh, kernel, grub, coreutils, bash, etc.) and the image builder produces GPT-partitioned qcow2 images via systemd-repart. However, the produced image is not bootable due to six gaps plus one bug.

### Gaps

1. **`populate_sysroot()` never called** — creates `/etc/passwd`, `/etc/shadow`, `/etc/fstab`, etc. Exists and works (unit tested) but only called from tests, not the build path.

2. **No EFI bootloader installed** — `setup_efi_boot()` searches for GRUB EFI binary in the sysroot, falls back to an unimplemented stub that writes nothing. Result: ESP has no bootloader.

3. **No initramfs generated** — dracut is referenced in the `package_phase()` classifier but is NOT in `BOOT_PACKAGES`. It is never installed or invoked.

4. **SSH not configured** — openssh is built but no `sshd_config`, no host keys, no authorized keys, sshd service not enabled.

5. **No systemd default target** — systemd is installed but no default boot target is set. System would hang at `sysinit.target`.

6. **No networking configured** — no network daemon enabled, no DHCP config. QEMU provides a DHCP server but without a client running, the guest gets no IP address and SSH is unreachable.

### Bug: Partition Label Mismatch

| Source | Root label | ESP label |
|--------|-----------|-----------|
| `repart.rs` definitions | `"root"` | `"ESP"` |
| `fstab` in `populate_sysroot()` | `CONARY_ROOT` | `CONARY_ESP` |
| Boot config in `image.rs` | `CONARY_ROOT` | — |

systemd-repart creates partitions with labels `root` and `ESP`, but fstab and the boot config reference `CONARY_ROOT` and `CONARY_ESP`. The kernel panics at boot because it cannot find the root filesystem.

### Bug: ESP Mount Point Mismatch

`fstab` in both `populate_sysroot()` (base.rs:1128) and `create_fstab()` (image.rs:939) mounts the ESP at `/boot/efi`. But systemd-repart's ESP definition uses `CopyFiles=/boot:/` — files placed in `<sysroot>/boot/` end up at the ESP root. At runtime, the ESP must be mounted at `/boot` (not `/boot/efi`) so that `/boot/vmlinuz-<ver>` resolves to the ESP. Mounting at `/boot/efi` breaks `bootctl` and future kernel updates.

## Design

### Bootloader: systemd-boot (not GRUB)

systemd-boot is simpler and already available:

- **Ships with systemd** — the EFI binary (`systemd-bootx64.efi`) is a file to copy, no `grub-mkimage` build step needed
- **BLS config** — plain text Boot Loader Specification entries, trivial to generate
- **Already expected** — T151 tests for BLS entries at `/boot/loader/entries/*.conf`
- **UEFI only** — matches our "strictly full EFI boot" requirement

**Prerequisite**: The systemd recipe must be built with `-Dbootloader=true` (meson flag) to produce `systemd-bootx64.efi`. Verify this in the recipe before implementation.

### SSH Authentication: Key-Based (Not Password)

The QEMU test runner uses `ssh -o BatchMode=yes` which disables interactive password prompts. Empty-password auth is incompatible with BatchMode. The correct approach:

- `finalize_sysroot()` generates an Ed25519 keypair
- Public key → `<sysroot>/root/.ssh/authorized_keys`
- Private key → `<sysroot>/boot/test-ssh-key` (included on the ESP for test tooling to retrieve, also copied to a well-known local path)
- QEMU test runner uses `-i <private_key_path>` for SSH connections
- sshd_config still permits root login but key-based auth is the primary mechanism

### File Changes

#### 1. `conary-core/src/bootstrap/base.rs`

**Add `dracut` to `BOOT_PACKAGES`**: dracut is referenced in `package_phase()` but not in any package list. Add `("dracut", "boot")` to `BOOT_PACKAGES`. Update package count from 60 to 61.

**Extend `populate_sysroot()`** — add these files to the existing function:

```
/etc/ssh/sshd_config                — PermitRootLogin yes, PubkeyAuthentication yes
/etc/os-release                     — conaryOS branding (update existing)
/etc/hostname                       — "conaryos" (update existing)
/etc/fstab                          — fix ESP mount: /boot not /boot/efi (update existing)
/etc/nsswitch.conf                  — passwd/group: files, hosts: files dns
/etc/systemd/network/80-dhcp.network — DHCP on all en* interfaces
/root/.bashrc                       — minimal prompt with hostname
```

Systemd service wiring (symlinks):
```
/etc/systemd/system/default.target
  → /usr/lib/systemd/system/multi-user.target

/etc/systemd/system/multi-user.target.wants/sshd.service
  → /usr/lib/systemd/system/sshd.service

/etc/systemd/system/multi-user.target.wants/systemd-networkd.service
  → /usr/lib/systemd/system/systemd-networkd.service

/etc/systemd/system/getty.target.wants/serial-getty@ttyS0.service
  → /usr/lib/systemd/system/serial-getty@.service
```

**`/etc/os-release` update:**
```
NAME="conaryOS"
ID=conaryos
VERSION_ID=0.1
PRETTY_NAME="conaryOS 0.1 (Bootstrap)"
HOME_URL="https://conaryos.com"
```

**`/etc/fstab` update (fix ESP mount):**
```
LABEL=CONARY_ROOT  /      ext4  defaults,noatime  0 1
LABEL=CONARY_ESP   /boot  vfat  defaults,noatime  0 2
tmpfs              /tmp   tmpfs defaults,nosuid   0 0
```

**`/etc/systemd/network/80-dhcp.network`:**
```
[Match]
Name=en*

[Network]
DHCP=yes
```

**Add `finalize_sysroot(sysroot: &Path)`** — new function, runs host tools against the populated sysroot after packages are installed:

1. **Detect kernel version**: scan `<sysroot>/usr/lib/modules/` for directory names
2. **Copy kernel**: `<sysroot>/usr/lib/modules/<ver>/vmlinuz` → `<sysroot>/boot/vmlinuz-<ver>`
3. **Bind-mount filesystems**: mount `--bind /proc`, `/sys`, `/dev`, `/dev/pts` into sysroot (required for dracut)
4. **Generate initramfs**: `chroot <sysroot> dracut --no-hostonly --force /boot/initramfs-<ver>.img <ver>`
5. **Unmount bind-mounts**: in reverse order (`/dev/pts`, `/dev`, `/sys`, `/proc`)
6. **Write loader config**: `<sysroot>/boot/loader/loader.conf`
7. **Write BLS entry**: `<sysroot>/boot/loader/entries/conaryos.conf`
8. **Copy systemd-boot EFI binary**: search paths in order:
   - `<sysroot>/usr/lib/systemd/boot/efi/systemd-bootx64.efi`
   - `/usr/lib/systemd/boot/efi/systemd-bootx64.efi` (host fallback)
   - Hard error if neither exists (not a warning)
   Copy to `<sysroot>/boot/EFI/BOOT/BOOTX64.EFI`
9. **Generate SSH host keys**: explicit per-key generation (not `ssh-keygen -A -f` which doesn't work for sysroots):
   ```
   ssh-keygen -t ed25519 -f <sysroot>/etc/ssh/ssh_host_ed25519_key -N ""
   ssh-keygen -t rsa -b 4096 -f <sysroot>/etc/ssh/ssh_host_rsa_key -N ""
   ssh-keygen -t ecdsa -f <sysroot>/etc/ssh/ssh_host_ecdsa_key -N ""
   ```
10. **Generate test SSH keypair**:
    ```
    ssh-keygen -t ed25519 -f <sysroot>/root/.ssh/conaryos-test-key -N ""
    cp <sysroot>/root/.ssh/conaryos-test-key.pub <sysroot>/root/.ssh/authorized_keys
    chmod 700 <sysroot>/root/.ssh
    chmod 600 <sysroot>/root/.ssh/authorized_keys
    ```
    Also copy the private key to a well-known location for the test runner.

`finalize_sysroot()` is separate from `populate_sysroot()` because populate creates static config files (no host tools needed), while finalize runs commands that depend on installed packages and host tooling.

**`/boot/loader/loader.conf`:**
```
default conaryos.conf
timeout 3
console-mode auto
editor no
```

**`/boot/loader/entries/conaryos.conf`:**
```
title   conaryOS
linux   /vmlinuz-<ver>
initrd  /initramfs-<ver>.img
options root=LABEL=CONARY_ROOT ro console=ttyS0,115200
```

**File placement rationale**: systemd-boot expects the kernel, initramfs, and loader config on the ESP. The repart ESP definition uses `CopyFiles=/boot:/` — everything in `<sysroot>/boot/` is copied to the ESP root. So placing files under `<sysroot>/boot/` is sufficient; systemd-repart handles the rest.

#### 2. `conary-core/src/bootstrap/image.rs`

**Replace GRUB EFI setup with systemd-boot.** Delete or deprecate:
- `setup_efi_boot()` — GRUB EFI binary search + stub fallback
- `create_grub_config()` — GRUB menu configuration
- `create_stub_efi()` — unimplemented placeholder
- `create_fstab()` — now handled by `populate_sysroot()` in base.rs

The EFI binary copy and boot config generation move to `finalize_sysroot()` in `base.rs` (they operate on the sysroot before image generation, not during it). `image.rs` no longer needs boot-specific logic — systemd-repart's `CopyFiles=/boot:/` handles everything.

Mark `generate_initramfs()` (the busybox-based fallback at line 1312) as deprecated — it hardcodes `/dev/vda2` and conflicts with the dracut approach. Add `#[allow(dead_code)]` with a comment.

The `grub_install` tool detection in `ImageTools` can remain with a comment: `// Retained for potential future BIOS boot support`.

#### 3. `conary-core/src/bootstrap/repart.rs`

Fix partition labels to match fstab and boot config:

```rust
// ESP: "ESP" → "CONARY_ESP"
label: Some("CONARY_ESP".to_string()),

// Root: "root" → "CONARY_ROOT"
label: Some("CONARY_ROOT".to_string()),
```

Update test assertions to match.

#### 4. `src/commands/bootstrap/mod.rs`

Wire the new steps into `cmd_bootstrap_base()`, after the `bootstrap.build_base()` call returns:

```rust
let sysroot_path = PathBuf::from(root);
BaseBuilder::populate_sysroot(&sysroot_path)?;
BaseBuilder::finalize_sysroot(&sysroot_path)?;
```

`populate_sysroot()` creates config files. `finalize_sysroot()` runs host tools (dracut, ssh-keygen, kernel copy, BLS generation).

#### 5. `conary-test/src/engine/qemu.rs`

Update SSH connection to use key-based auth:

- Add `-i <private_key_path>` to SSH args in `run_ssh_command()`
- Private key location: check `~/.cache/conary-test/conaryos-test-key` first, fall back to downloading from `<artifact_base_url>/conaryos-test-key`
- Remove `BatchMode=yes` (replaced by key-based auth which doesn't need it, or keep it since key auth works with BatchMode)

#### 6. `tests/integration/remi/manifests/phase3-group-n-qemu.toml`

Update T156 commands to validate the full boot:

```toml
commands = [
    "uname -r",
    "systemctl is-system-running --wait || true",
    "id -un",
    "cat /etc/os-release | grep conaryOS",
    "echo boot-verified",
]
expect_output = [
    "boot-verified",
]
```

### Boot Sequence

```
UEFI firmware (OVMF in QEMU)
  → loads EFI/BOOT/BOOTX64.EFI (systemd-boot)
  → reads /loader/loader.conf
  → selects /loader/entries/conaryos.conf
  → loads /vmlinuz-<ver> + /initramfs-<ver>.img from ESP
  → kernel boots with root=LABEL=CONARY_ROOT console=ttyS0,115200
  → initramfs finds CONARY_ROOT partition, mounts it
  → systemd starts as PID 1
  → systemd-networkd acquires DHCP lease
  → reaches multi-user.target
  → serial-getty@ttyS0 provides console login
  → sshd listens on port 22
```

### Build & Publish Workflow

After the pipeline fixes:

```bash
# On Remi (has packages, toolchain, disk space):
conary bootstrap init --work-dir /tmp/conaryos-build --target x86_64
conary bootstrap base --work-dir /tmp/conaryos-build --root /tmp/conaryos-build/sysroot
conary bootstrap image --work-dir /tmp/conaryos-build --output /conary/test-artifacts/minimal-boot-v2.qcow2 --format qcow2 --size 4G

# Copy the test SSH private key for the test runner:
cp /tmp/conaryos-build/sysroot/root/.ssh/conaryos-test-key /conary/test-artifacts/

# Verify manually:
qemu-system-x86_64 -m 1024 -bios /usr/share/edk2/ovmf/OVMF_CODE.fd \
  -drive file=minimal-boot-v2.qcow2,format=qcow2 \
  -netdev user,id=net0,hostfwd=tcp::2222-:22 -device e1000,netdev=net0 \
  -nographic -serial mon:stdio -accel kvm
```

### Testing Strategy

**Unit tests:**
- Extend `test_populate_sysroot_creates_files` to verify SSH config, systemd symlinks, DHCP network file, nsswitch.conf, conaryOS branding
- New test for `finalize_sysroot()` verifying file placement (mock the chroot/ssh-keygen calls, verify correct arguments)
- Existing repart tests updated for new labels

**Integration:**
- T156 validates full boot, systemd running, conaryOS branding, SSH accessible
- Manual VM testing for interactive login experience

### Branding

- **conaryOS** is the operating system distribution (conaryos.com)
- **Conary** is the package manager
- Same relationship as pacman/Arch Linux, apt/Debian
- `/etc/os-release` identifies the OS as conaryOS
- Partition labels (`CONARY_ROOT`, `CONARY_ESP`) reference the package manager — this is fine
- The `conary` CLI binary name is unchanged
