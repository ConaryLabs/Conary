# Bootstrap Build Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the bootstrap recipe execution so `conary bootstrap cross-tools` actually compiles binutils, GCC, linux-headers, glibc, and libstdc++ from source using the existing Kitchen/Cook engine.

**Architecture:** Phase 1/2a builds use Kitchen's Cook with an explicit dest_dir set to `$LFS` (skip CCS packaging). Phase 2b/3 builds assemble shell scripts from recipes and execute via `chroot` with `env_clear()`. A `ChrootEnv` struct manages mount lifecycle with `Drop`-based teardown.

**Tech Stack:** Rust 1.94, std::process::Command, rusqlite (stage checkpoints), conary-core recipe/kitchen

**Spec:** `docs/superpowers/specs/2026-03-19-bootstrap-build-pipeline.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `conary-core/src/recipe/kitchen/cook.rs` | Modify | Make `dest_dir`/`build_dir` flexible, add `new_with_dest()` |
| `conary-core/src/recipe/kitchen/mod.rs` | Modify | Add `Kitchen::new_cook_with_dest()` |
| `conary-core/src/bootstrap/chroot_env.rs` | Create | ChrootEnv mount lifecycle manager |
| `conary-core/src/bootstrap/mod.rs` | Modify | Register chroot_env, add `assemble_build_script()` |
| `conary-core/src/bootstrap/cross_tools.rs` | Modify | Wire `build_package()` to Kitchen |
| `conary-core/src/bootstrap/temp_tools.rs` | Modify | Wire 3 stub methods |
| `conary-core/src/bootstrap/final_system.rs` | Modify | Wire `build_package()` to chroot |

---

### Task 1: Make Cook Support External dest_dir

**Files:**
- Modify: `conary-core/src/recipe/kitchen/cook.rs`
- Modify: `conary-core/src/recipe/kitchen/mod.rs`

The `Cook` struct currently creates a `TempDir` and derives `dest_dir` from it. We need a variant where the caller provides the dest_dir (e.g., `$LFS`) and the temp dir is only used for the build tree, not the install target.

- [ ] **Step 1: Write test for `new_cook_with_dest`**

Add to `conary-core/src/recipe/kitchen/mod.rs` test module:

```rust
#[test]
fn test_new_cook_with_dest_uses_provided_dir() {
    let kitchen = Kitchen::with_defaults();
    let recipe_toml = r#"
[package]
name = "test"
version = "1.0"
release = "1"
summary = "test"
license = "MIT"

[source]
archive = "https://example.com/test-1.0.tar.gz"
checksum = "sha256:0000000000000000000000000000000000000000000000000000000000000000"

[build]
install = "echo installed"
"#;
    let recipe = crate::recipe::parse_recipe(recipe_toml).unwrap();
    let dest = tempfile::tempdir().unwrap();
    let cook = kitchen.new_cook_with_dest(&recipe, dest.path()).unwrap();
    assert_eq!(cook.dest_dir, dest.path());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_new_cook_with_dest -- --nocapture`
Expected: FAIL — `new_cook_with_dest` not defined

- [ ] **Step 3: Add `_build_dir_owner` field to Cook struct**

In `cook.rs`, change the `Cook` struct to own the TempDir optionally:

```rust
pub struct Cook<'a> {
    pub(super) kitchen: &'a Kitchen,
    pub(super) recipe: &'a Recipe,
    /// Temporary build directory (owned when internally created)
    _build_dir_owner: Option<TempDir>,
    /// Build directory path (always valid)
    pub(super) build_dir: PathBuf,
    /// Source directory within build_dir
    pub(super) source_dir: PathBuf,
    /// Destination directory (where files get installed)
    pub(super) dest_dir: PathBuf,
    /// Build log accumulator
    pub(super) log: String,
    /// Warnings
    pub(super) warnings: Vec<String>,
    /// Provenance capture for this build
    pub(super) provenance: ProvenanceCapture,
}
```

Update `Cook::new()` to set `_build_dir_owner = Some(temp_dir)` and `build_dir = temp_dir.path().to_path_buf()`.

Update all uses of `self.build_dir.path()` to `&self.build_dir` (it's now a `PathBuf`, not a `TempDir`).

- [ ] **Step 4: Add `Cook::new_with_dest()` constructor**

```rust
/// Create a Cook with a caller-provided destination directory.
///
/// Used by bootstrap builds where files install directly to $LFS
/// instead of a temporary staging area. A TempDir is still created
/// for the build tree (source extraction, object files).
pub(crate) fn new_with_dest(
    kitchen: &'a Kitchen,
    recipe: &'a Recipe,
    dest_dir: &Path,
) -> Result<Self> {
    let build_dir = TempDir::new()
        .map_err(|e| Error::IoError(format!("Failed to create build directory: {}", e)))?;

    let build_path = build_dir.path().to_path_buf();
    let source_dir = build_path.join("source");

    fs::create_dir_all(&source_dir)?;
    fs::create_dir_all(dest_dir)?;

    let mut provenance = ProvenanceCapture::new();
    for dep in &recipe.build.makedepends {
        provenance.add_build_dep(dep, "unknown", None);
    }

    Ok(Self {
        kitchen,
        recipe,
        _build_dir_owner: Some(build_dir),
        build_dir: build_path,
        source_dir,
        dest_dir: dest_dir.to_path_buf(),
        log: String::new(),
        warnings: Vec::new(),
        provenance,
    })
}
```

- [ ] **Step 5: Add `Kitchen::new_cook_with_dest()` in mod.rs**

```rust
/// Create a Cook that installs to an external destination directory.
///
/// This is used by bootstrap phases where files should be installed
/// directly to `$LFS` rather than a temporary staging directory.
/// The plate (CCS packaging) phase is skipped by the caller.
pub fn new_cook_with_dest<'a>(
    &'a self,
    recipe: &'a Recipe,
    dest_dir: &Path,
) -> Result<Cook<'a>> {
    Cook::new_with_dest(self, recipe, dest_dir)
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p conary-core test_new_cook_with_dest -- --nocapture`
Expected: PASS

- [ ] **Step 7: Run full test suite**

Run: `cargo test -p conary-core -- --nocapture 2>&1 | tail -5`
Expected: All existing tests still pass

- [ ] **Step 8: Commit**

```bash
git add conary-core/src/recipe/kitchen/cook.rs conary-core/src/recipe/kitchen/mod.rs
git commit -m "feat(recipe): add Cook::new_with_dest for bootstrap builds with external dest_dir"
```

---

### Task 2: Create ChrootEnv Mount Manager

**Files:**
- Create: `conary-core/src/bootstrap/chroot_env.rs`
- Modify: `conary-core/src/bootstrap/mod.rs`

- [ ] **Step 1: Write test for ChrootEnv directory creation**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chroot_env_creates_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let lfs = tmp.path().join("lfs");
        std::fs::create_dir_all(&lfs).unwrap();

        // setup() will fail on mounts (not root in tests) but dirs should be created
        let mut env = ChrootEnv::new(&lfs);
        let _ = env.setup(); // Ignore mount errors in test

        assert!(lfs.join("dev").exists());
        assert!(lfs.join("proc").exists());
        assert!(lfs.join("sys").exists());
        assert!(lfs.join("run").exists());
        assert!(lfs.join("usr/bin").exists());
        assert!(lfs.join("usr/lib").exists());
        assert!(lfs.join("usr/sbin").exists());
    }

    #[test]
    fn test_chroot_env_teardown_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let lfs = tmp.path().join("lfs");
        std::fs::create_dir_all(&lfs).unwrap();

        let mut env = ChrootEnv::new(&lfs);
        // No mounts succeeded, teardown should not panic
        env.teardown();
        env.teardown(); // Second call should be safe
    }
}
```

- [ ] **Step 2: Implement `ChrootEnv`**

Create `conary-core/src/bootstrap/chroot_env.rs`:

```rust
// conary-core/src/bootstrap/chroot_env.rs

//! Chroot environment setup and teardown for LFS bootstrap builds.
//!
//! Manages the virtual kernel filesystem mounts required by LFS Chapters 7-8.
//! Uses a mount tracking vector for safe partial teardown on error or panic.

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

/// Manages chroot mount lifecycle for LFS bootstrap builds.
///
/// Tracks which mounts succeeded so teardown only unmounts what was actually
/// mounted. The `Drop` impl ensures cleanup even on panic.
pub struct ChrootEnv {
    lfs_root: PathBuf,
    /// Mounts that succeeded, in order. Teardown reverses this.
    mounted: Vec<PathBuf>,
}

impl ChrootEnv {
    pub fn new(lfs_root: &Path) -> Self {
        Self {
            lfs_root: lfs_root.to_path_buf(),
            mounted: Vec::new(),
        }
    }

    /// Create directory structure and mount virtual filesystems.
    ///
    /// Follows LFS 13 Chapter 7.3-7.4. If a mount fails, previously
    /// mounted filesystems are tracked and will be cleaned up by `Drop`.
    pub fn setup(&mut self) -> anyhow::Result<()> {
        let lfs = &self.lfs_root;

        // Create directory hierarchy
        for dir in &[
            "dev", "proc", "sys", "run",
            "etc", "home", "mnt", "opt", "srv",
            "usr/bin", "usr/lib", "usr/sbin",
            "var/log", "var/mail", "var/spool",
        ] {
            std::fs::create_dir_all(lfs.join(dir))?;
        }

        // Create compatibility symlinks (LFS uses merged /usr)
        for (link, target) in &[
            ("bin", "usr/bin"),
            ("lib", "usr/lib"),
            ("sbin", "usr/sbin"),
            ("lib64", "usr/lib"),
        ] {
            let link_path = lfs.join(link);
            if !link_path.exists() {
                std::os::unix::fs::symlink(target, &link_path)?;
            }
        }

        // Mount virtual kernel filesystems
        self.mount_bind("/dev", &lfs.join("dev"))?;
        self.mount_fs("devpts", &lfs.join("dev/pts"), "devpts", "gid=5,mode=0620")?;
        self.mount_fs("proc", &lfs.join("proc"), "proc", "")?;
        self.mount_fs("sysfs", &lfs.join("sys"), "sysfs", "")?;
        self.mount_fs("tmpfs", &lfs.join("run"), "tmpfs", "")?;

        info!("Chroot environment ready at {}", lfs.display());
        Ok(())
    }

    fn mount_bind(&mut self, src: &str, dest: &Path) -> anyhow::Result<()> {
        let status = Command::new("mount")
            .args(["--bind", src, &dest.to_string_lossy()])
            .status()?;
        if status.success() {
            self.mounted.push(dest.to_path_buf());
            Ok(())
        } else {
            anyhow::bail!("mount --bind {} {} failed", src, dest.display());
        }
    }

    fn mount_fs(&mut self, dev: &str, dest: &Path, fstype: &str, opts: &str) -> anyhow::Result<()> {
        let mut cmd = Command::new("mount");
        cmd.arg("-t").arg(fstype);
        if !opts.is_empty() {
            cmd.arg("-o").arg(opts);
        }
        cmd.arg(dev).arg(&dest.to_string_lossy().to_string());

        let status = cmd.status()?;
        if status.success() {
            self.mounted.push(dest.to_path_buf());
            Ok(())
        } else {
            anyhow::bail!("mount -t {} {} {} failed", fstype, dev, dest.display());
        }
    }

    /// Unmount all tracked mounts in reverse order. Best-effort: errors are logged.
    pub fn teardown(&mut self) {
        while let Some(mount_point) = self.mounted.pop() {
            let result = Command::new("umount")
                .args(["--lazy", &mount_point.to_string_lossy()])
                .status();
            match result {
                Ok(status) if status.success() => {
                    info!("Unmounted {}", mount_point.display());
                }
                Ok(status) => {
                    warn!("umount {} exited with {}", mount_point.display(), status);
                }
                Err(e) => {
                    warn!("Failed to run umount for {}: {}", mount_point.display(), e);
                }
            }
        }
    }
}

impl Drop for ChrootEnv {
    fn drop(&mut self) {
        if !self.mounted.is_empty() {
            warn!("ChrootEnv dropped with {} active mounts, cleaning up", self.mounted.len());
            self.teardown();
        }
    }
}
```

- [ ] **Step 3: Register module in `bootstrap/mod.rs`**

Add `pub mod chroot_env;` to the module declarations in `conary-core/src/bootstrap/mod.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p conary-core chroot_env -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/bootstrap/chroot_env.rs conary-core/src/bootstrap/mod.rs
git commit -m "feat(bootstrap): add ChrootEnv mount lifecycle manager with Drop safety"
```

---

### Task 3: Add `assemble_build_script()` Helper

**Files:**
- Modify: `conary-core/src/bootstrap/mod.rs`

- [ ] **Step 1: Write test**

```rust
#[test]
fn test_assemble_build_script() {
    let toml = r#"
[package]
name = "test-pkg"
version = "1.0"
release = "1"
summary = "test"
license = "MIT"

[source]
archive = "https://example.com/test-1.0.tar.gz"
checksum = "sha256:0000000000000000000000000000000000000000000000000000000000000000"

[build]
configure = "./configure --prefix=/usr"
make = "make -j%(jobs)s"
install = "make DESTDIR=%(destdir)s install"

[variables]
jobs = "4"
"#;
    let recipe = crate::recipe::parse_recipe(toml).unwrap();
    let script = assemble_build_script(&recipe, "/");
    assert!(script.contains("set -e"));
    assert!(script.contains("./configure --prefix=/usr"));
    assert!(script.contains("make -j4"));
    assert!(script.contains("make DESTDIR=/ install"));
}
```

- [ ] **Step 2: Implement `assemble_build_script()`**

Add to `conary-core/src/bootstrap/mod.rs`:

```rust
/// Assemble a build script from recipe fields with variable substitution.
///
/// Used by chroot builds (Phase 2b and 3) where the Kitchen cannot run
/// directly. Each build phase (setup, configure, make, install, post_install)
/// is concatenated into a single `set -e` script.
pub fn assemble_build_script(recipe: &crate::recipe::format::Recipe, destdir: &str) -> String {
    let mut script = String::from("set -e\n");
    let phases = [
        &recipe.build.setup,
        &recipe.build.configure,
        &recipe.build.make,
        &recipe.build.install,
        &recipe.build.post_install,
    ];
    for phase in phases {
        if let Some(ref cmd) = phase {
            let substituted = recipe.substitute(cmd, destdir);
            script.push_str(&substituted);
            script.push('\n');
        }
    }
    script
}
```

- [ ] **Step 3: Run test**

Run: `cargo test -p conary-core test_assemble_build_script -- --nocapture`

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/bootstrap/mod.rs
git commit -m "feat(bootstrap): add assemble_build_script() for chroot builds"
```

---

### Task 4: Wire `cross_tools.rs::build_package()`

**Files:**
- Modify: `conary-core/src/bootstrap/cross_tools.rs`

- [ ] **Step 1: Rename `_runner` field to `runner`**

In the `CrossToolsBuilder` struct (line 80), change `_runner: PackageBuildRunner` to `runner: PackageBuildRunner`. Update the constructor at line 119 from `_runner: runner` to `runner`.

- [ ] **Step 2: Add recipe imports**

Add to the imports at the top of the file:

```rust
use crate::recipe::parser::parse_recipe_file;
use crate::recipe::kitchen::{Kitchen, KitchenConfig};
```

- [ ] **Step 3: Replace the `build_package()` stub**

Replace the placeholder at lines 168-177 with:

```rust
fn build_package(&self, name: &str) -> Result<(), CrossToolsError> {
    // Locate recipe — cross-tools recipes live alongside the source tree
    let recipe_path = Path::new("recipes/cross-tools").join(format!("{name}.toml"));
    if !recipe_path.exists() {
        return Err(CrossToolsError::BuildFailed {
            package: name.to_string(),
            reason: format!("Recipe not found: {}", recipe_path.display()),
        });
    }

    let recipe = parse_recipe_file(&recipe_path).map_err(|e| {
        CrossToolsError::BuildFailed {
            package: name.to_string(),
            reason: format!("Failed to parse recipe: {e}"),
        }
    })?;

    // Fetch source to cache
    info!("  Fetching source for {name}...");
    self.runner.fetch_source(name, &recipe).map_err(|e| {
        CrossToolsError::BuildFailed {
            package: name.to_string(),
            reason: format!("Source fetch failed: {e}"),
        }
    })?;

    // Build using Kitchen with $LFS as dest_dir
    let config = KitchenConfig {
        source_cache: self.work_dir.join("sources"),
        jobs: self.config.jobs as u32,
        use_isolation: false,
        ..Default::default()
    };
    let kitchen = Kitchen::new(config);
    let mut cook = kitchen.new_cook_with_dest(&recipe, &self.lfs_root).map_err(
        |e| CrossToolsError::BuildFailed {
            package: name.to_string(),
            reason: format!("Cook setup failed: {e}"),
        },
    )?;

    info!("  Preparing source for {name}...");
    cook.prep().map_err(|e| CrossToolsError::BuildFailed {
        package: name.to_string(),
        reason: format!("Prep failed: {e}"),
    })?;

    cook.unpack().map_err(|e| CrossToolsError::BuildFailed {
        package: name.to_string(),
        reason: format!("Unpack failed: {e}"),
    })?;

    cook.patch().map_err(|e| CrossToolsError::BuildFailed {
        package: name.to_string(),
        reason: format!("Patch failed: {e}"),
    })?;

    info!("  Building {name}...");
    cook.simmer().map_err(|e| CrossToolsError::BuildFailed {
        package: name.to_string(),
        reason: format!("Build failed: {e}"),
    })?;

    info!("  [OK] {name} built successfully");
    Ok(())
}
```

- [ ] **Step 4: Add per-package checkpoint to `build_all()`**

In `build_all()`, wrap the package loop with checkpoint logic. The function currently has access to `self` but not the stage manager. We need to accept a mutable stage manager or return package names for the caller to checkpoint. The simpler approach: have `build_all` return which packages were built, and let the caller (`BootstrapPipeline::build_cross_tools` in `mod.rs`) do the checkpointing.

Actually, looking at the current code, `build_all()` is called from `BootstrapPipeline::build_cross_tools()` in `mod.rs` which has `&mut self` with access to `self.stages`. So add checkpoint calls there instead. No changes needed to `build_all()` — just document that the caller should checkpoint.

But we DO need to add a `skip_packages` parameter or similar so `build_all()` can skip already-completed packages on resume. Add this:

```rust
pub fn build_all(&self, completed: &[String]) -> Result<Toolchain, CrossToolsError> {
    // ... existing logging ...

    for (i, pkg) in CROSS_TOOLS_ORDER.iter().enumerate() {
        if completed.contains(&pkg.to_string()) {
            info!("Skipping already-completed: {}", pkg);
            continue;
        }
        info!(
            "Building cross-tool [{}/{}]: {}",
            i + 1, CROSS_TOOLS_ORDER.len(), pkg
        );
        self.build_package(pkg)?;
    }

    // ... rest unchanged ...
}
```

Update the caller in `mod.rs` (`build_cross_tools`) to pass completed packages and checkpoint after each.

- [ ] **Step 5: Add bootstrap environment variables**

Before building, set LFS environment variables. Add a helper to `CrossToolsBuilder`:

```rust
fn bootstrap_env(&self) -> Vec<(String, String)> {
    vec![
        ("LFS".into(), self.lfs_root.to_string_lossy().to_string()),
        ("LFS_TGT".into(), LFS_TGT.to_string()),
        ("LC_ALL".into(), "C".into()),
        ("TZ".into(), "UTC".into()),
        ("SOURCE_DATE_EPOCH".into(), "0".into()),
        ("HOME".into(), "/root".into()),
        ("TERM".into(), "xterm".into()),
        ("MAKEFLAGS".into(), format!("-j{}", self.config.jobs)),
    ]
}
```

These need to be set before `cook.simmer()` runs. Add them to the recipe's `build.environment` or pass them through KitchenConfig. The simplest: set them as process env before cooking, since Phase 1 runs direct (no isolation).

- [ ] **Step 6: Build and test**

Run: `cargo build -p conary-core`
Expected: Compiles without errors

- [ ] **Step 7: Commit**

```bash
git add conary-core/src/bootstrap/cross_tools.rs
git commit -m "feat(bootstrap): wire cross_tools build_package to Kitchen/Cook execution"
```

---

### Task 5: Wire `temp_tools.rs` Three Methods

**Files:**
- Modify: `conary-core/src/bootstrap/temp_tools.rs`

- [ ] **Step 1: Rename `_runner` field to `runner`**

Same pattern as cross_tools — rename the field and constructor.

- [ ] **Step 2: Wire `build_cross_packages()`**

Replace the stub with recipe-driven builds using the Kitchen, same pattern as cross_tools but with PATH including `$LFS/tools/bin`:

```rust
pub fn build_cross_packages(&self, completed: &[String]) -> Result<(), TempToolsError> {
    for (i, pkg) in CH6_PACKAGES.iter().enumerate() {
        if completed.contains(&pkg.to_string()) {
            info!("Skipping already-completed: {}", pkg);
            continue;
        }
        info!("Cross-compiling [{}/{}]: {}", i + 1, CH6_PACKAGES.len(), pkg);

        let recipe_path = Path::new("recipes/temp-tools").join(format!("{pkg}.toml"));
        let recipe = parse_recipe_file(&recipe_path).map_err(|e| /* ... */)?;
        self.runner.fetch_source(pkg, &recipe).map_err(|e| /* ... */)?;

        let config = KitchenConfig {
            source_cache: self.work_dir.join("sources"),
            jobs: self.config.jobs as u32,
            use_isolation: false,
            ..Default::default()
        };
        let kitchen = Kitchen::new(config);
        let mut cook = kitchen.new_cook_with_dest(&recipe, &self.lfs_root).map_err(|e| /* ... */)?;
        cook.prep()?;
        cook.unpack()?;
        cook.patch()?;
        cook.simmer()?;
    }
    Ok(())
}
```

- [ ] **Step 3: Wire `setup_chroot()`**

Replace the stub with a call to `ChrootEnv`:

```rust
pub fn setup_chroot(&self) -> Result<ChrootEnv, TempToolsError> {
    let mut env = ChrootEnv::new(&self.lfs_root);
    env.setup().map_err(|e| TempToolsError::ChrootSetup(e.to_string()))?;
    Ok(env)
}
```

Add `ChrootSetup(String)` variant to `TempToolsError` if not already present.

- [ ] **Step 4: Wire `build_chroot_packages()`**

Replace with chroot-based execution:

```rust
pub fn build_chroot_packages(&self, completed: &[String]) -> Result<(), TempToolsError> {
    for (i, pkg) in CH7_PACKAGES.iter().enumerate() {
        if completed.contains(&pkg.to_string()) {
            info!("Skipping already-completed: {}", pkg);
            continue;
        }
        info!("Building in chroot [{}/{}]: {}", i + 1, CH7_PACKAGES.len(), pkg);

        let recipe_path = Path::new("recipes/temp-tools").join(format!("{pkg}.toml"));
        let recipe = parse_recipe_file(&recipe_path).map_err(|e| /* ... */)?;

        // Fetch source to $LFS/sources/ (accessible inside chroot)
        self.runner.fetch_source(pkg, &recipe).map_err(|e| /* ... */)?;

        // Assemble and run in chroot
        let script = super::assemble_build_script(&recipe, "/");
        let env = self.chroot_env();

        let output = std::process::Command::new("chroot")
            .arg(&self.lfs_root)
            .arg("/bin/sh")
            .arg("-c")
            .arg(&script)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .output()
            .map_err(|e| /* ... */)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TempToolsError::BuildFailed {
                package: pkg.to_string(),
                reason: stderr.to_string()
            });
        }
    }
    Ok(())
}

fn chroot_env(&self) -> Vec<(String, String)> {
    vec![
        ("PATH".into(), "/usr/bin:/usr/sbin".into()),
        ("HOME".into(), "/root".into()),
        ("TERM".into(), "xterm".into()),
        ("LC_ALL".into(), "C".into()),
        ("TZ".into(), "UTC".into()),
        ("SOURCE_DATE_EPOCH".into(), "0".into()),
        ("MAKEFLAGS".into(), format!("-j{}", self.config.jobs)),
        ("LFS_TGT".into(), self.cross_toolchain.target.clone()),
    ]
}
```

- [ ] **Step 5: Build**

Run: `cargo build -p conary-core`

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/bootstrap/temp_tools.rs
git commit -m "feat(bootstrap): wire temp_tools cross-compile, chroot setup, and chroot builds"
```

---

### Task 6: Wire `final_system.rs::build_package()`

**Files:**
- Modify: `conary-core/src/bootstrap/final_system.rs`

- [ ] **Step 1: Read the file and identify the stub**

Find `build_package()` or the equivalent build loop in `final_system.rs`.

- [ ] **Step 2: Wire to chroot execution**

Same pattern as `temp_tools::build_chroot_packages()` — load recipe from `recipes/system/`, assemble script, execute via `chroot` with `env_clear()`. Add `completed` parameter for checkpoint skip.

- [ ] **Step 3: Build**

Run: `cargo build -p conary-core`

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/bootstrap/final_system.rs
git commit -m "feat(bootstrap): wire final_system build to chroot execution"
```

---

### Task 7: Wire Checkpoints in `bootstrap/mod.rs`

**Files:**
- Modify: `conary-core/src/bootstrap/mod.rs`

The orchestrator methods (`build_cross_tools`, `build_temp_tools`, `build_final_system`) call the phase builders. They need to pass `completed_packages()` and call `mark_package_complete()` after each package.

- [ ] **Step 1: Update `build_cross_tools()`**

Read the current method. Update to pass completed packages and checkpoint:

```rust
pub fn build_cross_tools(&mut self) -> Result<Toolchain> {
    let completed = self.stages.completed_packages(BootstrapStage::CrossTools);
    // ... existing builder creation ...
    let toolchain = builder.build_all(&completed)?;
    // Mark each package that was built (not skipped) as complete
    for pkg in CROSS_TOOLS_ORDER {
        if !completed.contains(&pkg.to_string()) {
            self.stages.mark_package_complete(BootstrapStage::CrossTools, pkg)?;
        }
    }
    self.stages.mark_complete(BootstrapStage::CrossTools, &toolchain.path)?;
    Ok(toolchain)
}
```

- [ ] **Step 2: Update `build_temp_tools()` similarly**

- [ ] **Step 3: Update `build_final_system()` similarly**

- [ ] **Step 4: Build and run existing tests**

Run: `cargo build -p conary-core && cargo test -p conary-core bootstrap -- --nocapture`

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/bootstrap/mod.rs
git commit -m "feat(bootstrap): wire per-package checkpointing for resume support"
```

---

### Task 8: Integration Test — Phase 1 Build on Remi

This is the acceptance test. Run on Remi (12 cores, 64GB RAM).

- [ ] **Step 1: Deploy to Remi**

```bash
rsync -az --delete --exclude target --exclude .git ~/Conary/ root@ssh.conary.io:/root/conary-src/
ssh root@ssh.conary.io "source ~/.cargo/env && cd /root/conary-src && cargo build --release 2>&1 | tail -5"
ssh root@ssh.conary.io "systemctl stop remi && cp /root/conary-src/target/release/conary /usr/local/bin/conary && systemctl start remi"
```

- [ ] **Step 2: Clean and initialize bootstrap**

```bash
ssh root@ssh.conary.io "conary bootstrap clean --work-dir /conary/bootstrap"
ssh root@ssh.conary.io "conary bootstrap init --work-dir /conary/bootstrap --target x86_64"
```

- [ ] **Step 3: Run Phase 1**

```bash
ssh root@ssh.conary.io "nohup bash -c 'source ~/.cargo/env && conary bootstrap cross-tools --work-dir /conary/bootstrap -j 12 --skip-verify 2>&1 | tee /conary/bootstrap/logs/cross-tools.log' > /conary/bootstrap/logs/phase1.log 2>&1 &"
```

- [ ] **Step 4: Monitor progress**

```bash
ssh root@ssh.conary.io "tail -20 /conary/bootstrap/logs/phase1.log"
ssh root@ssh.conary.io "conary bootstrap status --work-dir /conary/bootstrap"
```

- [ ] **Step 5: Verify cross-toolchain**

```bash
ssh root@ssh.conary.io "/mnt/lfs/tools/bin/x86_64-conary-linux-gnu-gcc -v"
# Expected: gcc version 15.2.0
```

- [ ] **Step 6: Commit any fixes found during validation**

---

### Task 9: Final Build + Clippy

- [ ] **Step 1: Build both profiles**

```bash
cargo build
cargo build --features server
```

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -- -D warnings
```

- [ ] **Step 3: Run unit tests**

```bash
cargo test
```

- [ ] **Step 4: Fix any issues, commit**
