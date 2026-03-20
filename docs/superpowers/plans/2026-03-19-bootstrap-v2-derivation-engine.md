# Bootstrap v2: Derivation Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the CAS-layered derivation engine that turns TOML recipes into content-addressed, cacheable, composable package outputs — from single-package builds through full staged pipeline execution.

**Architecture:** A new `derivation` module in `conary-core` wraps around the existing Kitchen, CAS, EROFS builder, and Sandbox. The derivation engine computes deterministic derivation IDs from recipe inputs, executes builds in composefs-mounted environments, captures outputs to CAS, and orchestrates multi-stage pipelines from declarative system manifests.

**Tech Stack:** Rust 1.94, SQLite (rusqlite), SHA-256 (sha2 crate), TOML (toml crate), composefs-rs, existing conary-core primitives (CasStore, Kitchen, Sandbox, build_erofs_image)

**Spec:** `docs/superpowers/specs/2026-03-19-bootstrap-v2-cas-layered-design.md` (revision 3)

---

## Pre-Implementation API Notes

The code samples in this plan are illustrative. Before implementing each task, the agent MUST read the actual source files and adapt. Known API differences:

| Plan Assumes | Actual API | Fix |
|-------------|-----------|-----|
| `CasStore::new(path)` returns `CasStore` | Returns `Result<CasStore>` | Add `.unwrap()` in tests, `?` in production |
| `Kitchen::new_cook_with_dest(recipe, dest).cook()` | This method chain may not exist. Read `conary-core/src/recipe/kitchen/cook.rs` | Use the actual Kitchen cook API with `KitchenConfig::for_bootstrap(sysroot)` and adapt |
| `BuildSection { chroot: false, .. }` | `BuildSection` may not have a `chroot` field | Check `format.rs` and use actual fields with `Default::default()` for unknown fields |
| `PackageSection.summary` is `String` | May be `Option<String>` | Use `Some("test".into())` |
| `SourceSection.additional` is `Option<Vec<_>>` | May be `Vec<_>` (always present, possibly empty) | Use the actual type |
| Schema migration as inline SQL | Migrations may use a function dispatch pattern | Read `schema.rs` migration pattern and follow it |
| `nix::mount::umount()` in `environment.rs` | Check if `nix` crate is a dependency | Add to `Cargo.toml` if needed, or use `std::process::Command::new("umount")` |
| Pipeline calls `executor.execute()` without mounting | Must mount composefs before Kitchen.cook | Add `BuildEnvironment::mount()` call before execute in pipeline loop |
| Cache hit path doesn't preserve OutputManifest | EROFS composition needs manifests from cached derivations too | On cache hit, load manifest from CAS using `manifest_cas_hash` |

**Critical rule:** When a code sample doesn't compile, read the actual source file it references, understand the real API, and write code that works. The plan gives you the architecture and test structure; the exact API calls must match the codebase.

---

## File Structure

### New files (all under `conary-core/src/derivation/`)

| File | Responsibility |
|------|---------------|
| `mod.rs` | Module root, re-exports |
| `id.rs` | `DerivationId` — canonical serialization, SHA-256 computation |
| `output.rs` | `PackageOutput`, `OutputFile`, `OutputManifest` — build result types |
| `recipe_hash.rs` | `build_script_hash()` — hash recipe build sections deterministically |
| `index.rs` | `DerivationIndex` — SQLite-backed `derivation_id -> output_hash` mapping |
| `capture.rs` | `capture_output()` — walk DESTDIR, ingest to CAS, produce manifest |
| `executor.rs` | `DerivationExecutor` — single-package build: env mount -> Kitchen -> capture |
| `compose.rs` | `compose_erofs()` — merge N `PackageOutput`s into one EROFS image |
| `environment.rs` | `BuildEnvironment` — composefs mount/unmount lifecycle |
| `seed.rs` | `Seed` — import, verify, load as Layer 0 |
| `manifest.rs` | `SystemManifest` — parse system manifest TOML |
| `stages.rs` | `StageAssigner` — dependency graph analysis, SCC detection, assignment |
| `profile.rs` | `BuildProfile` — generation, serialization, diffing |
| `pipeline.rs` | `Pipeline` — full staged execution orchestrator |

### Modified files

| File | Change |
|------|--------|
| `conary-core/src/lib.rs` | Add `pub mod derivation;` |
| `conary-core/src/db/schema.rs` | Add v54 migration for `derivation_index` table |
| `src/cli/mod.rs` | Add `DerivationCommands`, `ProfileCommands` |
| `src/commands/mod.rs` | Add `derivation`, `profile` modules |

### New CLI files

| File | Responsibility |
|------|---------------|
| `src/cli/derivation.rs` | `DerivationCommands` clap definition |
| `src/cli/profile.rs` | `ProfileCommands` clap definition |
| `src/commands/derivation.rs` | `conary derivation build/show` implementation |
| `src/commands/profile.rs` | `conary profile generate/show/diff` implementation |

---

## Task 1: Derivation Data Model

**Files:**
- Create: `conary-core/src/derivation/mod.rs`
- Create: `conary-core/src/derivation/id.rs`
- Create: `conary-core/src/derivation/output.rs`
- Modify: `conary-core/src/lib.rs`

- [ ] **Step 1: Create module skeleton and declare in lib.rs**

Add `pub mod derivation;` to `conary-core/src/lib.rs` (after existing modules, alphabetical).

Create `conary-core/src/derivation/mod.rs`:
```rust
// conary-core/src/derivation/mod.rs
pub mod id;
pub mod output;

pub use id::{DerivationId, SourceDerivationId};
pub use output::{OutputFile, OutputManifest, OutputSymlink, PackageOutput};
```

- [ ] **Step 2: Write failing tests for DerivationId**

Create `conary-core/src/derivation/id.rs` with types and tests:
```rust
// conary-core/src/derivation/id.rs
use std::collections::BTreeMap;
use sha2::{Digest, Sha256};

/// Content-addressed derivation identifier.
/// SHA-256 of canonical input serialization.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DerivationId(String);

/// Derivation ID excluding build environment — for cross-seed verification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceDerivationId(String);

/// All inputs needed to compute a derivation ID.
#[derive(Debug, Clone)]
pub struct DerivationInputs {
    pub source_hash: String,
    pub build_script_hash: String,
    /// Sorted by name. BTreeMap ensures deterministic order.
    pub dependency_ids: BTreeMap<String, DerivationId>,
    pub build_env_hash: String,
    pub target_triple: String,
    /// Sorted by key. BTreeMap ensures deterministic order.
    pub build_options: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_id_is_deterministic() {
        let inputs = DerivationInputs {
            source_hash: "abc123".into(),
            build_script_hash: "def456".into(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "seed000".into(),
            target_triple: "x86_64-conary-linux-gnu".into(),
            build_options: BTreeMap::new(),
        };
        let id1 = DerivationId::compute(&inputs);
        let id2 = DerivationId::compute(&inputs);
        assert_eq!(id1, id2);
    }

    #[test]
    fn different_inputs_produce_different_ids() {
        let mut inputs1 = DerivationInputs {
            source_hash: "abc123".into(),
            build_script_hash: "def456".into(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "seed000".into(),
            target_triple: "x86_64-conary-linux-gnu".into(),
            build_options: BTreeMap::new(),
        };
        let mut inputs2 = inputs1.clone();
        inputs2.source_hash = "xyz789".into();
        assert_ne!(
            DerivationId::compute(&inputs1),
            DerivationId::compute(&inputs2),
        );
    }

    #[test]
    fn canonical_format_matches_spec() {
        let inputs = DerivationInputs {
            source_hash: "abc123".into(),
            build_script_hash: "def456".into(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "seed000".into(),
            target_triple: "x86_64-conary-linux-gnu".into(),
            build_options: BTreeMap::new(),
        };
        let canonical = DerivationId::canonical_string(&inputs);
        assert!(canonical.starts_with("CONARY-DERIVATION-V1\n"));
        assert!(canonical.contains("source:abc123\n"));
        assert!(canonical.contains("script:def456\n"));
        assert!(canonical.contains("env:seed000\n"));
        assert!(canonical.contains("target:x86_64-conary-linux-gnu\n"));
    }

    #[test]
    fn deps_are_sorted_by_name() {
        let mut deps = BTreeMap::new();
        deps.insert("zlib".into(), DerivationId("zzz".into()));
        deps.insert("glibc".into(), DerivationId("ggg".into()));
        deps.insert("binutils".into(), DerivationId("bbb".into()));

        let inputs = DerivationInputs {
            source_hash: "src".into(),
            build_script_hash: "script".into(),
            dependency_ids: deps,
            build_env_hash: "env".into(),
            target_triple: "x86_64-conary-linux-gnu".into(),
            build_options: BTreeMap::new(),
        };
        let canonical = DerivationId::canonical_string(&inputs);
        let dep_lines: Vec<&str> = canonical.lines()
            .filter(|l| l.starts_with("dep:"))
            .collect();
        assert_eq!(dep_lines[0], "dep:binutils:bbb");
        assert_eq!(dep_lines[1], "dep:glibc:ggg");
        assert_eq!(dep_lines[2], "dep:zlib:zzz");
    }

    #[test]
    fn source_derivation_id_excludes_env() {
        let inputs = DerivationInputs {
            source_hash: "abc".into(),
            build_script_hash: "def".into(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "env1".into(),
            target_triple: "x86_64-conary-linux-gnu".into(),
            build_options: BTreeMap::new(),
        };
        let mut inputs2 = inputs.clone();
        inputs2.build_env_hash = "env2".into();

        // Different build_env_hash -> different derivation IDs
        assert_ne!(
            DerivationId::compute(&inputs),
            DerivationId::compute(&inputs2),
        );
        // But same source derivation IDs
        assert_eq!(
            SourceDerivationId::compute(&inputs),
            SourceDerivationId::compute(&inputs2),
        );
    }

    #[test]
    fn derivation_id_is_64_char_hex() {
        let inputs = DerivationInputs {
            source_hash: "test".into(),
            build_script_hash: "test".into(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "test".into(),
            target_triple: "x86_64-conary-linux-gnu".into(),
            build_options: BTreeMap::new(),
        };
        let id = DerivationId::compute(&inputs);
        assert_eq!(id.as_str().len(), 64);
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p conary-core derivation::id --no-run 2>&1 | head -20`
Expected: Compilation errors — `DerivationId::compute`, `canonical_string`, `as_str` not yet implemented.

- [ ] **Step 4: Implement DerivationId**

Add to `conary-core/src/derivation/id.rs` (above tests):
```rust
impl DerivationId {
    pub fn compute(inputs: &DerivationInputs) -> Self {
        let canonical = Self::canonical_string(inputs);
        let hash = Sha256::digest(canonical.as_bytes());
        Self(hex::encode(hash))
    }

    pub fn canonical_string(inputs: &DerivationInputs) -> String {
        let mut s = String::with_capacity(512);
        s.push_str("CONARY-DERIVATION-V1\n");
        s.push_str(&format!("source:{}\n", inputs.source_hash));
        s.push_str(&format!("script:{}\n", inputs.build_script_hash));
        for (name, dep_id) in &inputs.dependency_ids {
            s.push_str(&format!("dep:{}:{}\n", name, dep_id.0));
        }
        s.push_str(&format!("env:{}\n", inputs.build_env_hash));
        s.push_str(&format!("target:{}\n", inputs.target_triple));
        for (key, value) in &inputs.build_options {
            s.push_str(&format!("opt:{}:{}\n", key, value));
        }
        s
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl SourceDerivationId {
    pub fn compute(inputs: &DerivationInputs) -> Self {
        let mut s = String::with_capacity(512);
        s.push_str("CONARY-DERIVATION-V1\n");
        s.push_str(&format!("source:{}\n", inputs.source_hash));
        s.push_str(&format!("script:{}\n", inputs.build_script_hash));
        for (name, dep_id) in &inputs.dependency_ids {
            s.push_str(&format!("dep:{}:{}\n", name, dep_id.0));
        }
        // NOTE: env: line deliberately omitted
        s.push_str(&format!("target:{}\n", inputs.target_triple));
        for (key, value) in &inputs.build_options {
            s.push_str(&format!("opt:{}:{}\n", key, value));
        }
        let hash = Sha256::digest(s.as_bytes());
        Self(hex::encode(hash))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DerivationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for SourceDerivationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

Ensure `sha2` and `hex` are in `conary-core/Cargo.toml` dependencies (they should be — check with `grep sha2 conary-core/Cargo.toml`). If not: `cargo add sha2 hex -p conary-core`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p conary-core derivation::id -- --nocapture`
Expected: All 6 tests pass.

- [ ] **Step 6: Write PackageOutput types with tests**

Create `conary-core/src/derivation/output.rs`:
```rust
// conary-core/src/derivation/output.rs
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A single file in a package output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputFile {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub mode: u32,
}

/// A symlink in a package output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputSymlink {
    pub path: String,
    pub target: String,
}

/// Complete build output manifest for one derivation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputManifest {
    pub derivation_id: String,
    pub output_hash: String,
    pub files: Vec<OutputFile>,
    pub symlinks: Vec<OutputSymlink>,
    pub build_duration_secs: u64,
    pub built_at: String,
}

/// Computed package output with CAS references.
#[derive(Debug, Clone)]
pub struct PackageOutput {
    pub manifest: OutputManifest,
    /// Raw TOML bytes of the manifest (stored in CAS).
    pub manifest_bytes: Vec<u8>,
    /// CAS hash of the serialized manifest.
    pub manifest_hash: String,
}

impl OutputManifest {
    /// Compute output_hash: SHA-256 of sorted file hashes + symlink targets.
    pub fn compute_output_hash(files: &[OutputFile], symlinks: &[OutputSymlink]) -> String {
        let mut hasher = Sha256::new();
        let mut sorted_files = files.to_vec();
        sorted_files.sort_by(|a, b| a.path.cmp(&b.path));
        for f in &sorted_files {
            hasher.update(format!("file:{}:{}\n", f.path, f.hash).as_bytes());
        }
        let mut sorted_symlinks = symlinks.to_vec();
        sorted_symlinks.sort_by(|a, b| a.path.cmp(&b.path));
        for s in &sorted_symlinks {
            hasher.update(format!("symlink:{}:{}\n", s.path, s.target).as_bytes());
        }
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_hash_is_deterministic() {
        let files = vec![
            OutputFile { path: "usr/bin/foo".into(), hash: "aaa".into(), size: 100, mode: 0o755 },
            OutputFile { path: "usr/lib/libfoo.so".into(), hash: "bbb".into(), size: 200, mode: 0o755 },
        ];
        let symlinks = vec![
            OutputSymlink { path: "usr/lib/libfoo.so.1".into(), target: "libfoo.so".into() },
        ];
        let h1 = OutputManifest::compute_output_hash(&files, &symlinks);
        let h2 = OutputManifest::compute_output_hash(&files, &symlinks);
        assert_eq!(h1, h2);
    }

    #[test]
    fn output_hash_is_order_independent() {
        let files_a = vec![
            OutputFile { path: "b".into(), hash: "bbb".into(), size: 1, mode: 0o644 },
            OutputFile { path: "a".into(), hash: "aaa".into(), size: 1, mode: 0o644 },
        ];
        let files_b = vec![
            OutputFile { path: "a".into(), hash: "aaa".into(), size: 1, mode: 0o644 },
            OutputFile { path: "b".into(), hash: "bbb".into(), size: 1, mode: 0o644 },
        ];
        assert_eq!(
            OutputManifest::compute_output_hash(&files_a, &[]),
            OutputManifest::compute_output_hash(&files_b, &[]),
        );
    }

    #[test]
    fn output_hash_changes_with_different_content() {
        let files1 = vec![
            OutputFile { path: "a".into(), hash: "aaa".into(), size: 1, mode: 0o644 },
        ];
        let files2 = vec![
            OutputFile { path: "a".into(), hash: "bbb".into(), size: 1, mode: 0o644 },
        ];
        assert_ne!(
            OutputManifest::compute_output_hash(&files1, &[]),
            OutputManifest::compute_output_hash(&files2, &[]),
        );
    }

    #[test]
    fn manifest_serializes_to_toml() {
        let manifest = OutputManifest {
            derivation_id: "abc123".into(),
            output_hash: "def456".into(),
            files: vec![OutputFile { path: "usr/bin/test".into(), hash: "fff".into(), size: 42, mode: 0o755 }],
            symlinks: vec![],
            build_duration_secs: 10,
            built_at: "2026-03-19T00:00:00Z".into(),
        };
        let toml_str = toml::to_string_pretty(&manifest).unwrap();
        assert!(toml_str.contains("derivation_id"));
        assert!(toml_str.contains("abc123"));
    }
}
```

- [ ] **Step 7: Run all derivation tests**

Run: `cargo test -p conary-core derivation -- --nocapture`
Expected: All tests pass (id + output).

- [ ] **Step 8: Commit**

```bash
git add conary-core/src/derivation/ conary-core/src/lib.rs
git commit -m "feat(derivation): add DerivationId, SourceDerivationId, and PackageOutput types

Canonical derivation ID computation per spec Section 1.1:
CONARY-DERIVATION-V1 format with sorted deps and options.
SourceDerivationId excludes build_env_hash for cross-seed verification."
```

---

## Task 2: Recipe Hashing

**Files:**
- Create: `conary-core/src/derivation/recipe_hash.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write failing tests for recipe hashing**

Create `conary-core/src/derivation/recipe_hash.rs`:
```rust
// conary-core/src/derivation/recipe_hash.rs
use sha2::{Digest, Sha256};
use crate::recipe::format::{BuildSection, Recipe};

/// Compute the build_script_hash for a recipe.
/// Covers: configure, make, install, check, environment, workdir, script_file.
/// Variables are expanded before hashing.
pub fn build_script_hash(recipe: &Recipe) -> String {
    let sections = collect_build_sections(&recipe.build, &recipe.variables);
    let mut hasher = Sha256::new();
    for (key, value) in &sections {
        hasher.update(format!("{}:{}\n", key, value).as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Collect all hashable build sections in deterministic order.
/// Variables (%(name)s) are expanded before collection.
fn collect_build_sections(
    build: &BuildSection,
    variables: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let expand = |s: &str| expand_variables(s, variables);

    // Deterministic key order
    if let Some(ref v) = build.configure { sections.push(("configure".into(), expand(v))); }
    if let Some(ref v) = build.make { sections.push(("make".into(), expand(v))); }
    if let Some(ref v) = build.install { sections.push(("install".into(), expand(v))); }
    if let Some(ref v) = build.check { sections.push(("check".into(), expand(v))); }

    sections
}

/// Expand %(name)s variables in a string.
fn expand_variables(s: &str, vars: &std::collections::HashMap<String, String>) -> String {
    let mut result = s.to_string();
    for (key, value) in vars {
        let pattern = format!("%({})", key) + "s";
        result = result.replace(&pattern, value);
    }
    result
}

/// Compute source_hash: SHA-256 of the source archive checksum string.
pub fn source_hash(recipe: &Recipe) -> String {
    let mut hasher = Sha256::new();
    hasher.update(recipe.source.checksum.as_bytes());
    // Include additional source checksums if present
    if let Some(ref additional) = recipe.source.additional {
        for src in additional {
            hasher.update(src.checksum.as_bytes());
        }
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_build_section(configure: &str, make: &str, install: &str) -> BuildSection {
        BuildSection {
            configure: Some(configure.into()),
            make: Some(make.into()),
            install: Some(install.into()),
            check: None,
            requires: vec![],
            makedepends: vec![],
            chroot: false,
            ..Default::default()
        }
    }

    #[test]
    fn same_recipe_same_hash() {
        let build = make_build_section("./configure", "make", "make install");
        let vars = HashMap::new();
        let sections1 = collect_build_sections(&build, &vars);
        let sections2 = collect_build_sections(&build, &vars);
        assert_eq!(sections1, sections2);
    }

    #[test]
    fn different_configure_different_hash() {
        let build1 = make_build_section("./configure --prefix=/usr", "make", "make install");
        let build2 = make_build_section("./configure --prefix=/opt", "make", "make install");
        let vars = HashMap::new();
        let h1 = {
            let s = collect_build_sections(&build1, &vars);
            let mut hasher = Sha256::new();
            for (k, v) in &s { hasher.update(format!("{}:{}\n", k, v).as_bytes()); }
            hex::encode(hasher.finalize())
        };
        let h2 = {
            let s = collect_build_sections(&build2, &vars);
            let mut hasher = Sha256::new();
            for (k, v) in &s { hasher.update(format!("{}:{}\n", k, v).as_bytes()); }
            hex::encode(hasher.finalize())
        };
        assert_ne!(h1, h2);
    }

    #[test]
    fn variables_are_expanded_before_hashing() {
        let build = make_build_section("make -j%(jobs)s", "make", "make install");
        let mut vars1 = HashMap::new();
        vars1.insert("jobs".into(), "4".into());
        let mut vars2 = HashMap::new();
        vars2.insert("jobs".into(), "8".into());

        let s1 = collect_build_sections(&build, &vars1);
        let s2 = collect_build_sections(&build, &vars2);
        // "make -j4" != "make -j8"
        assert_ne!(s1, s2);
    }

    #[test]
    fn expand_variables_works() {
        let mut vars = HashMap::new();
        vars.insert("version".into(), "1.3.2".into());
        vars.insert("jobs".into(), "4".into());
        let result = expand_variables("make -j%(jobs)s VERSION=%(version)s", &vars);
        assert_eq!(result, "make -j4 VERSION=1.3.2");
    }
}
```

Add `pub mod recipe_hash;` and `pub use recipe_hash::{build_script_hash, source_hash};` to `mod.rs`.

- [ ] **Step 2: Run tests — may need to check BuildSection fields**

Run: `cargo test -p conary-core derivation::recipe_hash --no-run 2>&1 | head -30`

If `BuildSection` doesn't have a `Default` impl or the field names differ, read `conary-core/src/recipe/format.rs` to find the actual field names and fix the test helper. The key fields to check: `configure`, `make`, `install`, `check` — confirm they are `Option<String>`.

- [ ] **Step 3: Fix any field name mismatches and get tests passing**

Run: `cargo test -p conary-core derivation::recipe_hash -- --nocapture`
Expected: All 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/derivation/recipe_hash.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add recipe hashing for build_script_hash computation

Deterministic hash of recipe build sections with variable expansion.
Covers configure, make, install, check phases."
```

---

## Task 3: Derivation Index (SQLite)

**Files:**
- Create: `conary-core/src/derivation/index.rs`
- Modify: `conary-core/src/db/schema.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Add v54 migration to schema.rs**

Read `conary-core/src/db/schema.rs` to find the migration pattern (look for the highest version block). Add after the last migration block:

```rust
// v54: Derivation index for CAS-layered bootstrap
if current_version < 54 {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS derivation_index (
            derivation_id TEXT PRIMARY KEY,
            output_hash TEXT NOT NULL,
            package_name TEXT NOT NULL,
            package_version TEXT NOT NULL,
            manifest_cas_hash TEXT NOT NULL,
            stage TEXT,
            build_env_hash TEXT,
            built_at TEXT NOT NULL,
            build_duration_secs INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_derivation_package
            ON derivation_index(package_name, package_version);
        CREATE INDEX IF NOT EXISTS idx_derivation_output
            ON derivation_index(output_hash);
        INSERT OR REPLACE INTO schema_version (version, applied_at)
            VALUES (54, datetime('now'));",
    )?;
}
```

Update `SCHEMA_VERSION` to `54`.

- [ ] **Step 2: Write failing tests for DerivationIndex**

Create `conary-core/src/derivation/index.rs`:
```rust
// conary-core/src/derivation/index.rs
use rusqlite::Connection;
use crate::derivation::output::OutputManifest;

/// SQLite-backed mapping from derivation_id -> output metadata.
pub struct DerivationIndex<'a> {
    conn: &'a Connection,
}

#[derive(Debug, Clone)]
pub struct DerivationRecord {
    pub derivation_id: String,
    pub output_hash: String,
    pub package_name: String,
    pub package_version: String,
    pub manifest_cas_hash: String,
    pub stage: Option<String>,
    pub build_env_hash: Option<String>,
    pub built_at: String,
    pub build_duration_secs: u64,
}

impl<'a> DerivationIndex<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Check if a derivation has been built before.
    pub fn lookup(&self, derivation_id: &str) -> Result<Option<DerivationRecord>, rusqlite::Error> {
        todo!()
    }

    /// Record a completed derivation build.
    pub fn insert(&self, record: &DerivationRecord) -> Result<(), rusqlite::Error> {
        todo!()
    }

    /// List all derivations for a package.
    pub fn by_package(&self, name: &str) -> Result<Vec<DerivationRecord>, rusqlite::Error> {
        todo!()
    }

    /// Remove a derivation record.
    pub fn remove(&self, derivation_id: &str) -> Result<bool, rusqlite::Error> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE derivation_index (
                derivation_id TEXT PRIMARY KEY,
                output_hash TEXT NOT NULL,
                package_name TEXT NOT NULL,
                package_version TEXT NOT NULL,
                manifest_cas_hash TEXT NOT NULL,
                stage TEXT,
                build_env_hash TEXT,
                built_at TEXT NOT NULL,
                build_duration_secs INTEGER NOT NULL
            );"
        ).unwrap();
        conn
    }

    fn sample_record() -> DerivationRecord {
        DerivationRecord {
            derivation_id: "abc123".into(),
            output_hash: "def456".into(),
            package_name: "zlib".into(),
            package_version: "1.3.2".into(),
            manifest_cas_hash: "manifest_hash".into(),
            stage: Some("system".into()),
            build_env_hash: Some("env_hash".into()),
            built_at: "2026-03-19T00:00:00Z".into(),
            build_duration_secs: 10,
        }
    }

    #[test]
    fn lookup_returns_none_for_missing() {
        let conn = setup_db();
        let idx = DerivationIndex::new(&conn);
        assert!(idx.lookup("nonexistent").unwrap().is_none());
    }

    #[test]
    fn insert_then_lookup() {
        let conn = setup_db();
        let idx = DerivationIndex::new(&conn);
        let record = sample_record();
        idx.insert(&record).unwrap();
        let found = idx.lookup("abc123").unwrap().unwrap();
        assert_eq!(found.output_hash, "def456");
        assert_eq!(found.package_name, "zlib");
    }

    #[test]
    fn by_package_returns_matching() {
        let conn = setup_db();
        let idx = DerivationIndex::new(&conn);
        idx.insert(&sample_record()).unwrap();
        let mut other = sample_record();
        other.derivation_id = "xyz789".into();
        other.package_name = "openssl".into();
        idx.insert(&other).unwrap();
        let results = idx.by_package("zlib").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].derivation_id, "abc123");
    }

    #[test]
    fn remove_deletes_record() {
        let conn = setup_db();
        let idx = DerivationIndex::new(&conn);
        idx.insert(&sample_record()).unwrap();
        assert!(idx.remove("abc123").unwrap());
        assert!(idx.lookup("abc123").unwrap().is_none());
    }

    #[test]
    fn remove_returns_false_for_missing() {
        let conn = setup_db();
        let idx = DerivationIndex::new(&conn);
        assert!(!idx.remove("nonexistent").unwrap());
    }
}
```

Add `pub mod index;` and `pub use index::{DerivationIndex, DerivationRecord};` to `mod.rs`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p conary-core derivation::index --no-run 2>&1 | head -10`
Expected: Fails on `todo!()` calls.

- [ ] **Step 4: Implement DerivationIndex methods**

Replace the `todo!()` calls:

```rust
impl<'a> DerivationIndex<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn lookup(&self, derivation_id: &str) -> Result<Option<DerivationRecord>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT derivation_id, output_hash, package_name, package_version,
                    manifest_cas_hash, stage, build_env_hash, built_at, build_duration_secs
             FROM derivation_index WHERE derivation_id = ?1"
        )?;
        let result = stmt.query_row([derivation_id], |row| {
            Ok(DerivationRecord {
                derivation_id: row.get(0)?,
                output_hash: row.get(1)?,
                package_name: row.get(2)?,
                package_version: row.get(3)?,
                manifest_cas_hash: row.get(4)?,
                stage: row.get(5)?,
                build_env_hash: row.get(6)?,
                built_at: row.get(7)?,
                build_duration_secs: row.get::<_, i64>(8)? as u64,
            })
        });
        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn insert(&self, record: &DerivationRecord) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT OR REPLACE INTO derivation_index
             (derivation_id, output_hash, package_name, package_version,
              manifest_cas_hash, stage, build_env_hash, built_at, build_duration_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                record.derivation_id,
                record.output_hash,
                record.package_name,
                record.package_version,
                record.manifest_cas_hash,
                record.stage,
                record.build_env_hash,
                record.built_at,
                record.build_duration_secs as i64,
            ],
        )?;
        Ok(())
    }

    pub fn by_package(&self, name: &str) -> Result<Vec<DerivationRecord>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT derivation_id, output_hash, package_name, package_version,
                    manifest_cas_hash, stage, build_env_hash, built_at, build_duration_secs
             FROM derivation_index WHERE package_name = ?1"
        )?;
        let rows = stmt.query_map([name], |row| {
            Ok(DerivationRecord {
                derivation_id: row.get(0)?,
                output_hash: row.get(1)?,
                package_name: row.get(2)?,
                package_version: row.get(3)?,
                manifest_cas_hash: row.get(4)?,
                stage: row.get(5)?,
                build_env_hash: row.get(6)?,
                built_at: row.get(7)?,
                build_duration_secs: row.get::<_, i64>(8)? as u64,
            })
        })?;
        rows.collect()
    }

    pub fn remove(&self, derivation_id: &str) -> Result<bool, rusqlite::Error> {
        let changes = self.conn.execute(
            "DELETE FROM derivation_index WHERE derivation_id = ?1",
            [derivation_id],
        )?;
        Ok(changes > 0)
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p conary-core derivation::index -- --nocapture`
Expected: All 5 tests pass.

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/derivation/index.rs conary-core/src/derivation/mod.rs conary-core/src/db/schema.rs
git commit -m "feat(derivation): add SQLite derivation index (schema v54)

Persistent derivation_id -> output_hash mapping.
Supports lookup, insert, by_package, and remove operations."
```

---

## Task 4: Output Capture (DESTDIR -> CAS)

**Files:**
- Create: `conary-core/src/derivation/capture.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write failing tests for capture**

Create `conary-core/src/derivation/capture.rs`:
```rust
// conary-core/src/derivation/capture.rs
use std::path::Path;
use crate::filesystem::cas::CasStore;
use crate::derivation::output::{OutputFile, OutputManifest, OutputSymlink};

/// Walk a DESTDIR, ingest every file into CAS, return an OutputManifest.
pub fn capture_output(
    destdir: &Path,
    cas: &CasStore,
    derivation_id: &str,
    build_duration_secs: u64,
) -> Result<OutputManifest, CaptureError> {
    let mut files = Vec::new();
    let mut symlinks = Vec::new();

    walk_destdir(destdir, destdir, &mut files, &mut symlinks, cas)?;

    let output_hash = OutputManifest::compute_output_hash(&files, &symlinks);
    let built_at = chrono::Utc::now().to_rfc3339();

    Ok(OutputManifest {
        derivation_id: derivation_id.to_string(),
        output_hash,
        files,
        symlinks,
        build_duration_secs,
        built_at,
    })
}

fn walk_destdir(
    root: &Path,
    current: &Path,
    files: &mut Vec<OutputFile>,
    symlinks: &mut Vec<OutputSymlink>,
    cas: &CasStore,
) -> Result<(), CaptureError> {
    for entry in std::fs::read_dir(current).map_err(|e| CaptureError::Io(e.to_string()))? {
        let entry = entry.map_err(|e| CaptureError::Io(e.to_string()))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|e| CaptureError::Io(e.to_string()))?;
        let relative = path.strip_prefix(root)
            .map_err(|e| CaptureError::Io(e.to_string()))?
            .to_string_lossy()
            .to_string();

        if metadata.is_symlink() {
            let target = std::fs::read_link(&path)
                .map_err(|e| CaptureError::Io(e.to_string()))?;
            symlinks.push(OutputSymlink {
                path: relative,
                target: target.to_string_lossy().to_string(),
            });
        } else if metadata.is_dir() {
            walk_destdir(root, &path, files, symlinks, cas)?;
        } else if metadata.is_file() {
            let content = std::fs::read(&path)
                .map_err(|e| CaptureError::Io(e.to_string()))?;
            let hash = cas.store(&content)
                .map_err(|e| CaptureError::Cas(e.to_string()))?;
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode()
            };
            #[cfg(not(unix))]
            let mode = 0o644;

            files.push(OutputFile {
                path: relative,
                hash,
                size: metadata.len(),
                mode,
            });
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("I/O error: {0}")]
    Io(String),
    #[error("CAS error: {0}")]
    Cas(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, TempDir, CasStore) {
        let cas_dir = TempDir::new().unwrap();
        let cas = CasStore::new(cas_dir.path());
        let destdir = TempDir::new().unwrap();
        (cas_dir, destdir, cas)
    }

    #[test]
    fn captures_files_to_cas() {
        let (_cas_dir, destdir, cas) = setup();
        let usr_bin = destdir.path().join("usr/bin");
        std::fs::create_dir_all(&usr_bin).unwrap();
        std::fs::write(usr_bin.join("hello"), b"#!/bin/sh\necho hello").unwrap();

        let manifest = capture_output(destdir.path(), &cas, "test_drv", 1).unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path, "usr/bin/hello");
        assert!(cas.exists(&manifest.files[0].hash));
    }

    #[test]
    fn captures_symlinks() {
        let (_cas_dir, destdir, cas) = setup();
        let usr_lib = destdir.path().join("usr/lib");
        std::fs::create_dir_all(&usr_lib).unwrap();
        std::fs::write(usr_lib.join("libfoo.so.1"), b"ELF").unwrap();
        std::os::unix::fs::symlink("libfoo.so.1", usr_lib.join("libfoo.so")).unwrap();

        let manifest = capture_output(destdir.path(), &cas, "test_drv", 1).unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.symlinks.len(), 1);
        assert_eq!(manifest.symlinks[0].target, "libfoo.so.1");
    }

    #[test]
    fn output_hash_is_set() {
        let (_cas_dir, destdir, cas) = setup();
        std::fs::create_dir_all(destdir.path().join("usr")).unwrap();
        std::fs::write(destdir.path().join("usr/test"), b"data").unwrap();

        let manifest = capture_output(destdir.path(), &cas, "test_drv", 5).unwrap();
        assert!(!manifest.output_hash.is_empty());
        assert_eq!(manifest.output_hash.len(), 64);
    }

    #[test]
    fn empty_destdir_produces_empty_manifest() {
        let (_cas_dir, destdir, cas) = setup();
        let manifest = capture_output(destdir.path(), &cas, "test_drv", 0).unwrap();
        assert!(manifest.files.is_empty());
        assert!(manifest.symlinks.is_empty());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core derivation::capture -- --nocapture`
Expected: All 4 tests pass (or fix compilation issues with CasStore API — check if `store()` returns `Result<String, _>` and adjust).

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/capture.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add output capture (DESTDIR walk -> CAS ingest)

Walks a DESTDIR, ingests all files into CAS via CasStore.store(),
captures symlinks, and produces an OutputManifest with computed output_hash."
```

---

## Task 5: EROFS Composition

**Files:**
- Create: `conary-core/src/derivation/compose.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write compose function with tests**

Create `conary-core/src/derivation/compose.rs`:
```rust
// conary-core/src/derivation/compose.rs
use std::path::Path;
use crate::derivation::output::{OutputManifest, OutputFile, OutputSymlink};
use crate::generation::builder::{BuildResult, FileEntryRef};

/// Compose multiple package outputs into a single list of FileEntryRef
/// suitable for build_erofs_image().
///
/// Handles file conflicts by last-writer-wins (later manifests override earlier).
/// This matches the stage composition model: each package's files overlay the base.
pub fn compose_file_entries(manifests: &[&OutputManifest]) -> Vec<FileEntryRef> {
    use std::collections::BTreeMap;

    // BTreeMap for deterministic iteration order
    let mut entries: BTreeMap<String, FileEntryRef> = BTreeMap::new();

    for manifest in manifests {
        for file in &manifest.files {
            entries.insert(
                file.path.clone(),
                FileEntryRef {
                    path: format!("/{}", file.path),  // FileEntryRef uses absolute paths
                    sha256_hash: file.hash.clone(),
                    size: file.size,
                    permissions: file.mode,
                },
            );
        }
    }

    entries.into_values().collect()
}

/// Compose multiple package outputs into an EROFS image.
/// Returns the BuildResult with image path and stats.
pub fn compose_erofs(
    manifests: &[&OutputManifest],
    output_dir: &Path,
) -> Result<BuildResult, ComposeError> {
    let entries = compose_file_entries(manifests);
    if entries.is_empty() {
        return Err(ComposeError::EmptyComposition);
    }

    crate::generation::builder::build_erofs_image(&entries, output_dir)
        .map_err(|e| ComposeError::Erofs(e.to_string()))
}

/// Compute the EROFS image hash (build_env_hash for derivation inputs).
/// SHA-256 of the image file bytes.
pub fn erofs_image_hash(image_path: &Path) -> Result<String, ComposeError> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(image_path)
        .map_err(|e| ComposeError::Io(e.to_string()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

#[derive(Debug, thiserror::Error)]
pub enum ComposeError {
    #[error("empty composition: no package outputs to compose")]
    EmptyComposition,
    #[error("EROFS build error: {0}")]
    Erofs(String),
    #[error("I/O error: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derivation::output::{OutputFile, OutputManifest, OutputSymlink};

    fn make_manifest(id: &str, files: Vec<(&str, &str)>) -> OutputManifest {
        let output_files: Vec<OutputFile> = files.iter().map(|(path, hash)| {
            OutputFile {
                path: path.to_string(),
                hash: hash.to_string(),
                size: 100,
                mode: 0o755,
            }
        }).collect();

        OutputManifest {
            derivation_id: id.into(),
            output_hash: "unused".into(),
            files: output_files,
            symlinks: vec![],
            build_duration_secs: 0,
            built_at: "2026-03-19T00:00:00Z".into(),
        }
    }

    #[test]
    fn compose_merges_files_from_multiple_outputs() {
        let m1 = make_manifest("d1", vec![("usr/bin/a", "hash_a")]);
        let m2 = make_manifest("d2", vec![("usr/bin/b", "hash_b")]);
        let entries = compose_file_entries(&[&m1, &m2]);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn compose_last_writer_wins_on_conflict() {
        let m1 = make_manifest("d1", vec![("usr/bin/foo", "old_hash")]);
        let m2 = make_manifest("d2", vec![("usr/bin/foo", "new_hash")]);
        let entries = compose_file_entries(&[&m1, &m2]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sha256_hash, "new_hash");
    }

    #[test]
    fn compose_produces_absolute_paths() {
        let m = make_manifest("d1", vec![("usr/bin/test", "hash")]);
        let entries = compose_file_entries(&[&m]);
        assert!(entries[0].path.starts_with('/'));
    }

    #[test]
    fn compose_is_deterministic() {
        let m1 = make_manifest("d1", vec![("usr/bin/z", "hz"), ("usr/bin/a", "ha")]);
        let entries1 = compose_file_entries(&[&m1]);
        let entries2 = compose_file_entries(&[&m1]);
        assert_eq!(entries1.len(), entries2.len());
        for (a, b) in entries1.iter().zip(entries2.iter()) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.sha256_hash, b.sha256_hash);
        }
    }

    #[test]
    fn empty_composition_returns_error() {
        let result = compose_erofs(&[], std::path::Path::new("/tmp"));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core derivation::compose -- --nocapture`
Expected: All 5 tests pass. The `compose_erofs` and `erofs_image_hash` functions depend on the EROFS builder, so the unit test for empty composition should work. Full integration tested in Task 9.

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/compose.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add EROFS composition from package outputs

Merges multiple OutputManifests into FileEntryRef list for build_erofs_image.
Last-writer-wins on path conflicts. Deterministic output ordering."
```

---

## Task 6: Build Environment (composefs mount lifecycle)

**Files:**
- Create: `conary-core/src/derivation/environment.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write BuildEnvironment struct with tests**

Create `conary-core/src/derivation/environment.rs`:
```rust
// conary-core/src/derivation/environment.rs
use std::path::{Path, PathBuf};

/// A mounted composefs build environment.
/// Manages the lifecycle of a composefs mount used as a build sysroot.
///
/// The build environment is a read-only composefs mount of an EROFS image
/// backed by CAS objects. No host filesystem paths are exposed.
pub struct BuildEnvironment {
    pub mount_point: PathBuf,
    pub image_path: PathBuf,
    pub cas_dir: PathBuf,
    pub build_env_hash: String,
    mounted: bool,
}

impl BuildEnvironment {
    /// Create a new build environment (not yet mounted).
    pub fn new(
        image_path: PathBuf,
        cas_dir: PathBuf,
        mount_point: PathBuf,
        build_env_hash: String,
    ) -> Self {
        Self {
            mount_point,
            image_path,
            cas_dir,
            build_env_hash,
            mounted: false,
        }
    }

    /// Mount the composefs build environment.
    /// Requires root or appropriate capabilities for mount syscall.
    pub fn mount(&mut self) -> Result<(), EnvironmentError> {
        if self.mounted {
            return Ok(());
        }
        std::fs::create_dir_all(&self.mount_point)
            .map_err(|e| EnvironmentError::Mount(format!("create mount point: {}", e)))?;

        // Use the generation mount infrastructure
        let opts = crate::generation::mount::MountOptions {
            image_path: self.image_path.clone(),
            basedir: self.cas_dir.clone(),
            mount_point: self.mount_point.clone(),
            verity: false,
            digest: None,
            upperdir: None,
            workdir: None,
        };
        crate::generation::mount::mount_generation(&opts)
            .map_err(|e| EnvironmentError::Mount(e.to_string()))?;

        self.mounted = true;
        Ok(())
    }

    /// Unmount the build environment.
    pub fn unmount(&mut self) -> Result<(), EnvironmentError> {
        if !self.mounted {
            return Ok(());
        }
        nix::mount::umount(&self.mount_point)
            .map_err(|e| EnvironmentError::Unmount(e.to_string()))?;
        self.mounted = false;
        Ok(())
    }

    pub fn is_mounted(&self) -> bool {
        self.mounted
    }
}

impl Drop for BuildEnvironment {
    fn drop(&mut self) {
        if self.mounted {
            let _ = self.unmount();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("mount failed: {0}")]
    Mount(String),
    #[error("unmount failed: {0}")]
    Unmount(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_environment_is_not_mounted() {
        let env = BuildEnvironment::new(
            PathBuf::from("/tmp/test.erofs"),
            PathBuf::from("/tmp/cas"),
            PathBuf::from("/tmp/mnt"),
            "hash123".into(),
        );
        assert!(!env.is_mounted());
    }

    #[test]
    fn build_env_hash_is_accessible() {
        let env = BuildEnvironment::new(
            PathBuf::from("/tmp/test.erofs"),
            PathBuf::from("/tmp/cas"),
            PathBuf::from("/tmp/mnt"),
            "abc_env_hash".into(),
        );
        assert_eq!(env.build_env_hash, "abc_env_hash");
    }
}
```

Note: `mount()` and `unmount()` require root. Full integration testing happens in Task 9 (which will need root or CI).

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core derivation::environment -- --nocapture`
Expected: Unit tests pass (they don't call mount/unmount).

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/environment.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add BuildEnvironment composefs mount lifecycle

Manages composefs mount/unmount for build sysroots.
Drop-safe unmount. Uses existing generation::mount infrastructure."
```

---

## Task 7: Derivation Executor

**Files:**
- Create: `conary-core/src/derivation/executor.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write DerivationExecutor struct**

Create `conary-core/src/derivation/executor.rs`:
```rust
// conary-core/src/derivation/executor.rs
use std::path::{Path, PathBuf};
use std::time::Instant;
use crate::derivation::id::{DerivationId, DerivationInputs};
use crate::derivation::output::{OutputManifest, PackageOutput};
use crate::derivation::capture::{capture_output, CaptureError};
use crate::derivation::index::{DerivationIndex, DerivationRecord};
use crate::derivation::recipe_hash;
use crate::filesystem::cas::CasStore;
use crate::recipe::format::Recipe;
use crate::recipe::kitchen::{Kitchen, KitchenConfig};
use rusqlite::Connection;

/// Executes a single derivation: compute ID, check cache, build, capture.
pub struct DerivationExecutor {
    cas: CasStore,
    cas_dir: PathBuf,
}

/// Result of executing a derivation.
pub enum ExecutionResult {
    /// Cache hit — derivation was already built.
    CacheHit {
        derivation_id: DerivationId,
        record: DerivationRecord,
    },
    /// Freshly built.
    Built {
        derivation_id: DerivationId,
        output: PackageOutput,
    },
}

impl DerivationExecutor {
    pub fn new(cas: CasStore, cas_dir: PathBuf) -> Self {
        Self { cas, cas_dir }
    }

    /// Execute a derivation: compute ID, check cache, build if needed, capture output.
    pub fn execute(
        &self,
        recipe: &Recipe,
        build_env_hash: &str,
        dep_ids: &std::collections::BTreeMap<String, DerivationId>,
        target_triple: &str,
        sysroot: &Path,
        conn: &Connection,
    ) -> Result<ExecutionResult, ExecutorError> {
        // 1. Compute derivation ID
        let inputs = DerivationInputs {
            source_hash: recipe_hash::source_hash(recipe),
            build_script_hash: recipe_hash::build_script_hash(recipe),
            dependency_ids: dep_ids.clone(),
            build_env_hash: build_env_hash.to_string(),
            target_triple: target_triple.to_string(),
            build_options: std::collections::BTreeMap::new(),
        };
        let derivation_id = DerivationId::compute(&inputs);

        // 2. Check cache
        let index = DerivationIndex::new(conn);
        if let Some(record) = index.lookup(derivation_id.as_str())
            .map_err(|e| ExecutorError::Index(e.to_string()))?
        {
            return Ok(ExecutionResult::CacheHit { derivation_id, record });
        }

        // 3. Build
        let start = Instant::now();
        let output_dir = tempfile::tempdir()
            .map_err(|e| ExecutorError::Io(e.to_string()))?;

        let config = KitchenConfig::for_bootstrap(sysroot);
        let kitchen = Kitchen::new(config);
        let cook_result = kitchen.new_cook_with_dest(recipe, output_dir.path())
            .cook()
            .map_err(|e| ExecutorError::Build(e.to_string()))?;

        let duration = start.elapsed().as_secs();

        // 4. Capture output to CAS
        let manifest = capture_output(
            output_dir.path(),
            &self.cas,
            derivation_id.as_str(),
            duration,
        ).map_err(ExecutorError::Capture)?;

        // 5. Store manifest in CAS
        let manifest_bytes = toml::to_string_pretty(&manifest)
            .map_err(|e| ExecutorError::Io(e.to_string()))?
            .into_bytes();
        let manifest_hash = self.cas.store(&manifest_bytes)
            .map_err(|e| ExecutorError::Cas(e.to_string()))?;

        // 6. Record in derivation index
        let record = DerivationRecord {
            derivation_id: derivation_id.as_str().to_string(),
            output_hash: manifest.output_hash.clone(),
            package_name: recipe.package.name.clone(),
            package_version: recipe.package.version.clone(),
            manifest_cas_hash: manifest_hash.clone(),
            stage: None,
            build_env_hash: Some(build_env_hash.to_string()),
            built_at: manifest.built_at.clone(),
            build_duration_secs: duration,
        };
        index.insert(&record)
            .map_err(|e| ExecutorError::Index(e.to_string()))?;

        let output = PackageOutput {
            manifest,
            manifest_bytes,
            manifest_hash,
        };

        Ok(ExecutionResult::Built { derivation_id, output })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("build failed: {0}")]
    Build(String),
    #[error("output capture failed: {0}")]
    Capture(#[from] CaptureError),
    #[error("CAS error: {0}")]
    Cas(String),
    #[error("derivation index error: {0}")]
    Index(String),
    #[error("I/O error: {0}")]
    Io(String),
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p conary-core 2>&1 | tail -20`

This task connects all previous tasks. Fix any API mismatches (Kitchen::new_cook_with_dest may have a different signature — read `conary-core/src/recipe/kitchen/cook.rs` and adjust). The executor struct may need adjustments based on actual Kitchen API.

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/executor.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add DerivationExecutor (compute -> cache check -> build -> capture)

Single-package build executor wiring together derivation ID computation,
cache lookup, Kitchen.cook, CAS capture, and index recording."
```

---

## Task 8: Seed Model

**Files:**
- Create: `conary-core/src/derivation/seed.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write Seed types with tests**

Create `conary-core/src/derivation/seed.rs`:
```rust
// conary-core/src/derivation/seed.rs
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Seed metadata — Layer 0 provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedMetadata {
    pub seed_id: String,
    pub source: SeedSource,
    pub origin_url: Option<String>,
    pub builder: Option<String>,
    pub packages: Vec<String>,
    pub target_triple: String,
    pub verified_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SeedSource {
    Community,
    Imported,
    SelfBuilt,
}

/// A loaded seed ready to be used as Layer 0.
pub struct Seed {
    pub metadata: SeedMetadata,
    pub image_path: PathBuf,
    pub cas_dir: PathBuf,
}

impl Seed {
    /// Load a seed from a local directory containing seed.erofs + seed.toml.
    pub fn load_local(seed_dir: &Path) -> Result<Self, SeedError> {
        let metadata_path = seed_dir.join("seed.toml");
        let image_path = seed_dir.join("seed.erofs");

        if !image_path.exists() {
            return Err(SeedError::MissingImage(image_path.display().to_string()));
        }
        if !metadata_path.exists() {
            return Err(SeedError::MissingMetadata(metadata_path.display().to_string()));
        }

        let metadata_str = std::fs::read_to_string(&metadata_path)
            .map_err(|e| SeedError::Io(e.to_string()))?;
        let metadata: SeedMetadata = toml::from_str(&metadata_str)
            .map_err(|e| SeedError::Parse(e.to_string()))?;

        // Verify seed_id matches image hash
        let actual_hash = crate::derivation::compose::erofs_image_hash(&image_path)
            .map_err(|e| SeedError::Io(e.to_string()))?;
        if actual_hash != metadata.seed_id {
            return Err(SeedError::HashMismatch {
                expected: metadata.seed_id.clone(),
                actual: actual_hash,
            });
        }

        Ok(Self {
            metadata,
            image_path,
            cas_dir: seed_dir.join("objects"),
        })
    }

    /// The build_env_hash for toolchain-stage derivations building against this seed.
    pub fn build_env_hash(&self) -> &str {
        &self.metadata.seed_id
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    #[error("seed image not found: {0}")]
    MissingImage(String),
    #[error("seed metadata not found: {0}")]
    MissingMetadata(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("metadata parse error: {0}")]
    Parse(String),
    #[error("seed hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_source_serializes() {
        let meta = SeedMetadata {
            seed_id: "abc123".into(),
            source: SeedSource::Community,
            origin_url: Some("https://seeds.conary.io/x86_64/2026Q1".into()),
            builder: Some("conary 0.9.0".into()),
            packages: vec!["gcc-15.2.0".into()],
            target_triple: "x86_64-conary-linux-gnu".into(),
            verified_by: vec![],
        };
        let toml_str = toml::to_string_pretty(&meta).unwrap();
        assert!(toml_str.contains("source = \"community\""));
    }

    #[test]
    fn load_local_fails_on_missing_dir() {
        let result = Seed::load_local(Path::new("/nonexistent/seed"));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core derivation::seed -- --nocapture`
Expected: Both tests pass.

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/seed.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add Seed model (Layer 0 loading and verification)

SeedMetadata with provenance tracking. Local seed loading with
EROFS image hash verification against seed_id."
```

---

## Task 9: System Manifest Parsing

**Files:**
- Create: `conary-core/src/derivation/manifest.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write SystemManifest types with tests**

Create `conary-core/src/derivation/manifest.rs`:
```rust
// conary-core/src/derivation/manifest.rs
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// User-facing system declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemManifest {
    pub system: SystemSection,
    pub seed: SeedReference,
    pub packages: PackageSelection,
    pub kernel: Option<KernelSection>,
    pub customization: Option<CustomizationSection>,
    pub substituters: Option<SubstituterSection>,
    pub integrity: Option<IntegritySection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSection {
    pub name: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedReference {
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageSelection {
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSection {
    pub config: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomizationSection {
    #[serde(default)]
    pub layers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubstituterSection {
    pub sources: Vec<String>,
    #[serde(default = "default_trust")]
    pub trust: String,
}

fn default_trust() -> String { "derivation".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegritySection {
    #[serde(default)]
    pub fsverity: bool,
    #[serde(default)]
    pub erofs_digest: bool,
}

impl SystemManifest {
    /// Load a system manifest from a TOML file.
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ManifestError::Io(e.to_string()))?;
        Self::parse(&content)
    }

    /// Parse a system manifest from a TOML string.
    pub fn parse(content: &str) -> Result<Self, ManifestError> {
        toml::from_str(content)
            .map_err(|e| ManifestError::Parse(e.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("I/O error: {0}")]
    Io(String),
    #[error("TOML parse error: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_MANIFEST: &str = r#"
[system]
name = "test-server"
target = "x86_64-conary-linux-gnu"

[seed]
source = "community"

[packages]
include = ["base-system", "openssh"]
"#;

    const FULL_MANIFEST: &str = r#"
[system]
name = "my-server"
target = "x86_64-conary-linux-gnu"

[seed]
source = "https://seeds.conary.io/x86_64/2026Q1"

[packages]
include = ["base-system", "openssh", "curl", "nginx"]
exclude = ["nano"]

[kernel]
config = "server"

[customization]
layers = ["./my-company-layer"]

[substituters]
sources = ["https://cache.conary.io"]
trust = "derivation"

[integrity]
fsverity = true
erofs_digest = true
"#;

    #[test]
    fn parse_minimal_manifest() {
        let manifest = SystemManifest::parse(MINIMAL_MANIFEST).unwrap();
        assert_eq!(manifest.system.name, "test-server");
        assert_eq!(manifest.seed.source, "community");
        assert_eq!(manifest.packages.include, vec!["base-system", "openssh"]);
        assert!(manifest.packages.exclude.is_empty());
        assert!(manifest.kernel.is_none());
    }

    #[test]
    fn parse_full_manifest() {
        let manifest = SystemManifest::parse(FULL_MANIFEST).unwrap();
        assert_eq!(manifest.system.name, "my-server");
        assert_eq!(manifest.packages.include.len(), 4);
        assert_eq!(manifest.packages.exclude, vec!["nano"]);
        assert_eq!(manifest.kernel.as_ref().unwrap().config, "server");
        assert_eq!(manifest.customization.as_ref().unwrap().layers.len(), 1);
        assert!(manifest.integrity.as_ref().unwrap().fsverity);
    }

    #[test]
    fn roundtrip_serialization() {
        let manifest = SystemManifest::parse(FULL_MANIFEST).unwrap();
        let toml_str = toml::to_string_pretty(&manifest).unwrap();
        let reparsed = SystemManifest::parse(&toml_str).unwrap();
        assert_eq!(manifest.system.name, reparsed.system.name);
        assert_eq!(manifest.packages.include.len(), reparsed.packages.include.len());
    }

    #[test]
    fn invalid_toml_returns_error() {
        let result = SystemManifest::parse("this is not toml {{{}");
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core derivation::manifest -- --nocapture`
Expected: All 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/manifest.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add SystemManifest TOML parser

Declarative system manifest with system, seed, packages, kernel,
customization, substituters, and integrity sections."
```

---

## Task 10: Stage Assignment Algorithm

**Files:**
- Create: `conary-core/src/derivation/stages.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write stage types and assignment tests**

Create `conary-core/src/derivation/stages.rs`:
```rust
// conary-core/src/derivation/stages.rs
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use crate::recipe::format::Recipe;

/// Build stages in the bootstrap pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    Toolchain,
    Foundation,
    System,
    Customization,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Toolchain => write!(f, "toolchain"),
            Self::Foundation => write!(f, "foundation"),
            Self::System => write!(f, "system"),
            Self::Customization => write!(f, "customization"),
        }
    }
}

/// Stage assignment for a package.
#[derive(Debug, Clone)]
pub struct StageAssignment {
    pub package: String,
    pub stage: Stage,
    pub build_order: usize,
}

/// Assign packages to stages based on dependency analysis.
pub fn assign_stages(
    recipes: &HashMap<String, Recipe>,
    custom_packages: &HashSet<String>,
) -> Result<Vec<StageAssignment>, StageError> {
    let mut assignments = Vec::new();

    // 1. Identify toolchain packages (multi-pass recipes: *-pass1, *-pass2)
    let mut toolchain_set: BTreeSet<String> = BTreeSet::new();
    let mut foundation_set: BTreeSet<String> = BTreeSet::new();

    for name in recipes.keys() {
        if name.ends_with("-pass1") || name.ends_with("-pass2") {
            toolchain_set.insert(name.clone());
        }
    }
    // linux-headers is always toolchain (needed for glibc cross-compile)
    if recipes.contains_key("linux-headers") {
        toolchain_set.insert("linux-headers".into());
    }
    // glibc and libstdcxx in toolchain if they depend on pass1 packages
    for name in ["glibc", "libstdcxx"] {
        if recipes.contains_key(name) {
            toolchain_set.insert(name.to_string());
        }
    }

    // 2. Foundation = self-hosting set (full versions of toolchain packages)
    //    These are packages with base names matching toolchain pass packages.
    for name in recipes.keys() {
        let base_name = name.trim_end_matches("-pass1").trim_end_matches("-pass2");
        if base_name != name.as_str() {
            // This is a pass recipe; its base name goes to foundation
            if recipes.contains_key(base_name) {
                foundation_set.insert(base_name.to_string());
            }
        }
    }
    // Core build tools always in foundation
    for name in ["gcc", "glibc", "binutils", "make", "bash", "coreutils",
                  "sed", "gawk", "grep", "findutils", "diffutils", "tar",
                  "gzip", "xz", "bison", "m4", "perl", "python", "gettext",
                  "texinfo", "util-linux", "ncurses", "file", "patch"] {
        if recipes.contains_key(name) && !toolchain_set.contains(name) {
            foundation_set.insert(name.to_string());
        }
    }

    // 3. Build topological order within each stage
    let toolchain_order = topological_sort(&toolchain_set, recipes)?;
    for (i, name) in toolchain_order.iter().enumerate() {
        assignments.push(StageAssignment {
            package: name.clone(),
            stage: Stage::Toolchain,
            build_order: i,
        });
    }

    let foundation_order = topological_sort(&foundation_set, recipes)?;
    for (i, name) in foundation_order.iter().enumerate() {
        assignments.push(StageAssignment {
            package: name.clone(),
            stage: Stage::Foundation,
            build_order: i,
        });
    }

    // 4. Everything else -> System or Customization
    let assigned: HashSet<String> = toolchain_set.union(&foundation_set).cloned().collect();
    let mut system_set: BTreeSet<String> = BTreeSet::new();
    let mut custom_set: BTreeSet<String> = BTreeSet::new();

    for name in recipes.keys() {
        if !assigned.contains(name) {
            if custom_packages.contains(name) {
                custom_set.insert(name.clone());
            } else {
                system_set.insert(name.clone());
            }
        }
    }

    let system_order = topological_sort(&system_set, recipes)?;
    for (i, name) in system_order.iter().enumerate() {
        assignments.push(StageAssignment {
            package: name.clone(),
            stage: Stage::System,
            build_order: i,
        });
    }

    let custom_order = topological_sort(&custom_set, recipes)?;
    for (i, name) in custom_order.iter().enumerate() {
        assignments.push(StageAssignment {
            package: name.clone(),
            stage: Stage::Customization,
            build_order: i,
        });
    }

    // 5. Apply stage hints from recipes (override automatic assignment)
    for assignment in &mut assignments {
        if let Some(recipe) = recipes.get(&assignment.package) {
            if let Some(ref stage_hint) = recipe.build.stage {
                assignment.stage = parse_stage(stage_hint)?;
            }
        }
    }

    Ok(assignments)
}

fn parse_stage(s: &str) -> Result<Stage, StageError> {
    match s {
        "toolchain" => Ok(Stage::Toolchain),
        "foundation" => Ok(Stage::Foundation),
        "system" => Ok(Stage::System),
        "customization" => Ok(Stage::Customization),
        _ => Err(StageError::InvalidStage(s.to_string())),
    }
}

/// Topological sort of packages within a set, based on makedepends.
fn topological_sort(
    packages: &BTreeSet<String>,
    recipes: &HashMap<String, Recipe>,
) -> Result<Vec<String>, StageError> {
    let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for name in packages {
        in_degree.entry(name.clone()).or_insert(0);
        if let Some(recipe) = recipes.get(name) {
            let all_deps: Vec<&String> = recipe.build.requires.iter()
                .chain(recipe.build.makedepends.iter())
                .collect();
            for dep in all_deps {
                if packages.contains(dep.as_str()) {
                    *in_degree.entry(name.clone()).or_insert(0) += 1;
                    dependents.entry(dep.clone()).or_default().push(name.clone());
                }
            }
        }
    }

    let mut queue: Vec<String> = in_degree.iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(name, _)| name.clone())
        .collect();
    queue.sort(); // deterministic order for zero-degree nodes

    let mut result = Vec::new();
    while let Some(name) = queue.pop() {
        result.push(name.clone());
        if let Some(deps) = dependents.get(&name) {
            for dep in deps {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(dep.clone());
                        queue.sort();
                    }
                }
            }
        }
    }

    if result.len() != packages.len() {
        return Err(StageError::CyclicDependency);
    }

    Ok(result)
}

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("invalid stage: {0}")]
    InvalidStage(String),
    #[error("cyclic dependency detected in stage")]
    CyclicDependency,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::format::*;

    fn minimal_recipe(name: &str, requires: &[&str], makedepends: &[&str]) -> Recipe {
        Recipe {
            package: PackageSection {
                name: name.into(),
                version: "1.0".into(),
                release: "1".into(),
                summary: "test".into(),
                description: None,
                license: None,
                homepage: None,
            },
            source: SourceSection {
                archive: "http://example.com/test.tar.gz".into(),
                checksum: "sha256:abc".into(),
                additional: None,
            },
            build: BuildSection {
                requires: requires.iter().map(|s| s.to_string()).collect(),
                makedepends: makedepends.iter().map(|s| s.to_string()).collect(),
                configure: None,
                make: None,
                install: None,
                check: None,
                chroot: false,
                stage: None,
                ..Default::default()
            },
            cross: None,
            patches: None,
            components: None,
            variables: HashMap::new(),
        }
    }

    #[test]
    fn pass_recipes_go_to_toolchain() {
        let mut recipes = HashMap::new();
        recipes.insert("gcc-pass1".into(), minimal_recipe("gcc-pass1", &[], &[]));
        recipes.insert("gcc".into(), minimal_recipe("gcc", &[], &[]));
        recipes.insert("zlib".into(), minimal_recipe("zlib", &["gcc"], &[]));

        let assignments = assign_stages(&recipes, &HashSet::new()).unwrap();
        let gcc_pass1 = assignments.iter().find(|a| a.package == "gcc-pass1").unwrap();
        assert_eq!(gcc_pass1.stage, Stage::Toolchain);
    }

    #[test]
    fn base_name_of_pass_recipe_goes_to_foundation() {
        let mut recipes = HashMap::new();
        recipes.insert("gcc-pass1".into(), minimal_recipe("gcc-pass1", &[], &[]));
        recipes.insert("gcc".into(), minimal_recipe("gcc", &[], &[]));

        let assignments = assign_stages(&recipes, &HashSet::new()).unwrap();
        let gcc = assignments.iter().find(|a| a.package == "gcc").unwrap();
        assert_eq!(gcc.stage, Stage::Foundation);
    }

    #[test]
    fn custom_packages_go_to_customization() {
        let mut recipes = HashMap::new();
        recipes.insert("my-agent".into(), minimal_recipe("my-agent", &[], &[]));
        let mut custom = HashSet::new();
        custom.insert("my-agent".into());

        let assignments = assign_stages(&recipes, &custom).unwrap();
        let agent = assignments.iter().find(|a| a.package == "my-agent").unwrap();
        assert_eq!(agent.stage, Stage::Customization);
    }

    #[test]
    fn topological_order_respects_deps() {
        let mut recipes = HashMap::new();
        recipes.insert("a".into(), minimal_recipe("a", &[], &[]));
        recipes.insert("b".into(), minimal_recipe("b", &["a"], &[]));
        recipes.insert("c".into(), minimal_recipe("c", &["b"], &[]));

        let mut set = BTreeSet::new();
        set.insert("a".into());
        set.insert("b".into());
        set.insert("c".into());

        let order = topological_sort(&set, &recipes).unwrap();
        let pos_a = order.iter().position(|n| n == "a").unwrap();
        let pos_b = order.iter().position(|n| n == "b").unwrap();
        let pos_c = order.iter().position(|n| n == "c").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }
}
```

Note: The `BuildSection` may not have a `stage` field yet — that's a new addition from the spec. Check the actual struct and add the field if needed: `pub stage: Option<String>`.

- [ ] **Step 2: Add `stage` field to BuildSection if missing**

Read `conary-core/src/recipe/format.rs`, find `BuildSection`, add:
```rust
#[serde(default)]
pub stage: Option<String>,
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-core derivation::stages -- --nocapture`
Expected: All 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/derivation/stages.rs conary-core/src/derivation/mod.rs conary-core/src/recipe/format.rs
git commit -m "feat(derivation): add stage assignment algorithm

Automatic stage detection: toolchain (pass recipes), foundation (self-hosting
set), system (everything else), customization (user packages).
Topological sort within stages. Recipe stage hints override auto-assignment."
```

---

## Task 11: Build Profile Generation

**Files:**
- Create: `conary-core/src/derivation/profile.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write BuildProfile types with tests**

Create `conary-core/src/derivation/profile.rs`:
```rust
// conary-core/src/derivation/profile.rs
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::derivation::id::DerivationId;
use crate::derivation::stages::{Stage, StageAssignment};

/// A complete, deterministic build plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildProfile {
    pub profile: ProfileMetadata,
    pub seed: ProfileSeedRef,
    pub stages: Vec<ProfileStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMetadata {
    pub manifest: String,
    pub profile_hash: String,
    pub generated_at: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSeedRef {
    pub id: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileStage {
    pub name: String,
    pub build_env: String,
    pub derivations: Vec<ProfileDerivation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileDerivation {
    pub package: String,
    pub version: String,
    pub derivation_id: String,
}

impl BuildProfile {
    /// Compute the profile hash from seed + all derivation IDs.
    pub fn compute_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("seed:{}\n", self.seed.id).as_bytes());
        for stage in &self.stages {
            hasher.update(format!("stage:{}\n", stage.name).as_bytes());
            for drv in &stage.derivations {
                hasher.update(format!("drv:{}:{}\n", drv.package, drv.derivation_id).as_bytes());
            }
        }
        hex::encode(hasher.finalize())
    }

    /// Serialize to TOML string.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Parse from TOML string.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Diff two profiles: what derivations changed.
    pub fn diff(&self, other: &BuildProfile) -> ProfileDiff {
        use std::collections::BTreeMap;

        let self_drvs: BTreeMap<String, String> = self.stages.iter()
            .flat_map(|s| s.derivations.iter())
            .map(|d| (d.package.clone(), d.derivation_id.clone()))
            .collect();
        let other_drvs: BTreeMap<String, String> = other.stages.iter()
            .flat_map(|s| s.derivations.iter())
            .map(|d| (d.package.clone(), d.derivation_id.clone()))
            .collect();

        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut changed = Vec::new();

        for (pkg, drv_id) in &other_drvs {
            match self_drvs.get(pkg) {
                None => added.push(pkg.clone()),
                Some(old_id) if old_id != drv_id => changed.push(pkg.clone()),
                _ => {}
            }
        }
        for pkg in self_drvs.keys() {
            if !other_drvs.contains_key(pkg) {
                removed.push(pkg.clone());
            }
        }

        ProfileDiff { added, removed, changed }
    }
}

#[derive(Debug, Clone)]
pub struct ProfileDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_profile(seed_id: &str, derivations: Vec<(&str, &str, &str)>) -> BuildProfile {
        let stages = vec![ProfileStage {
            name: "system".into(),
            build_env: "foundation".into(),
            derivations: derivations.iter().map(|(pkg, ver, drv)| ProfileDerivation {
                package: pkg.to_string(),
                version: ver.to_string(),
                derivation_id: drv.to_string(),
            }).collect(),
        }];
        BuildProfile {
            profile: ProfileMetadata {
                manifest: "test.toml".into(),
                profile_hash: String::new(),
                generated_at: "2026-03-19T00:00:00Z".into(),
                target: "x86_64-conary-linux-gnu".into(),
            },
            seed: ProfileSeedRef { id: seed_id.into(), source: "community".into() },
            stages,
        }
    }

    #[test]
    fn profile_hash_is_deterministic() {
        let profile = make_profile("seed1", vec![("gcc", "15.2.0", "drv1")]);
        let h1 = profile.compute_hash();
        let h2 = profile.compute_hash();
        assert_eq!(h1, h2);
    }

    #[test]
    fn different_seeds_different_hash() {
        let p1 = make_profile("seed1", vec![("gcc", "15.2.0", "drv1")]);
        let p2 = make_profile("seed2", vec![("gcc", "15.2.0", "drv1")]);
        assert_ne!(p1.compute_hash(), p2.compute_hash());
    }

    #[test]
    fn roundtrip_toml() {
        let profile = make_profile("seed1", vec![("gcc", "15.2.0", "drv1")]);
        let toml_str = profile.to_toml().unwrap();
        let reparsed = BuildProfile::from_toml(&toml_str).unwrap();
        assert_eq!(reparsed.seed.id, "seed1");
        assert_eq!(reparsed.stages[0].derivations[0].package, "gcc");
    }

    #[test]
    fn diff_detects_changes() {
        let p1 = make_profile("seed1", vec![
            ("gcc", "15.2.0", "drv1"),
            ("zlib", "1.3.2", "drv2"),
        ]);
        let p2 = make_profile("seed1", vec![
            ("gcc", "15.2.0", "drv1_changed"),
            ("openssl", "3.0", "drv3"),
        ]);
        let diff = p1.diff(&p2);
        assert_eq!(diff.changed, vec!["gcc"]);
        assert_eq!(diff.added, vec!["openssl"]);
        assert_eq!(diff.removed, vec!["zlib"]);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core derivation::profile -- --nocapture`
Expected: All 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/profile.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add BuildProfile generation, serialization, and diffing

Deterministic profile hash from seed + derivation IDs.
TOML roundtrip serialization. Profile diff for change detection."
```

---

## Task 12: Pipeline Executor

**Files:**
- Create: `conary-core/src/derivation/pipeline.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Write Pipeline struct**

Create `conary-core/src/derivation/pipeline.rs`:
```rust
// conary-core/src/derivation/pipeline.rs
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::derivation::compose;
use crate::derivation::executor::{DerivationExecutor, ExecutionResult, ExecutorError};
use crate::derivation::id::DerivationId;
use crate::derivation::output::OutputManifest;
use crate::derivation::profile::{BuildProfile, ProfileStage, ProfileDerivation, ProfileMetadata, ProfileSeedRef};
use crate::derivation::seed::Seed;
use crate::derivation::stages::{Stage, StageAssignment, assign_stages};
use crate::recipe::format::Recipe;
use rusqlite::Connection;

/// Pipeline execution configuration.
pub struct PipelineConfig {
    pub cas_dir: PathBuf,
    pub work_dir: PathBuf,
    pub target_triple: String,
    pub jobs: usize,
}

/// Pipeline progress callback.
pub enum PipelineEvent {
    StageStarted { name: String, package_count: usize },
    PackageBuilding { name: String, stage: String },
    PackageCached { name: String },
    PackageBuilt { name: String, duration_secs: u64 },
    PackageFailed { name: String, error: String },
    StageCompleted { name: String },
    PipelineCompleted { total_packages: usize, cached: usize, built: usize },
}

/// Orchestrates the full staged build pipeline.
pub struct Pipeline {
    config: PipelineConfig,
    executor: DerivationExecutor,
}

impl Pipeline {
    pub fn new(config: PipelineConfig, executor: DerivationExecutor) -> Self {
        Self { config, executor }
    }

    /// Generate a build profile from a manifest without executing.
    pub fn generate_profile(
        &self,
        seed: &Seed,
        recipes: &HashMap<String, Recipe>,
        assignments: &[StageAssignment],
        manifest_path: &str,
    ) -> BuildProfile {
        let mut stages = Vec::new();
        let stage_order = [Stage::Toolchain, Stage::Foundation, Stage::System, Stage::Customization];
        let build_env_names = ["seed", "toolchain", "foundation", "system"];

        for (stage, env_name) in stage_order.iter().zip(build_env_names.iter()) {
            let stage_assignments: Vec<&StageAssignment> = assignments.iter()
                .filter(|a| a.stage == *stage)
                .collect();

            if stage_assignments.is_empty() {
                continue;
            }

            let mut derivations = Vec::new();
            for assignment in &stage_assignments {
                if let Some(recipe) = recipes.get(&assignment.package) {
                    derivations.push(ProfileDerivation {
                        package: recipe.package.name.clone(),
                        version: recipe.package.version.clone(),
                        derivation_id: "pending".into(), // computed at build time
                    });
                }
            }

            stages.push(ProfileStage {
                name: stage.to_string(),
                build_env: env_name.to_string(),
                derivations,
            });
        }

        let mut profile = BuildProfile {
            profile: ProfileMetadata {
                manifest: manifest_path.to_string(),
                profile_hash: String::new(),
                generated_at: chrono::Utc::now().to_rfc3339(),
                target: self.config.target_triple.clone(),
            },
            seed: ProfileSeedRef {
                id: seed.metadata.seed_id.clone(),
                source: seed.metadata.origin_url.clone().unwrap_or_default(),
            },
            stages,
        };

        profile.profile.profile_hash = profile.compute_hash();
        profile
    }

    /// Execute the full pipeline.
    /// Returns the profile with all derivation IDs filled in.
    pub fn execute(
        &self,
        seed: &Seed,
        recipes: &HashMap<String, Recipe>,
        assignments: &[StageAssignment],
        conn: &Connection,
        mut on_event: impl FnMut(PipelineEvent),
    ) -> Result<BuildProfile, PipelineError> {
        let mut completed_outputs: HashMap<String, OutputManifest> = HashMap::new();
        let mut completed_ids: HashMap<String, DerivationId> = HashMap::new();
        let mut profile_stages = Vec::new();
        let mut total_cached = 0usize;
        let mut total_built = 0usize;

        let stage_order = [
            (Stage::Toolchain, "seed"),
            (Stage::Foundation, "toolchain"),
            (Stage::System, "foundation"),
            (Stage::Customization, "system"),
        ];

        let mut current_env_hash = seed.build_env_hash().to_string();

        for (stage, env_label) in &stage_order {
            let mut stage_packages: Vec<&StageAssignment> = assignments.iter()
                .filter(|a| a.stage == *stage)
                .collect();
            stage_packages.sort_by_key(|a| a.build_order);

            if stage_packages.is_empty() {
                continue;
            }

            on_event(PipelineEvent::StageStarted {
                name: stage.to_string(),
                package_count: stage_packages.len(),
            });

            let mut stage_derivations = Vec::new();

            for assignment in &stage_packages {
                let recipe = recipes.get(&assignment.package)
                    .ok_or_else(|| PipelineError::MissingRecipe(assignment.package.clone()))?;

                on_event(PipelineEvent::PackageBuilding {
                    name: assignment.package.clone(),
                    stage: stage.to_string(),
                });

                // Collect dependency derivation IDs
                let dep_ids = self.collect_dep_ids(recipe, &completed_ids);

                // Mount build environment (composefs EROFS sysroot)
                // NOTE: For the first stage, this is the seed's EROFS image.
                // For subsequent stages, it's the previous stage's composed EROFS.
                // The actual mount must happen before Kitchen.cook runs inside executor.
                // See BuildEnvironment in environment.rs for mount lifecycle.
                let sysroot = self.config.work_dir.join(format!("sysroot-{}", stage));
                // TODO: mount build environment here using BuildEnvironment::mount()
                // before passing sysroot to executor

                let result = self.executor.execute(
                    recipe,
                    &current_env_hash,
                    &dep_ids,
                    &self.config.target_triple,
                    &sysroot,
                    conn,
                )?;

                match result {
                    ExecutionResult::CacheHit { derivation_id, record } => {
                        on_event(PipelineEvent::PackageCached {
                            name: assignment.package.clone(),
                        });
                        // Load manifest from CAS so it's available for EROFS composition
                        let manifest_bytes = self.executor.cas.retrieve(&record.manifest_cas_hash)
                            .map_err(|e| PipelineError::Io(e.to_string()))?;
                        let manifest: OutputManifest = toml::from_str(
                            &String::from_utf8_lossy(&manifest_bytes)
                        ).map_err(|e| PipelineError::Io(e.to_string()))?;
                        completed_outputs.insert(assignment.package.clone(), manifest);
                        completed_ids.insert(assignment.package.clone(), derivation_id.clone());
                        total_cached += 1;
                        stage_derivations.push(ProfileDerivation {
                            package: recipe.package.name.clone(),
                            version: recipe.package.version.clone(),
                            derivation_id: derivation_id.as_str().to_string(),
                        });
                    }
                    ExecutionResult::Built { derivation_id, output } => {
                        on_event(PipelineEvent::PackageBuilt {
                            name: assignment.package.clone(),
                            duration_secs: output.manifest.build_duration_secs,
                        });
                        completed_outputs.insert(
                            assignment.package.clone(),
                            output.manifest.clone(),
                        );
                        completed_ids.insert(assignment.package.clone(), derivation_id.clone());
                        total_built += 1;
                        stage_derivations.push(ProfileDerivation {
                            package: recipe.package.name.clone(),
                            version: recipe.package.version.clone(),
                            derivation_id: derivation_id.as_str().to_string(),
                        });
                    }
                }
            }

            // Compose stage EROFS image for next stage's build environment
            let stage_manifests: Vec<&OutputManifest> = stage_packages.iter()
                .filter_map(|a| completed_outputs.get(&a.package))
                .collect();

            if !stage_manifests.is_empty() {
                let stage_dir = self.config.work_dir.join(format!("stage-{}", stage));
                std::fs::create_dir_all(&stage_dir)
                    .map_err(|e| PipelineError::Io(e.to_string()))?;
                let build_result = compose::compose_erofs(&stage_manifests, &stage_dir)
                    .map_err(|e| PipelineError::Compose(e.to_string()))?;
                current_env_hash = compose::erofs_image_hash(&build_result.image_path)
                    .map_err(|e| PipelineError::Compose(e.to_string()))?;
            }

            on_event(PipelineEvent::StageCompleted { name: stage.to_string() });

            profile_stages.push(ProfileStage {
                name: stage.to_string(),
                build_env: env_label.to_string(),
                derivations: stage_derivations,
            });
        }

        let total = total_cached + total_built;
        on_event(PipelineEvent::PipelineCompleted {
            total_packages: total,
            cached: total_cached,
            built: total_built,
        });

        let mut profile = BuildProfile {
            profile: ProfileMetadata {
                manifest: String::new(),
                profile_hash: String::new(),
                generated_at: chrono::Utc::now().to_rfc3339(),
                target: self.config.target_triple.clone(),
            },
            seed: ProfileSeedRef {
                id: seed.metadata.seed_id.clone(),
                source: seed.metadata.origin_url.clone().unwrap_or_default(),
            },
            stages: profile_stages,
        };
        profile.profile.profile_hash = profile.compute_hash();
        Ok(profile)
    }

    fn collect_dep_ids(
        &self,
        recipe: &Recipe,
        completed: &HashMap<String, DerivationId>,
    ) -> std::collections::BTreeMap<String, DerivationId> {
        let mut deps = std::collections::BTreeMap::new();
        for dep_name in recipe.build.requires.iter().chain(recipe.build.makedepends.iter()) {
            if let Some(id) = completed.get(dep_name.as_str()) {
                deps.insert(dep_name.clone(), id.clone());
            }
        }
        deps
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("missing recipe: {0}")]
    MissingRecipe(String),
    #[error("executor error: {0}")]
    Executor(#[from] ExecutorError),
    #[error("composition error: {0}")]
    Compose(String),
    #[error("I/O error: {0}")]
    Io(String),
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p conary-core 2>&1 | tail -20`
Fix any API mismatches. This is the integration point for the full pipeline.

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/pipeline.rs conary-core/src/derivation/mod.rs
git commit -m "feat(derivation): add Pipeline executor for staged bootstrap builds

Full pipeline: seed -> toolchain -> foundation -> system -> customization.
Profile generation, stage EROFS composition, event callbacks for progress."
```

---

## Task 13: CLI Commands

**Files:**
- Create: `src/cli/derivation.rs`
- Create: `src/cli/profile.rs`
- Create: `src/commands/derivation.rs`
- Create: `src/commands/profile.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/commands/mod.rs`

- [ ] **Step 1: Add CLI definitions**

Create `src/cli/derivation.rs`:
```rust
// src/cli/derivation.rs
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum DerivationCommands {
    /// Build a single recipe into CAS via the derivation engine
    Build {
        /// Path to the recipe TOML file
        recipe: PathBuf,
        /// Build environment EROFS image (seed or stage output)
        #[arg(long)]
        env: PathBuf,
        /// CAS objects directory
        #[arg(long, default_value = "/var/lib/conary/objects")]
        cas_dir: PathBuf,
        /// Database path
        #[arg(long)]
        db_path: Option<PathBuf>,
    },
    /// Show derivation ID for a recipe without building
    Show {
        /// Path to the recipe TOML file
        recipe: PathBuf,
        /// Build environment hash
        #[arg(long)]
        env_hash: String,
    },
}
```

Create `src/cli/profile.rs`:
```rust
// src/cli/profile.rs
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum ProfileCommands {
    /// Generate a build profile from a system manifest
    Generate {
        /// Path to the system manifest TOML
        manifest: PathBuf,
        /// Output profile path
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Display a build profile
    Show {
        /// Path to profile or manifest
        path: PathBuf,
    },
    /// Compare two profiles
    Diff {
        /// First profile
        old: PathBuf,
        /// Second profile
        new: PathBuf,
    },
}
```

- [ ] **Step 2: Wire CLI into main command enum**

Read `src/cli/mod.rs` to find the main `Commands` enum. Add:
```rust
/// Derivation engine operations
Derivation {
    #[command(subcommand)]
    command: DerivationCommands,
},
/// Build profile operations
Profile {
    #[command(subcommand)]
    command: ProfileCommands,
},
```

Add module declarations and imports for the new files.

- [ ] **Step 3: Create command implementations**

Create `src/commands/derivation.rs` and `src/commands/profile.rs` with basic implementations that parse args, load recipes, compute derivation IDs, and print results. These wire the CLI to the `conary-core::derivation` module.

- [ ] **Step 4: Verify compilation**

Run: `cargo build 2>&1 | tail -20`
Expected: Compiles. Fix any import issues.

- [ ] **Step 5: Test CLI help**

Run: `cargo run -- derivation --help`
Run: `cargo run -- profile --help`
Expected: Help text displays for both subcommands.

- [ ] **Step 6: Commit**

```bash
git add src/cli/derivation.rs src/cli/profile.rs src/commands/derivation.rs src/commands/profile.rs src/cli/mod.rs src/commands/mod.rs
git commit -m "feat(cli): add derivation and profile CLI commands

conary derivation build/show — single-package derivation operations.
conary profile generate/show/diff — build profile management."
```

---

## Task 14: Integration Test — Full Pipeline Smoke Test

**Files:**
- Create: `conary-core/src/derivation/tests.rs` (integration test within module)

- [ ] **Step 1: Write integration test**

Add an integration test to `conary-core/src/derivation/mod.rs` that verifies the full flow:
1. Create a minimal recipe in memory
2. Compute derivation ID
3. Verify cache miss
4. (Mock) execute build
5. Capture output
6. Write to derivation index
7. Verify cache hit on second lookup
8. Compose two outputs into EROFS entries

This test doesn't require root (no actual composefs mounts) — it exercises the data model, hashing, index, capture, and composition logic end-to-end.

- [ ] **Step 2: Run the integration test**

Run: `cargo test -p conary-core derivation::integration -- --nocapture`
Expected: Pass.

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p conary-core 2>&1 | tail -10`
Expected: All existing tests still pass. No regressions.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p conary-core -- -D warnings 2>&1 | tail -20`
Expected: Clean. Fix any warnings.

- [ ] **Step 5: Final commit**

```bash
git add conary-core/src/derivation/
git commit -m "test(derivation): add integration test for full derivation pipeline

End-to-end test: recipe -> derivation ID -> cache miss -> build ->
CAS capture -> index record -> cache hit -> EROFS composition."
```

---

## Summary

| Task | What | Files | Est. Steps |
|------|------|-------|------------|
| 1 | Data model types | `id.rs`, `output.rs`, `mod.rs` | 8 |
| 2 | Recipe hashing | `recipe_hash.rs` | 4 |
| 3 | Derivation index (SQLite) | `index.rs`, `schema.rs` | 6 |
| 4 | Output capture (DESTDIR -> CAS) | `capture.rs` | 3 |
| 5 | EROFS composition | `compose.rs` | 3 |
| 6 | Build environment (composefs) | `environment.rs` | 3 |
| 7 | Derivation executor | `executor.rs` | 3 |
| 8 | Seed model | `seed.rs` | 3 |
| 9 | System manifest | `manifest.rs` | 3 |
| 10 | Stage assignment | `stages.rs`, `format.rs` | 4 |
| 11 | Build profile | `profile.rs` | 3 |
| 12 | Pipeline executor | `pipeline.rs` | 3 |
| 13 | CLI commands | `cli/*.rs`, `commands/*.rs` | 6 |
| 14 | Integration test | `tests.rs` | 5 |

**Total: 14 tasks, ~57 steps, covering spec Phases 1-3.**

Tasks 1-7 are Phase 1 (derivation engine core + EROFS composition).
Tasks 8-9 are Phase 2 (seed + manifest).
Tasks 10-12 are Phase 3 (stage assignment + profile + pipeline).
Tasks 13-14 are cross-cutting (CLI + integration verification).

Tasks are ordered by dependency: each task builds on the previous. Within each task, steps follow TDD: write test, verify failure, implement, verify pass, commit.
