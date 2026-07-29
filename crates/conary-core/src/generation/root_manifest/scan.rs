// conary-core/src/generation/root_manifest/scan.rs

//! Exact selected-root scanner and CAS capture.

use super::{
    CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry,
    GenerationRootManifest, MutableStateManifest, RootPathDomain, classify_root_path,
};
use crate::filesystem::CasStore;
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug)]
struct Candidate {
    path: String,
    filesystem_path: PathBuf,
    metadata: std::fs::Metadata,
    domain: RootPathDomain,
}

/// Exact selected-root subtrees that are owned by the capture runtime itself.
///
/// The finite publication domains remain authoritative for `/run`, device/API
/// trees, user state, and other ephemeral paths. These exclusions are only for
/// additional absolute subtrees such as Conary's CAS and database directory,
/// whose contents must never become inputs to their own capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedRootCaptureExclusions {
    subtrees: Vec<String>,
}

impl SelectedRootCaptureExclusions {
    pub fn new(mut subtrees: Vec<String>) -> crate::Result<Self> {
        for subtree in &subtrees {
            super::validate_root_path(subtree)?;
        }
        subtrees.sort();
        subtrees.dedup();
        Ok(Self { subtrees })
    }

    pub fn empty() -> Self {
        Self {
            subtrees: Vec::new(),
        }
    }

    pub fn excludes(&self, path: &str) -> bool {
        self.subtrees.iter().any(|subtree| {
            path == subtree
                || path
                    .strip_prefix(subtree)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    }
}

pub fn scan_selected_root(root: &Path, cas: &CasStore) -> crate::Result<CapturedSelectedRoot> {
    scan_selected_root_with_exclusions(root, cas, &SelectedRootCaptureExclusions::empty())
}

pub fn scan_selected_root_with_exclusions(
    root: &Path,
    cas: &CasStore,
    exclusions: &SelectedRootCaptureExclusions,
) -> crate::Result<CapturedSelectedRoot> {
    let (root_node, candidates, hardlinks) =
        inspect_payload_tree(root, false, "selected-root", exclusions)?;

    let mut immutable = Vec::new();
    let mut state = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let entry = capture_candidate(candidate, index, &hardlinks, cas)?;
        match candidate.domain {
            RootPathDomain::Immutable => immutable.push(entry),
            RootPathDomain::ConfigState | RootPathDomain::MutableState => state.push(entry),
            RootPathDomain::EphemeralMountOrUser => unreachable!("filtered above"),
        }
    }

    let captured = CapturedSelectedRoot {
        generation: GenerationRootManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            root: root_node,
            entries: immutable,
        },
        state: MutableStateManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            entries: state,
        },
    };
    captured.generation.validate()?;
    captured.state.validate()?;
    Ok(captured)
}

/// Capture a complete package/build output tree without discarding any path
/// domain.
///
/// The returned node identities are numeric because they were resolved by the
/// build root that produced the tree. `hardlink_namespace` must be a stable,
/// unique identifier for the captured output (normally its derivation ID).
pub fn scan_payload_tree(
    root: &Path,
    cas: &CasStore,
    hardlink_namespace: &str,
) -> crate::Result<(ResolvedPayloadNode, Vec<GenerationRootEntry>)> {
    let (root_node, candidates, hardlinks) = inspect_payload_tree(
        root,
        true,
        hardlink_namespace,
        &SelectedRootCaptureExclusions::empty(),
    )?;
    let entries = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| capture_candidate(candidate, index, &hardlinks, cas))
        .collect::<crate::Result<Vec<_>>>()?;
    Ok((root_node, entries))
}

fn inspect_payload_tree(
    root: &Path,
    include_ephemeral: bool,
    hardlink_namespace: &str,
    exclusions: &SelectedRootCaptureExclusions,
) -> crate::Result<(ResolvedPayloadNode, Vec<Candidate>, HardlinkPlan)> {
    if hardlink_namespace.is_empty() || hardlink_namespace.contains('\0') {
        return Err(crate::Error::InvalidPath(
            "payload-tree hardlink namespace must be non-empty and NUL-free".to_string(),
        ));
    }
    let root_node = capture_root_node(root)?;
    let mut candidates = Vec::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .sort_by(|left, right| {
            left.file_name()
                .as_bytes()
                .cmp(right.file_name().as_bytes())
        })
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let Ok(path) = root_manifest_path(root, entry.path()) else {
                return true;
            };
            !exclusions.excludes(&path)
                && (include_ephemeral
                    || classify_root_path(&path).ok() != Some(RootPathDomain::EphemeralMountOrUser))
        });
    for entry in walker {
        let entry = entry.map_err(|error| {
            crate::Error::IoError(format!(
                "failed to walk selected generation root {}: {error}",
                root.display()
            ))
        })?;
        if entry.depth() == 0 {
            continue;
        }
        let path = root_manifest_path(root, entry.path())?;
        if exclusions.excludes(&path) {
            continue;
        }
        let domain = classify_root_path(&path)?;
        if !include_ephemeral && domain == RootPathDomain::EphemeralMountOrUser {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(|error| {
            crate::Error::IoError(format!(
                "failed to inspect selected-root path {}: {error}",
                entry.path().display()
            ))
        })?;
        candidates.push(Candidate {
            path,
            filesystem_path: entry.path().to_path_buf(),
            metadata,
            domain,
        });
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));

    let hardlinks = plan_hardlinks(&candidates, hardlink_namespace)?;
    Ok((root_node, candidates, hardlinks))
}

/// Capture exact metadata authority for the selected-root directory itself.
pub fn capture_root_node(root: &Path) -> crate::Result<ResolvedPayloadNode> {
    let root_metadata = std::fs::symlink_metadata(root).map_err(|error| {
        crate::Error::NotFound(format!(
            "selected generation root {} is unavailable: {error}",
            root.display()
        ))
    })?;
    if !root_metadata.file_type().is_dir() {
        return Err(crate::Error::InvalidPath(format!(
            "selected generation root is not a directory: {}",
            root.display()
        )));
    }
    node_from_metadata(root, &root_metadata, PayloadNodeKind::Directory)
}

/// Capture complete metadata authority for one existing filesystem node.
///
/// Regular files are represented without a hardlink identity because callers
/// using this single-node primitive have no complete inode-group view.
pub fn capture_existing_payload_node(path: &Path) -> crate::Result<ResolvedPayloadNode> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        crate::Error::NotFound(format!(
            "selected-root path {} is unavailable: {error}",
            path.display()
        ))
    })?;
    let kind = kind_from_metadata(path, &metadata)?;
    node_from_metadata(path, &metadata, kind)
}

#[derive(Debug)]
struct HardlinkPlan {
    role_by_index: BTreeMap<usize, HardlinkRole>,
}

#[derive(Debug)]
enum HardlinkRole {
    Primary { identity: String },
    Link { identity: String, target: String },
}

fn plan_hardlinks(
    candidates: &[Candidate],
    hardlink_namespace: &str,
) -> crate::Result<HardlinkPlan> {
    let mut inode_members = BTreeMap::<(u64, u64), Vec<usize>>::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.metadata.file_type().is_dir() {
            continue;
        }
        inode_members
            .entry((candidate.metadata.dev(), candidate.metadata.ino()))
            .or_default()
            .push(index);
    }

    let mut role_by_index = BTreeMap::new();
    let mut identity_number = 0_u64;
    for members in inode_members.values().filter(|members| members.len() > 1) {
        let primary = members[0];
        let primary_domain = candidates[primary].domain;
        if members
            .iter()
            .any(|index| candidates[*index].domain != primary_domain)
        {
            return Err(crate::Error::NotImplemented(format!(
                "hardlink group crosses generation publication domains at {}",
                candidates[primary].path
            )));
        }
        if !candidates[primary].metadata.file_type().is_file() {
            return Err(crate::Error::NotImplemented(format!(
                "non-regular hardlink groups are not representable by the shared payload node at {}",
                candidates[primary].path
            )));
        }
        identity_number += 1;
        let identity = format!("{hardlink_namespace}:hardlink:{identity_number}");
        role_by_index.insert(
            primary,
            HardlinkRole::Primary {
                identity: identity.clone(),
            },
        );
        for index in members.iter().skip(1) {
            role_by_index.insert(
                *index,
                HardlinkRole::Link {
                    identity: identity.clone(),
                    target: candidates[primary].path.clone(),
                },
            );
        }
    }
    Ok(HardlinkPlan { role_by_index })
}

fn capture_candidate(
    candidate: &Candidate,
    index: usize,
    hardlinks: &HardlinkPlan,
    cas: &CasStore,
) -> crate::Result<GenerationRootEntry> {
    if let Some(HardlinkRole::Link { identity, target }) = hardlinks.role_by_index.get(&index) {
        let node = node_from_metadata(
            &candidate.filesystem_path,
            &candidate.metadata,
            PayloadNodeKind::Hardlink {
                target: target.clone(),
                identity: identity.clone(),
            },
        )?;
        return Ok(GenerationRootEntry {
            path: candidate.path.clone(),
            node,
            content: None,
        });
    }

    let mut kind = kind_from_metadata(&candidate.filesystem_path, &candidate.metadata)?;
    if let Some(HardlinkRole::Primary { identity }) = hardlinks.role_by_index.get(&index) {
        let PayloadNodeKind::Regular { hardlink_identity } = &mut kind else {
            unreachable!("hardlink planner accepts only regular primary nodes");
        };
        *hardlink_identity = Some(identity.clone());
    }
    let node = node_from_metadata(&candidate.filesystem_path, &candidate.metadata, kind)?;
    let content = if matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
        let digest = cas
            .store_file_copy_from_existing(&candidate.filesystem_path)
            .map_err(|error| {
                crate::Error::IoError(format!(
                    "failed to capture selected-root regular file {} in CAS: {error}",
                    candidate.filesystem_path.display()
                ))
            })?;
        let after = std::fs::symlink_metadata(&candidate.filesystem_path)?;
        ensure_unchanged_during_capture(candidate, &after)?;
        Some(PayloadContentAuthority {
            sha256: digest,
            size: candidate.metadata.len(),
        })
    } else {
        None
    };
    Ok(GenerationRootEntry {
        path: candidate.path.clone(),
        node,
        content,
    })
}

fn ensure_unchanged_during_capture(
    before: &Candidate,
    after: &std::fs::Metadata,
) -> crate::Result<()> {
    if before.metadata.dev() != after.dev()
        || before.metadata.ino() != after.ino()
        || before.metadata.len() != after.len()
        || before.metadata.mtime() != after.mtime()
        || before.metadata.mtime_nsec() != after.mtime_nsec()
        || before.metadata.ctime() != after.ctime()
        || before.metadata.ctime_nsec() != after.ctime_nsec()
    {
        return Err(crate::Error::ConflictError(format!(
            "selected-root path changed during manifest capture: {}",
            before.filesystem_path.display()
        )));
    }
    Ok(())
}

fn kind_from_metadata(path: &Path, metadata: &std::fs::Metadata) -> crate::Result<PayloadNodeKind> {
    let file_type = metadata.file_type();
    if file_type.is_file() {
        return Ok(PayloadNodeKind::Regular {
            hardlink_identity: None,
        });
    }
    if file_type.is_dir() {
        return Ok(PayloadNodeKind::Directory);
    }
    if file_type.is_symlink() {
        let target = std::fs::read_link(path)?;
        let target = target.to_str().ok_or_else(|| {
            crate::Error::NotImplemented(format!(
                "selected-root symlink target is not UTF-8 and cannot use PayloadNode: {}",
                path.display()
            ))
        })?;
        return Ok(PayloadNodeKind::Symlink {
            target: target.to_string(),
        });
    }
    if file_type.is_block_device() {
        return Ok(PayloadNodeKind::BlockDevice {
            major: u64::from(libc::major(metadata.rdev())),
            minor: u64::from(libc::minor(metadata.rdev())),
        });
    }
    if file_type.is_char_device() {
        return Ok(PayloadNodeKind::CharacterDevice {
            major: u64::from(libc::major(metadata.rdev())),
            minor: u64::from(libc::minor(metadata.rdev())),
        });
    }
    if file_type.is_fifo() {
        return Ok(PayloadNodeKind::Fifo);
    }
    if file_type.is_socket() {
        return Ok(PayloadNodeKind::Socket);
    }
    Err(crate::Error::NotImplemented(format!(
        "selected-root node type is not representable at {}",
        path.display()
    )))
}

fn node_from_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    kind: PayloadNodeKind,
) -> crate::Result<ResolvedPayloadNode> {
    let nanoseconds = u32::try_from(metadata.mtime_nsec()).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "selected-root path has invalid negative mtime nanoseconds: {}",
            path.display()
        ))
    })?;
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
            nanoseconds,
        },
        xattrs: read_xattrs(path)?,
    };
    node.validate()
        .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
    ResolvedPayloadNode::from_numeric_source(node)
        .map_err(|error| crate::Error::InvalidPath(error.to_string()))
}

fn root_manifest_path(root: &Path, path: &Path) -> crate::Result<String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "selected-root path {} escaped {}",
            path.display(),
            root.display()
        ))
    })?;
    let relative = relative.to_str().ok_or_else(|| {
        crate::Error::NotImplemented(format!(
            "selected-root path is not UTF-8 and cannot use GenerationRootManifest: {}",
            path.display()
        ))
    })?;
    if relative.is_empty() {
        return Err(crate::Error::InvalidPath(
            "selected-root manifest entry cannot name the root".to_string(),
        ));
    }
    Ok(format!("/{}", relative.trim_end_matches('/')))
}

fn read_xattrs(path: &Path) -> crate::Result<BTreeMap<String, Vec<u8>>> {
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "selected-root path contains NUL: {}",
            path.display()
        ))
    })?;
    let names_len = unsafe { libc::llistxattr(c_path.as_ptr(), std::ptr::null_mut(), 0) };
    if names_len < 0 {
        return Err(xattr_error("list", path));
    }
    if names_len == 0 {
        return Ok(BTreeMap::new());
    }
    let names_len = usize::try_from(names_len).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "selected-root xattr name list is too large at {}",
            path.display()
        ))
    })?;
    let mut names = vec![0_u8; names_len];
    let actual = unsafe {
        libc::llistxattr(
            c_path.as_ptr(),
            names.as_mut_ptr().cast::<libc::c_char>(),
            names.len(),
        )
    };
    if actual < 0 {
        return Err(xattr_error("list", path));
    }
    names.truncate(usize::try_from(actual).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "selected-root xattr name list length is invalid at {}",
            path.display()
        ))
    })?);

    let mut xattrs = BTreeMap::new();
    for raw_name in names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = std::str::from_utf8(raw_name).map_err(|_| {
            crate::Error::NotImplemented(format!(
                "selected-root xattr name is not UTF-8 at {}",
                path.display()
            ))
        })?;
        let c_name = CString::new(raw_name).expect("xattr names are NUL-separated");
        let value_len =
            unsafe { libc::lgetxattr(c_path.as_ptr(), c_name.as_ptr(), std::ptr::null_mut(), 0) };
        if value_len < 0 {
            return Err(xattr_error(&format!("read {name}"), path));
        }
        let value_len = usize::try_from(value_len).map_err(|_| {
            crate::Error::InvalidPath(format!(
                "selected-root xattr value is too large for {name} at {}",
                path.display()
            ))
        })?;
        let mut value = vec![0_u8; value_len];
        if value_len > 0 {
            let actual = unsafe {
                libc::lgetxattr(
                    c_path.as_ptr(),
                    c_name.as_ptr(),
                    value.as_mut_ptr().cast::<libc::c_void>(),
                    value.len(),
                )
            };
            if actual < 0 {
                return Err(xattr_error(&format!("read {name}"), path));
            }
            value.truncate(usize::try_from(actual).map_err(|_| {
                crate::Error::InvalidPath(format!(
                    "selected-root xattr value length is invalid for {name} at {}",
                    path.display()
                ))
            })?);
        }
        xattrs.insert(name.to_string(), value);
    }
    Ok(xattrs)
}

fn xattr_error(operation: &str, path: &Path) -> crate::Error {
    crate::Error::IoError(format!(
        "failed to {operation} xattrs for selected-root path {}: {}",
        path.display(),
        std::io::Error::last_os_error()
    ))
}
