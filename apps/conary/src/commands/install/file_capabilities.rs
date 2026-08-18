// apps/conary/src/commands/install/file_capabilities.rs

use crate::commands::LiveRootFile;
use anyhow::{Context, Result, bail};
use conary_core::ccs::manifest::FileCapability;
use conary_core::payload::PayloadNodeKind;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

pub(crate) trait FileCapabilityApplier {
    fn apply_file_capability(&mut self, target: &Path, capability: &FileCapability) -> Result<()>;
}

pub(crate) struct SetcapCommandFileCapabilityApplier;

impl FileCapabilityApplier for SetcapCommandFileCapabilityApplier {
    fn apply_file_capability(&mut self, target: &Path, capability: &FileCapability) -> Result<()> {
        let spec = capability.to_setcap_spec()?;
        let status = Command::new("setcap")
            .arg(&spec)
            .arg(target)
            .status()
            .with_context(|| format!("Failed to execute setcap for {}", target.display()))?;
        if !status.success() {
            bail!(
                "setcap {} {} failed with status {}",
                spec,
                target.display(),
                status
            );
        }
        Ok(())
    }
}

pub(crate) fn apply_selected_file_capabilities<'a>(
    root: &Path,
    capabilities: &[FileCapability],
    installed_files: impl IntoIterator<Item = &'a LiveRootFile>,
) -> Result<usize> {
    let mut applier = SetcapCommandFileCapabilityApplier;
    apply_selected_file_capabilities_with(root, capabilities, installed_files, &mut applier)
}

pub(crate) fn apply_selected_file_capabilities_with<'a>(
    root: &Path,
    capabilities: &[FileCapability],
    installed_files: impl IntoIterator<Item = &'a LiveRootFile>,
    applier: &mut impl FileCapabilityApplier,
) -> Result<usize> {
    let installed_files = installed_files
        .into_iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut applied = 0;
    for capability in capabilities {
        let Some(installed_file) = installed_files.get(capability.path.as_str()) else {
            continue;
        };
        if !capability.path.starts_with('/') {
            bail!(
                "relative path not allowed in file_capabilities: {}",
                capability.path
            );
        }
        if !is_regular_file_capability_payload(&installed_file.node.source.kind) {
            bail!(
                "file capability target {} is not a regular installed file",
                capability.path
            );
        }
        let target = crate::commands::live_root::target_path(root, &capability.path)?;
        capability.validate()?;
        ensure_live_root_regular_file_target(&capability.path, &target)?;
        applier
            .apply_file_capability(&target, capability)
            .with_context(|| {
                format!("Failed to apply file capabilities for {}", capability.path)
            })?;
        applied += 1;
    }
    Ok(applied)
}

pub(in crate::commands::install) fn is_regular_file_capability_payload(
    kind: &PayloadNodeKind,
) -> bool {
    matches!(kind, PayloadNodeKind::Regular { .. })
}

fn ensure_live_root_regular_file_target(package_path: &str, target: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(target).with_context(|| {
        format!("file capability target {package_path} was selected but is missing after install")
    })?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_file() {
        bail!("file capability target {package_path} is not a regular installed file");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::payload::{PayloadNode, ResolvedPayloadNode};
    use std::path::PathBuf;

    #[derive(Default)]
    struct RecordingApplier {
        calls: Vec<(PathBuf, String)>,
    }

    impl FileCapabilityApplier for RecordingApplier {
        fn apply_file_capability(
            &mut self,
            target: &Path,
            capability: &FileCapability,
        ) -> anyhow::Result<()> {
            self.calls
                .push((target.to_path_buf(), capability.to_setcap_spec()?));
            Ok(())
        }
    }

    fn file_capability(path: &str) -> FileCapability {
        FileCapability {
            path: path.to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        }
    }

    fn live_root_file(path: &str) -> LiveRootFile {
        LiveRootFile {
            path: path.to_string(),
            content: crate::commands::LiveRootContent::from_in_memory_bytes(b""),
            node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
        }
    }

    fn live_root_symlink(path: &str, target: &str) -> LiveRootFile {
        let mut node = PayloadNode::regular(0o777);
        node.kind = PayloadNodeKind::Symlink {
            target: target.to_string(),
        };
        node.mode = libc::S_IFLNK | 0o777;
        LiveRootFile {
            path: path.to_string(),
            content: crate::commands::LiveRootContent::absent(),
            node: ResolvedPayloadNode::from_numeric_source(node).unwrap(),
        }
    }

    #[test]
    fn applies_selected_file_capabilities_inside_live_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr/bin")).unwrap();
        std::fs::write(root.path().join("usr/bin/demo"), b"demo").unwrap();
        let mut applier = RecordingApplier::default();
        let installed_files = [live_root_file("/usr/bin/demo")];

        let applied = apply_selected_file_capabilities_with(
            root.path(),
            &[
                file_capability("/usr/bin/demo"),
                file_capability("/usr/bin/other"),
            ],
            installed_files.iter(),
            &mut applier,
        )
        .expect("apply selected file capabilities");

        assert_eq!(applied, 1);
        assert_eq!(applier.calls.len(), 1);
        assert_eq!(applier.calls[0].0, root.path().join("usr/bin/demo"));
        assert_eq!(applier.calls[0].1, "cap_net_bind_service=ep");
    }

    #[test]
    fn refuses_file_capability_paths_that_escape_live_root() {
        let root = tempfile::tempdir().unwrap();
        let mut applier = RecordingApplier::default();
        let installed_files = [live_root_file("/usr/../demo")];

        let err = apply_selected_file_capabilities_with(
            root.path(),
            &[file_capability("/usr/../demo")],
            installed_files.iter(),
            &mut applier,
        )
        .expect_err("escape path must fail before invoking setcap");

        assert!(err.to_string().contains("escapes the target root"));
        assert!(applier.calls.is_empty());
    }

    #[test]
    fn refuses_selected_file_capability_symlink_targets() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr/bin")).unwrap();
        std::fs::write(root.path().join("usr/bin/server"), b"server").unwrap();
        std::os::unix::fs::symlink("server", root.path().join("usr/bin/server-link")).unwrap();
        let mut applier = RecordingApplier::default();
        let installed_files = [live_root_symlink("/usr/bin/server-link", "server")];

        let err = apply_selected_file_capabilities_with(
            root.path(),
            &[file_capability("/usr/bin/server-link")],
            installed_files.iter(),
            &mut applier,
        )
        .expect_err("symlink file capability target must fail before invoking setcap");

        assert!(err.to_string().contains("is not a regular installed file"));
        assert!(applier.calls.is_empty());
    }
}
