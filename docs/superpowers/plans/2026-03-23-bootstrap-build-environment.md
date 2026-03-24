# Bootstrap Build Environment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the multi-stage EROFS derivation pipeline with a mutable chroot pipeline supporting multiple seed sources, and add output-hash convergence verification.

**Architecture:** Single mutable overlayfs chroot where packages build sequentially in topological order. Each package's DESTDIR is captured to CAS then installed into the live chroot. Seed is pluggable (adopted distro, Phase 1+2, community). Content-addressed output hashes enable cross-seed convergence verification.

**Tech Stack:** Rust 1.94, SQLite, EROFS, overlayfs, composefs, CAS (content-addressable storage)

**Spec:** `docs/superpowers/specs/2026-03-23-bootstrap-build-environment-design.md`

---

## File Structure

### New Files

| File | Responsibility |
|------|----------------|
| `conary-core/src/derivation/build_order.rs` | Flat topological sort replacing staged assignment. `compute_build_order()` returns `Vec<BuildStep>` with informational `BuildPhase` labels. |
| `conary-core/src/derivation/install.rs` | Install CAS manifest entries into live chroot sysroot. Walk `OutputManifest`, hardlink/copy files from CAS, create symlinks, run `ldconfig`. |
| `conary-core/src/derivation/convergence.rs` | `verify-convergence` comparison logic: compare output hashes across seeds, produce match/mismatch report, `--diff` file-level detail. |
| `conary-core/src/bootstrap/adopt_seed.rs` | Create seed EROFS from adopted system filesystem. Probe-based validation. |

### Modified Files

| File | What Changes |
|------|-------------|
| `conary-core/src/derivation/mod.rs` | Add `build_order`, `install`, `convergence` modules. Update re-exports. |
| `conary-core/src/derivation/pipeline.rs` | Add `BuildMode::Chroot` variant. New `execute_chroot()` method: single loop, install-between-builds. Keep `execute()` for staged mode. |
| `conary-core/src/derivation/environment.rs` | Add `MutableEnvironment` for overlayfs with upperdir. Two-step mount (composefs then overlay) for CAS seeds, single-step for adopted seeds. Upper dir persistence. |
| `conary-core/src/derivation/output.rs` | Update `compute_output_hash()` to v2 format (include permissions via `OutputFile.mode`). Add `hash_version` field. |
| `conary-core/src/derivation/seed.rs` | Add `SeedSource::Adopted`. Add `origin_distro`, `origin_version` fields. Add `SeedValidation` probe. |
| `conary-core/src/db/schema.rs` | Bump to v57. Add `output_equivalence` table. |
| `conary-core/src/db/migrations.rs` | Add `migrate_v57()`. |
| `conary-core/src/bootstrap/mod.rs` | Wire `seed --from-adopted` to `adopt_seed.rs`. |
| `src/cli/bootstrap.rs` | Add `--from-adopted` flag to `Seed` variant, add `VerifyConvergence` and `DiffSeeds` variants to `BootstrapCommands`, add `--mode` flag to `Run` variant. |
| `src/commands/bootstrap.rs` | Wire new CLI variants to core functions. Existing bootstrap commands are in this file. |

### Deleted Code (within existing files)

| Location | What | Why |
|----------|------|-----|
| `derivation/stages.rs` | `FOUNDATION_PACKAGES` constant, `assign_stages()`, helpers | Replaced by `build_order.rs`. **Keep `stages.rs` and its `mod.rs` re-exports for backward compat** -- the staged pipeline mode (`execute()`) still uses it. Deprecate with `#[deprecated]` attribute. |
| `derivation/pipeline.rs` | Per-stage EROFS composition loop | Replaced by single-compose-at-end |

---

## Task 1: Build Order Module (replace staged assignment)

**Files:**
- Create: `conary-core/src/derivation/build_order.rs`
- Modify: `conary-core/src/derivation/mod.rs`
- Reference: `conary-core/src/derivation/stages.rs` (existing toposort to reuse)

- [ ] **Step 1: Write failing test for `compute_build_order` with simple 3-package graph**

In `conary-core/src/derivation/build_order.rs`, at the bottom in `#[cfg(test)]`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::derivation::test_helpers::helpers::make_recipe;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn test_build_order_respects_makedepends() {
        let mut recipes = HashMap::new();
        // gcc depends on gmp
        recipes.insert("gmp".into(), make_recipe("gmp", &[], &[]));
        recipes.insert("gcc".into(), make_recipe("gcc", &["gmp"], &[]));
        recipes.insert("bash".into(), make_recipe("bash", &[], &[]));

        let order = compute_build_order(&recipes, &HashSet::new()).unwrap();

        let gmp_pos = order.iter().position(|s| s.package == "gmp").unwrap();
        let gcc_pos = order.iter().position(|s| s.package == "gcc").unwrap();
        assert!(gmp_pos < gcc_pos, "gmp must build before gcc");
        assert_eq!(order.len(), 3);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core build_order::tests::test_build_order_respects_makedepends`
Expected: FAIL -- module doesn't exist yet.

- [ ] **Step 3: Write `BuildPhase`, `BuildStep`, `compute_build_order`**

Create `conary-core/src/derivation/build_order.rs`:

```rust
// conary-core/src/derivation/build_order.rs

//! Flat topological build ordering for bootstrap packages.
//!
//! Replaces the staged assignment model with a single topological sort.
//! Packages are labeled with informational `BuildPhase` tags for progress
//! reporting, but phases are not build boundaries.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;

use crate::recipe::Recipe;

/// Informational build phase label (not a build boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BuildPhase {
    Toolchain,
    Foundation,
    System,
    Customization,
}

impl fmt::Display for BuildPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toolchain => write!(f, "toolchain"),
            Self::Foundation => write!(f, "foundation"),
            Self::System => write!(f, "system"),
            Self::Customization => write!(f, "customization"),
        }
    }
}

/// A package's position in the global build order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStep {
    pub package: String,
    pub order: usize,
    pub phase: BuildPhase,
}

#[derive(Debug, thiserror::Error)]
pub enum BuildOrderError {
    #[error("cyclic dependency detected in build graph")]
    CyclicDependency,
}

/// Toolchain packages identified by name.
///
/// Note: gcc-pass1/gcc-pass2 are detected via pass suffix (not listed here).
/// The full `gcc` rebuild and its deps (gmp, mpfr, mpc) end up in System
/// via topological sort -- they sort after Foundation tools naturally.
const TOOLCHAIN_NAMED: &[&str] = &[
    "linux-headers", "glibc", "binutils", "libstdcxx",
];

/// Foundation packages (essential build tools).
const FOUNDATION_NAMED: &[&str] = &[
    "make", "bash", "coreutils", "sed", "gawk", "grep", "findutils",
    "diffutils", "patch", "tar", "gzip", "xz", "bzip2", "m4", "bison",
    "flex", "gettext", "perl", "python", "texinfo", "util-linux", "file",
    "ncurses", "readline", "zlib",
];

/// Compute a flat, topologically sorted build order for all recipes.
pub fn compute_build_order(
    recipes: &HashMap<String, Recipe>,
    custom_packages: &HashSet<String>,
) -> Result<Vec<BuildStep>, BuildOrderError> {
    let all_packages: BTreeSet<String> = recipes.keys().cloned().collect();
    let sorted = topological_sort(&all_packages, recipes)?;

    Ok(sorted
        .into_iter()
        .enumerate()
        .map(|(order, package)| {
            let phase = classify_phase(&package, custom_packages);
            BuildStep { package, order, phase }
        })
        .collect())
}

fn classify_phase(name: &str, custom: &HashSet<String>) -> BuildPhase {
    if custom.contains(name) {
        BuildPhase::Customization
    } else if TOOLCHAIN_NAMED.contains(&name) {
        BuildPhase::Toolchain
    } else if FOUNDATION_NAMED.contains(&name) {
        BuildPhase::Foundation
    } else {
        BuildPhase::System
    }
}

/// Topologically sort packages using Kahn's algorithm.
///
/// Uses `BTreeMap`/`BTreeSet` for deterministic output.
fn topological_sort(
    packages: &BTreeSet<String>,
    recipes: &HashMap<String, Recipe>,
) -> Result<Vec<String>, BuildOrderError> {
    // (Copy the existing topological_sort from stages.rs -- same algorithm,
    //  but operates on ALL packages, not per-stage subsets)
    // ...
}
```

Note to implementer: Copy the `topological_sort` function body from `stages.rs:241-303`. It works identically -- the only difference is the input set is all packages instead of a per-stage subset.

- [ ] **Step 4: Register module in `mod.rs`**

In `conary-core/src/derivation/mod.rs`, add:
```rust
pub mod build_order;
```
And add re-export:
```rust
pub use build_order::{BuildPhase, BuildStep, BuildOrderError, compute_build_order};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p conary-core build_order::tests::test_build_order_respects_makedepends`
Expected: PASS

- [ ] **Step 6: Write test for phase classification**

```rust
#[test]
fn test_phase_classification() {
    let mut recipes = HashMap::new();
    recipes.insert("glibc".into(), make_recipe("glibc", &[], &[]));
    recipes.insert("bash".into(), make_recipe("bash", &[], &[]));
    recipes.insert("openssl".into(), make_recipe("openssl", &[], &[]));
    recipes.insert("my-app".into(), make_recipe("my-app", &[], &[]));

    let custom: HashSet<String> = ["my-app".into()].into();
    let order = compute_build_order(&recipes, &custom).unwrap();

    let glibc = order.iter().find(|s| s.package == "glibc").unwrap();
    let bash = order.iter().find(|s| s.package == "bash").unwrap();
    let openssl = order.iter().find(|s| s.package == "openssl").unwrap();
    let my_app = order.iter().find(|s| s.package == "my-app").unwrap();

    assert_eq!(glibc.phase, BuildPhase::Toolchain);
    assert_eq!(bash.phase, BuildPhase::Foundation);
    assert_eq!(openssl.phase, BuildPhase::System);
    assert_eq!(my_app.phase, BuildPhase::Customization);
}
```

- [ ] **Step 7: Run test**

Run: `cargo test -p conary-core build_order::tests::test_phase_classification`
Expected: PASS (already implemented in step 3)

- [ ] **Step 8: Write test for cycle detection**

```rust
#[test]
fn test_cycle_detection() {
    let mut recipes = HashMap::new();
    recipes.insert("a".into(), make_recipe("a", &["b"], &[]));
    recipes.insert("b".into(), make_recipe("b", &["a"], &[]));

    let result = compute_build_order(&recipes, &HashSet::new());
    assert!(matches!(result, Err(BuildOrderError::CyclicDependency)));
}
```

- [ ] **Step 9: Run test, verify passes**

Run: `cargo test -p conary-core build_order::tests::test_cycle_detection`
Expected: PASS

- [ ] **Step 10: Run clippy and all derivation tests**

Run: `cargo clippy -p conary-core -- -D warnings && cargo test -p conary-core derivation`
Expected: All pass, no warnings.

- [ ] **Step 11: Commit**

```bash
git add conary-core/src/derivation/build_order.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add flat build_order module replacing staged assignment"
```

---

## Task 2: Chroot Install Module

**Files:**
- Create: `conary-core/src/derivation/install.rs`
- Modify: `conary-core/src/derivation/mod.rs`
- Reference: `conary-core/src/derivation/output.rs` (OutputManifest, OutputFile, OutputSymlink)
- Reference: `conary-core/src/filesystem/cas.rs` (CasStore)

- [ ] **Step 1: Write failing test for install_to_sysroot**

In `conary-core/src/derivation/install.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::derivation::output::{OutputFile, OutputManifest, OutputSymlink};
    use tempfile::TempDir;

    #[test]
    fn test_install_creates_files_and_symlinks() {
        let sysroot = TempDir::new().unwrap();
        let cas_dir = TempDir::new().unwrap();

        // Create a fake CAS object
        let content = b"#!/bin/sh\necho hello\n";
        let hash = crate::hash::sha256(content);
        let cas_path = cas_dir.path().join(&hash[..2]).join(&hash[2..]);
        std::fs::create_dir_all(cas_path.parent().unwrap()).unwrap();
        std::fs::write(&cas_path, content).unwrap();

        let manifest = OutputManifest {
            derivation_id: "test".into(),
            output_hash: "test".into(),
            files: vec![OutputFile {
                path: "/usr/bin/hello".into(),
                hash: hash.clone(),
                size: content.len() as u64,
                mode: 0o755,
            }],
            symlinks: vec![OutputSymlink {
                path: "/usr/bin/hi".into(),
                target: "hello".into(),
            }],
            build_duration_secs: 0,
            built_at: String::new(),
        };

        install_to_sysroot(&manifest, sysroot.path(), cas_dir.path()).unwrap();

        let installed = sysroot.path().join("usr/bin/hello");
        assert!(installed.exists());
        assert_eq!(std::fs::read(&installed).unwrap(), content);

        let link = sysroot.path().join("usr/bin/hi");
        assert!(link.is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap().to_str().unwrap(), "hello");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core install::tests::test_install_creates_files_and_symlinks`
Expected: FAIL -- module doesn't exist.

- [ ] **Step 3: Write `install_to_sysroot` implementation**

```rust
// conary-core/src/derivation/install.rs

//! Install derivation outputs into a live chroot sysroot.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tracing::{info, warn};

use super::output::OutputManifest;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("I/O error installing {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("CAS object not found: {0}")]
    MissingCasObject(String),
}

/// Install all files and symlinks from a manifest into the sysroot.
///
/// Reads file content from CAS objects (at `cas_dir/<hash[..2]>/<hash[2..]>`).
/// Creates parent directories as needed. Uses hard links when possible.
pub fn install_to_sysroot(
    manifest: &OutputManifest,
    sysroot: &Path,
    cas_dir: &Path,
) -> Result<u64, InstallError> {
    let mut installed_count: u64 = 0;

    for file in &manifest.files {
        let dest = sysroot.join(file.path.trim_start_matches('/'));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| InstallError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }

        let cas_path = cas_dir.join(&file.hash[..2]).join(&file.hash[2..]);
        if !cas_path.exists() {
            return Err(InstallError::MissingCasObject(file.hash.clone()));
        }

        // Remove existing file before installing (last-writer-wins)
        let _ = std::fs::remove_file(&dest);

        // Try hard link first, fall back to copy
        if std::fs::hard_link(&cas_path, &dest).is_err() {
            std::fs::copy(&cas_path, &dest).map_err(|e| InstallError::Io {
                path: dest.display().to_string(),
                source: e,
            })?;
        }

        // Set permissions
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(file.mode))
            .map_err(|e| InstallError::Io {
                path: dest.display().to_string(),
                source: e,
            })?;

        installed_count += 1;
    }

    for symlink in &manifest.symlinks {
        let dest = sysroot.join(symlink.path.trim_start_matches('/'));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| InstallError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }

        // Remove existing symlink/file if present (last-writer-wins)
        let _ = std::fs::remove_file(&dest);

        std::os::unix::fs::symlink(&symlink.target, &dest).map_err(|e| InstallError::Io {
            path: dest.display().to_string(),
            source: e,
        })?;

        installed_count += 1;
    }

    Ok(installed_count)
}

/// Run ldconfig inside the chroot if any .so files were installed.
pub fn run_ldconfig_if_needed(manifest: &OutputManifest, sysroot: &Path) {
    let has_shared_libs = manifest.files.iter().any(|f| {
        let p = &f.path;
        p.ends_with(".so") || p.contains(".so.")
    });
    if has_shared_libs {
        let ldconfig = sysroot.join("sbin/ldconfig")
            .exists()
            .then(|| "/sbin/ldconfig")
            .or_else(|| sysroot.join("usr/sbin/ldconfig").exists().then(|| "/usr/sbin/ldconfig"));

        if let Some(ldconfig_path) = ldconfig {
            let status = std::process::Command::new("chroot")
                .arg(sysroot)
                .arg(ldconfig_path)
                .status();
            match status {
                Ok(s) if s.success() => info!("ldconfig updated in sysroot"),
                Ok(s) => warn!("ldconfig exited with {s}"),
                Err(e) => warn!("ldconfig failed: {e}"),
            }
        }
    }
}
```

- [ ] **Step 4: Register module in `mod.rs`**

Add `pub mod install;` and `pub use install::{InstallError, install_to_sysroot};`

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p conary-core install::tests::test_install_creates_files_and_symlinks`
Expected: PASS

- [ ] **Step 6: Write test for conflict handling (last-writer-wins)**

```rust
#[test]
fn test_install_overwrites_existing_file() {
    let sysroot = TempDir::new().unwrap();
    let cas_dir = TempDir::new().unwrap();

    // Pre-create a file at the target path
    let dest = sysroot.path().join("usr/bin/tool");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, b"old content").unwrap();

    let content = b"new content";
    let hash = crate::hash::sha256(content);
    let cas_path = cas_dir.path().join(&hash[..2]).join(&hash[2..]);
    std::fs::create_dir_all(cas_path.parent().unwrap()).unwrap();
    std::fs::write(&cas_path, content).unwrap();

    let manifest = OutputManifest {
        derivation_id: "test2".into(),
        output_hash: "test2".into(),
        files: vec![OutputFile {
            path: "/usr/bin/tool".into(),
            hash,
            size: content.len() as u64,
            mode: 0o755,
        }],
        symlinks: vec![],
        build_duration_secs: 0,
        built_at: String::new(),
    };

    install_to_sysroot(&manifest, sysroot.path(), cas_dir.path()).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"new content");
}
```

- [ ] **Step 7: Run all install tests, clippy**

Run: `cargo test -p conary-core install::tests && cargo clippy -p conary-core -- -D warnings`
Expected: All pass.

- [ ] **Step 8: Commit**

```bash
git add conary-core/src/derivation/install.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add chroot install module for mutable sysroot"
```

---

## Task 3: Mutable Environment (overlayfs with upperdir)

**Files:**
- Modify: `conary-core/src/derivation/environment.rs`
- Reference: `conary-core/src/generation/mount.rs` (MountOptions, mount_generation)

- [ ] **Step 1: Write failing test for `MutableEnvironment`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutable_env_creates_upper_and_work_dirs() {
        let work_dir = tempfile::TempDir::new().unwrap();
        let env = MutableEnvironment::new(
            PathBuf::from("/fake/seed.erofs"),
            PathBuf::from("/fake/cas"),
            work_dir.path().to_path_buf(),
            "abc123".into(),
        );

        assert_eq!(env.upper_dir(), work_dir.path().join("upper"));
        assert_eq!(env.work_dir(), work_dir.path().join("work"));
        assert_eq!(env.sysroot(), work_dir.path().join("sysroot"));
        assert!(!env.is_mounted());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core environment::tests::test_mutable_env_creates_upper_and_work_dirs`
Expected: FAIL -- `MutableEnvironment` doesn't exist.

- [ ] **Step 3: Add `MutableEnvironment` struct**

In `conary-core/src/derivation/environment.rs`, add alongside the existing `BuildEnvironment`:

```rust
/// A mutable build sysroot using overlayfs on top of a seed image.
///
/// The seed image is mounted read-only (via composefs or loopback EROFS),
/// then an overlayfs is stacked with a writable upperdir. Package installs
/// go to the upperdir; the seed stays pristine.
pub struct MutableEnvironment {
    /// Path to the seed EROFS image.
    image_path: PathBuf,
    /// CAS object directory for composefs seeds.
    cas_dir: PathBuf,
    /// Base working directory (contains upper/, work/, sysroot/).
    base_dir: PathBuf,
    /// SHA-256 of the seed image.
    seed_id: String,
    /// Whether the overlay is currently mounted.
    mounted: bool,
    /// Inner read-only mount of the seed (kept alive for overlayfs lowerdir).
    seed_env: Option<BuildEnvironment>,
}

impl MutableEnvironment {
    pub fn new(
        image_path: PathBuf,
        cas_dir: PathBuf,
        base_dir: PathBuf,
        seed_id: String,
    ) -> Self {
        Self { image_path, cas_dir, base_dir, seed_id, mounted: false, seed_env: None }
    }

    pub fn upper_dir(&self) -> PathBuf { self.base_dir.join("upper") }
    pub fn work_dir(&self) -> PathBuf { self.base_dir.join("work") }
    pub fn sysroot(&self) -> PathBuf { self.base_dir.join("sysroot") }
    pub fn is_mounted(&self) -> bool { self.mounted }

    /// Check if the upper directory was created for a different seed.
    /// Returns true if it should be wiped and recreated.
    pub fn needs_reset(&self) -> bool {
        let marker = self.base_dir.join(".seed_id");
        match std::fs::read_to_string(&marker) {
            Ok(id) => id.trim() != self.seed_id,
            Err(_) => false, // No marker = fresh directory
        }
    }

    /// Mount the seed as a mutable overlayfs sysroot.
    ///
    /// 1. Mount seed EROFS read-only at a temporary point
    /// 2. Stack overlayfs with upperdir for writes
    pub fn mount(&mut self) -> Result<(), EnvironmentError> {
        if self.mounted { return Ok(()); }

        // Create directory structure
        for dir in [self.upper_dir(), self.work_dir(), self.sysroot()] {
            std::fs::create_dir_all(&dir)
                .map_err(|e| EnvironmentError::Mount(format!("{}: {e}", dir.display())))?;
        }

        // Reset upper if seed changed
        if self.needs_reset() {
            info!("Seed changed, wiping upper directory");
            let _ = std::fs::remove_dir_all(self.upper_dir());
            let _ = std::fs::remove_dir_all(self.work_dir());
            std::fs::create_dir_all(self.upper_dir())
                .map_err(|e| EnvironmentError::Mount(e.to_string()))?;
            std::fs::create_dir_all(self.work_dir())
                .map_err(|e| EnvironmentError::Mount(e.to_string()))?;
        }

        // Step 1: Mount seed EROFS read-only
        let ro_mount = self.base_dir.join("seed_ro");
        std::fs::create_dir_all(&ro_mount)
            .map_err(|e| EnvironmentError::Mount(e.to_string()))?;

        // Try composefs mount first, fall back to EROFS loopback
        let mut seed_env = BuildEnvironment::new(
            self.image_path.clone(),
            self.cas_dir.clone(),
            ro_mount.clone(),
            self.seed_id.clone(),
        );
        seed_env.mount()?;
        self.seed_env = Some(seed_env);  // Keep alive for overlayfs lowerdir

        // Step 2: Stack overlayfs
        let opts = format!(
            "lowerdir={},upperdir={},workdir={}",
            ro_mount.display(),
            self.upper_dir().display(),
            self.work_dir().display(),
        );
        let status = Command::new("mount")
            .args(["-t", "overlay", "overlay", "-o", &opts])
            .arg(self.sysroot())
            .status()
            .map_err(|e| EnvironmentError::Mount(e.to_string()))?;

        if !status.success() {
            return Err(EnvironmentError::Mount("overlayfs mount failed".into()));
        }

        // Write seed ID marker for resume detection
        let _ = std::fs::write(self.base_dir.join(".seed_id"), &self.seed_id);

        self.mounted = true;
        Ok(())
    }

    pub fn unmount(&mut self) -> Result<(), EnvironmentError> {
        if !self.mounted { return Ok(()); }

        // Unmount overlay first
        let status = Command::new("umount")
            .arg(self.sysroot())
            .status()
            .map_err(|e| EnvironmentError::Unmount(e.to_string()))?;
        if !status.success() {
            // Try lazy unmount as fallback
            let _ = Command::new("umount").arg("-l").arg(self.sysroot()).status();
        }

        // Then unmount seed read-only mount
        if let Some(mut env) = self.seed_env.take() {
            let _ = env.unmount();
        }

        self.mounted = false;
        Ok(())
    }
}

impl Drop for MutableEnvironment {
    fn drop(&mut self) {
        if self.mounted {
            let _ = self.unmount();
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p conary-core environment::tests::test_mutable_env_creates_upper_and_work_dirs`
Expected: PASS

- [ ] **Step 5: Write test for `needs_reset` logic**

```rust
#[test]
fn test_needs_reset_detects_seed_change() {
    let work_dir = tempfile::TempDir::new().unwrap();

    // No marker file = no reset needed
    let env = MutableEnvironment::new(
        PathBuf::from("/fake"), PathBuf::from("/fake"),
        work_dir.path().to_path_buf(), "seed_a".into(),
    );
    assert!(!env.needs_reset());

    // Write marker for seed_a
    std::fs::write(work_dir.path().join(".seed_id"), "seed_a").unwrap();
    assert!(!env.needs_reset());

    // Different seed = needs reset
    let env2 = MutableEnvironment::new(
        PathBuf::from("/fake"), PathBuf::from("/fake"),
        work_dir.path().to_path_buf(), "seed_b".into(),
    );
    assert!(env2.needs_reset());
}
```

- [ ] **Step 6: Run test, verify passes**

Run: `cargo test -p conary-core environment::tests`
Expected: All pass.

- [ ] **Step 7: Clippy**

Run: `cargo clippy -p conary-core -- -D warnings`
Expected: Clean.

- [ ] **Step 8: Commit**

```bash
git add conary-core/src/derivation/environment.rs
git commit -m "feat(derivation): add MutableEnvironment with overlayfs upperdir"
```

---

## Task 4: Seed Abstraction (Adopted source + validation)

**Files:**
- Modify: `conary-core/src/derivation/seed.rs`
- Create: `conary-core/src/bootstrap/adopt_seed.rs`
- Modify: `conary-core/src/bootstrap/mod.rs`

- [ ] **Step 1: Write failing test for `SeedSource::Adopted` serde**

In `conary-core/src/derivation/seed.rs` test section:

```rust
#[test]
fn test_adopted_source_serde() {
    let meta = SeedMetadata {
        seed_id: "abc".into(),
        source: SeedSource::Adopted,
        origin_url: None,
        builder: None,
        packages: vec!["gcc".into()],
        target_triple: "x86_64-unknown-linux-gnu".into(),
        verified_by: vec![],
        origin_distro: Some("archlinux".into()),
        origin_version: Some("2026.03.01".into()),
    };
    let toml_str = toml::to_string(&meta).unwrap();
    assert!(toml_str.contains("source = \"adopted\""));
    assert!(toml_str.contains("origin_distro = \"archlinux\""));

    let parsed: SeedMetadata = toml::from_str(&toml_str).unwrap();
    assert_eq!(parsed.source, SeedSource::Adopted);
    assert_eq!(parsed.origin_distro.as_deref(), Some("archlinux"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core seed::tests::test_adopted_source_serde`
Expected: FAIL -- `SeedSource::Adopted` doesn't exist.

- [ ] **Step 3: Add `Adopted` variant and new fields**

In `seed.rs`, modify `SeedSource`:
```rust
pub enum SeedSource {
    Community,
    Imported,
    SelfBuilt,
    Adopted,    // NEW
}
```

Add fields to `SeedMetadata`:
```rust
pub struct SeedMetadata {
    // ... existing fields ...
    /// Distro name for adopted seeds (e.g., "archlinux").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_distro: Option<String>,
    /// Distro version for adopted seeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_version: Option<String>,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p conary-core seed::tests::test_adopted_source_serde`
Expected: PASS

- [ ] **Step 5: Write `SeedValidation` probe**

In `seed.rs`:

```rust
/// Result of probing a seed's build environment capabilities.
#[derive(Debug)]
pub struct SeedValidation {
    pub has_c_compiler: bool,
    pub has_libc_headers: bool,
    pub has_make: bool,
    pub has_shell: bool,
    pub has_coreutils: bool,
    pub has_binutils: bool,
}

impl SeedValidation {
    /// Probe a mounted sysroot for required build tools.
    pub fn probe(sysroot: &Path) -> Self {
        Self {
            has_c_compiler: probe_cmd(sysroot, &["gcc", "--version"]),
            has_libc_headers: sysroot.join("usr/include/stdio.h").exists(),
            has_make: probe_cmd(sysroot, &["make", "--version"]),
            has_shell: probe_cmd(sysroot, &["/bin/sh", "-c", "echo ok"]),
            has_coreutils: probe_cmd(sysroot, &["ls", "--version"]),
            has_binutils: probe_cmd(sysroot, &["ld", "--version"]),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.has_c_compiler && self.has_libc_headers && self.has_make
            && self.has_shell && self.has_coreutils && self.has_binutils
    }

    pub fn missing_tools(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.has_c_compiler { missing.push("gcc"); }
        if !self.has_libc_headers { missing.push("libc headers"); }
        if !self.has_make { missing.push("make"); }
        if !self.has_shell { missing.push("/bin/sh"); }
        if !self.has_coreutils { missing.push("coreutils"); }
        if !self.has_binutils { missing.push("binutils (ld)"); }
        missing
    }
}

fn probe_cmd(sysroot: &Path, args: &[&str]) -> bool {
    std::process::Command::new("chroot")
        .arg(sysroot)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
```

- [ ] **Step 6: Write unit test for `SeedValidation::missing_tools`**

```rust
#[test]
fn test_seed_validation_missing_tools() {
    let v = SeedValidation {
        has_c_compiler: true,
        has_libc_headers: false,
        has_make: true,
        has_shell: true,
        has_coreutils: false,
        has_binutils: true,
    };
    assert!(!v.is_valid());
    assert_eq!(v.missing_tools(), vec!["libc headers", "coreutils"]);
}
```

- [ ] **Step 7: Run all seed tests, clippy**

Run: `cargo test -p conary-core seed::tests && cargo clippy -p conary-core -- -D warnings`
Expected: All pass.

- [ ] **Step 8: Commit**

```bash
git add conary-core/src/derivation/seed.rs
git commit -m "feat(derivation): add Adopted seed source with probe-based validation"
```

---

## Task 5: Output Hash v2 (add permissions)

**Files:**
- Modify: `conary-core/src/derivation/output.rs`

- [ ] **Step 1: Write failing test for v2 hash format**

```rust
#[test]
fn test_output_hash_v2_includes_permissions() {
    let files = vec![
        OutputFile { path: "/usr/bin/hello".into(), hash: "abc123".into(), size: 100, mode: 0o755 },
    ];
    let symlinks = vec![];

    let v1 = OutputManifest::compute_output_hash(&files, &symlinks);
    let v2 = OutputManifest::compute_output_hash_v2(&files, &symlinks);

    // v1 and v2 should differ (v2 includes mode)
    assert_ne!(v1, v2);

    // v2 should change when mode changes
    let files_644 = vec![
        OutputFile { path: "/usr/bin/hello".into(), hash: "abc123".into(), size: 100, mode: 0o644 },
    ];
    let v2_644 = OutputManifest::compute_output_hash_v2(&files_644, &symlinks);
    assert_ne!(v2, v2_644);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core output::tests::test_output_hash_v2_includes_permissions`
Expected: FAIL -- `compute_output_hash_v2` doesn't exist.

- [ ] **Step 3: Add `hash_version` field to `OutputManifest` and `compute_output_hash_v2`**

In `output.rs`, add field to `OutputManifest`:
```rust
pub struct OutputManifest {
    // ... existing fields ...
    /// Hash format version (1 = original, 2 = with permissions). Default 1.
    #[serde(default = "default_hash_version")]
    pub hash_version: u8,
}

fn default_hash_version() -> u8 { 1 }
```

Then add the v2 method:

```rust
/// Compute output hash v2 (includes file permissions).
///
/// Format: `file:<path>:<mode_octal>:<content_hash>` (sorted by path)
/// followed by `symlink:<path>:<target>` (sorted by path).
#[must_use]
pub fn compute_output_hash_v2(files: &[OutputFile], symlinks: &[OutputSymlink]) -> String {
    let mut hasher = hash::Hasher::new(hash::HashAlgorithm::Sha256);

    let mut sorted_files: Vec<&OutputFile> = files.iter().collect();
    sorted_files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut sorted_symlinks: Vec<&OutputSymlink> = symlinks.iter().collect();
    sorted_symlinks.sort_by(|a, b| a.path.cmp(&b.path));

    for file in sorted_files {
        hasher.update(format!("file:{}:{:o}:{}\n", file.path, file.mode, file.hash).as_bytes());
    }

    for symlink in sorted_symlinks {
        hasher.update(format!("symlink:{}:{}\n", symlink.path, symlink.target).as_bytes());
    }

    hasher.finalize().value
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p conary-core output::tests::test_output_hash_v2_includes_permissions`
Expected: PASS

- [ ] **Step 5: Clippy, commit**

Run: `cargo clippy -p conary-core -- -D warnings`

```bash
git add conary-core/src/derivation/output.rs
git commit -m "feat(derivation): add output hash v2 with permissions"
```

---

## Task 6: DB Migration v57 (output_equivalence table)

**Files:**
- Modify: `conary-core/src/db/schema.rs` (bump `SCHEMA_VERSION` to 57)
- Modify: `conary-core/src/db/migrations.rs` (add `migrate_v57`)

- [ ] **Step 1: Write `migrate_v57` in `migrations.rs`**

```rust
pub fn migrate_v57(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS output_equivalence (
            package_name TEXT NOT NULL,
            output_hash TEXT NOT NULL,
            derivation_id TEXT NOT NULL,
            seed_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (package_name, output_hash, seed_id)
        );

        CREATE INDEX IF NOT EXISTS idx_output_equivalence_hash
            ON output_equivalence(output_hash);",
    )?;
    Ok(())
}
```

- [ ] **Step 2: Wire into `run_migrations` dispatch and bump `SCHEMA_VERSION` to 57**

In `schema.rs`, change `pub const SCHEMA_VERSION: i32 = 56;` to `57`.
Add the v57 case to the migration dispatch in `run_migrations()`.

- [ ] **Step 3: Run existing DB tests to verify migration works**

Run: `cargo test -p conary-core db::tests`
Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/db/schema.rs conary-core/src/db/migrations.rs
git commit -m "feat(db): migration v57 adds output_equivalence table"
```

---

## Task 7: Chroot Pipeline Mode

**Files:**
- Modify: `conary-core/src/derivation/pipeline.rs`
- Reference: `conary-core/src/derivation/install.rs` (install_to_sysroot)
- Reference: `conary-core/src/derivation/build_order.rs` (BuildStep)
- Reference: `conary-core/src/derivation/environment.rs` (MutableEnvironment)

This is the largest task -- the core pipeline change.

- [ ] **Step 1: Add `BuildMode` enum to pipeline.rs**

```rust
/// How the pipeline executes builds.
#[derive(Debug, Clone)]
pub enum BuildMode {
    /// Original staged mode: EROFS between stages, read-only sysroot.
    Staged,
    /// Chroot mode: mutable overlayfs sysroot, install-as-you-go.
    Chroot,
}
```

Add `pub build_mode: BuildMode` to `PipelineConfig` with default `Staged`.

- [ ] **Step 2: Write `execute_chroot` method**

Model this after the existing `execute()` method in pipeline.rs (~lines 280-500). The key structural differences:
1. No outer per-stage loop -- single flat iteration over `build_steps`
2. No `BuildEnvironment` per-stage cycling -- one `MutableEnvironment` for the whole run
3. `build_env_hash` stays constant (seed hash) for all packages
4. After each build: `install_to_sysroot` + `run_ldconfig_if_needed`
5. `compose_erofs` called once at the end, not per-stage

```rust
impl Pipeline {
    pub async fn execute_chroot<F>(
        &self,
        seed: &Seed,
        recipes: &HashMap<String, Recipe>,
        build_steps: &[BuildStep],
        conn: &Connection,
        mut on_event: F,
    ) -> Result<BuildProfile, PipelineError>
    where
        F: FnMut(&PipelineEvent),
    {
        let build_env_hash = seed.build_env_hash().to_owned();

        // Mount mutable sysroot
        let mut mutable_env = super::environment::MutableEnvironment::new(
            seed.image_path.clone(),
            seed.cas_dir.clone(),
            self.config.work_dir.join("chroot"),
            build_env_hash.clone(),
        );
        if let Err(e) = mutable_env.mount() {
            warn!("Could not mount mutable environment (requires root): {e}");
        }
        let sysroot = mutable_env.sysroot();

        let mut completed: BTreeMap<String, (DerivationId, OutputManifest)> = BTreeMap::new();
        let mut all_manifests: Vec<OutputManifest> = Vec::new();
        let mut total_cached: usize = 0;
        let mut total_built: usize = 0;

        // Group steps by phase for progress reporting
        let mut current_phase: Option<super::build_order::BuildPhase> = None;
        let mut phase_idx = 0;
        let phase_counts = count_phases(build_steps);

        for step in build_steps {
            // Emit phase transition events
            if current_phase != Some(step.phase) {
                if current_phase.is_some() {
                    on_event(&PipelineEvent::StageCompleted {
                        name: current_phase.unwrap().to_string(),
                    });
                }
                current_phase = Some(step.phase);
                phase_idx = 0;
                on_event(&PipelineEvent::StageStarted {
                    name: step.phase.to_string(),
                    package_count: phase_counts[&step.phase],
                });
            }
            phase_idx += 1;

            let recipe = recipes
                .get(step.package.as_str())
                .ok_or_else(|| PipelineError::MissingRecipe(step.package.clone()))?;

            on_event(&PipelineEvent::PackageBuilding {
                name: step.package.clone(),
                stage: step.phase.to_string(),
            });

            // Collect dependency derivation IDs
            let dep_ids = collect_dep_ids(recipe, &completed);

            // Execute (cache check + build)
            let result = self.executor.execute(
                recipe,
                &build_env_hash,
                &dep_ids,
                &self.config.target_triple,
                &sysroot,
                conn,
            )?;

            let (manifest, was_cached) = match result {
                ExecutionResult::CacheHit { derivation_id, record } => {
                    let manifest = load_manifest_from_cas(
                        &self.executor, &record.manifest_cas_hash,
                    )?;
                    completed.insert(step.package.clone(), (derivation_id, manifest.clone()));
                    on_event(&PipelineEvent::PackageCached { name: step.package.clone() });
                    total_cached += 1;
                    (manifest, true)
                }
                ExecutionResult::Built { derivation_id, output } => {
                    completed.insert(
                        step.package.clone(),
                        (derivation_id, output.manifest.clone()),
                    );
                    on_event(&PipelineEvent::PackageBuilt {
                        name: step.package.clone(),
                        duration_secs: output.manifest.build_duration_secs,
                    });
                    total_built += 1;
                    (output.manifest, false)
                }
            };

            // Install into live chroot (for cached AND built packages)
            if let Err(e) = super::install::install_to_sysroot(
                &manifest, &sysroot, &self.config.cas_dir,
            ) {
                warn!("Install to sysroot failed for {}: {e}", step.package);
            }
            super::install::run_ldconfig_if_needed(&manifest, &sysroot);

            all_manifests.push(manifest);
        }

        // Emit final stage completed
        if let Some(phase) = current_phase {
            on_event(&PipelineEvent::StageCompleted { name: phase.to_string() });
        }

        // Compose final EROFS from ALL manifests
        let compose_dir = self.config.work_dir.join("final");
        std::fs::create_dir_all(&compose_dir)
            .map_err(|e| PipelineError::Io(e.to_string()))?;
        let manifest_refs: Vec<&OutputManifest> = all_manifests.iter().collect();
        compose_erofs(&manifest_refs, &compose_dir)?;

        on_event(&PipelineEvent::PipelineCompleted {
            total_packages: build_steps.len(),
            cached: total_cached,
            built: total_built,
        });

        // Build profile (simplified -- single "chroot" stage)
        // ... (follow existing generate_profile pattern)
        todo!("Build BuildProfile from completed map -- follow generate_profile pattern")
    }
}

/// Count packages per phase for progress reporting.
fn count_phases(steps: &[BuildStep]) -> HashMap<super::build_order::BuildPhase, usize> {
    let mut counts = HashMap::new();
    for step in steps {
        *counts.entry(step.phase).or_insert(0) += 1;
    }
    counts
}
```

Note to implementer: The `todo!()` at the bottom is just the `BuildProfile` assembly, which follows the existing `generate_profile` pattern exactly -- iterate completed packages, populate `ProfileDerivation` structs. See `execute()` lines ~470-510 for the template.

- [ ] **Step 4: Write test for `execute_chroot` with mock executor**

Since the full pipeline requires root (for mounts), write a unit test that validates the build order and install sequencing using the test helpers. The test should verify:
- Packages build in topological order
- `install_to_sysroot` is called after each build
- Cache hits also trigger install (for resume)
- The final compose_erofs receives all manifests

- [ ] **Step 5: Run test, verify passes**

Run: `cargo test -p conary-core pipeline::tests`
Expected: All pass.

- [ ] **Step 6: Clippy**

Run: `cargo clippy -p conary-core -- -D warnings`
Expected: Clean.

- [ ] **Step 7: Commit**

```bash
git add conary-core/src/derivation/pipeline.rs
git commit -m "feat(derivation): add chroot build mode to pipeline"
```

---

## Task 8: Adopted Seed Builder

**Files:**
- Create: `conary-core/src/bootstrap/adopt_seed.rs`
- Modify: `conary-core/src/bootstrap/mod.rs`

- [ ] **Step 1: Write `build_adopted_seed` function**

```rust
// conary-core/src/bootstrap/adopt_seed.rs

//! Create a bootstrap seed from an adopted system's filesystem.

use std::path::Path;

use crate::derivation::seed::{SeedMetadata, SeedSource, SeedValidation};

#[derive(Debug, thiserror::Error)]
pub enum AdoptSeedError {
    #[error("seed validation failed, missing: {0:?}")]
    ValidationFailed(Vec<&'static str>),
    #[error("EROFS build failed: {0}")]
    ErofsBuild(String),
    #[error("I/O error: {0}")]
    Io(String),
}

/// Build a seed EROFS from the system's root filesystem.
///
/// 1. Validate the system has required build tools
/// 2. Build EROFS image from /usr, /bin, /lib, /sbin, /etc
/// 3. Write seed.toml metadata
pub fn build_adopted_seed(
    output_dir: &Path,
    distro_name: &str,
    distro_version: &str,
) -> Result<SeedMetadata, AdoptSeedError> {
    use crate::derivation::compose::erofs_image_hash;

    // Validate system has build tools
    let validation = SeedValidation::probe(Path::new("/"));
    if !validation.is_valid() {
        return Err(AdoptSeedError::ValidationFailed(validation.missing_tools()));
    }

    std::fs::create_dir_all(output_dir)
        .map_err(|e| AdoptSeedError::Io(e.to_string()))?;

    let image_path = output_dir.join("seed.erofs");

    // Build EROFS image from system paths using mkfs.erofs.
    // Include /usr (binaries, libs, headers), /bin, /lib, /sbin (compat symlinks),
    // and /etc (ld.so.conf, passwd, group -- needed for chroot).
    let status = std::process::Command::new("mkfs.erofs")
        .arg(&image_path)
        .arg("/usr")
        .arg("/bin")
        .arg("/lib")
        .arg("/sbin")
        .arg("/etc")
        .status()
        .map_err(|e| AdoptSeedError::ErofsBuild(format!("mkfs.erofs: {e}")))?;

    if !status.success() {
        return Err(AdoptSeedError::ErofsBuild("mkfs.erofs exited non-zero".into()));
    }

    // Hash the image
    let seed_id = erofs_image_hash(&image_path)
        .map_err(|e| AdoptSeedError::Io(e.to_string()))?;

    // Write seed.toml
    let meta = SeedMetadata {
        seed_id: seed_id.clone(),
        source: SeedSource::Adopted,
        origin_url: None,
        builder: Some("conary-bootstrap".into()),
        packages: vec![], // Could populate from adopt DB, but not required
        target_triple: format!("{}-unknown-linux-gnu", std::env::consts::ARCH),
        verified_by: vec![],
        origin_distro: Some(distro_name.into()),
        origin_version: Some(distro_version.into()),
    };

    let toml_str = toml::to_string_pretty(&meta)
        .map_err(|e| AdoptSeedError::Io(e.to_string()))?;
    std::fs::write(output_dir.join("seed.toml"), toml_str)
        .map_err(|e| AdoptSeedError::Io(e.to_string()))?;

    Ok(meta)
}
```

- [ ] **Step 2: Register in `bootstrap/mod.rs`**

Add `pub mod adopt_seed;`

- [ ] **Step 3: Write unit test for validation failure path**

```rust
#[test]
fn test_adopt_seed_validates_tools() {
    // Test that build_adopted_seed returns ValidationFailed
    // when system tools are missing.
    // (This test validates the error path, not the happy path
    //  which requires root and mkfs.erofs)
}
```

- [ ] **Step 4: Clippy, commit**

```bash
git add conary-core/src/bootstrap/adopt_seed.rs conary-core/src/bootstrap/mod.rs
git commit -m "feat(bootstrap): add adopted seed builder"
```

---

## Task 9: Convergence Verification

**Files:**
- Create: `conary-core/src/derivation/convergence.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write failing test for `compare_builds`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_builds_detects_match_and_mismatch() {
        let a = vec![
            PackageComparison { package: "gcc".into(), hash_a: "aaa".into(), hash_b: "aaa".into() },
            PackageComparison { package: "python".into(), hash_a: "bbb".into(), hash_b: "ccc".into() },
        ];
        let report = ConvergenceReport::from_comparisons(a);
        assert_eq!(report.matched, 1);
        assert_eq!(report.mismatched, 1);
        assert_eq!(report.total, 2);
        assert!(!report.is_fully_converged());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL -- module doesn't exist.

- [ ] **Step 3: Implement convergence types**

```rust
// conary-core/src/derivation/convergence.rs

//! Cross-seed convergence verification.

/// Per-package comparison between two seeds.
#[derive(Debug, Clone)]
pub struct PackageComparison {
    pub package: String,
    pub hash_a: String,
    pub hash_b: String,
}

impl PackageComparison {
    pub fn matches(&self) -> bool { self.hash_a == self.hash_b }
}

/// Summary of convergence across all packages.
#[derive(Debug)]
pub struct ConvergenceReport {
    pub total: usize,
    pub matched: usize,
    pub mismatched: usize,
    pub comparisons: Vec<PackageComparison>,
}

impl ConvergenceReport {
    pub fn from_comparisons(comparisons: Vec<PackageComparison>) -> Self {
        let matched = comparisons.iter().filter(|c| c.matches()).count();
        let total = comparisons.len();
        Self { total, matched, mismatched: total - matched, comparisons }
    }

    pub fn is_fully_converged(&self) -> bool { self.mismatched == 0 }

    pub fn convergence_pct(&self) -> f64 {
        if self.total == 0 { 100.0 }
        else { (self.matched as f64 / self.total as f64) * 100.0 }
    }

    pub fn mismatches(&self) -> Vec<&PackageComparison> {
        self.comparisons.iter().filter(|c| !c.matches()).collect()
    }
}
```

- [ ] **Step 4: Register module, run test**

Add `pub mod convergence;` to `mod.rs`.

Run: `cargo test -p conary-core convergence::tests`
Expected: PASS

- [ ] **Step 5: Write `compare_seed_builds` function**

```rust
/// Compare output hashes from two seeds using the derivation index.
///
/// Identifies builds from each seed via `build_env_hash` (the seed's SHA-256,
/// which is stored as the `build_env_hash` for packages in the first stage).
pub fn compare_seed_builds(
    conn: &rusqlite::Connection,
    seed_a_id: &str,
    seed_b_id: &str,
) -> crate::error::Result<ConvergenceReport> {
    // Query packages built with seed A (build_env_hash = seed_a_id)
    let mut stmt = conn.prepare(
        "SELECT package_name, output_hash FROM derivation_index WHERE build_env_hash = ?1"
    )?;
    let seed_a: std::collections::HashMap<String, String> = stmt
        .query_map([seed_a_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let seed_b: std::collections::HashMap<String, String> = stmt
        .query_map([seed_b_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    // Match by package name, compare output hashes
    let all_packages: std::collections::BTreeSet<&str> = seed_a.keys()
        .chain(seed_b.keys())
        .map(|s| s.as_str())
        .collect();

    let comparisons: Vec<PackageComparison> = all_packages
        .into_iter()
        .filter_map(|pkg| {
            let hash_a = seed_a.get(pkg)?;
            let hash_b = seed_b.get(pkg)?;
            Some(PackageComparison {
                package: pkg.to_string(),
                hash_a: hash_a.clone(),
                hash_b: hash_b.clone(),
            })
        })
        .collect();

    Ok(ConvergenceReport::from_comparisons(comparisons))
}
```

- [ ] **Step 6: Clippy, commit**

```bash
git add conary-core/src/derivation/convergence.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add convergence verification module"
```

---

## Task 10: CLI Wiring + Integration

**Files:**
- Modify: `src/cli/bootstrap.rs` (CLI definitions -- clap `BootstrapCommands` enum)
- Modify: `src/commands/bootstrap.rs` (command handlers)

- [ ] **Step 1: Add `--from-adopted` flag and `--mode` flag to existing variants**

In `src/cli/bootstrap.rs`, modify the `Seed` variant:
```rust
Seed {
    /// Cross-tools directory to package
    #[arg(long, required_unless_present = "from_adopted")]
    from: Option<String>,

    /// Create seed from current adopted system
    #[arg(long)]
    from_adopted: bool,

    /// Distro name (for --from-adopted)
    #[arg(long, requires = "from_adopted")]
    distro: Option<String>,

    /// Distro version (for --from-adopted)
    #[arg(long, requires = "from_adopted")]
    distro_version: Option<String>,

    /// Output seed directory
    #[arg(short, long)]
    output: String,

    /// Target triple
    #[arg(long, default_value = "x86_64-conary-linux-gnu")]
    target: String,
},
```

Add `--mode` to the `Run` variant:
```rust
Run {
    // ... existing args ...

    /// Build mode: chroot (mutable sysroot) or staged (EROFS per stage)
    #[arg(long, default_value = "chroot")]
    mode: String,
},
```

Add new subcommands:
```rust
/// Verify convergence between builds from two different seeds
#[command(name = "verify-convergence")]
VerifyConvergence {
    #[arg(long)]
    seed_a: String,
    #[arg(long)]
    seed_b: String,
    #[arg(long)]
    diff: bool,
},

/// Diff two seed images
#[command(name = "diff-seeds")]
DiffSeeds {
    path_a: String,
    path_b: String,
},
```

- [ ] **Step 2: Wire `seed --from-adopted` to `adopt_seed::build_adopted_seed`**

In the command handler, check `from_adopted` flag. If set, call `build_adopted_seed(output_dir, distro, version)`. Otherwise, use existing `--from` path.

- [ ] **Step 3: Wire `run --mode chroot` to `Pipeline::execute_chroot`**

Modify the existing `bootstrap run` handler:
- Load seed (existing)
- If `mode == "chroot"`: call `compute_build_order` then `execute_chroot`
- If `mode == "staged"`: call `assign_stages` then `execute` (existing behavior)

- [ ] **Step 4: Wire `verify-convergence` to `convergence::compare_seed_builds`**

- [ ] **Step 5: Wire `diff-seeds`**

Basic implementation: mount both seeds read-only, walk file trees, compare. Can use existing `BuildEnvironment::mount` for each seed, then `diff -r` the mount points, or walk with Rust's `walkdir` and compare hashes.

- [ ] **Step 6: Run `cargo build` to verify everything compiles**

Run: `cargo build`
Expected: Clean build.

- [ ] **Step 7: Run full test suite**

Run: `cargo test`
Expected: All pass (existing + new tests).

- [ ] **Step 8: Commit**

```bash
git add src/cli/bootstrap.rs src/commands/bootstrap.rs
git commit -m "feat(cli): wire bootstrap chroot mode, adopted seed, and convergence CLI"
```

---

## Task 11: Remove Staged Workarounds

Only after all new code is working and tested.

**Files:**
- Modify: `conary-core/src/derivation/stages.rs` (deprecation notice)
- Modify: recipes if needed (revert Python workarounds)

- [ ] **Step 1: Add deprecation doc comment to `assign_stages`**

```rust
/// # Deprecated
///
/// Use [`build_order::compute_build_order`] for new bootstrap builds.
/// This function is retained for backward compatibility with the staged pipeline mode.
#[deprecated(note = "Use compute_build_order for chroot mode")]
pub fn assign_stages(...) -> ... {
```

- [ ] **Step 2: Revert Python PGO disable if recipe exists**

Check `recipes/python/recipe.toml` -- if it has `pgo = false` or `-Werror` stripping, add a comment noting these can be removed once chroot mode is verified working.

- [ ] **Step 3: Run full test suite**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: All pass, deprecation warnings only on `assign_stages` usage.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(derivation): deprecate assign_stages, note Python workaround removal"
```

---

## Execution Order and Dependencies

```
Task 1 (build_order) ─────────────────────────────┐
Task 2 (install) ──────────────────────────────────┤
Task 3 (mutable environment) ──────────────────────┤── Task 7 (chroot pipeline) ── Task 10 (CLI) ── Task 11 (cleanup)
Task 4 (seed abstraction) ─────────────────────────┤
Task 5 (output hash v2) ── Task 6 (DB migration) ─┘
Task 9 (convergence) ─────────────────────────────── Task 10 (CLI)
Task 8 (adopted seed) ────────────────────────────── Task 10 (CLI)
```

**Parallelizable:** Tasks 1-6 and 8-9 are independent. Task 7 depends on 1-6. Task 10 depends on 7-9. Task 11 is last.

**Estimated commits:** 11 (one per task).
