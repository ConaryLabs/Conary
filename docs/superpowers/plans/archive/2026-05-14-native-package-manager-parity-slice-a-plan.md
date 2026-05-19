# Native Package Manager Parity Slice A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Conary-owned install and remove work on mutable live hosts without a selected Conary generation, while preserving the generation-aware path and recording deferred follow-up truthfully.

**Architecture:** Add a shared changeset metadata envelope and a small mutable live-root transaction layer beside the existing composefs-native transaction engine. Install and remove choose between two explicit paths: generation-aware DB/CAS-to-generation publication when a Conary generation is selected, and live-root filesystem mutation plus DB commit when no generation is selected. Slice A intentionally does not implement update parity, the full daily-driver command matrix, or the conary-test distro matrix; it creates the foundation those slices depend on.

**Tech Stack:** Rust, rusqlite, serde/serde_json, Conary command modules, conary-core transaction/CAS helpers, tempfile-based integration tests, Markdown docs.

---

## Suggested `/goal`

Use this objective when launching implementation:

```text
/goal Slice A: Implement the no-generation live-host package operation foundation for Conary-owned install and remove. Add the live-root transaction engine or equivalent safe path that writes/removes package files on mutable live hosts without a selected Conary generation; preserve the generation-aware path for active-generation hosts; introduce the shared PackageOperationOutcome contract and deferred-follow-up history metadata; prove with unit and CLI tests.
```

The goal is complete only after the final verification block in this plan passes, or any skipped command is recorded in the final response with a concrete reason.

## Source Spec

- `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`

## Files

- Create: `apps/conary/src/commands/changeset_metadata.rs`
- Create: `apps/conary/src/commands/live_root.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Modify: `apps/conary/src/commands/query/history.rs`
- Modify: `apps/conary/src/commands/system.rs`
- Modify: `apps/conary/src/commands/install/inner.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Create: `apps/conary/tests/native_pm_live_root.rs`
- Modify: `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`

## Execution Boundaries

- In scope: Conary-owned `install` and `remove` on no-generation live roots.
- In scope: active-generation install/remove regression protection.
- In scope: `AppliedWithDeferredFollowUp` as an operation outcome represented by `ChangesetStatus::Applied` plus metadata.
- In scope: `system history` rendering of deferred follow-up.
- In scope: critical package refusal in remove/purge/autoremove paths.
- In scope: adopted `remove` refusing before mutation unless explicit destructive purge is supplied.
- Out of scope: `conary update` parity, `update --security`, `pin` selector cleanup, `query whatprovides`, `query whatbreaks`, full Fedora/Ubuntu/Arch conary-test matrix.

## Task 1: Add Changeset Metadata Envelope

**Files:**
- Create: `apps/conary/src/commands/changeset_metadata.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Modify: `apps/conary/src/commands/system.rs`
- Test: `apps/conary/src/commands/changeset_metadata.rs`

- [ ] **Step 1: Add the failing metadata tests**

Create `apps/conary/src/commands/changeset_metadata.rs` with the tests first. Include the path comment at the top.

```rust
// apps/conary/src/commands/changeset_metadata.rs

use super::{FileSnapshot, RevertMetadata, TroveSnapshot};
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub(crate) const CHANGESET_METADATA_SCHEMA: &str = "conary.changeset.metadata.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeferredFollowUp {
    pub kind: String,
    pub status: String,
    pub message: String,
    pub retry_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChangesetMetadataEnvelope {
    pub schema: String,
    #[serde(default)]
    pub removed_troves: Vec<TroveSnapshot>,
    #[serde(default)]
    pub deferred_follow_up: Vec<DeferredFollowUp>,
}

pub(crate) fn metadata_with_removed_troves(snapshots: Vec<TroveSnapshot>) -> Result<String> {
    serde_json::to_string(&ChangesetMetadataEnvelope {
        schema: CHANGESET_METADATA_SCHEMA.to_string(),
        removed_troves: snapshots,
        deferred_follow_up: Vec::new(),
    })
    .map_err(Into::into)
}

pub(crate) fn metadata_with_deferred_follow_up(
    snapshots: Vec<TroveSnapshot>,
    deferred_follow_up: Vec<DeferredFollowUp>,
) -> Result<String> {
    serde_json::to_string(&ChangesetMetadataEnvelope {
        schema: CHANGESET_METADATA_SCHEMA.to_string(),
        removed_troves: snapshots,
        deferred_follow_up,
    })
    .map_err(Into::into)
}

pub(crate) fn parse_rollback_snapshots(snapshot_json: &str) -> Result<Vec<TroveSnapshot>> {
    if let Ok(envelope) = serde_json::from_str::<ChangesetMetadataEnvelope>(snapshot_json)
        && envelope.schema == CHANGESET_METADATA_SCHEMA
    {
        return Ok(envelope.removed_troves);
    }
    if let Ok(wrapper) = serde_json::from_str::<RevertMetadata>(snapshot_json) {
        return Ok(wrapper.removed_troves);
    }
    Ok(vec![serde_json::from_str(snapshot_json)?])
}

pub(crate) fn deferred_follow_up(snapshot_json: Option<&str>) -> Vec<DeferredFollowUp> {
    snapshot_json
        .and_then(|raw| serde_json::from_str::<ChangesetMetadataEnvelope>(raw).ok())
        .filter(|envelope| envelope.schema == CHANGESET_METADATA_SCHEMA)
        .map(|envelope| envelope.deferred_follow_up)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(name: &str) -> TroveSnapshot {
        TroveSnapshot {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            architecture: Some("x86_64".to_string()),
            description: None,
            install_source: "repository".to_string(),
            installed_from_repository_id: None,
            files: vec![FileSnapshot {
                path: "/usr/bin/fixture".to_string(),
                sha256_hash: "0".repeat(64),
                size: 7,
                permissions: 0o100755,
                symlink_target: None,
            }],
        }
    }

    #[test]
    fn parses_legacy_single_trove_snapshot() {
        let raw = serde_json::to_string(&snapshot("fixture")).unwrap();

        let parsed = parse_rollback_snapshots(&raw).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "fixture");
    }

    #[test]
    fn parses_legacy_revert_metadata_wrapper() {
        let raw = serde_json::to_string(&RevertMetadata {
            removed_troves: vec![snapshot("one"), snapshot("two")],
        })
        .unwrap();

        let parsed = parse_rollback_snapshots(&raw).unwrap();

        assert_eq!(parsed.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["one", "two"]);
    }

    #[test]
    fn parses_versioned_envelope_snapshots_and_deferred_follow_up() {
        let warning = DeferredFollowUp {
            kind: "generation_rebuild".to_string(),
            status: "failed".to_string(),
            message: "root is not self-contained".to_string(),
            retry_command: Some(
                "conary --allow-live-system-mutation system generation build --summary \"Retry deferred package follow-up\""
                    .to_string(),
            ),
        };
        let raw = metadata_with_deferred_follow_up(vec![snapshot("fixture")], vec![warning.clone()])
            .unwrap();

        let parsed = parse_rollback_snapshots(&raw).unwrap();
        let deferred = deferred_follow_up(Some(&raw));

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "fixture");
        assert_eq!(deferred, vec![warning]);
    }

    #[test]
    fn malformed_or_legacy_metadata_has_no_deferred_follow_up() {
        let raw = serde_json::to_string(&snapshot("fixture")).unwrap();

        assert!(deferred_follow_up(Some(&raw)).is_empty());
        assert!(deferred_follow_up(Some("not-json")).is_empty());
        assert!(deferred_follow_up(None).is_empty());
    }
}
```

- [ ] **Step 2: Wire the module and verify tests compile-fail for call sites**

Modify `apps/conary/src/commands/mod.rs`:

```rust
mod changeset_metadata;
pub(crate) use changeset_metadata::{
    ChangesetMetadataEnvelope, DeferredFollowUp, deferred_follow_up,
    metadata_with_deferred_follow_up, metadata_with_removed_troves, parse_rollback_snapshots,
};
```

Run:

```bash
cargo test -p conary changeset_metadata -- --nocapture
```

Expected: metadata tests pass, or compilation fails because `system.rs` still has a private `parse_rollback_snapshots` with the same intent. If compilation fails, continue to Step 3.

- [ ] **Step 3: Replace rollback parsing in `system.rs`**

In `apps/conary/src/commands/system.rs`, delete the private `parse_rollback_snapshots` helper and call the shared function instead:

```rust
let snapshots = crate::commands::parse_rollback_snapshots(metadata.as_str())?;
```

Keep the existing `RevertMetadata` tests in `system.rs`, but change their call sites to the shared `crate::commands::parse_rollback_snapshots` helper.

- [ ] **Step 4: Run metadata and rollback parser tests**

Run:

```bash
cargo test -p conary changeset_metadata -- --nocapture
cargo test -p conary parse_rollback_snapshots -- --nocapture
```

Expected: both commands pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/changeset_metadata.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/system.rs
git commit -m "feat(history): add changeset metadata envelope"
```

## Task 2: Render Deferred Follow-Up In History

**Files:**
- Modify: `apps/conary/src/commands/query/history.rs`
- Test: `apps/conary/src/commands/query/history.rs`

- [ ] **Step 1: Add failing history formatting tests**

Add a formatting helper and tests to `apps/conary/src/commands/query/history.rs`. Keep `cmd_history` as the stdout-producing wrapper.

```rust
fn format_changeset_line(changeset: &conary_core::db::models::Changeset) -> String {
    let timestamp = changeset
        .applied_at
        .as_ref()
        .or(changeset.rolled_back_at.as_ref())
        .or(changeset.created_at.as_ref())
        .map(|s| s.as_str())
        .unwrap_or("pending");
    let id = changeset
        .id
        .map(|i| i.to_string())
        .unwrap_or_else(|| "?".to_string());
    let deferred = crate::commands::deferred_follow_up(changeset.metadata.as_deref());
    let marker = if deferred.is_empty() { "" } else { " [deferred]" };
    format!(
        "  [{}] {} - {} ({:?}){}",
        id, timestamp, changeset.description, changeset.status, marker
    )
}

fn format_deferred_follow_up_lines(
    changeset: &conary_core::db::models::Changeset,
) -> Vec<String> {
    crate::commands::deferred_follow_up(changeset.metadata.as_deref())
        .into_iter()
        .map(|follow_up| {
            let retry = follow_up
                .retry_command
                .map(|command| format!(" Retry: {command}."))
                .unwrap_or_default();
            format!(
                "      deferred {} {}: {}{}",
                follow_up.kind, follow_up.status, follow_up.message, retry
            )
        })
        .collect()
}
```

Add tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Changeset, ChangesetStatus};

    #[test]
    fn clean_applied_changeset_has_no_deferred_marker() {
        let mut changeset = Changeset::new("Install fixture-1.0.0".to_string());
        changeset.id = Some(7);
        changeset.status = ChangesetStatus::Applied;
        changeset.applied_at = Some("2026-05-14 12:00:00".to_string());

        assert_eq!(
            format_changeset_line(&changeset),
            "  [7] 2026-05-14 12:00:00 - Install fixture-1.0.0 (Applied)"
        );
        assert!(format_deferred_follow_up_lines(&changeset).is_empty());
    }

    #[test]
    fn applied_changeset_with_deferred_metadata_is_marked() {
        let warning = crate::commands::DeferredFollowUp {
            kind: "generation_rebuild".to_string(),
            status: "failed".to_string(),
            message: "root is not self-contained".to_string(),
            retry_command: Some("conary --allow-live-system-mutation system generation build --summary retry".to_string()),
        };
        let mut changeset = Changeset::new("Install fixture-1.0.0".to_string());
        changeset.id = Some(8);
        changeset.status = ChangesetStatus::Applied;
        changeset.applied_at = Some("2026-05-14 12:01:00".to_string());
        changeset.metadata = Some(
            crate::commands::metadata_with_deferred_follow_up(Vec::new(), vec![warning]).unwrap(),
        );

        assert_eq!(
            format_changeset_line(&changeset),
            "  [8] 2026-05-14 12:01:00 - Install fixture-1.0.0 (Applied) [deferred]"
        );
        let details = format_deferred_follow_up_lines(&changeset);
        assert_eq!(details.len(), 1);
        assert!(details[0].contains("deferred generation_rebuild failed"));
        assert!(details[0].contains("Retry: conary --allow-live-system-mutation"));
    }
}
```

- [ ] **Step 2: Run the history tests and verify they fail**

Run:

```bash
cargo test -p conary query::history -- --nocapture
```

Expected: tests fail until `cmd_history` uses the helper and the metadata module is wired.

- [ ] **Step 3: Update `cmd_history` to use the helper**

Replace the inline `println!` body in the changeset loop with:

```rust
for changeset in &changesets {
    println!("{}", format_changeset_line(changeset));
    for line in format_deferred_follow_up_lines(changeset) {
        println!("{line}");
    }
}
```

- [ ] **Step 4: Run the history tests**

Run:

```bash
cargo test -p conary query::history -- --nocapture
```

Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/query/history.rs
git commit -m "feat(history): show deferred follow-up"
```

## Task 3: Add Mutable Live-Root Transaction Helpers

**Files:**
- Create: `apps/conary/src/commands/live_root.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Test: `apps/conary/src/commands/live_root.rs`

- [ ] **Step 1: Add the live-root helper tests**

Create `apps/conary/src/commands/live_root.rs` with these tests and helper contract before install/remove are wired to it.

```rust
// apps/conary/src/commands/live_root.rs

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(crate) struct LiveRootFile {
    pub path: String,
    pub content: Vec<u8>,
    pub mode: i32,
    pub symlink_target: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct LiveRootStats {
    pub files_written: usize,
    pub files_removed: usize,
    pub dirs_created: usize,
    pub dirs_removed: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct LiveRootJournal {
    schema: String,
    tx_uuid: String,
    operation: String,
    state: String,
    backups: Vec<BackupRecord>,
    created_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupRecord {
    path: String,
    backup_path: String,
}

pub(crate) struct LiveRootTransaction {
    root: PathBuf,
    journal_path: PathBuf,
    tx_uuid: String,
    operation: String,
    backups: Vec<BackupRecord>,
    created_paths: Vec<PathBuf>,
    committed: bool,
}

pub(crate) fn target_path(root: &Path, package_path: &str) -> Result<PathBuf> {
    let relative = package_path.strip_prefix('/').unwrap_or(package_path);
    let relative_path = Path::new(relative);
    if relative_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("package path {package_path} escapes the target root");
    }
    Ok(root.join(relative_path))
}

impl LiveRootTransaction {
    pub(crate) fn begin(
        runtime_root: &Path,
        root: &Path,
        tx_uuid: String,
        operation: impl Into<String>,
    ) -> Result<Self> {
        let journal_dir = runtime_root.join("live-root-journals");
        fs::create_dir_all(&journal_dir)?;
        let operation = operation.into();
        let journal_path = journal_dir.join(format!("{tx_uuid}.json"));
        let transaction = Self {
            root: root.to_path_buf(),
            journal_path,
            tx_uuid,
            operation,
            backups: Vec::new(),
            created_paths: Vec::new(),
            committed: false,
        };
        transaction.write_journal("pending")?;
        Ok(transaction)
    }

    pub(crate) fn apply_install_files(&mut self, files: &[LiveRootFile]) -> Result<LiveRootStats> {
        let mut stats = LiveRootStats::default();
        for file in files {
            let target = target_path(&self.root, &file.path)?;
            self.ensure_parent(&target, &mut stats)?;
            self.backup_existing(&target)?;
            if let Some(target_value) = file.symlink_target.as_deref() {
                let temp = target.with_extension(format!("conary-tmp-{}", self.tx_uuid));
                let _ = fs::remove_file(&temp);
                symlink(target_value, &temp)
                    .with_context(|| format!("Failed to create symlink {}", temp.display()))?;
                fs::rename(&temp, &target)
                    .with_context(|| format!("Failed to move symlink {}", target.display()))?;
            } else {
                let temp = target.with_extension(format!("conary-tmp-{}", self.tx_uuid));
                fs::write(&temp, &file.content)
                    .with_context(|| format!("Failed to write {}", temp.display()))?;
                fs::set_permissions(&temp, fs::Permissions::from_mode((file.mode as u32) & 0o7777))?;
                fs::rename(&temp, &target)
                    .with_context(|| format!("Failed to move file {}", target.display()))?;
            }
            stats.files_written += 1;
            self.write_journal("in_progress")?;
        }
        Ok(stats)
    }

    pub(crate) fn apply_remove_paths(&mut self, package_paths: &[String]) -> Result<LiveRootStats> {
        let mut stats = LiveRootStats::default();
        let mut dirs = Vec::new();
        for package_path in package_paths {
            let target = target_path(&self.root, package_path)?;
            match fs::symlink_metadata(&target) {
                Ok(meta) if meta.is_dir() => dirs.push(target),
                Ok(_) => {
                    self.backup_existing(&target)?;
                    fs::remove_file(&target)
                        .with_context(|| format!("Failed to remove {}", target.display()))?;
                    stats.files_removed += 1;
                    self.write_journal("in_progress")?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).with_context(|| format!("Failed to inspect {}", target.display())),
            }
        }
        dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        dirs.dedup();
        for dir in dirs {
            match fs::remove_dir(&dir) {
                Ok(()) => stats.dirs_removed += 1,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) => {}
                Err(error) => return Err(error).with_context(|| format!("Failed to remove {}", dir.display())),
            }
        }
        Ok(stats)
    }

    pub(crate) fn rollback(&mut self) -> Result<()> {
        for created in self.created_paths.iter().rev() {
            if created.is_file() || created.is_symlink() {
                let _ = fs::remove_file(created);
            } else {
                let _ = fs::remove_dir(created);
            }
        }
        for backup in self.backups.iter().rev() {
            let target = PathBuf::from(&backup.path);
            let backup_path = PathBuf::from(&backup.backup_path);
            if backup_path.exists() {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(&backup_path, &target)?;
            }
        }
        self.write_journal("rolled_back")?;
        Ok(())
    }

    pub(crate) fn commit(mut self) -> Result<()> {
        self.committed = true;
        self.write_journal("committed")?;
        let _ = fs::remove_file(&self.journal_path);
        Ok(())
    }

    fn ensure_parent(&mut self, target: &Path, stats: &mut LiveRootStats) -> Result<()> {
        let Some(parent) = target.parent() else {
            return Ok(());
        };
        let mut current = PathBuf::new();
        for component in parent.strip_prefix(&self.root).unwrap_or(parent).components() {
            current.push(component.as_os_str());
            let full = self.root.join(&current);
            if !full.exists() {
                fs::create_dir(&full)?;
                self.created_paths.push(full);
                stats.dirs_created += 1;
            }
        }
        Ok(())
    }

    fn backup_existing(&mut self, target: &Path) -> Result<()> {
        if fs::symlink_metadata(target).is_err() {
            self.created_paths.push(target.to_path_buf());
            return Ok(());
        }
        let backup_dir = self.journal_path.with_extension("backups");
        fs::create_dir_all(&backup_dir)?;
        let backup_path = backup_dir.join(format!("backup-{}", self.backups.len()));
        fs::rename(target, &backup_path)?;
        self.backups.push(BackupRecord {
            path: target.to_string_lossy().into_owned(),
            backup_path: backup_path.to_string_lossy().into_owned(),
        });
        Ok(())
    }

    fn write_journal(&self, state: &str) -> Result<()> {
        let journal = LiveRootJournal {
            schema: "conary.live-root-journal.v1".to_string(),
            tx_uuid: self.tx_uuid.clone(),
            operation: self.operation.clone(),
            state: state.to_string(),
            backups: self.backups.clone(),
            created_paths: self
                .created_paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        };
        fs::write(&self.journal_path, serde_json::to_vec_pretty(&journal)?)?;
        Ok(())
    }
}

impl Drop for LiveRootTransaction {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}

pub(crate) fn recover_pending_journals(runtime_root: &Path, root: &Path) -> Result<()> {
    let journal_dir = runtime_root.join("live-root-journals");
    if !journal_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&journal_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read(&path)?;
        let journal: LiveRootJournal = serde_json::from_slice(&raw)?;
        if journal.state == "committed" || journal.state == "rolled_back" {
            let _ = fs::remove_file(&path);
            continue;
        }
        let mut tx = LiveRootTransaction {
            root: root.to_path_buf(),
            journal_path: path,
            tx_uuid: journal.tx_uuid,
            operation: journal.operation,
            backups: journal.backups,
            created_paths: journal.created_paths.into_iter().map(PathBuf::from).collect(),
            committed: false,
        };
        tx.rollback()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn target_path_rejects_parent_dir_escape() {
        let root = TempDir::new().unwrap();
        let err = target_path(root.path(), "/usr/../escape").unwrap_err().to_string();

        assert!(err.contains("escapes the target root"));
    }

    #[test]
    fn install_writes_regular_file_and_symlink() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&root).unwrap();
        let mut tx = LiveRootTransaction::begin(
            &runtime,
            &root,
            Uuid::new_v4().to_string(),
            "install fixture",
        )
        .unwrap();

        let stats = tx
            .apply_install_files(&[
                LiveRootFile {
                    path: "/usr/bin/fixture".to_string(),
                    content: b"fixture".to_vec(),
                    mode: 0o100755,
                    symlink_target: None,
                },
                LiveRootFile {
                    path: "/usr/bin/fixture-link".to_string(),
                    content: Vec::new(),
                    mode: 0o120777,
                    symlink_target: Some("fixture".to_string()),
                },
            ])
            .unwrap();
        tx.commit().unwrap();

        assert_eq!(stats.files_written, 2);
        assert_eq!(fs::read_to_string(root.join("usr/bin/fixture")).unwrap(), "fixture");
        assert_eq!(
            fs::read_link(root.join("usr/bin/fixture-link")).unwrap(),
            PathBuf::from("fixture")
        );
        assert_eq!(
            fs::metadata(root.join("usr/bin/fixture")).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn rollback_restores_replaced_file() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::write(root.join("usr/bin/fixture"), "old").unwrap();

        let mut tx = LiveRootTransaction::begin(
            &runtime,
            &root,
            Uuid::new_v4().to_string(),
            "install fixture",
        )
        .unwrap();
        tx.apply_install_files(&[LiveRootFile {
            path: "/usr/bin/fixture".to_string(),
            content: b"new".to_vec(),
            mode: 0o100755,
            symlink_target: None,
        }])
        .unwrap();
        tx.rollback().unwrap();

        assert_eq!(fs::read_to_string(root.join("usr/bin/fixture")).unwrap(), "old");
    }

    #[test]
    fn remove_deletes_files_and_empty_dirs() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("usr/share/fixture")).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::write(root.join("usr/share/fixture/readme"), "fixture").unwrap();

        let mut tx = LiveRootTransaction::begin(
            &runtime,
            &root,
            Uuid::new_v4().to_string(),
            "remove fixture",
        )
        .unwrap();
        let stats = tx
            .apply_remove_paths(&[
                "/usr/share/fixture/readme".to_string(),
                "/usr/share/fixture/".to_string(),
            ])
            .unwrap();
        tx.commit().unwrap();

        assert_eq!(stats.files_removed, 1);
        assert_eq!(stats.dirs_removed, 1);
        assert!(!root.join("usr/share/fixture").exists());
    }
}
```

- [ ] **Step 2: Wire the module**

Modify `apps/conary/src/commands/mod.rs`:

```rust
mod live_root;
pub(crate) use live_root::{
    LiveRootFile, LiveRootStats, LiveRootTransaction, recover_pending_journals, target_path,
};
```

- [ ] **Step 3: Confirm dependency wiring**

Confirm `apps/conary/Cargo.toml` already has `uuid.workspace = true`. The current workspace has this dependency available, so no Cargo edit should be needed for `Uuid::new_v4()`.

- [ ] **Step 4: Run live-root tests**

Run:

```bash
cargo test -p conary live_root -- --nocapture
```

Expected: all live-root tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/live_root.rs apps/conary/src/commands/mod.rs Cargo.toml apps/conary/Cargo.toml
git commit -m "feat(package): add live-root transaction helper"
```

## Task 4: Refactor Install Inner To Reuse CAS File Hashes

**Files:**
- Modify: `apps/conary/src/commands/install/inner.rs`
- Test: `apps/conary/src/commands/install/inner.rs`

- [ ] **Step 1: Add the reusable stored-file type and tests**

In `apps/conary/src/commands/install/inner.rs`, add this type near `InnerInstallResult`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StoredInstallFile {
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub mode: i32,
    pub symlink_target: Option<String>,
}
```

Add a test beside `install_inner_replaces_live_root_owned_overlapping_path`:

```rust
#[test]
fn store_install_files_in_cas_preserves_symlink_targets() {
    let temp = tempfile::tempdir().unwrap();
    let config = TransactionConfig::new(temp.path());
    let engine = TransactionEngine::new(config).unwrap();
    let package = FakePackage {
        name: "fixture".to_string(),
        version: "1.0.0".to_string(),
        files: vec![],
        extracted_files: vec![ExtractedFile {
            path: "/usr/bin/fixture-link".to_string(),
            content: Vec::new(),
            size: 7,
            mode: 0o120777,
            sha256: None,
            symlink_target: Some("fixture".to_string()),
        }],
        dependencies: Vec::new(),
        scriptlets: Vec::new(),
    };
    let extraction = ExtractionResult {
        extracted_files: package.extracted_files.clone(),
        classified: HashMap::from([(
            conary_core::components::ComponentType::Runtime,
            vec!["/usr/bin/fixture-link".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_pre_remove_script: None,
        installed_component_types: vec![conary_core::components::ComponentType::Runtime],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };

    let stored = store_install_files_in_cas(&engine, &extraction).unwrap();

    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].path, "/usr/bin/fixture-link");
    assert_eq!(stored[0].symlink_target.as_deref(), Some("fixture"));
    assert!(!stored[0].hash.is_empty());
}
```

- [ ] **Step 2: Extract CAS storage**

Move the current file-hash loop out of `install_inner` into:

```rust
pub(super) fn store_install_files_in_cas(
    engine: &TransactionEngine,
    extraction: &ExtractionResult,
) -> Result<Vec<StoredInstallFile>> {
    let mut stored = Vec::with_capacity(extraction.extracted_files.len());
    for file in &extraction.extracted_files {
        let hash = if let Some(target) = file.symlink_target.as_deref() {
            engine
                .cas()
                .store_symlink(target)
                .with_context(|| format!("Failed to store symlink {} in CAS", file.path))?
        } else {
            engine
                .cas()
                .store(&file.content)
                .with_context(|| format!("Failed to store {} in CAS", file.path))?
        };
        stored.push(StoredInstallFile {
            path: file.path.clone(),
            hash,
            size: file.size,
            mode: file.mode,
            symlink_target: file.symlink_target.clone(),
        });
    }
    Ok(stored)
}
```

- [ ] **Step 3: Split DB writes from CAS storage**

Add a new helper with the same body as the current DB-writing portion of `install_inner`, but use `stored_files` instead of the old local `file_hashes` tuple vector:

```rust
pub(super) fn install_inner_with_stored_files(
    tx: &Transaction<'_>,
    changeset_id: i64,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    stored_files: &[StoredInstallFile],
) -> Result<InnerInstallResult> {
    let is_upgrade = ctx.old_trove_to_upgrade.is_some();
    let selection_reason = ctx.selection_reason;
    let classified = &extraction.classified;
    let language_provides = &extraction.language_provides;
    let scriptlets = pkg.scriptlets();

    let trove_id = {
        if let Some(old_trove) = ctx.old_trove_to_upgrade
            && let Some(old_id) = old_trove.id
        {
            info!("Removing old version {} before upgrade", old_trove.version);
            conary_core::db::models::Trove::delete(tx, old_id)?;
        }

        let mut trove = pkg.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.version_scheme = Some(scheme_to_string(ctx.semantics.version_scheme));
        if let Some(reason) = selection_reason {
            trove.selection_reason = Some(reason.to_string());
        }

        let trove_id = trove.insert(tx)?;

        let mut path_to_component: HashMap<&str, i64> = HashMap::new();
        if let (Some(component_names), Some(component_names_by_path)) = (
            extraction.installed_component_names.as_ref(),
            extraction.component_names_by_path.as_ref(),
        ) {
            let mut component_ids: HashMap<&str, i64> = HashMap::new();
            for component_name in component_names {
                let mut component = Component::new(trove_id, component_name.clone());
                component.description = Some(format!("{component_name} files"));
                let comp_id = component.insert(tx)?;
                component_ids.insert(component_name.as_str(), comp_id);
            }
            for (path, component_name) in component_names_by_path {
                if let Some(&comp_id) = component_ids.get(component_name.as_str()) {
                    path_to_component.insert(path.as_str(), comp_id);
                }
            }
        } else {
            let mut component_ids: HashMap<ComponentType, i64> = HashMap::new();
            for comp_type in classified.keys() {
                let mut component = Component::from_type(trove_id, *comp_type);
                component.description = Some(format!("{} files", comp_type.as_str()));
                let comp_id = component.insert(tx)?;
                component_ids.insert(*comp_type, comp_id);
            }
            for (comp_type, files) in classified {
                if let Some(&comp_id) = component_ids.get(comp_type) {
                    for path in files {
                        path_to_component.insert(path.as_str(), comp_id);
                    }
                }
            }
        }

        for file in stored_files {
            if file.hash.len() < 3 {
                warn!("Skipping file with short hash: {} (hash={})", file.path, file.hash);
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    &file.hash,
                    &format!("objects/{}/{}", &file.hash[0..2], &file.hash[2..]),
                    file.size,
                ],
            )?;
            let component_id = path_to_component.get(file.path.as_str()).copied();
            let mut file_entry = FileEntry::new(
                file.path.clone(),
                file.hash.clone(),
                file.size,
                file.mode,
                trove_id,
            );
            file_entry.component_id = component_id;
            file_entry.symlink_target = file.symlink_target.clone();
            insert_file_entry_claiming_live_root_overlap(tx, &mut file_entry, pkg.name())?;

            let action = if is_upgrade { "modify" } else { "add" };
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![changeset_id, &file.path, &file.hash, action],
            )?;
        }

        for dep in pkg.dependencies() {
            let mut dep_entry = DependencyEntry::new(
                trove_id,
                dep.name.clone(),
                None,
                dep.dep_type.as_str().to_string(),
                dep.version.clone(),
            );
            dep_entry.insert(tx)?;
        }

        for scriptlet in scriptlets {
            let mut entry = ScriptletEntry::with_flags(
                trove_id,
                scriptlet.phase.to_string(),
                scriptlet.interpreter.clone(),
                scriptlet.content.clone(),
                scriptlet.flags.clone(),
                match ctx.semantics.source {
                    super::PreparedSourceKind::Legacy { format } => format.as_str(),
                    super::PreparedSourceKind::Ccs => "ccs",
                },
            );
            entry.insert(tx)?;
        }

        if let Some(script) = extraction.ccs_pre_remove_script.as_deref() {
            let mut entry = ScriptletEntry::new(
                trove_id,
                "pre-remove".to_string(),
                "/bin/sh".to_string(),
                script.to_string(),
                "ccs",
            );
            entry.insert(tx)?;
        }

        for lang_dep in language_provides {
            let kind = match lang_dep.class {
                DependencyClass::Package => "package",
                _ => lang_dep.class.prefix(),
            };
            let mut provide = ProvideEntry::new_typed(
                trove_id,
                kind,
                lang_dep.name.clone(),
                lang_dep.version_constraint.clone(),
            );
            provide.insert_or_ignore(tx)?;
        }

        let mut pkg_provide = ProvideEntry::new(
            trove_id,
            pkg.name().to_string(),
            Some(pkg.version().to_string()),
        );
        pkg_provide.insert_or_ignore(tx)?;

        trove_id
    };

    if let Some(old_trove) = ctx.old_trove_to_upgrade {
        mark_upgraded_parent_deriveds_stale(
            tx,
            pkg.name(),
            Some(&old_trove.version),
            pkg.version(),
        );
    }

    Ok(InnerInstallResult { trove_id })
}
```

The helper must include the existing repository ID lookup block from the current `install_inner` before `trove.insert(tx)`. Keep the current `installed_from_repository_id` behavior byte-for-byte when moving that block.

- [ ] **Step 4: Keep `install_inner` as the existing wrapper**

Replace the body of `install_inner` with:

```rust
progress.set_phase(pkg.name(), InstallPhase::Deploying);
let stored_files = store_install_files_in_cas(engine, extraction)?;
info!("Stored {} files in CAS for {}", stored_files.len(), pkg.name());
install_inner_with_stored_files(tx, changeset_id, pkg, extraction, ctx, &stored_files)
```

- [ ] **Step 5: Run install inner tests**

Run:

```bash
cargo test -p conary install::inner -- --nocapture
```

Expected: all install inner tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/install/inner.rs
git commit -m "refactor(install): split CAS storage from DB writes"
```

## Task 5: Wire No-Generation Install To Live Root

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/inner.rs`
- Test: `apps/conary/src/commands/install/mod.rs`

- [ ] **Step 1: Add execution-path detection**

In `apps/conary/src/commands/install/mod.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageExecutionPath {
    GenerationAware,
    MutableLiveRoot,
}

fn package_execution_path(db_path: &str) -> PackageExecutionPath {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(std::path::PathBuf::from(db_path));
    match conary_core::generation::mount::current_generation(runtime_root.root()).unwrap_or(None) {
        Some(_) => PackageExecutionPath::GenerationAware,
        None => PackageExecutionPath::MutableLiveRoot,
    }
}
```

Change `TransactionContext`:

```rust
execution_path: PackageExecutionPath,
```

Then update all `TransactionContext` construction sites. For the main `cmd_install` path:

```rust
execution_path: package_execution_path(db_path),
```

For CCS direct-install callers that still pass `defer_generation`, map the flag without preserving DB-only behavior:

```rust
execution_path: if opts.defer_generation {
    PackageExecutionPath::MutableLiveRoot
} else {
    package_execution_path(opts.db_path)
},
```

- [ ] **Step 2: Add failing no-generation install test**

Add a focused test in `apps/conary/src/commands/install/mod.rs` under the existing `#[cfg(test)]` module or create one if the file does not already have a test module:

```rust
#[test]
fn no_generation_install_transaction_materializes_live_root_file() {
    use conary_core::db::models::{ChangesetStatus, FileEntry, Trove};
    use conary_core::packages::traits::{Dependency, ExtractedFile, PackageFile, PackageFormat, Scriptlet};
    use std::collections::HashMap;

    struct FakePackage;
    impl PackageFormat for FakePackage {
        fn parse(_path: &str) -> conary_core::Result<Self> {
            unreachable!("test constructs package directly")
        }
        fn name(&self) -> &str { "fixture" }
        fn version(&self) -> &str { "1.0.0" }
        fn architecture(&self) -> Option<&str> { Some("x86_64") }
        fn description(&self) -> Option<&str> { None }
        fn files(&self) -> &[PackageFile] { &[] }
        fn dependencies(&self) -> &[Dependency] { &[] }
        fn extract_file_contents(&self) -> conary_core::Result<Vec<ExtractedFile>> { Ok(vec![]) }
        fn scriptlets(&self) -> &[Scriptlet] { &[] }
        fn to_trove(&self) -> Trove {
            Trove::new("fixture".to_string(), "1.0.0".to_string(), conary_core::db::models::TroveType::Package)
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let extraction = ExtractionResult {
        extracted_files: vec![ExtractedFile {
            path: "/usr/bin/fixture".to_string(),
            content: b"fixture".to_vec(),
            size: 7,
            mode: 0o100755,
            sha256: None,
            symlink_target: None,
        }],
        classified: HashMap::from([(
            conary_core::components::ComponentType::Runtime,
            vec!["/usr/bin/fixture".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_pre_remove_script: None,
        installed_component_types: vec![conary_core::components::ComponentType::Runtime],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };
    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();
    let ctx = TransactionContext {
        db_path: &db_path_string,
        root: &root_string,
        semantics: InstallSemantics::legacy(PackageFormatType::Rpm),
        selection_reason: None,
        old_trove_to_upgrade: None,
        ccs_manifest_provides: None,
        ccs_capabilities: None,
        execution_path: PackageExecutionPath::MutableLiveRoot,
    };

    let result = execute_install_transaction(
        &mut conn,
        &FakePackage,
        &extraction,
        &ctx,
        &InstallProgress::single("Installing"),
    )
    .unwrap();

    assert_eq!(std::fs::read_to_string(root.join("usr/bin/fixture")).unwrap(), "fixture");
    assert!(FileEntry::find_by_path(&conn, "/usr/bin/fixture").unwrap().is_some());
    let changeset = conary_core::db::models::Changeset::find_by_id(&conn, result.changeset_id)
        .unwrap()
        .unwrap();
    assert_eq!(changeset.status, ChangesetStatus::Applied);
}
```

- [ ] **Step 3: Run the failing install test**

Run:

```bash
cargo test -p conary no_generation_install_transaction_materializes_live_root_file -- --nocapture
```

Expected: test fails because `execute_install_transaction` still skips or rebuilds generation instead of materializing files.

- [ ] **Step 4: Implement mutable live-root install branch**

In `execute_install_transaction`, keep the current generation-aware branch unchanged except for replacing `ctx.defer_generation` with `ctx.execution_path`.

Add this mutable branch after acquiring the engine lock and before the current DB-first transaction body:

```rust
if ctx.execution_path == PackageExecutionPath::MutableLiveRoot {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(ctx.db_path));
    crate::commands::recover_pending_journals(runtime_root.root(), Path::new(ctx.root))?;

    let tx_uuid = uuid::Uuid::new_v4().to_string();
    let tx_description = if let Some(old_trove) = ctx.old_trove_to_upgrade {
        format!(
            "Upgrade {} from {} to {}",
            pkg.name(),
            old_trove.version,
            pkg.version()
        )
    } else {
        format!("Install {}-{}", pkg.name(), pkg.version())
    };
    let mut changeset = Changeset::with_tx_uuid(tx_description.clone(), tx_uuid.clone());
    let changeset_id = changeset.insert(conn)?;
    let stored_files = inner::store_install_files_in_cas(&engine, extraction)?;
    let live_files = extraction
        .extracted_files
        .iter()
        .map(|file| crate::commands::LiveRootFile {
            path: file.path.clone(),
            content: file.content.clone(),
            mode: file.mode,
            symlink_target: file.symlink_target.clone(),
        })
        .collect::<Vec<_>>();
    let mut live_tx = crate::commands::LiveRootTransaction::begin(
        runtime_root.root(),
        Path::new(ctx.root),
        tx_uuid,
        tx_description,
    )?;
    live_tx.apply_install_files(&live_files)?;

    let tx = conn.unchecked_transaction()?;
    let inner_result = match inner::install_inner_with_stored_files(
        &tx,
        changeset_id,
        pkg,
        extraction,
        ctx,
        &stored_files,
    ) {
        Ok(result) => result,
        Err(error) => {
            live_tx.rollback()?;
            return Err(error);
        }
    };
    if let Some(provides) = ctx.ccs_manifest_provides {
        persist_ccs_manifest_provides(&tx, inner_result.trove_id, pkg.name(), provides)?;
    }
    if let Some(capabilities) = ctx.ccs_capabilities {
        conary_core::capability::store_capabilities(&tx, inner_result.trove_id, capabilities)?;
    }
    tx.commit().inspect_err(|_| {
        let _ = live_tx.rollback();
    })?;
    changeset.update_status(conn, ChangesetStatus::Applied)?;
    live_tx.commit()?;
    engine.release_lock();
    return Ok(InstallTransactionResult { changeset_id });
}
```

After adding this branch, remove the old `ctx.defer_generation` branch that marked a DB-only install as applied.

- [ ] **Step 5: Run install transaction tests**

Run:

```bash
cargo test -p conary no_generation_install_transaction_materializes_live_root_file -- --nocapture
cargo test -p conary install::inner -- --nocapture
```

Expected: tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/install/mod.rs apps/conary/src/commands/install/inner.rs
git commit -m "feat(install): materialize no-generation live-root installs"
```

## Task 6: Wire No-Generation Remove To Live Root

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`
- Test: `apps/conary/src/commands/remove.rs`

- [ ] **Step 1: Replace the old active-generation refusal test**

Delete `remove_requires_active_generation_before_live_root_mutation` and replace `purge_remove_requires_active_generation_before_touching_files` with:

```rust
#[tokio::test]
async fn no_generation_remove_deletes_files_and_db_rows() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let db_path = root.join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let payload = root.join("usr/bin/fixture");
    std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
    std::fs::write(&payload, "fixture").unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = conary_core::db::models::Trove::new_with_source(
        "fixture".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::db::models::InstallSource::Repository,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut file = conary_core::db::models::FileEntry::new(
        "/usr/bin/fixture".to_string(),
        "0".repeat(64),
        "fixture".len() as i64,
        0o100755,
        trove_id,
    );
    file.insert(&conn).unwrap();
    drop(conn);

    cmd_remove(
        "fixture",
        db_path.to_string_lossy().as_ref(),
        root.to_string_lossy().as_ref(),
        None,
        None,
        true,
        SandboxMode::None,
        false,
    )
    .await
    .unwrap();

    assert!(!payload.exists());
    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(conary_core::db::models::Trove::find_by_name(&conn, "fixture").unwrap().is_empty());
}
```

Add a critical package refusal test:

```rust
#[tokio::test]
async fn remove_refuses_critical_package_before_file_mutation() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let db_path = root.join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let payload = root.join("usr/bin/bash");
    std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
    std::fs::write(&payload, "bash").unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = conary_core::db::models::Trove::new_with_source(
        "bash".to_string(),
        "5.2".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::db::models::InstallSource::Repository,
    );
    let trove_id = trove.insert(&conn).unwrap();
    conary_core::db::models::FileEntry::new(
        "/usr/bin/bash".to_string(),
        "0".repeat(64),
        4,
        0o100755,
        trove_id,
    )
    .insert(&conn)
    .unwrap();
    drop(conn);

    let err = cmd_remove(
        "bash",
        db_path.to_string_lossy().as_ref(),
        root.to_string_lossy().as_ref(),
        None,
        None,
        true,
        SandboxMode::None,
        false,
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(err.contains("critical package"));
    assert_eq!(std::fs::read_to_string(&payload).unwrap(), "bash");
}
```

- [ ] **Step 2: Run the failing remove tests**

Run:

```bash
cargo test -p conary no_generation_remove_deletes_files_and_db_rows remove_refuses_critical_package_before_file_mutation -- --nocapture
```

Expected: no-generation remove fails on the active generation gate, and critical remove is not blocked by critical package policy yet.

- [ ] **Step 3: Add critical/adopted preflight before engine begin**

In `cmd_remove`, after the pinned check and before dependency breakage:

```rust
if crate::commands::install::is_package_blocked(&trove.name) {
    anyhow::bail!(
        "Refusing to remove critical package '{}'. Use the native package manager for this system package.",
        trove.name
    );
}
```

Change adopted no-purge behavior from tracking removal to refusal:

```rust
if trove.install_source.is_adopted() && !purge_files {
    let pkg_mgr = conary_core::packages::SystemPackageManager::detect();
    anyhow::bail!(
        "Refusing to remove adopted package '{}': native package manager authority is preserved. \
         Use '{}' to uninstall it, or 'conary system unadopt {}' to remove Conary tracking only.",
        package_name,
        pkg_mgr.remove_command(package_name),
        package_name
    );
}
```

- [ ] **Step 4: Split the remove execution path**

First split `remove_inner` into a preparation phase and a DB commit phase so the no-generation path does not run pre-remove scriptlets twice:

```rust
pub(crate) struct PreparedRemove {
    pub(crate) snapshot: TroveSnapshot,
    trove: Trove,
    stored_scriptlets: Vec<ScriptletEntry>,
    scriptlet_format: ScriptletPackageFormat,
    removed_count: usize,
    dirs_removed: usize,
}

fn prepare_remove(
    conn: &rusqlite::Connection,
    trove: &Trove,
    root: &str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    progress: &RemoveProgress,
) -> Result<PreparedRemove> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;
    let files = FileEntry::find_by_trove(conn, trove_id)?;
    let stored_scriptlets = ScriptletEntry::find_by_trove(conn, trove_id)?;
    let scriptlet_format = stored_scriptlets
        .first()
        .and_then(|s| ScriptletPackageFormat::parse(&s.package_format))
        .unwrap_or(ScriptletPackageFormat::Rpm);

    if !no_scripts && !stored_scriptlets.is_empty() {
        progress.set_phase(RemovePhase::PreScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        )
        .with_sandbox_mode(sandbox_mode);
        if let Some(pre) = stored_scriptlets.iter().find(|s| s.phase == "pre-remove") {
            executor.execute_entry(pre, &ExecutionMode::Remove)?;
        }
    }

    let breaking_now =
        conary_core::resolver::solve_removal(conn, std::slice::from_ref(&trove.name))?;
    if !breaking_now.is_empty() {
        return Err(conary_core::Error::IoError(format!(
            "Concurrent change: '{}' now required by: {}",
            trove.name,
            breaking_now.join(", ")
        ))
        .into());
    }

    let (directories, regular_files): (Vec<_>, Vec<_>) = files
        .iter()
        .partition(|f| f.path.ends_with('/') || (f.permissions & 0o170000) == 0o040000);
    let snapshot = TroveSnapshot {
        name: trove.name.clone(),
        version: trove.version.clone(),
        architecture: trove.architecture.clone(),
        description: trove.description.clone(),
        install_source: trove.install_source.as_str().to_string(),
        installed_from_repository_id: trove.installed_from_repository_id,
        files: files
            .iter()
            .map(|f| FileSnapshot {
                path: f.path.clone(),
                sha256_hash: f.sha256_hash.clone(),
                size: f.size,
                permissions: f.permissions,
                symlink_target: f.symlink_target.clone(),
            })
            .collect(),
    };

    Ok(PreparedRemove {
        snapshot,
        trove: trove.clone(),
        stored_scriptlets,
        scriptlet_format,
        removed_count: regular_files.len(),
        dirs_removed: directories.len(),
    })
}

fn commit_remove_db(
    tx: &rusqlite::Transaction<'_>,
    changeset_id: i64,
    prepared: PreparedRemove,
) -> Result<RemoveInnerResult> {
    let trove_id = prepared
        .trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;
    for file in &prepared.snapshot.files {
        let hash = if file.sha256_hash.len() == 64
            && file.sha256_hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            Some(file.sha256_hash.as_str())
        } else {
            None
        };
        match hash {
            Some(hash) => tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![changeset_id, &file.path, hash, "delete"],
            )?,
            None => tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, NULL, ?3)",
                rusqlite::params![changeset_id, &file.path, "delete"],
            )?,
        };
    }
    conary_core::db::models::Trove::delete(tx, trove_id)?;
    Ok(RemoveInnerResult {
        snapshot: prepared.snapshot,
        trove: prepared.trove,
        stored_scriptlets: prepared.stored_scriptlets,
        scriptlet_format: prepared.scriptlet_format,
        removed_count: prepared.removed_count,
        dirs_removed: prepared.dirs_removed,
    })
}
```

Then reduce `remove_inner` to:

```rust
let prepared = prepare_remove(tx, trove, root, no_scripts, sandbox_mode, progress)?;
commit_remove_db(tx, changeset_id, prepared)
```

Replace the active-generation hard bail with an execution-path branch:

```rust
let runtime_root =
    conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
let active_generation =
    conary_core::generation::mount::current_generation(runtime_root.root()).unwrap_or(None);

if active_generation.is_none() {
    crate::commands::recover_pending_journals(runtime_root.root(), Path::new(root))?;
    let tx_uuid = uuid::Uuid::new_v4().to_string();
    let mut changeset =
        conary_core::db::models::Changeset::with_tx_uuid(
            format!("Remove {}-{}", trove.name, trove.version),
            tx_uuid.clone(),
        );
    let remove_changeset_id = changeset.insert(&conn)?;

    let prepared = prepare_remove(
        &conn,
        trove,
        root,
        no_scripts,
        sandbox_mode,
        &progress,
    )?;

    let remove_paths = prepared
        .snapshot
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let mut live_tx = crate::commands::LiveRootTransaction::begin(
        runtime_root.root(),
        Path::new(root),
        tx_uuid,
        format!("Remove {}", package_name),
    )?;
    let stats = live_tx.apply_remove_paths(&remove_paths)?;

    let tx = conn.unchecked_transaction()?;
    let remove_result = match commit_remove_db(
        &tx,
        remove_changeset_id,
        prepared,
    ) {
        Ok(result) => result,
        Err(error) => {
            live_tx.rollback()?;
            return Err(error);
        }
    };
    let snapshot_json = crate::commands::metadata_with_removed_troves(vec![remove_result.snapshot.clone()])?;
    tx.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![snapshot_json, remove_changeset_id],
    )?;
    tx.commit().inspect_err(|_| {
        let _ = live_tx.rollback();
    })?;
    changeset.update_status(&conn, conary_core::db::models::ChangesetStatus::Applied)?;
    live_tx.commit()?;
    engine.release_lock();

    print_remove_summary(&remove_result, &stats);
    return Ok(());
}
```

Extract the existing final remove summary printing into:

```rust
fn print_remove_summary(remove_result: &RemoveInnerResult, stats: &crate::commands::LiveRootStats) {
    println!(
        "Removed package: {} version {}",
        remove_result.trove.name, remove_result.trove.version
    );
    println!(
        "  Architecture: {}",
        remove_result.trove.architecture.as_deref().unwrap_or("none")
    );
    println!("  Files removed: {}", stats.files_removed);
    if stats.dirs_removed > 0 {
        println!("  Directories removed: {}", stats.dirs_removed);
    }
}
```

Keep the active-generation composefs branch using `rebuild_and_mount`.

- [ ] **Step 5: Run remove tests**

Run:

```bash
cargo test -p conary no_generation_remove_deletes_files_and_db_rows remove_refuses_critical_package_before_file_mutation -- --nocapture
cargo test -p conary direct_live_root_removal -- --nocapture
```

Expected: tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/remove.rs
git commit -m "feat(remove): support no-generation live-root removal"
```

## Task 7: Record Deferred Follow-Up Instead Of Failing After Required Mutation

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Test: `apps/conary/src/commands/install/mod.rs`
- Test: `apps/conary/src/commands/remove.rs`

- [ ] **Step 1: Add a helper to record deferred follow-up**

Add a small helper in `apps/conary/src/commands/changeset_metadata.rs`:

```rust
pub(crate) fn append_deferred_follow_up_metadata(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    follow_up: DeferredFollowUp,
) -> Result<()> {
    let existing: Option<String> = conn.query_row(
        "SELECT metadata FROM changesets WHERE id = ?1",
        [changeset_id],
        |row| row.get(0),
    )?;
    let mut removed_troves = existing
        .as_deref()
        .map(parse_rollback_snapshots)
        .transpose()?
        .unwrap_or_default();
    let mut deferred = deferred_follow_up(existing.as_deref());
    deferred.push(follow_up);
    let metadata = metadata_with_deferred_follow_up(std::mem::take(&mut removed_troves), deferred)?;
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![metadata, changeset_id],
    )?;
    Ok(())
}
```

Export it from `commands/mod.rs`.

- [ ] **Step 2: Add a focused metadata append test**

Add in `changeset_metadata.rs`:

```rust
#[test]
fn append_deferred_follow_up_preserves_removed_troves() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut changeset = conary_core::db::models::Changeset::new("Remove fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    let initial = metadata_with_removed_troves(vec![snapshot("fixture")]).unwrap();
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![initial, changeset_id],
    )
    .unwrap();

    append_deferred_follow_up_metadata(
        &conn,
        changeset_id,
        DeferredFollowUp {
            kind: "state_snapshot".to_string(),
            status: "failed".to_string(),
            message: "snapshot failed".to_string(),
            retry_command: Some("conary system state create \"Remove fixture\"".to_string()),
        },
    )
    .unwrap();

    let raw: String = conn
        .query_row(
            "SELECT metadata FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parse_rollback_snapshots(&raw).unwrap()[0].name, "fixture");
    assert_eq!(deferred_follow_up(Some(&raw)).len(), 1);
}
```

- [ ] **Step 3: Convert generation-aware post-commit failures to deferred metadata**

In generation-aware install and remove branches, wrap `rebuild_and_mount` and `create_state_snapshot` failures after required mutation succeeded. The pattern is:

```rust
let rebuild_result = crate::commands::composefs_ops::rebuild_and_mount(
    conn,
    ctx.db_path,
    &tx_description,
    Some(prev_etc),
);
if let Err(error) = rebuild_result {
    crate::commands::append_deferred_follow_up_metadata(
        conn,
        changeset_id,
        crate::commands::DeferredFollowUp {
            kind: "generation_rebuild".to_string(),
            status: "failed".to_string(),
            message: error.to_string(),
            retry_command: Some(
                "conary --allow-live-system-mutation system generation build --summary \"Retry deferred package follow-up\""
                    .to_string(),
            ),
        },
    )?;
    eprintln!(
        "WARNING: package mutation completed, but generation rebuild was deferred: {error}"
    );
}
changeset.update_status(conn, ChangesetStatus::Applied)?;
```

Use the same pattern for state snapshot failures in `finalize_install` and the remove path:

```rust
if let Err(error) = create_state_snapshot(conn, tx_result.changeset_id, &format!("Install {}", pkg.name())) {
    crate::commands::append_deferred_follow_up_metadata(
        conn,
        tx_result.changeset_id,
        crate::commands::DeferredFollowUp {
            kind: "state_snapshot".to_string(),
            status: "failed".to_string(),
            message: error.to_string(),
            retry_command: Some(format!("conary system state create \"Install {}\"", pkg.name())),
        },
    )?;
    eprintln!("WARNING: package mutation completed, but state snapshot was deferred: {error}");
}
```

- [ ] **Step 4: Run metadata/history/install/remove tests**

Run:

```bash
cargo test -p conary changeset_metadata -- --nocapture
cargo test -p conary query::history -- --nocapture
cargo test -p conary no_generation_install_transaction_materializes_live_root_file -- --nocapture
cargo test -p conary no_generation_remove_deletes_files_and_db_rows -- --nocapture
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/changeset_metadata.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/install/mod.rs apps/conary/src/commands/remove.rs
git commit -m "feat(package): record deferred package follow-up"
```

## Task 8: Add CLI-Level No-Generation Proof

**Files:**
- Create: `apps/conary/tests/native_pm_live_root.rs`

- [ ] **Step 1: Add CLI tests for no-generation remove and history**

Create `apps/conary/tests/native_pm_live_root.rs`:

```rust
// apps/conary/tests/native_pm_live_root.rs

use std::process::Command;

fn run_conary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

#[test]
fn no_generation_remove_deletes_file_and_history_records_apply() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let payload = root.path().join("usr/bin/fixture");
    std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
    std::fs::write(&payload, "fixture").unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = conary_core::db::models::Trove::new_with_source(
        "fixture".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::db::models::InstallSource::Repository,
    );
    let trove_id = trove.insert(&conn).unwrap();
    conary_core::db::models::FileEntry::new(
        "/usr/bin/fixture".to_string(),
        "0".repeat(64),
        7,
        0o100755,
        trove_id,
    )
    .insert(&conn)
    .unwrap();
    drop(conn);

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "remove",
        "fixture",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!payload.exists());

    let history = run_conary(&[
        "system",
        "history",
        "--db-path",
        db_path.to_str().unwrap(),
    ]);
    assert!(history.status.success());
    let stdout = String::from_utf8_lossy(&history.stdout);
    assert!(stdout.contains("Remove fixture-1.0.0"));
    assert!(stdout.contains("Applied"));
}
```

- [ ] **Step 2: Record why local native install CLI proof stays out of Slice A**

Confirm the current local native package fixtures are corrupted adversarial fixtures:

```bash
rg --files apps/conary/tests/fixtures | rg '\\.(rpm|deb|pkg\\.tar\\.zst)$'
```

Expected current output:

```text
apps/conary/tests/fixtures/adversarial/corrupted/native/output/native-package-corrupted.rpm
apps/conary/tests/fixtures/adversarial/corrupted/native/output/native-package-corrupted.pkg.tar.zst
apps/conary/tests/fixtures/adversarial/corrupted/native/output/native-package-corrupted.deb
```

Do not use corrupted adversarial fixtures as positive install proof. Slice A keeps install proof at the unit/transaction level. Slice D owns fresh RPM/DEB/Arch positive fixture creation and full CLI install proof.

- [ ] **Step 3: Run CLI tests**

Run:

```bash
cargo test -p conary --test native_pm_live_root -- --nocapture
```

Expected: tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/conary/tests/native_pm_live_root.rs
git commit -m "test(cli): prove no-generation live-root package remove"
```

## Task 9: Verify Spec Link And Run Final Verification

**Files:**
- Modify: `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`

- [ ] **Step 1: Verify Slice A evidence in the spec**

The spec should already say Slice A has an implementation plan and list the plan path:

```markdown
**Status:** Review-patched design direction with Slice A implementation plan
```

Add a short line under "Recommended Codex Goal Decomposition":

```markdown
Slice A plan: `docs/superpowers/plans/2026-05-14-native-package-manager-parity-slice-a-plan.md`.
```

If implementation changes the plan filename or materially changes the slice boundary, update this line in the same commit as the code/docs evidence.

- [ ] **Step 2: Run focused verification**

Run:

```bash
cargo fmt --check
cargo test -p conary changeset_metadata -- --nocapture
cargo test -p conary query::history -- --nocapture
cargo test -p conary live_root -- --nocapture
cargo test -p conary no_generation_install_transaction_materializes_live_root_file -- --nocapture
cargo test -p conary no_generation_remove_deletes_files_and_db_rows remove_refuses_critical_package_before_file_mutation -- --nocapture
cargo test -p conary --test native_pm_live_root -- --nocapture
```

Expected: all pass.

- [ ] **Step 3: Run workspace gates**

Run:

```bash
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md docs/superpowers/plans/2026-05-14-native-package-manager-parity-slice-a-plan.md
git commit -m "docs: plan native package manager parity slice a"
```

## Final Acceptance Checklist

- [ ] `conary install` no-generation path writes package files to the supplied live root and records DB/file-history state.
- [ ] `conary remove` no-generation path deletes package files from the supplied live root and removes DB rows.
- [ ] Active-generation install/remove still use generation-aware behavior.
- [ ] DB-only no-generation install success is gone.
- [ ] Remove no longer requires an active generation for Conary-owned packages.
- [ ] Remove refuses critical packages before mutation.
- [ ] Remove refuses adopted packages before mutation unless explicit purge is supplied.
- [ ] Deferred generation/state follow-up is stored in changeset metadata and visible in `system history`.
- [ ] Legacy rollback metadata parsing still supports single `TroveSnapshot` and `RevertMetadata`.
- [ ] Focused tests and workspace gates pass.

## Next Goal After This Plan

After Slice A is implemented and merged, launch Slice B:

```text
/goal Slice B: Implement Conary-owned update parity on top of Slice A. Ensure update works on no-generation live hosts, treats adopted packages as native-authoritative unless takeover is explicit, blocks critical takeover/remove/update cases, reports partial multi-package outcomes truthfully, and refuses or reports security-metadata-unavailable sources before mutation. Prove with unit, CLI, and distro tests.
```
