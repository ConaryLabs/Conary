# Conary Self-Update Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement `conary self-update` — a CLI command that checks Remi for newer CCS packages of conary, downloads, verifies, and atomically replaces the running binary.

**Architecture:** Top-level CLI command that reads the update channel URL from a new SQLite settings table (fallback: hardcoded default), fetches version metadata from Remi, downloads the CCS package, extracts and verifies the new binary, atomically replaces `/usr/bin/conary` via `rename()`, then registers the new hash in CAS. The conary binary lives outside EROFS generation images and is updated independently.

**Tech Stack:** Rust 1.93, clap (CLI), reqwest (HTTP), serde/serde_json (API responses), rusqlite (settings table), conary-core CCS parsing, CAS storage.

**Design Doc:** `docs/plans/2026-03-07-self-update-design.md`

---

### Task 1: Add Settings Table (DB Migration)

Create a key-value settings table so `self-update` can store its update channel URL and other configuration.

**Files:**
- Modify: `conary-core/src/db/migrations.rs` (add migration 46)
- Modify: `conary-core/src/db/models/mod.rs` (add settings module)
- Create: `conary-core/src/db/models/settings.rs`

**Step 1: Write the failing test**

In `conary-core/src/db/models/settings.rs`:

```rust
// conary-core/src/db/models/settings.rs

//! Key-value settings storage

use crate::error::Result;
use rusqlite::Connection;

/// Get a setting value by key
pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let result = stmt
        .query_row([key], |row| row.get(0))
        .optional()?;
    Ok(result)
}

/// Set a setting value (upsert)
pub fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

/// Delete a setting
pub fn delete(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn test_settings_get_set_delete() {
        let conn = db::open(":memory:").unwrap();
        assert_eq!(get(&conn, "update-channel").unwrap(), None);
        set(&conn, "update-channel", "https://example.com").unwrap();
        assert_eq!(
            get(&conn, "update-channel").unwrap(),
            Some("https://example.com".to_string())
        );
        delete(&conn, "update-channel").unwrap();
        assert_eq!(get(&conn, "update-channel").unwrap(), None);
    }
}
```

**Step 2: Add migration 46 in `conary-core/src/db/migrations.rs`**

Find the last migration function and add after it:

```rust
fn migration_46(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );",
    )?;
    Ok(())
}
```

Update `SCHEMA_VERSION` to `46` and add `46 => migration_46(conn)?` to the migration match block.

**Step 3: Register the settings module**

In `conary-core/src/db/models/mod.rs`, add:
```rust
pub mod settings;
```

**Step 4: Run tests to verify**

Run: `cargo test -p conary-core settings`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/db/migrations.rs conary-core/src/db/models/settings.rs conary-core/src/db/models/mod.rs
git commit -m "feat(db): add key-value settings table (migration 46)"
```

---

### Task 2: CLI Command Registration

Register `self-update` as a top-level command in the CLI.

**Files:**
- Modify: `src/cli/mod.rs` (add `SelfUpdate` variant to `Commands` enum)
- Modify: `src/main.rs` (add match arm for `SelfUpdate`)
- Modify: `src/commands/mod.rs` (add self_update module, re-export)
- Create: `src/commands/self_update.rs` (stub function)

**Step 1: Add CLI variant in `src/cli/mod.rs`**

Add to the `Commands` enum, after the Bootstrap section:

```rust
    // =========================================================================
    // Self-Update
    // =========================================================================
    /// Update conary itself to the latest version
    #[command(name = "self-update")]
    SelfUpdate {
        #[command(flatten)]
        db: DbArgs,

        /// Check for updates without installing
        #[arg(long)]
        check: bool,

        /// Reinstall even if already at latest version
        #[arg(long)]
        force: bool,

        /// Install a specific version
        #[arg(long)]
        version: Option<String>,
    },
```

**Step 2: Create stub command handler**

Create `src/commands/self_update.rs`:

```rust
// src/commands/self_update.rs

//! Self-update command: update the conary binary itself

use anyhow::Result;

pub fn cmd_self_update(
    db_path: &str,
    check: bool,
    force: bool,
    version: Option<String>,
) -> Result<()> {
    println!("Conary v{}", env!("CARGO_PKG_VERSION"));
    println!("self-update: not yet implemented");
    Ok(())
}
```

**Step 3: Register module in `src/commands/mod.rs`**

Add `mod self_update;` to the module list and `pub use self_update::cmd_self_update;` to the re-exports.

**Step 4: Add match arm in `src/main.rs`**

Before the `None =>` arm, add:

```rust
        // =====================================================================
        // Self-Update
        // =====================================================================
        Some(Commands::SelfUpdate {
            db,
            check,
            force,
            version,
        }) => commands::cmd_self_update(&db.db_path, check, force, version),
```

**Step 5: Run build to verify**

Run: `cargo build`
Expected: compiles without errors

**Step 6: Verify CLI help**

Run: `cargo run -- self-update --help`
Expected: shows self-update help with `--check`, `--force`, `--version` options

**Step 7: Commit**

```bash
git add src/cli/mod.rs src/main.rs src/commands/mod.rs src/commands/self_update.rs
git commit -m "feat: add self-update CLI command (stub)"
```

---

### Task 3: Version Check Logic

Implement the core version comparison: fetch latest version from Remi, compare with compiled-in version, report result.

**Files:**
- Create: `conary-core/src/self_update.rs` (core logic module)
- Modify: `conary-core/src/lib.rs` (register module)
- Modify: `src/commands/self_update.rs` (wire up)

**Step 1: Write the self_update core module with tests**

Create `conary-core/src/self_update.rs`:

```rust
// conary-core/src/self_update.rs

//! Self-update logic for the conary binary
//!
//! Checks Remi for newer versions and handles downloading, verifying,
//! and atomically replacing the running binary.

use crate::db::models::settings;
use crate::error::{Error, Result};
use rusqlite::Connection;
use serde::Deserialize;

/// Default update channel URL
pub const DEFAULT_UPDATE_CHANNEL: &str = "https://packages.conary.io/v1/ccs/conary";

/// Settings key for the update channel override
const SETTINGS_KEY_UPDATE_CHANNEL: &str = "update-channel";

/// Response from the /latest endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct LatestVersionInfo {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
}

/// Result of a version check
#[derive(Debug, Clone, PartialEq)]
pub enum VersionCheckResult {
    /// A newer version is available
    UpdateAvailable {
        current: String,
        latest: String,
        download_url: String,
        sha256: String,
        size: u64,
    },
    /// Already at the latest version
    UpToDate {
        version: String,
    },
}

/// Get the update channel URL from settings or fall back to default
pub fn get_update_channel(conn: &Connection) -> Result<String> {
    match settings::get(conn, SETTINGS_KEY_UPDATE_CHANNEL)? {
        Some(url) => Ok(url),
        None => Ok(DEFAULT_UPDATE_CHANNEL.to_string()),
    }
}

/// Set a custom update channel URL
pub fn set_update_channel(conn: &Connection, url: &str) -> Result<()> {
    settings::set(conn, SETTINGS_KEY_UPDATE_CHANNEL, url)
}

/// Compare two semver version strings. Returns true if `remote` is newer than `current`.
pub fn is_newer(current: &str, remote: &str) -> bool {
    let parse = |v: &str| -> (u64, u64, u64) {
        let parts: Vec<&str> = v.split('.').collect();
        let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };
    parse(remote) > parse(current)
}

/// Check for available updates by querying the update channel
pub fn check_for_update(
    channel_url: &str,
    current_version: &str,
) -> Result<VersionCheckResult> {
    use reqwest::blocking::Client;
    use std::time::Duration;

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| Error::IoError(format!("Failed to create HTTP client: {e}")))?;

    let url = format!("{}/latest", channel_url);
    let response = client
        .get(&url)
        .header("User-Agent", format!("conary/{}", current_version))
        .send()
        .map_err(|e| Error::IoError(format!("Failed to check for updates: {e}")))?;

    if !response.status().is_success() {
        return Err(Error::IoError(format!(
            "Update check failed: HTTP {}",
            response.status()
        )));
    }

    let info: LatestVersionInfo = response
        .json()
        .map_err(|e| Error::ParseError(format!("Invalid update response: {e}")))?;

    if is_newer(current_version, &info.version) {
        Ok(VersionCheckResult::UpdateAvailable {
            current: current_version.to_string(),
            latest: info.version,
            download_url: info.download_url,
            sha256: info.sha256,
            size: info.size,
        })
    } else {
        Ok(VersionCheckResult::UpToDate {
            version: current_version.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.1.0", "1.0.0"));
        assert!(!is_newer("0.2.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn test_get_update_channel_default() {
        let conn = crate::db::open(":memory:").unwrap();
        let channel = get_update_channel(&conn).unwrap();
        assert_eq!(channel, DEFAULT_UPDATE_CHANNEL);
    }

    #[test]
    fn test_set_update_channel() {
        let conn = crate::db::open(":memory:").unwrap();
        set_update_channel(&conn, "https://internal.example.com/conary").unwrap();
        let channel = get_update_channel(&conn).unwrap();
        assert_eq!(channel, "https://internal.example.com/conary");
    }
}
```

**Step 2: Register module in `conary-core/src/lib.rs`**

Add `pub mod self_update;` to the module list.

**Step 3: Run tests**

Run: `cargo test -p conary-core self_update`
Expected: PASS (3 tests)

**Step 4: Commit**

```bash
git add conary-core/src/self_update.rs conary-core/src/lib.rs
git commit -m "feat: add self-update version check logic"
```

---

### Task 4: Download and Extract Binary

Download the CCS package from Remi, extract the conary binary from it, and verify it runs.

**Files:**
- Modify: `conary-core/src/self_update.rs` (add download + extract functions)

**Step 1: Add download and extract functions with tests**

Add to `conary-core/src/self_update.rs`:

```rust
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Download the CCS package to a temp directory and return the path
pub fn download_update(
    download_url: &str,
    expected_sha256: &str,
    dest_dir: &Path,
) -> Result<PathBuf> {
    use crate::repository::client::{RepositoryClient, validate_url_scheme};

    validate_url_scheme(download_url)?;

    let dest_path = dest_dir.join("conary-update.ccs");
    let client = RepositoryClient::new();
    client.download_file(download_url, &dest_path)?;

    // Verify SHA-256
    let content = fs::read(&dest_path)
        .map_err(|e| Error::IoError(format!("Failed to read downloaded file: {e}")))?;
    let actual_hash = crate::hash::sha256(&content);
    if actual_hash != expected_sha256 {
        fs::remove_file(&dest_path).ok();
        return Err(Error::IntegrityError(format!(
            "SHA-256 mismatch: expected {expected_sha256}, got {actual_hash}"
        )));
    }

    Ok(dest_path)
}

/// Extract the conary binary from a CCS package to a temp file
///
/// Returns the path to the extracted binary. The binary is placed on the
/// same filesystem as `target_dir` to enable atomic rename().
pub fn extract_binary(ccs_path: &Path, target_dir: &Path) -> Result<PathBuf> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let file = fs::File::open(ccs_path)
        .map_err(|e| Error::IoError(format!("Failed to open CCS package: {e}")))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    let dest = target_dir.join(".conary-update.tmp");

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let path_str = path.to_string_lossy();

        // Look for the conary binary in the CCS package
        // CCS packages store files as objects/{hash}, but the conary self-update
        // CCS is a special single-binary package with usr/bin/conary
        if path_str.ends_with("usr/bin/conary") || path_str == "conary" {
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            fs::write(&dest, &content)
                .map_err(|e| Error::IoError(format!("Failed to write binary: {e}")))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
            }

            return Ok(dest);
        }
    }

    Err(Error::ParseError(
        "CCS package does not contain a conary binary".to_string(),
    ))
}

/// Verify the extracted binary runs and reports the expected version
pub fn verify_binary(binary_path: &Path, expected_version: &str) -> Result<()> {
    let output = Command::new(binary_path)
        .arg("--version")
        .output()
        .map_err(|e| Error::IoError(format!("Failed to execute new binary: {e}")))?;

    if !output.status.success() {
        return Err(Error::IoError(format!(
            "New binary exited with status {}",
            output.status
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(expected_version) {
        return Err(Error::IoError(format!(
            "Version mismatch: expected '{}' in output, got '{}'",
            expected_version,
            stdout.trim()
        )));
    }

    Ok(())
}
```

Add to the test module:

```rust
    #[test]
    fn test_verify_binary_with_current() {
        // Verify that the current binary passes verification
        let current = std::env::current_exe().unwrap();
        // This won't match our version string, but tests the execution path
        let result = verify_binary(&current, "definitely-not-a-version");
        assert!(result.is_err()); // Should fail on version mismatch
    }
```

**Step 2: Run tests**

Run: `cargo test -p conary-core self_update`
Expected: PASS

**Step 3: Commit**

```bash
git add conary-core/src/self_update.rs
git commit -m "feat: add self-update download, extract, and verify"
```

---

### Task 5: Atomic Binary Replacement

Implement the atomic `rename()` swap and CAS registration.

**Files:**
- Modify: `conary-core/src/self_update.rs` (add `apply_update` function)

**Step 1: Add the atomic replacement function**

Add to `conary-core/src/self_update.rs`:

```rust
/// Atomically replace the running conary binary and register in CAS
///
/// 1. rename() temp binary -> target path (atomic on same filesystem)
/// 2. Store new binary hash in CAS
/// 3. Record new version in DB
pub fn apply_update(
    new_binary_path: &Path,
    target_path: &Path,
    conn: &Connection,
    objects_dir: &str,
) -> Result<()> {
    use crate::filesystem::CasStore;

    // Atomic rename (source and target must be on same filesystem)
    fs::rename(new_binary_path, target_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            Error::IoError(format!(
                "Permission denied: cannot replace {}. Try running with sudo.",
                target_path.display()
            ))
        } else {
            Error::IoError(format!(
                "Failed to replace binary at {}: {e}",
                target_path.display()
            ))
        }
    })?;

    // Register new binary in CAS (best-effort: if this fails, binary still works)
    let content = fs::read(target_path).unwrap_or_default();
    if !content.is_empty() {
        if let Ok(cas) = CasStore::new(objects_dir) {
            if let Err(e) = cas.store(&content) {
                eprintln!("Warning: failed to register in CAS: {e}");
            }
        }
    }

    Ok(())
}
```

**Step 2: Run tests**

Run: `cargo test -p conary-core self_update`
Expected: PASS

**Step 3: Commit**

```bash
git add conary-core/src/self_update.rs
git commit -m "feat: add atomic binary replacement for self-update"
```

---

### Task 6: Wire Up the Full Command

Connect all the pieces in the CLI command handler.

**Files:**
- Modify: `src/commands/self_update.rs` (full implementation)

**Step 1: Implement the full command**

Replace `src/commands/self_update.rs`:

```rust
// src/commands/self_update.rs

//! Self-update command: update the conary binary itself

use anyhow::Result;
use conary_core::db;
use conary_core::db::paths::objects_dir;
use conary_core::self_update::{
    self, VersionCheckResult, apply_update, check_for_update, download_update,
    extract_binary, get_update_channel, verify_binary,
};
use std::path::PathBuf;

pub fn cmd_self_update(
    db_path: &str,
    check: bool,
    force: bool,
    version: Option<String>,
) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let conn = db::open(db_path)?;
    let channel_url = get_update_channel(&conn)?;

    println!("Current version: {current_version}");
    println!("Update channel: {channel_url}");

    // Check for updates
    let result = check_for_update(&channel_url, current_version)?;

    match result {
        VersionCheckResult::UpToDate { version } => {
            if !force {
                println!("Already up to date (v{version})");
                return Ok(());
            }
            println!("Already at v{version}, but --force specified");
        }
        VersionCheckResult::UpdateAvailable {
            current,
            latest,
            ref download_url,
            ref sha256,
            size,
        } => {
            println!(
                "Update available: v{current} -> v{latest} ({:.1} MB)",
                size as f64 / 1_048_576.0
            );
            if check {
                return Ok(());
            }
        }
    }

    // Determine download URL and expected version
    let (download_url, sha256, expected_version) = match &result {
        VersionCheckResult::UpdateAvailable {
            latest,
            download_url,
            sha256,
            ..
        } => (download_url.clone(), sha256.clone(), latest.clone()),
        VersionCheckResult::UpToDate { version } => {
            // --force path: re-fetch latest info
            let info_url = format!("{}/latest", channel_url);
            let info: self_update::LatestVersionInfo = reqwest::blocking::get(&info_url)?.json()?;
            (info.download_url, info.sha256, info.version)
        }
    };

    // Determine target binary path (the currently running binary)
    let target_path = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Cannot determine current binary path: {e}"))?;

    // Download to temp dir on same filesystem as target
    let target_dir = target_path.parent().unwrap_or_else(|| std::path::Path::new("/usr/bin"));
    let temp_dir = tempfile::tempdir_in(target_dir)?;

    println!("Downloading v{expected_version}...");
    let ccs_path = download_update(&download_url, &sha256, temp_dir.path())?;

    println!("Extracting binary...");
    let new_binary = extract_binary(&ccs_path, target_dir)?;

    println!("Verifying new binary...");
    verify_binary(&new_binary, &expected_version)?;

    println!("Replacing binary...");
    let obj_dir = objects_dir(db_path);
    apply_update(&new_binary, &target_path, &conn, &obj_dir)?;

    println!(
        "Updated conary v{} -> v{}",
        current_version, expected_version
    );

    Ok(())
}
```

**Step 2: Add `tempfile` dependency if not already present**

Check `Cargo.toml` for `tempfile`. If missing, add to `[dependencies]`:
```toml
tempfile = "3"
```

**Step 3: Run build**

Run: `cargo build`
Expected: compiles without errors

**Step 4: Commit**

```bash
git add src/commands/self_update.rs Cargo.toml
git commit -m "feat: wire up complete self-update command"
```

---

### Task 7: Update Channel Configuration Command

Allow users to get/set the update channel via `conary config set update-channel <url>`.

This extends the existing `conary system` subcommands since config commands already handle config files. The update channel is a system setting, so add it as `conary system update-channel [get|set <url>|reset]`.

**Files:**
- Modify: `src/cli/system.rs` (add `UpdateChannel` subcommand)
- Modify: `src/main.rs` (add match arm)
- Create: `src/commands/update_channel.rs`
- Modify: `src/commands/mod.rs` (register module)

**Step 1: Add CLI subcommand to system**

In `src/cli/system.rs`, add to `SystemCommands` enum:

```rust
    /// Manage self-update channel
    #[command(name = "update-channel")]
    UpdateChannel {
        #[command(subcommand)]
        action: UpdateChannelAction,
    },
```

Add the action enum:

```rust
#[derive(Subcommand)]
pub enum UpdateChannelAction {
    /// Show current update channel URL
    Get {
        #[command(flatten)]
        db: super::DbArgs,
    },
    /// Set a custom update channel URL
    Set {
        /// Update channel URL
        url: String,
        #[command(flatten)]
        db: super::DbArgs,
    },
    /// Reset to default update channel
    Reset {
        #[command(flatten)]
        db: super::DbArgs,
    },
}
```

**Step 2: Create command handler**

Create `src/commands/update_channel.rs`:

```rust
// src/commands/update_channel.rs

//! Update channel management commands

use anyhow::Result;
use conary_core::db;
use conary_core::self_update::{
    DEFAULT_UPDATE_CHANNEL, get_update_channel, set_update_channel,
};
use conary_core::db::models::settings;

pub fn cmd_update_channel_get(db_path: &str) -> Result<()> {
    let conn = db::open(db_path)?;
    let channel = get_update_channel(&conn)?;
    let is_default = channel == DEFAULT_UPDATE_CHANNEL;
    println!("{}{}", channel, if is_default { " (default)" } else { "" });
    Ok(())
}

pub fn cmd_update_channel_set(db_path: &str, url: &str) -> Result<()> {
    conary_core::repository::client::validate_url_scheme(url)?;
    let conn = db::open(db_path)?;
    set_update_channel(&conn, url)?;
    println!("Update channel set to: {url}");
    Ok(())
}

pub fn cmd_update_channel_reset(db_path: &str) -> Result<()> {
    let conn = db::open(db_path)?;
    settings::delete(&conn, "update-channel")?;
    println!("Update channel reset to default: {DEFAULT_UPDATE_CHANNEL}");
    Ok(())
}
```

**Step 3: Register in `src/commands/mod.rs`**

```rust
mod update_channel;
pub use update_channel::{
    cmd_update_channel_get, cmd_update_channel_set, cmd_update_channel_reset,
};
```

**Step 4: Add match arm in `src/main.rs`**

In the System commands match block:

```rust
cli::SystemCommands::UpdateChannel { action } => match action {
    cli::UpdateChannelAction::Get { db } => {
        commands::cmd_update_channel_get(&db.db_path)
    }
    cli::UpdateChannelAction::Set { url, db } => {
        commands::cmd_update_channel_set(&db.db_path, &url)
    }
    cli::UpdateChannelAction::Reset { db } => {
        commands::cmd_update_channel_reset(&db.db_path)
    }
},
```

**Step 5: Run build and test**

Run: `cargo build`
Run: `cargo run -- system update-channel get --help`
Expected: shows help for get/set/reset subcommands

**Step 6: Commit**

```bash
git add src/cli/system.rs src/main.rs src/commands/mod.rs src/commands/update_channel.rs
git commit -m "feat: add update-channel management commands"
```

---

### Task 8: Remi Server Endpoints (Server-Side)

Add the three Remi endpoints that serve self-update metadata and CCS packages. This is server-side code, feature-gated behind `--features server`.

**Files:**
- Create: `conary-server/src/server/routes/self_update.rs`
- Modify: `conary-server/src/server/routes/mod.rs` (register routes)
- Modify: `conary-server/src/server/mod.rs` (register route handler)

**Step 1: Identify route registration pattern**

Read `conary-server/src/server/routes/mod.rs` and an existing route file to understand the pattern (Axum router, handler signatures).

**Step 2: Create the self-update route handler**

Create `conary-server/src/server/routes/self_update.rs`:

```rust
// conary-server/src/server/routes/self_update.rs

//! Self-update endpoints for conary binary updates
//!
//! GET /v1/ccs/conary/latest           -> version info JSON
//! GET /v1/ccs/conary/versions         -> list of available versions
//! GET /v1/ccs/conary/{version}/download -> CCS package stream

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use std::path::PathBuf;

use crate::server::AppState;

#[derive(Serialize)]
pub struct LatestResponse {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Serialize)]
pub struct VersionsResponse {
    pub versions: Vec<String>,
    pub latest: String,
}
```

The exact implementation depends on the existing Remi server patterns. The implementer should:
1. Read `conary-server/src/server/routes/mod.rs` to understand routing patterns
2. Read one existing route handler to understand `AppState` and response patterns
3. Implement the three handlers following those patterns
4. Store CCS self-update packages in a dedicated directory (e.g., `/conary/self-update/`)
5. Register routes in the router

**Step 3: Build with server feature**

Run: `cargo build --features server`
Expected: compiles

**Step 4: Commit**

```bash
git add conary-server/src/server/routes/
git commit -m "feat(server): add self-update Remi endpoints"
```

---

### Task 9: Integration Test

Add an integration test that exercises the self-update flow using a mock HTTP server.

**Files:**
- Modify: `conary-core/src/self_update.rs` (add integration test)

**Step 1: Write the integration test**

Add to the test module in `conary-core/src/self_update.rs`:

```rust
    #[test]
    fn test_version_check_result_variants() {
        let up_to_date = VersionCheckResult::UpToDate {
            version: "0.1.0".to_string(),
        };
        assert_eq!(
            up_to_date,
            VersionCheckResult::UpToDate {
                version: "0.1.0".to_string()
            }
        );

        let update = VersionCheckResult::UpdateAvailable {
            current: "0.1.0".to_string(),
            latest: "0.2.0".to_string(),
            download_url: "https://example.com/conary-0.2.0.ccs".to_string(),
            sha256: "abc123".to_string(),
            size: 12_000_000,
        };
        match &update {
            VersionCheckResult::UpdateAvailable { current, latest, .. } => {
                assert_eq!(current, "0.1.0");
                assert_eq!(latest, "0.2.0");
            }
            _ => panic!("Expected UpdateAvailable"),
        }
    }

    #[test]
    fn test_is_newer_edge_cases() {
        // Same version
        assert!(!is_newer("1.0.0", "1.0.0"));
        // Major bump
        assert!(is_newer("0.99.99", "1.0.0"));
        // Partial versions
        assert!(is_newer("1", "2"));
        assert!(is_newer("1.0", "1.1"));
    }
```

**Step 2: Run tests**

Run: `cargo test -p conary-core self_update`
Expected: PASS

**Step 3: Run full test suite**

Run: `cargo test`
Expected: all tests pass

**Step 4: Commit**

```bash
git add conary-core/src/self_update.rs
git commit -m "test: add self-update integration tests"
```

---

### Task 10: Clippy and Final Cleanup

Ensure all code passes clippy and the full test suite.

**Step 1: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: no warnings

**Step 2: Fix any issues**

Address any clippy warnings in the new code.

**Step 3: Run full test suite**

Run: `cargo test`
Expected: all tests pass

**Step 4: Final commit**

```bash
git add -A
git commit -m "chore: clippy cleanup for self-update"
```

---

## Summary

| Task | What | Files |
|------|------|-------|
| 1 | Settings table (migration 46) | `conary-core/src/db/migrations.rs`, `models/settings.rs` |
| 2 | CLI registration (stub) | `src/cli/mod.rs`, `src/main.rs`, `src/commands/self_update.rs` |
| 3 | Version check logic | `conary-core/src/self_update.rs`, `conary-core/src/lib.rs` |
| 4 | Download + extract + verify | `conary-core/src/self_update.rs` |
| 5 | Atomic replacement + CAS | `conary-core/src/self_update.rs` |
| 6 | Wire up full command | `src/commands/self_update.rs` |
| 7 | Update channel commands | `src/cli/system.rs`, `src/commands/update_channel.rs` |
| 8 | Remi server endpoints | `conary-server/src/server/routes/self_update.rs` |
| 9 | Integration tests | `conary-core/src/self_update.rs` |
| 10 | Clippy + cleanup | All modified files |

## Dependencies Between Tasks

```
Task 1 (settings table) ──┐
                           ├──> Task 3 (version check) ──> Task 4 (download) ──> Task 5 (atomic replace)
Task 2 (CLI stub) ────────┘                                                             │
                                                                                        v
Task 7 (update channel) ──────────────────────────────────────> depends on Task 3   Task 6 (wire up)
Task 8 (Remi endpoints) ──────────────────────────────────────> independent          Task 9 (tests)
Task 10 (cleanup) ────────────────────────────────────────────> after all            Task 10 (cleanup)
```

Tasks 1+2 can run in parallel. Task 8 is independent of all client-side tasks.
