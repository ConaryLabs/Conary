// apps/conary/src/commands/generation/selected_root/deferred_ima.rs

//! Typed carry-through for IMA signatures unavailable on staging filesystems.

use crate::commands::LiveRootFile;
use anyhow::{Context, Result, bail};
use conary_core::generation::root_manifest::{CapturedSelectedRoot, GenerationRootEntry};
use conary_core::payload::{PayloadContentAuthority, PayloadNodeKind};
use std::collections::BTreeMap;

const SECURITY_IMA_XATTR: &str = "security.ima";

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeferredImaSignature {
    value: Vec<u8>,
    content: PayloadContentAuthority,
}

/// IMA signatures remain typed package and generation authority when an
/// intermediate selected-root filesystem cannot apply `security.ima`.
///
/// The signature is restored into a captured manifest only when the selected
/// root still has the exact signed content digest and size. Lifecycle writes
/// therefore invalidate deferred signatures without relying on path or
/// package-name heuristics.
#[derive(Debug, Default)]
pub(super) struct DeferredImaAuthority {
    signatures: BTreeMap<String, DeferredImaSignature>,
    content_by_path: BTreeMap<String, PayloadContentAuthority>,
}

impl DeferredImaAuthority {
    pub(super) fn from_captured(captured: &CapturedSelectedRoot) -> Result<Self> {
        captured.generation.validate()?;
        captured.state.validate()?;
        let entries = captured
            .generation
            .entries
            .iter()
            .chain(&captured.state.entries)
            .collect::<Vec<_>>();
        let mut authority = Self::default();
        authority.record_entries(entries)?;
        Ok(authority)
    }

    pub(super) fn record_overlay(&mut self, files: &[LiveRootFile]) -> Result<()> {
        let entries = files
            .iter()
            .map(|file| GenerationRootEntry {
                path: file.path.clone(),
                node: file.node.clone(),
                content: file.content.authority().cloned(),
            })
            .collect::<Vec<_>>();
        self.record_entries(entries.iter().collect())
    }

    pub(super) fn remove_paths(&mut self, paths: &[String]) {
        for path in paths {
            remove_path_and_descendants(&mut self.signatures, path);
            remove_path_and_descendants(&mut self.content_by_path, path);
        }
    }

    pub(super) fn restore_into(&self, captured: &mut CapturedSelectedRoot) -> Result<usize> {
        captured.generation.validate()?;
        captured.state.validate()?;
        let actual_content = content_by_path(
            captured
                .generation
                .entries
                .iter()
                .chain(&captured.state.entries),
        )?;
        let mut restored = 0;
        for entry in captured
            .generation
            .entries
            .iter_mut()
            .chain(&mut captured.state.entries)
        {
            let Some(signature) = self.signatures.get(&entry.path) else {
                continue;
            };
            if actual_content.get(&entry.path) != Some(&signature.content)
                || entry.node.source.xattrs.contains_key(SECURITY_IMA_XATTR)
            {
                continue;
            }
            entry
                .node
                .source
                .xattrs
                .insert(SECURITY_IMA_XATTR.to_string(), signature.value.clone());
            restored += 1;
        }
        captured.generation.validate()?;
        captured.state.validate()?;
        Ok(restored)
    }

    fn record_entries(&mut self, entries: Vec<&GenerationRootEntry>) -> Result<()> {
        for entry in &entries {
            self.signatures.remove(&entry.path);
            self.content_by_path.remove(&entry.path);
        }

        for entry in &entries {
            if matches!(entry.node.source.kind, PayloadNodeKind::Regular { .. }) {
                let content = entry.content.clone().with_context(|| {
                    format!(
                        "deferred IMA regular path {} has no content authority",
                        entry.path
                    )
                })?;
                self.content_by_path.insert(entry.path.clone(), content);
            }
        }
        for entry in &entries {
            if let PayloadNodeKind::Hardlink { target, .. } = &entry.node.source.kind {
                let content = self.content_by_path.get(target).cloned().with_context(|| {
                    format!(
                        "deferred IMA hardlink {} targets unavailable content {target}",
                        entry.path
                    )
                })?;
                self.content_by_path.insert(entry.path.clone(), content);
            }
        }

        for entry in entries {
            let Some(value) = entry.node.source.xattrs.get(SECURITY_IMA_XATTR) else {
                continue;
            };
            let content = self
                .content_by_path
                .get(&entry.path)
                .cloned()
                .with_context(|| {
                    format!(
                        "security.ima authority at {} is not attached to regular content",
                        entry.path
                    )
                })?;
            if value.is_empty() {
                bail!("security.ima authority at {} is empty", entry.path);
            }
            self.signatures.insert(
                entry.path.clone(),
                DeferredImaSignature {
                    value: value.clone(),
                    content,
                },
            );
        }
        Ok(())
    }
}

fn content_by_path<'a>(
    entries: impl Iterator<Item = &'a GenerationRootEntry>,
) -> Result<BTreeMap<String, PayloadContentAuthority>> {
    let entries = entries.collect::<Vec<_>>();
    let mut content = BTreeMap::new();
    for entry in &entries {
        if matches!(entry.node.source.kind, PayloadNodeKind::Regular { .. }) {
            content.insert(
                entry.path.clone(),
                entry.content.clone().with_context(|| {
                    format!(
                        "captured regular path {} has no content authority",
                        entry.path
                    )
                })?,
            );
        }
    }
    for entry in entries {
        if let PayloadNodeKind::Hardlink { target, .. } = &entry.node.source.kind {
            let target_content = content.get(target).cloned().with_context(|| {
                format!(
                    "captured hardlink {} targets unavailable content {target}",
                    entry.path
                )
            })?;
            content.insert(entry.path.clone(), target_content);
        }
    }
    Ok(content)
}

fn remove_path_and_descendants<T>(map: &mut BTreeMap<String, T>, path: &str) {
    let descendant_prefix = format!("{}/", path.trim_end_matches('/'));
    map.retain(|candidate, _| candidate != path && !candidate.starts_with(&descendant_prefix));
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::generation::root_manifest::{
        GENERATION_ROOT_MANIFEST_VERSION, GenerationRootManifest, MutableStateManifest,
    };
    use conary_core::hash::sha256;
    use conary_core::payload::{PayloadIdentity, PayloadNode, ResolvedPayloadNode};

    #[test]
    fn exact_content_restores_deferred_ima_into_captured_authority() {
        let source = signed_root(b"signed");
        let authority = DeferredImaAuthority::from_captured(&source).unwrap();
        let mut captured = source.clone();
        file_mut(&mut captured)
            .node
            .source
            .xattrs
            .remove(SECURITY_IMA_XATTR);

        assert_eq!(authority.restore_into(&mut captured).unwrap(), 1);
        assert_eq!(captured, source);
    }

    #[test]
    fn changed_content_invalidates_deferred_ima() {
        let source = signed_root(b"signed");
        let authority = DeferredImaAuthority::from_captured(&source).unwrap();
        let mut captured = source.clone();
        let file = file_mut(&mut captured);
        file.node.source.xattrs.remove(SECURITY_IMA_XATTR);
        file.content = Some(content(b"changed"));

        assert_eq!(authority.restore_into(&mut captured).unwrap(), 0);
        assert!(
            !file_ref(&captured)
                .node
                .source
                .xattrs
                .contains_key(SECURITY_IMA_XATTR)
        );
    }

    #[test]
    fn unsigned_overlay_replaces_prior_signature_authority() {
        let source = signed_root(b"signed");
        let mut authority = DeferredImaAuthority::from_captured(&source).unwrap();
        let unsigned = LiveRootFile {
            path: "/usr/bin/tool".to_string(),
            content: crate::commands::LiveRootContent::from_in_memory_bytes(b"signed"),
            node: regular_node(BTreeMap::new()),
        };
        authority.record_overlay(&[unsigned]).unwrap();
        let mut captured = source;
        file_mut(&mut captured)
            .node
            .source
            .xattrs
            .remove(SECURITY_IMA_XATTR);

        assert_eq!(authority.restore_into(&mut captured).unwrap(), 0);
    }

    #[test]
    fn hardlink_group_restores_one_signed_content_authority() {
        let mut source = signed_root(b"signed");
        let primary = file_mut(&mut source);
        primary.node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some("signed-inode".to_string()),
        };
        let mut linked_node = primary.node.clone();
        linked_node.source.kind = PayloadNodeKind::Hardlink {
            target: "/usr/bin/tool".to_string(),
            identity: "signed-inode".to_string(),
        };
        source.generation.entries.push(GenerationRootEntry {
            path: "/usr/bin/tool-link".to_string(),
            node: linked_node,
            content: None,
        });
        source.generation.validate().unwrap();

        let authority = DeferredImaAuthority::from_captured(&source).unwrap();
        let mut captured = source.clone();
        for entry in captured
            .generation
            .entries
            .iter_mut()
            .filter(|entry| entry.path.starts_with("/usr/bin/tool"))
        {
            entry.node.source.xattrs.remove(SECURITY_IMA_XATTR);
        }

        assert_eq!(authority.restore_into(&mut captured).unwrap(), 2);
        assert_eq!(captured, source);
    }

    fn signed_root(bytes: &[u8]) -> CapturedSelectedRoot {
        let mut xattrs = BTreeMap::new();
        xattrs.insert(SECURITY_IMA_XATTR.to_string(), b"signature".to_vec());
        CapturedSelectedRoot {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root: directory_node(),
                entries: vec![
                    directory_entry("/usr"),
                    directory_entry("/usr/bin"),
                    GenerationRootEntry {
                        path: "/usr/bin/tool".to_string(),
                        node: regular_node(xattrs),
                        content: Some(content(bytes)),
                    },
                ],
            },
            state: MutableStateManifest::empty(),
        }
    }

    fn file_ref(captured: &CapturedSelectedRoot) -> &GenerationRootEntry {
        captured
            .generation
            .entries
            .iter()
            .find(|entry| entry.path == "/usr/bin/tool")
            .unwrap()
    }

    fn file_mut(captured: &mut CapturedSelectedRoot) -> &mut GenerationRootEntry {
        captured
            .generation
            .entries
            .iter_mut()
            .find(|entry| entry.path == "/usr/bin/tool")
            .unwrap()
    }

    fn directory_entry(path: &str) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: directory_node(),
            content: None,
        }
    }

    fn directory_node() -> conary_core::payload::ResolvedPayloadNode {
        let mut node = PayloadNode::regular(0o755);
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        ResolvedPayloadNode::from_numeric_source(node).unwrap()
    }

    fn regular_node(
        xattrs: BTreeMap<String, Vec<u8>>,
    ) -> conary_core::payload::ResolvedPayloadNode {
        let mut node = PayloadNode::regular(0o755);
        node.user = PayloadIdentity::Numeric { id: 0 };
        node.group = PayloadIdentity::Numeric { id: 0 };
        node.xattrs = xattrs;
        ResolvedPayloadNode::from_numeric_source(node).unwrap()
    }

    fn content(bytes: &[u8]) -> PayloadContentAuthority {
        PayloadContentAuthority {
            sha256: sha256(bytes),
            size: bytes.len() as u64,
        }
    }
}
