# Bootstrap Pipeline Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a tiered bootstrap pipeline that produces a bootable, self-hosting Conary Linux system from scratch on Remi.

**Architecture:** Bash orchestrator (`scripts/bootstrap-remi.sh`) drives existing `conary bootstrap` CLI commands in three tiers (A: minimal boot, B: full base, C: self-hosting). Rust code changes add per-package build mode, initramfs generation, and Conary stage implementation.

**Tech Stack:** Rust (conary-core), Bash (orchestrator), QEMU (validation), busybox (initramfs)

**Spec:** `docs/superpowers/specs/2026-03-09-bootstrap-pipeline-design.md`

## Research Findings (2026-03-09)

Validated against current LFS development (r13.0-3), mkosi, and upstream projects:

### Package Versions (aligned with LFS r13.0-3)
Our `recipes/core/versions.toml` is current: GCC 15.2.0, glibc 2.43, binutils 2.46,
Linux 6.18.x (LFS uses 6.18.10, we have 6.19.5 -- fine), coreutils 9.10, bash 5.3,
util-linux 2.41.3, systemd 259.x. No version bumps needed.

### GCC 15 + glibc 2.43 Patch Required
GCC 15 requires a one-line fix for glibc 2.43 compatibility:
`sed -i 's/char [*]q/const &/' libgomp/affinity-fmt.c`
This changes `char *q` to `const char *q` in `gomp_display_affinity()` due to glibc's
C23 const-preserving `strchr()`. Must be applied in Stage 1 gcc-pass2 recipe.
**Action:** Verify `recipes/core/stage1/gcc-pass2.toml` includes this patch.

### Busybox: Use 1.37.0, Build From Host
The pre-built static binaries on busybox.net are outdated (1.35.0 from 2022).
Current stable is **1.37.0** (per Repology). Two options:
1. Build busybox 1.37.0 statically on the host (Remi has gcc + musl available)
2. Use the distro's `busybox-static` package (Ubuntu 24.04 on Remi likely has 1.36.x)

**Decision:** Use Remi's `busybox-static` package if available, else download and
build 1.37.0 with `make LDFLAGS="--static"`. Simpler than trusting 4-year-old binaries.

### Image Generation: systemd-repart Preferred
Remi (Ubuntu 24.04) should have systemd-repart available. The existing `image.rs` code
already prefers systemd-repart over legacy loop-device method. This is the modern
approach (used by mkosi) and runs rootless -- no `losetup` or `mount` needed.
**Action:** Verify systemd-repart is installed on Remi, install if not.

### QEMU Testing: Serial Log Polling
The serial log polling approach in the orchestrator script is the standard pattern for
automated VM testing (used by Yocto, OpenEmbedded). No need for pexpect or expect --
`-serial file:log` + `grep` + `timeout` is simpler and works in bash.

---

## Chunk 1: CLI and Base Builder Per-Package Mode

### Task 1: Add `--package` and `--tier` flags to `conary bootstrap base`

**Files:**
- Modify: `src/cli/bootstrap.rs:109-129` (add args to Base variant)
- Modify: `src/commands/bootstrap/mod.rs:298-361` (update handler)
- Modify: `src/main.rs` or wherever CLI dispatch happens (pass new args)

- [ ] **Step 1: Add CLI args to `BootstrapCommands::Base`**

In `src/cli/bootstrap.rs`, add to the `Base` variant:

```rust
/// Build a single package by name
#[arg(short, long)]
package: Option<String>,

/// Build only packages for a specific tier (a, b, c)
#[arg(long)]
tier: Option<String>,
```

- [ ] **Step 2: Update `cmd_bootstrap_base` signature and handler**

In `src/commands/bootstrap/mod.rs`, update `cmd_bootstrap_base` to accept `package: Option<&str>` and `tier: Option<&str>`. When `--package` is set, build only that one package. When `--tier` is set, build only packages in that tier's list. When neither is set, build all (existing behavior).

- [ ] **Step 3: Update CLI dispatch to pass new args**

Find where `cmd_bootstrap_base` is called (search for the match arm on `BootstrapCommands::Base`) and pass the new `package` and `tier` fields.

- [ ] **Step 4: Run `cargo build` to verify compilation**

Run: `cargo build`
Expected: compiles clean

- [ ] **Step 5: Run `cargo clippy -- -D warnings`**

Run: `cargo clippy -- -D warnings`
Expected: no warnings

- [ ] **Step 6: Commit**

```bash
git add src/cli/bootstrap.rs src/commands/bootstrap/mod.rs
git commit -m "feat(bootstrap): add --package and --tier flags to base command"
```

### Task 2: Add tier package lists to `BaseBuilder`

**Files:**
- Modify: `conary-core/src/bootstrap/base.rs:177-258` (add tier constants and filter method)

- [ ] **Step 1: Add tier package constants**

After the existing `BOOT_PACKAGES` constant in `BaseBuilder`, add:

```rust
/// Tier A: minimal boot to login prompt (16 packages)
const TIER_A_PACKAGES: &'static [&'static str] = &[
    "zlib", "xz", "zstd", "openssl", "ncurses", "readline",
    "libcap", "kmod", "elfutils", "dbus", "linux-pam",
    "util-linux", "coreutils", "bash", "systemd", "linux",
];

/// Tier B: full base system (adds ~45 packages on top of Tier A)
/// Includes everything NOT in Tier A
const TIER_B_PACKAGES: &'static [&'static str] = &[
    "libmnl", "make", "m4", "autoconf", "automake", "libtool",
    "pkgconf", "bison", "flex", "gettext", "perl", "python",
    "cmake", "ninja", "meson", "iproute2", "openssh",
    "grep", "sed", "gawk", "less", "diffutils", "patch",
    "findutils", "file", "tar", "gzip", "bzip2", "cpio",
    "ca-certificates", "curl", "wget2", "git",
    "procps-ng", "psmisc", "shadow", "sudo",
    "vim", "nano",
    "popt", "efivar", "efibootmgr", "dosfstools", "grub",
];
```

- [ ] **Step 2: Add `packages_for_tier` method**

```rust
/// Get package names for a specific tier
pub fn packages_for_tier(tier: &str) -> Option<&'static [&'static str]> {
    match tier {
        "a" => Some(Self::TIER_A_PACKAGES),
        "b" => Some(Self::TIER_B_PACKAGES),
        _ => None,
    }
}
```

- [ ] **Step 3: Add `build_single_package` public method**

Add a method that builds just one named package, loading its recipe and running the build steps. This wraps the existing `build_package` private method with recipe loading and status reporting:

```rust
/// Build a single named package
pub fn build_single(&mut self, name: &str) -> Result<(), BaseError> {
    // Find package index
    let idx = self.packages.iter().position(|p| p.name == name)
        .ok_or_else(|| BaseError::RecipeNotFound(name.to_string()))?;

    // Load recipe if needed
    if self.packages[idx].recipe.is_none() {
        let recipe = self.load_recipe(&self.packages[idx])?;
        self.packages[idx].recipe = Some(recipe);
    }

    self.build_package(idx)?;
    self.packages[idx].status = BaseBuildStatus::Complete;
    self.save_log(idx)?;
    Ok(())
}
```

- [ ] **Step 4: Update `cmd_bootstrap_base` to use tier/package filtering**

In the handler, after initializing the `BaseBuilder`:
- If `--package foo` is set: call `builder.build_single("foo")`
- If `--tier a` is set: iterate `TIER_A_PACKAGES`, call `build_single` for each
- If neither: call `builder.build()` (existing behavior)

- [ ] **Step 5: Run `cargo build && cargo clippy -- -D warnings`**

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/bootstrap/base.rs src/commands/bootstrap/mod.rs
git commit -m "feat(bootstrap): add tier package lists and per-package build mode"
```

### Task 2b: Verify GCC 15 + glibc 2.43 patch in recipes

**Files:**
- Check: `recipes/core/stage1/gcc-pass2.toml`
- Check: `recipes/core/stage1/gcc-pass1.toml`

- [ ] **Step 1: Check if gcc-pass2 recipe includes the libgomp fix**

Read `recipes/core/stage1/gcc-pass2.toml` and look for `sed -i 's/char [*]q/const &/' libgomp/affinity-fmt.c` or equivalent. GCC 15.2.0 with glibc 2.43 requires this patch (changes `char *q` to `const char *q` in `gomp_display_affinity()`).

If missing, add it to the `configure` or `setup` step of the recipe:
```toml
setup = """
sed -i 's/char [*]q/const &/' libgomp/affinity-fmt.c
"""
```

- [ ] **Step 2: Verify versions.toml matches LFS r13.0-3**

Confirm `recipes/core/versions.toml` has: GCC 15.2.0, glibc 2.43, binutils 2.46, kernel headers 6.18+. These match current LFS development.

- [ ] **Step 3: Commit if any changes were needed**

```bash
git add recipes/core/stage1/gcc-pass2.toml recipes/core/versions.toml
git commit -m "fix(bootstrap): add GCC 15 libgomp patch for glibc 2.43 compatibility"
```

---

## Chunk 2: Initramfs Generation

### Task 3: Add initramfs builder to `image.rs`

**Files:**
- Modify: `conary-core/src/bootstrap/image.rs` (add initramfs generation)

- [ ] **Step 1: Add initramfs generation method to `ImageBuilder`**

Add a `generate_initramfs` method that:
1. Creates a temp directory for the initramfs root
2. Downloads a static busybox binary (or uses a cached one from `sources/`)
3. Creates the `/init` script
4. Creates directory structure (`/proc`, `/sys`, `/dev`, `/mnt/root`, `/bin`)
5. Creates `/dev/console` device node (using `mknod` command)
6. Packs it all into a cpio archive with gzip compression
7. Installs to `{sysroot}/boot/initramfs.img`

```rust
/// Busybox static binary -- prefer host system's busybox-static package,
/// fall back to building from source. Pre-built binaries from busybox.net
/// are outdated (1.35.0 from 2022); current stable is 1.37.0.
const BUSYBOX_SOURCE_URL: &str = "https://busybox.net/downloads/busybox-1.37.0.tar.bz2";
const BUSYBOX_SOURCE_SHA256: &str = "3311dff32e746499f4df0d5df04d7eb396382c7a3eef7b66e99a08e759c0a710";

/// Generate a minimal initramfs for booting
pub fn generate_initramfs(&self, sources_dir: &Path) -> Result<PathBuf, ImageError> {
    let initramfs_root = self.work_dir.join("initramfs");
    let output = self.sysroot.join("boot/initramfs.img");

    // Create structure
    for dir in ["bin", "dev", "proc", "sys", "mnt/root"] {
        fs::create_dir_all(initramfs_root.join(dir))?;
    }

    // Find or build static busybox
    // Priority: 1) host busybox-static package  2) build from source
    let busybox_cache = sources_dir.join("busybox-static");
    if !busybox_cache.exists() {
        // Try host system's busybox-static first
        let host_busybox = Command::new("which").arg("busybox").output();
        if let Ok(ref out) = host_busybox && out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // Check if it's statically linked
            let ldd = Command::new("ldd").arg(&path).output();
            if let Ok(ref ldd_out) = ldd && !ldd_out.status.success() {
                // ldd fails on static binaries -- this is what we want
                fs::copy(&path, &busybox_cache)?;
            }
        }
        if !busybox_cache.exists() {
            // Build from source with static linking
            // Download, extract, make defconfig, make LDFLAGS="--static"
            return Err(ImageError::CommandFailed(
                "busybox-static not found on host; install busybox-static package \
                 or place a static busybox binary at sources/busybox-static".into()
            ));
        }
    }

    // Copy busybox and make executable
    fs::copy(&busybox_cache, initramfs_root.join("bin/busybox"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            initramfs_root.join("bin/busybox"),
            fs::Permissions::from_mode(0o755),
        )?;
    }

    // Create symlinks for busybox applets we need
    for applet in ["sh", "mount", "switch_root"] {
        std::os::unix::fs::symlink("busybox", initramfs_root.join("bin").join(applet))?;
    }

    // Create /init script
    let init_script = "#!/bin/sh\n\
        mount -t proc proc /proc\n\
        mount -t sysfs sys /sys\n\
        mount -t devtmpfs dev /dev\n\
        mount /dev/vda2 /mnt/root\n\
        exec switch_root /mnt/root /lib/systemd/systemd\n";
    fs::write(initramfs_root.join("init"), init_script)?;
    #[cfg(unix)]
    fs::set_permissions(
        initramfs_root.join("init"),
        fs::Permissions::from_mode(0o755),
    )?;

    // Create /dev/console
    Command::new("mknod")
        .args([initramfs_root.join("dev/console").to_str().unwrap(), "c", "5", "1"])
        .status()?;

    // Pack as cpio + gzip
    // cd initramfs_root && find . | cpio -o -H newc | gzip > output
    let cpio_cmd = format!(
        "cd {} && find . | cpio -o -H newc 2>/dev/null | gzip > {}",
        initramfs_root.display(),
        output.display()
    );
    let status = Command::new("sh").args(["-c", &cpio_cmd]).status()?;
    if !status.success() {
        return Err(ImageError::CommandFailed("cpio/gzip failed".into()));
    }

    // Cleanup temp dir
    let _ = fs::remove_dir_all(&initramfs_root);

    info!("Initramfs generated: {}", output.display());
    Ok(output)
}
```

- [ ] **Step 2: Add unit test for initramfs directory structure**

```rust
#[test]
fn test_initramfs_init_script_content() {
    let script = "#!/bin/sh\n\
        mount -t proc proc /proc\n\
        mount -t sysfs sys /sys\n\
        mount -t devtmpfs dev /dev\n\
        mount /dev/vda2 /mnt/root\n\
        exec switch_root /mnt/root /lib/systemd/systemd\n";
    assert!(script.starts_with("#!/bin/sh"));
    assert!(script.contains("switch_root"));
    assert!(script.contains("/lib/systemd/systemd"));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-core -- initramfs`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/bootstrap/image.rs
git commit -m "feat(bootstrap): add initramfs generation with static busybox"
```

---

## Chunk 3: Sysroot Population

### Task 4: Add sysroot population for essential system files

**Files:**
- Modify: `conary-core/src/bootstrap/base.rs` (add `populate_sysroot` method)

- [ ] **Step 1: Add `populate_sysroot` method to `BaseBuilder`**

This creates the essential `/etc` files needed for the system to boot and accept login:

```rust
/// Populate the sysroot with essential system configuration files.
///
/// Creates /etc/passwd, /etc/group, /etc/shadow, /etc/hostname,
/// /etc/os-release, /etc/machine-id, and /etc/fstab.
/// These are needed before the first boot -- without them,
/// login fails and systemd reports degraded state.
pub fn populate_sysroot(root: &Path) -> Result<(), BaseError> {
    let etc = root.join("etc");
    fs::create_dir_all(&etc)?;

    // /etc/passwd -- root with no password
    fs::write(etc.join("passwd"),
        "root:x:0:0:root:/root:/bin/bash\nnobody:x:65534:65534:Nobody:/:/sbin/nologin\n")?;

    // /etc/group
    fs::write(etc.join("group"),
        "root:x:0:\nwheel:x:10:\ntty:x:5:\nnogroup:x:65534:\n")?;

    // /etc/shadow -- root with empty password (permits passwordless login)
    fs::write(etc.join("shadow"),
        "root::0:0:99999:7:::\nnobody:!:0:0:99999:7:::\n")?;

    // Restrict shadow permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(etc.join("shadow"), fs::Permissions::from_mode(0o600))?;
    }

    // /etc/hostname
    fs::write(etc.join("hostname"), "conary\n")?;

    // /etc/os-release
    fs::write(etc.join("os-release"),
        "NAME=\"Conary Linux\"\n\
         ID=conary\n\
         VERSION_ID=0.1\n\
         PRETTY_NAME=\"Conary Linux 0.1 (Bootstrap)\"\n\
         HOME_URL=\"https://conary.io\"\n")?;

    // /etc/machine-id -- empty, systemd generates on first boot
    fs::write(etc.join("machine-id"), "")?;

    // /etc/fstab
    fs::write(etc.join("fstab"),
        "# /etc/fstab - Conary system\n\
         LABEL=CONARY_ROOT  /          ext4  defaults,noatime  0 1\n\
         LABEL=CONARY_ESP   /boot/efi  vfat  defaults,noatime  0 2\n\
         tmpfs              /tmp       tmpfs defaults,nosuid   0 0\n")?;

    info!("Sysroot populated with essential system files");
    Ok(())
}
```

- [ ] **Step 2: Add test**

```rust
#[test]
fn test_populate_sysroot_creates_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    fs::create_dir_all(&root).unwrap();
    BaseBuilder::populate_sysroot(&root).unwrap();

    assert!(root.join("etc/passwd").exists());
    assert!(root.join("etc/group").exists());
    assert!(root.join("etc/shadow").exists());
    assert!(root.join("etc/hostname").exists());
    assert!(root.join("etc/os-release").exists());
    assert!(root.join("etc/machine-id").exists());
    assert!(root.join("etc/fstab").exists());

    let passwd = fs::read_to_string(root.join("etc/passwd")).unwrap();
    assert!(passwd.contains("root:x:0:0"));

    let os_release = fs::read_to_string(root.join("etc/os-release")).unwrap();
    assert!(os_release.contains("Conary Linux"));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-core -- populate_sysroot`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/bootstrap/base.rs
git commit -m "feat(bootstrap): add sysroot population for essential etc files"
```

---

## Chunk 4: Conary Stage Implementation

### Task 5: Implement `build_rust()` in `conary_stage.rs`

**Files:**
- Modify: `conary-core/src/bootstrap/conary_stage.rs:112-117`

- [ ] **Step 1: Update `RUST_VERSION` constant**

Change line 41 from `"1.93.0"` to `"1.94.0"` to match the project's Rust version.

- [ ] **Step 2: Implement `build_rust()`**

Replace the `NotImplemented` stub with:
1. Download the Rust bootstrap binary from `rust_bootstrap_url()`
2. Extract to a temp directory
3. Run the Rust installer script (`install.sh`) targeting the sysroot
4. Verify `rustc` and `cargo` exist in the sysroot

```rust
pub fn build_rust(&self) -> Result<PathBuf, ConaryStageError> {
    info!("Building Rust {} for sysroot", RUST_VERSION);

    let rust_dir = self._work_dir.join("rust");
    std::fs::create_dir_all(&rust_dir)
        .map_err(|e| ConaryStageError::Io(e))?;

    // Download bootstrap binary
    let url = self.rust_bootstrap_url();
    let archive = rust_dir.join(format!("rust-{RUST_VERSION}.tar.xz"));

    if !archive.exists() {
        info!("Downloading Rust bootstrap from {}", url);
        let status = std::process::Command::new("curl")
            .args(["-fSL", "-o"])
            .arg(&archive)
            .arg(&url)
            .status()
            .map_err(|e| ConaryStageError::RustBootstrapFailed(e.to_string()))?;
        if !status.success() {
            return Err(ConaryStageError::RustBootstrapFailed(
                "curl download failed".into()
            ));
        }
    }

    // Extract
    let extract_dir = rust_dir.join("extract");
    if extract_dir.exists() {
        std::fs::remove_dir_all(&extract_dir)?;
    }
    std::fs::create_dir_all(&extract_dir)?;

    let status = std::process::Command::new("tar")
        .args(["xf"])
        .arg(&archive)
        .arg("-C")
        .arg(&extract_dir)
        .arg("--strip-components=1")
        .status()
        .map_err(|e| ConaryStageError::RustBootstrapFailed(e.to_string()))?;
    if !status.success() {
        return Err(ConaryStageError::RustBootstrapFailed("tar extract failed".into()));
    }

    // Run installer
    let status = std::process::Command::new(extract_dir.join("install.sh"))
        .arg(format!("--prefix={}/usr", self.sysroot.display()))
        .status()
        .map_err(|e| ConaryStageError::RustBuildFailed(e.to_string()))?;
    if !status.success() {
        return Err(ConaryStageError::RustBuildFailed("install.sh failed".into()));
    }

    // Verify
    let rustc = self.sysroot.join("usr/bin/rustc");
    let cargo = self.sysroot.join("usr/bin/cargo");
    if !rustc.exists() {
        return Err(ConaryStageError::RustBuildFailed(
            format!("rustc not found at {}", rustc.display())
        ));
    }
    if !cargo.exists() {
        return Err(ConaryStageError::RustBuildFailed(
            format!("cargo not found at {}", cargo.display())
        ));
    }

    info!("[COMPLETE] Rust {} installed to sysroot", RUST_VERSION);
    Ok(rustc)
}
```

- [ ] **Step 3: Run `cargo build && cargo clippy -- -D warnings`**

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/bootstrap/conary_stage.rs
git commit -m "feat(bootstrap): implement build_rust() -- install Rust bootstrap to sysroot"
```

### Task 6: Implement `build_conary()` in `conary_stage.rs`

**Files:**
- Modify: `conary-core/src/bootstrap/conary_stage.rs:123-128`

- [ ] **Step 1: Implement `build_conary()`**

Replace the `NotImplemented` stub. This builds Conary from source inside the sysroot using the installed Rust toolchain:

```rust
pub fn build_conary(&self) -> Result<PathBuf, ConaryStageError> {
    info!("Building Conary in sysroot");

    let cargo = self.sysroot.join("usr/bin/cargo");
    if !cargo.exists() {
        return Err(ConaryStageError::ConaryBuildFailed(
            "cargo not found -- run build_rust() first".into()
        ));
    }

    // Clone or copy conary source into sysroot
    let src_dir = self._work_dir.join("conary-src");
    if !src_dir.exists() {
        // Copy from the current source tree
        let status = std::process::Command::new("cp")
            .args(["-a", ".", src_dir.to_str().unwrap()])
            .status()
            .map_err(|e| ConaryStageError::ConaryBuildFailed(e.to_string()))?;
        if !status.success() {
            return Err(ConaryStageError::ConaryBuildFailed("source copy failed".into()));
        }
    }

    // Build conary using the sysroot's cargo
    let status = std::process::Command::new(&cargo)
        .args(["build", "--release"])
        .current_dir(&src_dir)
        .status()
        .map_err(|e| ConaryStageError::ConaryBuildFailed(e.to_string()))?;
    if !status.success() {
        return Err(ConaryStageError::ConaryBuildFailed("cargo build failed".into()));
    }

    // Install binary to sysroot
    let binary = src_dir.join("target/release/conary");
    let dest = self.sysroot.join("usr/bin/conary");
    std::fs::copy(&binary, &dest)
        .map_err(|e| ConaryStageError::ConaryBuildFailed(
            format!("install failed: {e}")
        ))?;

    info!("[COMPLETE] Conary installed to {}", dest.display());
    Ok(dest)
}
```

- [ ] **Step 2: Update existing test**

The `test_conary_stage_build_returns_not_implemented` test now needs updating since `build_rust()` no longer returns `NotImplemented`. Update it to test a different failure mode (e.g., missing curl for download). Or remove the test and rely on the sysroot validation test.

- [ ] **Step 3: Run `cargo test -p conary-core && cargo clippy -- -D warnings`**

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/bootstrap/conary_stage.rs
git commit -m "feat(bootstrap): implement build_conary() -- compile Conary in sysroot"
```

---

## Chunk 5: Orchestrator Script

### Task 7: Create `scripts/bootstrap-remi.sh`

**Files:**
- Create: `scripts/bootstrap-remi.sh`

- [ ] **Step 1: Write the orchestrator script**

The script should:
1. Parse args (`--tier all|a|b|c`, `--resume`, `--clean`)
2. Set `BOOTSTRAP_DIR=/conary/bootstrap`
3. Check prerequisites: `which qemu-system-x86_64 sfdisk mkfs.ext4 mkfs.fat curl cpio`
4. Build conary if needed: `cargo build`
5. Define `CONARY_BIN=./target/debug/conary`
6. Define tier package arrays (matching Rust constants)
7. For Stage 0+1: run `$CONARY_BIN bootstrap stage0` and `stage1` with appropriate flags
8. For each tier: iterate packages, run `$CONARY_BIN bootstrap base --package $pkg`
9. After Tier A packages: run `populate_sysroot` (via conary command or inline), generate initramfs, create image, run QEMU smoke test
10. After Tier B: install GRUB, regenerate image, run QEMU+SSH test
11. After Tier C: run `$CONARY_BIN bootstrap conary`, regenerate image, run self-rebuild test
12. Structured logging to `$BOOTSTRAP_DIR/logs/`
13. Resume logic via `jq` reads of `bootstrap-state.json`

Key functions:
- `log_info()`, `log_error()`, `log_step()` -- structured logging
- `check_prerequisites()` -- verify tools
- `run_stage0()`, `run_stage1()` -- toolchain builds
- `build_tier_a()`, `build_tier_b()`, `build_tier_c()` -- package builds
- `generate_image()` -- create disk image
- `qemu_test_tier_a()` -- boot to login test
- `qemu_test_tier_b()` -- SSH test
- `qemu_test_tier_c()` -- self-rebuild test

The script must use `set -euo pipefail` and be `shellcheck`-clean.

- [ ] **Step 2: Make executable**

```bash
chmod +x scripts/bootstrap-remi.sh
```

- [ ] **Step 3: Verify with shellcheck (if available)**

Run: `shellcheck scripts/bootstrap-remi.sh` (or skip if not installed)

- [ ] **Step 4: Commit**

```bash
git add scripts/bootstrap-remi.sh
git commit -m "feat(bootstrap): add orchestrator script for Remi bootstrap pipeline"
```

---

## Chunk 6: QEMU Smoke Tests

### Task 8: Add QEMU test functions to the orchestrator

**Files:**
- Modify: `scripts/bootstrap-remi.sh` (add QEMU test functions)

- [ ] **Step 1: Add `qemu_test_tier_a()` function**

Boot with `-kernel` direct boot, capture serial output, grep for "login:", then use `expect`-style interaction (via heredoc to QEMU monitor) or `timeout` + serial log parsing:

```bash
qemu_test_tier_a() {
    local image="$BOOTSTRAP_DIR/images/tier-a.img"
    local kernel="$BOOTSTRAP_DIR/sysroot/boot/vmlinuz"
    local initrd="$BOOTSTRAP_DIR/sysroot/boot/initramfs.img"
    local serial_log="$BOOTSTRAP_DIR/logs/qemu-tier-a.log"

    log_step "QEMU Tier A: boot to login prompt"

    timeout 120 qemu-system-x86_64 \
        -kernel "$kernel" \
        -initrd "$initrd" \
        -append "root=/dev/vda2 console=ttyS0 init=/lib/systemd/systemd" \
        -drive "file=$image,format=raw" \
        -m 1024 \
        -nographic \
        -no-reboot \
        -serial file:"$serial_log" \
        -monitor none &
    local qemu_pid=$!

    # Wait for login prompt or timeout
    local elapsed=0
    while [ $elapsed -lt 90 ]; do
        if grep -q "login:" "$serial_log" 2>/dev/null; then
            kill $qemu_pid 2>/dev/null || true
            log_info "[PASS] Tier A: login prompt detected"
            return 0
        fi
        sleep 2
        elapsed=$((elapsed + 2))
    done

    kill $qemu_pid 2>/dev/null || true
    log_error "[FAIL] Tier A: no login prompt after 90s"
    log_error "Serial log: $serial_log"
    return 1
}
```

- [ ] **Step 2: Add `qemu_test_tier_b()` function**

Boot from image with GRUB, enable SSH port forwarding, wait for SSH to respond:

```bash
qemu_test_tier_b() {
    local image="$BOOTSTRAP_DIR/images/tier-b.img"
    local serial_log="$BOOTSTRAP_DIR/logs/qemu-tier-b.log"

    log_step "QEMU Tier B: boot with GRUB + SSH"

    qemu-system-x86_64 \
        -drive "file=$image,format=raw" \
        -m 2048 \
        -nographic \
        -no-reboot \
        -serial file:"$serial_log" \
        -net nic -net user,hostfwd=tcp::2222-:22 \
        -monitor none &
    local qemu_pid=$!

    # Wait for SSH
    local elapsed=0
    while [ $elapsed -lt 120 ]; do
        if ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=no \
               -p 2222 root@localhost "uname -r" 2>/dev/null; then
            # Run additional checks
            ssh -p 2222 root@localhost "ls /usr/bin/grep && python3 --version && git --version" || true
            kill $qemu_pid 2>/dev/null || true
            log_info "[PASS] Tier B: SSH working, commands verified"
            return 0
        fi
        sleep 3
        elapsed=$((elapsed + 3))
    done

    kill $qemu_pid 2>/dev/null || true
    log_error "[FAIL] Tier B: SSH not responding after 120s"
    return 1
}
```

- [ ] **Step 3: Add `qemu_test_tier_c()` function**

```bash
qemu_test_tier_c() {
    local image="$BOOTSTRAP_DIR/images/tier-c.img"
    local serial_log="$BOOTSTRAP_DIR/logs/qemu-tier-c.log"

    log_step "QEMU Tier C: self-hosting verification"

    qemu-system-x86_64 \
        -drive "file=$image,format=raw" \
        -m 4096 \
        -smp 4 \
        -nographic \
        -no-reboot \
        -serial file:"$serial_log" \
        -net nic -net user,hostfwd=tcp::2222-:22 \
        -monitor none &
    local qemu_pid=$!

    # Wait for SSH
    local elapsed=0
    while [ $elapsed -lt 120 ]; do
        if ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=no \
               -p 2222 root@localhost "rustc --version" 2>/dev/null; then
            break
        fi
        sleep 3
        elapsed=$((elapsed + 3))
    done

    if [ $elapsed -ge 120 ]; then
        kill $qemu_pid 2>/dev/null || true
        log_error "[FAIL] Tier C: SSH not responding"
        return 1
    fi

    # Verify tools
    ssh -p 2222 root@localhost "rustc --version && cargo --version && conary --version" || {
        kill $qemu_pid 2>/dev/null || true
        log_error "[FAIL] Tier C: tools not found"
        return 1
    }

    log_info "[PASS] Tier C: self-hosting tools verified"
    kill $qemu_pid 2>/dev/null || true
    return 0
}
```

- [ ] **Step 4: Commit**

```bash
git add scripts/bootstrap-remi.sh
git commit -m "feat(bootstrap): add QEMU smoke tests for all tiers"
```

---

## Chunk 7: Integration and Dry Run

### Task 9: Test the full pipeline locally (dry-run)

**Files:**
- No code changes -- validation only

- [ ] **Step 1: Verify `conary bootstrap dry-run` still works**

Run: `cargo run -- bootstrap dry-run --recipe-dir recipes`
Expected: resolves dependency graph, prints build order, no errors

- [ ] **Step 2: Verify `--tier a` flag works (compilation)**

Run: `cargo run -- bootstrap base --tier a --work-dir /tmp/test-bootstrap --root /tmp/test-sysroot`
Expected: fails cleanly with "Stage 1 toolchain not found" (we haven't built it), but proves the CLI arg parsing works

- [ ] **Step 3: Verify `--package zlib` flag works (compilation)**

Run: `cargo run -- bootstrap base --package zlib --work-dir /tmp/test-bootstrap --root /tmp/test-sysroot`
Expected: fails cleanly with "Stage 1 toolchain not found"

- [ ] **Step 4: Verify the orchestrator script syntax**

Run: `bash -n scripts/bootstrap-remi.sh`
Expected: no syntax errors

- [ ] **Step 5: Commit any fixes from integration testing**

### Task 10: Update ROADMAP.md

**Files:**
- Modify: `ROADMAP.md:67-69`

- [ ] **Step 1: Update bootstrap items**

Mark base system builds with checkpointing as in progress:

```markdown
- [ ] Base system builds with checkpointing (pipeline ready, needs Remi run)
- [ ] Image generation produces bootable output (pipeline ready, needs Remi run)
- [ ] (Stretch) Boot the image in QEMU and verify
```

- [ ] **Step 2: Commit**

```bash
git add ROADMAP.md
git commit -m "docs: update ROADMAP with bootstrap pipeline status"
```

---

## Execution Order

Tasks must be executed in this order due to dependencies:

1. **Task 1** (CLI flags) -- needed by orchestrator
2. **Task 2** (tier lists + per-package build) -- needed by orchestrator
3. **Task 2b** (GCC/glibc patch verification) -- needed before any build
4. **Task 3** (initramfs) -- needed for QEMU tests
5. **Task 4** (sysroot population) -- needed for boot
6. **Task 5** (build_rust) -- Tier C
7. **Task 6** (build_conary) -- Tier C
8. **Task 7** (orchestrator script) -- ties everything together
9. **Task 8** (QEMU tests) -- validation
10. **Task 9** (dry-run verification) -- final check
11. **Task 10** (docs update) -- housekeeping

**Parallelizable groups:**
- Tasks 1 + 2 + 2b (independent, no cross-deps)
- Tasks 3 + 4 (independent)
- Tasks 5 + 6 (sequential but isolated from 3-4)
- Tasks 7 + 8 (script + tests)
- Tasks 9 + 10 (final sequential)
