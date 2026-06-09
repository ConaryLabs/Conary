// conary-core/src/generation/builder/root_validation.rs

use std::collections::{HashMap, HashSet};

use super::{FileEntryRef, SymlinkEntryRef, hex_to_digest};
use crate::generation::metadata::ROOT_SYMLINKS;

pub(super) fn validate_runtime_generation_root_is_self_contained(
    file_refs: &[FileEntryRef],
    symlink_refs: &[SymlinkEntryRef],
) -> crate::Result<()> {
    if generation_root_has_init_entrypoint(file_refs, symlink_refs) {
        return Ok(());
    }

    Err(crate::error::Error::NotFound(
        "exportable runtime generation is not self-contained: missing executable /sbin/init in the CAS-backed generation root; refusing to scrape the live host root to make the image bootable".to_string(),
    ))
}

fn generation_root_has_init_entrypoint(
    file_refs: &[FileEntryRef],
    symlink_refs: &[SymlinkEntryRef],
) -> bool {
    let symlink_paths: HashSet<String> = symlink_refs
        .iter()
        .filter_map(|symlink| normalize_virtual_path(&symlink.path, "/"))
        .collect();
    let files: HashMap<String, u32> = file_refs
        .iter()
        .filter_map(|file| {
            let path = normalize_virtual_path(&file.path, "/")?;
            if symlink_paths.contains(&path) || hex_to_digest(&file.sha256_hash).is_err() {
                return None;
            }
            Some((path, file.permissions))
        })
        .collect();
    let symlinks = generation_symlink_map(symlink_refs);

    resolve_virtual_path("/sbin/init", &symlinks)
        .and_then(|resolved| files.get(&resolved).copied())
        .is_some_and(|permissions| permissions & 0o111 != 0)
}

fn generation_symlink_map(symlink_refs: &[SymlinkEntryRef]) -> HashMap<String, String> {
    let mut symlinks = HashMap::new();
    for symlink in symlink_refs {
        if let Some(path) = normalize_virtual_path(&symlink.path, "/") {
            symlinks.insert(path, symlink.target.clone());
        }
    }
    for (link, target) in ROOT_SYMLINKS {
        symlinks.insert(format!("/{link}"), (*target).to_string());
    }
    symlinks
}

fn resolve_virtual_path(path: &str, symlinks: &HashMap<String, String>) -> Option<String> {
    let mut current = normalize_virtual_path(path, "/")?;
    for _ in 0..40 {
        let Some(next) = rewrite_first_symlink_component(&current, symlinks) else {
            return Some(current);
        };
        current = next;
    }
    None
}

fn rewrite_first_symlink_component(
    path: &str,
    symlinks: &HashMap<String, String>,
) -> Option<String> {
    let components: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();

    for index in 0..components.len() {
        let prefix = format!("/{}", components[..=index].join("/"));
        let Some(target) = symlinks.get(&prefix) else {
            continue;
        };
        let base = parent_virtual_path(&prefix);
        let mut rewritten = normalize_virtual_path(target, &base)?;
        for component in &components[index + 1..] {
            if rewritten != "/" {
                rewritten.push('/');
            }
            rewritten.push_str(component);
        }
        return normalize_virtual_path(&rewritten, "/");
    }

    None
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
    use super::super::{FileEntryRef, SymlinkEntryRef};
    use super::*;

    #[test]
    fn runtime_root_init_detection_resolves_usr_merge_and_package_symlinks() {
        let file_refs = vec![FileEntryRef {
            path: "/usr/lib/systemd/systemd".to_string(),
            sha256_hash: "a".repeat(64),
            size: 6,
            permissions: 0o755,
            owner: None,
            group_name: None,
        }];
        let symlink_refs = vec![SymlinkEntryRef {
            path: "/usr/sbin/init".to_string(),
            target: "../lib/systemd/systemd".to_string(),
        }];

        assert!(generation_root_has_init_entrypoint(
            &file_refs,
            &symlink_refs
        ));
    }
}
