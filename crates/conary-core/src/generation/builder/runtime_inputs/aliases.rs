// conary-core/src/generation/builder/runtime_inputs/aliases.rs

//! Exact projection from package-invoked paths through manifest-owned aliases.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::generation::root_manifest::GenerationRootEntry;
use crate::payload::PayloadNodeKind;

/// Project nodes below manifest-owned symlinks at their materialized paths.
///
/// Native package databases retain invoked paths such as `/lib/modules` on a
/// usr-merged system. Package ownership must keep that spelling, while a
/// generation tree cannot contain children below its `/lib -> usr/lib`
/// symlink. Resolve only typed symlink nodes already present in the exact
/// payload authority, then coalesce aliases only when their complete node and
/// content authority agree.
pub(super) fn canonicalize_runtime_alias_paths(
    entries: Vec<GenerationRootEntry>,
) -> crate::Result<Vec<GenerationRootEntry>> {
    let symlinks = canonical_runtime_symlink_map(&entries)?;
    let mut canonical = BTreeMap::<String, (String, GenerationRootEntry)>::new();

    for mut entry in entries {
        let source_path = entry.path.clone();
        entry.path = resolve_runtime_entry_path(&entry.path, &symlinks)?;
        let hardlink_target = match &entry.node.source.kind {
            PayloadNodeKind::Hardlink { target, .. } => {
                Some(resolve_runtime_entry_path(target, &symlinks)?)
            }
            _ => None,
        };
        if let (
            Some(target),
            PayloadNodeKind::Hardlink {
                target: entry_target,
                ..
            },
        ) = (hardlink_target, &mut entry.node.source.kind)
        {
            *entry_target = target;
        }
        entry.validate().map_err(|error| {
            crate::Error::InvalidPath(format!(
                "runtime generation alias projection from {source_path} to {} is invalid: {error}",
                entry.path
            ))
        })?;

        match canonical.entry(entry.path.clone()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert((source_path, entry));
            }
            std::collections::btree_map::Entry::Occupied(slot) if slot.get().1 == entry => {}
            std::collections::btree_map::Entry::Occupied(slot) => {
                return Err(crate::Error::InvalidPath(format!(
                    "runtime generation aliases {} and {} resolve to {} with conflicting exact payload authority",
                    slot.get().0,
                    source_path,
                    entry.path
                )));
            }
        }
    }

    Ok(canonical
        .into_values()
        .map(|(_source_path, entry)| entry)
        .collect())
}

fn canonical_runtime_symlink_map(
    entries: &[GenerationRootEntry],
) -> crate::Result<HashMap<String, String>> {
    let raw = entries
        .iter()
        .filter_map(|entry| match &entry.node.source.kind {
            PayloadNodeKind::Symlink { target } => Some((entry.path.clone(), target.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut symlinks = raw.iter().cloned().collect::<HashMap<_, _>>();
    if symlinks.len() != raw.len() {
        return Err(crate::Error::InvalidPath(
            "runtime generation has duplicate symlink path authority".to_string(),
        ));
    }

    for _ in 0..40 {
        let mut next = HashMap::new();
        for (path, target) in &raw {
            let canonical_path = resolve_runtime_entry_path(path, &symlinks)?;
            if let Some(existing) = next.insert(canonical_path.clone(), target.clone())
                && existing != *target
            {
                return Err(crate::Error::InvalidPath(format!(
                    "runtime generation symlink aliases resolve to {canonical_path} with conflicting targets"
                )));
            }
        }
        if next == symlinks {
            return Ok(next);
        }
        symlinks = next;
    }

    Err(crate::Error::InvalidPath(
        "runtime generation symlink ancestry does not converge within 40 edges".to_string(),
    ))
}

/// Resolve symlinks in a node's ancestors without following the node itself.
fn resolve_runtime_entry_path(
    path: &str,
    symlinks: &HashMap<String, String>,
) -> crate::Result<String> {
    let parsed = Path::new(path);
    let leaf = parsed
        .file_name()
        .and_then(|leaf| leaf.to_str())
        .ok_or_else(|| {
            crate::Error::InvalidPath(format!("runtime generation path has no UTF-8 leaf: {path}"))
        })?;
    let parent = parsed.parent().and_then(Path::to_str).ok_or_else(|| {
        crate::Error::InvalidPath(format!(
            "runtime generation path has no UTF-8 parent: {path}"
        ))
    })?;
    let resolved_parent = super::super::root_validation::resolve_virtual_path(parent, symlinks)
        .map_err(|error| {
            crate::Error::InvalidPath(format!(
                "runtime generation cannot resolve exact manifest symlink ancestry for {path}: {error}"
            ))
        })?;
    if resolved_parent == "/" {
        Ok(format!("/{leaf}"))
    } else {
        Ok(format!("{resolved_parent}/{leaf}"))
    }
}
