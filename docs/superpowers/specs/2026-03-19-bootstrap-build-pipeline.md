# Bootstrap Build Pipeline: Wiring Recipe Execution to LFS Phases

## Problem

The bootstrap pipeline has full scaffolding (phase orchestration, stage tracking,
114 LFS 13-aligned recipes with configure/make/install blocks, source fetching
with checksum verification, a working recipe execution engine) but the
`build_package()` functions in each phase are stubs. The Kitchen/Cook system in
`recipe/kitchen/cook.rs` can execute shell commands via `Command::new("sh")` —
it just isn't called from the bootstrap phases.

## Goal

Wire the four stub functions to the existing Kitchen/Cook execution engine so
that `conary bootstrap cross-tools` actually compiles binutils, GCC, glibc, etc.
Validate by building Phase 1 (5 cross-tool packages) end-to-end on Remi.

## Architecture

Two execution modes, chosen by phase:

**Phase 1 and 2a (Direct):** Load recipe, fetch source via `BuildRunner`,
construct a `Cook` instance with `dest_dir` set to `$LFS`, call `prep()`
(extract/patch) then `simmer()` (configure/make/install). Skip `plate()`.

**Phase 2b and 3 (Chroot):** Load recipe, fetch source to `$LFS/sources/`,
assemble a build script from recipe fields with variable substitution, execute
via `Command::new("chroot")` with `env_clear()` for hermetic environment.

```
Phase 1/2a:                          Phase 2b/3:

Recipe TOML --> parse_recipe_file()  Recipe TOML --> parse_recipe_file()
                    |                                    |
            BuildRunner::fetch_source()         fetch to $LFS/sources/
                    |                                    |
            Cook::new(dest_dir=$LFS)            assemble_build_script()
                    |                             + recipe.substitute()
            Cook::prep()  (extract/patch)                |
                    |                           Command::new("chroot")
            Cook::simmer() (build)                + env_clear()
                    |                                    |
            Files in $LFS                       Files in chroot /
```

### Per-Phase Configuration

| Phase | DESTDIR | PATH | Execution | Cross-compile |
|-------|---------|------|-----------|---------------|
| 1: Cross-tools | `$LFS` | host `/usr/bin` | Direct | Yes (host compiler) |
| 2a: Temp cross | `$LFS` | `$LFS/tools/bin:/usr/bin` | Direct | Yes (cross-toolchain) |
| 2b: Temp chroot | `/` inside chroot | `/usr/bin` inside chroot | `chroot` | No (native) |
| 3: Final system | `/` inside chroot | `/usr/bin` inside chroot | `chroot` | No (native) |

### Reproducibility Environment

All phases set (via `env_clear()` + explicit list for chroot, via `envs()` for
direct):

- `SOURCE_DATE_EPOCH=0` — deterministic timestamps
- `LC_ALL=C` — deterministic locale sorting
- `TZ=UTC` — consistent time output
- `MAKEFLAGS=-j{N}` — from config
- `LFS={lfs_root}` — LFS root path
- `LFS_TGT=x86_64-conary-linux-gnu` — cross-target triple
- `HOME=/root` — consistent home directory
- `TERM=xterm` — prevent build script warnings

Phase 1 and 2a additionally set cross-compilation vars from the recipe's
`[cross]` section: `CC`, `CXX`, `AR`, `LD`, `RANLIB`.

Chroot builds (Phase 2b and 3) use `Command::env_clear()` before setting vars
to prevent host environment contamination (`CC`, `CFLAGS`, `PKG_CONFIG_PATH`,
`LIBRARY_PATH` from the host could break builds).

## Chroot Setup

Before chroot builds (Phase 2b and 3), `setup_chroot()` prepares the LFS root
following LFS 13 Chapter 7.3-7.4:

**Directory creation:**
```
$LFS/{dev,proc,sys,run,etc,home,mnt,opt,srv}
$LFS/usr/{bin,lib,sbin}
$LFS/var/{log,mail,spool}
Symlinks: bin->usr/bin, lib->usr/lib, sbin->usr/sbin, lib64->usr/lib
```

**Virtual kernel filesystems:**
```
mount --bind /dev $LFS/dev
mount -t devpts devpts $LFS/dev/pts -o gid=5,mode=0620
mount -t proc proc $LFS/proc
mount -t sysfs sysfs $LFS/sys
mount -t tmpfs tmpfs $LFS/run
```

**ChrootEnv struct** manages mount lifecycle:
- `mounted: Vec<PathBuf>` — tracks successfully mounted paths in order
- `setup()` — creates dirs, mounts filesystems, pushes each mount to `mounted`
  only after success. If a mount fails mid-setup, already-mounted paths are
  tracked and will be cleaned up.
- `teardown()` — iterates `mounted` in reverse, calls `umount --lazy` for each.
  Errors are logged but do not propagate (best-effort cleanup).
- `Drop` impl calls `teardown()` to prevent leaked mounts on panic.

## Chroot Build Execution

For Phase 2b and Phase 3, build scripts are assembled from recipe fields with
variable substitution applied via `recipe.substitute()`. The `%(destdir)s`
variable is set to `/` (chroot root) rather than a host temp path. Source
tarballs are pre-fetched to `$LFS/sources/` by the caller before entering
the chroot.

```rust
fn build_in_chroot(lfs_root: &Path, recipe: &Recipe, env: &[(&str, &str)]) -> Result<()> {
    let destdir = "/";
    let script = assemble_build_script(recipe, destdir);

    let output = Command::new("chroot")
        .arg(lfs_root)
        .arg("/bin/sh")
        .arg("-c")
        .arg(&script)
        .env_clear()               // Hermetic: no host env leaks
        .envs(env.iter().copied())  // Only known-good vars
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Build failed in chroot: {stderr}");
    }
    Ok(())
}

fn assemble_build_script(recipe: &Recipe, destdir: &str) -> String {
    let mut script = String::from("set -e\n");
    for phase in [&recipe.build.setup, &recipe.build.configure,
                  &recipe.build.make, &recipe.build.install,
                  &recipe.build.post_install] {
        if let Some(ref cmd) = phase {
            let substituted = recipe.substitute(cmd, destdir);
            script.push_str(&substituted);
            script.push('\n');
        }
    }
    script
}
```

## Kitchen Integration for Phase 1/2a (Direct Execution)

Rather than adding a `cook_without_plate()` method (which would fight the
Kitchen's internal `TempDir` ownership of `dest_dir`), we construct a `Cook`
struct directly with an explicit `dest_dir` set to `$LFS`:

```rust
fn build_package(&self, name: &str) -> Result<(), CrossToolsError> {
    let recipe_path = self.recipe_dir.join(format!("{name}.toml"));
    let recipe = parse_recipe_file(&recipe_path)
        .map_err(|e| CrossToolsError::Build { package: name.into(), reason: e.to_string() })?;

    // Fetch source to cache
    self.runner.fetch_source(name, &recipe)
        .map_err(|e| CrossToolsError::Build { package: name.into(), reason: e.to_string() })?;

    // Build using Kitchen's Cook with $LFS as dest_dir
    let config = KitchenConfig {
        jobs: self.config.jobs,
        use_isolation: false,
        source_cache: Some(self.sources_dir.clone()),
        ..Default::default()
    };
    let kitchen = Kitchen::new(config);
    let mut cook = kitchen.new_cook_with_dest(&recipe, &self.lfs_root)?;
    cook.prep()?;
    cook.simmer()?;
    // Skip plate() — no CCS packaging during bootstrap

    Ok(())
}
```

This requires adding `Kitchen::new_cook_with_dest(recipe, dest_dir)` — a
variant of `new_cook()` that accepts an external `dest_dir` instead of
creating a `TempDir`. The Cook's `dest_dir` field becomes a `PathBuf` (the
caller owns the directory lifetime). The `TempDir` field becomes
`Option<TempDir>` — `None` when using an external dest.

The `_runner` field in `CrossToolsBuilder` must be renamed to `runner` (remove
the underscore prefix that marked it as intentionally unused).

## Per-Package Checkpoint Integration

The stage manager has `mark_package_complete()` and `completed_packages()` but
the build loops don't call them. Each build loop must:

1. Read `completed_packages()` at loop start
2. Skip packages already in the completed set
3. Call `mark_package_complete()` after each successful build

```rust
let completed = self.stages.completed_packages(stage);
for (i, pkg) in PACKAGES.iter().enumerate() {
    if completed.contains(&pkg.to_string()) {
        info!("Skipping already-completed package: {pkg}");
        continue;
    }
    self.build_package(pkg)?;
    self.stages.mark_package_complete(stage, pkg)?;
}
```

This must be wired into all four build loops: `cross_tools::build_all()`,
`temp_tools::build_cross_packages()`, `temp_tools::build_chroot_packages()`,
`final_system::build_all()`.

## Files Changed

| File | Action | Lines | What |
|------|--------|-------|------|
| `bootstrap/chroot_env.rs` | Create | ~90 | ChrootEnv with mount tracking, setup/teardown/Drop |
| `bootstrap/cross_tools.rs` | Modify | ~60 | Wire `build_package()`, rename `_runner`, add checkpoint skip |
| `bootstrap/temp_tools.rs` | Modify | ~90 | Wire 3 methods: cross builds, setup_chroot, chroot builds + checkpoints |
| `bootstrap/final_system.rs` | Modify | ~50 | Wire `build_package()` to chroot + checkpoint skip |
| `bootstrap/mod.rs` | Modify | ~15 | Register chroot_env, add `assemble_build_script()` helper |
| `recipe/kitchen/mod.rs` | Modify | ~25 | Add `Kitchen::new_cook_with_dest()` method |
| `recipe/kitchen/cook.rs` | Modify | ~15 | Make `dest_dir` a `PathBuf`, `TempDir` optional |

**Total: ~345 lines of new/modified Rust code.**

No recipe TOML changes. No new dependencies. No schema changes.

## Validation

**Phase 1 end-to-end on Remi** (acceptance test):

```bash
# Build cross-toolchain (5 packages, ~20 min on 12 cores)
conary bootstrap cross-tools --work-dir /conary/bootstrap -j 12 --skip-verify

# Verify
conary bootstrap status --work-dir /conary/bootstrap
# Phase 1: [COMPLETE]

$LFS/tools/bin/x86_64-conary-linux-gnu-gcc -v
# gcc version 15.2.0
```

If Phase 1 passes, Phase 2-3 use the same pattern. Full pipeline produces
`minimal-boot-v3.qcow2`.

## Not In Scope

- Recipe version bumps (already done — LFS 13 aligned on 2026-03-17)
- New recipe TOML fields
- CCS packaging during bootstrap
- Namespace isolation for bootstrap (raw chroot is correct for LFS)
- Phase 4 (system config) and Phase 5 (image) — already implemented
- Phase 6 (tier2) — separate effort
