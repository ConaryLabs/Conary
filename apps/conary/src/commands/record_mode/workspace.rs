// apps/conary/src/commands/record_mode/workspace.rs

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::Builder;
use walkdir::WalkDir;

const ACTIVE_MARKER: &str = ".conary-record-active";
const KEEP_RAW_TRACE_MARKER: &str = ".conary-record-keep-raw-trace";

#[derive(Debug)]
pub(crate) struct RecordWorkspace {
    pub(crate) private_root: PathBuf,
    pub(crate) source_root: PathBuf,
    pub(crate) work_root: PathBuf,
    pub(crate) install_root: PathBuf,
    pub(crate) raw_trace_dir: PathBuf,
    pub(crate) output_dir: PathBuf,
    keep_raw_trace: bool,
}

impl RecordWorkspace {
    pub(crate) fn create(source: &Path, output_dir: &Path, keep_raw_trace: bool) -> Result<Self> {
        let source = source
            .canonicalize()
            .with_context(|| format!("failed to canonicalize source {}", source.display()))?;
        let private_temp = Builder::new().prefix("conary-record-").tempdir()?;
        let private_root = private_temp.keep();
        fs::set_permissions(&private_root, fs::Permissions::from_mode(0o700))?;
        fs::write(private_root.join(ACTIVE_MARKER), b"active")?;

        let workspace = Self {
            source_root: private_root.join("source"),
            work_root: private_root.join("work"),
            install_root: private_root.join("destdir"),
            raw_trace_dir: private_root.join("raw-trace"),
            output_dir: output_dir.to_path_buf(),
            private_root,
            keep_raw_trace,
        };
        fs::create_dir_all(&workspace.source_root)?;
        fs::create_dir_all(&workspace.work_root)?;
        fs::create_dir_all(&workspace.install_root)?;
        fs::create_dir_all(&workspace.raw_trace_dir)?;
        copy_tree(&source, &workspace.source_root)?;
        Ok(workspace)
    }

    pub(crate) fn publish_source_snapshot(&self) -> Result<()> {
        let public_source = self.output_dir.join("source");
        if public_source.exists() {
            fs::remove_dir_all(&public_source)?;
        }
        fs::create_dir_all(&self.output_dir)?;
        copy_tree(&self.source_root, &public_source)
    }

    pub(crate) fn cleanup(self) -> Result<()> {
        if self.keep_raw_trace {
            for path in [&self.source_root, &self.work_root, &self.install_root] {
                if path.exists() {
                    fs::remove_dir_all(path)?;
                }
            }
            let active_marker = self.private_root.join(ACTIVE_MARKER);
            if active_marker.exists() {
                fs::remove_file(active_marker)?;
            }
            fs::write(
                self.private_root.join(KEEP_RAW_TRACE_MARKER),
                b"keep-raw-trace",
            )?;
            return Ok(());
        }
        if self.private_root.exists() {
            fs::remove_dir_all(&self.private_root)?;
        }
        Ok(())
    }
}

pub(crate) fn cleanup_stale_workspaces(parent: &Path) -> Result<usize> {
    let mut removed = 0;
    if !parent.is_dir() {
        return Ok(0);
    }
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with("conary-record-")
            && path.is_dir()
            && !path.join(ACTIVE_MARKER).exists()
            && !path.join(KEEP_RAW_TRACE_MARKER).exists()
        {
            fs::remove_dir_all(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        } else if entry.file_type().is_symlink() {
            #[cfg(unix)]
            {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let link_target = fs::read_link(entry.path())?;
                std::os::unix::fs::symlink(link_target, &target)?;
            }
            #[cfg(not(unix))]
            anyhow::bail!("record-mode source snapshots require Unix symlink support");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn workspace_uses_private_permissions_and_public_source_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let output = temp.path().join("recorded/demo");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("main.c"), "int main(void){return 0;}\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("main.c", source.join("main-link.c")).unwrap();

        let workspace = RecordWorkspace::create(&source, &output, false).unwrap();
        let mode = std::fs::metadata(&workspace.private_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        assert!(workspace.source_root.join("main.c").is_file());

        workspace.publish_source_snapshot().unwrap();
        assert!(output.join("source/main.c").is_file());
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(output.join("source/main-link.c")).unwrap(),
            std::path::PathBuf::from("main.c")
        );
        assert!(!output.join("raw-trace").exists());
    }

    #[test]
    fn cleanup_removes_raw_trace_when_not_kept() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let output = temp.path().join("recorded/demo");
        std::fs::create_dir_all(&source).unwrap();

        let workspace = RecordWorkspace::create(&source, &output, false).unwrap();
        std::fs::write(workspace.raw_trace_dir.join("events.jsonl"), "secret").unwrap();
        let private_root = workspace.private_root.clone();
        workspace.cleanup().unwrap();

        assert!(!private_root.exists());
    }

    #[test]
    fn keep_raw_trace_preserves_private_trace_dir_only() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let output = temp.path().join("recorded/demo");
        std::fs::create_dir_all(&source).unwrap();

        let workspace = RecordWorkspace::create(&source, &output, true).unwrap();
        std::fs::write(workspace.raw_trace_dir.join("events.jsonl"), "secret").unwrap();
        let raw_trace_dir = workspace.raw_trace_dir.clone();
        let source_root = workspace.source_root.clone();
        let work_root = workspace.work_root.clone();
        let install_root = workspace.install_root.clone();
        workspace.cleanup().unwrap();

        assert!(raw_trace_dir.exists());
        assert!(!source_root.exists());
        assert!(!work_root.exists());
        assert!(!install_root.exists());
        assert!(!output.join("raw-trace").exists());
    }

    #[test]
    fn stale_cleanup_only_removes_record_prefixes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("conary-record-old")).unwrap();
        std::fs::create_dir_all(temp.path().join("unrelated")).unwrap();

        assert_eq!(cleanup_stale_workspaces(temp.path()).unwrap(), 1);
        assert!(!temp.path().join("conary-record-old").exists());
        assert!(temp.path().join("unrelated").exists());
    }

    #[test]
    fn stale_cleanup_skips_active_and_kept_raw_trace_workspaces() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("conary-record-active");
        let kept = temp.path().join("conary-record-kept");
        std::fs::create_dir_all(&active).unwrap();
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(active.join(ACTIVE_MARKER), "active").unwrap();
        std::fs::write(kept.join(KEEP_RAW_TRACE_MARKER), "keep").unwrap();

        assert_eq!(cleanup_stale_workspaces(temp.path()).unwrap(), 0);
        assert!(active.exists());
        assert!(kept.exists());
    }
}
