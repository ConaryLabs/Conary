// conary-core/src/derivation/compose.rs

//! Exact generation-root composition from derivation outputs.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::derivation::output::OutputManifest;
use crate::generation::builder::BuildResult;
use crate::generation::root_manifest::{
    CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry,
    GenerationRootManifest, MutableStateManifest, RootPathDomain,
    build_erofs_image_from_root_manifest, classify_root_path,
};
use crate::hash::{HashAlgorithm, hash_reader};
use crate::payload::ResolvedPayloadNode;

/// Errors that can occur during exact root composition.
#[derive(Debug, thiserror::Error)]
pub enum ComposeError {
    #[error("empty composition: no package outputs to compose")]
    EmptyComposition,
    #[error("invalid output manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid composed root: {0}")]
    InvalidComposition(String),
    #[error("EROFS build error: {0}")]
    Erofs(String),
    #[error("I/O error: {0}")]
    Io(String),
}

/// Compose exact output entries with deterministic last-writer-wins semantics.
///
/// Paths must already be normalized absolute paths. No mode, identity, xattr,
/// timestamp, node kind, link target, or content authority is synthesized.
pub fn compose_entries(
    manifests: &[&OutputManifest],
) -> Result<Vec<GenerationRootEntry>, ComposeError> {
    if manifests.is_empty() {
        return Err(ComposeError::EmptyComposition);
    }

    let mut merged = BTreeMap::<String, GenerationRootEntry>::new();
    for manifest in manifests {
        manifest
            .validate()
            .map_err(|error| ComposeError::InvalidManifest(error.to_string()))?;
        for entry in &manifest.entries {
            if let Some(previous) = merged.get(&entry.path)
                && std::mem::discriminant(&previous.node.source.kind)
                    != std::mem::discriminant(&entry.node.source.kind)
            {
                tracing::warn!(
                    path = %entry.path,
                    previous_kind = ?previous.node.source.kind,
                    replacement_kind = ?entry.node.source.kind,
                    "derivation composition replaced a path with a different node kind"
                );
            }
            merged.insert(entry.path.clone(), entry.clone());
        }
    }
    Ok(merged.into_values().collect())
}

/// Compose current derivation outputs into exact immutable and mutable-state
/// root manifests.
///
/// Ephemeral mount and user-owned domains are deliberately absent from a
/// persistent generation. Their classification is the typed
/// [`RootPathDomain`] contract, not a filename heuristic.
pub fn compose_selected_root(
    manifests: &[&OutputManifest],
    root: ResolvedPayloadNode,
) -> Result<CapturedSelectedRoot, ComposeError> {
    let entries = compose_entries(manifests)?;
    let mut immutable = Vec::new();
    let mut state = Vec::new();
    for entry in entries {
        match classify_root_path(&entry.path)
            .map_err(|error| ComposeError::InvalidComposition(error.to_string()))?
        {
            RootPathDomain::Immutable => immutable.push(entry),
            RootPathDomain::ConfigState | RootPathDomain::MutableState => state.push(entry),
            RootPathDomain::EphemeralMountOrUser => {}
        }
    }

    let captured = CapturedSelectedRoot {
        generation: GenerationRootManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            root,
            entries: immutable,
        },
        state: MutableStateManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            entries: state,
        },
    };
    captured
        .generation
        .validate()
        .map_err(|error| ComposeError::InvalidComposition(error.to_string()))?;
    captured
        .state
        .validate()
        .map_err(|error| ComposeError::InvalidComposition(error.to_string()))?;
    Ok(captured)
}

/// Compose only the exact immutable generation-root manifest.
pub fn compose_generation_root(
    manifests: &[&OutputManifest],
    root: ResolvedPayloadNode,
) -> Result<GenerationRootManifest, ComposeError> {
    Ok(compose_selected_root(manifests, root)?.generation)
}

/// Compose multiple package outputs into an exact EROFS generation.
///
/// The immutable manifest is the direct EROFS input. The mutable-state
/// manifest is persisted beside it for later exact reconstruction.
pub fn compose_erofs(
    manifests: &[&OutputManifest],
    root: ResolvedPayloadNode,
    output_dir: &Path,
) -> Result<BuildResult, ComposeError> {
    let captured = compose_selected_root(manifests, root)?;
    let build = build_erofs_image_from_root_manifest(&captured.generation, output_dir)
        .map_err(|error| ComposeError::Erofs(error.to_string()))?;
    captured
        .state
        .write_to(output_dir)
        .map_err(|error| ComposeError::Io(error.to_string()))?;
    Ok(build)
}

/// Compute the SHA-256 hash of an EROFS image file.
pub fn erofs_image_hash(image_path: &Path) -> Result<String, ComposeError> {
    let file = File::open(image_path)
        .map_err(|error| ComposeError::Io(format!("{}: {error}", image_path.display())))?;
    let mut reader = BufReader::new(file);
    let hash = hash_reader(HashAlgorithm::Sha256, &mut reader)
        .map_err(|error| ComposeError::Io(format!("{}: {error}", image_path.display())))?;
    Ok(hash.value)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    };

    fn node(kind: PayloadNodeKind, permissions: u32) -> ResolvedPayloadNode {
        let mode_type = match kind {
            PayloadNodeKind::Regular { .. } | PayloadNodeKind::Hardlink { .. } => libc::S_IFREG,
            PayloadNodeKind::Directory => libc::S_IFDIR,
            PayloadNodeKind::Symlink { .. } => libc::S_IFLNK,
            PayloadNodeKind::BlockDevice { .. } => libc::S_IFBLK,
            PayloadNodeKind::CharacterDevice { .. } => libc::S_IFCHR,
            PayloadNodeKind::Fifo => libc::S_IFIFO,
            PayloadNodeKind::Socket => libc::S_IFSOCK,
        };
        ResolvedPayloadNode {
            source: PayloadNode {
                kind,
                mode: mode_type | permissions,
                user: PayloadIdentity::Numeric { id: 0 },
                group: PayloadIdentity::Numeric { id: 0 },
                mtime: PayloadTimestamp::UNIX_EPOCH,
                xattrs: BTreeMap::new(),
            },
            uid: 0,
            gid: 0,
        }
    }

    fn root() -> ResolvedPayloadNode {
        node(PayloadNodeKind::Directory, 0o755)
    }

    fn directory(path: &str) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: node(PayloadNodeKind::Directory, 0o755),
            content: None,
        }
    }

    fn regular(path: &str, digest: char) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: node(
                PayloadNodeKind::Regular {
                    hardlink_identity: None,
                },
                0o644,
            ),
            content: Some(PayloadContentAuthority {
                sha256: digest.to_string().repeat(64),
                size: 3,
            }),
        }
    }

    fn manifest(package: &str, entries: Vec<GenerationRootEntry>) -> OutputManifest {
        OutputManifest::new(
            package.repeat(64).chars().take(64).collect::<String>(),
            package,
            "1",
            root(),
            entries,
            1,
            "2026-03-19T00:00:00Z".to_string(),
        )
        .unwrap()
    }

    #[test]
    fn exact_entries_are_merged_in_path_order() {
        let first = manifest(
            "a",
            vec![
                directory("/usr"),
                directory("/usr/bin"),
                regular("/usr/bin/hello", 'a'),
            ],
        );
        let second = manifest(
            "b",
            vec![
                directory("/opt"),
                regular("/opt/tool", 'b'),
                directory("/usr"),
                directory("/usr/bin"),
            ],
        );
        let entries = compose_entries(&[&first, &second]).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/opt", "/opt/tool", "/usr", "/usr/bin", "/usr/bin/hello"]
        );
        assert_eq!(entries[4].content.as_ref().unwrap().sha256, "a".repeat(64));
    }

    #[test]
    fn last_writer_preserves_full_typed_authority() {
        let first = manifest(
            "a",
            vec![
                directory("/usr"),
                directory("/usr/bin"),
                regular("/usr/bin/hello", 'a'),
            ],
        );
        let mut replacement = regular("/usr/bin/hello", 'b');
        replacement.node.uid = 42;
        replacement.node.source.user = PayloadIdentity::Numeric { id: 42 };
        let second = manifest(
            "b",
            vec![
                directory("/usr"),
                directory("/usr/bin"),
                replacement.clone(),
            ],
        );
        let entries = compose_entries(&[&first, &second]).unwrap();
        let actual = entries
            .iter()
            .find(|entry| entry.path == "/usr/bin/hello")
            .unwrap();
        assert_eq!(actual, &replacement);
    }

    #[test]
    fn composition_splits_typed_publication_domains() {
        let manifest = manifest(
            "a",
            vec![
                directory("/etc"),
                regular("/etc/tool.conf", 'a'),
                directory("/run"),
                regular("/run/transient", 'b'),
                directory("/usr"),
                directory("/usr/bin"),
                regular("/usr/bin/tool", 'c'),
            ],
        );
        let captured = compose_selected_root(&[&manifest], root()).unwrap();
        assert_eq!(
            captured
                .generation
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/usr", "/usr/bin", "/usr/bin/tool"]
        );
        assert_eq!(
            captured
                .state
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/etc", "/etc/tool.conf"]
        );
    }

    #[test]
    fn invalid_or_empty_inputs_are_rejected() {
        assert!(matches!(
            compose_entries(&[]),
            Err(ComposeError::EmptyComposition)
        ));
        let mut invalid = manifest("a", vec![directory("/usr")]);
        invalid.output_hash = "f".repeat(64);
        assert!(matches!(
            compose_entries(&[&invalid]),
            Err(ComposeError::InvalidManifest(_))
        ));
    }

    #[test]
    fn erofs_image_hash_produces_valid_hex() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.erofs");
        std::fs::write(&file, b"fake erofs content").unwrap();
        let hash = erofs_image_hash(&file).unwrap();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|character| character.is_ascii_hexdigit()));
    }
}
