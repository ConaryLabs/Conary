# Bootstrap & Derivation Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 4 correctness/security bugs in the derivation engine, make 3 incomplete features fail honestly, and implement EROFS symlink collection from sysroot.

**Architecture:** Targeted fixes to 7 files. No new files. The EROFS symlink collection enhances the existing `walk_sysroot_to_cas()` with a second out-parameter. All other changes are localized bug fixes or error-return changes.

**Tech Stack:** Rust 1.94, rusqlite, composefs-rs (for EROFS), std::fs (for symlink_metadata/read_link)

**Spec:** `docs/superpowers/specs/2026-03-25-bootstrap-derivation-fixes.md`

---

## File Map

| File | Change |
|------|--------|
| `conary-core/src/bootstrap/build_runner.rs` | Verify checksum on cached sources |
| `conary-core/src/derivation/compose.rs` | Unified path map with ComposedEntry enum |
| `conary-core/src/derivation/environment.rs` | Set mount state before marker write |
| `conary-core/src/derivation/pipeline.rs` | Mount failure returns error |
| `conary-core/src/bootstrap/image.rs` | Symlink collection + honest bootability flags |
| `conary-core/src/bootstrap/mod.rs` | Route boot formats through build_tier1_image |
| `conary-core/src/bootstrap/tier2.rs` | Return NotImplemented error |

---

## Task 1: Cached source checksum verification

**Files:**
- Modify: `conary-core/src/bootstrap/build_runner.rs:98-137`

- [ ] **Step 1: Implement the fix**

At line 107, instead of returning immediately when the file exists, verify the checksum first:

```rust
pub fn fetch_source(
    &self,
    pkg_name: &str,
    recipe: &Recipe,
) -> Result<PathBuf, BuildRunnerError> {
    let url = recipe.archive_url();
    let filename = recipe.archive_filename();
    let target_path = self.sources_dir.join(&filename);

    if target_path.exists() {
        // Always verify cached sources -- a stale, corrupted, or
        // colliding download must not be silently reused.
        match self.verify_checksum(pkg_name, &recipe.source.checksum, &target_path) {
            Ok(()) => {
                info!("  Using cached source (checksum verified): {}", filename);
                return Ok(target_path);
            }
            Err(e) => {
                warn!(
                    "  Cached source {} failed verification: {e} -- re-downloading",
                    filename
                );
                if let Err(rm_err) = std::fs::remove_file(&target_path) {
                    warn!("  Failed to remove corrupted cache file: {rm_err}");
                }
            }
        }
    }

    info!("  Fetching: {}", url);
    // ... rest of download code unchanged ...
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/bootstrap/build_runner.rs
git commit -m "fix(bootstrap): verify checksum on cached sources, re-download on mismatch"
```

---

## Task 2: Compose path conflicts across entry types

**Files:**
- Modify: `conary-core/src/derivation/compose.rs:75-134`

- [ ] **Step 1: Add ComposedEntry enum**

Before the `compose_entries` function, add:

```rust
/// Internal enum for unified path deduplication during composition.
/// Ensures a path can only be a file OR a symlink, not both.
enum ComposedEntry {
    File(FileEntryRef),
    Symlink(SymlinkEntryRef),
}
```

- [ ] **Step 2: Rewrite compose_entries to use unified map**

Replace the two separate `BTreeMap`s with one `BTreeMap<String, ComposedEntry>`:

```rust
pub fn compose_entries(manifests: &[&OutputManifest]) -> ComposedEntries {
    let mut merged: BTreeMap<String, ComposedEntry> = BTreeMap::new();

    for manifest in manifests {
        for file in &manifest.files {
            let abs_path = if file.path.starts_with('/') {
                file.path.clone()
            } else {
                format!("/{}", file.path)
            };

            // Warn on cross-type conflict (symlink -> file)
            if let Some(ComposedEntry::Symlink(_)) = merged.get(&abs_path) {
                tracing::warn!(
                    "Compose conflict: {} changes from symlink to file (last writer wins)",
                    abs_path
                );
            }

            merged.insert(
                abs_path.clone(),
                ComposedEntry::File(FileEntryRef {
                    path: abs_path,
                    sha256_hash: file.hash.clone(),
                    size: file.size,
                    permissions: file.mode,
                }),
            );
        }

        for symlink in &manifest.symlinks {
            let abs_path = if symlink.path.starts_with('/') {
                symlink.path.clone()
            } else {
                format!("/{}", symlink.path)
            };

            // Warn on cross-type conflict (file -> symlink)
            if let Some(ComposedEntry::File(_)) = merged.get(&abs_path) {
                tracing::warn!(
                    "Compose conflict: {} changes from file to symlink (last writer wins)",
                    abs_path
                );
            }

            merged.insert(
                abs_path.clone(),
                ComposedEntry::Symlink(SymlinkEntryRef {
                    path: abs_path,
                    target: symlink.target.clone(),
                }),
            );
        }
    }

    // Split back into separate vectors for the EROFS builder interface
    let mut files = Vec::new();
    let mut symlinks = Vec::new();
    for entry in merged.into_values() {
        match entry {
            ComposedEntry::File(f) => files.push(f),
            ComposedEntry::Symlink(s) => symlinks.push(s),
        }
    }

    ComposedEntries { files, symlinks }
}
```

- [ ] **Step 3: Verify existing tests pass**

Run: `cargo test -p conary-core compose -- --nocapture`

- [ ] **Step 4: Add cross-type conflict test**

```rust
#[test]
fn compose_entries_cross_type_conflict_last_writer_wins() {
    use super::*;

    let file_manifest = OutputManifest {
        files: vec![OutputFile {
            path: "/usr/lib/libfoo.so".to_string(),
            hash: "sha256:abc123".to_string(),
            size: 1024,
            mode: 0o755,
        }],
        symlinks: vec![],
    };

    let symlink_manifest = OutputManifest {
        files: vec![],
        symlinks: vec![OutputSymlink {
            path: "/usr/lib/libfoo.so".to_string(),
            target: "libfoo.so.3".to_string(),
        }],
    };

    // Symlink manifest comes last -- should win
    let result = compose_entries(&[&file_manifest, &symlink_manifest]);
    assert!(result.files.is_empty(), "file should be replaced by symlink");
    assert_eq!(result.symlinks.len(), 1);
    assert_eq!(result.symlinks[0].target, "libfoo.so.3");

    // File manifest comes last -- should win
    let result = compose_entries(&[&symlink_manifest, &file_manifest]);
    assert_eq!(result.files.len(), 1);
    assert!(result.symlinks.is_empty(), "symlink should be replaced by file");
}
```

Check if `OutputManifest`, `OutputFile`, `OutputSymlink` are the correct struct names -- read the imports in compose.rs tests to confirm.

- [ ] **Step 5: Run tests**

Run: `cargo test -p conary-core compose -- --nocapture`

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/derivation/compose.rs
git commit -m "fix(derivation): unified path map prevents cross-type compose conflicts"
```

---

## Task 3: Mount leak on marker write failure

**Files:**
- Modify: `conary-core/src/derivation/environment.rs:260-340`

- [ ] **Step 1: Implement the fix**

In `MutableEnvironment::mount()`, move `self.seed_env = Some(seed_env)` and `self.mounted = true` to BEFORE the `.seed_id` marker write. The current order (lines 331-340) is:

```
marker write (line 331-337)  -- can fail
self.seed_env = Some(seed_env) (line 339)
self.mounted = true (line 340)
```

Change to:

```
self.seed_env = Some(seed_env)  -- track for cleanup
self.mounted = true              -- destructor will unmount
marker write                     -- if this fails, cleanup happens via Drop
```

Find lines 339-340 and the marker write block above them. Move the state assignments before the marker write.

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/environment.rs
git commit -m "fix(derivation): set mount state before marker write to prevent leak"
```

---

## Task 4: Pipeline mount failure is fatal

**Files:**
- Modify: `conary-core/src/derivation/pipeline.rs:310-312`

- [ ] **Step 1: Implement the fix**

At line 310-311, change:

```rust
// Before
if let Err(e) = env.mount() {
    warn!("Could not mount mutable environment (requires root): {e}");
}
```

To:

```rust
// After
env.mount().map_err(|e| {
    PipelineError::Io(format!(
        "Mutable environment mount failed (requires root): {e}"
    ))
})?;
```

Check what `PipelineError` variants exist and use the appropriate one. If `Io(String)` exists, use it. Otherwise use whichever variant fits.

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/derivation/pipeline.rs
git commit -m "fix(derivation): make mutable environment mount failure fatal"
```

---

## Task 5: Honest ISO bootability flags

**Files:**
- Modify: `conary-core/src/bootstrap/image.rs`

Lines to fix (found via grep):
- Line 820: `efi_bootable: true` in `build_raw_legacy()`
- Line 872: `efi_bootable: true` in `build_raw_repart()`
- Line 926-927: `efi_bootable: true, bios_bootable: true` in `build_qcow2()`
- Line 965-966: `efi_bootable: true, bios_bootable: true` in `build_iso()`

- [ ] **Step 1: Fix all legacy bootability claims**

For each location above, change `true` to `false` and add a warning before the return:

```rust
tracing::warn!("Boot artifact population not yet implemented -- image may not be bootable");
```

For `build_from_generation()` at line ~1612, `efi_bootable` is already `false` -- leave it.

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/bootstrap/image.rs
git commit -m "fix(bootstrap): honest bootability flags -- false until boot artifacts are populated"
```

---

## Task 6: Phase 5 boot validation routing

**Files:**
- Modify: `conary-core/src/bootstrap/mod.rs:467-493`

- [ ] **Step 1: Implement the fix**

At line 487, `builder.build()` is called directly. Change to route boot-requiring formats through `build_tier1_image()`:

```rust
let result = match format {
    ImageFormat::Erofs => builder.build()?,  // Generation, no boot needed
    _ => builder.build_tier1_image()?,       // Raw/Qcow2/Iso need boot validation
};
```

Check what `ImageFormat` variants exist and how `format` is determined in this function. The `format` variable should be available from the function parameters or config.

If `build_tier1_image()` is not accessible from here (it might be on `ImageBuilder`), check the method. It should be `pub` already (it's called from the CLI at `src/commands/bootstrap/mod.rs`).

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/bootstrap/mod.rs
git commit -m "fix(bootstrap): route boot formats through build_tier1_image for validation"
```

---

## Task 7: Tier-2 returns NotImplemented

**Files:**
- Modify: `conary-core/src/bootstrap/tier2.rs:32-52, 117-148`

- [ ] **Step 1: Add NotImplemented variant to Tier2Error**

At line 32, in the `Tier2Error` enum, add:

```rust
#[error("not implemented: {0}")]
NotImplemented(String),
```

- [ ] **Step 2: Change build_all() to return error**

At line 117, replace the body of `build_all()`. Keep the method signature but return an error instead of iterating through stubs:

```rust
pub fn build_all(&self) -> Result<(), Tier2Error> {
    Err(Tier2Error::NotImplemented(
        "Tier-2 self-hosting builds not yet implemented. \
         Recipe-driven build pipeline needed."
            .to_string(),
    ))
}
```

Keep `add_ssh_config()` unchanged -- it's real code callable independently.

- [ ] **Step 3: Update caller to handle NotImplemented gracefully**

In `conary-core/src/bootstrap/mod.rs:339`, the caller does `builder.build_all().map_err(...)`. The error will now propagate. Check if the caller needs special handling (e.g., skip Tier-2 with a message rather than failing the entire bootstrap). If the orchestrator should continue past Tier-2 failure, wrap the call:

```rust
match builder.build_all() {
    Ok(()) => info!("Tier-2 builds complete"),
    Err(Tier2Error::NotImplemented(msg)) => {
        warn!("Skipping Tier-2: {msg}");
    }
    Err(e) => return Err(anyhow::anyhow!("{e}")),
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check`

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/bootstrap/tier2.rs conary-core/src/bootstrap/mod.rs
git commit -m "fix(bootstrap): Tier-2 build_all returns NotImplemented instead of false success"
```

---

## Task 8: EROFS symlink collection

**Files:**
- Modify: `conary-core/src/bootstrap/image.rs:541-677, 684-775`

This is the one real feature. The EROFS builder already handles symlinks -- we just need to collect them from the sysroot.

- [ ] **Step 1: Add symlinks parameter to walk_sysroot_to_cas**

Change the function signature at line 684:

```rust
fn walk_sysroot_to_cas(
    &mut self,
    cas: &crate::filesystem::CasStore,
    sysroot: &Path,
    entries: &mut Vec<(String, String, u64, u32)>,
    symlinks: &mut Vec<(String, String)>,  // NEW: (path, target)
) -> Result<(), ImageError> {
```

- [ ] **Step 2: Collect symlinks in the walker**

At line 737, where non-files are currently skipped, add symlink collection:

```rust
if metadata.is_symlink() {
    // Collect symlink target for EROFS generation
    match std::fs::read_link(&path) {
        Ok(target) => {
            let target_str = target.to_string_lossy().to_string();
            let rel = format!("/{}", rel_path.trim_start_matches('/'));
            symlinks.push((rel, target_str));
        }
        Err(e) => {
            warn!("Cannot read symlink target {}: {e}", path.display());
        }
    }
    continue;
}

if !metadata.is_file() {
    // Skip special files (sockets, devices, etc.)
    continue;
}
```

Note: `fs::symlink_metadata()` is already used at line 724, so `metadata.is_symlink()` will work correctly (unlike `metadata.is_file()` which follows symlinks with regular `fs::metadata`).

- [ ] **Step 3: Update the call site in build_erofs_generation**

At line ~567 where `walk_sysroot_to_cas` is called, add the symlinks parameter:

```rust
let mut cas_entries = Vec::new();
let mut sysroot_symlinks = Vec::new();
self.walk_sysroot_to_cas(&cas, &sysroot, &mut cas_entries, &mut sysroot_symlinks)?;
```

- [ ] **Step 4: Pass symlinks to build_erofs_image**

At line ~619, convert collected symlinks to `SymlinkEntryRef` and pass them:

```rust
use crate::generation::builder::SymlinkEntryRef;

let symlink_refs: Vec<SymlinkEntryRef> = sysroot_symlinks
    .iter()
    .map(|(path, target)| SymlinkEntryRef {
        path: path.as_str(),
        target: target.as_str(),
    })
    .collect();
```

Change the `build_erofs_image` call from `&[]` to `&symlink_refs`.

Check the exact parameter name/position by reading the `build_erofs_image` signature in `conary-core/src/generation/builder.rs`.

- [ ] **Step 5: Verify compilation**

Run: `cargo check`

- [ ] **Step 6: Run full test suite**

Run: `cargo test`

- [ ] **Step 7: Commit**

```bash
git add conary-core/src/bootstrap/image.rs
git commit -m "feat(bootstrap): collect sysroot symlinks for EROFS generation images"
```

---

## Task 9: Final verification

- [ ] **Step 1: Full test suite**

```bash
cargo test
```

- [ ] **Step 2: Clippy**

```bash
cargo clippy -- -D warnings
```

- [ ] **Step 3: Format check**

```bash
cargo fmt --check
```
