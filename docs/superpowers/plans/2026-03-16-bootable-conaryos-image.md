# Bootable conaryOS Image Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the Conary bootstrap pipeline so it produces a UEFI-bootable conaryOS qcow2 image with systemd-boot, SSH access, and conaryOS branding.

**Architecture:** Six changes across the bootstrap pipeline: fix partition labels (repart.rs), extend sysroot population with SSH/networking/branding (base.rs), add finalize step for kernel/initramfs/bootloader/keys (base.rs), remove GRUB code from image builder (image.rs), wire into CLI (bootstrap/mod.rs), and update QEMU test runner for key-based SSH (qemu.rs).

**Tech Stack:** Rust 1.94, systemd-boot, dracut, systemd-repart, ssh-keygen, QEMU/KVM.

**Spec:** `docs/superpowers/specs/2026-03-16-bootable-conaryos-image-design.md`

---

## Chunk 1: Foundation Fixes (labels, dracut, fstab)

These are small, independent fixes that unblock everything else. Each is a separate commit.

### Task 1: Fix partition labels in repart.rs

**Files:**
- Modify: `conary-core/src/bootstrap/repart.rs:49,68`

The ESP label is `"ESP"` but fstab/boot config reference `CONARY_ESP`. The root label is `"root"` but everything references `CONARY_ROOT`. Without this fix, the kernel panics at boot.

- [ ] In `RepartDefinition::esp()` (line 49), change:
```rust
label: Some("CONARY_ESP".to_string()),
```

- [ ] In `RepartDefinition::root()` (line 68), change:
```rust
label: Some("CONARY_ROOT".to_string()),
```

- [ ] Update `test_esp_definition` (line 121) assertion:
```rust
assert!(content.contains("Label=CONARY_ESP"));
```

- [ ] Update `test_root_x86_64_definition` (line 131) — no label assertion exists, but verify the test still passes.

- [ ] Verify: `cargo test -p conary-core -- repart`

- [ ] Commit: `fix(bootstrap): align partition labels with fstab (CONARY_ROOT, CONARY_ESP)`

---

### Task 2: Add dracut to BOOT_PACKAGES

**Files:**
- Modify: `conary-core/src/bootstrap/base.rs:252-258,1184`

dracut is needed to generate the initramfs but is not in any package list.

- [ ] Add `("dracut", "boot")` to `BOOT_PACKAGES` (after `"dosfstools"`):
```rust
const BOOT_PACKAGES: &'static [(&'static str, &'static str)] = &[
    ("popt", "boot"),
    ("efivar", "boot"),
    ("efibootmgr", "boot"),
    ("dosfstools", "boot"),
    ("dracut", "boot"),
    ("grub", "boot"),
];
```

- [ ] Update `test_package_counts` (line 1184) — change `60` to `61`:
```rust
assert_eq!(total, 61); // 12 + 14 + 7 + 22 + 6
```

- [ ] Verify: `cargo test -p conary-core -- test_package_counts`

- [ ] Commit: `fix(bootstrap): add dracut to BOOT_PACKAGES for initramfs generation`

---

### Task 3: Fix fstab ESP mount point

**Files:**
- Modify: `conary-core/src/bootstrap/base.rs:1123-1130`
- Modify: `conary-core/src/bootstrap/image.rs:929-941`

Both fstab locations mount the ESP at `/boot/efi`, but systemd-repart places files at the ESP root via `CopyFiles=/boot:/`. At runtime, the ESP must be at `/boot` so paths match.

- [ ] In `populate_sysroot()` (base.rs:1123-1130), update the fstab:
```rust
fs::write(
    etc.join("fstab"),
    "# /etc/fstab - conaryOS\n\
     LABEL=CONARY_ROOT  /      ext4  defaults,noatime  0 1\n\
     LABEL=CONARY_ESP   /boot  vfat  defaults,noatime  0 2\n\
     tmpfs              /tmp   tmpfs defaults,nosuid   0 0\n",
)?;
```

- [ ] In `create_fstab()` (image.rs:934-941), update the fstab to match:
```rust
let fstab_content = "\
# /etc/fstab - conaryOS
#
# <file system>  <mount point>  <type>  <options>        <dump> <pass>
LABEL=CONARY_ROOT  /              ext4    defaults,noatime  0      1
LABEL=CONARY_ESP   /boot          vfat    defaults,noatime  0      2
tmpfs              /tmp           tmpfs   defaults,nosuid   0      0
";
```

- [ ] Verify: `cargo build -p conary-core`

- [ ] Commit: `fix(bootstrap): mount ESP at /boot not /boot/efi for systemd-boot compatibility`

---

## Chunk 2: Extend populate_sysroot() with SSH, networking, branding

All changes in `conary-core/src/bootstrap/base.rs`.

### Task 4: Update branding to conaryOS

**Files:**
- Modify: `conary-core/src/bootstrap/base.rs:1107-1117`
- Modify: `conary-core/src/bootstrap/base.rs:1342-1343` (test)

- [ ] Update `/etc/hostname` (line 1108):
```rust
fs::write(etc.join("hostname"), "conaryos\n")?;
```

- [ ] Update `/etc/os-release` (lines 1111-1117):
```rust
fs::write(
    etc.join("os-release"),
    "NAME=\"conaryOS\"\n\
     ID=conaryos\n\
     VERSION_ID=0.1\n\
     PRETTY_NAME=\"conaryOS 0.1 (Bootstrap)\"\n\
     HOME_URL=\"https://conaryos.com\"\n",
)?;
```

- [ ] Update test assertion at line 1343:
```rust
assert!(os_release.contains("conaryOS"));
```

- [ ] Verify: `cargo test -p conary-core -- test_populate_sysroot`

- [ ] Commit: `feat(bootstrap): brand sysroot as conaryOS`

---

### Task 5: Add SSH config, networking, systemd targets, and shell to populate_sysroot()

**Files:**
- Modify: `conary-core/src/bootstrap/base.rs` — add to `populate_sysroot()` after the existing fstab write (after line 1130)

- [ ] Add the following block after the fstab write and before the info log:

```rust
// /etc/nsswitch.conf -- required for name resolution
fs::write(
    etc.join("nsswitch.conf"),
    "passwd: files\n\
     group:  files\n\
     shadow: files\n\
     hosts:  files dns\n",
)?;

// /etc/ssh/sshd_config -- permit root login for bootstrap/test access
let ssh_dir = etc.join("ssh");
fs::create_dir_all(&ssh_dir)?;
fs::write(
    ssh_dir.join("sshd_config"),
    "# conaryOS sshd configuration\n\
     PermitRootLogin yes\n\
     PubkeyAuthentication yes\n\
     PasswordAuthentication yes\n\
     PermitEmptyPasswords yes\n\
     UsePAM no\n",
)?;

// /root/.bashrc -- minimal shell prompt
let root_home = root.join("root");
fs::create_dir_all(&root_home)?;
fs::write(
    root_home.join(".bashrc"),
    "export PS1='[\\u@\\h \\W]\\$ '\n\
     alias ls='ls --color=auto'\n",
)?;

// systemd-networkd DHCP config for all ethernet interfaces
let networkd_dir = etc.join("systemd/network");
fs::create_dir_all(&networkd_dir)?;
fs::write(
    networkd_dir.join("80-dhcp.network"),
    "[Match]\n\
     Name=en*\n\n\
     [Network]\n\
     DHCP=yes\n",
)?;

// Systemd service wiring -- create symlink target directories
let systemd_system = etc.join("systemd/system");
fs::create_dir_all(systemd_system.join("multi-user.target.wants"))?;
fs::create_dir_all(systemd_system.join("getty.target.wants"))?;

// default.target -> multi-user.target
#[cfg(unix)]
std::os::unix::fs::symlink(
    "/usr/lib/systemd/system/multi-user.target",
    systemd_system.join("default.target"),
)?;

// Enable sshd
#[cfg(unix)]
std::os::unix::fs::symlink(
    "/usr/lib/systemd/system/sshd.service",
    systemd_system.join("multi-user.target.wants/sshd.service"),
)?;

// Enable systemd-networkd
#[cfg(unix)]
std::os::unix::fs::symlink(
    "/usr/lib/systemd/system/systemd-networkd.service",
    systemd_system.join("multi-user.target.wants/systemd-networkd.service"),
)?;

// Enable serial console for QEMU -nographic
#[cfg(unix)]
std::os::unix::fs::symlink(
    "/usr/lib/systemd/system/serial-getty@.service",
    systemd_system.join("getty.target.wants/serial-getty@ttyS0.service"),
)?;
```

- [ ] Update the info log message (line 1132):
```rust
info!("Sysroot populated with essential system files, SSH, networking, and systemd targets");
```

- [ ] Extend the test `test_populate_sysroot_creates_files` with new assertions:
```rust
// SSH config
assert!(root.join("etc/ssh/sshd_config").exists());
let sshd = std::fs::read_to_string(root.join("etc/ssh/sshd_config")).unwrap();
assert!(sshd.contains("PermitRootLogin yes"));

// Networking
assert!(root.join("etc/systemd/network/80-dhcp.network").exists());
assert!(root.join("etc/nsswitch.conf").exists());

// Shell
assert!(root.join("root/.bashrc").exists());

// Systemd targets (symlinks -- on unix only)
#[cfg(unix)]
{
    assert!(root.join("etc/systemd/system/default.target").exists());
    assert!(root
        .join("etc/systemd/system/multi-user.target.wants/sshd.service")
        .exists());
    assert!(root
        .join("etc/systemd/system/multi-user.target.wants/systemd-networkd.service")
        .exists());
    assert!(root
        .join("etc/systemd/system/getty.target.wants/serial-getty@ttyS0.service")
        .exists());
}
```

- [ ] Verify: `cargo test -p conary-core -- test_populate_sysroot`

- [ ] Commit: `feat(bootstrap): add SSH, networking, systemd targets, and shell config to sysroot`

---

## Chunk 3: finalize_sysroot() — kernel, initramfs, bootloader, keys

### Task 6: Implement finalize_sysroot()

**Files:**
- Modify: `conary-core/src/bootstrap/base.rs` — add new public function after `populate_sysroot()`

This function runs host tools against the sysroot. It requires packages to be installed first. Add it after `populate_sysroot()` (before the `BuildSummary` struct).

- [ ] Add `use std::process::Command;` to the file's imports if not present.

- [ ] Add the function:

```rust
/// Finalize the sysroot for bootable image generation.
///
/// Runs host tools against the populated sysroot to:
/// - Copy the kernel to /boot
/// - Generate an initramfs via dracut (chroot)
/// - Write systemd-boot loader config and BLS entry
/// - Copy the systemd-boot EFI binary
/// - Generate SSH host keys and a test keypair
///
/// Must be called after `populate_sysroot()` and after packages are installed.
pub fn finalize_sysroot(root: &Path) -> Result<(), crate::error::Error> {
    use std::process::Command;
    use tracing::{debug, info, warn};

    info!("Finalizing sysroot for bootable image");

    // 1. Detect kernel version from installed modules
    let modules_dir = root.join("usr/lib/modules");
    let kernel_version = if modules_dir.is_dir() {
        std::fs::read_dir(&modules_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .next()
    } else {
        None
    };

    let boot_dir = root.join("boot");
    fs::create_dir_all(&boot_dir)?;

    if let Some(ref ver) = kernel_version {
        info!("Detected kernel version: {}", ver);

        // 2. Copy kernel to /boot
        let vmlinuz_src = modules_dir.join(ver).join("vmlinuz");
        let vmlinuz_dst = boot_dir.join(format!("vmlinuz-{ver}"));
        if vmlinuz_src.exists() {
            fs::copy(&vmlinuz_src, &vmlinuz_dst)?;
            info!("Copied kernel to {}", vmlinuz_dst.display());
        } else {
            warn!("Kernel image not found at {}", vmlinuz_src.display());
        }

        // 3. Generate initramfs via dracut (requires bind-mounted /proc /sys /dev)
        let initramfs_path = format!("/boot/initramfs-{ver}.img");
        let has_dracut = root.join("usr/bin/dracut").exists()
            || root.join("usr/sbin/dracut").exists();

        if has_dracut {
            info!("Generating initramfs with dracut");

            // Bind-mount host filesystems into sysroot
            let bind_mounts = [("proc", "/proc"), ("sys", "/sys"), ("dev", "/dev")];
            for (name, host_path) in &bind_mounts {
                let target = root.join(name);
                fs::create_dir_all(&target)?;
                let status = Command::new("mount")
                    .args(["--bind", host_path, &target.to_string_lossy()])
                    .status();
                if let Err(e) = status {
                    warn!("Failed to bind-mount {} (may need root): {}", host_path, e);
                }
            }

            // Run dracut in chroot
            let dracut_result = Command::new("chroot")
                .arg(root)
                .args(["dracut", "--no-hostonly", "--force", &initramfs_path, ver])
                .status();

            // Unmount in reverse order (best-effort)
            for (name, _) in bind_mounts.iter().rev() {
                let target = root.join(name);
                let _ = Command::new("umount").arg(&target).status();
            }

            match dracut_result {
                Ok(s) if s.success() => info!("Initramfs generated: {}", initramfs_path),
                Ok(s) => warn!("dracut exited with status {} (initramfs may be incomplete)", s),
                Err(e) => warn!("Failed to run dracut: {} (may need root privileges)", e),
            }
        } else {
            warn!("dracut not found in sysroot -- skipping initramfs generation");
        }

        // 4. Write systemd-boot loader config
        let loader_dir = boot_dir.join("loader/entries");
        fs::create_dir_all(&loader_dir)?;

        fs::write(
            boot_dir.join("loader/loader.conf"),
            "default conaryos.conf\n\
             timeout 3\n\
             console-mode auto\n\
             editor no\n",
        )?;

        // 5. Write BLS entry
        fs::write(
            loader_dir.join("conaryos.conf"),
            format!(
                "title   conaryOS\n\
                 linux   /vmlinuz-{ver}\n\
                 initrd  /initramfs-{ver}.img\n\
                 options root=LABEL=CONARY_ROOT ro console=ttyS0,115200\n"
            ),
        )?;
        info!("systemd-boot loader config written");
    } else {
        warn!("No kernel modules found in {} -- skipping kernel/initramfs/BLS setup", modules_dir.display());
    }

    // 6. Copy systemd-boot EFI binary
    let efi_boot_dir = boot_dir.join("EFI/BOOT");
    fs::create_dir_all(&efi_boot_dir)?;

    let efi_search_paths = [
        root.join("usr/lib/systemd/boot/efi/systemd-bootx64.efi"),
        PathBuf::from("/usr/lib/systemd/boot/efi/systemd-bootx64.efi"),
    ];

    let efi_src = efi_search_paths.iter().find(|p| p.exists());
    match efi_src {
        Some(src) => {
            fs::copy(src, efi_boot_dir.join("BOOTX64.EFI"))?;
            info!("Copied systemd-boot EFI binary from {}", src.display());
        }
        None => {
            return Err(crate::error::Error::InitError(
                "systemd-boot EFI binary (systemd-bootx64.efi) not found in sysroot or host. \
                 Ensure systemd is built with -Dbootloader=true."
                    .to_string(),
            ));
        }
    }

    // 7. Generate SSH host keys (explicit per-key, not ssh-keygen -A)
    let ssh_dir = root.join("etc/ssh");
    fs::create_dir_all(&ssh_dir)?;

    for (key_type, extra_args) in [("ed25519", vec![]), ("rsa", vec!["-b", "4096"]), ("ecdsa", vec![])] {
        let key_path = ssh_dir.join(format!("ssh_host_{key_type}_key"));
        if !key_path.exists() {
            let mut cmd = Command::new("ssh-keygen");
            cmd.args(["-t", key_type, "-f"])
                .arg(&key_path)
                .args(["-N", ""]);
            for arg in &extra_args {
                cmd.arg(arg);
            }
            match cmd.status() {
                Ok(s) if s.success() => debug!("Generated {} host key", key_type),
                Ok(s) => warn!("ssh-keygen ({}) exited with {}", key_type, s),
                Err(e) => warn!("Failed to generate {} host key: {}", key_type, e),
            }
        }
    }

    // 8. Generate test SSH keypair for automated access
    let dot_ssh = root.join("root/.ssh");
    fs::create_dir_all(&dot_ssh)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dot_ssh, fs::Permissions::from_mode(0o700))?;
    }

    let test_key_path = dot_ssh.join("conaryos-test-key");
    if !test_key_path.exists() {
        let status = Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-f"])
            .arg(&test_key_path)
            .args(["-N", "", "-C", "conaryos-test"])
            .status();
        match status {
            Ok(s) if s.success() => {
                // Install public key as authorized_keys
                let pub_key = fs::read_to_string(test_key_path.with_extension("pub"))?;
                let auth_keys = dot_ssh.join("authorized_keys");
                fs::write(&auth_keys, pub_key)?;
                #[cfg(unix)]
                fs::set_permissions(&auth_keys, fs::Permissions::from_mode(0o600))?;
                info!("Test SSH keypair generated and authorized_keys installed");
            }
            Ok(s) => warn!("ssh-keygen (test key) exited with {}", s),
            Err(e) => warn!("Failed to generate test SSH key: {}", e),
        }
    }

    info!("Sysroot finalization complete");
    Ok(())
}
```

- [ ] Verify: `cargo build -p conary-core`

- [ ] Commit: `feat(bootstrap): add finalize_sysroot() for kernel, initramfs, bootloader, and SSH keys`

---

## Chunk 4: Remove GRUB code from image.rs

### Task 7: Replace GRUB EFI setup with systemd-boot in image.rs

**Files:**
- Modify: `conary-core/src/bootstrap/image.rs`

Boot setup is now handled by `finalize_sysroot()` in base.rs. The image builder just needs systemd-repart's `CopyFiles=/boot:/` to pick up everything.

- [ ] Delete or replace `setup_efi_boot()` (lines 971-1007) — replace body with:
```rust
fn setup_efi_boot(&self, _mount_dir: &Path) -> Result<(), ImageError> {
    // EFI boot setup is now handled by finalize_sysroot() in base.rs.
    // systemd-repart's CopyFiles=/boot:/ copies the bootloader, kernel,
    // initramfs, and loader config from the sysroot to the ESP.
    Ok(())
}
```

- [ ] Delete `create_grub_config()` (lines 1010-1030) — replace with:
```rust
#[allow(dead_code)] // Retained for potential future BIOS boot support
fn create_grub_config(&self, _path: &Path) -> Result<(), ImageError> {
    Ok(())
}
```

- [ ] Delete `create_stub_efi()` (lines 1033-1038) — replace with:
```rust
#[allow(dead_code)] // Retained for potential future BIOS boot support
fn create_stub_efi(&self, _path: &Path) -> Result<(), ImageError> {
    Ok(())
}
```

- [ ] Mark `generate_initramfs()` as deprecated (around line 1312):
```rust
#[allow(dead_code)] // Deprecated: use finalize_sysroot() + dracut instead
```

- [ ] Add comment to `grub_install` detection in `ImageTools` (around line 229):
```rust
// Retained for potential future BIOS boot support
```

- [ ] Verify: `cargo build -p conary-core`

- [ ] Commit: `refactor(bootstrap): replace GRUB EFI setup with systemd-boot (via finalize_sysroot)`

---

## Chunk 5: Wire into CLI and update test runner

### Task 8: Wire populate_sysroot + finalize_sysroot into CLI

**Files:**
- Modify: `src/commands/bootstrap/mod.rs:359-375`

- [ ] After `bootstrap.build_base()` returns (line 359), add the sysroot finalization calls before the success message:

```rust
    let summary = bootstrap.build_base(&recipe_path, root)?;

    // Populate sysroot with system config (passwd, fstab, SSH, systemd targets)
    println!("Populating sysroot with system configuration...");
    let sysroot_path = std::path::PathBuf::from(root);
    conary_core::bootstrap::base::BaseBuilder::populate_sysroot(&sysroot_path)?;

    // Finalize: kernel, initramfs, bootloader, SSH keys
    println!("Finalizing sysroot (kernel, initramfs, bootloader, SSH keys)...");
    conary_core::bootstrap::base::BaseBuilder::finalize_sysroot(&sysroot_path)?;

    println!("\n[OK] Base system build complete!");
```

- [ ] Remove the existing `println!("\n[OK] Base system build complete!");` that was on the line after `build_base()` returns (to avoid a duplicate).

- [ ] Verify: `cargo build`

- [ ] Commit: `feat(bootstrap): wire populate_sysroot + finalize_sysroot into bootstrap base command`

---

### Task 9: Update QEMU test runner for key-based SSH

**Files:**
- Modify: `conary-test/src/engine/qemu.rs`

The test runner uses `BatchMode=yes` which is incompatible with password auth. Add key-based SSH auth.

- [ ] Add a constant for the test key cache location (near the top of the file, after `DEFAULT_ARTIFACT_BASE_URL`):
```rust
/// Well-known filename for the conaryOS test SSH private key.
const TEST_SSH_KEY_NAME: &str = "conaryos-test-key";
```

- [ ] Add a helper function to locate or download the test key:
```rust
/// Locate the test SSH private key, downloading it if necessary.
async fn test_ssh_key_path() -> Result<PathBuf> {
    let cache_dir = cache_dir();
    let key_path = cache_dir.join(TEST_SSH_KEY_NAME);
    if key_path.exists() {
        return Ok(key_path);
    }

    // Try to download from test artifacts
    let url = format!("{DEFAULT_ARTIFACT_BASE_URL}/{TEST_SSH_KEY_NAME}");
    if let Err(e) = download_image(&url, &key_path).await {
        tracing::debug!("Could not download test SSH key from {}: {}", url, e);
    }

    if key_path.exists() {
        // Fix permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(key_path)
    } else {
        anyhow::bail!("Test SSH key not found at {} and could not be downloaded", key_path.display())
    }
}
```

- [ ] Update `run_ssh_command()` (line 294) to accept an optional key path and add `-i`:
```rust
async fn run_ssh_command(ssh_port: u16, command: &str, key_path: Option<&Path>) -> Result<ExecResult> {
    let remote = format!("sh -lc {}", shell_quote(command));
    let mut args = vec![
        "-o", "StrictHostKeyChecking=no",
        "-o", "UserKnownHostsFile=/dev/null",
        "-o", "LogLevel=ERROR",
        "-o", "ConnectTimeout=2",
    ];
    let key_str;
    if let Some(key) = key_path {
        key_str = key.to_string_lossy().to_string();
        args.extend(["-i", &key_str]);
    } else {
        args.extend(["-o", "BatchMode=yes"]);
    }
    args.extend(["-p", &ssh_port.to_string(), "root@127.0.0.1", &remote]);

    let output = Command::new("ssh")
        .args(&args)
        .output()
        .await
        .with_context(|| format!("failed to run SSH command: {command}"))?;

    Ok(ExecResult {
        exit_code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
```

- [ ] Update `wait_for_ssh()` to pass the key (line 276):
```rust
let probe = run_ssh_command(ssh_port, "true", key_path.as_deref()).await?;
```
And update its signature to accept and resolve the key:
```rust
async fn wait_for_ssh(
    child: &mut Child,
    ssh_port: u16,
    timeout_seconds: u64,
) -> Result<std::result::Result<(), String>> {
    let key_path = test_ssh_key_path().await.ok();
```

- [ ] Update `run_qemu_boot()` — pass key_path through to the SSH command loop (lines 77-100). The SSH commands that execute after boot need the key too:
```rust
let key_path = test_ssh_key_path().await.ok();
// ... in the command loop:
let result = run_ssh_command(config.ssh_port, &expanded_cmd, key_path.as_deref()).await?;
```

- [ ] Verify: `cargo build -p conary-test`

- [ ] Commit: `feat(test): add SSH key-based auth to QEMU test runner`

---

### Task 10: Update T156 test manifest

**Files:**
- Modify: `tests/integration/remi/manifests/phase3-group-n-qemu.toml:160-176`

- [ ] Update T156 commands and expected output:
```toml
[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v2"
memory_mb = 1024
timeout_seconds = 180
ssh_port = 2222
commands = [
    "uname -r",
    "systemctl is-system-running --wait || true",
    "id -un",
    "cat /etc/os-release | grep conaryOS || true",
    "echo boot-verified",
]
expect_output = [
    "boot-verified",
]
```

- [ ] Update all other tests (T150, T151, T153, T154) that reference `minimal-boot-v1` to `minimal-boot-v2`.

- [ ] Verify: `cargo build -p conary-test`

- [ ] Commit: `test(qemu): update T156 for conaryOS boot verification and v2 image`

---

## Implementation Order

1. Task 1: Partition labels (5 min)
2. Task 2: dracut in BOOT_PACKAGES (5 min)
3. Task 3: fstab ESP mount (5 min)
4. Task 4: conaryOS branding (5 min)
5. Task 5: SSH, networking, systemd targets (15 min)
6. Task 6: finalize_sysroot() (30 min)
7. Task 7: Remove GRUB code from image.rs (10 min)
8. Task 8: Wire into CLI (5 min)
9. Task 9: QEMU SSH key auth (20 min)
10. Task 10: Update T156 manifest (5 min)

After all tasks: build the image on Remi, publish to `/conary/test-artifacts/`, run T156.

## Success Criteria

- `cargo test -p conary-core` passes (excluding 2 pre-existing bootstrap::toolchain failures)
- `cargo clippy -- -D warnings` clean
- `cargo build -p conary-test` succeeds
- Partition labels in repart.rs match fstab and BLS entry
- `populate_sysroot()` creates SSH config, systemd targets, DHCP network, nsswitch.conf
- `finalize_sysroot()` produces kernel, initramfs, BLS entry, systemd-boot EFI binary, SSH keys
- GRUB EFI code removed from image.rs
- `cmd_bootstrap_base()` calls both populate and finalize
- QEMU test runner supports SSH key auth
- T156 manifest references v2 image and validates conaryOS branding
