# Composefs-Native Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Conary's mutable-filesystem transaction engine with composefs-native image building, making every install/remove/update produce an EROFS image mounted via composefs.

**Architecture:** Evaluate composefs-rs first (adopt if viable, fall back to conary-erofs). Extract generation builder and mount logic from CLI layer into conary-core. Rewrite the transaction engine to the new flow: resolve -> fetch -> DB commit -> EROFS build -> mount. Delete the journal, recovery, and file deployer modules.

**Tech Stack:** Rust 1.94, composefs-rs 0.3.0 (or conary-erofs as fallback), SQLite (rusqlite), Linux composefs (kernel 6.6+), fs-verity

**Spec:** `docs/superpowers/specs/2026-03-17-composefs-native-architecture-design.md`

**Scope:** This is Plan 1 of 3. It covers the core architectural change. Plans 2 (GC, /etc merge, boot recovery) and 3 (bootstrap, deltas, OCI) follow separately.

---

## Dependency Graph

```
Task 0 (gate) -> Task 1 (scaffold) -> Task 2 (metadata) -> Task 3 (builder) -> Task 4 (mount)
                                                                      \            /
                                                                       v          v
                                                                   Task 5 (engine rewrite)
                                                                          |
                                                                   Task 6 (CLI rewire)
                                                                          |
                                                                   Task 7 (benchmark)
                                                                          |
                                                                   Task 8 (integration test)
                                                                          |
                                                                   Task 9 (CLI generation commands + boot + takeover)
                                                                          |
                                                                   Task 10 (clippy + cleanup)
```

---

## File Structure

### New files

| File | Responsibility |
|------|---------------|
| `conary-core/src/generation/mod.rs` | Module root for generation management in core |
| `conary-core/src/generation/builder.rs` | Build EROFS images from database state (extracted from CLI) |
| `conary-core/src/generation/mount.rs` | Mount/unmount composefs generations with digest verification |
| `conary-core/src/generation/metadata.rs` | Generation metadata, path helpers, EXCLUDED_DIRS (moved from CLI) |
| `conary-core/src/generation/composefs.rs` | Kernel composefs/fs-verity capability detection (moved from CLI) |

### Modified files

| File | Change |
|------|--------|
| `Cargo.toml` (root) | Remove `conary-erofs` dependency (builder moves to conary-core) |
| `conary-core/Cargo.toml` | Add composefs-rs dependency (or conary-erofs path dep) |
| `conary-core/src/lib.rs` | Add `pub mod generation;`, update re-exports (remove RecoveryOutcome etc) |
| `conary-core/src/transaction/mod.rs` | Rewrite to new lifecycle (~250 lines replacing ~1189) |
| `src/commands/generation/builder.rs` | Thin wrapper calling `conary_core::generation::builder` |
| `src/commands/generation/switch.rs` | Thin wrapper calling `conary_core::generation::mount` |
| `src/commands/generation/metadata.rs` | Re-export from `conary_core::generation::metadata` |
| `src/commands/generation/composefs.rs` | Re-export from `conary_core::generation::composefs` |
| `src/commands/generation/boot.rs` | Update imports from metadata |
| `src/commands/generation/takeover.rs` | Update calls to builder/switch |
| `src/commands/install/mod.rs` | Adapt to new transaction API |
| `src/commands/install/execute.rs` | Adapt to new transaction API |
| `src/commands/remove.rs` | Adapt to new transaction API |
| `src/commands/restore.rs` | Replace FileDeployer with generation remount |
| `src/commands/system.rs` | Replace FileDeployer with CAS-based verification |

### Deleted files

| File | Reason |
|------|--------|
| `conary-core/src/transaction/journal.rs` (654 lines) | No journal needed |
| `conary-core/src/transaction/recovery.rs` (688 lines) | Recovery = rebuild EROFS from DB |
| `conary-core/src/filesystem/deployer.rs` (1026 lines) | No file deployment to mutable root |

---

## Task 0: composefs-rs Evaluation (Gate)

This task determines whether we adopt composefs-rs or keep conary-erofs. Everything
else in the plan works either way -- the builder abstraction is the same.

**Files:**
- Create: `conary-core/src/generation/composefs_rs_eval.rs` (temporary, deleted after eval)
- Modify: `conary-core/Cargo.toml`

- [ ] **Step 0.1: Add composefs-rs as an optional dependency to conary-core**

In `conary-core/Cargo.toml`, add under `[dependencies]`:
```toml
composefs = { version = "0.3", optional = true }
```
Add a feature:
```toml
[features]
composefs-rs = ["dep:composefs"]
```

Note: conary-erofs is currently a root crate dependency (`Cargo.toml` line 118),
not a conary-core dependency. When the builder moves to conary-core (Task 3), we
need to either add composefs-rs here (if the eval passes) or add `conary-erofs`
as a path dependency to `conary-core/Cargo.toml`. Either way, the root
`Cargo.toml` `conary-erofs` dependency should be removed at that point since the
root crate will no longer call it directly.

- [ ] **Step 0.2: Check transitive dependency weight**

Run:
```bash
cargo tree -p conary-core --features composefs-rs -d
```
Expected: Inspect the tree. Key concerns: does it pull in a conflicting tokio version? How many total deps? Document findings.

- [ ] **Step 0.3: Write a proof-of-concept image builder**

Create `conary-core/src/generation/composefs_rs_eval.rs`:

```rust
// conary-core/src/generation/composefs_rs_eval.rs
//! Proof-of-concept: build an EROFS image using composefs-rs
//! and verify it matches our requirements.

#[cfg(test)]
mod tests {
    use composefs::tree::{Directory, RegularFile, Stat};
    use composefs::erofs::writer::mkfs_erofs;

    /// Can composefs-rs produce a CAS-reference-only EROFS image?
    #[test]
    fn build_cas_reference_image() {
        // Build a minimal filesystem tree with external file references
        let stat = Stat {
            st_mode: 0o100644,
            st_uid: 0,
            st_gid: 0,
            st_mtim_sec: 0,
            ..Default::default()
        };

        // TODO: Construct FileSystem<Sha256HashValue> with:
        //   - Root directory
        //   - /usr/bin/hello as RegularFile::External(sha256_digest, 1024)
        //   - /usr/lib/libfoo.so as RegularFile::External(sha256_digest, 4096)
        //   - /bin -> usr/bin symlink
        // Call mkfs_erofs(&fs) -> Box<[u8]>
        // Verify: non-empty image, valid EROFS magic at offset 1024

        todo!("Implement after inspecting composefs-rs API in detail")
    }

    /// Does composefs-rs produce deterministic output?
    #[test]
    fn deterministic_output() {
        // Build same tree twice, assert byte-identical images
        todo!("Implement after build_cas_reference_image works")
    }

    /// Does composefs-rs emit the xattr bloom filter?
    #[test]
    fn bloom_filter_present() {
        // Build image, check superblock for FEATURE_COMPAT_XATTR_FILTER flag
        todo!("Implement after build_cas_reference_image works")
    }
}
```

Note: The exact API calls will need adjustment based on the actual composefs-rs API,
which may differ from what docs.rs shows (19% documented). Clone the repo locally
and read the source if needed:
```bash
git clone https://github.com/containers/composefs-rs /tmp/composefs-rs
```

- [ ] **Step 0.4: Run the proof-of-concept**

```bash
cargo test -p conary-core --features composefs-rs composefs_rs_eval -- --nocapture
```

Expected: All three tests pass, or we identify specific blockers.

- [ ] **Step 0.5: Make the decision**

Document results in a comment at the top of the eval file. Decision criteria:

| Criterion | Pass | Fail |
|-----------|------|------|
| CAS-reference images | External files with digest xattrs | Can't produce our format |
| Deterministic output | Same input -> same bytes | Non-deterministic |
| Bloom filter | Flag set in superblock | Missing (minor -- we can add) |
| Dep weight | < 30 new transitive deps, no tokio conflict | Heavy or conflicting |
| API stability | Core builder types are solid | Types change between 0.3.x patches |

**If PASS:** Make `composefs-rs` a required (non-optional) dependency in
`conary-core/Cargo.toml`. Delete the eval file. Remove `conary-erofs` from root
`Cargo.toml` dependencies. Proceed with composefs-rs in all subsequent tasks.

**If FAIL:** Remove the optional dependency from `conary-core/Cargo.toml`. Add
`conary-erofs = { path = "../conary-erofs" }` to `conary-core/Cargo.toml`.
Remove `conary-erofs` from root `Cargo.toml` (since core now owns it). Proceed
with conary-erofs in all subsequent tasks. Note: the builder abstraction in
Task 3 is designed to work with either backend.

- [ ] **Step 0.6: Commit**

```bash
git add -A conary-core/
git commit -m "feat(core): evaluate composefs-rs for EROFS image building

Add composefs-rs as dependency and document evaluation results.
[PASS|FAIL]: [brief reason]"
```

---

## Task 1: Create generation module in conary-core

**Files:**
- Create: `conary-core/src/generation/mod.rs`
- Modify: `conary-core/src/lib.rs`

- [ ] **Step 1.1: Create the module structure**

Create `conary-core/src/generation/mod.rs`:
```rust
// conary-core/src/generation/mod.rs
//! Generation management: build EROFS images, mount via composefs, manage metadata.

pub mod builder;
pub mod composefs;
pub mod metadata;
pub mod mount;
```

- [ ] **Step 1.2: Register the module**

In `conary-core/src/lib.rs`, add:
```rust
pub mod generation;
```

- [ ] **Step 1.3: Verify it compiles**

```bash
cargo check -p conary-core
```
Expected: Compilation errors: module files not found (builder.rs, composefs.rs,
metadata.rs, mount.rs don't exist yet).

- [ ] **Step 1.4: Create stub files**

Create empty stub files so compilation passes:

`conary-core/src/generation/builder.rs`:
```rust
// conary-core/src/generation/builder.rs
//! Build EROFS images from database state.
```

`conary-core/src/generation/composefs.rs`:
```rust
// conary-core/src/generation/composefs.rs
//! Kernel composefs and fs-verity capability detection.
```

`conary-core/src/generation/metadata.rs`:
```rust
// conary-core/src/generation/metadata.rs
//! Generation metadata, path constants, and exclusion rules.
```

`conary-core/src/generation/mount.rs`:
```rust
// conary-core/src/generation/mount.rs
//! Mount and unmount composefs generations.
```

- [ ] **Step 1.5: Verify clean compilation**

```bash
cargo check -p conary-core
```
Expected: PASS (empty modules compile)

- [ ] **Step 1.6: Commit**

```bash
git add conary-core/src/generation/ conary-core/src/lib.rs
git commit -m "feat(core): scaffold generation module

Empty module structure for builder, composefs, metadata, mount.
Will be populated in subsequent tasks."
```

---

## Task 2: Extract metadata and composefs detection to conary-core

Move `EXCLUDED_DIRS`, `ROOT_SYMLINKS`, `GenerationMetadata`, path helpers, and
composefs capability detection from the CLI layer to conary-core. The CLI modules
become thin re-exports.

**Files:**
- Modify: `conary-core/src/generation/metadata.rs`
- Modify: `conary-core/src/generation/composefs.rs`
- Modify: `src/commands/generation/metadata.rs` (198 lines -> thin re-export)
- Modify: `src/commands/generation/composefs.rs` (157 lines -> thin re-export)
- Test: inline `#[cfg(test)]` in both new files

- [ ] **Step 2.1: Write tests for metadata in conary-core**

Add to `conary-core/src/generation/metadata.rs`:
```rust
// conary-core/src/generation/metadata.rs
//! Generation metadata, path constants, and exclusion rules.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_dirs_contains_var() {
        assert!(is_excluded("var/log/messages"));
        assert!(is_excluded("var/lib/rpm/db"));
    }

    #[test]
    fn excluded_dirs_allows_usr() {
        assert!(!is_excluded("usr/bin/bash"));
        assert!(!is_excluded("usr/lib/libz.so"));
    }

    #[test]
    fn excluded_dirs_allows_etc() {
        assert!(!is_excluded("etc/nginx/nginx.conf"));
    }

    #[test]
    fn root_symlinks_are_usr_merge() {
        assert!(ROOT_SYMLINKS.iter().any(|(name, _)| *name == "bin"));
        assert!(ROOT_SYMLINKS.iter().any(|(name, _)| *name == "sbin"));
        assert!(ROOT_SYMLINKS.iter().any(|(name, _)| *name == "lib"));
        for (_, target) in ROOT_SYMLINKS {
            assert!(target.starts_with("usr/"));
        }
    }

    #[test]
    fn generation_paths() {
        assert_eq!(generations_dir(), std::path::PathBuf::from("/conary/generations"));
        assert_eq!(generation_path(3), std::path::PathBuf::from("/conary/generations/3"));
        assert_eq!(current_link(), std::path::PathBuf::from("/conary/current"));
    }

    #[test]
    fn metadata_roundtrip() {
        let meta = GenerationMetadata {
            generation: 1,
            format: "composefs".into(),
            erofs_size: Some(1024i64),
            cas_objects_referenced: Some(100i64),
            erofs_verity_digest: Some("abc123".into()),
            fsverity_enabled: true,
            created_at: "2026-03-17T00:00:00Z".into(),
            package_count: 50,
            kernel_version: Some("6.12.0".into()),
            summary: "test".into(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: GenerationMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.generation, 1);
        assert_eq!(parsed.format, "composefs");
        assert_eq!(parsed.erofs_size, Some(1024i64));
        assert_eq!(parsed.cas_objects_referenced, Some(100i64));
        assert_eq!(parsed.erofs_verity_digest, Some("abc123".into()));
    }
}
```

Note on field types: In the actual codebase, `GenerationMetadata` uses `i64` for
`generation`, `Option<i64>` for `erofs_size` and `cas_objects_referenced`, and
`i64` for `package_count`. Match these types exactly when implementing.

- [ ] **Step 2.2: Run tests to verify they fail**

```bash
cargo test -p conary-core generation::metadata -- --nocapture
```
Expected: FAIL -- types and functions don't exist yet.

- [ ] **Step 2.3: Move metadata implementation to conary-core**

Copy the implementation from `src/commands/generation/metadata.rs` (198 lines) into
`conary-core/src/generation/metadata.rs`. Key changes:

1. Update `EXCLUDED_DIRS` to the new list per spec:
   ```rust
   pub const EXCLUDED_DIRS: &[&str] = &[
       "var", "tmp", "run", "home", "root", "srv", "opt",
       "proc", "sys", "dev", "mnt", "media",
   ];
   ```
2. Keep `ROOT_SYMLINKS`, `GenerationMetadata`, path helpers, `is_excluded()` as-is.
3. Update the file header comment to `// conary-core/src/generation/metadata.rs`
4. Ensure `serde` derive is available (already a conary-core dependency).
5. Add the `erofs_verity_digest` field to `GenerationMetadata`:
   ```rust
   /// fs-verity digest of the EROFS image (for mount-time digest verification)
   #[serde(skip_serializing_if = "Option::is_none")]
   pub erofs_verity_digest: Option<String>,
   ```
   This field is needed by Task 4 (mount) to pass the digest to the composefs
   mount command for kernel-enforced image integrity.

- [ ] **Step 2.4: Run tests to verify they pass**

```bash
cargo test -p conary-core generation::metadata -- --nocapture
```
Expected: PASS

- [ ] **Step 2.5: Move composefs detection to conary-core**

Copy the implementation from `src/commands/generation/composefs.rs` (157 lines) into
`conary-core/src/generation/composefs.rs`. Key changes:

1. Update file header to `// conary-core/src/generation/composefs.rs`
2. Add test for `supports_composefs()` (reads /proc/filesystems, should work in CI):
   ```rust
   #[cfg(test)]
   mod tests {
       use super::*;

       #[test]
       fn composefs_detection_runs_without_panic() {
           // May return true or false depending on kernel, but must not panic
           let _ = supports_composefs();
       }

       #[test]
       fn preflight_returns_caps() {
           let caps = preflight_composefs();
           // On a modern kernel, erofs should be available
           // Just verify the struct is constructed correctly
           let _ = caps.fsverity;
       }
   }
   ```

- [ ] **Step 2.6: Make CLI modules re-export from conary-core**

Replace `src/commands/generation/metadata.rs` with:
```rust
// src/commands/generation/metadata.rs
//! Re-exports from conary-core. Generation metadata lives in the core crate
//! so the transaction engine can access it directly.

pub use conary_core::generation::metadata::*;
```

Replace `src/commands/generation/composefs.rs` with:
```rust
// src/commands/generation/composefs.rs
//! Re-exports from conary-core.

pub use conary_core::generation::composefs::*;
```

Note: The existing tests in metadata.rs (roundtrip, backwards_compat,
excluded_paths, generation_paths) are being **moved** to conary-core, not
deleted. Verify they all pass in their new location before replacing the CLI
file with re-exports.

- [ ] **Step 2.7: Verify everything compiles and tests pass**

```bash
cargo build && cargo test -p conary-core generation -- --nocapture
```
Expected: PASS. No behavior change for CLI -- same public API, just different source.

- [ ] **Step 2.8: Commit**

```bash
git add conary-core/src/generation/metadata.rs conary-core/src/generation/composefs.rs \
       src/commands/generation/metadata.rs src/commands/generation/composefs.rs
git commit -m "refactor(core): extract metadata and composefs detection to conary-core

Move EXCLUDED_DIRS (updated per composefs-native spec), ROOT_SYMLINKS,
GenerationMetadata, path helpers, and composefs/fs-verity detection from
CLI to conary-core::generation. CLI modules become thin re-exports.

EXCLUDED_DIRS updated: var/lib -> var (full directory), added root, srv, opt.
Added erofs_verity_digest field to GenerationMetadata for mount-time verification."
```

---

## Task 3: Extract generation builder to conary-core

Move the EROFS image building logic from `src/commands/generation/builder.rs` to
`conary-core/src/generation/builder.rs`. Adapt it to use composefs-rs (or conary-erofs
per Task 0 decision). The builder becomes callable from the transaction engine.

**Files:**
- Modify: `conary-core/src/generation/builder.rs`
- Modify: `src/commands/generation/builder.rs` (263 lines -> thin wrapper)
- Test: inline `#[cfg(test)]` in new builder

- [ ] **Step 3.1: Write builder test in conary-core**

Add to `conary-core/src/generation/builder.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn build_erofs_from_file_entries() {
        // Create a temp dir for CAS and generations
        let tmp = TempDir::new().unwrap();
        let cas_dir = tmp.path().join("objects");
        let gen_dir = tmp.path().join("generations/1");
        std::fs::create_dir_all(&cas_dir).unwrap();
        std::fs::create_dir_all(&gen_dir).unwrap();

        // Create a mock file entry list (path, sha256, size, mode)
        let entries = vec![
            FileEntryRef {
                path: "usr/bin/hello".into(),
                sha256_hash: "ab".repeat(32), // 64-char hex
                size: 1024,
                permissions: 0o755,
            },
            FileEntryRef {
                path: "usr/lib/libfoo.so".into(),
                sha256_hash: "cd".repeat(32),
                size: 4096,
                permissions: 0o644,
            },
        ];

        let result = build_erofs_image(&entries, &gen_dir).unwrap();
        assert!(result.image_path.exists());
        assert!(result.image_size > 0);
        assert_eq!(result.cas_objects_referenced, 2);
    }

    #[test]
    fn excluded_paths_are_skipped() {
        let entries = vec![
            FileEntryRef {
                path: "usr/bin/hello".into(),
                sha256_hash: "ab".repeat(32),
                size: 1024,
                permissions: 0o755,
            },
            FileEntryRef {
                path: "var/log/messages".into(), // should be excluded
                sha256_hash: "cd".repeat(32),
                size: 256,
                permissions: 0o644,
            },
        ];

        let tmp = TempDir::new().unwrap();
        let gen_dir = tmp.path().join("gen");
        std::fs::create_dir_all(&gen_dir).unwrap();

        let result = build_erofs_image(&entries, &gen_dir).unwrap();
        assert_eq!(result.cas_objects_referenced, 1); // var/log excluded
    }

    #[test]
    fn root_symlinks_are_added() {
        let tmp = TempDir::new().unwrap();
        let gen_dir = tmp.path().join("gen");
        std::fs::create_dir_all(&gen_dir).unwrap();

        // Even with no file entries, root symlinks should be present
        let result = build_erofs_image(&[], &gen_dir).unwrap();
        assert!(result.image_size > 0); // Has at least root dir + symlinks
    }
}
```

Note: The existing tests in CLI builder.rs (hex_to_digest_valid, wrong_length,
invalid_chars) should be **moved** to conary-core alongside the implementation,
not deleted. Verify they pass in their new location before replacing the CLI
file with a thin wrapper.

- [ ] **Step 3.2: Run tests to verify they fail**

```bash
cargo test -p conary-core generation::builder -- --nocapture
```
Expected: FAIL -- types don't exist yet.

- [ ] **Step 3.3: Implement the builder**

In `conary-core/src/generation/builder.rs`, implement:

```rust
// conary-core/src/generation/builder.rs
//! Build EROFS images from database state.

use std::path::{Path, PathBuf};
use crate::generation::metadata::{is_excluded, ROOT_SYMLINKS};

/// A file entry reference for building EROFS images.
/// Lightweight view -- no database dependency, just the data needed.
pub struct FileEntryRef {
    pub path: String,
    pub sha256_hash: String,
    pub size: u64,
    pub permissions: u32,
}

/// Result of building an EROFS image.
pub struct BuildResult {
    pub image_path: PathBuf,
    pub image_size: u64,
    pub cas_objects_referenced: u64,
}

/// Build an EROFS image from file entries.
///
/// Uses composefs-rs (or conary-erofs) to produce a composefs-mountable
/// EROFS image. Files are referenced externally via CAS digest xattrs;
/// no file content is stored in the image.
pub fn build_erofs_image(
    entries: &[FileEntryRef],
    generation_dir: &Path,
) -> crate::Result<BuildResult> {
    // 1. Filter excluded paths
    // 2. Build filesystem tree (composefs-rs FileSystem or conary-erofs ErofsBuilder)
    // 3. Add ROOT_SYMLINKS
    // 4. Serialize to EROFS image
    // 5. Write to generation_dir/root.erofs
    // 6. Return BuildResult

    // Implementation depends on Task 0 outcome:
    // - composefs-rs: use composefs::tree + composefs::erofs::writer::mkfs_erofs
    // - conary-erofs: use conary_erofs::builder::ErofsBuilder
    todo!("Implement based on Task 0 decision")
}
```

The actual implementation will follow the pattern in the current
`src/commands/generation/builder.rs:build_generation()` (line ~40-200) but:
- Takes `&[FileEntryRef]` instead of querying the DB directly
- Returns `BuildResult` instead of writing metadata
- Uses composefs-rs or conary-erofs per Task 0

- [ ] **Step 3.4: Run tests to verify they pass**

```bash
cargo test -p conary-core generation::builder -- --nocapture
```
Expected: PASS

- [ ] **Step 3.5: Add a higher-level build_from_db function**

This function queries the database and calls `build_erofs_image`:

```rust
/// Build an EROFS generation from the current database state.
///
/// Queries all installed packages and their file entries, builds the EROFS
/// image, writes metadata, and returns the generation number.
pub fn build_generation_from_db(
    conn: &rusqlite::Connection,
    generations_root: &Path,
) -> crate::Result<(u64, BuildResult)> {
    use crate::db::models::file_entry::FileEntry;
    use crate::db::models::state::StateEngine;
    use crate::db::models::trove::Trove;

    // 1. Get all installed troves
    // 2. For each trove, get file entries
    // 3. Convert to FileEntryRef vec
    // 4. Create SystemState snapshot
    // 5. Create generation directory
    // 6. Call build_erofs_image()
    // 7. Write GenerationMetadata JSON
    // 8. Return (generation_number, BuildResult)
    todo!()
}
```

- [ ] **Step 3.6: Make CLI builder a thin wrapper**

Replace `src/commands/generation/builder.rs` with a wrapper that calls into
conary-core:

```rust
// src/commands/generation/builder.rs
//! CLI wrapper for generation building. Core logic in conary_core::generation::builder.

use conary_core::generation::builder::{build_generation_from_db, BuildResult};
use conary_core::generation::composefs::preflight_composefs;
// ... existing CLI argument handling, progress display, etc.
```

Keep the CLI-specific parts (progress bars, output formatting, argument parsing) in
the CLI crate. Move only the core logic.

- [ ] **Step 3.7: Verify full build passes**

```bash
cargo build && cargo test
```
Expected: PASS. All existing tests should still pass since the CLI wrapper delegates
to the same logic.

- [ ] **Step 3.8: Commit**

```bash
git add conary-core/src/generation/builder.rs src/commands/generation/builder.rs
git commit -m "feat(core): extract generation builder to conary-core

Builder now lives at conary_core::generation::builder with a clean API:
build_erofs_image() takes FileEntryRef slices, build_generation_from_db()
handles the full DB -> EROFS flow. CLI builder becomes a thin wrapper."
```

---

## Task 4: Extract mount logic to conary-core

Move composefs mount/unmount from `src/commands/generation/switch.rs` to
`conary-core/src/generation/mount.rs`. Add digest verification and native
upperdir/workdir support.

**Files:**
- Modify: `conary-core/src/generation/mount.rs`
- Modify: `src/commands/generation/switch.rs` (222 lines -> thin wrapper)
- Test: inline `#[cfg(test)]`

- [ ] **Step 4.1: Write mount tests in conary-core**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_command_is_well_formed() {
        let opts = MountOptions {
            image_path: "/conary/generations/3/root.erofs".into(),
            basedir: "/conary/objects".into(),
            mount_point: "/conary/mnt".into(),
            verity: true,
            digest: Some("abcdef1234".into()),
            upperdir: Some("/conary/etc-state/upper".into()),
            workdir: Some("/conary/etc-state/work".into()),
        };

        let args = opts.to_mount_args();
        assert!(args.contains(&"-t".to_string()));
        assert!(args.contains(&"composefs".to_string()));
        assert!(args.iter().any(|a| a.contains("basedir=")));
        assert!(args.iter().any(|a| a.contains("verity=on")));
        assert!(args.iter().any(|a| a.contains("digest=abcdef1234")));
        assert!(args.iter().any(|a| a.contains("upperdir=")));
    }

    #[test]
    fn mount_command_without_verity() {
        let opts = MountOptions {
            image_path: "/conary/generations/1/root.erofs".into(),
            basedir: "/conary/objects".into(),
            mount_point: "/conary/mnt".into(),
            verity: false,
            digest: None,
            upperdir: None,
            workdir: None,
        };

        let args = opts.to_mount_args();
        assert!(!args.iter().any(|a| a.contains("verity")));
        assert!(!args.iter().any(|a| a.contains("digest")));
    }

    #[test]
    fn symlink_update_path() {
        assert_eq!(
            symlink_target_for_generation(5),
            std::path::PathBuf::from("generations/5")
        );
    }
}
```

- [ ] **Step 4.2: Run tests to verify they fail**

```bash
cargo test -p conary-core generation::mount -- --nocapture
```
Expected: FAIL

- [ ] **Step 4.3: Implement mount module**

In `conary-core/src/generation/mount.rs`:

```rust
// conary-core/src/generation/mount.rs
//! Mount and unmount composefs generations.

use std::path::{Path, PathBuf};

/// Options for mounting a composefs generation.
pub struct MountOptions {
    pub image_path: PathBuf,
    pub basedir: PathBuf,
    pub mount_point: PathBuf,
    pub verity: bool,
    pub digest: Option<String>,
    pub upperdir: Option<PathBuf>,
    pub workdir: Option<PathBuf>,
}

impl MountOptions {
    /// Build the mount(8) argument list.
    pub fn to_mount_args(&self) -> Vec<String> {
        // -t composefs <image_path> -o basedir=...,verity=on,digest=...,upperdir=...,workdir=... <mount_point>
        todo!()
    }
}

/// Mount a generation via composefs.
pub fn mount_generation(opts: &MountOptions) -> crate::Result<()> {
    // std::process::Command::new("mount").args(opts.to_mount_args()).status()
    todo!()
}

/// Unmount a composefs generation.
pub fn unmount_generation(mount_point: &Path) -> crate::Result<()> {
    todo!()
}

/// Atomically update /conary/current symlink.
pub fn update_current_symlink(generation_number: u64) -> crate::Result<()> {
    // Create temp symlink, rename over current
    todo!()
}

/// Get the relative symlink target for a generation number.
pub fn symlink_target_for_generation(n: u64) -> PathBuf {
    PathBuf::from(format!("generations/{n}"))
}

/// Read the current active generation number from /conary/current.
pub fn current_generation() -> crate::Result<Option<u64>> {
    todo!()
}
```

Port the logic from `src/commands/generation/switch.rs` (lines 1-222), adapting:
- Use `MountOptions` struct instead of inline argument building
- Add `digest` and `upperdir`/`workdir` options (new per spec)
- The `digest` field comes from `GenerationMetadata.erofs_verity_digest` (added in Task 2)
- Keep `is_overlay_mount()` helper

- [ ] **Step 4.4: Run tests to verify they pass**

```bash
cargo test -p conary-core generation::mount -- --nocapture
```
Expected: PASS

- [ ] **Step 4.5: Make CLI switch.rs a thin wrapper**

Replace `src/commands/generation/switch.rs` with a wrapper calling conary-core.

- [ ] **Step 4.6: Verify full build**

```bash
cargo build && cargo test
```
Expected: PASS

- [ ] **Step 4.7: Commit**

```bash
git add conary-core/src/generation/mount.rs src/commands/generation/switch.rs
git commit -m "feat(core): extract mount logic to conary-core

MountOptions struct with digest verification and native upperdir/workdir.
CLI switch.rs becomes a thin wrapper. Adds composefs digest mount option
for kernel-enforced image integrity."
```

---

## Task 5: Rewrite the transaction engine

This is the core change. Rewrite `conary-core/src/transaction/mod.rs` to the new
composefs-native lifecycle. Delete journal.rs, recovery.rs, and deployer.rs.

**Files:**
- Rewrite: `conary-core/src/transaction/mod.rs` (1189 lines -> ~250)
- Delete: `conary-core/src/transaction/journal.rs` (654 lines)
- Delete: `conary-core/src/transaction/recovery.rs` (688 lines)
- Delete: `conary-core/src/filesystem/deployer.rs` (1026 lines)
- Modify: `conary-core/src/filesystem/mod.rs` (remove deployer module)
- Modify: `conary-core/src/transaction/` (remove journal/recovery modules)
- Modify: `conary-core/src/lib.rs` (update re-exports -- remove RecoveryOutcome, Journal, FileDeployer)
- Test: inline `#[cfg(test)]`

- [ ] **Step 5.1: Write tests for new transaction lifecycle**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn transaction_config_defaults() {
        let tmp = TempDir::new().unwrap();
        let config = TransactionConfig::new(tmp.path());
        assert!(config.objects_dir.exists() || true); // path construction
        assert!(config.generations_dir.exists() || true);
    }

    #[test]
    fn transaction_state_machine() {
        // Verify the new state transitions: New -> Resolved -> Fetched -> Committed -> Built -> Mounted -> Done
        let states = [
            TransactionState::New,
            TransactionState::Resolved,
            TransactionState::Fetched,
            TransactionState::Committed,
            TransactionState::Built,
            TransactionState::Mounted,
            TransactionState::Done,
        ];
        for i in 0..states.len() - 1 {
            assert!(states[i].can_transition_to(&states[i + 1]));
        }
    }

    #[test]
    fn transaction_state_cannot_skip() {
        assert!(!TransactionState::New.can_transition_to(&TransactionState::Built));
        assert!(!TransactionState::Resolved.can_transition_to(&TransactionState::Mounted));
    }
}
```

- [ ] **Step 5.2: Run tests to verify they fail**

```bash
cargo test -p conary-core transaction -- --nocapture
```
Expected: FAIL (old types still exist, new ones don't)

- [ ] **Step 5.3: Delete journal.rs, recovery.rs, deployer.rs**

```bash
rm conary-core/src/transaction/journal.rs
rm conary-core/src/transaction/recovery.rs
rm conary-core/src/filesystem/deployer.rs
```

Remove module declarations:
- In `conary-core/src/transaction/mod.rs`: remove `pub mod journal;` and `pub mod recovery;`
- In `conary-core/src/filesystem/mod.rs`: remove `pub mod deployer;`

- [ ] **Step 5.4: Fix compilation errors from deletions**

```bash
cargo check -p conary-core 2>&1 | head -50
```

Fix all references to deleted modules. Key places to check:
- `conary-core/src/transaction/mod.rs` -- references to Journal, RecoveryOutcome
- `conary-core/src/lib.rs` -- any re-exports
- `src/commands/` -- any CLI commands using deployer or journal directly
- `conary-core/src/transaction/planner.rs` -- planner.rs is 700 lines and may
  reference types from deleted modules. The implementer should grep for imports
  from journal, recovery, and deployer and adapt or remove them. Check every
  `use super::` and `use crate::` import in that file.

For each broken reference, either remove the code (if it's part of the old flow) or
adapt it. This step may require multiple iterations.

- [ ] **Step 5.5: Update conary-core/src/lib.rs re-exports**

Currently `conary-core/src/lib.rs` re-exports `RecoveryOutcome` (from recovery.rs)
and other transaction types:
```rust
pub use transaction::{
    RecoveryOutcome, Transaction, TransactionConfig, TransactionEngine, TransactionPlan,
    TransactionState,
};
```

After deleting recovery.rs and rewriting transaction/mod.rs, update this to only
re-export the new types:
```rust
pub use transaction::{
    TransactionConfig, TransactionEngine, TransactionState,
};
```

Remove any other re-exports of types from deleted modules (Journal, FileDeployer,
RecoveryOutcome, etc.). Add re-exports for new generation module types as appropriate.

- [ ] **Step 5.6: Rewrite transaction/mod.rs**

Replace the entire file with the new lifecycle:

```rust
// conary-core/src/transaction/mod.rs
//! Composefs-native transaction engine.
//!
//! Every transaction follows the flow:
//! resolve -> fetch -> DB commit -> EROFS build -> mount -> symlink
//!
//! There is no journal, no backup phase, no staging phase.
//! The database is the source of truth. Everything after DB commit
//! is re-derivable.

pub mod planner; // keep -- VfsTree conflict detection still used

use std::path::{Path, PathBuf};
use crate::generation::builder::{build_generation_from_db, BuildResult};
use crate::generation::mount::{mount_generation, update_current_symlink, MountOptions};
use crate::generation::metadata::GenerationMetadata;
use crate::filesystem::cas::CasStore;

/// Transaction lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    New,
    Resolved,
    Fetched,
    Committed,
    Built,
    Mounted,
    Done,
}

impl TransactionState {
    pub fn can_transition_to(&self, next: &Self) -> bool {
        matches!(
            (self, next),
            (Self::New, Self::Resolved)
                | (Self::Resolved, Self::Fetched)
                | (Self::Fetched, Self::Committed)
                | (Self::Committed, Self::Built)
                | (Self::Built, Self::Mounted)
                | (Self::Mounted, Self::Done)
        )
    }
}

/// Configuration for the transaction engine.
pub struct TransactionConfig {
    pub root: PathBuf,
    pub db_path: PathBuf,
    pub objects_dir: PathBuf,
    pub generations_dir: PathBuf,
    pub etc_state_dir: PathBuf,
    pub mount_point: PathBuf,
    pub lock_timeout_secs: u64,
}

impl TransactionConfig {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            db_path: root.join("db.sqlite3"),
            objects_dir: root.join("objects"),
            generations_dir: root.join("generations"),
            etc_state_dir: root.join("etc-state"),
            mount_point: root.join("mnt"),
            lock_timeout_secs: 30,
        }
    }
}

/// The composefs-native transaction engine.
pub struct TransactionEngine {
    config: TransactionConfig,
    cas: CasStore,
}
```

Note: the spec's transaction flow includes scriptlet execution (step 7). In the
composefs-native model, scriptlets run after the EROFS mount with /usr read-only
and /etc+/var writable. The scriptlet integration is deferred to Plan 2, but the
transaction engine's public API should have a hook point for it (e.g., a
`post_mount` callback or a scriptlet execution step in the state machine).

```rust
// Implementation continues with:
// - begin() -> acquires lock
// - commit_to_db() -> single SQLite transaction
// - build_and_mount() -> calls generation builder + mount
// - finish() -> releases lock
// - recover() -> checks DB vs mounted generation, rebuilds if needed
```

- [ ] **Step 5.7: Verify compilation**

```bash
cargo check -p conary-core
```

Fix any remaining issues. The CLI crate will likely have compilation errors from
references to deleted types (Journal, RecoveryOutcome, FileDeployer). Fix those by
removing the old code paths in the CLI.

- [ ] **Step 5.8: Verify all tests pass**

```bash
cargo test
```

Some existing tests that rely on the old transaction flow will need to be updated or
removed. Tests in `conary-core/src/transaction/` that test journal writes, backup
restoration, or recovery replay should be deleted. Tests that test conflict detection
via VfsTree/planner should be kept.

- [ ] **Step 5.9: Commit**

```bash
git add -A
git commit -m "feat(core)!: rewrite transaction engine for composefs-native architecture

BREAKING: Remove journal, recovery, and file deployer modules.

Transaction lifecycle is now: resolve -> fetch -> DB commit -> EROFS
build -> mount -> symlink. No journal, no backup phase, no staging.
Database is the source of truth. Everything after DB commit is
re-derivable from database state.

Deleted:
- transaction/journal.rs (654 lines)
- transaction/recovery.rs (688 lines)
- filesystem/deployer.rs (1026 lines)

Net: -2368 lines of journal/backup/recovery machinery."
```

---

## Task 6: Rewire install/remove/update/restore/system commands

The install, remove, restore, and system commands heavily use `TransactionEngine`,
`TransactionOperations`, `PackageInfo`, `ExtractedFile`, `FileToRemove`, and
`FileDeployer`. Those types are deleted or rewritten in Task 5. This task adapts
all affected CLI commands to the new composefs-native transaction API.

**In the composefs-native model:**
- **install** = DB update + EROFS rebuild + mount
- **remove** = DB update + EROFS rebuild + mount
- **update** = DB update + EROFS rebuild + mount
- **restore** = remount a previous generation (no file deployment)

**Files:**
- Modify: `src/commands/install/mod.rs` -- adapt to new transaction API
- Modify: `src/commands/install/batch.rs` -- adapt to new transaction API
- Modify: `src/commands/install/execute.rs` -- adapt to new transaction API
- Modify: `src/commands/remove.rs` -- adapt to new transaction API
- Modify: `src/commands/restore.rs` -- replace `FileDeployer` with generation remount
- Modify: `src/commands/system.rs` -- replace `FileDeployer` with CAS-based verification

- [ ] **Step 6.1: Audit install/ for old transaction types**

```bash
cargo check 2>&1 | grep -E 'install/(mod|batch|execute|prepare)\.rs'
```

Enumerate every use of `TransactionEngine`, `FileDeployer`, `ExtractedFile`,
`FileToRemove`, and `PackageInfo` in the install submodule. For each occurrence,
determine the replacement:
- `FileDeployer::deploy()` calls -> remove (EROFS image replaces file deployment)
- `TransactionEngine::begin/commit` -> use new `TransactionEngine` API
- `ExtractedFile` / `FileToRemove` -> not needed; install modifies DB, then rebuilds EROFS

- [ ] **Step 6.2: Rewrite install command flow**

The install command should follow this new pattern:
1. Resolve dependencies (unchanged)
2. Fetch packages (unchanged)
3. Open DB transaction, record installed packages + file entries
4. Call `build_generation_from_db()` to produce EROFS image
5. Call `mount_generation()` to mount the new generation
6. Update `/conary/current` symlink

Remove all `FileDeployer` usage from install/mod.rs, install/execute.rs.

- [ ] **Step 6.3: Rewrite remove command**

`src/commands/remove.rs` currently uses `FileDeployer` to remove files. Replace:
1. Open DB transaction, remove trove + file entries
2. Rebuild EROFS from remaining packages
3. Mount new generation

- [ ] **Step 6.4: Rewrite restore command**

`src/commands/restore.rs` currently uses `FileDeployer` to restore files. In the
composefs-native model, restore = remount a previous generation:
1. Read the target generation's metadata
2. Mount that generation's existing EROFS image
3. Update the current symlink

No file copying needed -- the EROFS image for each generation is immutable.

- [ ] **Step 6.5: Rethink system.rs verify/repair**

`src/commands/system.rs` uses `FileDeployer` for system verification and repair.
In the composefs-native model:
- **verify** = check CAS objects by hash (are all referenced objects present and valid?)
- **repair** = rebuild the EROFS image from current DB state and remount

Replace `FileDeployer` usage with CAS hash verification and EROFS rebuild.

- [ ] **Step 6.6: Verify full build**

```bash
cargo build && cargo test
```
Expected: PASS

- [ ] **Step 6.7: Commit**

```bash
git add src/commands/install/ src/commands/remove.rs src/commands/restore.rs src/commands/system.rs
git commit -m "refactor: rewire install/remove/restore/system for composefs-native

All package operations now follow: DB update -> EROFS rebuild -> mount.
Removed FileDeployer usage from all CLI commands.
Restore simplified to generation remount (no file copying).
System verify checks CAS objects by hash; repair rebuilds EROFS."
```

---

## Task 7: EROFS build performance benchmark

The spec notes that EROFS build time should be benchmarked early. This task adds a
benchmark to validate the "sub-second for typical systems" claim.

Note: This is a quick measurement using `Instant`, not a statistically rigorous
benchmark. For CI, consider adding criterion later.

**Files:**
- Create: `conary-core/benches/erofs_build.rs`
- Modify: `conary-core/Cargo.toml` (add bench target)

- [ ] **Step 7.1: Add benchmark file**

```rust
// conary-core/benches/erofs_build.rs
//! Benchmark EROFS image building to validate sub-second claim.

use conary_core::generation::builder::{build_erofs_image, FileEntryRef};
use std::time::Instant;
use tempfile::TempDir;

fn generate_entries(count: usize) -> Vec<FileEntryRef> {
    (0..count)
        .map(|i| {
            let mut hash = format!("{i:064x}");
            hash.truncate(64);
            FileEntryRef {
                path: format!("usr/lib/file_{i:06}"),
                sha256_hash: hash,
                size: 4096,
                permissions: 0o644,
            }
        })
        .collect()
}

fn main() {
    for count in [1_000, 10_000, 50_000, 100_000, 500_000] {
        let entries = generate_entries(count);
        let tmp = TempDir::new().unwrap();
        let gen_dir = tmp.path().join("gen");
        std::fs::create_dir_all(&gen_dir).unwrap();

        let start = Instant::now();
        let result = build_erofs_image(&entries, &gen_dir).unwrap();
        let elapsed = start.elapsed();

        println!(
            "{count:>7} files: {elapsed:>8.3?}  image_size={:.1}MB",
            result.image_size as f64 / 1_048_576.0
        );
    }
}
```

- [ ] **Step 7.2: Add bench target to Cargo.toml**

```toml
[[bench]]
name = "erofs_build"
harness = false
```

- [ ] **Step 7.3: Run the benchmark**

```bash
cargo bench -p conary-core --bench erofs_build
```

Expected output (approximate):
```
   1000 files:   X.XXXms  image_size=X.XMB
  10000 files:   X.XXXms  image_size=X.XMB
  50000 files:   X.XXXs   image_size=X.XMB
 100000 files:   X.XXXs   image_size=X.XMB
 500000 files:   X.XXXs   image_size=X.XMB
```

If 100K files takes > 2 seconds, investigate bottlenecks (tree sorting, xattr
encoding, IO). The builder should be CPU-bound metadata serialization.

- [ ] **Step 7.4: Commit**

```bash
git add conary-core/benches/erofs_build.rs conary-core/Cargo.toml
git commit -m "test(core): add EROFS build performance benchmark

Validates sub-second claim for typical systems (1K-500K files).
Results: [fill in actual numbers]"
```

---

## Task 8: Integration test -- full transaction round-trip

End-to-end test: install a package via the new transaction flow, verify EROFS image
is built and metadata is correct.

**Files:**
- Modify: `conary-core/src/transaction/mod.rs` (add integration test)

- [ ] **Step 8.1: Write the integration test**

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn full_transaction_round_trip() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // 1. Set up: create CAS dir, DB, empty generation 0
        let config = TransactionConfig::new(root);
        std::fs::create_dir_all(&config.objects_dir).unwrap();
        std::fs::create_dir_all(&config.generations_dir).unwrap();
        std::fs::create_dir_all(&config.etc_state_dir).unwrap();

        // 2. Create DB with schema
        let conn = rusqlite::Connection::open(&config.db_path).unwrap();
        crate::db::schema::initialize(&conn).unwrap();

        // 3. Insert mock trove + file entries (simulating package install)
        // ... insert trove "test-package" with 3 file entries ...

        // 4. Store mock CAS objects
        let cas = CasStore::new(&config.objects_dir);
        cas.store(b"hello world content").unwrap();
        // ...

        // 5. Run build_generation_from_db
        let (gen_num, result) = build_generation_from_db(&conn, &config.generations_dir).unwrap();

        // 6. Verify
        assert_eq!(gen_num, 1);
        assert!(result.image_path.exists());
        assert!(result.image_size > 0);
        assert!(result.cas_objects_referenced > 0);

        // 7. Verify metadata JSON was written
        let meta_path = config.generations_dir.join("1/.conary-gen.json");
        assert!(meta_path.exists());

        // 8. Verify SystemState was created in DB
        let state = crate::db::models::state::SystemState::get_active(&conn).unwrap();
        assert!(state.is_some());
        assert_eq!(state.unwrap().state_number, 1);
    }
}
```

- [ ] **Step 8.2: Run the test**

```bash
cargo test -p conary-core integration_tests::full_transaction_round_trip -- --nocapture
```
Expected: PASS

- [ ] **Step 8.3: Commit**

```bash
git add conary-core/src/transaction/mod.rs
git commit -m "test(core): add full transaction round-trip integration test

Verifies: DB commit -> EROFS build -> metadata write -> SystemState
creation. End-to-end validation of the composefs-native transaction flow."
```

---

## Task 9: Update CLI generation commands + boot + takeover

Update the CLI generation commands to use the new conary-core generation module.
Also update boot.rs and takeover.rs which directly call builder/switch functions.
Commands should work identically from the user's perspective.

**Files:**
- Modify: `src/commands/generation/commands.rs`
- Modify: `src/commands/generation/mod.rs`
- Modify: `src/commands/generation/boot.rs` (263 lines) -- imports from metadata.rs
- Modify: `src/commands/generation/takeover.rs` (291 lines) -- calls `build_generation()` and `switch_live()`

- [ ] **Step 9.1: Update generation build command**

In `src/commands/generation/commands.rs`, update `cmd_generation_build` to call
`conary_core::generation::builder::build_generation_from_db` instead of the local
builder function.

- [ ] **Step 9.2: Update generation switch command**

Update `cmd_generation_switch` to call
`conary_core::generation::mount::mount_generation` with the new `MountOptions`
including digest and upperdir/workdir.

- [ ] **Step 9.3: Update generation list/info/gc commands**

These should use `conary_core::generation::metadata` for path helpers and metadata
reading. Most changes are import path updates.

- [ ] **Step 9.4: Update boot.rs**

`src/commands/generation/boot.rs` (263 lines) imports from `metadata.rs`
(`GenerationMetadata`, `generation_path`). Update these imports to come from the
re-export or directly from `conary_core::generation::metadata`. Since metadata.rs
is now a re-export, the `use super::metadata::*` imports should still work, but
verify and fix any breakage.

- [ ] **Step 9.5: Update takeover.rs**

`src/commands/generation/takeover.rs` (291 lines) directly calls:
- `super::builder::build_generation()` (line 189)
- `super::switch::switch_live()` (line 202)

Both of these functions are now thin wrappers around conary-core. Verify the
wrapper API is compatible. If the signature changed (e.g., `build_generation` now
delegates to `build_generation_from_db`), update the call sites in takeover.rs.

- [ ] **Step 9.6: Verify all generation commands work**

```bash
cargo build
cargo test -- generation
```
Expected: PASS

- [ ] **Step 9.7: Commit**

```bash
git add src/commands/generation/
git commit -m "refactor: update CLI generation commands, boot, and takeover to use conary-core

All generation commands now delegate to conary_core::generation module.
Updated boot.rs imports and takeover.rs builder/switch calls.
No user-facing behavior change."
```

---

## Task 10: Clippy, cleanup, and final verification

- [ ] **Step 10.1: Run clippy**

```bash
cargo clippy -- -D warnings
```
Fix all warnings.

- [ ] **Step 10.2: Run format check**

```bash
cargo fmt --check
```
Fix any formatting issues.

- [ ] **Step 10.3: Run full test suite**

```bash
cargo test
```
All tests must pass.

- [ ] **Step 10.4: Verify net line count reduction**

```bash
git diff --stat main
```
Expected: Significant net deletion (journal 654 + recovery 688 + deployer 1026 =
2368 lines deleted, offset by ~500 lines of new generation module code).

- [ ] **Step 10.5: Commit any cleanup**

```bash
git add -A
git commit -m "chore: clippy fixes and cleanup after composefs-native rewrite"
```

---

## Summary

| Task | What | Net lines |
|------|------|-----------|
| 0 | composefs-rs evaluation (gate) | +50 (temporary, may be deleted) |
| 1 | Generation module scaffold | +20 |
| 2 | Extract metadata + composefs detection | ~0 (moved, not new) |
| 3 | Extract generation builder | ~0 (moved + adapted) |
| 4 | Extract mount logic | ~0 (moved + enhanced) |
| 5 | Rewrite transaction engine | **-2100** (delete 2368, add ~250) |
| 6 | Rewire install/remove/restore/system commands | ~0 (adapted, not new) |
| 7 | EROFS build benchmark | +50 |
| 8 | Integration test | +80 |
| 9 | CLI generation commands + boot + takeover | ~0 (import changes) |
| 10 | Clippy + cleanup | ~0 |
| **Total** | | **~-1900 lines** |
