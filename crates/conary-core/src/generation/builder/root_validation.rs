// conary-core/src/generation/builder/root_validation.rs

use std::collections::{HashMap, HashSet};

use crate::generation::root_manifest::GenerationRootManifest;
use crate::payload::PayloadNodeKind;

pub(super) fn validate_runtime_generation_root_is_self_contained(
    manifest: &GenerationRootManifest,
) -> crate::Result<()> {
    manifest.validate()?;
    if generation_root_has_init_entrypoint(manifest) {
        return Ok(());
    }

    Err(crate::Error::NotFound(
        "runtime generation is not self-contained: its exact root manifest has no executable /sbin/init entrypoint"
            .to_string(),
    ))
}

fn generation_root_has_init_entrypoint(manifest: &GenerationRootManifest) -> bool {
    let symlinks = manifest
        .entries
        .iter()
        .filter_map(|entry| match &entry.node.source.kind {
            PayloadNodeKind::Symlink { target } => Some((entry.path.clone(), target.clone())),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let executable_paths = manifest
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.node.source.kind,
                PayloadNodeKind::Regular { .. } | PayloadNodeKind::Hardlink { .. }
            ) && entry.node.source.mode & 0o111 != 0
        })
        .map(|entry| entry.path.clone())
        .collect::<HashSet<_>>();

    resolve_virtual_path("/sbin/init", &symlinks)
        .is_ok_and(|resolved| executable_paths.contains(&resolved))
}

pub(super) fn resolve_virtual_path(
    path: &str,
    symlinks: &HashMap<String, String>,
) -> crate::Result<String> {
    let mut current = normalize_virtual_path(path, "/").ok_or_else(|| {
        crate::Error::InvalidPath(format!("virtual path escapes the generation root: {path}"))
    })?;
    for _ in 0..40 {
        let Some(next) = rewrite_first_symlink_component(&current, symlinks)? else {
            return Ok(current);
        };
        current = next;
    }
    Err(crate::Error::InvalidPath(format!(
        "virtual path exceeds 40 manifest symlink edges: {path}"
    )))
}

fn rewrite_first_symlink_component(
    path: &str,
    symlinks: &HashMap<String, String>,
) -> crate::Result<Option<String>> {
    let components = path
        .trim_start_matches('/')
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    for index in 0..components.len() {
        let prefix = format!("/{}", components[..=index].join("/"));
        let Some(target) = symlinks.get(&prefix) else {
            continue;
        };
        let base = parent_virtual_path(&prefix);
        let mut rewritten = normalize_virtual_path(target, &base).ok_or_else(|| {
            crate::Error::InvalidPath(format!(
                "manifest symlink {prefix} target {target:?} escapes the generation root"
            ))
        })?;
        for component in &components[index + 1..] {
            if rewritten != "/" {
                rewritten.push('/');
            }
            rewritten.push_str(component);
        }
        return normalize_virtual_path(&rewritten, "/")
            .map(Some)
            .ok_or_else(|| {
                crate::Error::InvalidPath(format!(
                    "manifest symlink {prefix} rewrites {path} outside the generation root"
                ))
            });
    }
    Ok(None)
}

fn normalize_virtual_path(path: &str, base: &str) -> Option<String> {
    let combined = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), path)
    };
    let mut components = Vec::new();
    for component in combined.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            component => components.push(component),
        }
    }
    Some(format!("/{}", components.join("/")))
}

fn parent_virtual_path(path: &str) -> String {
    let path = path.trim_end_matches('/');
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::root_manifest::{GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry};
    use crate::payload::{
        PayloadContentAuthority, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
    };

    fn resolved(node: PayloadNode) -> ResolvedPayloadNode {
        ResolvedPayloadNode::from_numeric_source(node).unwrap()
    }

    #[test]
    fn init_detection_resolves_only_manifest_owned_symlinks() {
        let manifest = manifest(vec![
            directory("/usr"),
            directory("/usr/lib"),
            directory("/usr/lib/systemd"),
            regular("/usr/lib/systemd/systemd", 0o755),
            directory("/usr/sbin"),
            symlink("/usr/sbin/init", "../lib/systemd/systemd"),
            symlink("/sbin", "usr/sbin"),
        ]);
        assert!(generation_root_has_init_entrypoint(&manifest));
    }

    #[test]
    fn init_detection_does_not_synthesize_usr_merge_links() {
        let manifest = manifest(vec![
            directory("/usr"),
            directory("/usr/sbin"),
            regular("/usr/sbin/init", 0o755),
        ]);
        assert!(!generation_root_has_init_entrypoint(&manifest));
    }

    #[test]
    fn virtual_path_resolution_rejects_escape_and_loops() {
        let escape = HashMap::from([("/lib".to_string(), "../../outside".to_string())]);
        let error = resolve_virtual_path("/lib/modules", &escape).unwrap_err();
        assert!(
            error.to_string().contains("escapes the generation root"),
            "{error}"
        );

        let cycle = HashMap::from([
            ("/first".to_string(), "/second".to_string()),
            ("/second".to_string(), "/first".to_string()),
        ]);
        let error = resolve_virtual_path("/first/tool", &cycle).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("exceeds 40 manifest symlink edges"),
            "{error}"
        );
    }

    fn manifest(entries: Vec<GenerationRootEntry>) -> GenerationRootManifest {
        let mut root = PayloadNode::regular(0o755);
        root.kind = PayloadNodeKind::Directory;
        root.mode = libc::S_IFDIR | 0o755;
        GenerationRootManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            root: resolved(root),
            entries,
        }
    }

    fn directory(path: &str) -> GenerationRootEntry {
        let mut node = PayloadNode::regular(0o755);
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        GenerationRootEntry {
            path: path.to_string(),
            node: resolved(node),
            content: None,
        }
    }

    fn regular(path: &str, permissions: u32) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: resolved(PayloadNode::regular(permissions)),
            content: Some(PayloadContentAuthority {
                sha256: "a".repeat(64),
                size: 1,
            }),
        }
    }

    fn symlink(path: &str, target: &str) -> GenerationRootEntry {
        let mut node = PayloadNode::regular(0o777);
        node.kind = PayloadNodeKind::Symlink {
            target: target.to_string(),
        };
        node.mode = libc::S_IFLNK | 0o777;
        GenerationRootEntry {
            path: path.to_string(),
            node: resolved(node),
            content: None,
        }
    }
}
