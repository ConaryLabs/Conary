# Composefs-Native Plan 2: Supporting Systems

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire up EROFS rebuild/mount at all deferred TODO sites, implement CAS GC via DB queries, add boot recovery, and /etc three-way merge.

**Architecture:** Every package mutation (install, remove, restore, rollback) must end with `build_generation_from_db()` + `mount_generation()`. CAS GC uses DB queries instead of nlink counting. Boot recovery rebuilds from DB if needed.

**Tech Stack:** Rust 1.94, composefs-rs, SQLite, composefs (kernel 6.6+)

**Spec:** `docs/superpowers/specs/2026-03-17-composefs-native-architecture-design.md`

---

## Task 1: Wire up EROFS rebuild/mount at all 8 TODO sites

**Files:**
- Modify: `src/commands/install/mod.rs:1558`
- Modify: `src/commands/install/batch.rs:456`
- Modify: `src/commands/remove.rs:288`
- Modify: `src/commands/restore.rs:105, 204`
- Modify: `src/commands/system.rs:277, 373, 480`

At each TODO site, replace the comment + info log with:
```rust
// Build new EROFS generation from updated DB state
let generations_dir = conary_core::generation::metadata::generations_dir();
let (gen_num, build_result) = conary_core::generation::builder::build_generation_from_db(
    &conn, &generations_dir, "summary of operation",
)?;
info!("Built generation {gen_num} ({} bytes, {} CAS objects)",
    build_result.image_size, build_result.cas_objects_referenced);

// Mount the new generation
conary_core::generation::mount::mount_generation(
    &conary_core::generation::mount::MountOptions {
        image_path: build_result.image_path,
        basedir: PathBuf::from("/conary/objects"),
        mount_point: PathBuf::from("/conary/mnt"),
        verity: false,
        digest: None,
        upperdir: Some(PathBuf::from("/conary/etc-state/upper")),
        workdir: Some(PathBuf::from("/conary/etc-state/work")),
    },
)?;
conary_core::generation::mount::update_current_symlink(
    Path::new("/conary"), gen_num,
)?;
```

Adapt the summary string per operation (install, remove, restore, rollback). Extract a helper function to avoid repeating this block 8 times.

- [ ] Step 1.1: Create helper function `rebuild_and_mount(conn, summary) -> Result<i64>`
- [ ] Step 1.2: Wire up install/mod.rs
- [ ] Step 1.3: Wire up install/batch.rs
- [ ] Step 1.4: Wire up remove.rs
- [ ] Step 1.5: Wire up restore.rs (2 sites)
- [ ] Step 1.6: Wire up system.rs (3 sites)
- [ ] Step 1.7: Verify build + tests
- [ ] Step 1.8: Commit

---

## Task 2: CAS GC via DB queries

**Files:**
- Modify: `src/commands/generation/commands.rs` (gc command)
- Modify or create: `conary-core/src/generation/gc.rs`

Replace nlink-based CAS liveness with the DB query from the spec:
```sql
SELECT DISTINCT f.sha256_hash FROM files f
JOIN troves t ON f.trove_id = t.id
JOIN state_members sm ON sm.trove_name = t.name AND sm.trove_version = t.version
WHERE sm.state_id IN (surviving generation state IDs)
```

- [ ] Step 2.1: Add `conary-core/src/generation/gc.rs` with `live_cas_hashes()` and `gc_cas_objects()`
- [ ] Step 2.2: Write tests for live_cas_hashes query
- [ ] Step 2.3: Wire into generation gc command
- [ ] Step 2.4: Verify + commit

---

## Task 3: Boot recovery

**Files:**
- Create: `deploy/dracut/module-setup.sh` (Dracut module)
- Create: `deploy/dracut/mount-conary.sh` (mount script)
- Modify: `conary-core/src/transaction/mod.rs` (improve recover())

The recover() method already exists from Plan 1 (with P0 fixes). This task adds the Dracut integration and improves the recovery logic to handle the 4-step fallback from the spec.

- [ ] Step 3.1: Improve recover() with 4-step fallback (try current, rebuild from DB, scan for intact image)
- [ ] Step 3.2: Create Dracut module that calls `conary generation recover` on boot
- [ ] Step 3.3: Test recover() with various failure scenarios
- [ ] Step 3.4: Commit

---

## Task 4: /etc three-way merge

**Files:**
- Create: `conary-core/src/generation/etc_merge.rs`

Compare previous generation's /etc (EROFS lower), new generation's /etc (EROFS lower), and current upper layer. Detect conflicts per the spec.

- [ ] Step 4.1: Create etc_merge.rs with `plan_etc_merge()` function
- [ ] Step 4.2: Implement three-way comparison logic
- [ ] Step 4.3: Wire into the rebuild_and_mount helper (between EROFS build and mount)
- [ ] Step 4.4: Test with mock /etc scenarios
- [ ] Step 4.5: Commit

---

## Task 5: Cleanup and verification

- [ ] Step 5.1: cargo clippy -- -D warnings
- [ ] Step 5.2: cargo fmt --check
- [ ] Step 5.3: cargo test (all pass)
- [ ] Step 5.4: Commit
