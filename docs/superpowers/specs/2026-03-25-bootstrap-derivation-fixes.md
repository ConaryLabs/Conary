# Bootstrap & Derivation Fixes: Bugs, Honest Stubs, EROFS Symlinks

Sub-project 1 of 4 for the bootstrap pipeline. Fixes 4 bugs, makes 3 stubs honest, and implements EROFS symlink collection.

## Context

Codex chunk 8 review found 7 HIGH and 1 MEDIUM findings across the derivation engine and bootstrap pipeline. This sub-project addresses the correctness bugs and makes incomplete features fail honestly instead of claiming success. The one real feature is collecting sysroot symlinks for EROFS generation images.

Later sub-projects:
- Sub-project 2: EROFS symlink collection from DB path (generation/builder.rs)
- Sub-project 3: Bootable image pipeline (ISO, GRUB, systemd-boot, kernel population)
- Sub-project 4: Tier-2 self-hosting builds (recipe pipeline for 50+ packages)

## Bug fixes

### 1. Cached source checksum verification

**File:** `conary-core/src/bootstrap/build_runner.rs`
**Problem:** `fetch_source()` returns immediately when `sources/<filename>` exists. Cached primary tarballs are never revalidated.
**Fix:** Always call `verify_checksum()` on cached files. On failure: log warning, delete cached file, re-download. Cache key remains filename (checksum-keyed cache is a larger refactor).

### 2. Compose path conflicts across entry types

**File:** `conary-core/src/derivation/compose.rs`
**Problem:** `compose_entries()` uses separate maps for files and symlinks. If one manifest installs `/path` as a file and another as a symlink, both survive in the composed output.
**Fix:** Replace separate `BTreeMap<String, FileEntryRef>` and `BTreeMap<String, SymlinkEntryRef>` with a unified `BTreeMap<String, ComposedEntry>`:

```rust
enum ComposedEntry {
    File(FileEntryRef),
    Symlink(SymlinkEntryRef),
}
```

Last writer wins regardless of type. Emit `tracing::warn!` when a path changes type (file -> symlink or vice versa). Output splits back into separate `files` and `symlinks` vectors for the EROFS builder interface.

### 3. Mount leak on marker write failure

**File:** `conary-core/src/derivation/environment.rs`
**Problem:** After seed + overlay mounts succeed, if `.seed_id` marker write fails, mounts are left active with no cleanup path.
**Fix:** Set `self.mounted = true` before the marker write so the destructor handles cleanup. Also explicitly call `unmount()` on the error path before returning.

### 4. Pipeline mount failure is fatal

**File:** `conary-core/src/derivation/pipeline.rs`
**Problem:** `execute()` logs a warning when `env.mount()` fails, then continues building against a bare directory.
**Fix:** Change `warn!` to `return Err(PipelineError::...)` when mount fails. No opt-out flag. Building against a bare directory instead of an overlay-backed sysroot produces incorrect results.

## Honest stubs

### 5. ISO bootability claims

**File:** `conary-core/src/bootstrap/image.rs`
**Problem:** `create_iso()` and fallback ISO path return `efi_bootable: true` / `bios_bootable: true` without creating real boot artifacts.
**Fix:** Return `false` for both. Emit `tracing::warn!("ISO boot artifact population not yet implemented")`. The ISO is still created (squashfs + xorriso) but honestly reports it's not bootable. Actual boot artifact population is sub-project 3.

Same for any path through `create_efi_image()` that claims bootability without populating the EFI image with a real bootloader.

### 6. Phase 5 boot validation

**File:** `conary-core/src/bootstrap/mod.rs`
**Problem:** `Bootstrap::build_image()` calls `ImageBuilder::build()` directly, bypassing `build_tier1_image()` validation.
**Fix:** Route through `build_tier1_image()` when the format requires boot artifacts (raw, qcow2, iso). For EROFS-only output, skip boot validation since it's a generation artifact, not a bootable image.

### 7. Tier-2 returns honest error

**File:** `conary-core/src/bootstrap/tier2.rs`
**Problem:** `build_all()` loops through packages but never builds anything. Returns success.
**Fix:** Return `Err(Error::NotImplemented("Tier-2 self-hosting builds not yet implemented"))`. Keep `add_ssh_config()` callable independently since it's real code. Actual Tier-2 implementation is sub-project 4.

## Feature: EROFS symlink collection

### Problem

`walk_sysroot_to_cas()` in `image.rs` explicitly skips symlinks (line ~738). `build_erofs_generation()` passes `&[]` for symlinks to `build_erofs_image()`. Result: bootstrap EROFS generations lose all symlinks -- soname links, layout links, alternative links, everything.

The EROFS builder itself fully supports symlinks via `SymlinkEntryRef`. The compose pipeline (`compose.rs`) correctly passes symlinks. The gap is only in the bootstrap sysroot walker.

### Fix

**Enhance `walk_sysroot_to_cas()` return type:**

```rust
// Before
fn walk_sysroot_to_cas(&self, ...) -> Result<Vec<(String, String, u64, u32)>>

// After
struct SysrootWalkResult {
    files: Vec<(String, String, u64, u32)>,  // (path, hash, size, mode)
    symlinks: Vec<(String, String)>,          // (path, target)
}
fn walk_sysroot_to_cas(&self, ...) -> Result<SysrootWalkResult>
```

When the walker encounters a symlink:
1. Apply `is_excluded()` -- skip symlinks under `/proc`, `/sys`, `/dev`, `/tmp`
2. Read target via `std::fs::read_link()`
3. Collect `(path_relative_to_sysroot, target_string)`

**Enhance `build_erofs_generation()`:**

Convert collected symlinks to `SymlinkEntryRef` and pass to `build_erofs_image()`:

```rust
let symlink_refs: Vec<SymlinkEntryRef> = walk_result.symlinks
    .iter()
    .map(|(path, target)| SymlinkEntryRef {
        path: path.as_str(),
        target: target.as_str(),
    })
    .collect();

build_erofs_image(&file_refs, &symlink_refs, output_dir)?;
```

### What this enables

Bootstrap EROFS generations include:
- Soname symlinks (`libfoo.so -> libfoo.so.3`)
- Layout symlinks (`/lib64 -> lib`)
- Alternative symlinks
- Any package-installed symlinks from build phases

### What this doesn't do

- Does not store symlinks in CAS (they're metadata, not content -- correct)
- Does not change the EROFS builder (already has full symlink support)
- Does not change the compose pipeline (already passes symlinks)

## Files touched

| File | Change |
|------|--------|
| `conary-core/src/bootstrap/build_runner.rs` | Verify checksum on cached sources |
| `conary-core/src/derivation/compose.rs` | Unified path map, ComposedEntry enum |
| `conary-core/src/derivation/environment.rs` | Unmount on marker write failure |
| `conary-core/src/derivation/pipeline.rs` | Mount failure is fatal |
| `conary-core/src/bootstrap/image.rs` | Symlink collection, honest ISO bootability |
| `conary-core/src/bootstrap/mod.rs` | Route through build_tier1_image for boot formats |
| `conary-core/src/bootstrap/tier2.rs` | Return NotImplemented error |

## Testing

- Compose: test that file+symlink at same path produces warning and last writer wins
- Compose: test that same-type conflicts work as before
- Symlink collection: test that walk_sysroot_to_cas collects symlinks
- Symlink collection: test that excluded paths are skipped for symlinks too
- Mount failure: test that pipeline returns error (not warning) on mount fail
- Tier-2: test that build_all() returns NotImplemented
- ISO: test that bootability flags are false

## Non-goals

- Implementing actual ISO boot artifact population (sub-project 3)
- Implementing Tier-2 recipe builds (sub-project 4)
- Changing CAS storage to include symlinks
- Changing the EROFS builder interface
