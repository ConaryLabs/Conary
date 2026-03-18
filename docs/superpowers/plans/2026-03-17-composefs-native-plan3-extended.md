# Composefs-Native Plan 3: Extended Capabilities

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** EROFS-native bootstrap output, binary deltas between generations, OCI container export.

**Architecture:** Bootstrap produces CAS + EROFS + DB instead of qcow2. Deltas are binary diffs of deterministic EROFS images. OCI export wraps EROFS + CAS in OCI format.

**Tech Stack:** Rust 1.94, composefs-rs (composefs-oci for OCI), SQLite

**Spec:** `docs/superpowers/specs/2026-03-17-composefs-native-architecture-design.md`

---

## Task 1: Bootstrap EROFS output

**Files:**
- Modify: `conary-core/src/bootstrap/image.rs`
- Modify: `conary-core/src/bootstrap/mod.rs`

Change bootstrap pipeline to output EROFS + CAS + DB instead of qcow2. The bootstrap output becomes "generation 1" — same artifact type as any runtime generation.

- [ ] Step 1.1: Modify image builder to use build_generation_from_db after populating CAS + DB
- [ ] Step 1.2: Output CAS directory + root.erofs + db.sqlite3 + boot config
- [ ] Step 1.3: Keep qcow2 wrapping as optional (for VM images)
- [ ] Step 1.4: Test bootstrap dry-run produces EROFS
- [ ] Step 1.5: Commit

---

## Task 2: EROFS delta support

**Files:**
- Create: `conary-core/src/generation/delta.rs`

Binary diff between deterministic EROFS images for efficient updates.

- [ ] Step 2.1: Add bsdiff/zstd dependency for binary patching
- [ ] Step 2.2: Implement `compute_delta(old_image, new_image) -> Vec<u8>`
- [ ] Step 2.3: Implement `apply_delta(old_image, delta) -> Vec<u8>`
- [ ] Step 2.4: Test roundtrip: build two images, compute delta, apply, verify identical
- [ ] Step 2.5: Commit

---

## Task 3: OCI container export

**Files:**
- Create: `src/commands/export.rs`
- Modify: `src/cli/mod.rs` (add export subcommand)

`conary export --oci` wraps a generation's EROFS + CAS as an OCI container image.

- [ ] Step 3.1: Evaluate composefs-oci crate for OCI framing
- [ ] Step 3.2: Implement export command with OCI manifest + layer assembly
- [ ] Step 3.3: Test: export, then verify with skopeo/podman inspect
- [ ] Step 3.4: Commit

---

## Task 4: Cleanup and verification

- [ ] Step 4.1: cargo clippy + fmt + test
- [ ] Step 4.2: Commit
