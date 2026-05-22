# Goal 6 Recovery, Boot, And Artifact Trust Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden generation recovery, bootable artifact export, output provenance, and self-host validation freshness for daily-driver use.

**Architecture:** Keep the existing `GenerationArtifact` source contract as the only input to bootable artifact export. Add output-side provenance manifests for raw/qcow2/ISO without weakening source artifact validation, implement ISO as a UEFI bootable generation carrier, teach the QEMU test harness to boot ISO outputs, and make self-host validation fail before QEMU when staged inputs are stale.

**Tech Stack:** Rust (`conary-core`, `conary`, `conary-test`), shell wrappers under `scripts/bootstrap-vm`, QEMU/OVMF, systemd-boot, xorriso, mkfs.vfat, mtools.

---

## File Structure

- Modify `crates/conary-core/src/generation/export.rs`: output provenance manifest writing; ISO export backend; ISO staging helpers; tests with fake tools.
- Modify `apps/conary/src/commands/generation/export.rs`: CLI output for provenance sidecar and ISO method text.
- Modify `packaging/dracut/90conary/conary-init.sh`: read-only carrier root option for ISO-booted runtime generations.
- Modify `packaging/dracut/90conary/conary-generator.sh`: tmpfs-backed `/etc` overlay upper/work when carrier root is read-only.
- Modify `crates/conary-core/src/bootstrap/system_config.rs`: keep bootstrap-run initramfs behavior aligned with the packaged dracut scripts.
- Modify `apps/conary-test/src/config/manifest.rs`, `apps/conary-test/src/engine/qemu.rs`, and `apps/conary-test/src/engine/variables.rs`: optional QEMU image format support for ISO boot.
- Add `apps/conary/tests/integration/remi/manifests/phase3-group-p-iso-export.toml`: focused ISO export QEMU proof.
- Modify `scripts/local-qemu-validation.sh`: include Group P and result-gate failed/skipped/cancelled outcomes consistently.
- Modify `scripts/bootstrap-vm/validate-selfhost-vm.sh`: fail before QEMU when workspace tarball checksum is invalid or stale versus current checkout.
- Modify `scripts/bootstrap-vm/test-validate-selfhost-vm.sh`: stale-input regression coverage.
- Update `README.md`, `ROADMAP.md`, `docs/INTEGRATION-TESTING.md`, `docs/modules/bootstrap.md`, `docs/operations/post-generation-export-follow-up-roadmap.md`, and doc audit metadata.

## Task 1: Output Provenance Sidecars For Raw And Qcow2

**Files:**
- Modify: `crates/conary-core/src/generation/export.rs`
- Modify: `apps/conary/src/commands/generation/export.rs`

- [x] **Step 1: Write the failing raw provenance test**

Add a test near the existing `export_raw_uses_repart_backend` style tests:

```rust
#[cfg(unix)]
#[test]
fn raw_export_writes_output_provenance_manifest() {
    let fixture = Fixture::new();
    let tools = fake_tools(fixture._tmp.path());
    let output = fixture._tmp.path().join("gen.raw");

    let result = export_generation_image_with_tools(
        GenerationExportOptions {
            generation: None,
            generation_path: Some(fixture.generation_dir.clone()),
            format: GenerationExportFormat::Raw,
            output: output.clone(),
            size_bytes: None,
        },
        &tools,
    )
    .unwrap();

    let manifest_path = output.with_extension("raw.conary-provenance.json");
    assert_eq!(result.provenance_path.as_deref(), Some(manifest_path.as_path()));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["generation"], 7);
    assert_eq!(manifest["architecture"], "x86_64");
    assert_eq!(manifest["format"], "raw");
    assert_eq!(manifest["output"]["path"], output.display().to_string());
    assert_eq!(manifest["output"]["size"], 3);
    assert_eq!(manifest["output"]["sha256"], crate::hash::sha256(b"raw"));
    assert_eq!(
        manifest["source"]["artifact_manifest_sha256"],
        fixture.artifact().metadata.artifact_manifest_sha256.unwrap()
    );
}
```

- [x] **Step 2: Run the focused test and confirm RED**

Run:

```bash
cargo test -p conary-core raw_export_writes_output_provenance_manifest -- --exact
```

Expected: compile failure or assertion failure because `GenerationExportResult::provenance_path` and sidecar writing do not exist.

- [x] **Step 3: Implement sidecar manifest writing**

Add a `provenance_path: Option<PathBuf>` field to `GenerationExportResult`, write `<output>.<format>.conary-provenance.json`, and include `version`, `created_at`, `generation`, `architecture`, `format`, `source.artifact_manifest_sha256`, `source.generation_metadata`, `source.artifact_manifest`, `source.cas_manifest`, `source.boot_assets_manifest`, and `output.path/size/sha256`.

- [x] **Step 4: Run raw/qcow2 focused tests**

Run:

```bash
cargo test -p conary-core generation::export
cargo test -p conary --bin conary generation::export
```

Expected: all generation export tests pass.

## Task 2: Bootable ISO Export Backend

**Files:**
- Modify: `crates/conary-core/src/generation/export.rs`
- Modify: `apps/conary/src/commands/generation/export.rs`

- [x] **Step 1: Replace the former ISO-reservation test with a failing implementation test**

Update the old ISO placeholder test to expect a generated ISO and provenance manifest using fake `xorriso`, `mkfs.vfat`, `mmd`, and `mcopy` tools.

- [x] **Step 2: Run the ISO test and confirm RED**

Run:

```bash
cargo test -p conary-core iso_export_writes_bootable_generation_carrier -- --exact
```

Expected: failure because ISO export still returns the old not-implemented error.

- [x] **Step 3: Add ISO tools and backend**

Extend `GenerationExportTools` with `xorriso`, `mkfs_vfat`, `mmd`, and `mcopy`. Implement `export_iso` to project the generation root into an ISO staging directory, create an EFI FAT image containing `EFI/BOOT/BOOTX64.EFI`, `loader/loader.conf`, `loader/entries/conary-gen-N.conf`, `vmlinuz`, and `initramfs.img`, then run xorriso with UEFI El Torito options and write provenance.

- [x] **Step 4: Run focused export tests**

Run:

```bash
cargo test -p conary-core generation::export
cargo test -p conary --bin conary generation::export
cargo run -p conary -- system generation export --help
```

Expected: ISO is listed as implemented generation export.

## Task 3: Read-Only Carrier Root Boot Support

**Files:**
- Modify: `packaging/dracut/90conary/conary-init.sh`
- Modify: `packaging/dracut/90conary/conary-generator.sh`
- Modify: `crates/conary-core/src/bootstrap/system_config.rs`

- [x] **Step 1: Add script-content regression tests**

Extend existing initramfs content tests to require parsing `conary.carrier=readonly`, mounting ISO roots with `rootfstype=iso9660`, mounting tmpfs on `/sysroot/run`, and using `/sysroot/run/conary/etc-state` for overlay upper/work when the carrier is read-only.

- [x] **Step 2: Run tests and confirm RED**

Run:

```bash
cargo test -p conary-core bootstrap::system_config
```

Expected: failure because the generated initramfs does not contain the read-only carrier branches yet.

- [x] **Step 3: Implement read-only carrier mode**

Teach both dracut scripts and the bootstrap initramfs string to read `conary.carrier=readonly`. In that mode, mount the root read-only, mount a tmpfs at `/sysroot/run` if needed, and place generation `/etc` overlay upper/work under `/sysroot/run/conary/etc-state`.

- [x] **Step 4: Run script and generation tests**

Run:

```bash
cargo test -p conary-core bootstrap::system_config
cargo test -p conary-core generation::export
```

Expected: both pass.

## Task 4: QEMU Harness ISO Boot Support And Group P Manifest

**Files:**
- Modify: `apps/conary-test/src/config/manifest.rs`
- Modify: `apps/conary-test/src/engine/qemu.rs`
- Modify: `apps/conary-test/src/engine/variables.rs`
- Add: `apps/conary/tests/integration/remi/manifests/phase3-group-p-iso-export.toml`
- Modify: `scripts/local-qemu-validation.sh`

- [x] **Step 1: Write failing parser and qemu-args tests**

Add `image_format = "iso"` to `QemuBoot`, expand it through variables, and assert QEMU args use `-cdrom <path>` for ISO instead of `-drive file=...,format=qcow2`.

- [x] **Step 2: Run tests and confirm RED**

Run:

```bash
cargo test -p conary-test qemu_image_format
```

Expected: failure because `image_format` does not exist.

- [x] **Step 3: Implement QEMU image format support**

Support `qcow2` default, `raw`, and `iso`. Keep existing manifests unchanged by defaulting to qcow2. For ISO, pass `-cdrom <path>` and keep OVMF firmware enabled.

- [x] **Step 4: Add Group P ISO manifest**

Model the new manifest on `phase3-group-o-generation-export.toml`: build a bootstrap-run generation, export `--format iso`, copy the ISO and provenance sidecar out, then boot the ISO with `local_image_path` and `image_format = "iso"` and assert `conary.generation=1`, current symlink, artifact files, and an `iso-generation-export-booted` marker.

- [x] **Step 5: Run manifest inventory**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: `Phase 3 Group P: ISO Generation Export` appears with its test count.

## Task 5: Self-Host Validation Freshness Gate

**Files:**
- Modify: `scripts/bootstrap-vm/validate-selfhost-vm.sh`
- Modify: `scripts/bootstrap-vm/test-validate-selfhost-vm.sh`
- Modify: `docs/operations/bootstrap-selfhosting-vm.md`

- [x] **Step 1: Write stale-workspace shell regression**

Extend `test-validate-selfhost-vm.sh` so one invocation has a mismatched `conary-workspace.tar.gz.sha256` and asserts QEMU is not launched.

- [x] **Step 2: Run test and confirm RED**

Run:

```bash
bash scripts/bootstrap-vm/test-validate-selfhost-vm.sh
```

Expected: failure because validation currently does not reject stale input before QEMU.

- [x] **Step 3: Implement pre-QEMU freshness checks**

Before `trap cleanup EXIT` and before `start_qemu`, validate that the tarball matches its sidecar and that a freshly generated deterministic tarball from the current checkout hashes to the same digest. Use a temp file under `LOGS_DIR` and remove it after comparison.

- [x] **Step 4: Run shell tests**

Run:

```bash
bash scripts/bootstrap-vm/test-validate-selfhost-vm.sh
bash scripts/bootstrap-vm/test-build-selfhost-qcow2.sh
```

Expected: both pass.

## Task 5A: Selected-Generation Failure Coverage

**Files:**
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-composefs-modernization.toml`
- Modify: `crates/conary-core/src/transaction/mod.rs`

- [x] **Step 1: Expand activation and rollback failure assertions**

Update the composefs modernization manifest so failed `generation switch` leaves
`/conary/current` unchanged, both state rollback and generation rollback fail
without an active generation, and no selected generation pointer is created.

- [x] **Step 2: Add failed boot-selection recovery unit coverage**

Add a transaction recovery test that creates only a magic-only generation image,
runs explicit boot-selection recovery, and asserts it fails closed without
creating `/conary/current`.

- [x] **Step 3: Run focused selected-generation tests**

Run:

```bash
cargo test -p conary-core test_boot_selection_recovery_fails_without_valid_artifacts_and_preserves_missing_current
cargo run -p conary-test -- list
```

Expected: unit coverage passes and the Phase 3 composefs manifest still parses.

## Task 6: Docs, Audit Metadata, And Final Verification

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/modules/bootstrap.md`
- Modify: `docs/operations/post-generation-export-follow-up-roadmap.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [x] **Step 1: Update active docs**

Replace ISO follow-up wording with the exact implemented scope: x86_64 UEFI QEMU-validated generation-carrier ISO, output provenance sidecars for raw/qcow2/ISO, and non-x86_64 still reserved.

- [x] **Step 2: Run stale wording sweeps**

Run:

Run an `rg` sweep for the old ISO placeholder phrases across active docs, crates,
apps, and scripts.

Expected: only historical/archive or explicitly superseded references remain.

- [x] **Step 3: Run shared verification gate**

Run:

```bash
cargo fmt --check
cargo test -p conary --bin conary generation::export
cargo test -p conary-core generation::artifact
cargo test -p conary-core generation::export
cargo test -p conary-test qemu
cargo run -p conary-test -- list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: all pass.

- [ ] **Step 4: Run Goal 6 QEMU gates**

Run on a KVM-capable host:

```bash
scripts/local-qemu-validation.sh
cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3
```

Expected: zero failed, skipped, and cancelled results; Group N, Group O, and Group P markers appear.

2026-05-21 result: `scripts/local-qemu-validation.sh` passed composefs
modernization (2/2), Group N (5/5), and Group O (4/4). The focused
`cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3`
rerun passed `TISO01` with 1 passed / 0 failed / 0 skipped / 0 cancelled,
after provisioning ISO helpers, exporting a bootstrap-run generation to ISO,
copying the ISO plus provenance sidecar to
`target/local-validation/group-p-iso-export/`, booting the ISO under UEFI, and
proving the readonly-carrier `/etc` overlay.

## Self-Review Notes

- Spec coverage: output provenance, ISO export, QEMU ISO validation, self-host freshness, docs/audit, and Group N/O/P gates are represented.
- Scope guard: signed portable bundles and non-x86_64 boot assets remain out of scope.
- Ambiguity resolved: ISO means an x86_64 UEFI generation-carrier ISO, not installer media or broad hardware support.
- Current blocker: none for focused Group P evidence; keep the full
  verification gate green before marking the goal complete.
