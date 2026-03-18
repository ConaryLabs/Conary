# LFS 13 Bootstrap Alignment — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Realign the Conary bootstrap pipeline with LFS 13 (systemd version), producing a fully from-source bootable conaryOS image.

**Architecture:** Six-phase pipeline (cross-tools, temp-tools, final-system, config, image, tier2) replaces the current stage0/stage1/base structure. LFS 13 is the authoritative reference for package versions, configure flags, and build order. Every deviation is documented.

**Tech Stack:** Rust (conary-core bootstrap modules), TOML recipes, LFS 13 systemd reference, QEMU for boot verification.

**Spec:** `docs/superpowers/specs/2026-03-17-lfs-bootstrap-alignment-design.md`

**LFS Reference:** `https://www.linuxfromscratch.org/lfs/view/systemd/`

---

## Recipe TOML Template

All recipes follow this format. Workers should fetch the LFS page for each package to get exact configure flags, then translate into this template:

```toml
# recipes/<phase>/<package>.toml
#
# LFS 13 Section X.Y — Package-Version
# https://www.linuxfromscratch.org/lfs/view/systemd/chapterNN/package.html

[package]
name = "package"
version = "1.0.0"
release = "1"
summary = "One-line description"
description = """
Multi-line description from LFS.
"""
license = "LICENSE-ID"
homepage = "https://..."

[source]
archive = "https://download-url/package-%(version)s.tar.xz"
checksum = "sha256:HASH_FROM_LFS_PACKAGES_PAGE"

[build]
requires = ["dep1", "dep2"]
makedepends = ["build-dep1"]

configure = """
./configure \
    --prefix=/usr \
    ...flags from LFS...
"""

make = "make -j%(jobs)s"

install = """
make DESTDIR=%(destdir)s install
"""

# Only if flags differ from LFS:
# [deviations]
# lfs_section = "8.X"
# lfs_url = "https://..."
# changes = [
#     { flag = "--flag", lfs = "value", conary = "value", reason = "why" },
# ]

[variables]
jobs = "$(nproc)"
```

**For cross-tools/ and temp-tools/ recipes**, the configure flags are simpler (minimal builds). The key additions are `--host=$LFS_TGT` and `--build=$(build-aux/config.guess)` for cross-compilation.

**Source URLs and checksums** come from the LFS packages page: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter03/packages.html`

---

## Chunk 1: Rust Module Restructuring

Delete old modules, create new skeleton modules, update infrastructure. The goal is a compiling codebase with the new module structure before any recipes are written.

### Task 1: Delete obsolete modules

**Files:**
- Delete: `conary-core/src/bootstrap/stage0.rs`
- Delete: `conary-core/src/bootstrap/stage2.rs`

- [ ] **Step 1: Delete stage0.rs and stage2.rs**

These modules implement crosstool-ng (stage0) and the optional purity rebuild (stage2). Both are replaced by the LFS chapter structure.

```bash
rm conary-core/src/bootstrap/stage0.rs conary-core/src/bootstrap/stage2.rs
```

- [ ] **Step 2: Remove their exports from mod.rs**

In `conary-core/src/bootstrap/mod.rs`, remove all `pub use` lines referencing `stage0` and `stage2` modules, and remove the `mod stage0;` and `mod stage2;` declarations.

- [ ] **Step 3: Remove ct-ng from Prerequisites**

In `conary-core/src/bootstrap/mod.rs`, find the `Prerequisites` struct and:
- Remove the `crosstool_ng` field
- Remove the `ct-ng` check from `Prerequisites::check()`
- Remove `crosstool_ng` from `all_present()` and `missing()`

- [ ] **Step 4: Fix compilation errors in CLI**

In `src/commands/bootstrap/mod.rs`:
- Remove `cmd_bootstrap_stage0` function entirely
- Remove `cmd_bootstrap_stage2` if it exists
- Remove `Stage0Builder` and `Stage2Builder` from imports (both are deleted)
- Remove crosstool-ng references from `cmd_bootstrap_check`
- Update `cmd_bootstrap_init` to remove stage0 references from "next steps" output

In `src/cli/` bootstrap subcommand definitions, remove the `stage0` and `stage2` subcommands.

- [ ] **Step 5: Verify compilation**

Run: `cargo build 2>&1 | head -50`
Fix any remaining compilation errors from removed references.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(bootstrap): delete stage0 (crosstool-ng) and stage2 (purity rebuild)"
```

### Task 2: Create new module skeletons

**Files:**
- Create: `conary-core/src/bootstrap/cross_tools.rs`
- Create: `conary-core/src/bootstrap/temp_tools.rs`
- Create: `conary-core/src/bootstrap/final_system.rs`
- Create: `conary-core/src/bootstrap/system_config.rs`
- Create: `conary-core/src/bootstrap/tier2.rs`

- [ ] **Step 1: Create cross_tools.rs skeleton**

Phase 1 builder — replaces stage1.rs for building the LFS Ch5 cross-toolchain. Reuse the `PackageBuildRunner` from `build_runner.rs` for fetching/extracting sources.

```rust
// conary-core/src/bootstrap/cross_tools.rs

//! Phase 1: Cross-Toolchain (LFS Ch5)
//!
//! Builds the cross-compiler using the host system's GCC.
//! Produces a cross-compiler at $LFS/tools/ targeting x86_64-conary-linux-gnu.
//!
//! Packages (in order):
//! 1. Binutils Pass 1 (5.2)
//! 2. GCC Pass 1 (5.3)
//! 3. Linux API Headers (5.4)
//! 4. Glibc (5.5)
//! 5. Libstdc++ from GCC (5.6)

use super::build_runner::PackageBuildRunner;
use super::config::BootstrapConfig;
use super::toolchain::Toolchain;

use crate::recipe::{Recipe, parse_recipe_file};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum CrossToolsError {
    #[error("Host GCC not found")]
    HostGccNotFound,

    #[error("Recipe not found: {0}")]
    RecipeNotFound(String),

    #[error("Build failed for {0}: {1}")]
    BuildFailed(String, String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Cross-toolchain build order (LFS Ch5)
const CROSS_TOOLS_ORDER: &[&str] = &[
    "binutils-pass1",
    "gcc-pass1",
    "linux-headers",
    "glibc",
    "libstdcxx",
];

/// Builder for the LFS Ch5 cross-toolchain
pub struct CrossToolsBuilder {
    work_dir: PathBuf,
    config: BootstrapConfig,
    lfs_dir: PathBuf,        // $LFS
    tools_dir: PathBuf,      // $LFS/tools
    recipe_dir: PathBuf,     // recipes/cross-tools/
    build_env: HashMap<String, String>,
}

impl CrossToolsBuilder {
    /// Target triple for cross-compilation
    pub const LFS_TGT: &str = "x86_64-conary-linux-gnu";

    pub fn new(
        work_dir: impl AsRef<Path>,
        config: &BootstrapConfig,
        lfs_dir: impl AsRef<Path>,
        recipe_dir: impl AsRef<Path>,
    ) -> Result<Self, CrossToolsError> {
        let work_dir = work_dir.as_ref().to_path_buf();
        let lfs_dir = lfs_dir.as_ref().to_path_buf();
        let tools_dir = lfs_dir.join("tools");
        let recipe_dir = recipe_dir.as_ref().to_path_buf();

        // Verify host GCC exists
        if Command::new("gcc").arg("--version").output().is_err() {
            return Err(CrossToolsError::HostGccNotFound);
        }

        std::fs::create_dir_all(&tools_dir)?;
        std::fs::create_dir_all(work_dir.join("sources"))?;

        let build_env = Self::setup_env(&tools_dir);

        Ok(Self {
            work_dir,
            config: config.clone(),
            lfs_dir,
            tools_dir,
            recipe_dir,
            build_env,
        })
    }

    fn setup_env(tools_dir: &Path) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("LFS_TGT".to_string(), Self::LFS_TGT.to_string());
        // PATH: tools/bin first, then host
        let path = format!(
            "{}:/usr/bin:/usr/sbin:/bin:/sbin",
            tools_dir.join("bin").display()
        );
        env.insert("PATH".to_string(), path);
        env
    }

    /// Build all cross-tools in order
    pub fn build_all(&mut self) -> Result<Toolchain, CrossToolsError> {
        for name in CROSS_TOOLS_ORDER {
            info!("Building cross-tool: {name}");
            self.build_package(name)?;
        }

        Ok(Toolchain {
            kind: ToolchainKind::Stage1, // Closest existing kind; update ToolchainKind if needed
            path: self.tools_dir.clone(),
            target: Self::LFS_TGT.to_string(),
            gcc_version: None, // TODO: detect from built gcc
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        })
    }

    fn build_package(&self, name: &str) -> Result<(), CrossToolsError> {
        let recipe_path = self.recipe_dir.join(format!("{name}.toml"));
        if !recipe_path.exists() {
            return Err(CrossToolsError::RecipeNotFound(name.to_string()));
        }
        // TODO: Parse recipe, fetch source, configure, build, install
        // Uses PackageBuildRunner for source fetching/verification
        // Build output goes to $LFS/tools/ (binutils, gcc) or $LFS/ (glibc, headers)
        info!("  Built {name} [placeholder]");
        Ok(())
    }

    /// Verify the cross-compiler works
    pub fn verify(&self) -> Result<bool, CrossToolsError> {
        // Write hello.c, compile with cross-gcc, check with `file`
        let hello_c = self.work_dir.join("hello.c");
        std::fs::write(&hello_c, "#include <stdio.h>\nint main() { puts(\"Hello\"); return 0; }\n")?;

        let gcc = self.tools_dir.join("bin").join(format!("{}-gcc", Self::LFS_TGT));
        let output = Command::new(&gcc)
            .arg(&hello_c)
            .arg("-o")
            .arg(self.work_dir.join("hello"))
            .output();

        match output {
            Ok(o) if o.status.success() => {
                info!("Cross-compiler verification: PASS");
                Ok(true)
            }
            Ok(o) => {
                warn!("Cross-compiler verification: FAIL - {}", String::from_utf8_lossy(&o.stderr));
                Ok(false)
            }
            Err(e) => {
                warn!("Cross-compiler not found at {}: {e}", gcc.display());
                Ok(false)
            }
        }
    }
}
```

- [ ] **Step 2: Create temp_tools.rs skeleton**

Phase 2 builder — cross-compiles temporary tools (LFS Ch6) and builds chroot tools (LFS Ch7).

```rust
// conary-core/src/bootstrap/temp_tools.rs

//! Phase 2: Temporary Tools (LFS Ch6-7)
//!
//! Cross-compiles 17 packages with the Phase 1 toolchain (Ch6),
//! then enters chroot to build 6 additional packages (Ch7).

use super::build_runner::PackageBuildRunner;
use super::config::BootstrapConfig;
use super::cross_tools::CrossToolsBuilder;

use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum TempToolsError {
    #[error("Cross-toolchain not found at {0}")]
    ToolchainNotFound(PathBuf),

    #[error("Build failed for {0}: {1}")]
    BuildFailed(String, String),

    #[error("Chroot setup failed: {0}")]
    ChrootFailed(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Ch6 packages: cross-compiled with --host=$LFS_TGT
const CH6_PACKAGES: &[&str] = &[
    "m4", "ncurses", "bash", "coreutils", "diffutils", "file",
    "findutils", "gawk", "grep", "gzip", "make", "patch", "sed",
    "tar", "xz", "binutils-pass2", "gcc-pass2",
];

/// Ch7 packages: built inside chroot
const CH7_PACKAGES: &[&str] = &[
    "gettext", "bison", "perl", "python", "texinfo", "util-linux",
];

pub struct TempToolsBuilder {
    work_dir: PathBuf,
    config: BootstrapConfig,
    lfs_dir: PathBuf,
    recipe_dir: PathBuf,
}

impl TempToolsBuilder {
    pub fn new(
        work_dir: impl AsRef<Path>,
        config: &BootstrapConfig,
        lfs_dir: impl AsRef<Path>,
        recipe_dir: impl AsRef<Path>,
    ) -> Result<Self, TempToolsError> {
        let lfs_dir = lfs_dir.as_ref().to_path_buf();
        let tools_bin = lfs_dir.join("tools/bin");
        if !tools_bin.exists() {
            return Err(TempToolsError::ToolchainNotFound(tools_bin));
        }

        Ok(Self {
            work_dir: work_dir.as_ref().to_path_buf(),
            config: config.clone(),
            lfs_dir,
            recipe_dir: recipe_dir.as_ref().to_path_buf(),
        })
    }

    /// Build Ch6 packages (cross-compiled)
    pub fn build_cross_packages(&mut self) -> Result<(), TempToolsError> {
        for name in CH6_PACKAGES {
            info!("Cross-compiling temporary tool: {name}");
            // TODO: parse recipe, build with --host=$LFS_TGT
        }
        Ok(())
    }

    /// Set up chroot environment (LFS Ch7.2-7.6)
    pub fn setup_chroot(&self) -> Result<(), TempToolsError> {
        // Create essential directories (7.5)
        // Create essential files and symlinks (7.6)
        // Mount virtual kernel filesystems (7.3)
        info!("Chroot environment prepared");
        Ok(())
    }

    /// Build Ch7 packages (in chroot)
    pub fn build_chroot_packages(&mut self) -> Result<(), TempToolsError> {
        for name in CH7_PACKAGES {
            info!("Building chroot tool: {name}");
            // TODO: build inside chroot
        }
        Ok(())
    }

    /// Verify chroot works
    pub fn verify(&self) -> Result<bool, TempToolsError> {
        let status = Command::new("chroot")
            .arg(&self.lfs_dir)
            .args(["/bin/bash", "-c", "gcc --version && make --version"])
            .status();

        match status {
            Ok(s) if s.success() => Ok(true),
            _ => Ok(false),
        }
    }
}
```

- [ ] **Step 3: Create final_system.rs skeleton**

Phase 3 builder — builds 77 packages in LFS Ch8 order inside chroot.

```rust
// conary-core/src/bootstrap/final_system.rs

//! Phase 3: Final System (LFS Ch8)
//!
//! Builds 77 packages inside the chroot in LFS order.
//! The toolchain (glibc, binutils, GCC) is rebuilt as final-system packages.
//! Build order is hardcoded from LFS Ch8 — no dependency graph needed.

use super::build_runner::PackageBuildRunner;
use super::config::BootstrapConfig;

use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum FinalSystemError {
    #[error("Chroot not ready at {0}")]
    ChrootNotReady(PathBuf),

    #[error("Recipe not found: {0}")]
    RecipeNotFound(String),

    #[error("Build failed for {0}: {1}")]
    BuildFailed(String, String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// LFS Ch8 build order (77 packages, minus GRUB/Tcl/Expect/DejaGNU)
const SYSTEM_BUILD_ORDER: &[&str] = &[
    "man-pages",       // 8.3
    "iana-etc",        // 8.4
    "glibc",           // 8.5
    "zlib",            // 8.6
    "bzip2",           // 8.7
    "xz",              // 8.8
    "lz4",             // 8.9
    "zstd",            // 8.10
    "file",            // 8.11
    "readline",        // 8.12
    "pcre2",           // 8.13
    "m4",              // 8.14
    "bc",              // 8.15
    "flex",            // 8.16
    "pkgconf",         // 8.17
    "binutils",        // 8.18
    "gmp",             // 8.19
    "mpfr",            // 8.20
    "mpc",             // 8.21
    "attr",            // 8.22
    "acl",             // 8.23
    "libcap",          // 8.24
    "libxcrypt",       // 8.25
    "shadow",          // 8.26
    "gcc",             // 8.27
    "ncurses",         // 8.28
    "sed",             // 8.29
    "psmisc",          // 8.30
    "gettext",         // 8.31
    "bison",           // 8.32
    "grep",            // 8.33
    "bash",            // 8.34
    "libtool",         // 8.35
    "gdbm",            // 8.36
    "gperf",           // 8.37
    "expat",           // 8.38
    "inetutils",       // 8.39
    "less",            // 8.40
    "perl",            // 8.41
    "xml-parser",      // 8.42
    "intltool",        // 8.43
    "autoconf",        // 8.44
    "automake",        // 8.45
    "openssl",         // 8.46
    "elfutils",        // 8.47
    "libffi",          // 8.48
    "sqlite",          // 8.49
    "python",          // 8.50
    "flit-core",       // 8.51
    "packaging",       // 8.52
    "wheel",           // 8.53
    "setuptools",      // 8.54
    "ninja",           // 8.55
    "meson",           // 8.56
    "kmod",            // 8.57
    "coreutils",       // 8.58
    "diffutils",       // 8.59
    "gawk",            // 8.60
    "findutils",       // 8.61
    "groff",           // 8.62
    "gzip",            // 8.63
    "iproute2",        // 8.64
    "kbd",             // 8.65
    "libpipeline",     // 8.66
    "make",            // 8.67
    "patch",           // 8.68
    "tar",             // 8.69
    "texinfo",         // 8.70
    "vim",             // 8.71
    "markupsafe",      // 8.72
    "jinja2",          // 8.73
    "systemd",         // 8.74
    "dbus",            // 8.75
    "man-db",          // 8.76
    "procps-ng",       // 8.77
    "util-linux",      // 8.78
    "e2fsprogs",       // 8.79
];

pub struct FinalSystemBuilder {
    work_dir: PathBuf,
    config: BootstrapConfig,
    sysroot: PathBuf,
    recipe_dir: PathBuf,
    completed: Vec<String>,
}

impl FinalSystemBuilder {
    pub fn new(
        work_dir: impl AsRef<Path>,
        config: &BootstrapConfig,
        sysroot: impl AsRef<Path>,
        recipe_dir: impl AsRef<Path>,
    ) -> Result<Self, FinalSystemError> {
        Ok(Self {
            work_dir: work_dir.as_ref().to_path_buf(),
            config: config.clone(),
            sysroot: sysroot.as_ref().to_path_buf(),
            recipe_dir: recipe_dir.as_ref().to_path_buf(),
            completed: Vec::new(),
        })
    }

    /// Build all packages in LFS order
    pub fn build_all(&mut self) -> Result<(), FinalSystemError> {
        for name in SYSTEM_BUILD_ORDER {
            info!("Building system package [{}/{}]: {name}",
                self.completed.len() + 1,
                SYSTEM_BUILD_ORDER.len()
            );
            self.build_package(name)?;
            self.completed.push(name.to_string());
        }
        info!("Final system complete: {} packages built", self.completed.len());
        Ok(())
    }

    /// Build a single named package (for resume support)
    pub fn build_from(&mut self, start: &str) -> Result<(), FinalSystemError> {
        let start_idx = SYSTEM_BUILD_ORDER.iter()
            .position(|&n| n == start)
            .ok_or_else(|| FinalSystemError::RecipeNotFound(start.to_string()))?;

        for name in &SYSTEM_BUILD_ORDER[start_idx..] {
            info!("Building system package: {name}");
            self.build_package(name)?;
            self.completed.push(name.to_string());
        }
        Ok(())
    }

    fn build_package(&self, name: &str) -> Result<(), FinalSystemError> {
        let recipe_path = self.recipe_dir.join(format!("{name}.toml"));
        if !recipe_path.exists() {
            return Err(FinalSystemError::RecipeNotFound(name.to_string()));
        }
        // TODO: Parse recipe, build inside chroot
        Ok(())
    }

    /// Verify the final system
    pub fn verify(&self) -> Result<bool, FinalSystemError> {
        // Check gcc, python3 sqlite3, systemctl, mke2fs
        Ok(true) // placeholder
    }
}
```

- [ ] **Step 4: Create system_config.rs skeleton**

Phase 4 — extracted from `base.rs` `populate_sysroot()`. Move the function (lines 1088-1220 of base.rs) here, keeping the same logic.

```rust
// conary-core/src/bootstrap/system_config.rs

//! Phase 4: System Configuration (LFS Ch9)
//!
//! Populates the sysroot with /etc files, network config, locale,
//! hostname, and systemd service wiring.

use std::fs;
use std::path::Path;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum SystemConfigError {
    #[error("Sysroot not found at {0}")]
    SysrootNotFound(std::path::PathBuf),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Populate the sysroot with system configuration files.
///
/// Creates /etc/passwd, /etc/shadow, /etc/group, /etc/hostname,
/// /etc/os-release, /etc/fstab, /etc/nsswitch.conf, systemd
/// network config, and service symlinks.
///
/// This is extracted from BaseBuilder::populate_sysroot() in the old base.rs.
pub fn configure_system(root: &Path) -> Result<(), SystemConfigError> {
    if !root.exists() {
        return Err(SystemConfigError::SysrootNotFound(root.to_path_buf()));
    }
    // TODO: Move populate_sysroot() logic here from base.rs
    // (lines 1088-1220 of the old base.rs)
    // Keep the same content but remove SSH config (that's Tier 2)
    info!("System configured");
    Ok(())
}
```

- [ ] **Step 5: Create tier2.rs skeleton**

Phase 6 — adapted from `conary_stage.rs`, extended with BLFS packages.

```rust
// conary-core/src/bootstrap/tier2.rs

//! Phase 6: Tier 2 (BLFS + Conary)
//!
//! Builds BLFS packages (openssh, curl, etc.) and the Conary stage
//! (Rust bootstrap binary + Conary build) into the sysroot.

use super::config::BootstrapConfig;

use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum Tier2Error {
    #[error("Sysroot not found at {0}")]
    SysrootNotFound(PathBuf),

    #[error("Build failed for {0}: {1}")]
    BuildFailed(String, String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Tier 2 build order
const TIER2_ORDER: &[&str] = &[
    "linux-pam",
    "openssh",
    "ca-certificates",
    "curl",
    "sudo",
    "nano",
    "rust",
    "conary",
];

pub struct Tier2Builder {
    work_dir: PathBuf,
    config: BootstrapConfig,
    sysroot: PathBuf,
    recipe_dir: PathBuf,
}

impl Tier2Builder {
    pub fn new(
        work_dir: impl AsRef<Path>,
        config: &BootstrapConfig,
        sysroot: impl AsRef<Path>,
        recipe_dir: impl AsRef<Path>,
    ) -> Result<Self, Tier2Error> {
        Ok(Self {
            work_dir: work_dir.as_ref().to_path_buf(),
            config: config.clone(),
            sysroot: sysroot.as_ref().to_path_buf(),
            recipe_dir: recipe_dir.as_ref().to_path_buf(),
        })
    }

    pub fn build_all(&mut self) -> Result<(), Tier2Error> {
        for name in TIER2_ORDER {
            info!("Building Tier 2 package: {name}");
            // TODO: build in chroot
        }
        Ok(())
    }
}
```

- [ ] **Step 6: Register new modules in mod.rs**

Update `conary-core/src/bootstrap/mod.rs`:
- Add `mod cross_tools;`, `mod temp_tools;`, `mod final_system;`, `mod system_config;`, `mod tier2;`
- Add `pub use` for their public types
- Keep existing `mod stage1;` for now (will be deleted after cross_tools.rs is fully implemented)

- [ ] **Step 7: Verify compilation**

Run: `cargo build 2>&1 | head -50`
Expected: compiles with no errors (new modules are skeleton stubs)

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "feat(bootstrap): create LFS-aligned module skeletons (cross_tools, temp_tools, final_system, system_config, tier2)"
```

### Task 3: Update stages.rs for new phase structure

**Files:**
- Modify: `conary-core/src/bootstrap/stages.rs`

- [ ] **Step 1: Replace the stage enum**

Replace the old `BootstrapStage` enum with the new 6-phase pipeline:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BootstrapStage {
    /// Phase 1: Cross-toolchain (LFS Ch5)
    CrossTools,
    /// Phase 2: Temporary tools (LFS Ch6-7)
    TempTools,
    /// Phase 3: Final system (LFS Ch8)
    FinalSystem,
    /// Phase 4: System configuration (LFS Ch9)
    SystemConfig,
    /// Phase 5: Bootable image (LFS Ch10)
    BootableImage,
    /// Phase 6: Tier 2 — BLFS + Conary
    Tier2,
}
```

- [ ] **Step 2: Update StageManager and Display impl**

Update `StageManager` methods, `Display` impl, and serialization to use the new variants. Remove references to `Stage0`, `Stage1`, `Stage2`, `Base`, `ConaryStage`.

- [ ] **Step 3: Fix compilation errors**

Fix all references to old stage names throughout the codebase.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "refactor(bootstrap): update BootstrapStage enum to 6-phase LFS pipeline"
```

### Task 4: Update config.rs and toolchain.rs

**Files:**
- Modify: `conary-core/src/bootstrap/config.rs`
- Modify: `conary-core/src/bootstrap/toolchain.rs`

- [ ] **Step 1: Remove crosstool-ng config from config.rs**

Remove `crosstool_config` field and `with_crosstool_config()` method. Keep `skip_verify`, `verbose`, `jobs`, `target_arch`.

- [ ] **Step 2: Update toolchain.rs paths**

The `Toolchain` struct and `env()` method need to model LFS paths:
- `$LFS/tools/bin` — Phase 1 cross-tools
- `$LFS/usr/bin` — Phase 3 final system tools

Remove references to crosstool-ng tool paths and stage0 tools directory.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "refactor(bootstrap): update config and toolchain for LFS paths"
```

### Task 5: Update build_runner.rs for cross-compilation context

**Files:**
- Modify: `conary-core/src/bootstrap/build_runner.rs`

- [ ] **Step 1: Add build context enum**

The build runner needs to know whether it's cross-compiling (Phase 1-2) or building natively in chroot (Phase 3+):

```rust
/// Build context determines how configure/make are invoked
#[derive(Debug, Clone)]
pub enum BuildContext {
    /// Cross-compilation: --host=$LFS_TGT --build=$(config.guess)
    Cross {
        host: String,    // e.g., "x86_64-conary-linux-gnu"
        sysroot: PathBuf,
    },
    /// Native build inside chroot
    Chroot {
        root: PathBuf,
    },
}
```

- [ ] **Step 2: Update PackageBuildRunner to accept BuildContext**

Add a `context: Option<BuildContext>` field. When `Cross`, inject `--host` and `--build` environment variables and prepend cross-tools to PATH. When `Chroot`, run build commands via `chroot`.

- [ ] **Step 3: Verify compilation**

Run: `cargo build`

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat(bootstrap): add cross-compilation context to build_runner"
```

### Task 6: Update image.rs — remove host fallbacks

**Files:**
- Modify: `conary-core/src/bootstrap/image.rs`

- [ ] **Step 1: Remove dracut from finalize_sysroot**

The `finalize_sysroot()` function is being moved to `system_config.rs` but parts of it (kernel copy, bootloader config) stay in `image.rs` as Phase 5 logic. For now, in the existing `image.rs`:

- Remove the dracut chroot call (lines ~1297-1314 of base.rs, but the copy in image.rs)
- Remove `generate_initramfs()` method entirely (already marked deprecated)
- Remove GRUB references from `ImageTools` and `check_for_format()`

- [ ] **Step 2: Make copy_efi_binary a hard error**

In `copy_efi_binary()` (if it exists in image.rs or is moved here), remove the host fallback path. Only look in the sysroot:

```rust
fn copy_efi_binary(root: &Path) -> Result<(), ImageError> {
    let efi_name = "systemd-bootx64.efi";
    let sysroot_efi = root.join(format!("usr/lib/systemd/boot/efi/{efi_name}"));

    if !sysroot_efi.exists() {
        return Err(ImageError::BootloaderFailed(format!(
            "systemd-boot EFI binary not found in sysroot at {}. \
             Ensure systemd was built with -Dbootloader=true",
            sysroot_efi.display()
        )));
    }

    let efi_dst = root.join("boot/EFI/BOOT");
    std::fs::create_dir_all(&efi_dst)?;
    std::fs::copy(&sysroot_efi, efi_dst.join("BOOTX64.EFI"))?;
    Ok(())
}
```

- [ ] **Step 3: Update BLS entry — no initrd line**

Wherever the BLS entry is written (finalize_sysroot or image builder), ensure the entry has no `initrd` line:

```rust
let bls_entry = format!(
    "title   conaryOS\n\
     linux   /vmlinuz-{ver}\n\
     options root=LABEL=CONARY_ROOT ro console=ttyS0,115200\n"
);
```

- [ ] **Step 4: Verify compilation**

Run: `cargo build`

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "fix(bootstrap): remove host fallbacks, dracut, and initrd from image pipeline"
```

### Task 6: Update Bootstrap orchestrator in mod.rs

**Files:**
- Modify: `conary-core/src/bootstrap/mod.rs`

- [ ] **Step 1: Update the Bootstrap struct methods**

Replace `build_stage0`, `build_stage1`, `build_stage2`, `build_base`, `build_conary_stage` with:
- `build_cross_tools()` — Phase 1
- `build_temp_tools()` — Phase 2
- `build_final_system()` — Phase 3
- `configure_system()` — Phase 4
- `build_image()` — Phase 5 (update existing)
- `build_tier2()` — Phase 6

Each method creates the appropriate builder, runs it, and marks the stage complete.

- [ ] **Step 2: Update dry_run and resume methods**

Update to use new stage names.

- [ ] **Step 3: Keep stage1.rs and base.rs alive for now**

Do NOT delete `stage1.rs` or `base.rs` yet — they still compile and their `mod` declarations in `mod.rs` should remain until Task 25 (final cleanup in Chunk 7). The new methods in the Bootstrap struct call the new skeleton modules, but the old modules stay as dead code until they are fully replaced.

- [ ] **Step 4: Verify compilation + run tests**

```bash
cargo build && cargo test -p conary-core -- bootstrap
```

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(bootstrap): wire up 6-phase pipeline in Bootstrap orchestrator"
```

### Task 7: Update CLI bootstrap commands

**Files:**
- Modify: `src/commands/bootstrap/mod.rs`
- Modify: `src/cli/` (bootstrap subcommand definitions)

- [ ] **Step 1: Replace stage commands with phase commands**

Remove `cmd_bootstrap_stage0`, `cmd_bootstrap_stage1`, `cmd_bootstrap_stage2`. Add:
- `cmd_bootstrap_cross_tools` — calls `bootstrap.build_cross_tools()`
- `cmd_bootstrap_temp_tools` — calls `bootstrap.build_temp_tools()`
- `cmd_bootstrap_system` — calls `bootstrap.build_final_system()`
- `cmd_bootstrap_config` — calls `bootstrap.configure_system()`
- `cmd_bootstrap_tier2` — calls `bootstrap.build_tier2()`

Keep `cmd_bootstrap_image` and `cmd_bootstrap_base` (the latter now calls phases 1-3 sequentially).

- [ ] **Step 2: Update CLI definitions**

In `src/cli/`, update the bootstrap subcommand enum to match new phase names.

- [ ] **Step 3: Verify compilation**

```bash
cargo build
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "refactor(bootstrap): update CLI commands for 6-phase LFS pipeline"
```

---

## Chunk 2: Recipe Infrastructure + Phase 1 Cross-Tools

### Task 8: Delete old recipe directories, create new structure

**Files:**
- Delete: `recipes/core/`, `recipes/base/`, `recipes/stage1/`, `recipes/conary/`
- Create: `recipes/cross-tools/`, `recipes/temp-tools/`, `recipes/system/`, `recipes/tier2/`

- [ ] **Step 1: Move old recipes to archive (preserve for reference)**

```bash
mkdir -p recipes/archive
mv recipes/core recipes/archive/core
mv recipes/base recipes/archive/base
mv recipes/stage1 recipes/archive/stage1
mv recipes/conary recipes/archive/conary
```

- [ ] **Step 2: Create new directory structure**

```bash
mkdir -p recipes/cross-tools recipes/temp-tools recipes/system recipes/tier2
```

- [ ] **Step 3: Update versions.toml to LFS 13 versions**

Rewrite `recipes/versions.toml` (currently at `recipes/core/versions.toml`) with LFS 13 package versions. The version list comes from:
`https://www.linuxfromscratch.org/lfs/view/systemd/chapter03/packages.html`

Key version updates from current → LFS 13:
- binutils: 2.46 → 2.46.0
- linux: 6.19.5 → 6.19.8
- systemd: 259.2 → 259.5
- openssl: 3.5.4 → 3.6.1
- xz: 5.6.4 → 5.8.2
- m4: 1.4.20 → 1.4.21
- gettext: 0.26 → 1.0
- gawk: 5.3.2 → 5.4.0
- less: 685 → 692
- file: 5.46 → 5.47
- libcap: 2.73 → 2.77
- kmod: 34 → 34.2
- gzip: 1.13 → 1.14
- ninja: 1.13.1 → 1.13.2
- meson: 1.7.0 → 1.10.1
- perl: 5.42.0 → 5.42.1
- shadow: 4.16.0 → 4.19.4
- procps-ng: 4.1.0 → 4.0.6
- vim: 9.2 → 9.2.0161
- pkgconf: 2.3.0 → 2.5.1

Add new packages not in current versions.toml:
- lz4, pcre2, bc, gmp, mpfr, mpc, attr, acl, libxcrypt, gdbm, gperf, expat, inetutils, libffi, sqlite, groff, kbd, libpipeline, texinfo, markupsafe, jinja2, man-db, e2fsprogs, man-pages, iana-etc, flit-core, packaging, wheel, setuptools, xml-parser, intltool

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "chore(recipes): restructure recipe directories for LFS 13 alignment"
```

### Task 9: Write Phase 1 cross-tools recipes (5 files)

**Files:**
- Create: `recipes/cross-tools/binutils-pass1.toml`
- Create: `recipes/cross-tools/gcc-pass1.toml`
- Create: `recipes/cross-tools/linux-headers.toml`
- Create: `recipes/cross-tools/glibc.toml`
- Create: `recipes/cross-tools/libstdcxx.toml`

For each recipe:
1. Fetch the LFS page for exact configure flags
2. Get the source URL and checksum from the LFS packages page
3. Translate into the TOML recipe template

- [ ] **Step 1: Write binutils-pass1.toml**

Fetch: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter05/binutils-pass1.html`

Key flags: `--prefix=$LFS/tools --with-sysroot=$LFS --target=$LFS_TGT --disable-nls --enable-gprofng=no --disable-werror --enable-new-dtags --enable-default-hash-style=gnu`

- [ ] **Step 2: Write gcc-pass1.toml**

Fetch: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter05/gcc-pass1.html`

Note: GCC requires GMP, MPFR, MPC sources to be extracted inside the gcc source tree. The recipe must handle this (download and extract companion libs).

- [ ] **Step 3: Write linux-headers.toml**

Fetch: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter05/linux-headers.html`

Simple: `make mrproper && make headers` then copy to `$LFS/usr/include/`.

- [ ] **Step 4: Write glibc.toml**

Fetch: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter05/glibc.html`

Key flags: `--host=$LFS_TGT --build=$(../scripts/config.guess) --prefix=/usr --enable-kernel=4.19 --with-headers=$LFS/usr/include --disable-nscd libc_cv_slibdir=/usr/lib`

- [ ] **Step 5: Write libstdcxx.toml**

Fetch: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter05/gcc-libstdc++.html`

Built from the GCC source tree. Key flags: `--host=$LFS_TGT --build=$(../config.guess) --prefix=/usr --disable-multilib --disable-nls --disable-libstdcxx-pch --with-gxx-include-dir=/tools/$LFS_TGT/include/c++/15.2.0`

- [ ] **Step 6: Commit**

```bash
git add recipes/cross-tools/ && git commit -m "feat(recipes): add Phase 1 cross-toolchain recipes (LFS Ch5)"
```

---

## Chunk 3: Phase 2 Temporary Tools Recipes

### Task 10: Write Ch6 temporary tool recipes (15 files)

**Files:**
- Create: `recipes/temp-tools/{m4,ncurses,bash,coreutils,diffutils,file,findutils,gawk,grep,gzip,make,patch,sed,tar,xz}.toml`

These are minimal cross-compiled builds. Each uses `--host=$LFS_TGT --build=$(build-aux/config.guess)` and installs to `$LFS`.

- [ ] **Step 1: Fetch LFS Ch6 pages and write all 15 recipes**

For each package, fetch the corresponding LFS page:
- `https://www.linuxfromscratch.org/lfs/view/systemd/chapter06/{package}.html`

These are intentionally simple builds with minimal configure flags. The LFS page for each is typically short.

- [ ] **Step 2: Commit**

```bash
git add recipes/temp-tools/ && git commit -m "feat(recipes): add Phase 2 Ch6 temporary tool recipes (15 packages)"
```

### Task 11: Write binutils-pass2 and gcc-pass2 recipes

**Files:**
- Create: `recipes/temp-tools/binutils-pass2.toml`
- Create: `recipes/temp-tools/gcc-pass2.toml`

These are more complex than the simple temp tools — they rebuild the compiler toolchain with additional features.

- [ ] **Step 1: Write binutils-pass2.toml**

Fetch: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter06/binutils-pass2.html`

Key: builds against `$LFS/tools`, enables 64-bit BFD, uses `--with-build-sysroot=$LFS`.

- [ ] **Step 2: Write gcc-pass2.toml**

Fetch: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter06/gcc-pass2.html`

Key: Enables C and C++, builds libstdc++ with the new compiler, requires GMP/MPFR/MPC extracted in tree.

- [ ] **Step 3: Commit**

```bash
git add recipes/temp-tools/ && git commit -m "feat(recipes): add Phase 2 binutils/gcc pass 2 recipes (LFS Ch6)"
```

### Task 12: Write Ch7 chroot tool recipes (6 files)

**Files:**
- Create: `recipes/temp-tools/{gettext,bison,perl,python,texinfo,util-linux}.toml`

These are built inside the chroot, not cross-compiled. Simpler configure flags (no --host).

- [ ] **Step 1: Fetch LFS Ch7 pages and write all 6 recipes**

For each: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter07/{package}.html`

- [ ] **Step 2: Commit**

```bash
git add recipes/temp-tools/ && git commit -m "feat(recipes): add Phase 2 Ch7 chroot tool recipes (6 packages)"
```

---

## Chunk 4: Phase 3 Final System Recipes (Part A — packages 1-39)

### Task 13: Write system recipes — compression and text (packages 1-15)

**Files:**
- Create: `recipes/system/{man-pages,iana-etc,glibc,zlib,bzip2,xz,lz4,zstd,file,readline,pcre2,m4,bc,flex,pkgconf}.toml`

- [ ] **Step 1: Fetch LFS Ch8 pages and write recipes**

For each package, fetch `https://www.linuxfromscratch.org/lfs/view/systemd/chapter08/{package}.html` and translate to TOML.

Key notes:
- `glibc` (8.5): Complex recipe. Includes timezone data installation, locale generation. Mark as `[deviations]` if differing from LFS.
- `man-pages` (8.3): Just `make prefix=/usr install` — no configure/build step
- `iana-etc` (8.4): Just `cp services protocols /etc/` — no build step
- `bzip2` (8.7): No autotools — uses Makefile directly with custom sed patches
- `pcre2` (8.13): New recipe. Standard autotools.
- `bc` (8.15): New recipe. Uses `./configure --prefix=/usr -O3 -G`

- [ ] **Step 2: Commit**

```bash
git add recipes/system/ && git commit -m "feat(recipes): add Phase 3 system recipes 1-15 (compression, text, early tools)"
```

### Task 14: Write system recipes — toolchain rebuild (packages 16-25)

**Files:**
- Create: `recipes/system/{binutils,gmp,mpfr,mpc,attr,acl,libcap,libxcrypt,shadow,gcc}.toml`

- [ ] **Step 1: Fetch LFS pages and write recipes**

Key notes:
- `binutils` (8.18): Final system build with `--enable-shared --enable-64-bit-bfd`
- `gcc` (8.27): Most complex recipe in the entire build. Requires GMP/MPFR/MPC in-tree. Includes SED fixup for `/usr/lib` multilib path. Full C/C++ with threading.
- `gmp` (8.19): `--enable-cxx` required
- `attr`/`acl` (8.22-8.23): New packages. Standard autotools.
- `libxcrypt` (8.25): New. Password hashing replacement.
- `shadow` (8.26): Disable cracklib, sed to use `/usr/bin/passwd`

- [ ] **Step 2: Commit**

```bash
git add recipes/system/ && git commit -m "feat(recipes): add Phase 3 system recipes 16-25 (toolchain rebuild, security)"
```

### Task 15: Write system recipes — core system (packages 26-39)

**Files:**
- Create: `recipes/system/{ncurses,sed,psmisc,gettext,bison,grep,bash,libtool,gdbm,gperf,expat,inetutils,less,perl}.toml`

- [ ] **Step 1: Fetch LFS pages and write recipes**

Key notes:
- `ncurses` (8.28): Wide character support enabled
- `bash` (8.34): Final system bash with readline
- `gdbm` (8.36): New. `--enable-libgdbm-compat`
- `gperf` (8.37): New. Simple autotools.
- `expat` (8.38): New. Standard autotools.
- `inetutils` (8.39): New. Provides hostname, ping, etc. Disable obsolete daemons.
- `perl` (8.41): Complex. `-Dpager=/usr/bin/less`, `-Dman1dir=/usr/share/man/man1`

- [ ] **Step 2: Commit**

```bash
git add recipes/system/ && git commit -m "feat(recipes): add Phase 3 system recipes 26-39 (core system, database, network)"
```

---

## Chunk 5: Phase 3 Final System Recipes (Part B — packages 40-77) + Kernel

### Task 16: Write system recipes — i18n, crypto, Python packaging (packages 40-54)

**Files:**
- Create: `recipes/system/{xml-parser,intltool,autoconf,automake,openssl,elfutils,libffi,sqlite,python,flit-core,packaging,wheel,setuptools,ninja,meson}.toml`

- [ ] **Step 1: Fetch LFS pages and write recipes**

Key notes:
- `xml-parser` (8.42): Perl module. Uses `perl Makefile.PL`.
- `intltool` (8.43): Requires XML::Parser, sed fix for perl warnings
- `openssl` (8.46): `./config --prefix=/usr --openssldir=/etc/ssl --libdir=lib shared zlib-dynamic`
- `libffi` (8.48): New. `--disable-static --with-gcc-arch=native`
- `sqlite` (8.49): New. Enable FTS and column metadata for Python.
- `python` (8.50): `--enable-shared --with-system-expat --enable-optimizations`
- `flit-core`, `packaging`, `wheel`, `setuptools` (8.51-8.54): Python packaging tools. Use `pip3 install --no-build-isolation`.

- [ ] **Step 2: Commit**

```bash
git add recipes/system/ && git commit -m "feat(recipes): add Phase 3 system recipes 40-54 (i18n, crypto, Python)"
```

### Task 17: Write system recipes — system utilities, docs (packages 55-69)

**Files:**
- Create: `recipes/system/{kmod,coreutils,diffutils,gawk,findutils,groff,gzip,iproute2,kbd,libpipeline,make,patch,tar,texinfo,vim}.toml`

- [ ] **Step 1: Fetch LFS pages and write recipes**

Key notes:
- `kmod` (8.57): Uses meson. `-Dzstd=enabled -Dopenssl=enabled`
- `coreutils` (8.58): Requires ACL/Attr support. Patch for hostname.
- `groff` (8.62): New. `PAGE=letter ./configure --prefix=/usr`
- `kbd` (8.65): New. Patch + autotools.
- `libpipeline` (8.66): New. Standard autotools.
- `texinfo` (8.70): New. Standard autotools.

- [ ] **Step 2: Commit**

```bash
git add recipes/system/ && git commit -m "feat(recipes): add Phase 3 system recipes 55-69 (utilities, docs)"
```

### Task 18: Write system recipes — systemd, boot, final (packages 70-77)

**Files:**
- Create: `recipes/system/{markupsafe,jinja2,systemd,dbus,man-db,procps-ng,util-linux,e2fsprogs}.toml`

- [ ] **Step 1: Fetch LFS pages and write recipes**

Key notes:
- `markupsafe` (8.72): Python package, `pip3 install` with `--no-build-isolation`
- `jinja2` (8.73): Python package, `pip3 install` with `--no-build-isolation`
- `systemd` (8.74): Complex meson build. Add deviation: `-Dbootloader=true` (LFS doesn't set this since they use GRUB). Add `-Dversion-tag=259.5-conary`.
- `dbus` (8.75): `--disable-static --disable-doxygen-docs`
- `man-db` (8.76): New. Requires libpipeline, groff.
- `e2fsprogs` (8.79): New. Build in a subdir. `--enable-elf-shlibs --disable-fsck`

- [ ] **Step 2: Commit**

```bash
git add recipes/system/ && git commit -m "feat(recipes): add Phase 3 system recipes 70-77 (systemd, boot, e2fsprogs)"
```

### Task 19: Write kernel recipe

**Files:**
- Create: `recipes/system/linux.toml`

- [ ] **Step 1: Fetch LFS kernel page and write recipe**

Fetch: `https://www.linuxfromscratch.org/lfs/view/systemd/chapter10/kernel.html`

The kernel recipe must:
- Use `defconfig` (not `menuconfig` — deviation from LFS, documented)
- Apply all required kernel config options via `scripts/config`:
  - LFS required: CGROUPS, MEMCG, DEVTMPFS, DEVTMPFS_MOUNT, TMPFS, TMPFS_POSIX_ACL, INOTIFY_USER, NET, INET, IPV6
  - conaryOS additions: NAMESPACES, USER_NS, PID_NS, NET_NS, OVERLAY_FS, SECCOMP, SECCOMP_FILTER
  - Built-in for no-initramfs: EXT4_FS=y, VIRTIO=y, VIRTIO_PCI=y, VIRTIO_BLK=y, VIRTIO_NET=y, VFAT_FS=y
- Install to `%(destdir)s/boot/vmlinuz-%(version)s` and `%(destdir)s/usr/lib/modules/`

- [ ] **Step 2: Commit**

```bash
git add recipes/system/linux.toml && git commit -m "feat(recipes): add kernel recipe with LFS + QEMU + no-initramfs config"
```

---

## Chunk 6: Phases 4-5 (System Config + Bootable Image)

### Task 20: Implement system_config.rs (Phase 4)

**Files:**
- Modify: `conary-core/src/bootstrap/system_config.rs` (fill in skeleton from Task 2)

- [ ] **Step 1: Move populate_sysroot logic from base.rs**

Copy the body of `BaseBuilder::populate_sysroot()` (lines 1088-1220 of base.rs) into `system_config::configure_system()`. Keep all the same content:
- /etc/passwd, /etc/group, /etc/shadow
- /etc/hostname (conaryos)
- /etc/os-release (conaryOS branding)
- /etc/machine-id (empty)
- /etc/fstab (LABEL=CONARY_ROOT, LABEL=CONARY_ESP)
- /etc/nsswitch.conf
- systemd-networkd DHCP config
- Systemd service symlinks (multi-user.target → default, networkd, serial-getty)

Remove: SSH config (sshd_config) — that's Tier 2 territory.

- [ ] **Step 2: Add LFS Ch9 locale and clock configuration**

Add locale generation and clock config per LFS Ch9 instructions.

- [ ] **Step 3: Verify compilation**

Run: `cargo build`

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat(bootstrap): implement system_config.rs (Phase 4, LFS Ch9)"
```

### Task 21: Update image.rs for Phase 5

**Files:**
- Modify: `conary-core/src/bootstrap/image.rs`

- [ ] **Step 1: Add kernel build step**

Phase 5 builds the kernel from `system/linux.toml` before image creation. Add a method that:
1. Loads `system/linux.toml` recipe
2. Builds the kernel using `build_runner.rs`
3. Installs to sysroot `/boot/vmlinuz-{ver}` and `/usr/lib/modules/{ver}/`

- [ ] **Step 2: Update bootloader config for systemd-boot**

The `finalize_sysroot` logic that's staying in image.rs (bootloader parts) should:
1. Write `loader.conf` to `/boot/loader/loader.conf`
2. Write BLS entry to `/boot/loader/entries/conaryos.conf` — **no initrd line**
3. Copy `systemd-bootx64.efi` from sysroot (hard error if missing)

- [ ] **Step 3: Add conaryos-base.qcow2 output name**

Update the default output name to `conaryos-base.qcow2` for Tier 1 images.

- [ ] **Step 4: Verify compilation**

Run: `cargo build`

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(bootstrap): update image.rs for Phase 5 (kernel build, systemd-boot, no initramfs)"
```

---

## Chunk 7: Phase 6 (Tier 2) + Integration + Cleanup

### Task 22: Write Tier 2 recipes (8 files)

**Files:**
- Create: `recipes/tier2/{linux-pam,openssh,ca-certificates,curl,sudo,nano,rust,conary}.toml`

- [ ] **Step 1: Write BLFS-based recipes**

For openssh, linux-pam, curl, sudo, nano: fetch BLFS pages for configure flags:
- `https://www.linuxfromscratch.org/blfs/view/systemd/`

For openssh: fix the PAM contradiction — build with `--without-pam` initially (matching the recipe), OR build with PAM and set `UsePAM yes`. Pick one and be consistent.

For ca-certificates: install Mozilla CA bundle.

- [ ] **Step 2: Write rust.toml and conary.toml**

These are adapted from the existing `recipes/conary/` recipes:
- `rust.toml`: Downloads Rust 1.94.0 bootstrap binary, runs install.sh
- `conary.toml`: Copies source, runs `cargo build --release`, installs binary

- [ ] **Step 3: Commit**

```bash
git add recipes/tier2/ && git commit -m "feat(recipes): add Tier 2 recipes (openssh, curl, Conary, etc.)"
```

### Task 23: Implement tier2.rs and second image creation

**Files:**
- Modify: `conary-core/src/bootstrap/tier2.rs` (fill in skeleton)

- [ ] **Step 1: Implement Tier 2 builder**

The Tier 2 builder:
1. Continues in the same chroot from Phase 3/4
2. Builds each TIER2_ORDER package using `build_runner.rs` with `BuildContext::Chroot`
3. Adds SSH configuration (sshd_config, host keys, test keypair) — moved from old `populate_sysroot()`. **Important:** `generate_ssh_host_keys()` must use the sysroot's `/usr/bin/ssh-keygen` (built from openssh in Tier 2), NOT the host's ssh-keygen. If openssh hasn't been built, this is an error.
4. Creates second image by calling `ImageBuilder::new()` with output path `conaryos.qcow2` (same `ImageBuilder` API used for Tier 1, just different output name). The sysroot is the same — it now has Tier 2 packages installed on top.

- [ ] **Step 2: Commit**

```bash
git add -A && git commit -m "feat(bootstrap): implement tier2.rs (Phase 6, BLFS + Conary)"
```

### Task 24: Update QEMU test manifest

**Files:**
- Modify: `tests/integration/remi/manifests/phase3-group-n-qemu.toml`

- [ ] **Step 1: Update T156 for new image names**

Change image reference from `minimal-boot-v2` to `conaryos` (Tier 2 image). Update expected output strings if needed.

- [ ] **Step 2: Commit**

```bash
git add -A && git commit -m "test(qemu): update T156 for conaryos image name"
```

### Task 25: Final cleanup

- [ ] **Step 1: Delete old modules**

Delete the three replaced modules and remove their `mod` declarations and `pub use` lines from `mod.rs`:

```bash
rm conary-core/src/bootstrap/base.rs
rm conary-core/src/bootstrap/stage1.rs
rm conary-core/src/bootstrap/conary_stage.rs
```

- `base.rs` → split into `final_system.rs` + `system_config.rs`
- `stage1.rs` → replaced by `cross_tools.rs`
- `conary_stage.rs` → replaced by `tier2.rs`

- [ ] **Step 3: Verify full compilation and tests**

```bash
cargo build && cargo build --features server && cargo test && cargo clippy -- -D warnings
```

- [ ] **Step 4: Delete recipes/archive if no longer needed**

Or keep it — the old recipes are useful reference for configure flags that worked.

- [ ] **Step 5: Final commit**

```bash
git add -A && git commit -m "refactor(bootstrap): complete LFS 13 alignment — delete old stage/base modules"
```
