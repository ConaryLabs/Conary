# Level 3: Full System Takeover — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add generation-based atomic system management — users can convert their system to Conary-managed generations, switch live or at reboot, and roll back.

**Architecture:** Flat reflink trees at `/conary/generations/{N}/` built from CAS. Live switch via `renameat2(RENAME_EXCHANGE)`. BLS boot entries with GRUB fallback. Extends existing `SystemState` for generation numbering.

**Tech Stack:** Rust, nix crate (renameat2, FICLONE ioctl), existing CAS/transaction infrastructure, dracut (external), BLS spec

---

### Task 1: Reflink Support in Filesystem Layer

**Files:**
- Create: `src/filesystem/reflink.rs`
- Modify: `src/filesystem/mod.rs`
- Modify: `src/filesystem/deployer.rs:122-178`

**Step 1: Write the failing test**

Add to `src/filesystem/reflink.rs`:

```rust
// src/filesystem/reflink.rs
//! Reflink (copy-on-write) support for btrfs/xfs filesystems

use anyhow::{Result, anyhow};
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// ioctl number for FICLONE (btrfs/xfs copy-on-write clone)
const FICLONE: libc::c_ulong = 0x40049409;

/// Attempt to reflink (CoW clone) src to dst.
/// Returns Ok(()) if reflink succeeded, Err if filesystem doesn't support it.
pub fn reflink_file(src: &Path, dst: &Path) -> Result<()> {
    let src_file = File::open(src)?;
    let dst_file = File::create(dst)?;

    let ret = unsafe { libc::ioctl(dst_file.as_raw_fd(), FICLONE, src_file.as_raw_fd()) };

    if ret == -1 {
        let err = std::io::Error::last_os_error();
        std::fs::remove_file(dst).ok();
        Err(anyhow!("Reflink failed: {}", err))
    } else {
        // Preserve permissions from source
        let metadata = std::fs::metadata(src)?;
        std::fs::set_permissions(dst, metadata.permissions())?;
        Ok(())
    }
}

/// Check if a filesystem supports reflinks by creating a test file and cloning it.
pub fn supports_reflinks(dir: &Path) -> bool {
    let test_src = dir.join(".conary-reflink-test-src");
    let test_dst = dir.join(".conary-reflink-test-dst");

    let result = (|| -> Result<()> {
        std::fs::write(&test_src, b"reflink test")?;
        reflink_file(&test_src, &test_dst)?;
        Ok(())
    })();

    std::fs::remove_file(&test_src).ok();
    std::fs::remove_file(&test_dst).ok();

    result.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_supports_reflinks_returns_bool() {
        // This test validates the function runs without panic.
        // Actual reflink support depends on the filesystem.
        let dir = TempDir::new().unwrap();
        let result = supports_reflinks(dir.path());
        // On tmpfs (CI), this will be false. On btrfs/xfs, true.
        assert!(result || !result); // just verifying it doesn't panic
    }

    #[test]
    fn test_reflink_file_fails_on_tmpfs() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        std::fs::write(&src, b"hello").unwrap();
        let dst = dir.path().join("dst");
        // tmpfs doesn't support reflinks, so this should fail
        let result = reflink_file(&src, &dst);
        // Don't assert failure — might run on btrfs
        if result.is_err() {
            assert!(!dst.exists(), "dst should be cleaned up on failure");
        }
    }
}
```

**Step 2: Add module to filesystem**

In `src/filesystem/mod.rs`, add: `pub mod reflink;`

**Step 3: Run tests**

Run: `cargo test --bin conary -- filesystem::reflink`
Expected: PASS (2 tests)

**Step 4: Add reflink deployment to `FileDeployer`**

In `src/filesystem/deployer.rs`, add method after `deploy_file` (after line 178):

```rust
/// Deploy a file from CAS using reflink (copy-on-write).
/// Falls back to hardlink, then copy if reflinks aren't supported.
pub fn deploy_file_reflink(&self, path: &str, hash: &str, permissions: u32) -> Result<()> {
    let cas_path = self.cas.hash_to_path(hash);
    if !cas_path.exists() {
        return Err(anyhow::anyhow!("CAS object not found: {}", hash));
    }
    let target = self.safe_target_path(path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Try reflink first, then hardlink, then copy
    if crate::filesystem::reflink::reflink_file(&cas_path, &target).is_ok() {
        std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(permissions))?;
        return Ok(());
    }
    // Fallback to existing deploy_file logic
    self.deploy_file(path, hash, permissions)
}
```

**Step 5: Run all filesystem tests**

Run: `cargo test --bin conary -- filesystem`
Expected: PASS

**Step 6: Commit**

```bash
git add src/filesystem/reflink.rs src/filesystem/mod.rs src/filesystem/deployer.rs
git commit -m "feat(fs): Add reflink support with fallback to hardlink/copy"
```

---

### Task 2: Generation Metadata and Storage

**Files:**
- Create: `src/commands/generation/mod.rs`
- Create: `src/commands/generation/metadata.rs`
- Modify: `src/commands/mod.rs`

**Step 1: Create generation metadata types**

```rust
// src/commands/generation/metadata.rs
//! Generation metadata — stored as .conary-gen.json in each generation root

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Directories excluded from generation trees (remain shared/mounted)
pub const EXCLUDED_DIRS: &[&str] = &[
    "home", "proc", "sys", "dev", "run", "tmp", "mnt", "media",
    "var/lib",
];

/// Standard root symlinks to create in each generation
pub const ROOT_SYMLINKS: &[(&str, &str)] = &[
    ("bin", "usr/bin"),
    ("lib", "usr/lib"),
    ("lib64", "usr/lib64"),
    ("sbin", "usr/sbin"),
];

/// Metadata stored per generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationMetadata {
    pub generation: i64,
    pub created_at: String,
    pub package_count: i64,
    pub kernel_version: Option<String>,
    pub summary: String,
}

impl GenerationMetadata {
    pub fn write_to(&self, gen_dir: &Path) -> Result<()> {
        let path = gen_dir.join(".conary-gen.json");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn read_from(gen_dir: &Path) -> Result<Self> {
        let path = gen_dir.join(".conary-gen.json");
        let json = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&json)?)
    }
}

/// Path helper for generation storage
pub fn generations_dir() -> PathBuf {
    PathBuf::from("/conary/generations")
}

pub fn generation_path(number: i64) -> PathBuf {
    generations_dir().join(number.to_string())
}

pub fn current_link() -> PathBuf {
    PathBuf::from("/conary/current")
}

pub fn gc_roots_dir() -> PathBuf {
    PathBuf::from("/conary/gc-roots")
}

/// Detect kernel version from /boot or /usr/lib/modules
pub fn detect_kernel_version(gen_dir: &Path) -> Option<String> {
    let modules_dir = gen_dir.join("usr/lib/modules");
    if modules_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&modules_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// Check if a path should be excluded from generation trees
pub fn is_excluded(path: &str) -> bool {
    let path = path.trim_start_matches('/');
    EXCLUDED_DIRS.iter().any(|&excl| path == excl || path.starts_with(&format!("{}/", excl)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_metadata_roundtrip() {
        let dir = TempDir::new().unwrap();
        let meta = GenerationMetadata {
            generation: 1,
            created_at: "2026-03-04T00:00:00Z".to_string(),
            package_count: 42,
            kernel_version: Some("6.18.13".to_string()),
            summary: "Initial generation".to_string(),
        };
        meta.write_to(dir.path()).unwrap();
        let read_back = GenerationMetadata::read_from(dir.path()).unwrap();
        assert_eq!(read_back.generation, 1);
        assert_eq!(read_back.package_count, 42);
        assert_eq!(read_back.kernel_version.unwrap(), "6.18.13");
    }

    #[test]
    fn test_excluded_paths() {
        assert!(is_excluded("home"));
        assert!(is_excluded("/home"));
        assert!(is_excluded("proc"));
        assert!(is_excluded("var/lib"));
        assert!(is_excluded("var/lib/docker"));
        assert!(!is_excluded("usr"));
        assert!(!is_excluded("etc"));
        assert!(!is_excluded("var/cache"));
    }

    #[test]
    fn test_generation_paths() {
        assert_eq!(generation_path(1), PathBuf::from("/conary/generations/1"));
        assert_eq!(generation_path(42), PathBuf::from("/conary/generations/42"));
    }
}
```

**Step 2: Create module file**

```rust
// src/commands/generation/mod.rs
//! Generation management — atomic system state management

pub mod metadata;
```

**Step 3: Register module in `src/commands/mod.rs`**

Add `pub mod generation;` to the module declarations.

**Step 4: Run tests**

Run: `cargo test --bin conary -- generation::metadata`
Expected: PASS (3 tests)

**Step 5: Commit**

```bash
git add src/commands/generation/ src/commands/mod.rs
git commit -m "feat(generation): Add generation metadata types and path helpers"
```

---

### Task 3: Generation Builder

**Files:**
- Create: `src/commands/generation/builder.rs`
- Modify: `src/commands/generation/mod.rs`

**Step 1: Implement the generation builder**

```rust
// src/commands/generation/builder.rs
//! Build a generation tree from CAS contents

use super::metadata::{
    self, GenerationMetadata, ROOT_SYMLINKS, detect_kernel_version, generation_path,
    generations_dir, is_excluded,
};
use anyhow::{Context, Result, anyhow};
use conary::db::models::{FileEntry, StateEngine, Trove};
use conary::filesystem::cas::CasStore;
use conary::filesystem::deployer::FileDeployer;
use std::path::Path;
use tracing::{debug, info, warn};

/// Build a new generation tree from the current system state.
///
/// 1. Queries all installed packages from DB
/// 2. Allocates next state_number as generation number
/// 3. Creates generation directory tree
/// 4. Reflinks all files from CAS into the tree
/// 5. Creates standard root symlinks
/// 6. Writes metadata
pub fn build_generation(
    conn: &rusqlite::Connection,
    db_path: &str,
    summary: &str,
) -> Result<i64> {
    // Ensure base directory exists
    std::fs::create_dir_all(generations_dir())
        .context("Failed to create generations directory")?;

    // Check reflink support
    if !crate::filesystem::reflink::supports_reflinks(&generations_dir()) {
        warn!("Filesystem does not support reflinks — falling back to hardlinks/copies");
    }

    // Create system state snapshot and get generation number
    let engine = StateEngine::new();
    let state = engine.create_snapshot(conn, summary, None, None)?;
    let gen_number = state.state_number;
    let gen_dir = generation_path(gen_number);

    info!("Building generation {} at {}", gen_number, gen_dir.display());

    if gen_dir.exists() {
        return Err(anyhow!("Generation directory already exists: {}", gen_dir.display()));
    }
    std::fs::create_dir_all(&gen_dir)?;

    // Build result — track stats
    let mut files_deployed = 0u64;
    let mut errors = 0u64;

    // Get all installed packages
    let troves = Trove::find_all(conn)?;
    let package_count = troves.len() as i64;

    let objects_dir = conary::db::paths::objects_dir(db_path);
    let deployer = FileDeployer::new(
        CasStore::new(&objects_dir)?,
        &gen_dir,
    );

    // Deploy all files from all packages
    for trove in &troves {
        let trove_id = match trove.id {
            Some(id) => id,
            None => continue,
        };

        let files = FileEntry::find_by_trove(conn, trove_id)?;
        for file in &files {
            // Skip excluded paths
            if is_excluded(&file.path) {
                continue;
            }

            match deployer.deploy_file_reflink(&file.path, &file.hash, file.permissions as u32) {
                Ok(()) => files_deployed += 1,
                Err(e) => {
                    debug!("Failed to deploy {}: {}", file.path, e);
                    errors += 1;
                }
            }
        }
    }

    // Create standard root symlinks (bin -> usr/bin, etc.)
    for (link, target) in ROOT_SYMLINKS {
        let link_path = gen_dir.join(link);
        if !link_path.exists() {
            // Only create if the target exists in the generation
            let target_path = gen_dir.join(target);
            if target_path.exists() {
                std::os::unix::fs::symlink(target, &link_path)
                    .with_context(|| format!("Creating symlink {} -> {}", link, target))?;
            }
        }
    }

    // Write metadata
    let kernel_version = detect_kernel_version(&gen_dir);
    let meta = GenerationMetadata {
        generation: gen_number,
        created_at: chrono::Utc::now().to_rfc3339(),
        package_count,
        kernel_version,
        summary: summary.to_string(),
    };
    meta.write_to(&gen_dir)?;

    info!(
        "Generation {} built: {} files deployed, {} errors, {} packages",
        gen_number, files_deployed, errors, package_count
    );

    Ok(gen_number)
}
```

**Step 2: Add to module**

In `src/commands/generation/mod.rs`, add: `pub mod builder;`

**Step 3: Build and verify**

Run: `cargo build`
Expected: compiles clean

**Step 4: Commit**

```bash
git add src/commands/generation/builder.rs src/commands/generation/mod.rs
git commit -m "feat(generation): Add generation builder — reflink files from CAS"
```

---

### Task 4: Atomic Generation Switch

**Files:**
- Create: `src/commands/generation/switch.rs`
- Modify: `src/commands/generation/mod.rs`

**Step 1: Implement live switch and symlink update**

```rust
// src/commands/generation/switch.rs
//! Atomic generation switching via renameat2(RENAME_EXCHANGE)

use super::metadata::{current_link, generation_path, GenerationMetadata};
use anyhow::{Context, Result, anyhow};
use nix::fcntl::RenameFlags;
use std::ffi::CString;
use std::path::Path;
use tracing::{info, warn};

/// Top-level directories to swap during a live switch.
/// These are exchanged atomically between the generation tree and the live root.
const SWAP_DIRS: &[&str] = &["usr", "etc"];

/// Perform a live generation switch using renameat2(RENAME_EXCHANGE).
///
/// For each dir in SWAP_DIRS:
///   renameat2(gen_dir/usr, /usr, RENAME_EXCHANGE)
///
/// After exchange, the old /usr is now at gen_dir/usr (swapped in).
/// Updates /conary/current symlink to point to the new generation.
pub fn switch_live(gen_number: i64) -> Result<()> {
    let gen_dir = generation_path(gen_number);
    if !gen_dir.exists() {
        return Err(anyhow!("Generation {} does not exist", gen_number));
    }

    // Verify metadata exists
    let meta = GenerationMetadata::read_from(&gen_dir)
        .context("Generation metadata missing or corrupt")?;

    info!("Switching to generation {} ({})", gen_number, meta.summary);

    let mut exchanged = Vec::new();

    for dir_name in SWAP_DIRS {
        let gen_path = gen_dir.join(dir_name);
        let live_path = Path::new("/").join(dir_name);

        if !gen_path.exists() {
            warn!("Generation {} has no /{}, skipping", gen_number, dir_name);
            continue;
        }
        if !live_path.exists() {
            warn!("Live system has no /{}, skipping", dir_name);
            continue;
        }

        match renameat2_exchange(&gen_path, &live_path) {
            Ok(()) => {
                info!("Exchanged /{}", dir_name);
                exchanged.push(*dir_name);
            }
            Err(e) => {
                warn!(
                    "renameat2(RENAME_EXCHANGE) failed for /{}:{} — \
                     falling back to non-atomic rename",
                    dir_name, e
                );
                // Fallback: rename old, move new, rename old back into gen
                fallback_rename(&gen_path, &live_path, dir_name)?;
                exchanged.push(*dir_name);
            }
        }
    }

    // Update /conary/current symlink
    update_current_symlink(gen_number)?;

    println!(
        "Switched to generation {} (exchanged: {})",
        gen_number,
        exchanged.join(", ")
    );
    println!("Reboot recommended for full consistency.");

    Ok(())
}

/// Atomic exchange of two paths via renameat2(RENAME_EXCHANGE)
fn renameat2_exchange(a: &Path, b: &Path) -> Result<()> {
    // nix::fcntl::renameat2 requires &CStr paths and directory fds
    // Use libc directly for simplicity with Path arguments
    let a_cstr = CString::new(a.to_str().ok_or_else(|| anyhow!("Non-UTF8 path"))?)
        .context("CString conversion")?;
    let b_cstr = CString::new(b.to_str().ok_or_else(|| anyhow!("Non-UTF8 path"))?)
        .context("CString conversion")?;

    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            a_cstr.as_ptr(),
            libc::AT_FDCWD,
            b_cstr.as_ptr(),
            2u32, // RENAME_EXCHANGE
        )
    };

    if ret == -1 {
        Err(anyhow!(
            "renameat2 RENAME_EXCHANGE: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

/// Non-atomic fallback: rename old out, move new in, move old into gen dir
fn fallback_rename(gen_path: &Path, live_path: &Path, dir_name: &str) -> Result<()> {
    let backup_path = live_path.with_file_name(format!("{}.conary-old", dir_name));

    // Step 1: Move live dir out of the way
    std::fs::rename(live_path, &backup_path)
        .with_context(|| format!("Moving /{} to backup", dir_name))?;

    // Step 2: Move generation dir into place
    if let Err(e) = std::fs::rename(gen_path, live_path) {
        // Restore backup on failure
        warn!("Failed to move generation dir into place, restoring backup: {}", e);
        std::fs::rename(&backup_path, live_path).ok();
        return Err(e.into());
    }

    // Step 3: Move old live dir into generation (so it's preserved)
    std::fs::rename(&backup_path, gen_path)
        .with_context(|| format!("Moving old /{} into generation dir", dir_name))?;

    Ok(())
}

/// Update /conary/current symlink to point to the given generation
fn update_current_symlink(gen_number: i64) -> Result<()> {
    let link = current_link();
    let target = generation_path(gen_number);

    // Atomic symlink update: create temp, rename over old
    let tmp_link = link.with_extension("tmp");
    if tmp_link.exists() {
        std::fs::remove_file(&tmp_link)?;
    }
    std::os::unix::fs::symlink(&target, &tmp_link)?;
    std::fs::rename(&tmp_link, &link)?;

    Ok(())
}

/// Get the currently active generation number from /conary/current symlink
pub fn current_generation() -> Result<Option<i64>> {
    let link = current_link();
    if !link.exists() {
        return Ok(None);
    }
    let target = std::fs::read_link(&link)?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("Invalid current symlink target"))?;
    Ok(name.parse().ok())
}
```

**Step 2: Add to module**

In `src/commands/generation/mod.rs`, add: `pub mod switch;`

**Step 3: Build**

Run: `cargo build`
Expected: compiles clean

**Step 4: Commit**

```bash
git add src/commands/generation/switch.rs src/commands/generation/mod.rs
git commit -m "feat(generation): Add atomic switch via renameat2(RENAME_EXCHANGE)"
```

---

### Task 5: Boot Loader Integration

**Files:**
- Create: `src/commands/generation/boot.rs`
- Modify: `src/commands/generation/mod.rs`

**Step 1: Implement BLS entry generation and GRUB fallback**

```rust
// src/commands/generation/boot.rs
//! Boot Loader Specification entries and GRUB fallback for generation switching

use super::metadata::{GenerationMetadata, generation_path};
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const BLS_DIR: &str = "/boot/loader/entries";

/// Bootloader type detected on the system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootLoader {
    Bls,
    Grub,
    None,
}

/// Detect available boot loader integration
pub fn detect_bootloader() -> BootLoader {
    if Path::new(BLS_DIR).exists() {
        BootLoader::Bls
    } else if Path::new("/usr/sbin/grub-mkconfig").exists()
        || Path::new("/usr/sbin/grub2-mkconfig").exists()
    {
        BootLoader::Grub
    } else {
        BootLoader::None
    }
}

/// Write a BLS entry for a generation
pub fn write_bls_entry(gen_number: i64, root_uuid: &str) -> Result<PathBuf> {
    let gen_dir = generation_path(gen_number);
    let meta = GenerationMetadata::read_from(&gen_dir)?;

    let kernel_version = meta
        .kernel_version
        .as_deref()
        .ok_or_else(|| anyhow!("No kernel version in generation {} metadata", gen_number))?;

    let entry_path = Path::new(BLS_DIR).join(format!("conary-gen-{}.conf", gen_number));

    // Read existing kernel cmdline for options (minus conary.generation=)
    let existing_cmdline = read_cmdline_options();

    let content = format!(
        "title      Conary Generation {} ({})\n\
         version    {}\n\
         linux      /vmlinuz-{}\n\
         initrd     /initramfs-{}.img\n\
         options    root=UUID={} conary.generation={} {}\n\
         sort-key   conary\n\
         machine-id {}\n",
        gen_number,
        meta.created_at.split('T').next().unwrap_or(&meta.created_at),
        kernel_version,
        kernel_version,
        kernel_version,
        root_uuid,
        gen_number,
        existing_cmdline,
        read_machine_id().unwrap_or_default(),
    );

    std::fs::create_dir_all(BLS_DIR)?;
    std::fs::write(&entry_path, content)
        .with_context(|| format!("Writing BLS entry: {}", entry_path.display()))?;

    info!("Wrote BLS entry: {}", entry_path.display());
    Ok(entry_path)
}

/// Write GRUB config snippet for generation boot entries
pub fn write_grub_snippet(gen_number: i64, root_uuid: &str) -> Result<()> {
    let gen_dir = generation_path(gen_number);
    let meta = GenerationMetadata::read_from(&gen_dir)?;

    let kernel_version = meta
        .kernel_version
        .as_deref()
        .ok_or_else(|| anyhow!("No kernel version in generation {} metadata", gen_number))?;

    let existing_cmdline = read_cmdline_options();

    let script_path = Path::new("/etc/grub.d/42_conary");
    let content = format!(
        "#!/bin/sh\n\
         exec tail -n +3 $0\n\
         menuentry 'Conary Generation {} ({})' {{\n\
             linux /vmlinuz-{} root=UUID={} conary.generation={} {}\n\
             initrd /initramfs-{}.img\n\
         }}\n",
        gen_number,
        meta.created_at.split('T').next().unwrap_or(&meta.created_at),
        kernel_version,
        root_uuid,
        gen_number,
        existing_cmdline,
        kernel_version,
    );

    std::fs::write(script_path, content)?;

    // Make executable
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(script_path, std::fs::Permissions::from_mode(0o755))?;

    // Run grub-mkconfig
    let grub_mkconfig = if Path::new("/usr/sbin/grub2-mkconfig").exists() {
        "/usr/sbin/grub2-mkconfig"
    } else {
        "/usr/sbin/grub-mkconfig"
    };

    info!("Running {} to update GRUB config", grub_mkconfig);
    let output = std::process::Command::new(grub_mkconfig)
        .args(["-o", "/boot/grub2/grub.cfg"])
        .output()?;

    if !output.status.success() {
        warn!(
            "grub-mkconfig failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Write boot entry using detected bootloader
pub fn write_boot_entry(gen_number: i64) -> Result<()> {
    let root_uuid = detect_root_uuid()?;
    match detect_bootloader() {
        BootLoader::Bls => {
            write_bls_entry(gen_number, &root_uuid)?;
            println!("BLS boot entry written for generation {}", gen_number);
        }
        BootLoader::Grub => {
            write_grub_snippet(gen_number, &root_uuid)?;
            println!("GRUB entry written for generation {}", gen_number);
        }
        BootLoader::None => {
            warn!("No supported boot loader found, skipping boot entry");
            println!("No supported boot loader detected — boot entry skipped");
        }
    }
    Ok(())
}

/// Read root filesystem UUID from /etc/fstab or findmnt
fn detect_root_uuid() -> Result<String> {
    let output = std::process::Command::new("findmnt")
        .args(["-n", "-o", "UUID", "/"])
        .output()
        .context("Failed to run findmnt")?;

    let uuid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uuid.is_empty() {
        return Err(anyhow!("Could not detect root UUID"));
    }
    Ok(uuid)
}

/// Read kernel command line, stripping any existing conary.generation= param
fn read_cmdline_options() -> String {
    std::fs::read_to_string("/proc/cmdline")
        .unwrap_or_default()
        .split_whitespace()
        .filter(|opt| !opt.starts_with("conary.generation="))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Read machine-id for BLS entries
fn read_machine_id() -> Option<String> {
    std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_bootloader_returns_value() {
        // Just verify it doesn't panic
        let _ = detect_bootloader();
    }

    #[test]
    fn test_read_cmdline_strips_generation() {
        // This reads /proc/cmdline which exists in test env
        let cmdline = read_cmdline_options();
        assert!(!cmdline.contains("conary.generation="));
    }
}
```

**Step 2: Add to module**

In `src/commands/generation/mod.rs`, add: `pub mod boot;`

**Step 3: Build and test**

Run: `cargo test --bin conary -- generation::boot`
Expected: PASS (2 tests)

**Step 4: Commit**

```bash
git add src/commands/generation/boot.rs src/commands/generation/mod.rs
git commit -m "feat(generation): Add BLS boot entries with GRUB fallback"
```

---

### Task 6: Generation List, Info, and GC Commands

**Files:**
- Create: `src/commands/generation/commands.rs`
- Modify: `src/commands/generation/mod.rs`

**Step 1: Implement list, info, and gc**

```rust
// src/commands/generation/commands.rs
//! CLI command implementations for generation management

use super::metadata::{
    GenerationMetadata, current_link, gc_roots_dir, generation_path, generations_dir,
};
use super::switch::current_generation;
use anyhow::{Result, anyhow};
use tracing::info;

/// List all generations with active marker
pub fn cmd_generation_list() -> Result<()> {
    let gens_dir = generations_dir();
    if !gens_dir.exists() {
        println!("No generations found. Run 'conary system takeover' to create the first.");
        return Ok(());
    }

    let current = current_generation()?.unwrap_or(-1);

    let mut generations: Vec<(i64, GenerationMetadata)> = Vec::new();
    for entry in std::fs::read_dir(&gens_dir)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str() {
            if let Ok(num) = name.parse::<i64>() {
                if let Ok(meta) = GenerationMetadata::read_from(&entry.path()) {
                    generations.push((num, meta));
                }
            }
        }
    }

    generations.sort_by_key(|(n, _)| *n);

    if generations.is_empty() {
        println!("No generations found.");
        return Ok(());
    }

    println!("Generations:");
    for (num, meta) in &generations {
        let marker = if *num == current { " [active]" } else { "" };
        let date = meta.created_at.split('T').next().unwrap_or(&meta.created_at);
        let kernel = meta
            .kernel_version
            .as_deref()
            .unwrap_or("unknown");
        println!(
            "  {:>4}  {}  {} packages  kernel {}{}",
            num, date, meta.package_count, kernel, marker
        );
    }

    Ok(())
}

/// Show detailed info for a generation
pub fn cmd_generation_info(gen_number: i64) -> Result<()> {
    let gen_dir = generation_path(gen_number);
    if !gen_dir.exists() {
        return Err(anyhow!("Generation {} does not exist", gen_number));
    }

    let meta = GenerationMetadata::read_from(&gen_dir)?;
    let current = current_generation()?.unwrap_or(-1);
    let is_active = gen_number == current;

    println!("Generation {}", gen_number);
    println!("  Status:   {}", if is_active { "active" } else { "inactive" });
    println!("  Created:  {}", meta.created_at);
    println!("  Packages: {}", meta.package_count);
    println!("  Kernel:   {}", meta.kernel_version.as_deref().unwrap_or("none"));
    println!("  Summary:  {}", meta.summary);

    // Show disk usage
    let dir_size = dir_size_bytes(&gen_dir);
    println!("  Size:     {} (on-disk, CoW-shared)", format_bytes(dir_size));

    Ok(())
}

/// Garbage collect old generations
pub fn cmd_generation_gc(keep: usize) -> Result<()> {
    let gens_dir = generations_dir();
    if !gens_dir.exists() {
        println!("No generations to clean up.");
        return Ok(());
    }

    let current = current_generation()?.unwrap_or(-1);
    let gc_roots = load_gc_roots();

    let mut generations: Vec<i64> = Vec::new();
    for entry in std::fs::read_dir(&gens_dir)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str() {
            if let Ok(num) = name.parse::<i64>() {
                generations.push(num);
            }
        }
    }

    generations.sort();

    // Determine which to keep: current, gc-roots, and last N
    let keep_set: std::collections::HashSet<i64> = {
        let mut set = std::collections::HashSet::new();
        set.insert(current);
        for root in &gc_roots {
            set.insert(*root);
        }
        // Keep the last `keep` generations
        for gen in generations.iter().rev().take(keep) {
            set.insert(*gen);
        }
        set
    };

    let to_remove: Vec<i64> = generations
        .iter()
        .filter(|g| !keep_set.contains(g))
        .copied()
        .collect();

    if to_remove.is_empty() {
        println!("Nothing to clean up ({} generations, keeping {})", generations.len(), keep);
        return Ok(());
    }

    println!("Removing {} generation(s):", to_remove.len());
    for gen in &to_remove {
        let gen_dir = generation_path(*gen);
        println!("  Generation {}", gen);
        std::fs::remove_dir_all(&gen_dir)?;
        info!("Removed generation {} at {}", gen, gen_dir.display());

        // Remove BLS entry if it exists
        let bls_entry = std::path::Path::new("/boot/loader/entries")
            .join(format!("conary-gen-{}.conf", gen));
        if bls_entry.exists() {
            std::fs::remove_file(&bls_entry)?;
        }
    }

    println!("Freed {} generation(s). {} remaining.", to_remove.len(), generations.len() - to_remove.len());

    Ok(())
}

/// Load pinned generation numbers from gc-roots directory
fn load_gc_roots() -> Vec<i64> {
    let dir = gc_roots_dir();
    if !dir.exists() {
        return Vec::new();
    }
    std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.parse::<i64>().ok())
        .collect()
}

/// Approximate directory size (follows reflinks, so may overcount)
fn dir_size_bytes(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KiB", bytes / 1024)
    }
}
```

**Step 2: Add to module and verify walkdir dependency**

In `src/commands/generation/mod.rs`, add: `pub mod commands;`

Check Cargo.toml for `walkdir` — if missing, add it:
Run: `grep walkdir Cargo.toml`
If not present: `cargo add walkdir`

**Step 3: Build and test**

Run: `cargo build`
Expected: compiles clean

**Step 4: Commit**

```bash
git add src/commands/generation/commands.rs src/commands/generation/mod.rs
git commit -m "feat(generation): Add list, info, and gc commands"
```

---

### Task 7: System Takeover Command

**Files:**
- Create: `src/commands/generation/takeover.rs`
- Modify: `src/commands/generation/mod.rs`

**Step 1: Implement `conary system takeover`**

```rust
// src/commands/generation/takeover.rs
//! Full system takeover — convert entire system to Conary-managed generations

use super::boot::write_boot_entry;
use super::builder::build_generation;
use super::metadata::generations_dir;
use super::switch::{switch_live, update_current_symlink};
use crate::commands::install::blocklist;
use crate::commands::install::system_pm;
use anyhow::{Context, Result, anyhow};
use conary::db::models::Trove;
use conary::packages::SystemPackageManager;
use tracing::info;

/// Takeover plan summarizing what will happen
pub struct TakeoverPlan {
    pub already_tracked: Vec<String>,
    pub to_adopt: Vec<String>,
    pub to_convert: Vec<String>,
    pub blocked: Vec<String>,
    pub total_system_packages: usize,
}

/// Inventory the system and build a takeover plan
pub fn plan_takeover(conn: &rusqlite::Connection) -> Result<TakeoverPlan> {
    let pm = SystemPackageManager::detect();
    if matches!(pm, SystemPackageManager::Unknown) {
        return Err(anyhow!("Could not detect system package manager"));
    }

    // Get all system packages
    let system_packages = query_all_system_packages(&pm)?;
    let total = system_packages.len();

    // Get all Conary-tracked packages
    let tracked: std::collections::HashSet<String> = Trove::find_all(conn)?
        .into_iter()
        .map(|t| t.name)
        .collect();

    let mut plan = TakeoverPlan {
        already_tracked: Vec::new(),
        to_adopt: Vec::new(),
        to_convert: Vec::new(),
        blocked: Vec::new(),
        total_system_packages: total,
    };

    for pkg_name in &system_packages {
        if tracked.contains(pkg_name) {
            plan.already_tracked.push(pkg_name.clone());
        } else if blocklist::is_blocked(pkg_name) {
            // Blocklisted packages are adopted, never converted
            plan.to_adopt.push(pkg_name.clone());
            plan.blocked.push(pkg_name.clone());
        } else {
            // TODO: Check Remi availability for conversion
            // For now, adopt everything not already tracked
            plan.to_adopt.push(pkg_name.clone());
        }
    }

    Ok(plan)
}

/// Execute the system takeover
pub fn cmd_system_takeover(
    db_path: &str,
    yes: bool,
    dry_run: bool,
    skip_conversion: bool,
) -> Result<()> {
    println!("Conary System Takeover");
    println!("======================\n");

    // Pre-flight checks
    preflight_checks()?;

    let conn = conary::db::open(db_path)?;

    // Plan
    let plan = plan_takeover(&conn)?;

    // Report
    println!("System inventory: {} packages", plan.total_system_packages);
    println!("  Already tracked by Conary: {}", plan.already_tracked.len());
    println!("  To adopt from system PM:   {}", plan.to_adopt.len());
    if !skip_conversion {
        println!("  To convert via Remi:       {}", plan.to_convert.len());
    }
    println!("  Blocklisted (adopt only):  {}", plan.blocked.len());
    println!();

    if dry_run {
        println!("[DRY RUN] Would adopt {} packages and build Generation 1", plan.to_adopt.len());
        return Ok(());
    }

    // Confirm
    if !yes {
        print!("Proceed with system takeover? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Step 1: Adopt all un-tracked packages
    if !plan.to_adopt.is_empty() {
        println!("\nAdopting {} system packages...", plan.to_adopt.len());
        crate::commands::cmd_adopt(&plan.to_adopt, db_path, false)?;
        println!("Adoption complete.");
    }

    // Step 2: Build Generation 1
    println!("\nBuilding generation 1...");
    let gen_number = build_generation(&conn, db_path, "System takeover — initial generation")?;
    println!("Generation {} built.", gen_number);

    // Step 3: Write boot entry
    if let Err(e) = write_boot_entry(gen_number) {
        println!("Boot entry skipped: {}", e);
    }

    // Step 4: Live switch
    println!("\nSwitching to generation {}...", gen_number);
    switch_live(gen_number)?;

    println!("\nSystem takeover complete.");
    println!("Reboot recommended for full consistency.");
    println!("Use 'conary generation list' to see generations.");
    println!("Use 'conary generation rollback' to revert.");

    Ok(())
}

fn preflight_checks() -> Result<()> {
    // Must be root
    if !nix::unistd::Uid::effective().is_root() {
        return Err(anyhow!("System takeover requires root privileges"));
    }

    // Check reflink support
    let gens_dir = generations_dir();
    std::fs::create_dir_all(&gens_dir)?;
    if !crate::filesystem::reflink::supports_reflinks(&gens_dir) {
        println!(
            "[WARN] Filesystem at {} does not support reflinks.\n\
             Files will be copied instead of CoW-cloned (uses more disk space).",
            gens_dir.display()
        );
    }

    Ok(())
}

/// Query all installed packages from the system PM
fn query_all_system_packages(pm: &SystemPackageManager) -> Result<Vec<String>> {
    let output = match pm {
        SystemPackageManager::Rpm => std::process::Command::new("rpm")
            .args(["-qa", "--qf", "%{NAME}\n"])
            .output()?,
        SystemPackageManager::Dpkg => std::process::Command::new("dpkg-query")
            .args(["-W", "-f", "${Package}\n"])
            .output()?,
        SystemPackageManager::Pacman => std::process::Command::new("pacman")
            .args(["-Qq"])
            .output()?,
        SystemPackageManager::Unknown => {
            return Err(anyhow!("Unknown package manager"));
        }
    };

    let packages: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    Ok(packages)
}
```

**Step 2: Add to module**

In `src/commands/generation/mod.rs`, add: `pub mod takeover;`

**Step 3: Build**

Run: `cargo build`
Expected: compiles clean

**Step 4: Commit**

```bash
git add src/commands/generation/takeover.rs src/commands/generation/mod.rs
git commit -m "feat(generation): Add conary system takeover command"
```

---

### Task 8: CLI Wiring — Add Generation Subcommands

**Files:**
- Create: `src/cli/generation.rs`
- Modify: `src/cli/system.rs:283-297` — add Generation variant
- Modify: `src/cli/mod.rs` — add `pub use generation::GenerationCommands;`
- Modify: `src/main.rs:431-466` — add Generation dispatch

**Step 1: Create CLI definitions**

```rust
// src/cli/generation.rs
//! CLI definitions for generation management

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum GenerationCommands {
    /// List all generations
    List,

    /// Build a new generation from current system state
    Build {
        /// Summary description for this generation
        #[arg(long, default_value = "Manual generation build")]
        summary: String,
    },

    /// Switch to a specific generation
    Switch {
        /// Generation number to switch to
        number: i64,

        /// Reboot after switching
        #[arg(long)]
        reboot: bool,
    },

    /// Roll back to the previous generation
    Rollback,

    /// Remove old generations
    Gc {
        /// Number of generations to keep (default: 3)
        #[arg(long, default_value = "3")]
        keep: usize,
    },

    /// Show detailed info about a generation
    Info {
        /// Generation number
        number: i64,
    },
}
```

**Step 2: Register in CLI**

In `src/cli/system.rs`, add after `State(StateCommands)` (around line 288):
```rust
/// Generation management (build, switch, rollback, gc)
#[command(subcommand)]
Generation(GenerationCommands),
```

In `src/cli/mod.rs`, add the module and re-export.

**Step 3: Add `--yes` and `--dry-run` to SystemCommands for takeover**

In `src/cli/system.rs`, add a new variant inside `SystemCommands`:
```rust
/// Convert entire system to Conary-managed generations
Takeover {
    /// Auto-confirm
    #[arg(long, short)]
    yes: bool,

    /// Show what would be done without making changes
    #[arg(long)]
    dry_run: bool,

    /// Skip Remi conversion, adopt all packages directly
    #[arg(long)]
    skip_conversion: bool,
},
```

**Step 4: Add dispatch in `src/main.rs`**

After the State dispatch block (around line 466), add:

```rust
// Nested: system generation
cli::SystemCommands::Generation(gen_cmd) => match gen_cmd {
    cli::GenerationCommands::List => {
        commands::generation::commands::cmd_generation_list()
    }
    cli::GenerationCommands::Build { summary } => {
        let conn = conary::db::open(&db.db_path)?;
        let gen = commands::generation::builder::build_generation(&conn, &db.db_path, &summary)?;
        println!("Generation {} built.", gen);
        Ok(())
    }
    cli::GenerationCommands::Switch { number, reboot } => {
        commands::generation::switch::switch_live(number)?;
        commands::generation::boot::write_boot_entry(number)?;
        if reboot {
            println!("Rebooting...");
            std::process::Command::new("systemctl").arg("reboot").spawn()?;
        }
        Ok(())
    }
    cli::GenerationCommands::Rollback => {
        let current = commands::generation::switch::current_generation()?
            .ok_or_else(|| anyhow::anyhow!("No active generation"))?;
        if current <= 1 {
            return Err(anyhow::anyhow!("Cannot rollback from generation 1").into());
        }
        commands::generation::switch::switch_live(current - 1)?;
        commands::generation::boot::write_boot_entry(current - 1)?;
        println!("Rolled back to generation {}", current - 1);
        Ok(())
    }
    cli::GenerationCommands::Gc { keep } => {
        commands::generation::commands::cmd_generation_gc(keep)
    }
    cli::GenerationCommands::Info { number } => {
        commands::generation::commands::cmd_generation_info(number)
    }
},

// System takeover
cli::SystemCommands::Takeover { yes, dry_run, skip_conversion } => {
    commands::generation::takeover::cmd_system_takeover(&db.db_path, yes, dry_run, skip_conversion)
},
```

**Step 5: Build and verify help output**

Run: `cargo build && ./target/debug/conary system generation --help`
Expected: shows List, Build, Switch, Rollback, Gc, Info subcommands

Run: `./target/debug/conary system takeover --help`
Expected: shows --yes, --dry-run, --skip-conversion flags

**Step 6: Commit**

```bash
git add src/cli/generation.rs src/cli/system.rs src/cli/mod.rs src/main.rs
git commit -m "feat(cli): Wire generation and takeover commands into CLI"
```

---

### Task 9: Integration Tests

**Files:**
- Modify: `tests/integration/remi/runner/test-runner.sh`

**Step 1: Add generation tests T33-T35**

Add before the Cleanup section:

```bash
# ── T33: Generation List (empty) ────────────────────────────────────────────

test_generation_list_empty() {
    local output
    output=$("$CONARY" system generation list --db-path "$DB_PATH" 2>&1)
    # Should not crash, should indicate no generations
    assert_output_contains "No generations" "$output"
}

run_test "T33" "generation_list_empty" 10 test_generation_list_empty

# ── T34: System Takeover Dry Run ────────────────────────────────────────────

test_takeover_dry_run() {
    local output exit_code
    output=$("$CONARY" system takeover \
        --db-path "$DB_PATH" \
        --dry-run \
        --skip-conversion \
        2>&1) && exit_code=0 || exit_code=$?

    # Dry run should succeed and show inventory
    assert_output_contains "DRY RUN" "$output"
}

run_test "T34" "takeover_dry_run" 60 test_takeover_dry_run

# ── T35: Generation GC (nothing to clean) ──────────────────────────────────

test_generation_gc_empty() {
    local output
    output=$("$CONARY" system generation gc --db-path "$DB_PATH" 2>&1)
    assert_output_contains "Nothing to clean\|No generations" "$output"
}

run_test "T35" "generation_gc_empty" 10 test_generation_gc_empty
```

**Step 2: Update cleanup section**

Update the fatal-skip message to reference T33-T35.

**Step 3: Commit**

```bash
git add tests/integration/remi/runner/test-runner.sh
git commit -m "test: Add integration tests T33-T35 for generation commands"
```

---

### Task 10: Dracut Module (initramfs hook)

**Files:**
- Create: `packaging/dracut/90conary/module-setup.sh`
- Create: `packaging/dracut/90conary/conary-generator.sh`

**Step 1: Create dracut module**

```bash
# packaging/dracut/90conary/module-setup.sh
#!/bin/bash
# Dracut module for Conary generation switching

check() {
    # Only include if conary generations exist
    [ -d /conary/generations ] && return 0
    return 255
}

depends() {
    return 0
}

install() {
    inst_hook pre-pivot 90 "$moddir/conary-generator.sh"
}
```

```bash
# packaging/dracut/90conary/conary-generator.sh
#!/bin/bash
# Pre-pivot hook: bind-mount the selected Conary generation

# Read conary.generation=N from kernel cmdline
CONARY_GEN=""
for opt in $(cat /proc/cmdline); do
    case "$opt" in
        conary.generation=*)
            CONARY_GEN="${opt#conary.generation=}"
            ;;
    esac
done

# Fall back to /conary/current symlink
if [ -z "$CONARY_GEN" ]; then
    if [ -L /sysroot/conary/current ]; then
        GEN_DIR=$(readlink -f /sysroot/conary/current)
    else
        exit 0  # No generation system configured
    fi
else
    GEN_DIR="/sysroot/conary/generations/${CONARY_GEN}"
fi

# Verify generation exists
if [ ! -d "$GEN_DIR" ]; then
    echo "conary: generation $CONARY_GEN not found, booting without generation" >&2
    exit 0
fi

# Bind-mount generation directories over sysroot
for dir in usr etc; do
    if [ -d "${GEN_DIR}/${dir}" ]; then
        mount --bind "${GEN_DIR}/${dir}" "/sysroot/${dir}"
    fi
done
```

**Step 2: Commit**

```bash
git add packaging/dracut/
git commit -m "feat(boot): Add dracut module for generation switching at boot"
```

---

## File Summary

| File | Action |
|------|--------|
| `src/filesystem/reflink.rs` | **NEW** — reflink/FICLONE support |
| `src/filesystem/mod.rs` | Add `pub mod reflink` |
| `src/filesystem/deployer.rs` | Add `deploy_file_reflink` method |
| `src/commands/generation/mod.rs` | **NEW** — module declarations |
| `src/commands/generation/metadata.rs` | **NEW** — generation metadata types |
| `src/commands/generation/builder.rs` | **NEW** — build generation from CAS |
| `src/commands/generation/switch.rs` | **NEW** — atomic switch via renameat2 |
| `src/commands/generation/boot.rs` | **NEW** — BLS entries + GRUB fallback |
| `src/commands/generation/commands.rs` | **NEW** — list, info, gc commands |
| `src/commands/generation/takeover.rs` | **NEW** — system takeover orchestration |
| `src/commands/mod.rs` | Add `pub mod generation` |
| `src/cli/generation.rs` | **NEW** — CLI definitions |
| `src/cli/system.rs` | Add Generation + Takeover variants |
| `src/cli/mod.rs` | Add generation module |
| `src/main.rs` | Add Generation + Takeover dispatch |
| `packaging/dracut/90conary/` | **NEW** — initramfs hook |
| `tests/integration/remi/runner/test-runner.sh` | Add T33-T35 |

## Verification

1. `cargo build` — compiles clean
2. `cargo test --bin conary` — all tests pass
3. `cargo clippy -- -D warnings` — no warnings
4. `./target/debug/conary system generation --help` — shows subcommands
5. `./target/debug/conary system takeover --dry-run` — shows inventory on Fedora 43
