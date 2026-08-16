// conary-core/src/ccs/native_export/hardlinks.rs
//! Shared fail-closed hardlink validation for native package exporters.

use crate::ccs::builder::{BuildResult, FileEntry};
use crate::payload::{PayloadIdentity, PayloadNode, PayloadNodeKind};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::Path;
use tar::Builder;

#[derive(Debug)]
pub(super) struct Topology<'a> {
    anchors: BTreeMap<&'a str, &'a FileEntry>,
    ordered: Vec<&'a FileEntry>,
}

pub(super) fn append_tar_payload<W: std::io::Write>(
    archive: &mut Builder<W>,
    staging_root: &Path,
    topology: &Topology<'_>,
    emit_pax: bool,
) -> Result<()> {
    for file in topology.ordered_files() {
        let archive_path = file.path.trim_start_matches('/');
        if archive_path.is_empty() {
            anyhow::bail!("native export payload node path must not resolve to archive root");
        }
        let mut header = exact_tar_header(&file.node)?;
        if emit_pax {
            append_node_pax_extensions(archive, &file.node)?;
        }
        match &file.node.kind {
            PayloadNodeKind::Regular { .. } => {
                let authority = file.content.as_ref().with_context(|| {
                    format!(
                        "regular payload node {} has no content authority",
                        file.path
                    )
                })?;
                let staged = crate::filesystem::safe_join(staging_root, &file.path)?;
                header.set_size(authority.size);
                archive.append_data(&mut header, archive_path, File::open(staged)?)?;
            }
            PayloadNodeKind::Symlink { target } => {
                archive.append_link(&mut header, archive_path, target)?;
            }
            PayloadNodeKind::Hardlink { target, .. } => {
                archive.append_link(&mut header, archive_path, target.trim_start_matches('/'))?;
            }
            PayloadNodeKind::Directory => {
                archive.append_data(&mut header, archive_path, std::io::empty())?;
            }
            other => {
                anyhow::bail!(
                    "native tar exporter does not encode {} node {}",
                    kind_name(other),
                    file.path
                );
            }
        }
    }
    Ok(())
}

fn exact_tar_header(node: &PayloadNode) -> Result<tar::Header> {
    node.validate()?;
    let mut header = tar::Header::new_gnu();
    header.set_mode(node.mode & 0o7777);
    let PayloadIdentity::Numeric { id: uid } = &node.user else {
        anyhow::bail!("native tar export requires numeric payload user authority");
    };
    let PayloadIdentity::Numeric { id: gid } = &node.group else {
        anyhow::bail!("native tar export requires numeric payload group authority");
    };
    header.set_uid(*uid);
    header.set_gid(*gid);
    header.set_mtime(u64::try_from(node.mtime.seconds).unwrap_or(0));
    header.set_size(0);
    header.set_entry_type(match &node.kind {
        PayloadNodeKind::Regular { .. } => tar::EntryType::Regular,
        PayloadNodeKind::Directory => tar::EntryType::Directory,
        PayloadNodeKind::Symlink { .. } => tar::EntryType::Symlink,
        PayloadNodeKind::Hardlink { .. } => tar::EntryType::Link,
        other => {
            anyhow::bail!(
                "native tar exporter does not encode {} payload nodes",
                kind_name(other)
            );
        }
    });
    Ok(header)
}

fn append_node_pax_extensions<W: std::io::Write>(
    archive: &mut Builder<W>,
    node: &PayloadNode,
) -> Result<()> {
    let mut extensions = Vec::new();
    if node.mtime.nanoseconds != 0 || node.mtime.seconds < 0 {
        extensions.push((
            "mtime".to_string(),
            exact_pax_timestamp(node.mtime.seconds, node.mtime.nanoseconds).into_bytes(),
        ));
    }
    for (name, value) in &node.xattrs {
        extensions.push((format!("SCHILY.xattr.{name}"), value.clone()));
    }
    if !extensions.is_empty() {
        archive.append_pax_extensions(
            extensions
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_slice())),
        )?;
    }
    Ok(())
}

fn exact_pax_timestamp(seconds: i64, nanoseconds: u32) -> String {
    if nanoseconds == 0 {
        return seconds.to_string();
    }
    if seconds >= 0 {
        return format!("{seconds}.{nanoseconds:09}");
    }
    let whole = seconds.unsigned_abs() - 1;
    let fractional = 1_000_000_000 - nanoseconds;
    format!("-{whole}.{fractional:09}")
}

fn kind_name(kind: &PayloadNodeKind) -> &'static str {
    match kind {
        PayloadNodeKind::Regular { .. } => "regular",
        PayloadNodeKind::Directory => "directory",
        PayloadNodeKind::Symlink { .. } => "symlink",
        PayloadNodeKind::Hardlink { .. } => "hardlink",
        PayloadNodeKind::BlockDevice { .. } => "block-device",
        PayloadNodeKind::CharacterDevice { .. } => "character-device",
        PayloadNodeKind::Fifo => "fifo",
        PayloadNodeKind::Socket => "socket",
    }
}

impl<'a> Topology<'a> {
    pub(super) fn validate(result: &'a BuildResult) -> Result<Self> {
        let mut by_path = HashMap::new();
        let mut anchors = BTreeMap::new();
        let mut aliases = BTreeMap::<&str, Vec<&FileEntry>>::new();
        for file in &result.files {
            if by_path.insert(file.path.as_str(), file).is_some() {
                anyhow::bail!(
                    "native export payload contains duplicate path {}",
                    file.path
                );
            }
            match &file.node.kind {
                PayloadNodeKind::Regular {
                    hardlink_identity: Some(identity),
                } => {
                    if anchors.insert(identity.as_str(), file).is_some() {
                        anyhow::bail!(
                            "native export hardlink identity {identity} has multiple regular anchors"
                        );
                    }
                }
                PayloadNodeKind::Hardlink { identity, .. } => {
                    aliases.entry(identity).or_default().push(file);
                }
                _ => {}
            }
        }

        for (identity, anchor) in &anchors {
            if anchor.content.is_none() {
                anyhow::bail!(
                    "native export hardlink identity {identity} anchor {} has no content authority",
                    anchor.path
                );
            }
            if !aliases.contains_key(identity) {
                anyhow::bail!(
                    "native export hardlink identity {identity} is a partial set with anchor {} and no aliases",
                    anchor.path
                );
            }
        }
        for (identity, members) in &aliases {
            let anchor = anchors.get(identity).with_context(|| {
                format!(
                    "native export hardlink identity {identity} has aliases but no regular anchor"
                )
            })?;
            for member in members {
                if member.content.is_some() || member.chunks.is_some() {
                    anyhow::bail!(
                        "native export hardlink identity {identity} alias {} carries non-anchor content authority",
                        member.path
                    );
                }
                let resolved = resolve_target(member, &by_path)?;
                if resolved.path != anchor.path {
                    anyhow::bail!(
                        "native export hardlink {} resolves to {} instead of identity {identity} anchor {}",
                        member.path,
                        resolved.path,
                        anchor.path
                    );
                }
                require_metadata_agreement(anchor, member, identity)?;
            }
        }

        let mut ordered = result
            .files
            .iter()
            .filter(|file| !matches!(file.node.kind, PayloadNodeKind::Hardlink { .. }))
            .collect::<Vec<_>>();
        let mut emitted = ordered
            .iter()
            .map(|file| file.path.as_str())
            .collect::<HashSet<_>>();
        let mut remaining = result
            .files
            .iter()
            .filter(|file| matches!(file.node.kind, PayloadNodeKind::Hardlink { .. }))
            .collect::<Vec<_>>();
        while !remaining.is_empty() {
            let before = remaining.len();
            let mut deferred = Vec::new();
            for file in remaining {
                let PayloadNodeKind::Hardlink { target, .. } = &file.node.kind else {
                    unreachable!("hardlink ordering only receives hardlink nodes");
                };
                if emitted.contains(target.as_str()) {
                    emitted.insert(file.path.as_str());
                    ordered.push(file);
                } else {
                    deferred.push(file);
                }
            }
            if deferred.len() == before {
                anyhow::bail!("native export hardlink topology has no target-before-alias order");
            }
            remaining = deferred;
        }

        Ok(Self { anchors, ordered })
    }

    pub(super) fn ordered_files(&self) -> &[&'a FileEntry] {
        &self.ordered
    }

    pub(super) fn anchor(&self, identity: &str) -> &'a FileEntry {
        self.anchors[identity]
    }

    pub(super) fn require_deb_hardlink_metadata(&self) -> Result<()> {
        for file in &self.ordered {
            let belongs_to_set = matches!(
                file.node.kind,
                PayloadNodeKind::Regular {
                    hardlink_identity: Some(_)
                } | PayloadNodeKind::Hardlink { .. }
            );
            if !belongs_to_set {
                continue;
            }
            if file.node.mtime.seconds < 0 || file.node.mtime.nanoseconds != 0 {
                anyhow::bail!(
                    "DEB hardlink export cannot represent subsecond or negative mtime authority at {}",
                    file.path
                );
            }
            if !file.node.xattrs.is_empty() {
                anyhow::bail!(
                    "DEB hardlink export cannot represent xattr authority at {}",
                    file.path
                );
            }
        }
        Ok(())
    }
}

fn resolve_target<'a>(
    member: &'a FileEntry,
    by_path: &HashMap<&str, &'a FileEntry>,
) -> Result<&'a FileEntry> {
    let PayloadNodeKind::Hardlink { identity, target } = &member.node.kind else {
        anyhow::bail!("native export target resolution requires a hardlink node");
    };
    let mut path = target.as_str();
    let mut visited = HashSet::from([member.path.as_str()]);
    loop {
        if !visited.insert(path) {
            anyhow::bail!("native export hardlink cycle reaches {path}");
        }
        let target_file = by_path.get(path).with_context(|| {
            format!(
                "native export hardlink {} targets missing path {path}",
                member.path
            )
        })?;
        match &target_file.node.kind {
            PayloadNodeKind::Regular {
                hardlink_identity: Some(target_identity),
            } if target_identity == identity => return Ok(target_file),
            PayloadNodeKind::Hardlink {
                target,
                identity: target_identity,
            } if target_identity == identity => path = target,
            _ => {
                anyhow::bail!(
                    "native export hardlink {} target {path} does not belong to identity {identity}",
                    member.path
                );
            }
        }
    }
}

fn require_metadata_agreement(
    anchor: &FileEntry,
    member: &FileEntry,
    identity: &str,
) -> Result<()> {
    if metadata_without_kind(&anchor.node) != metadata_without_kind(&member.node) {
        anyhow::bail!(
            "native export hardlink identity {identity} has incompatible metadata at {} and {}",
            anchor.path,
            member.path
        );
    }
    Ok(())
}

fn metadata_without_kind(
    node: &PayloadNode,
) -> (
    u32,
    &crate::payload::PayloadIdentity,
    &crate::payload::PayloadIdentity,
    crate::payload::PayloadTimestamp,
    &std::collections::BTreeMap<String, Vec<u8>>,
) {
    (node.mode, &node.user, &node.group, node.mtime, &node.xattrs)
}
