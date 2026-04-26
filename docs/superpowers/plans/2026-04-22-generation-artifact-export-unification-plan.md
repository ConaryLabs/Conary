# Generation Artifact Export Unification Implementation Plan

> **Historical execution plan:** This plan drove the generation artifact export
> unification slice that landed on `main` in
> `3df9716f feat(generation): unify artifact image export`. The checkboxes
> below were planning scaffolding, not an authoritative post-merge ledger.

**Goal:** Replace the false legacy generation image path with one validated generation-artifact export pipeline that emits raw/qcow2 images through `systemd-repart` and reserves ISO on the same contract.

**Architecture:** Add a core generation artifact contract in `conary-core`, make runtime/bootstrap generation producers write that contract, project validated artifacts into staged ESP/rootfs trees, and feed those staged trees into a shared raw-image backend. The CLI moves generation-derived disk export to `conary system generation export`; `conary bootstrap image` stays sysroot-oriented and `--from-generation` is removed entirely.

**Tech Stack:** Rust, Clap, Serde JSON, SHA-256 hashing, `composefs-rs`, `systemd-repart`, `qemu-img`, `tempfile`, `walkdir`, and `conary-test` QEMU suites.

**Spec:** `docs/superpowers/specs/2026-04-22-generation-artifact-export-unification-design.md`

---

## Current State

Implemented and merged:

- the legacy `conary bootstrap image --from-generation` surface is removed
- `conary system generation export` is wired to the shared core export path
- generation artifacts now include `.conary-artifact.json`,
  `cas-manifest.json`, and `boot-assets/manifest.json`
- bootstrap and runtime producers stage explicit boot assets
- raw export uses the shared `systemd-repart` backend
- qcow2 export is raw plus `qemu-img convert`
- ISO parses but returns the explicit reserved/not-implemented error
- `conary-test` has manifest support for `qemu_boot.local_image_path` and
  `qemu_boot.copy_from_guest`
- the `Generation Artifact Export QEMU` suite exists and appears in
  `cargo run -p conary-test -- list`

Still pending before closing the slice operationally:

- run and record:

```bash
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora43 --phase 3
```

- if that suite exposes a remote image/tooling/bootstrap fixture blocker, fix
  the blocker directly or record it as a narrow follow-up with maintainer
  approval

Follow-up design work after this slice is tracked in
`docs/operations/post-generation-export-follow-up-roadmap.md`.

## Scope Guard

- Do not keep a compatibility path for `conary bootstrap image --from-generation`.
- Do not leave `ImageBuilder::build_from_generation()` or equivalent imperative generation image writing in place.
- Do not scrape live host `/boot` during export. Runtime generation build may stage boot assets, but export only consumes generation-local artifacts.
- Do not copy an entire CAS store. Export copies exactly `cas-manifest.json` objects after size and SHA-256 verification.
- Implement bootable raw/qcow2 for `x86_64` only in this slice. `aarch64` and `riscv64` fail closed with explicit unsupported-architecture errors.
- ISO parses through the new CLI but returns the explicit reserved/not-implemented error.
- Keep OCI export unchanged in this slice.

## File Map

| File | Responsibility |
|------|----------------|
| `docs/superpowers/specs/2026-04-22-generation-artifact-export-unification-design.md` | Reviewed design source of truth |
| `docs/superpowers/plans/2026-04-22-generation-artifact-export-unification-plan.md` | This implementation plan |
| `apps/conary/src/cli/bootstrap.rs` | Remove `--from-generation` from `bootstrap image` |
| `apps/conary/src/cli/generation.rs` | Add `system generation export` CLI shape |
| `apps/conary/src/cli/mod.rs` | CLI parse regression tests |
| `apps/conary/src/dispatch.rs` | Route generation export and remove legacy bootstrap argument plumbing |
| `apps/conary/src/commands/bootstrap/mod.rs` | Keep bootstrap image sysroot-oriented; update EROFS guidance |
| `apps/conary/src/commands/generation/mod.rs` | Expose generation export command module |
| `apps/conary/src/commands/generation/export.rs` | CLI wrapper for generation export |
| `crates/conary-core/src/lib.rs` | Expose shared image module |
| `crates/conary-core/src/bootstrap/image.rs` | Remove `build_from_generation`; make EROFS output stage export contract |
| `crates/conary-core/src/bootstrap/repart.rs` | Retire or thinly re-export the moved shared repart types during refactor |
| `crates/conary-core/src/image/mod.rs` | New shared image backend module |
| `crates/conary-core/src/image/size.rs` | Shared image size parser moved from bootstrap image code |
| `crates/conary-core/src/image/repart.rs` | Shared `systemd-repart` definitions and raw materialization |
| `crates/conary-core/src/generation/mod.rs` | Expose artifact/export modules |
| `crates/conary-core/src/generation/metadata.rs` | Add `artifact_manifest_sha256` to generation metadata |
| `crates/conary-core/src/generation/artifact.rs` | Manifest schemas, path validation, digest validation, artifact loader |
| `crates/conary-core/src/generation/export.rs` | Rootfs/ESP projection and raw/qcow2/ISO export orchestration |
| `crates/conary-core/src/generation/builder.rs` | Stage manifests and boot assets for runtime generations |
| `apps/conary-test/src/config/manifest.rs` | Add QEMU local-image and guest-copy manifest fields if needed |
| `apps/conary-test/src/engine/qemu.rs` | Let QEMU tests copy generated images out of a guest and boot local qcow2 paths |
| `apps/conary-test/src/engine/variables.rs` | Expand variables in new QEMU manifest fields |
| `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml` | QEMU validation for installed-generation fail-closed behavior and bootable bootstrap-run exports |
| `docs/modules/bootstrap.md` | Update canonical CLI guidance after removing legacy export |
| `docs/INTEGRATION-TESTING.md` | Mention the generation export QEMU suite if added |

## Chunk 1: CLI Migration And Legacy Surface Removal

### Task 1: Lock CLI migration behavior with failing tests

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify later: `apps/conary/src/cli/bootstrap.rs`
- Modify later: `apps/conary/src/cli/generation.rs`

- [ ] **Step 1: Add failing CLI parse tests**

Add tests under `#[cfg(test)] mod tests` in `apps/conary/src/cli/mod.rs`:

```rust
#[test]
fn cli_rejects_bootstrap_image_from_generation() {
    let err = Cli::try_parse_from([
        "conary",
        "bootstrap",
        "image",
        "--from-generation",
        "output/generations/1",
    ])
    .expect_err("--from-generation must be removed from bootstrap image");

    assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn cli_accepts_generation_export_from_explicit_path() {
    let cli = Cli::try_parse_from([
        "conary",
        "system",
        "generation",
        "export",
        "--path",
        "output/generations/1",
        "--format",
        "raw",
        "--output",
        "gen1.raw",
    ])
    .expect("generation export from path should parse");

    match cli.command {
        Some(Commands::System(SystemCommands::Generation(GenerationCommands::Export {
            generation,
            path,
            format,
            output,
            size,
        }))) => {
            assert_eq!(generation, None);
            assert_eq!(path.as_deref(), Some("output/generations/1"));
            assert_eq!(format, "raw");
            assert_eq!(output, "gen1.raw");
            assert_eq!(size, None);
        }
        _ => panic!("expected system generation export command"),
    }
}

#[test]
fn cli_rejects_generation_export_path_and_number_together() {
    let err = Cli::try_parse_from([
        "conary",
        "system",
        "generation",
        "export",
        "7",
        "--path",
        "output/generations/1",
        "--format",
        "raw",
        "--output",
        "gen.raw",
    ])
    .expect_err("--path must conflict with positional generation number");

    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary --bin conary cli::tests::cli_rejects_bootstrap_image_from_generation
cargo test -p conary --bin conary cli::tests::cli_accepts_generation_export_from_explicit_path
cargo test -p conary --bin conary cli::tests::cli_rejects_generation_export_path_and_number_together
```

Expected:
- first test fails because `--from-generation` still parses
- second/third tests fail because `GenerationCommands::Export` does not exist

- [ ] **Step 3: Remove bootstrap legacy flag**

Modify `apps/conary/src/cli/bootstrap.rs`:

- remove `from_generation: Option<String>` from `BootstrapCommands::Image`
- remove the `#[arg(long)]` metadata for that field

- [ ] **Step 4: Add generation export CLI variant**

Modify `apps/conary/src/cli/generation.rs`:

```rust
    /// Export a generation artifact as a disk image.
    Export {
        /// Installed generation number to export (defaults to current generation).
        #[arg(conflicts_with = "path")]
        generation: Option<i64>,

        /// Explicit generation directory, e.g. output/generations/1.
        #[arg(long)]
        path: Option<String>,

        /// Output format: raw, qcow2, or iso.
        #[arg(long, default_value = "qcow2")]
        format: String,

        /// Output image path.
        #[arg(short, long)]
        output: String,

        /// Optional image size larger than the computed minimum, e.g. 8G.
        #[arg(long)]
        size: Option<String>,
    },
```

- [ ] **Step 5: Update dispatch to match the new CLI shape**

Modify `apps/conary/src/dispatch.rs`:

- remove `from_generation` from the `BootstrapCommands::Image` match arm
- call `commands::cmd_bootstrap_image(&work_dir, &output, &format, &size).await`
- add a `GenerationCommands::Export` arm that calls a temporary command stub:

```rust
cli::GenerationCommands::Export {
    generation,
    path,
    format,
    output,
    size,
} => {
    commands::generation::export::cmd_generation_export(
        generation,
        path.as_deref(),
        &format,
        &output,
        size.as_deref(),
    )
    .await
}
```

- [ ] **Step 6: Add the temporary command module**

Create `apps/conary/src/commands/generation/export.rs`:

```rust
// apps/conary/src/commands/generation/export.rs
//! Generation disk-image export command wrapper.

use anyhow::Result;

pub async fn cmd_generation_export(
    _generation: Option<i64>,
    _path: Option<&str>,
    _format: &str,
    _output: &str,
    _size: Option<&str>,
) -> Result<()> {
    Err(anyhow::anyhow!(
        "generation export backend is not implemented yet"
    ))
}
```

Modify `apps/conary/src/commands/generation/mod.rs`:

```rust
pub mod export;
```

- [ ] **Step 7: Update bootstrap command signature and remove legacy branch**

Modify `apps/conary/src/commands/bootstrap/mod.rs`:

- change `cmd_bootstrap_image` signature to remove `from_generation`
- delete the `if let Some(gen_dir) = from_generation` branch
- update the EROFS success text to point at `conary system generation export --path <output>/generations/1 ...`

- [ ] **Step 8: Run CLI tests and compile**

Run:

```bash
cargo test -p conary --bin conary cli::tests::cli_rejects_bootstrap_image_from_generation
cargo test -p conary --bin conary cli::tests::cli_accepts_generation_export_from_explicit_path
cargo test -p conary --bin conary cli::tests::cli_rejects_generation_export_path_and_number_together
cargo build -p conary
```

Expected:
- all three CLI tests pass
- `cargo build -p conary` exits `0`

- [ ] **Step 9: Commit CLI migration**

```bash
git add apps/conary/src/cli/bootstrap.rs apps/conary/src/cli/generation.rs apps/conary/src/cli/mod.rs apps/conary/src/dispatch.rs apps/conary/src/commands/bootstrap/mod.rs apps/conary/src/commands/generation/mod.rs apps/conary/src/commands/generation/export.rs
git commit -m "feat(generation): add export command surface"
```

### Task 2: Delete the old generation image builder

**Files:**
- Modify: `crates/conary-core/src/bootstrap/image.rs`

- [ ] **Step 1: Add a regression search command to the task notes**

Run before editing:

```bash
rg -n 'build_from_generation|--from-generation|from_generation:\s*Option<String>' apps crates docs --glob '!docs/superpowers/specs/2026-04-22-generation-artifact-export-unification-design.md' --glob '!docs/superpowers/plans/2026-04-22-generation-artifact-export-unification-plan.md'
```

Expected:
- references remain in code before deletion

- [ ] **Step 2: Remove `ImageBuilder::build_from_generation()`**

Delete the full `pub fn build_from_generation(...)` implementation from `crates/conary-core/src/bootstrap/image.rs`.

Also remove imports that become unused only because of this deletion, especially `Stdio` if no other code needs it.

- [ ] **Step 3: Run focused bootstrap image tests**

Run:

```bash
cargo test -p conary-core bootstrap::image
cargo build -p conary
```

Expected:
- compile errors expose only unused imports or direct references to removed code
- after cleanup, both commands exit `0`

- [ ] **Step 4: Verify the old path is gone**

Run:

```bash
rg -n 'build_from_generation|--from-generation|from_generation:\s*Option<String>' apps crates docs --glob '!docs/superpowers/specs/2026-04-22-generation-artifact-export-unification-design.md' --glob '!docs/superpowers/plans/2026-04-22-generation-artifact-export-unification-plan.md'
```

Expected:
- no matches outside archived/spec/plan references

- [ ] **Step 5: Commit legacy removal**

```bash
git add crates/conary-core/src/bootstrap/image.rs
git commit -m "refactor(bootstrap): remove legacy generation image builder"
```

## Chunk 2: Generation Artifact Contract

### Task 3: Add artifact manifest digest to generation metadata

**Files:**
- Modify: `crates/conary-core/src/generation/metadata.rs`

- [ ] **Step 1: Extend metadata tests first**

Update `test_metadata_roundtrip` to set and assert:

```rust
artifact_manifest_sha256: Some(
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
),
```

Add to the old-format test:

```rust
assert_eq!(loaded.artifact_manifest_sha256, None);
```

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary-core generation::metadata::tests::test_metadata_roundtrip
cargo test -p conary-core generation::metadata::tests::test_metadata_backwards_compat
```

Expected:
- compile fails because the field does not exist

- [ ] **Step 3: Add the field**

Modify `GenerationMetadata`:

```rust
    /// SHA-256 of the exact on-disk `.conary-artifact.json` bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_manifest_sha256: Option<String>,
```

Update all `GenerationMetadata { ... }` initializers in the repo to include either:

```rust
artifact_manifest_sha256: None,
```

or a real digest once producer tasks implement artifact writing.

- [ ] **Step 4: Run metadata tests**

Run:

```bash
cargo test -p conary-core generation::metadata
```

Expected: all generation metadata tests pass.

- [ ] **Step 5: Commit metadata field**

```bash
git add crates/conary-core/src/generation/metadata.rs crates/conary-core/src/generation/builder.rs crates/conary-core/src/bootstrap/image.rs
git commit -m "feat(generation): record artifact manifest digest"
```

### Task 4: Implement manifest schemas and path validation

**Files:**
- Create: `crates/conary-core/src/generation/artifact.rs`
- Modify: `crates/conary-core/src/generation/mod.rs`

- [ ] **Step 1: Write failing schema/path tests**

Create `crates/conary-core/src/generation/artifact.rs` with tests first. Include tests for:

- artifact manifest JSON round-trip
- CAS manifest JSON round-trip
- boot-assets manifest JSON round-trip
- `metadata`, `erofs`, `cas_manifest`, and `boot_assets` rejecting absolute paths and `..`
- `cas_base = "../../objects"` resolving from `output/generations/1` to `output/objects`
- `cas_base` rejecting absolute paths and paths outside `<artifact-root>/objects`
- explicit `--path` directory whose parent is not `generations` failing with a clear error

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary-core generation::artifact
```

Expected:
- module does not compile until exported and implemented

- [ ] **Step 3: Add module export**

Modify `crates/conary-core/src/generation/mod.rs`:

```rust
pub mod artifact;
```

- [ ] **Step 4: Implement manifest structs**

In `crates/conary-core/src/generation/artifact.rs`, implement:

```rust
// crates/conary-core/src/generation/artifact.rs

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const ARTIFACT_MANIFEST_FILE: &str = ".conary-artifact.json";
pub const CAS_MANIFEST_FILE: &str = "cas-manifest.json";
pub const BOOT_ASSETS_DIR: &str = "boot-assets";
pub const BOOT_ASSETS_MANIFEST_REL: &str = "boot-assets/manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationArtifactManifest {
    pub version: u32,
    pub generation: i64,
    pub architecture: String,
    pub metadata: String,
    pub erofs: String,
    pub erofs_sha256: String,
    pub cas_base: String,
    pub cas_manifest: String,
    pub cas_manifest_sha256: String,
    pub boot_assets: String,
    pub boot_assets_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasManifest {
    pub version: u32,
    pub generation: i64,
    pub architecture: String,
    pub objects: Vec<CasObjectRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasObjectRef {
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootAssetsManifest {
    pub version: u32,
    pub generation: i64,
    pub architecture: String,
    pub kernel_version: String,
    pub kernel: String,
    pub kernel_sha256: String,
    pub initramfs: String,
    pub initramfs_sha256: String,
    pub efi_bootloader: String,
    pub efi_bootloader_sha256: String,
    pub created_at: String,
}
```

- [ ] **Step 5: Implement path validators**

Implement helpers:

```rust
fn validate_generation_relative_path(field: &str, rel: &str) -> crate::Result<PathBuf>;
fn validate_boot_asset_relative_path(field: &str, rel: &str) -> crate::Result<PathBuf>;
fn infer_artifact_root(generation_dir: &Path) -> crate::Result<PathBuf>;
fn resolve_cas_base(generation_dir: &Path, rel: &str) -> crate::Result<PathBuf>;
```

Rules:
- non-`cas_base` paths must be relative, not contain `..`, and stay under generation dir
- boot asset paths must be relative to `boot-assets/`, not contain `..`, and stay under that subtree
- `cas_base` may contain `..`, but must be relative and canonicalize exactly to `<artifact-root>/objects`
- artifact root is the parent of `generations`; fail if the generation directory's parent is not named `generations`

- [ ] **Step 6: Run artifact schema/path tests**

Run:

```bash
cargo test -p conary-core generation::artifact
```

Expected: schema/path tests pass.

- [ ] **Step 7: Commit manifest schema**

```bash
git add crates/conary-core/src/generation/artifact.rs crates/conary-core/src/generation/mod.rs
git commit -m "feat(generation): define export artifact manifests"
```

### Task 5: Implement artifact loading and digest verification

**Files:**
- Modify: `crates/conary-core/src/generation/artifact.rs`
- Modify: `crates/conary-core/src/filesystem/cas.rs` if a small public helper is needed

- [ ] **Step 1: Add failing validation tests**

Add tests for:

- complete artifact loads successfully
- pending generations are rejected
- missing `.conary-artifact.json` reports pre-export-contract generation
- missing `.conary-gen.json` reports corrupt or incomplete artifact metadata
- artifact manifest present but no matching `artifact_manifest_sha256` reports corrupt artifact
- mismatched generation across manifests fails
- mismatched architecture across manifests fails
- bad `root.erofs` digest fails
- bad child manifest digest fails
- missing `cas-manifest.json` fails
- CAS object missing fails
- CAS object size mismatch fails
- CAS object SHA-256 mismatch fails
- duplicate CAS manifest entries are rejected
- unsorted CAS manifest entries load successfully
- missing `boot-assets/manifest.json` fails
- boot asset missing fails
- boot asset symlink fails
- boot asset SHA-256 mismatch fails
- mixed-case and wrong-length SHA-256 strings are rejected
- artifact, CAS, and boot-assets manifests with unknown `version` values are rejected
- `aarch64` and `riscv64` fail as unsupported for export

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary-core generation::artifact
```

Expected: new loader tests fail because loader is not implemented.

- [ ] **Step 3: Implement loader output**

Add:

```rust
#[derive(Debug, Clone)]
pub struct GenerationArtifact {
    pub generation: i64,
    pub generation_dir: PathBuf,
    pub artifact_manifest: GenerationArtifactManifest,
    pub metadata: GenerationMetadata,
    pub erofs_path: PathBuf,
    pub cas_dir: PathBuf,
    pub cas_objects: Vec<CasObjectRef>,
    pub boot_assets: BootAssetsManifest,
}

pub fn load_generation_artifact(generation_dir: &Path) -> crate::Result<GenerationArtifact>;
pub fn load_installed_generation_artifact(generation: i64) -> crate::Result<GenerationArtifact>;
```

- [ ] **Step 4: Implement SHA-256 helpers**

Use `sha2::Sha256` directly or repo hashing helpers. Add private helpers:

```rust
fn sha256_file(path: &Path) -> crate::Result<String>;
fn sha256_bytes(bytes: &[u8]) -> String;
fn validate_sha256_hex(field: &str, value: &str) -> crate::Result<()>;
```

The artifact manifest digest must be over exact on-disk `.conary-artifact.json` bytes.

- [ ] **Step 5: Implement CAS object verification**

Use `crate::filesystem::cas::object_path(&cas_dir, &sha256)` to resolve objects. For every `CasObjectRef`:

- validate lowercase 64-character hex
- reject duplicates
- require file exists
- check `metadata.len() == size`
- re-hash the file content and compare to `sha256`

- [ ] **Step 6: Implement boot asset verification**

For `kernel`, `initramfs`, and `efi_bootloader`:

- reject symlinks via `std::fs::symlink_metadata`
- require regular file
- re-hash and compare to corresponding digest field

- [ ] **Step 7: Run loader tests**

Run:

```bash
cargo test -p conary-core generation::artifact
```

Expected: all artifact loader tests pass.

- [ ] **Step 8: Commit loader**

```bash
git add crates/conary-core/src/generation/artifact.rs crates/conary-core/src/filesystem/cas.rs
git commit -m "feat(generation): validate export artifacts"
```

## Chunk 3: Producers Write Exportable Generation Artifacts

### Task 6: Stage bootstrap EROFS output as a complete export artifact

**Files:**
- Modify: `crates/conary-core/src/bootstrap/image.rs`
- Modify: `crates/conary-core/src/generation/artifact.rs`

- [ ] **Step 1: Extend bootstrap EROFS test**

Update `test_erofs_generation_from_sysroot` to create boot assets:

```rust
fs::create_dir_all(sysroot.join("boot/EFI/BOOT")).unwrap();
fs::write(sysroot.join("boot/vmlinuz"), b"kernel").unwrap();
fs::write(sysroot.join("boot/initramfs.img"), b"initramfs").unwrap();
fs::write(sysroot.join("boot/EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
```

These fake bytes are opaque to the artifact loader; this test only validates
staging, digesting, and manifest wiring, not kernel/initramfs/EFI structure.

Assert output contains:

```rust
assert!(output.join("generations/1/.conary-artifact.json").is_file());
assert!(output.join("generations/1/cas-manifest.json").is_file());
assert!(output.join("generations/1/boot-assets/manifest.json").is_file());
assert!(output.join("generations/1/boot-assets/vmlinuz").is_file());
assert!(output.join("generations/1/boot-assets/initramfs.img").is_file());
assert!(output.join("generations/1/boot-assets/EFI/BOOT/BOOTX64.EFI").is_file());
```

Then call `load_generation_artifact(&output.join("generations/1"))`.

- [ ] **Step 2: Run test and verify red**

Run:

```bash
cargo test -p conary-core bootstrap::image::tests::test_erofs_generation_from_sysroot --features composefs-rs
```

Expected: fails because bootstrap output does not stage manifests/boot assets.

- [ ] **Step 3: Add artifact writer helpers**

In `generation/artifact.rs`, add producer helpers:

```rust
pub struct ArtifactWriteInputs<'a> {
    pub generation_dir: &'a Path,
    pub generation: i64,
    pub architecture: &'a str,
    pub erofs_path: &'a Path,
    pub cas_base_rel: &'a str,
    pub cas_objects: Vec<CasObjectRef>,
    pub boot_assets: BootAssetsManifest,
}

pub fn write_generation_artifact(inputs: ArtifactWriteInputs<'_>) -> crate::Result<String>;
```

Return the SHA-256 of the exact artifact manifest bytes so the caller can write it to `GenerationMetadata.artifact_manifest_sha256`.

- [ ] **Step 4: Stage bootstrap boot assets**

In `ImageBuilder::build_erofs_generation()`:

- copy from sysroot:
  - `boot/vmlinuz`
  - `boot/initramfs.img`
  - `boot/EFI/BOOT/BOOTX64.EFI`
- require bootstrap to have staged `systemd-bootx64.efi` into the sysroot as
  `boot/EFI/BOOT/BOOTX64.EFI` before this step. A plain distro chroot may only
  have `/usr/lib/systemd/boot/efi/systemd-bootx64.efi`; if the bootstrap
  pipeline has not copied it into `/boot/EFI/BOOT/`, fail with an actionable
  error and treat the TGE02 QEMU path as blocked until the fixture is fixed
- write them under `generations/1/boot-assets/`
- fail if any required file is missing
- dereference source symlinks while copying and always write plain regular
  files at the destination
- verify each staged destination with `symlink_metadata()` and reject anything
  that is not a regular file
- compute boot asset digests

- [ ] **Step 5: Write CAS manifest from bootstrap file entries**

Convert `file_entries` into deduplicated `CasObjectRef { sha256, size }`, sorted by `sha256`. Add a producer test that intentionally feeds unsorted input and asserts the written `cas-manifest.json` is sorted.

- [ ] **Step 6: Write artifact before metadata**

Sequence:

1. build `root.erofs`
2. stage boot assets
3. write `cas-manifest.json`
4. write `boot-assets/manifest.json`
5. write `.conary-artifact.json`
6. write `.conary-gen.json` with `artifact_manifest_sha256: Some(digest)`

- [ ] **Step 7: Run bootstrap EROFS tests**

Run:

```bash
cargo test -p conary-core bootstrap::image::tests::test_erofs_generation_from_sysroot --features composefs-rs
cargo test -p conary-core generation::artifact --features composefs-rs
```

Expected: tests pass.

- [ ] **Step 8: Commit bootstrap producer**

```bash
git add crates/conary-core/src/bootstrap/image.rs crates/conary-core/src/generation/artifact.rs
git commit -m "feat(bootstrap): emit exportable generation artifacts"
```

### Task 7: Stage runtime generations as exportable artifacts

**Files:**
- Modify: `crates/conary-core/src/generation/builder.rs`
- Modify: `crates/conary-core/src/generation/artifact.rs`

- [ ] **Step 0: Inventory every generation-producing entry point**

Run:

```bash
rg -n 'create_dir_all\(.+generations|generation_path\(|GenerationMetadata \{|write_to\(&gen_dir|root\.erofs|GENERATION_METADATA_FILE|build_generation_from_db|rebuild_generation_image' apps crates packaging
```

Expected:
- every place that materializes a generation directory or writes
  `.conary-gen.json` is identified
- each producer is either routed through the new artifact staging path or
  explicitly classified as producing pre-export-contract data that must fail
  closed in the loader
- runtime generation build, recovery rebuild, bootstrap EROFS output, and
  tests/fixtures are all accounted for before implementation continues

- [ ] **Step 1: Add runtime generation artifact test**

Add a focused test in `generation/builder.rs` or `generation/artifact.rs` using a temp generations root and synthetic DB/file entries. The test should assert:

- `.conary-artifact.json` exists
- `cas-manifest.json` exists
- `boot-assets/manifest.json` exists
- `.conary-gen.json` includes `artifact_manifest_sha256`
- `load_generation_artifact()` accepts the generated directory

If a full DB setup is too heavy, add unit tests around a new helper that takes already-collected `FileEntryRef` values and boot asset source paths.

- [ ] **Step 2: Run test and verify red**

Run:

```bash
cargo test -p conary-core generation::builder --features composefs-rs
```

Expected: fails because runtime builder does not write artifact manifests.

- [ ] **Step 3: Add runtime boot asset staging helper**

In `generation/artifact.rs` or a small private helper in `generation/builder.rs`, implement:

```rust
fn stage_runtime_boot_assets(gen_dir: &Path, kernel_version: &str) -> crate::Result<BootAssetsManifest>;
```

For this slice, source paths are:

- `/boot/vmlinuz-{kernel_version}`
- `/boot/initramfs-{kernel_version}.img`
- `/boot/EFI/BOOT/BOOTX64.EFI`

`kernel_version` should come from the kernel package selected by the current
generation transaction. Recovery rebuilds may fall back to the running
`uname -r` only when no transaction-selected kernel version is available, and
must log that fallback.

If any are missing, generation build fails with an actionable error. This is generation-build time, not export time, so it is allowed to read host `/boot`.

The copy must dereference source symlinks and write plain regular files under
`boot-assets/`. After copying, verify each destination with
`symlink_metadata()` and reject anything that is not a regular file.

- [ ] **Step 4: Write runtime CAS manifest**

Use `file_refs` from `build_generation_from_db()` to generate deduplicated, sorted `CasObjectRef` entries.

- [ ] **Step 5: Write runtime artifact and metadata**

Update `build_generation_from_db()` sequence so artifact writing happens before metadata writing, and metadata includes `artifact_manifest_sha256`.

Update `rebuild_generation_image()` similarly, because recovery should not recreate incomplete modern generation metadata.

- [ ] **Step 6: Run runtime generation tests**

Run:

```bash
cargo test -p conary-core generation::builder --features composefs-rs
cargo test -p conary-core generation::artifact --features composefs-rs
```

Expected: tests pass.

- [ ] **Step 7: Commit runtime producer**

```bash
git add crates/conary-core/src/generation/builder.rs crates/conary-core/src/generation/artifact.rs
git commit -m "feat(generation): stage export artifacts during builds"
```

## Chunk 4: Shared Image Backend And Projections

### Task 8: Move repart into a shared raw image backend

**Files:**
- Create: `crates/conary-core/src/image/mod.rs`
- Create: `crates/conary-core/src/image/size.rs`
- Create: `crates/conary-core/src/image/repart.rs`
- Modify: `crates/conary-core/src/lib.rs`
- Modify: `crates/conary-core/src/bootstrap/image.rs`
- Modify or delete: `crates/conary-core/src/bootstrap/repart.rs`

- [ ] **Step 1: Add shared backend tests**

Create tests in `crates/conary-core/src/image/repart.rs` for:

- ESP partition definition copies staged ESP into `/`
- root partition definition copies staged root into `/`
- root partition uses `ext4`
- root label is `CONARY_ROOT`
- ESP label is `CONARY_ESP`
- x86_64 root partition type is `root-x86-64`
- `ImageSize` parsing accepts existing bootstrap size syntax from the new
  `crate::image::size` module
- root partition filesystem and BLS `rootfstype=` both read from one shared
  source, such as `crate::image::repart::ROOT_FILESYSTEM`, so changing one
  side without the other fails a test

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary-core image::repart
cargo test -p conary-core image::size
```

Expected: module does not exist yet.

- [ ] **Step 3: Add `image` module**

Modify `crates/conary-core/src/lib.rs`:

```rust
pub mod image;
```

Create `crates/conary-core/src/image/mod.rs`:

```rust
// crates/conary-core/src/image/mod.rs
//! Shared disk image planning and materialization.

pub mod repart;
pub mod size;
```

- [ ] **Step 4: Move repart definitions**

Move the existing bootstrap-owned `ImageSize` parser into
`crates/conary-core/src/image/size.rs` and update bootstrap imports to use
`crate::image::size::ImageSize`. Do not create a second local parser.

Move `RepartDefinition` and `generate_repart_definitions()` into `crates/conary-core/src/image/repart.rs`.
Define a single root filesystem source of truth, for example:

```rust
pub const ROOT_FILESYSTEM: &str = "ext4";
```

Use this constant for the root partition definition and for generation export's
BLS `rootfstype=` value.

Adjust the generator so it accepts a plan/source shape rather than assuming bootstrap ownership:

```rust
pub struct DiskImagePlan {
    pub architecture: TargetArch,
    pub esp_staging_dir: PathBuf,
    pub root_staging_dir: PathBuf,
    pub output_raw: PathBuf,
    pub size_bytes: u64,
}
```

If moving all raw materialization at once is too much, keep `create_raw_image()` in this task minimal and move only partition definitions first.

- [ ] **Step 5: Update bootstrap image builder imports**

Update `crates/conary-core/src/bootstrap/image.rs` to call `crate::image::repart::generate_repart_definitions(...)`.

Either delete `bootstrap/repart.rs` and remove `pub mod repart`, or leave a temporary re-export:

```rust
pub use crate::image::repart::*;
```

Use deletion if no code still imports `bootstrap::repart`.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p conary-core image::repart
cargo test -p conary-core image::size
cargo test -p conary-core bootstrap::image
```

Expected: tests pass.

- [ ] **Step 7: Commit shared repart module**

```bash
git add crates/conary-core/src/lib.rs crates/conary-core/src/image/mod.rs crates/conary-core/src/image/size.rs crates/conary-core/src/image/repart.rs crates/conary-core/src/bootstrap/image.rs crates/conary-core/src/bootstrap/repart.rs crates/conary-core/src/bootstrap/mod.rs
git commit -m "refactor(image): share repart backend"
```

### Task 9: Implement ESP and rootfs projection

**Files:**
- Create: `crates/conary-core/src/generation/export.rs`
- Modify: `crates/conary-core/src/generation/mod.rs`

- [ ] **Step 1: Write projection tests**

In `generation/export.rs`, add tests that build a synthetic `GenerationArtifact` and assert:

- rootfs projection creates `/conary/generations/<N>/root.erofs`
- rootfs projection copies `.conary-gen.json`, `.conary-artifact.json`, `cas-manifest.json`, and `boot-assets/`
- rootfs projection creates `/conary/current -> generations/<N>`
- rootfs projection copies only manifest-listed CAS objects
- rootfs projection creates `/conary/etc-state`
- rootfs projection creates runtime mountpoints
- rootfs projection creates usr-merge symlinks from `ROOT_SYMLINKS`
- ESP projection writes `EFI/BOOT/BOOTX64.EFI`, `vmlinuz`, `initramfs.img`
- ESP projection writes `loader/loader.conf` with `default conary-gen-<N>`, `timeout 3`, `console-mode max`, `editor no`
- ESP projection writes BLS options with `root=PARTLABEL=CONARY_ROOT`, `rootfstype=ext4`, `rw`, `conary.generation=<N>`, `console=tty0`, and `console=ttyS0`
- ESP projection writes BLS `sort-key conary-<N>`
- ESP projection rejects unsupported architectures before writing partial output

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary-core generation::export
```

Expected: module does not exist or tests fail.

- [ ] **Step 3: Add module export**

Modify `crates/conary-core/src/generation/mod.rs`:

```rust
pub mod export;
```

- [ ] **Step 4: Implement projection functions**

In `generation/export.rs`, implement:

```rust
pub fn project_generation_rootfs(
    artifact: &GenerationArtifact,
    staging_dir: &Path,
) -> crate::Result<PathBuf>;

pub fn project_generation_esp(
    artifact: &GenerationArtifact,
    staging_dir: &Path,
) -> crate::Result<PathBuf>;
```

Both functions return the root of the staging tree they created.

- [ ] **Step 5: Run projection tests**

Run:

```bash
cargo test -p conary-core generation::export
```

Expected: projection tests pass.

- [ ] **Step 6: Commit projections**

```bash
git add crates/conary-core/src/generation/export.rs crates/conary-core/src/generation/mod.rs
git commit -m "feat(generation): project export staging trees"
```

### Task 10: Implement raw/qcow2 export orchestration

**Files:**
- Modify: `crates/conary-core/src/generation/export.rs`
- Modify: `crates/conary-core/src/image/repart.rs`

- [ ] **Step 1: Add export orchestration tests**

Add tests for:

- `iso` returns explicit reserved/not-implemented error
- `aarch64` and `riscv64` return unsupported architecture before image materialization
- computed minimum size includes GPT overhead, fixed ESP, root staging, CAS object size, and margin
- user-provided `--size` below minimum returns requested and minimum sizes in the error
- raw export calls the shared raw backend with staged ESP/rootfs
- qcow2 export converts raw through `qemu-img`
- temp staging directories are cleaned on success and failure
- qcow2 raw temp files are removed on conversion success and failure

For command-running tests, inject a small trait or command-path struct so tests can use fake commands rather than requiring real `systemd-repart`/`qemu-img`.

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary-core generation::export
```

Expected: orchestration tests fail because export entrypoint is missing.

- [ ] **Step 3: Implement export API**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationExportFormat {
    Raw,
    Qcow2,
    Iso,
}

pub struct GenerationExportOptions {
    pub generation: Option<i64>,
    pub generation_path: Option<PathBuf>,
    pub format: GenerationExportFormat,
    pub output: PathBuf,
    pub size_bytes: Option<u64>,
}

pub struct GenerationExportResult {
    pub path: PathBuf,
    pub format: GenerationExportFormat,
    pub size: u64,
    pub raw_path: Option<PathBuf>,
}

pub fn export_generation_image(options: GenerationExportOptions) -> crate::Result<GenerationExportResult>;
```

- [ ] **Step 4: Implement format parsing**

Support exactly:

- `raw`
- `qcow2`
- `iso`

Reject any other format with `expected raw, qcow2, or iso`.

- [ ] **Step 5: Implement raw export**

Flow:

1. load `GenerationArtifact`
2. create temp staging dir next to output or under `std::env::temp_dir()`
   using a cleanup guard such as `tempfile::TempDir`
3. project rootfs
4. project ESP
5. compute minimum size
6. call shared raw backend
7. return result after the staging tree has been cleaned on success or failure

- [ ] **Step 6: Implement qcow2 export**

Flow:

1. create raw output path as `<output>.raw.tmp`
2. call raw export internals
3. run `qemu-img convert -f raw -O qcow2 -c <raw> <output>`
4. remove temp raw after successful conversion
5. also remove temp raw on conversion failure before returning the error
6. return qcow2 result

- [ ] **Step 7: Implement ISO reserved error**

Return exact message:

```text
ISO export is reserved on the generation artifact contract but not implemented yet
```

- [ ] **Step 8: Run core export tests**

Run:

```bash
cargo test -p conary-core generation::export
cargo test -p conary-core image::repart
```

Expected: tests pass.

- [ ] **Step 9: Commit export core**

```bash
git add crates/conary-core/src/generation/export.rs crates/conary-core/src/image/repart.rs
git commit -m "feat(generation): export artifacts as disk images"
```

## Chunk 5: CLI Integration And Docs

### Task 11: Wire `conary system generation export` to core export

**Files:**
- Modify: `apps/conary/src/commands/generation/export.rs`
- Modify: `apps/conary/src/dispatch.rs`

- [ ] **Step 1: Add command behavior tests**

Add unit tests in `apps/conary/src/commands/generation/export.rs` for:

- format parsing errors mention `raw, qcow2, or iso`
- `--path` and generation number conflict is already covered by CLI test
- ISO returns the reserved error
- undersized image errors are surfaced without panic

Use temp directories and synthetic artifact fixtures from `conary_core::generation::artifact` test helpers if exposed under `#[cfg(test)]`.

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary --bin conary commands::generation::export
```

Expected: tests fail while command wrapper is still a stub.

- [ ] **Step 3: Implement command wrapper**

`cmd_generation_export()` should:

- parse `format`
- parse optional `size` using `conary_core::image::size::ImageSize`
- call `conary_core::generation::export::export_generation_image`
- print output path, format, size, and method
- return the explicit ISO error unchanged

- [ ] **Step 4: Run CLI and command tests**

Run:

```bash
cargo test -p conary --bin conary cli::tests::cli_accepts_generation_export_from_explicit_path
cargo test -p conary --bin conary commands::generation::export
cargo build -p conary
```

Expected: tests and build pass.

- [ ] **Step 5: Commit CLI integration**

```bash
git add apps/conary/src/commands/generation/export.rs apps/conary/src/dispatch.rs
git commit -m "feat(generation): wire disk export command"
```

### Task 12: Update docs and active guidance

**Files:**
- Modify: `docs/modules/bootstrap.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/superpowers/specs/2026-04-22-generation-artifact-export-unification-design.md` only if implementation details intentionally diverge

- [ ] **Step 1: Update bootstrap module docs**

In `docs/modules/bootstrap.md`:

- remove any guidance implying `conary bootstrap image` wraps generation output
- add `conary system generation export --path ./output/generations/1 --format qcow2 --output gen1.qcow2`
- keep `conary bootstrap image --format erofs` as generation artifact production, not disk export

- [ ] **Step 2: Update integration testing docs**

In `docs/INTEGRATION-TESTING.md`, mention:

```bash
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora43 --phase 3
```

Only add this after the manifest exists in Task 13.

- [ ] **Step 3: Search for stale legacy docs**

Run:

```bash
rg -n 'build_from_generation|--from-generation|from_generation:\s*Option<String>|wrap in a qcow2|generation image path' docs apps crates
```

Expected:
- no active doc or code references to removed legacy behavior
- references in the design/plan are historical or explicit regression assertions

- [ ] **Step 4: Commit docs**

```bash
git add docs/modules/bootstrap.md docs/INTEGRATION-TESTING.md docs/superpowers/specs/2026-04-22-generation-artifact-export-unification-design.md
git commit -m "docs: document generation export flow"
```

## Chunk 6: QEMU Validation And Final Gates

### Task 13: Add generation export QEMU suite

**Files:**
- Modify: `apps/conary-test/src/config/manifest.rs`
- Modify: `apps/conary-test/src/engine/qemu.rs`
- Modify: `apps/conary-test/src/engine/variables.rs`
- Create: `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml`
- Modify: `docs/INTEGRATION-TESTING.md`

- [ ] **Step 1: Add failing conary-test support tests for generated images**

The existing `qemu_boot` step boots named cached artifacts only. Add tests first
for two new capabilities:

- `qemu_boot.local_image_path` boots a qcow2 already present on the host
- `qemu_boot.copy_from_guest` copies a generated image from the guest to the host after commands finish

In `apps/conary-test/src/config/manifest.rs`, add parser tests for:

```toml
[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v2"
local_image_path = "/tmp/generated.qcow2"
copy_from_guest = [
  { source = "/tmp/out.qcow2", dest = "/tmp/conary-generation-export/host-out.qcow2" },
]
commands = ["true"]
```

In `apps/conary-test/src/engine/variables.rs`, add expansion tests for both new fields.
In `apps/conary-test/src/engine/qemu.rs`, add a unit test or fake-command test
showing that `copy_from_guest.dest` parent directories are created before
`scp` runs.

- [ ] **Step 2: Run tests and verify red**

Run:

```bash
cargo test -p conary-test config::manifest::validation_tests::test_qemu_boot_local_image_and_copy_fields_parse
cargo test -p conary-test engine::variables::tests::test_expand_qemu_boot_expands_local_image_and_copy_fields
```

Expected:
- tests fail because fields do not exist

- [ ] **Step 3: Implement manifest fields**

Modify `apps/conary-test/src/config/manifest.rs`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct QemuGuestCopy {
    pub source: String,
    pub dest: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QemuBoot {
    pub image: String,
    #[serde(default)]
    pub local_image_path: Option<String>,
    #[serde(default)]
    pub copy_from_guest: Vec<QemuGuestCopy>,
    #[serde(default = "default_qemu_memory")]
    pub memory_mb: u32,
    #[serde(default = "default_qemu_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    pub commands: Vec<String>,
    #[serde(default)]
    pub expect_output: Vec<String>,
}
```

Keep `image` required for existing manifests and use it as the cache artifact
name unless `local_image_path` is set.

- [ ] **Step 4: Implement local image booting**

Modify `apps/conary-test/src/engine/qemu.rs`:

- if `local_image_path` is `Some`, use that path directly and do not download from Remi
- if `local_image_path` is missing, preserve existing named-image download/cache behavior
- fail clearly if `local_image_path` is set but the file does not exist

- [ ] **Step 5: Implement guest-to-host copy**

After all SSH commands finish and before shutting down QEMU, copy each
`copy_from_guest` entry with `scp` using the same SSH key/port logic as command
execution. A failed copy should make the step fail because the generated image
is part of the assertion. Before invoking `scp`, create the parent directory of
each host-side `dest` path with `fs::create_dir_all()` so a fresh host can run
the suite without precreating `/tmp/conary-generation-export`.

- [ ] **Step 6: Run conary-test QEMU support tests**

Run:

```bash
cargo test -p conary-test config::manifest
cargo test -p conary-test engine::variables
cargo test -p conary-test engine::qemu
```

Expected: all tests pass.

- [ ] **Step 7: Add QEMU manifest**

Create `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml`:

```toml
# tests/integration/remi/manifests/phase3-group-o-generation-export.toml

[suite]
name = "Generation Artifact Export QEMU"
phase = 3

[[test]]
id = "TGE01"
name = "installed_generation_export_fails_closed_without_self_contained_root"
description = "A runtime generation whose base OS is not represented in Conary CAS must fail before publishing a bootable artifact"
timeout = 900
group = "generation-export"
fatal = true

[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v2"
memory_mb = 2048
timeout_seconds = 900
ssh_port = 2240
commands = [
    "conary system init",
    "mkdir -p /var/tmp/conary-generation-export",
    "conary system generation build --allow-live-system-mutation > /var/tmp/conary-generation-export/build.log 2>&1; code=$?; cat /var/tmp/conary-generation-export/build.log; test \"$code\" -ne 0",
    "grep -q 'not self-contained' /var/tmp/conary-generation-export/build.log",
    "grep -q '/sbin/init' /var/tmp/conary-generation-export/build.log",
    "test ! -e /conary/generations/0/.conary-artifact.json",
    "echo installed-generation-export-failed-closed",
]
expect_output = [
    "installed-generation-export-failed-closed",
]

[test.step.assert]
exit_code = 0

[[test]]
id = "TGE02"
name = "bootstrap_run_generation_export_boots"
description = "Export a bootstrap-run generation artifact to qcow2, copy it to the host, then boot it under UEFI"
timeout = 1800
group = "generation-export"

[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v2"
memory_mb = 2048
timeout_seconds = 900
ssh_port = 2242
commands = [
    "conary repo sync fedora-remi --force",
    "conary install dosfstools --repo fedora-remi --yes --sandbox never --allow-live-system-mutation",
    "conary install qemu-img --repo fedora-remi --yes --sandbox never --allow-live-system-mutation",
    "test -d /var/lib/conary/bootstrap-inputs",
    "test -f /var/lib/conary/bootstrap-inputs/conaryos.toml",
    "test -d /var/lib/conary/bootstrap-inputs/seed",
    "mkdir -p /var/tmp/conary-generation-export",
    "conary bootstrap run /var/lib/conary/bootstrap-inputs/conaryos.toml --seed /var/lib/conary/bootstrap-inputs/seed --work-dir /var/tmp/bootstrap-run --up-to system",
    "conary system generation export --path /var/tmp/bootstrap-run/output/generations/1 --format qcow2 --output /var/tmp/conary-generation-export/bootstrap-run-generation.qcow2",
    "test -s /var/tmp/conary-generation-export/bootstrap-run-generation.qcow2",
    "echo bootstrap-run-generation-export-ok",
]
copy_from_guest = [
    { source = "/var/tmp/conary-generation-export/bootstrap-run-generation.qcow2", dest = "/tmp/conary-generation-export/bootstrap-run-generation.qcow2" },
]
expect_output = [
    "bootstrap-run-generation-export-ok",
]

[test.step.assert]
exit_code = 0

[[test.step]]
[test.step.qemu_boot]
image = "local-bootstrap-run-generation-export"
local_image_path = "/tmp/conary-generation-export/bootstrap-run-generation.qcow2"
memory_mb = 2048
timeout_seconds = 420
ssh_port = 2243
commands = [
    "grep -q 'conary.generation=1' /proc/cmdline",
    "TARGET=$(readlink /conary/current); test \"$TARGET\" = \"generations/1\" || test \"$TARGET\" = \"1\" || test \"$(readlink -f /conary/current)\" = \"/conary/generations/1\"",
    "test -f /conary/generations/1/.conary-artifact.json",
    "test -f /conary/generations/1/cas-manifest.json",
    "test -f /conary/generations/1/boot-assets/manifest.json",
    "echo bootstrap-run-generation-export-booted",
]
expect_output = [
    "bootstrap-run-generation-export-booted",
]

[test.step.assert]
exit_code = 0
```

If `/var/lib/conary/bootstrap-inputs` does not exist in the current test image,
do not merge an incomplete test. Add the missing fixture staging to the test
image or create a dedicated `minimal-bootstrap-inputs-v1` QEMU artifact as
part of this task.

Before committing the manifest, verify the command spelling in the source
image with `conary system generation build --help`, `conary system generation
export --help`, and `conary bootstrap run --help`. If
`--allow-live-system-mutation` or `--up-to system` has drifted, update the
manifest and CLI docs instead of preserving stale flags. Also verify the source
image has the tools needed by the first QEMU step, especially `qemu-img`,
`mkfs.vfat` from `dosfstools` for ESP formatting, a working kernel package
channel for `conary install kernel`, and whatever bootstrap fixture work is
needed to stage `BOOTX64.EFI`.

- [ ] **Step 8: Run manifest inventory**

Run:

```bash
cargo run -p conary-test -- list
```

Expected:
- the new suite appears in the list
- no manifest parse errors
- the manifest follows the phase/group filename convention in the suite list
- the named source image `minimal-boot-v2` resolves through the existing QEMU
  image cache/download path; if it does not, update the fixture image before
  running the suite

- [ ] **Step 9: Run QEMU suite**

Run:

```bash
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora43 --phase 3
```

Expected:
- passes when QEMU tooling and images are available
- if host QEMU prerequisites are missing, the suite should skip through existing QEMU skip behavior rather than failing with a misleading product error

- [ ] **Step 10: Update integration docs**

Add the command from Step 9 to `docs/INTEGRATION-TESTING.md`.

- [ ] **Step 11: Commit QEMU validation**

```bash
git add apps/conary-test/src/config/manifest.rs apps/conary-test/src/engine/qemu.rs apps/conary-test/src/engine/variables.rs apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml docs/INTEGRATION-TESTING.md
git commit -m "test(generation): add export boot validation"
```

### Task 14: Run final verification and cleanup

**Files:**
- Read: all touched files

- [ ] **Step 1: Run targeted tests**

Run:

```bash
cargo test -p conary --bin conary cli::tests::cli_rejects_bootstrap_image_from_generation
cargo test -p conary --bin conary cli::tests::cli_accepts_generation_export_from_explicit_path
cargo test -p conary --bin conary commands::generation::export
cargo test -p conary-core generation::metadata
cargo test -p conary-core generation::artifact --features composefs-rs
cargo test -p conary-core generation::export --features composefs-rs
cargo test -p conary-core image::repart
cargo test -p conary-core image::size
cargo test -p conary-core bootstrap::image --features composefs-rs
cargo test -p conary-test config::manifest
```

Expected: all targeted tests pass.

- [ ] **Step 2: Run full owning-package checks**

Run:

```bash
cargo build -p conary
cargo build -p conary-core
cargo build -p conary-test
cargo test -p conary-core --features composefs-rs
cargo test -p conary --bin conary
cargo run -p conary-test -- list
```

Expected: all commands exit `0`.

- [ ] **Step 3: Run final workspace gates**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Expected: both commands exit `0`.

- [ ] **Step 4: Run regression searches**

Run:

```bash
rg -n 'build_from_generation|--from-generation|from_generation:\s*Option<String>' apps crates docs --glob '!docs/superpowers/specs/2026-04-22-generation-artifact-export-unification-design.md' --glob '!docs/superpowers/plans/2026-04-22-generation-artifact-export-unification-plan.md'
rg -n 'Command::new\("sfdisk"\)|Command::new\("mkfs\.fat"\)|Command::new\("mount"\)|Command::new\("umount"\)|"/boot|/boot/' crates/conary-core/src/generation/export.rs apps/conary/src/commands/generation/export.rs
rg -n 'std::fs::(read|read_to_string|copy)\([^)]*"/conary|File::open\([^)]*"/conary|Path::new\("/conary' crates/conary-core/src/generation/export.rs apps/conary/src/commands/generation/export.rs
```

Expected:
- first search has no active legacy references
- second search has no generation export code using direct `sfdisk`, `mkfs.fat`,
  `mount`, `umount`, or host `/boot` reads
- third search has no live host `/conary` reads or copies during export; staged
  rootfs projection paths are allowed, but direct reads from absolute
  `/conary` are not

- [ ] **Step 5: Inspect working tree**

Run:

```bash
git status --short
git log --oneline --max-count=10
```

Expected:
- only intentional changes remain
- implementation commits are small and ordered by task

- [ ] **Step 6: Final commit if needed**

If final verification required tiny cleanup changes:

```bash
git add <changed-files>
git commit -m "fix(generation): finalize export validation"
```

## Execution Notes

- If `composefs-rs` feature behavior differs locally, keep the contract tests feature-gated the same way existing EROFS tests are gated.
- If QEMU boot validation cannot run locally due to missing host tools, record the exact skip/failure output and do not claim QEMU validation passed.
- If an updated crate or API breaks during implementation, fix the breakage. Do not pin crates backward to avoid the work.
- If the plan discovers that bootstrap-run output cannot yet create a real self-contained generation fixture for QEMU, stop and either add the missing fixture production as part of this slice or explicitly downgrade that QEMU case to a documented follow-up with user approval.
