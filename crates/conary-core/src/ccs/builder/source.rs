// conary-core/src/ccs/builder/source.rs
//! Native filesystem discovery and exact node-authority collection.

use super::{BuilderError, CcsBuilder, FileEntry};
use crate::payload::{PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub(super) struct CollectedSource {
    pub(super) entry: FileEntry,
}

impl CcsBuilder {
    pub(super) fn collect_source(
        &self,
        source_path: &Path,
        hardlink_anchors: &mut HashMap<(u64, u64), (String, String)>,
    ) -> Result<CollectedSource> {
        let relative = source_path
            .strip_prefix(&self.source_dir)
            .map_err(|_| BuilderError::FileNotUnderSource(source_path.to_path_buf()))?;
        let install_path = self.install_prefix.as_path().join(relative);
        let install_path_text = install_path
            .to_str()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "CCS payload install path derived from {} is not valid UTF-8",
                    source_path.display()
                )
            })?
            .to_string();
        let metadata = fs::symlink_metadata(source_path)?;
        let source_kind = metadata.file_type();
        let xattrs = read_payload_xattrs(source_path)?;

        let kind = if source_kind.is_symlink() {
            let target = fs::read_link(source_path)?;
            let target = target.into_os_string().into_string().map_err(|_| {
                anyhow::anyhow!(
                    "payload symlink {} has a non-UTF-8 target",
                    source_path.display()
                )
            })?;
            PayloadNodeKind::Symlink { target }
        } else if source_kind.is_dir() {
            PayloadNodeKind::Directory
        } else if source_kind.is_file() {
            let hardlink_key = (metadata.dev(), metadata.ino());
            if metadata.nlink() > 1
                && let Some((target, identity)) = hardlink_anchors.get(&hardlink_key)
            {
                PayloadNodeKind::Hardlink {
                    target: target.clone(),
                    identity: identity.clone(),
                }
            } else {
                let hardlink_identity = (metadata.nlink() > 1).then(|| {
                    let identity = format!("path:{install_path_text}");
                    hardlink_anchors
                        .insert(hardlink_key, (install_path_text.clone(), identity.clone()));
                    identity
                });
                PayloadNodeKind::Regular { hardlink_identity }
            }
        } else if source_kind.is_block_device() {
            let (major, minor) = payload_device_numbers(metadata.rdev());
            PayloadNodeKind::BlockDevice { major, minor }
        } else if source_kind.is_char_device() {
            let (major, minor) = payload_device_numbers(metadata.rdev());
            PayloadNodeKind::CharacterDevice { major, minor }
        } else if source_kind.is_fifo() {
            PayloadNodeKind::Fifo
        } else if source_kind.is_socket() {
            PayloadNodeKind::Socket
        } else {
            anyhow::bail!(
                "unsupported source payload entry kind at {}",
                source_path.display()
            );
        };
        let node = PayloadNode {
            kind,
            mode: metadata.mode(),
            user: PayloadIdentity::Numeric {
                id: u64::from(metadata.uid()),
            },
            group: PayloadIdentity::Numeric {
                id: u64::from(metadata.gid()),
            },
            mtime: PayloadTimestamp {
                seconds: metadata.mtime(),
                nanoseconds: u32::try_from(metadata.mtime_nsec()).map_err(|_| {
                    anyhow::anyhow!(
                        "payload entry {} has invalid negative mtime nanoseconds",
                        source_path.display()
                    )
                })?,
            },
            xattrs,
        };
        node.validate()?;
        let component = self.component_for_path(&install_path_text)?;
        Ok(CollectedSource {
            entry: FileEntry {
                path: install_path_text,
                node,
                content: None,
                component,
                chunks: None,
            },
        })
    }

    pub(super) fn scan_source_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for entry in WalkDir::new(&self.source_dir) {
            let entry = entry.map_err(|error| {
                anyhow::anyhow!(
                    "failed to scan complete CCS source authority at {}: {error}",
                    self.source_dir.display()
                )
            })?;
            let path = entry.path();
            if path == self.source_dir {
                continue;
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches!(name, "ccs.toml" | "MANIFEST.toml"))
            {
                continue;
            }
            files.push(path.to_path_buf());
        }
        files.sort();
        Ok(files)
    }
}

fn read_payload_xattrs(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut attributes = BTreeMap::new();
    for name in xattr::list(path)? {
        let name = name.into_string().map_err(|_| {
            anyhow::anyhow!(
                "payload entry {} has a non-UTF-8 xattr name",
                path.display()
            )
        })?;
        let value = xattr::get(path, &name)?.ok_or_else(|| {
            anyhow::anyhow!(
                "payload xattr {} disappeared while reading {}",
                name,
                path.display()
            )
        })?;
        attributes.insert(name, value);
    }
    Ok(attributes)
}

fn payload_device_numbers(device: u64) -> (u64, u64) {
    (nix::sys::stat::major(device), nix::sys::stat::minor(device))
}
