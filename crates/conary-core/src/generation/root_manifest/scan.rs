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
use std::fs::OpenOptions;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug)]
struct CapturedCandidate {
    entry: GenerationRootEntry,
    domain: RootPathDomain,
    identity: FilesystemIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FilesystemIdentity {
    device: u64,
    inode: u64,
}

// A live selected root has no global filesystem snapshot. Keep the retry budget
// local to one node so unrelated paths are never read again after a conflict.
const STABLE_NODE_CAPTURE_ATTEMPTS: usize = 8;

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
    let (root_node, candidates) =
        capture_payload_tree(root, cas, false, "selected-root", exclusions)?;

    let mut immutable = Vec::new();
    let mut state = Vec::new();
    for candidate in candidates {
        match candidate.domain {
            RootPathDomain::Immutable => immutable.push(candidate.entry),
            RootPathDomain::ConfigState | RootPathDomain::MutableState => {
                state.push(candidate.entry);
            }
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
    let (root_node, candidates) = capture_payload_tree(
        root,
        cas,
        true,
        hardlink_namespace,
        &SelectedRootCaptureExclusions::empty(),
    )?;
    let entries = candidates
        .into_iter()
        .map(|candidate| candidate.entry)
        .collect();
    Ok((root_node, entries))
}

fn capture_payload_tree(
    root: &Path,
    cas: &CasStore,
    include_ephemeral: bool,
    hardlink_namespace: &str,
    exclusions: &SelectedRootCaptureExclusions,
) -> crate::Result<(ResolvedPayloadNode, Vec<CapturedCandidate>)> {
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
        // Capture beside discovery. Holding this metadata until the complete
        // walk ends makes ordinary mutable state inherently racy.
        candidates.push(capture_path_stably(
            path,
            entry.path().to_path_buf(),
            domain,
            cas,
        )?);
    }
    candidates.sort_by(|left, right| left.entry.path.cmp(&right.entry.path));

    assign_hardlinks(&mut candidates, hardlink_namespace)?;
    Ok((root_node, candidates))
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

fn assign_hardlinks(
    candidates: &mut [CapturedCandidate],
    hardlink_namespace: &str,
) -> crate::Result<()> {
    let mut inode_members = BTreeMap::<FilesystemIdentity, Vec<usize>>::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if matches!(candidate.entry.node.source.kind, PayloadNodeKind::Directory) {
            continue;
        }
        inode_members
            .entry(candidate.identity)
            .or_default()
            .push(index);
    }

    let mut groups = inode_members
        .into_values()
        .filter(|members| members.len() > 1)
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        candidates[left[0]]
            .entry
            .path
            .cmp(&candidates[right[0]].entry.path)
    });

    let mut identity_number = 0_u64;
    for members in groups {
        let primary = members[0];
        let primary_domain = candidates[primary].domain;
        if members
            .iter()
            .any(|index| candidates[*index].domain != primary_domain)
        {
            return Err(crate::Error::NotImplemented(format!(
                "hardlink group crosses generation publication domains at {}",
                candidates[primary].entry.path
            )));
        }
        if !matches!(
            candidates[primary].entry.node.source.kind,
            PayloadNodeKind::Regular { .. }
        ) {
            return Err(crate::Error::NotImplemented(format!(
                "non-regular hardlink groups are not representable by the shared payload node at {}",
                candidates[primary].entry.path
            )));
        }

        identity_number += 1;
        let identity = format!("{hardlink_namespace}:hardlink:{identity_number}");
        let primary_path = candidates[primary].entry.path.clone();
        let PayloadNodeKind::Regular { hardlink_identity } =
            &mut candidates[primary].entry.node.source.kind
        else {
            unreachable!("regular hardlink primary was checked above");
        };
        *hardlink_identity = Some(identity.clone());
        let primary_node = candidates[primary].entry.node.clone();

        for index in members.into_iter().skip(1) {
            let mut node = primary_node.clone();
            node.source.kind = PayloadNodeKind::Hardlink {
                target: primary_path.clone(),
                identity: identity.clone(),
            };
            node.validate()
                .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
            candidates[index].entry.node = node;
            candidates[index].entry.content = None;
        }
    }
    Ok(())
}

fn capture_path_stably(
    path: String,
    filesystem_path: PathBuf,
    domain: RootPathDomain,
    cas: &CasStore,
) -> crate::Result<CapturedCandidate> {
    capture_path_stably_with_observer(path, filesystem_path, domain, cas, &mut |_, _| {})
}

fn capture_path_stably_with_observer(
    path: String,
    filesystem_path: PathBuf,
    domain: RootPathDomain,
    cas: &CasStore,
    observer: &mut impl FnMut(usize, &Path),
) -> crate::Result<CapturedCandidate> {
    for attempt in 1..=STABLE_NODE_CAPTURE_ATTEMPTS {
        let before = std::fs::symlink_metadata(&filesystem_path).map_err(|error| {
            crate::Error::IoError(format!(
                "failed to inspect selected-root path {}: {error}",
                filesystem_path.display()
            ))
        })?;
        let captured = if before.file_type().is_file() {
            capture_regular_attempt(&filesystem_path, &before, cas, attempt, observer)?
        } else {
            capture_non_regular_attempt(&filesystem_path, &before)?
        };
        if let Some(captured) = captured {
            return Ok(CapturedCandidate {
                entry: GenerationRootEntry {
                    path,
                    node: captured.node,
                    content: captured.content,
                },
                domain,
                identity: captured.identity,
            });
        }
        std::thread::yield_now();
    }

    Err(crate::Error::ConflictError(format!(
        "selected-root path did not remain stable during any of {STABLE_NODE_CAPTURE_ATTEMPTS} per-node capture attempts: {}",
        filesystem_path.display()
    )))
}

#[derive(Debug)]
struct CapturedNodeSnapshot {
    node: ResolvedPayloadNode,
    content: Option<PayloadContentAuthority>,
    identity: FilesystemIdentity,
}

fn capture_regular_attempt(
    path: &Path,
    discovered: &std::fs::Metadata,
    cas: &CasStore,
    attempt: usize,
    observer: &mut impl FnMut(usize, &Path),
) -> crate::Result<Option<CapturedNodeSnapshot>> {
    // Bind content, metadata, and xattrs to one descriptor. Reopening the path
    // for CAS ingestion would restore the same path-replacement race.
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => {
            if std::fs::symlink_metadata(path)
                .is_ok_and(|after| !metadata_snapshot_matches(discovered, &after))
            {
                return Ok(None);
            }
            return Err(crate::Error::IoError(format!(
                "failed to open selected-root regular file {}: {error}",
                path.display()
            )));
        }
    };
    let before = file.metadata().map_err(|error| {
        crate::Error::IoError(format!(
            "failed to inspect open selected-root file {}: {error}",
            path.display()
        ))
    })?;
    if !before.file_type().is_file() || !same_filesystem_node(discovered, &before) {
        return Ok(None);
    }

    let xattrs = read_xattrs_from_fd(&file, path)?;
    observer(attempt, path);

    let mut bytes = Vec::new();
    file.by_ref()
        .take(before.len().saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            crate::Error::IoError(format!(
                "failed to read selected-root regular file {}: {error}",
                path.display()
            ))
        })?;
    let after = file.metadata().map_err(|error| {
        crate::Error::IoError(format!(
            "failed to re-inspect open selected-root file {}: {error}",
            path.display()
        ))
    })?;
    let Ok(path_after) = std::fs::symlink_metadata(path) else {
        return Ok(None);
    };
    let byte_count = u64::try_from(bytes.len()).map_err(|_| {
        crate::Error::IoError(format!(
            "selected-root regular file is too large to capture: {}",
            path.display()
        ))
    })?;
    if !metadata_snapshot_matches(&before, &after)
        || !metadata_snapshot_matches(&after, &path_after)
        || byte_count != before.len()
    {
        return Ok(None);
    }

    let node = node_from_metadata_and_xattrs(
        path,
        &before,
        PayloadNodeKind::Regular {
            hardlink_identity: None,
        },
        xattrs,
    )?;
    let digest = cas.store_private_copy(&bytes).map_err(|error| {
        crate::Error::IoError(format!(
            "failed to capture selected-root regular file {} in CAS: {error}",
            path.display()
        ))
    })?;
    Ok(Some(CapturedNodeSnapshot {
        node,
        content: Some(PayloadContentAuthority {
            sha256: digest,
            size: byte_count,
        }),
        identity: filesystem_identity(&before),
    }))
}

fn capture_non_regular_attempt(
    path: &Path,
    before: &std::fs::Metadata,
) -> crate::Result<Option<CapturedNodeSnapshot>> {
    let kind = kind_from_metadata(path, before);
    let node = kind.and_then(|kind| node_from_metadata(path, before, kind));
    let Ok(after) = std::fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if !metadata_snapshot_matches(before, &after) {
        return Ok(None);
    }
    Ok(Some(CapturedNodeSnapshot {
        node: node?,
        content: None,
        identity: filesystem_identity(before),
    }))
}

fn filesystem_identity(metadata: &std::fs::Metadata) -> FilesystemIdentity {
    FilesystemIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

fn same_filesystem_node(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.mode() & libc::S_IFMT == right.mode() & libc::S_IFMT
}

fn metadata_snapshot_matches(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    same_filesystem_node(left, right)
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && left.uid() == right.uid()
        && left.gid() == right.gid()
        && left.rdev() == right.rdev()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
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
    node_from_metadata_and_xattrs(path, metadata, kind, read_xattrs(path)?)
}

fn node_from_metadata_and_xattrs(
    path: &Path,
    metadata: &std::fs::Metadata,
    kind: PayloadNodeKind,
    xattrs: BTreeMap<String, Vec<u8>>,
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
        xattrs,
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

fn read_xattrs_from_fd(
    file: &std::fs::File,
    path: &Path,
) -> crate::Result<BTreeMap<String, Vec<u8>>> {
    let fd = file.as_raw_fd();
    let names_len = unsafe { libc::flistxattr(fd, std::ptr::null_mut(), 0) };
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
    let actual =
        unsafe { libc::flistxattr(fd, names.as_mut_ptr().cast::<libc::c_char>(), names.len()) };
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
        let value_len = unsafe { libc::fgetxattr(fd, c_name.as_ptr(), std::ptr::null_mut(), 0) };
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
                libc::fgetxattr(
                    fd,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_capture_restarts_only_the_node_that_changed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("system.journal");
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        std::fs::write(&path, b"journal before capture").unwrap();

        let final_bytes = b"journal state after a concurrent append";
        let mut attempts = 0;
        let captured = capture_path_stably_with_observer(
            "/var/log/journal/system.journal".to_string(),
            path,
            RootPathDomain::MutableState,
            &cas,
            &mut |attempt, path| {
                attempts += 1;
                if attempt == 1 {
                    std::fs::write(path, final_bytes).unwrap();
                }
            },
        )
        .unwrap();

        assert_eq!(attempts, 2);
        assert_eq!(captured.entry.path, "/var/log/journal/system.journal");
        assert_eq!(captured.domain, RootPathDomain::MutableState);
        let content = captured.entry.content.unwrap();
        assert_eq!(content.size, final_bytes.len() as u64);
        assert_eq!(cas.retrieve(&content.sha256).unwrap(), final_bytes);
    }
}
