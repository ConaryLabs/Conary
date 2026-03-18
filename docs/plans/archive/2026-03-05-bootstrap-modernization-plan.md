# Bootstrap Modernization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the bootstrap pipeline production-ready: LFS 12.4 toolchain, systemd-repart images, all stages implemented, dependency-resolved build ordering, consistent sandboxing, real recipe files.

**Architecture:** 8-task plan working bottom-up: config/toolchain first, then stages in pipeline order, then image builder, then CLI/integration. Each task is independently testable. Recipe files are written alongside the stage that consumes them.

**Tech Stack:** Rust 2024, SQLite, crosstool-ng 1.28.0, systemd-repart, LFS 12.4 (binutils 2.45, gcc 15.2.0, glibc 2.42, kernel 6.16.1)

---

### Task 1: Config and Toolchain Modernization

**Files:**
- Modify: `conary-core/src/bootstrap/config.rs:71-111` (BootstrapConfig struct)
- Modify: `conary-core/src/bootstrap/toolchain.rs:46-69` (Toolchain struct)
- Modify: `conary-core/src/bootstrap/toolchain.rs:97-98` (TODO: detect versions)

**Step 1: Write failing tests for version detection**

In `conary-core/src/bootstrap/toolchain.rs`, add to the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_parse_gcc_version() {
    let output = "gcc (GCC) 15.2.0\nCopyright (C) 2025 Free Software Foundation, Inc.";
    let version = Toolchain::parse_version_output(output, "gcc");
    assert_eq!(version, Some("15.2.0".to_string()));
}

#[test]
fn test_parse_glibc_version() {
    let output = "ldd (GNU libc) 2.42\nCopyright (C) 2025 Free Software Foundation, Inc.";
    let version = Toolchain::parse_version_output(output, "ldd");
    assert_eq!(version, Some("2.42".to_string()));
}

#[test]
fn test_parse_binutils_version() {
    let output = "GNU ld (GNU Binutils) 2.45";
    let version = Toolchain::parse_version_output(output, "ld");
    assert_eq!(version, Some("2.45".to_string()));
}

#[test]
fn test_parse_version_output_empty() {
    let version = Toolchain::parse_version_output("", "gcc");
    assert_eq!(version, None);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p conary-core toolchain::tests::test_parse -- --nocapture`
Expected: FAIL -- `parse_version_output` doesn't exist

**Step 3: Implement version detection**

In `conary-core/src/bootstrap/toolchain.rs`, add a public method to `impl Toolchain`:

```rust
/// Parse version from tool's `--version` output.
/// Extracts the first semver-like pattern (X.Y.Z or X.Y).
pub fn parse_version_output(output: &str, _tool: &str) -> Option<String> {
    let re = regex::Regex::new(r"(\d+\.\d+(?:\.\d+)?)").ok()?;
    re.captures(output).map(|c| c[1].to_string())
}

/// Detect glibc version by running `ldd --version` with the toolchain's ldd.
pub fn detect_glibc_version(&mut self) {
    if let Ok(output) = std::process::Command::new(self.tool("ldd"))
        .arg("--version")
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}{stderr}");
        self.glibc_version = Self::parse_version_output(&combined, "ldd");
    }
}

/// Detect binutils version by running `ld --version`.
pub fn detect_binutils_version(&mut self) {
    if let Ok(output) = std::process::Command::new(self.tool("ld"))
        .arg("--version")
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        self.binutils_version = Self::parse_version_output(&stdout, "ld");
    }
}
```

Replace lines 97-98 in `Toolchain::from_prefix()` -- instead of `None` with TODO comments, call the detect methods after construction:

```rust
glibc_version: None,
binutils_version: None,
```

Then after the struct is built, add:
```rust
toolchain.detect_glibc_version();
toolchain.detect_binutils_version();
```

Add `regex` to conary-core's Cargo.toml if not already present (check first).

**Step 4: Update BootstrapConfig default versions**

In `conary-core/src/bootstrap/config.rs`, update `BootstrapConfig::new()` (line 135) to set LFS 12.4 defaults. Add version fields to the struct (around line 71):

```rust
/// Target GCC version
pub gcc_version: String,
/// Target glibc version
pub glibc_version: String,
/// Target binutils version
pub binutils_version: String,
/// Target kernel version (for headers)
pub kernel_version: String,
/// crosstool-ng version
pub crosstool_version: String,
```

Default values in `new()`:
```rust
gcc_version: "15.2.0".to_string(),
glibc_version: "2.42".to_string(),
binutils_version: "2.45".to_string(),
kernel_version: "6.16.1".to_string(),
crosstool_version: "1.28.0".to_string(),
```

**Step 5: Run all tests**

Run: `cargo test -p conary-core bootstrap -- --nocapture`
Expected: All pass (existing + new)

**Step 6: Commit**

```bash
git add conary-core/src/bootstrap/config.rs conary-core/src/bootstrap/toolchain.rs
git commit -m "bootstrap: Add version detection, update to LFS 12.4 defaults"
```

---

### Task 2: Seed Caching and Stage 0 Fixes

**Files:**
- Modify: `conary-core/src/bootstrap/stage0.rs:264-265` (has_local_seed stub)
- Modify: `conary-core/src/bootstrap/stage0.rs:268` (download_and_install_seed)

**Step 1: Write failing test for seed caching**

Add to `stage0.rs` tests:

```rust
#[test]
fn test_seed_cache_detection() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();

    // No seed file -> false
    assert!(!Stage0Builder::has_cached_seed(&downloads, "x86_64-conary-linux-gnu"));

    // Create a fake seed tarball
    let seed_path = downloads.join("x86_64-conary-linux-gnu-seed.tar.xz");
    std::fs::write(&seed_path, b"fake").unwrap();

    // Seed file exists -> true
    assert!(Stage0Builder::has_cached_seed(&downloads, "x86_64-conary-linux-gnu"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core stage0::tests::test_seed_cache -- --nocapture`
Expected: FAIL -- `has_cached_seed` doesn't exist as pub

**Step 3: Implement seed caching**

Replace the `has_local_seed()` private method (line 264) with a public static method:

```rust
/// Check if a cached seed tarball exists in the downloads directory.
pub fn has_cached_seed(downloads_dir: &Path, triple: &str) -> bool {
    let seed_name = format!("{triple}-seed.tar.xz");
    downloads_dir.join(seed_name).exists()
}
```

Update `download_and_install_seed()` to check the cache first:

```rust
let downloads_dir = self.work_dir.join("downloads");
std::fs::create_dir_all(&downloads_dir)?;

let triple = self.config.triple();
let seed_name = format!("{triple}-seed.tar.xz");
let cached_path = downloads_dir.join(&seed_name);

if cached_path.exists() {
    println!("  Using cached seed: {}", cached_path.display());
} else if let Some(ref url) = self.config.seed_url {
    // ... existing download logic, save to cached_path ...
} else {
    return Err(Stage0Error::SeedNotFound);
}
// Extract from cached_path
```

**Step 4: Run all Stage 0 tests**

Run: `cargo test -p conary-core stage0 -- --nocapture`
Expected: All pass

**Step 5: Commit**

```bash
git add conary-core/src/bootstrap/stage0.rs
git commit -m "bootstrap: Implement Stage 0 seed caching"
```

---

### Task 3: Checksum Enforcement

**Files:**
- Modify: `conary-core/src/bootstrap/stage1.rs:414` (VERIFY_BEFORE_BUILD skip)
- Modify: `conary-core/src/bootstrap/base.rs:690` (VERIFY_BEFORE_BUILD skip)
- Modify: `conary-core/src/bootstrap/config.rs` (add skip_verify flag)

**Step 1: Write failing test**

Add to `stage1.rs` tests:

```rust
#[test]
fn test_verify_checksum_rejects_placeholder() {
    // Should NOT silently skip placeholders
    let result = verify_checksum_strict(
        Path::new("/dev/null"),
        "VERIFY_BEFORE_BUILD",
        false,
    );
    assert!(result.is_err());
}

#[test]
fn test_verify_checksum_skip_flag() {
    // Should skip with explicit flag
    let result = verify_checksum_strict(
        Path::new("/dev/null"),
        "VERIFY_BEFORE_BUILD",
        true, // skip_verify = true
    );
    assert!(result.is_ok());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core stage1::tests::test_verify_checksum -- --nocapture`
Expected: FAIL

**Step 3: Add skip_verify flag to config**

In `config.rs` BootstrapConfig struct, add:
```rust
/// Skip checksum verification (development only)
pub skip_verify: bool,
```
Default to `false` in `new()`.

**Step 4: Replace placeholder skip logic**

In both `stage1.rs:414` and `base.rs:690`, replace the block:
```rust
if expected.contains("VERIFY_BEFORE_BUILD") || expected.contains("FIXME") {
    warn!("Skipping checksum verification...");
    return Ok(());
}
```

With:
```rust
if expected.contains("VERIFY_BEFORE_BUILD") || expected.contains("FIXME") {
    if self.config.skip_verify {
        warn!("Skipping placeholder checksum (--skip-verify enabled)");
        return Ok(());
    }
    return Err(/* appropriate error */("Recipe has placeholder checksum '{}' -- provide a real SHA-256 or use --skip-verify", expected));
}
```

Extract the shared logic into a standalone function `verify_checksum_strict(path, expected, skip_verify)` usable by both stage1 and base.

**Step 5: Run all tests**

Run: `cargo test -p conary-core bootstrap -- --nocapture`
Expected: All pass

**Step 6: Commit**

```bash
git add conary-core/src/bootstrap/stage1.rs conary-core/src/bootstrap/base.rs conary-core/src/bootstrap/config.rs
git commit -m "bootstrap: Enforce checksums, reject placeholders unless --skip-verify"
```

---

### Task 4: Stage 1 Recipe Files + Cook Integration

**Files:**
- Create: `recipes/stage1/linux-headers.toml`
- Create: `recipes/stage1/binutils.toml`
- Create: `recipes/stage1/gcc-pass1.toml`
- Create: `recipes/stage1/glibc.toml`
- Create: `recipes/stage1/gcc-pass2.toml`
- Modify: `conary-core/src/bootstrap/stage1.rs:259` (build_package -- route through Cook)

**Step 1: Write Stage 1 recipe files**

Each recipe follows the Recipe TOML format from `conary-core/src/recipe/format.rs`. Example for linux-headers:

```toml
[package]
name = "linux-headers"
version = "6.16.1"
release = "1"
summary = "Linux kernel headers for userspace"
license = "GPL-2.0"
homepage = "https://kernel.org"

[source]
url = "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.16.1.tar.xz"
checksum = "sha256:<real-hash-from-kernel.org>"

[build]
requires = []
makedepends = []
install = "make INSTALL_HDR_PATH=%(destdir)s/usr headers_install"
environment = { ARCH = "%(kernel_arch)s" }

[cross]
target = "%(target)s"
sysroot = "%(sysroot)s"
stage = "stage1"
```

Write similar files for binutils, gcc-pass1, glibc, gcc-pass2 with build instructions matching LFS 12.4 Chapter 5. Use real SHA-256 checksums fetched from upstream mirrors.

**Step 2: Write test for recipe loading**

Add to `stage1.rs` tests:

```rust
#[test]
fn test_load_stage1_recipes() {
    let recipe_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .join("recipes/stage1");
    if !recipe_dir.exists() {
        // Skip in CI if recipes not present
        return;
    }
    for name in &["linux-headers", "binutils", "gcc-pass1", "glibc", "gcc-pass2"] {
        let path = recipe_dir.join(format!("{name}.toml"));
        assert!(path.exists(), "Missing recipe: {path:?}");
        let recipe = crate::recipe::parse_recipe_file(&path).unwrap();
        assert!(!recipe.package.name.is_empty());
        assert!(!recipe.source.checksum.contains("VERIFY_BEFORE_BUILD"));
        assert!(!recipe.source.checksum.contains("FIXME"));
    }
}
```

**Step 3: Integrate Cook into Stage 1 build_package**

In `stage1.rs`, modify `build_package()` (line 259) to optionally route through the Cook pipeline from `crate::recipe::kitchen`. The key change:

- Currently: manually calls `fetch_source()`, `extract_source()`, `run_configure()`, `run_make()`, `run_install()` with custom shell commands
- New: create a `Kitchen` with `NoopResolver`, create a `Cook`, call `prep()`, `unpack()`, `patch()`, `simmer()`
- The Cook already uses `ContainerConfig::pristine_for_bootstrap()` internally (cook.rs:306)
- Fall back to direct execution if Kitchen creation fails (for environments without container support)

This is the trickiest refactor. The key insight: Cook's `simmer()` method (line 200 of cook.rs) runs configure/make/install phases, which is exactly what Stage 1's manual code does. The substitution variables (`%(target)s`, `%(sysroot)s`, etc.) are handled by the Recipe system.

**Step 4: Run tests**

Run: `cargo test -p conary-core stage1 -- --nocapture`
Expected: All pass

**Step 5: Commit**

```bash
git add recipes/stage1/ conary-core/src/bootstrap/stage1.rs
git commit -m "bootstrap: Add Stage 1 LFS 12.4 recipes, integrate Cook pipeline"
```

---

### Task 5: Stage 2 Implementation

**Files:**
- Create: `conary-core/src/bootstrap/stage2.rs`
- Modify: `conary-core/src/bootstrap/mod.rs` (add stage2 module, pub use, build_stage2 method)
- Modify: `conary-core/src/bootstrap/stages.rs` (wire Stage2 into pipeline)
- Modify: `src/commands/bootstrap/mod.rs` (add cmd_bootstrap_stage2)
- Modify: `src/cli/bootstrap.rs` (add Stage2 subcommand)

**Step 1: Write failing test for Stage 2**

Create `conary-core/src/bootstrap/stage2.rs` with tests first:

```rust
// conary-core/src/bootstrap/stage2.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage2_reuses_stage1_recipes() {
        // Stage 2 builds the same 5 packages as Stage 1
        let packages = Stage2Builder::package_names();
        assert_eq!(packages, &[
            "linux-headers", "binutils", "gcc-pass1", "glibc", "gcc-pass2"
        ]);
    }

    #[test]
    fn test_stage2_uses_stage1_toolchain() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let stage1_tc = Toolchain::from_prefix(
            dir.path().join("stage1"),
            "x86_64-conary-linux-gnu",
            ToolchainKind::Stage1,
        );
        let builder = Stage2Builder::new(
            dir.path().to_path_buf(),
            config,
            stage1_tc,
        );
        assert_eq!(builder.toolchain().kind(), &ToolchainKind::Stage1);
    }
}
```

**Step 2: Implement Stage2Builder**

Stage2Builder is structurally identical to Stage1Builder but:
- Takes a Stage1 toolchain as input (not Stage0)
- Outputs to a separate directory (`<work_dir>/stage2/`)
- Compares output hashes against Stage 1 output for reproducibility verification
- Reuses the same recipe files from `recipes/stage1/`

```rust
// conary-core/src/bootstrap/stage2.rs
use crate::bootstrap::config::BootstrapConfig;
use crate::bootstrap::toolchain::{Toolchain, ToolchainKind};

#[derive(Debug, thiserror::Error)]
pub enum Stage2Error {
    #[error("Stage 1 toolchain required")]
    NoStage1Toolchain,
    #[error("Build failed for {package}: {reason}")]
    BuildFailed { package: String, reason: String },
    #[error("Reproducibility mismatch for {package}")]
    ReproducibilityMismatch { package: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct Stage2Builder {
    work_dir: PathBuf,
    config: BootstrapConfig,
    toolchain: Toolchain,
    // ... same fields as Stage1Builder
}
```

The implementation delegates to the same Cook pipeline as Stage 1, just with a different input toolchain. Add a `verify_reproducibility()` method that compares file hashes between stage1 and stage2 output directories.

**Step 3: Wire into mod.rs and CLI**

In `conary-core/src/bootstrap/mod.rs`:
- Add `mod stage2;`
- Add `pub use stage2::{Stage2Builder, Stage2Error};`
- Add `pub fn build_stage2(&self, recipe_dir: &Path) -> Result<Toolchain>`

In `src/cli/bootstrap.rs`:
- Add `Stage2 { ... }` variant with `--skip` flag

In `src/commands/bootstrap/mod.rs`:
- Add `pub fn cmd_bootstrap_stage2(...)`
- Wire into resume logic

**Step 4: Run all tests**

Run: `cargo test -p conary-core bootstrap -- --nocapture`
Expected: All pass

**Step 5: Commit**

```bash
git add conary-core/src/bootstrap/stage2.rs conary-core/src/bootstrap/mod.rs \
  src/commands/bootstrap/mod.rs src/cli/bootstrap.rs
git commit -m "bootstrap: Implement Stage 2 (reproducibility rebuild)"
```

---

### Task 6: Base System Overhaul (RecipeGraph + Sandboxing)

**Files:**
- Modify: `conary-core/src/bootstrap/base.rs` (replace hardcoded lists with RecipeGraph)
- Create: `recipes/base/*.toml` (~80 recipe files)
- Modify: `conary-core/src/bootstrap/stages.rs` (Boot/Networking become BaseSystem checkpoints)

**Step 1: Write test for graph-based ordering**

Add to `base.rs` tests:

```rust
#[test]
fn test_base_recipe_graph_resolves() {
    let recipe_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .join("recipes/base");
    if !recipe_dir.exists() {
        return;
    }
    let mut graph = crate::recipe::RecipeGraph::new();
    // Load all recipes and add to graph
    for entry in std::fs::read_dir(&recipe_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "toml") {
            let recipe = crate::recipe::parse_recipe_file(&path).unwrap();
            graph.add_from_recipe(&recipe);
        }
    }
    // Must resolve without cycles
    let order = graph.topological_sort().unwrap();
    assert!(!order.is_empty());
    // zlib must come before everything that depends on it
    let zlib_pos = order.iter().position(|n| n == "zlib");
    let curl_pos = order.iter().position(|n| n == "curl");
    if let (Some(z), Some(c)) = (zlib_pos, curl_pos) {
        assert!(z < c, "zlib must build before curl");
    }
}

#[test]
fn test_base_all_recipes_have_real_checksums() {
    let recipe_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .join("recipes/base");
    if !recipe_dir.exists() {
        return;
    }
    for entry in std::fs::read_dir(&recipe_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "toml") {
            let recipe = crate::recipe::parse_recipe_file(&path).unwrap();
            assert!(
                !recipe.source.checksum.contains("FIXME"),
                "Placeholder checksum in {}",
                path.display()
            );
            assert!(
                !recipe.source.checksum.contains("VERIFY_BEFORE_BUILD"),
                "Placeholder checksum in {}",
                path.display()
            );
        }
    }
}
```

**Step 2: Write base recipe files**

Create ~80 TOML files in `recipes/base/` aligned with LFS 12.4 Chapter 8. Each recipe includes:
- Real source URLs from upstream
- Real SHA-256 checksums
- Build instructions (configure/make/install)
- Dependencies in `build.requires` for graph resolution
- A `tag` field for phase classification (libraries, dev, core, userland, boot, networking)

The package list expands the current 60 to ~80 by adding LFS 12.4 packages not currently present (e.g., wheel, flit-core, markupsafe, jinja2, meson's Python deps, etc.) and the networking packages (iproute2, openssh, dhcpcd, wget, curl) that were in the Networking stub stage.

**Step 3: Refactor BaseBuilder to use RecipeGraph**

Replace the hardcoded constant arrays (lines 162-241) with dynamic loading:

```rust
impl BaseBuilder {
    pub fn load_recipes_from_dir(recipe_dir: &Path) -> Result<(RecipeGraph, Vec<Recipe>)> {
        let mut graph = RecipeGraph::new();
        let mut recipes = Vec::new();
        for entry in std::fs::read_dir(recipe_dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "toml") {
                let recipe = parse_recipe_file(&path)?;
                graph.add_from_recipe(&recipe);
                recipes.push(recipe);
            }
        }
        let order = graph.topological_sort()?;
        Ok((graph, recipes, order))
    }
}
```

Keep the phase constants as tag lists for progress reporting only:
```rust
const LIBRARY_TAGS: &[&str] = &["zlib", "xz", "zstd", ...];
// Used for: "Building phase: Libraries (12/80)"
```

**Step 4: Add per-package checkpointing**

Add to `StageManager` a method for sub-stage progress:

```rust
pub fn mark_package_complete(&mut self, stage: BootstrapStage, package: &str) {
    // Persist to bootstrap-state.json under stage.packages_complete
}

pub fn completed_packages(&self, stage: BootstrapStage) -> Vec<String> {
    // Read from state
}
```

In `BaseBuilder::build()`, skip packages already in `completed_packages()`.

**Step 5: Add Sandbox isolation to base builds**

Replace the bare `Command::new("bash")` calls in `base.rs:852` (`run_shell_command`) with the same `ContainerConfig::pristine_for_bootstrap()` pattern used in Stage 1. Import from `crate::container`.

**Step 6: Collapse Boot/Networking stages**

In `stages.rs`, update `BootstrapStage::is_required()` (line 92):
- Boot -> not required (checkpoint only)
- Networking -> not required (checkpoint only)

In `BaseBuilder`, after building all boot-tagged packages, call `stages.mark_complete(BootstrapStage::Boot)`. Same for networking-tagged packages.

**Step 7: Run all tests**

Run: `cargo test -p conary-core bootstrap -- --nocapture`
Expected: All pass

**Step 8: Commit**

```bash
git add recipes/base/ conary-core/src/bootstrap/base.rs conary-core/src/bootstrap/stages.rs
git commit -m "bootstrap: Graph-ordered base system with ~80 LFS 12.4 recipes"
```

---

### Task 7: Conary Stage Implementation

**Files:**
- Create: `conary-core/src/bootstrap/conary_stage.rs`
- Create: `recipes/conary/rust.toml`
- Create: `recipes/conary/conary.toml`
- Modify: `conary-core/src/bootstrap/mod.rs` (add module, build_conary method)
- Modify: `src/commands/bootstrap/mod.rs` (add cmd_bootstrap_conary)
- Modify: `src/cli/bootstrap.rs` (add Conary subcommand with --skip flag)

**Step 1: Write test**

```rust
#[test]
fn test_conary_stage_requires_base_system() {
    let dir = tempfile::tempdir().unwrap();
    let config = BootstrapConfig::new();
    let builder = ConaryStageBuilder::new(dir.path().to_path_buf(), config);
    // Should fail without a sysroot
    assert!(builder.validate_sysroot().is_err());
}

#[test]
fn test_conary_stage_packages() {
    assert_eq!(ConaryStageBuilder::package_names(), &["rust", "conary"]);
}
```

**Step 2: Implement ConaryStageBuilder**

```rust
// conary-core/src/bootstrap/conary_stage.rs

pub struct ConaryStageBuilder {
    work_dir: PathBuf,
    config: BootstrapConfig,
    sysroot: PathBuf,
}

impl ConaryStageBuilder {
    pub fn new(work_dir: PathBuf, config: BootstrapConfig) -> Self { ... }

    pub fn validate_sysroot(&self) -> Result<()> {
        // Check sysroot has /usr/bin/gcc, /usr/lib/libc.so, etc.
    }

    pub fn build_rust(&self) -> Result<PathBuf> {
        // 1. Download rustc bootstrap binary (from static.rust-lang.org)
        // 2. Extract to work_dir/rust-bootstrap/
        // 3. Run ./x.py build --target <triple> --prefix <sysroot>/usr
        // 4. Run ./x.py install
        // Network access required (controlled via ContainerConfig::allow_network)
    }

    pub fn build_conary(&self) -> Result<PathBuf> {
        // 1. Copy Conary source to work_dir/conary-src/
        // 2. Set up cargo cross-compilation env vars
        // 3. cargo build --release --target <triple>
        // 4. Install binary to sysroot/usr/bin/conary
    }

    pub fn build(&self) -> Result<()> {
        self.validate_sysroot()?;
        self.build_rust()?;
        self.build_conary()?;
        Ok(())
    }
}
```

**Step 3: Write recipe files**

`recipes/conary/rust.toml` -- Rust compiler from source. Uses bootstrap binary download + `./x.py`. Network access flag set.

`recipes/conary/conary.toml` -- Conary itself. Source is the local repo or a tagged release tarball. `cargo build --release`.

**Step 4: Wire into CLI**

Add `Conary` subcommand to `src/cli/bootstrap.rs` with `--skip` flag.
Add `cmd_bootstrap_conary()` to `src/commands/bootstrap/mod.rs`.
Update resume logic to handle Conary stage.

**Step 5: Run tests**

Run: `cargo test -p conary-core bootstrap -- --nocapture`
Expected: All pass

**Step 6: Commit**

```bash
git add conary-core/src/bootstrap/conary_stage.rs recipes/conary/ \
  conary-core/src/bootstrap/mod.rs src/commands/bootstrap/mod.rs src/cli/bootstrap.rs
git commit -m "bootstrap: Implement Conary stage (Rust + self-hosting)"
```

---

### Task 8: Image Builder Modernization (systemd-repart)

**Files:**
- Modify: `conary-core/src/bootstrap/image.rs` (add systemd-repart path, keep sfdisk fallback)
- Create: `conary-core/src/bootstrap/repart.rs` (repart.d definition generation)

**Step 1: Write failing test for repart definitions**

Create `conary-core/src/bootstrap/repart.rs` with test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_esp_definition() {
        let def = RepartDefinition::esp(512);
        let content = def.to_string();
        assert!(content.contains("[Partition]"));
        assert!(content.contains("Type=esp"));
        assert!(content.contains("SizeMinBytes=536870912")); // 512 MB
        assert!(content.contains("Format=vfat"));
    }

    #[test]
    fn test_generate_root_definition() {
        let def = RepartDefinition::root(TargetArch::X86_64);
        let content = def.to_string();
        assert!(content.contains("Type=root-x86-64"));
        assert!(content.contains("Format=ext4"));
    }

    #[test]
    fn test_generate_repart_dir() {
        let dir = tempfile::tempdir().unwrap();
        generate_repart_definitions(
            dir.path(),
            TargetArch::X86_64,
            512,
        ).unwrap();
        assert!(dir.path().join("00-esp.conf").exists());
        assert!(dir.path().join("10-root.conf").exists());
    }
}
```

**Step 2: Implement repart definition generation**

```rust
// conary-core/src/bootstrap/repart.rs

use crate::bootstrap::config::TargetArch;

pub struct RepartDefinition {
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

impl RepartDefinition {
    pub fn esp(size_mb: u64) -> Self {
        // [Partition]
        // Type=esp
        // SizeMinBytes=<size_mb * 1024 * 1024>
        // SizeMaxBytes=<same>
        // Format=vfat
        // CopyFiles=/boot:/
    }

    pub fn root(arch: TargetArch) -> Self {
        let part_type = match arch {
            TargetArch::X86_64 => "root-x86-64",
            TargetArch::Aarch64 => "root-arm64",
            TargetArch::Riscv64 => "root-riscv64",
        };
        // [Partition]
        // Type=<part_type>
        // Format=ext4
        // CopyFiles=/:/
        // Minimize=guess
    }
}

pub fn generate_repart_definitions(
    output_dir: &Path,
    arch: TargetArch,
    esp_size_mb: u64,
) -> Result<()> {
    std::fs::write(
        output_dir.join("00-esp.conf"),
        RepartDefinition::esp(esp_size_mb).to_string(),
    )?;
    std::fs::write(
        output_dir.join("10-root.conf"),
        RepartDefinition::root(arch).to_string(),
    )?;
    Ok(())
}
```

**Step 3: Add systemd-repart path to ImageBuilder**

In `image.rs`, add to `ImageTools::check()` (line 200):
```rust
pub systemd_repart: Option<PathBuf>,
pub ukify: Option<PathBuf>,
```

Add `build_raw_repart()` method that:
1. Generates repart.d definitions via `generate_repart_definitions()`
2. Invokes `systemd-repart --empty=create --size=<size> --definitions=<dir> --root=<sysroot> <output>`
3. No root required, no loop devices

Update `build_raw()` (line 419) to try repart first, fall back to sfdisk:
```rust
fn build_raw(&self) -> Result<ImageResult> {
    if self.tools.systemd_repart.is_some() {
        self.build_raw_repart()
    } else {
        self.build_raw_legacy()
    }
}
```

Rename existing `build_raw()` to `build_raw_legacy()`.

**Step 4: Add UKI generation**

If `ukify` is available, generate a Unified Kernel Image:
```rust
fn generate_uki(&self) -> Result<Option<PathBuf>> {
    let Some(ukify) = &self.tools.ukify else { return Ok(None) };
    // ukify build \
    //   --linux <sysroot>/boot/vmlinuz \
    //   --initrd <sysroot>/boot/initrd.img \
    //   --cmdline "root=PARTUUID=... rw" \
    //   --output <esp>/EFI/Linux/conary.efi
}
```

**Step 5: Run all tests**

Run: `cargo test -p conary-core bootstrap -- --nocapture`
Expected: All pass

**Step 6: Commit**

```bash
git add conary-core/src/bootstrap/repart.rs conary-core/src/bootstrap/image.rs \
  conary-core/src/bootstrap/mod.rs
git commit -m "bootstrap: Add systemd-repart image builder with UKI support"
```

---

### Task 9: CLI Updates and Dry-Run Validation

**Files:**
- Modify: `src/cli/bootstrap.rs` (add --skip-stage2, --skip-conary, --skip-verify flags)
- Modify: `src/commands/bootstrap/mod.rs` (update resume, add dry-run)
- Modify: `conary-core/src/bootstrap/mod.rs` (add dry_run validation method)

**Step 1: Write dry-run test**

Add to `mod.rs` tests:

```rust
#[test]
fn test_dry_run_validates_recipes() {
    let dir = tempfile::tempdir().unwrap();
    let config = BootstrapConfig::new();
    let bootstrap = Bootstrap::with_config(dir.path().to_path_buf(), config);

    let recipe_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .join("recipes");
    if !recipe_dir.exists() {
        return;
    }

    let result = bootstrap.dry_run(&recipe_dir);
    assert!(result.is_ok(), "Dry run failed: {:?}", result.err());
}
```

**Step 2: Implement dry_run on Bootstrap**

```rust
impl Bootstrap {
    /// Validate the full pipeline without building anything.
    /// Checks: recipes exist, checksums parse, graph resolves, sandbox configs valid.
    pub fn dry_run(&self, recipe_dir: &Path) -> Result<DryRunReport> {
        let mut report = DryRunReport::new();

        // Check Stage 1 recipes
        report.check_stage1_recipes(&recipe_dir.join("stage1"))?;

        // Check Base recipes and graph resolution
        report.check_base_recipes(&recipe_dir.join("base"))?;

        // Check Conary recipes (optional)
        if recipe_dir.join("conary").exists() {
            report.check_conary_recipes(&recipe_dir.join("conary"))?;
        }

        // Verify no placeholder checksums
        report.verify_no_placeholders()?;

        Ok(report)
    }
}
```

**Step 3: Update CLI flags**

In `src/cli/bootstrap.rs`, add global flags:
- `--skip-stage2` on `Base` and `Resume` subcommands
- `--skip-conary` on `Resume` subcommand
- `--skip-verify` on all build subcommands

Add new subcommand:
```rust
/// Validate the full pipeline without building
DryRun {
    /// Working directory
    #[arg(long, default_value = ".")]
    work_dir: String,
    /// Recipe directory
    #[arg(long, default_value = "recipes")]
    recipe_dir: String,
},
```

**Step 4: Update resume logic**

In `src/commands/bootstrap/mod.rs`, replace the `[NOT IMPLEMENTED]` stub (line 469) with actual stage dispatch for Stage2, Conary, and Image stages, respecting skip flags.

**Step 5: Run full test suite**

Run: `cargo test -p conary-core bootstrap -- --nocapture && cargo test bootstrap -- --nocapture`
Expected: All pass

**Step 6: Commit**

```bash
git add src/cli/bootstrap.rs src/commands/bootstrap/mod.rs conary-core/src/bootstrap/mod.rs
git commit -m "bootstrap: Add dry-run validation, CLI flags, complete resume logic"
```

---

### Task 10: Integration Test and Final Verification

**Files:**
- Create: `conary-core/src/bootstrap/integration_test.rs` (or add to existing test modules)
- Modify: `conary-core/src/bootstrap/mod.rs` (ensure all modules wired)

**Step 1: Write integration test**

```rust
#[test]
fn test_full_pipeline_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    let config = BootstrapConfig::new();
    let bootstrap = Bootstrap::with_config(dir.path().to_path_buf(), config);

    let recipe_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .join("recipes");
    if !recipe_dir.exists() {
        return; // Skip if recipes not checked in yet
    }

    // Dry run validates everything
    let report = bootstrap.dry_run(&recipe_dir).unwrap();

    // Stage 1: 5 recipes
    assert_eq!(report.stage1_count, 5);

    // Base: ~80 recipes, graph resolves
    assert!(report.base_count >= 60);
    assert!(report.graph_resolved);

    // No placeholder checksums
    assert_eq!(report.placeholder_count, 0);
}
```

**Step 2: Run full cargo test**

Run: `cargo test -- --nocapture`
Expected: All 1800+ tests pass including new bootstrap tests

**Step 3: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: Clean

**Step 4: Final commit**

```bash
git add -A
git commit -m "bootstrap: Integration tests and final verification"
```

---

## Task Dependency Graph

```
Task 1 (config/toolchain) ──┬── Task 2 (seed caching)
                             ├── Task 3 (checksum enforcement)
                             │
Task 3 ──────────────────────┼── Task 4 (Stage 1 recipes + Cook)
                             │
Task 4 ──────────────────────┼── Task 5 (Stage 2)
                             ├── Task 6 (Base system overhaul)
                             │
Task 5 + Task 6 ─────────────┼── Task 7 (Conary stage)
                             │
Task 6 ──────────────────────┼── Task 8 (Image builder)
                             │
Task 7 + Task 8 ─────────────┼── Task 9 (CLI + dry-run)
                             │
Task 9 ──────────────────────┴── Task 10 (Integration test)
```

**Parallelizable pairs:** Tasks 2+3 (after Task 1). Tasks 5+6 (after Task 4). Tasks 7+8 (after 5+6).
