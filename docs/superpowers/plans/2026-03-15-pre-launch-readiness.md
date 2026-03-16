# Pre-Launch Readiness Sweep Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all readiness gaps found in the pre-launch audit so that `cargo test` passes, every README claim is backed by working code, and new users hit no embarrassing errors.

**Architecture:** Nine independent fix tasks across production bugs (VFS, WAL), test updates (toolchain, container), UX improvements (error hints, capability commands), and documentation alignment (ROADMAP, README). Each task is self-contained and can be committed independently.

**Tech Stack:** Rust 1.94, rusqlite, clap, anyhow.

**Spec:** `docs/superpowers/specs/2026-03-15-pre-launch-readiness-design.md` (rev 2)

---

## File Map

| Action | File | Task |
|--------|------|------|
| Modify | `conary-core/src/bootstrap/toolchain.rs:407-430, 487-509` | T1: Fix toolchain tests |
| Modify | `conary-core/src/container/mod.rs:303-309, 1176-1203` | T2: Fix container pristine |
| Modify | `tests/target_root.rs:154-203` | T2: Fix integration test |
| Modify | `conary-core/src/filesystem/vfs/mod.rs:427-460` | T3: Fix VFS remove bug |
| Modify | `conary-core/src/db/mod.rs:43-78` | T4: Fix WAL validation bug |
| Modify | `src/cli/capability.rs:63, 86, 108` | T5: Un-hide capability commands |
| Modify | `src/main.rs:18-26` | T6: Centralized DB hint |
| Modify | `src/commands/repo.rs:53` | T7: Friendly repo add error |
| Modify | `ROADMAP.md` | T8: Update roadmap |
| Modify | `README.md` | T8: Update readme |
| Modify | `conary-server/Cargo.toml` | T8: Add version comment |
| Modify | `conary-test/Cargo.toml` | T8: Add version comment |

---

## Chunk 1: Production Bug Fixes + Test Fixes

### Task 1: Fix toolchain test assertions

**Files:**
- Modify: `conary-core/src/bootstrap/toolchain.rs:407-430, 487-509`

The `tool()` method now checks if the prefixed binary exists on disk and
falls back to unprefixed. Tests create a `Toolchain` pointing at `/tools/`
which doesn't exist, so the fallback always fires.

- [ ] **Step 1: Fix test_toolchain_tool_paths**

The test at line 407 asserts prefixed paths like
`/tools/bin/x86_64-conary-linux-gnu-gcc`. Since `/tools/bin/` doesn't exist,
`tool()` falls back to `/tools/bin/gcc`. Fix: create a temp dir with the
prefixed binaries, OR change the test to verify the fallback behavior:

```rust
#[test]
fn test_toolchain_tool_paths() {
    // Test with a temp directory where prefixed tools exist
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    // Create prefixed tool files so tool() finds them
    for name in &["gcc", "g++", "ar", "ld", "ranlib", "strip"] {
        let prefixed = bin_dir.join(format!("x86_64-conary-linux-gnu-{name}"));
        std::fs::write(&prefixed, "").unwrap();
    }

    let toolchain = Toolchain {
        kind: ToolchainKind::Stage0,
        path: tmp.path().to_path_buf(),
        target: "x86_64-conary-linux-gnu".to_string(),
        gcc_version: Some("13.3.0".to_string()),
        glibc_version: None,
        binutils_version: None,
        is_static: true,
    };

    assert_eq!(
        toolchain.gcc(),
        bin_dir.join("x86_64-conary-linux-gnu-gcc")
    );
    assert_eq!(
        toolchain.ar(),
        bin_dir.join("x86_64-conary-linux-gnu-ar")
    );
}

#[test]
fn test_toolchain_tool_paths_fallback() {
    // Test fallback to unprefixed when prefixed doesn't exist
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    // Only create unprefixed tools
    for name in &["gcc", "g++", "ar"] {
        std::fs::write(bin_dir.join(name), "").unwrap();
    }

    let toolchain = Toolchain {
        kind: ToolchainKind::Stage0,
        path: tmp.path().to_path_buf(),
        target: "x86_64-conary-linux-gnu".to_string(),
        gcc_version: None,
        glibc_version: None,
        binutils_version: None,
        is_static: true,
    };

    // Should fall back to unprefixed
    assert_eq!(toolchain.gcc(), bin_dir.join("gcc"));
    assert_eq!(toolchain.ar(), bin_dir.join("ar"));
}
```

- [ ] **Step 2: Fix test_toolchain_env similarly**

Update to use a temp dir with prefixed tools, or verify the env uses the
fallback paths correctly.

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-core bootstrap::toolchain -- --nocapture`
Expected: All pass

- [ ] **Step 4: Commit**

```
fix(test): update toolchain tests for tool() fallback behavior
```

---

### Task 2: Fix container pristine tests

**Files:**
- Modify: `conary-core/src/container/mod.rs:303-309, 1176-1203`
- Modify: `tests/target_root.rs:154-203`

`is_pristine()` at line 303 rejects mounts from `/bin` and `/lib64`, but
`pristine_for_bootstrap()` now adds those. Two fixes needed: update
`is_pristine()` to accept bootstrap host mounts, and update both tests.

- [ ] **Step 1: Update is_pristine() to accept bootstrap host paths**

The current check rejects `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`. But
bootstrap containers need `/usr/bin`, `/lib64`, etc. The distinction:
mounting ALL of `/usr` is not pristine, but mounting specific subdirectories
like `/usr/bin` for host tools IS acceptable.

Change `is_pristine()` to check for full system mounts, not individual
host tool directories:

```rust
pub fn is_pristine(&self) -> bool {
    // Pristine = no full system root mounts (mounting /usr/bin for
    // host tools is fine; mounting all of /usr is not)
    !self.bind_mounts.iter().any(|m| {
        let src = m.source.to_string_lossy();
        // Only reject broad system mounts, not specific subdirectories
        src == "/usr" || src == "/sbin" || src == "/"
    })
}
```

- [ ] **Step 2: Update unit test**

The test at line 1176 should still pass since `/usr/bin`, `/lib64`, etc.
are now accepted by `is_pristine()`.

- [ ] **Step 3: Update integration test in tests/target_root.rs**

Same change — the `assert!(config.is_pristine())` should now pass.

- [ ] **Step 4: Run tests**

Run: `cargo test -p conary-core container -- --nocapture && cargo test --test target_root -- --nocapture`
Expected: All pass

- [ ] **Step 5: Commit**

```
fix(container): update is_pristine() to accept bootstrap host tool mounts
```

---

### Task 3: Fix VFS remove production bug

**Files:**
- Modify: `conary-core/src/filesystem/vfs/mod.rs:427-460`

The `remove()` method processes `to_remove` sequentially, setting
`parent = None` on each node. Later `get_path(id)` calls for descendant
nodes fail because the parent chain is already broken.

- [ ] **Step 1: Fix the ordering — collect paths BEFORE mutating**

```rust
pub fn remove(&mut self, path: impl AsRef<Path>) -> Result<()> {
    let path = normalize_path(path.as_ref());

    if path == Path::new("/") {
        return Err(Error::InvalidPath("cannot remove root".into()));
    }

    let node_id = self
        .lookup(&path)
        .ok_or_else(|| Error::NotFound(format!("path not found: {}", path.display())))?;

    // Collect all descendants to remove
    let mut to_remove = Vec::new();
    self.collect_descendants(node_id, &mut to_remove);
    to_remove.push(node_id);

    // Remove from parent's children list
    let parent_id = self.get_node(node_id).parent.ok_or_else(|| {
        Error::InternalError("non-root node has no parent (corrupted VFS tree)".into())
    })?;
    let parent = self.get_node_mut(parent_id);
    parent.children.retain(|&id| id != node_id);

    // Collect all paths BEFORE mutating nodes (get_path needs parent chain intact)
    let paths_to_remove: Vec<PathBuf> = to_remove.iter().map(|&id| self.get_path(id)).collect();

    // Now remove paths from index and mark nodes as orphaned
    for (i, &id) in to_remove.iter().enumerate() {
        self.path_index.remove(&paths_to_remove[i]);
        self.get_node_mut(id).parent = None;
        self.get_node_mut(id).children.clear();
    }

    Ok(())
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p conary-core filesystem::vfs::tests::test_remove_directory_with_children -- --nocapture`
Expected: PASS

- [ ] **Step 3: Run full VFS tests**

Run: `cargo test -p conary-core filesystem::vfs -- --nocapture`
Expected: All pass

- [ ] **Step 4: Commit**

```
fix(vfs): collect paths before mutating nodes in remove()

The remove() method set parent=None on each node sequentially, breaking
the parent chain for later get_path() calls on descendant nodes. Now
collects all paths first while the tree is intact, then mutates.
```

---

### Task 4: Fix WAL validation production bug

**Files:**
- Modify: `conary-core/src/db/mod.rs:43-78`

`validate_wal_file()` at line 46 constructs the WAL path using
`path.with_extension(format!("{}-wal", ext))`. For extensionless files,
`ext` is empty, producing `path.-wal` instead of `path-wal`.

- [ ] **Step 1: Fix the path construction**

Replace lines 43-50:

```rust
fn validate_wal_file(path: &Path) -> Result<()> {
    // Construct WAL path: for "foo.db" -> "foo.db-wal", for "foo" -> "foo-wal"
    let wal_path = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}-wal")),
        None => {
            let mut p = path.as_os_str().to_os_string();
            p.push("-wal");
            PathBuf::from(p)
        }
    };
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p conary-core db::tests::test_open_rejects_corrupt_wal_sidecar -- --nocapture`
Expected: PASS (the test writes to `*.db-wal` which now matches)

- [ ] **Step 3: Run full DB tests**

Run: `cargo test -p conary-core db::tests -- --nocapture`
Expected: All pass

- [ ] **Step 4: Commit**

```
fix(db): handle extensionless files in WAL path construction

validate_wal_file() produced "path.-wal" for files without an extension.
Now correctly produces "path-wal" by appending directly to the OsString
when no extension is present.
```

---

## Chunk 2: UX Improvements

### Task 5: Un-hide capability enforce/audit commands

**Files:**
- Modify: `src/cli/capability.rs:63, 86, 108`

- [ ] **Step 1: Remove hide annotations**

In `src/cli/capability.rs`:
- Line 86: remove `#[command(hide = true)]` from `Audit`
- Line 108: remove `#[command(hide = true)]` from `Run`
- Keep `Generate` hidden (line 63) — it's an internal tool

- [ ] **Step 2: Add `Enforce` variant as alias for `Run`**

Add a new variant after `Run`:

```rust
    /// Enforce capability restrictions on a command (alias for 'run')
    Enforce {
        /// Package whose capabilities to enforce
        package: String,

        /// Command to run with enforcement
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,

        /// Run in permissive/audit mode (log but don't block)
        #[arg(long)]
        permissive: bool,

        #[command(flatten)]
        common: CommonArgs,
    },
```

Wire the `Enforce` match arm to call `cmd_capability_run()` with the same
args as `Run`.

- [ ] **Step 3: Verify commands appear in --help**

Run: `cargo run -- capability --help`
Expected: Shows `audit`, `run`, `enforce` (not `generate`)

- [ ] **Step 4: Commit**

```
feat(capability): un-hide audit/run commands, add enforce alias

Both capability audit and capability run were fully implemented but
hidden from --help. Un-hide them. Add 'enforce' as a visible alias
for 'run' matching the README documentation.
```

---

### Task 6: Centralized "Database not initialized" hint

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add error enhancement in main()**

Find where `main()` returns the Result. Before the implicit anyhow print,
intercept `DatabaseNotFound` errors:

```rust
fn main() {
    // ... existing setup ...

    if let Err(err) = run() {
        // Enhance specific errors with user-friendly hints
        let msg = format!("{err:#}");
        if msg.contains("Database not found") {
            eprintln!("Error: Database not initialized.");
            eprintln!("Run 'conary system init' to set up the package database.");
            std::process::exit(1);
        }
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}
```

NOTE: Check the actual structure of main(). If it uses `fn main() -> Result<()>`,
change to `fn main()` with explicit error handling. Or add a wrapper function.

- [ ] **Step 2: Test manually**

Run: `cargo run -- install foo --db-path /tmp/nonexistent.db`
Expected: "Error: Database not initialized. Run 'conary system init'..."

- [ ] **Step 3: Commit**

```
fix(cli): show helpful hint when database is not initialized

Instead of raw "Database not found at path: ...", now shows
"Database not initialized. Run 'conary system init' to set up
the package database."
```

---

### Task 7: Friendly duplicate repo add error

**Files:**
- Modify: `src/commands/repo.rs:53`

- [ ] **Step 1: Catch the UNIQUE constraint error**

Replace the bare `repo.insert(&conn)?;` at line 53 with:

```rust
    if let Err(e) = repo.insert(&conn) {
        let msg = e.to_string();
        if msg.contains("UNIQUE constraint failed") {
            anyhow::bail!(
                "Repository '{}' already exists.\nUse 'conary repo list' to see configured repositories.",
                name
            );
        }
        return Err(e.into());
    }
```

- [ ] **Step 2: Test manually**

Run: `cargo run -- system init --db-path /tmp/test-dup.db && cargo run -- repo add remi https://example.com --db-path /tmp/test-dup.db`
Expected: "Repository 'remi' already exists."

- [ ] **Step 3: Commit**

```
fix(cli): show friendly error when adding duplicate repository

Instead of raw "UNIQUE constraint failed: repositories.name",
now shows "Repository 'remi' already exists. Use 'conary repo list'
to see configured repositories."
```

---

## Chunk 3: Documentation + Final Verification

### Task 8: ROADMAP, README, version comments

**Files:**
- Modify: `ROADMAP.md`
- Modify: `README.md`
- Modify: `conary-server/Cargo.toml`
- Modify: `conary-test/Cargo.toml`

- [ ] **Step 1: Update ROADMAP.md**

Mark completed items:
- Bootstrap base system builds: `[x]` with note "31 packages, qcow2 image"
- Bootstrap image generation: `[x]`
- Update test count from 76/76 to 249 tests
- Add Phase 4 (Feature Validation) section as [COMPLETE]
- Add test infrastructure overhaul as [COMPLETE]

- [ ] **Step 2: Update README.md**

- Add project status badge/section showing 249 tests
- Mark system generations as "functional, limited production testing"
- Update capability section to show `enforce` and `audit` commands
- Mark bootstrap as "31 packages from source, qcow2 image generation"

- [ ] **Step 3: Add version track comments**

In `conary-server/Cargo.toml`, add after the version line:
```toml
# Separate version track (server-v* tags). See CLAUDE.md "Version Groups".
```

Same for `conary-test/Cargo.toml`:
```toml
# Separate version track (test-v* tags). See CLAUDE.md "Version Groups".
```

- [ ] **Step 4: Commit**

```
docs: update ROADMAP and README for pre-launch accuracy
```

---

### Task 9: Error message sweep + final verification

**Files:** Various command handlers

- [ ] **Step 1: Grep for raw SQLite error propagation**

Run: `grep -rn '\.insert\(&conn\)?' src/commands/ | grep -v '// '`
Check each for bare `?` without error mapping.

- [ ] **Step 2: Test common error paths**

Test each and verify the message is user-friendly:
```bash
cargo run -- repo sync --db-path /tmp/empty.db     # no repos configured
cargo run -- remove nonexistent --db-path /tmp/t.db  # not installed
cargo run -- ccs install /nonexistent.ccs --db-path /tmp/t.db  # missing file
```

Fix any that show raw errors.

- [ ] **Step 3: Run full test suite**

```bash
cargo test
cargo test --features server
cargo clippy -- -D warnings
cargo clippy --features server -- -D warnings
```

All must pass with 0 failures.

- [ ] **Step 4: Final commit**

```
chore: pre-launch readiness verification complete
```

---

## Success Criteria

- [ ] `cargo test` exits 0 with no failures
- [ ] `cargo test --features server` exits 0
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `conary capability enforce` and `conary capability audit` appear in --help
- [ ] `conary install foo` on uninitialized DB says "Run 'conary system init'"
- [ ] `conary repo add remi ...` on duplicate says "already exists"
- [ ] README makes no claims the code can't back up
- [ ] ROADMAP reflects actual project state
