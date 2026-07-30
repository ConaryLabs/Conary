// conary-core/src/generation/root_manifest/composefs.rs

//! Composefs/EROFS serialization from the exact typed generation-root tree.

use super::GenerationRootManifest;
use crate::generation::builder::BuildResult;
use std::path::Path;

/// Persist the root manifest and serialize the same typed tree to
/// `root.erofs`.
#[cfg(feature = "composefs-rs")]
pub fn build_erofs_image_from_root_manifest(
    manifest: &GenerationRootManifest,
    generation_dir: &Path,
) -> crate::Result<BuildResult> {
    use crate::generation::metadata::EROFS_IMAGE_NAME;
    use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};
    use composefs::erofs::writer::{ValidatedFileSystem, mkfs_erofs};
    use composefs::fsverity::{FsVerityHashValue, Sha256HashValue};
    use composefs::tree::{
        Directory, FileSystem, Inode, LeafContent, RegularFile, Stat, generic_tree::LeafId,
    };
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::io::Write;

    fn stat(node: &ResolvedPayloadNode, path: &str) -> crate::Result<Stat> {
        let uid = u32::try_from(node.uid).map_err(|_| {
            crate::Error::InvalidPath(format!(
                "uid {} is not representable in EROFS at {path}",
                node.uid
            ))
        })?;
        let gid = u32::try_from(node.gid).map_err(|_| {
            crate::Error::InvalidPath(format!(
                "gid {} is not representable in EROFS at {path}",
                node.gid
            ))
        })?;
        Ok(Stat {
            // composefs applies the file-type bits from LeafContent/Inode.
            st_mode: node.source.mode & 0o7777,
            st_uid: uid,
            st_gid: gid,
            st_mtim_sec: node.source.mtime.seconds,
            st_mtim_nsec: node.source.mtime.nanoseconds,
            xattrs: node
                .source
                .xattrs
                .iter()
                .map(|(name, value)| (OsStr::new(name).into(), value.clone().into()))
                .collect(),
        })
    }

    fn parent_and_name(path: &str) -> crate::Result<(&str, &str)> {
        let relative = path.strip_prefix('/').ok_or_else(|| {
            crate::Error::InvalidPath(format!("manifest path is not absolute: {path:?}"))
        })?;
        let (parent, name) = relative
            .rsplit_once('/')
            .map_or(("", relative), |(parent, name)| (parent, name));
        if name.is_empty() {
            return Err(crate::Error::InvalidPath(format!(
                "manifest path has no filename: {path:?}"
            )));
        }
        Ok((parent, name))
    }

    fn parent_directory_mut<'a>(
        root: &'a mut Directory<Sha256HashValue>,
        parent: &str,
        path: &str,
    ) -> crate::Result<&'a mut Directory<Sha256HashValue>> {
        if parent.is_empty() {
            Ok(root)
        } else {
            root.get_directory_mut(OsStr::new(parent)).map_err(|error| {
                crate::Error::InvalidPath(format!(
                    "manifest path {path} has unavailable parent {parent}: {error}"
                ))
            })
        }
    }

    manifest.validate()?;
    std::fs::create_dir_all(generation_dir)?;
    let mut fs = FileSystem::<Sha256HashValue>::new(stat(&manifest.root, "/")?);
    let mut hardlink_leaves = BTreeMap::<String, LeafId>::new();
    let mut cas_objects_referenced = 0_u64;

    for entry in manifest
        .entries
        .iter()
        .filter(|entry| matches!(entry.node.source.kind, PayloadNodeKind::Directory))
    {
        let (parent, name) = parent_and_name(&entry.path)?;
        let directory = Directory::new(stat(&entry.node, &entry.path)?);
        parent_directory_mut(&mut fs.root, parent, &entry.path)?
            .insert(OsStr::new(name), Inode::Directory(Box::new(directory)));
    }

    for entry in manifest.entries.iter().filter(|entry| {
        !matches!(
            entry.node.source.kind,
            PayloadNodeKind::Directory | PayloadNodeKind::Hardlink { .. }
        )
    }) {
        let (parent, name) = parent_and_name(&entry.path)?;
        let kind = &entry.node.source.kind;
        let content = match kind {
            PayloadNodeKind::Regular { .. } => {
                let authority = entry.content.as_ref().ok_or_else(|| {
                    crate::Error::InvalidPath(format!(
                        "regular manifest entry has no content authority: {}",
                        entry.path
                    ))
                })?;
                let digest = authority
                    .sha256
                    .strip_prefix("sha256:")
                    .unwrap_or(&authority.sha256);
                let digest = Sha256HashValue::from_hex(digest).map_err(|error| {
                    crate::Error::InvalidPath(format!(
                        "invalid SHA-256 digest for {}: {error}",
                        entry.path
                    ))
                })?;
                cas_objects_referenced += 1;
                LeafContent::Regular(RegularFile::External(digest, authority.size))
            }
            PayloadNodeKind::Symlink { target } => LeafContent::Symlink(OsStr::new(target).into()),
            PayloadNodeKind::BlockDevice { major, minor } => {
                let major = libc::c_uint::try_from(*major).map_err(|_| {
                    crate::Error::InvalidPath(format!(
                        "block-device major is not representable at {}",
                        entry.path
                    ))
                })?;
                let minor = libc::c_uint::try_from(*minor).map_err(|_| {
                    crate::Error::InvalidPath(format!(
                        "block-device minor is not representable at {}",
                        entry.path
                    ))
                })?;
                LeafContent::BlockDevice(libc::makedev(major, minor))
            }
            PayloadNodeKind::CharacterDevice { major, minor } => {
                let major = libc::c_uint::try_from(*major).map_err(|_| {
                    crate::Error::InvalidPath(format!(
                        "character-device major is not representable at {}",
                        entry.path
                    ))
                })?;
                let minor = libc::c_uint::try_from(*minor).map_err(|_| {
                    crate::Error::InvalidPath(format!(
                        "character-device minor is not representable at {}",
                        entry.path
                    ))
                })?;
                LeafContent::CharacterDevice(libc::makedev(major, minor))
            }
            PayloadNodeKind::Fifo => LeafContent::Fifo,
            PayloadNodeKind::Socket => LeafContent::Socket,
            PayloadNodeKind::Directory | PayloadNodeKind::Hardlink { .. } => {
                unreachable!("filtered above")
            }
        };
        let leaf_id = fs.push_leaf(stat(&entry.node, &entry.path)?, content);
        if let PayloadNodeKind::Regular {
            hardlink_identity: Some(identity),
        } = kind
        {
            hardlink_leaves.insert(identity.clone(), leaf_id);
        }
        parent_directory_mut(&mut fs.root, parent, &entry.path)?
            .insert(OsStr::new(name), Inode::leaf(leaf_id));
    }

    for entry in manifest
        .entries
        .iter()
        .filter(|entry| matches!(entry.node.source.kind, PayloadNodeKind::Hardlink { .. }))
    {
        let PayloadNodeKind::Hardlink { identity, .. } = &entry.node.source.kind else {
            unreachable!("filtered above");
        };
        let (parent, name) = parent_and_name(&entry.path)?;
        let leaf_id = hardlink_leaves.get(identity).copied().ok_or_else(|| {
            crate::Error::InvalidPath(format!(
                "hardlink {} refers to unavailable identity {identity:?}",
                entry.path
            ))
        })?;
        parent_directory_mut(&mut fs.root, parent, &entry.path)?
            .insert(OsStr::new(name), Inode::leaf(leaf_id));
    }

    fs.compact();
    let fs = ValidatedFileSystem::new(fs).map_err(|error| {
        crate::Error::InternalError(format!(
            "invalid generation-root composefs tree before EROFS serialization: {error}"
        ))
    })?;
    let image_bytes = mkfs_erofs(&fs);
    let image_size = image_bytes.len() as u64;
    let erofs_verity_digest =
        composefs::fsverity::compute_verity::<Sha256HashValue>(&image_bytes).to_hex();

    let image_path = generation_dir.join(EROFS_IMAGE_NAME);
    let tmp_path = generation_dir.join(format!(".{EROFS_IMAGE_NAME}.tmp"));
    {
        let mut tmp_file = std::fs::File::create(&tmp_path).map_err(|error| {
            crate::Error::IoError(format!(
                "failed to create temporary EROFS image {}: {error}",
                tmp_path.display()
            ))
        })?;
        tmp_file.write_all(&image_bytes)?;
        tmp_file.sync_all()?;
    }
    std::fs::rename(&tmp_path, &image_path)?;
    std::fs::File::open(generation_dir)?.sync_all()?;
    manifest.write_to(generation_dir)?;

    Ok(BuildResult {
        image_path,
        image_size,
        cas_objects_referenced,
        erofs_verity_digest: Some(erofs_verity_digest),
    })
}

/// Feature-gated stub for builds without composefs support.
#[cfg(not(feature = "composefs-rs"))]
pub fn build_erofs_image_from_root_manifest(
    _manifest: &GenerationRootManifest,
    _generation_dir: &Path,
) -> crate::Result<BuildResult> {
    Err(crate::Error::NotImplemented(
        "generation-root EROFS building requires the 'composefs-rs' feature".to_string(),
    ))
}
