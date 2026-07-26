// conary-core/src/generation/builder/sysroot.rs

use std::path::Path;

use super::runtime_inputs;
use crate::filesystem::CasStore;
use crate::generation::root_manifest::{CapturedSelectedRoot, materialize_captured_selected_root};

pub(super) fn materialize_runtime_generation_sysroot(
    runtime_inputs: &runtime_inputs::RuntimeGenerationInputs,
    objects_dir: &Path,
    artifact_root: &Path,
) -> crate::Result<tempfile::TempDir> {
    let workspace = tempfile::Builder::new()
        .prefix(".generation-sysroot-workspace-")
        .tempdir_in(artifact_root)
        .map_err(|error| {
            crate::Error::IoError(format!(
                "failed to create temporary generation workspace under {}: {error}",
                artifact_root.display()
            ))
        })?;
    let sysroot_path = workspace.path();
    let cas = CasStore::new(objects_dir)?;
    materialize_captured_selected_root(
        &CapturedSelectedRoot {
            generation: runtime_inputs.generation.clone(),
            state: runtime_inputs.state.clone(),
        },
        &cas,
        sysroot_path,
    )?;
    materialize_runtime_sysroot_ephemeral_dirs(sysroot_path)?;
    Ok(workspace)
}

/// Add the finite runtime-only mountpoint/workspace directories needed by
/// initramfs tooling. These are not generation artifact authority.
fn materialize_runtime_sysroot_ephemeral_dirs(sysroot: &Path) -> crate::Result<()> {
    for directory in ["dev", "proc", "run", "sys", "tmp", "var", "var/tmp"] {
        std::fs::create_dir_all(sysroot.join(directory))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for directory in ["tmp", "var/tmp"] {
            std::fs::set_permissions(
                sysroot.join(directory),
                std::fs::Permissions::from_mode(0o1777),
            )?;
        }
    }
    Ok(())
}

pub(super) fn runtime_generation_architecture() -> crate::Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64"),
        "aarch64" => Err(crate::Error::NotImplemented(
            "aarch64 generation export boot assets are reserved but not implemented".to_string(),
        )),
        "riscv64" => Err(crate::Error::NotImplemented(
            "riscv64 generation export boot assets are reserved but not implemented".to_string(),
        )),
        other => Err(crate::Error::NotImplemented(format!(
            "unsupported runtime architecture for generation export: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::runtime_inputs::RuntimeGenerationInputs;
    use super::*;
    use crate::generation::root_manifest::{
        GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry, GenerationRootManifest,
        MutableStateManifest,
    };
    use crate::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
    };

    #[test]
    fn sysroot_materialization_uses_exact_manifest_without_synthetic_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let objects = temp.path().join("objects");
        let cas = CasStore::new(&objects).unwrap();
        let digest = cas.store(b"fixture").unwrap();
        let runtime_inputs = RuntimeGenerationInputs {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root: directory_node(0o755),
                entries: vec![
                    directory_entry("/usr"),
                    directory_entry("/usr/bin"),
                    GenerationRootEntry {
                        path: "/usr/bin/fixture".to_string(),
                        node: owned_regular_node(0o755),
                        content: Some(PayloadContentAuthority {
                            sha256: digest,
                            size: 7,
                        }),
                    },
                ],
            },
            state: MutableStateManifest::empty(),
            adopted_track_count: 0,
        };

        let workspace =
            materialize_runtime_generation_sysroot(&runtime_inputs, &objects, temp.path()).unwrap();
        let root = workspace.path();
        assert_eq!(
            std::fs::read(root.join("usr/bin/fixture")).unwrap(),
            b"fixture"
        );
        assert!(!root.join("bin").exists());
        assert!(root.join("tmp").is_dir());
        assert!(root.join("var/tmp").is_dir());
    }

    fn directory_entry(path: &str) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: directory_node(0o755),
            content: None,
        }
    }

    fn directory_node(permissions: u32) -> ResolvedPayloadNode {
        let mut node = owned_regular_source(permissions);
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | permissions;
        ResolvedPayloadNode::from_numeric_source(node).unwrap()
    }

    fn owned_regular_node(permissions: u32) -> ResolvedPayloadNode {
        ResolvedPayloadNode::from_numeric_source(owned_regular_source(permissions)).unwrap()
    }

    fn owned_regular_source(permissions: u32) -> PayloadNode {
        let mut node = PayloadNode::regular(permissions);
        node.user = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::geteuid() }),
        };
        node.group = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::getegid() }),
        };
        node
    }
}
