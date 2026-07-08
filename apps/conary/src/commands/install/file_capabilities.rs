// apps/conary/src/commands/install/file_capabilities.rs

use anyhow::{Context, Result, bail};
use conary_core::ccs::manifest::FileCapability;
use std::collections::BTreeSet;
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
    installed_paths: impl IntoIterator<Item = &'a str>,
) -> Result<usize> {
    let mut applier = SetcapCommandFileCapabilityApplier;
    apply_selected_file_capabilities_with(root, capabilities, installed_paths, &mut applier)
}

pub(crate) fn apply_selected_file_capabilities_with<'a>(
    root: &Path,
    capabilities: &[FileCapability],
    installed_paths: impl IntoIterator<Item = &'a str>,
    applier: &mut impl FileCapabilityApplier,
) -> Result<usize> {
    let installed_paths = installed_paths.into_iter().collect::<BTreeSet<_>>();
    let mut applied = 0;
    for capability in capabilities {
        if !installed_paths.contains(capability.path.as_str()) {
            continue;
        }
        if !capability.path.starts_with('/') {
            bail!(
                "relative path not allowed in file_capabilities: {}",
                capability.path
            );
        }
        let target = crate::commands::live_root::target_path(root, &capability.path)?;
        capability.validate()?;
        if !target.exists() {
            bail!(
                "file capability target {} was selected but is missing after install",
                capability.path
            );
        }
        applier
            .apply_file_capability(&target, capability)
            .with_context(|| {
                format!("Failed to apply file capabilities for {}", capability.path)
            })?;
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn applies_selected_file_capabilities_inside_live_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr/bin")).unwrap();
        std::fs::write(root.path().join("usr/bin/demo"), b"demo").unwrap();
        let mut applier = RecordingApplier::default();

        let applied = apply_selected_file_capabilities_with(
            root.path(),
            &[
                file_capability("/usr/bin/demo"),
                file_capability("/usr/bin/other"),
            ],
            ["/usr/bin/demo"],
            &mut applier,
        )
        .expect("apply selected file capabilities");

        assert_eq!(applied, 1);
        assert_eq!(applier.calls.len(), 1);
        assert_eq!(applier.calls[0].0, root.path().join("usr/bin/demo"));
        assert_eq!(applier.calls[0].1, "cap_net_bind_service=+ep");
    }

    #[test]
    fn refuses_file_capability_paths_that_escape_live_root() {
        let root = tempfile::tempdir().unwrap();
        let mut applier = RecordingApplier::default();

        let err = apply_selected_file_capabilities_with(
            root.path(),
            &[file_capability("/usr/../demo")],
            ["/usr/../demo"],
            &mut applier,
        )
        .expect_err("escape path must fail before invoking setcap");

        assert!(err.to_string().contains("escapes the target root"));
        assert!(applier.calls.is_empty());
    }
}
