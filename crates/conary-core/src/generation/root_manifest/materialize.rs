// conary-core/src/generation/root_manifest/materialize.rs

//! Exact materialization of typed generation-root and mutable-state manifests.

use super::{
    CapturedSelectedRoot, GenerationRootEntry, GenerationRootManifest, MutableStateManifest,
};
use crate::filesystem::CasStore;
use crate::hash::HashAlgorithm;
use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

/// Recreate an immutable generation root in an empty destination.
pub fn materialize_generation_root(
    manifest: &GenerationRootManifest,
    cas: &CasStore,
    destination: &Path,
) -> crate::Result<()> {
    manifest.validate()?;
    require_sha256_cas(cas)?;
    prepare_empty_destination(destination)?;
    materialize_entries(&manifest.entries, cas, destination)?;
    apply_resolved_payload_metadata(destination, &manifest.root)?;
    sync_directory(destination)?;
    Ok(())
}

/// Recreate `/etc`, `/var`, and `/srv` state below an existing selected root.
///
/// The destination must not already contain any path named by the state
/// manifest. This is a reconstruction primitive, not a live-state merge.
pub fn materialize_state_root(
    manifest: &MutableStateManifest,
    cas: &CasStore,
    destination: &Path,
) -> crate::Result<()> {
    manifest.validate()?;
    require_sha256_cas(cas)?;
    require_directory(destination)?;
    materialize_entries(&manifest.entries, cas, destination)
}

/// Recreate the `/etc` subtree as an overlay upper directory.
///
/// Mutable-state manifests use selected-root paths such as `/etc/passwd`,
/// while an overlay upper for `/etc` stores that node as `passwd`. This
/// projection keeps the typed manifest as authority while removing exactly
/// one `/etc` prefix. `/var` and `/srv` entries remain owned by their live
/// mutable roots and are not projected into the config upper.
pub fn materialize_config_state_upper(
    manifest: &MutableStateManifest,
    cas: &CasStore,
    destination: &Path,
) -> crate::Result<()> {
    manifest.validate()?;
    require_sha256_cas(cas)?;
    prepare_empty_destination(destination)?;

    let mut root = None;
    let mut entries = Vec::new();
    for entry in &manifest.entries {
        if entry.path == "/etc" {
            if !matches!(entry.node.source.kind, PayloadNodeKind::Directory) {
                return Err(crate::Error::InvalidPath(
                    "config-state root /etc must describe a directory".to_string(),
                ));
            }
            root = Some(&entry.node);
            continue;
        }
        let Some(relative) = entry.path.strip_prefix("/etc/") else {
            continue;
        };
        let mut projected = entry.clone();
        projected.path = format!("/{relative}");
        if let PayloadNodeKind::Hardlink { target, identity } = &entry.node.source.kind {
            let projected_target = target.strip_prefix("/etc/").ok_or_else(|| {
                crate::Error::InvalidPath(format!(
                    "config-state hardlink {} targets path outside /etc: {target}",
                    entry.path
                ))
            })?;
            projected.node.source.kind = PayloadNodeKind::Hardlink {
                target: format!("/{projected_target}"),
                identity: identity.clone(),
            };
        }
        entries.push(projected);
    }

    materialize_entries(&entries, cas, destination)?;
    if let Some(root) = root {
        apply_resolved_payload_metadata(destination, root)?;
    } else {
        fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;
    }
    sync_directory(destination)
}

/// Recreate a complete selected root while preserving the captured root
/// directory metadata after state entries have been installed.
pub fn materialize_captured_selected_root(
    captured: &CapturedSelectedRoot,
    cas: &CasStore,
    destination: &Path,
) -> crate::Result<()> {
    captured.generation.validate()?;
    captured.state.validate()?;
    materialize_generation_root(&captured.generation, cas, destination)?;
    materialize_state_root(&captured.state, cas, destination)?;
    apply_resolved_payload_metadata(destination, &captured.generation.root)?;
    sync_directory(destination)
}

/// Overlay one validated package payload tree onto an existing root.
///
/// Directory nodes merge with existing directories and replace non-directory
/// nodes. Leaf nodes replace any existing node at the same path. Exact
/// ownership, mode, timestamps, xattrs, hardlinks, and special-node kinds are
/// then restored from the typed entries.
pub fn overlay_payload_entries(
    entries: &[GenerationRootEntry],
    cas: &CasStore,
    destination: &Path,
) -> crate::Result<u64> {
    require_sha256_cas(cas)?;
    require_directory(destination)?;
    let directories = entries
        .iter()
        .filter(|entry| matches!(entry.node.source.kind, PayloadNodeKind::Directory))
        .collect::<Vec<_>>();
    let leaves = entries
        .iter()
        .filter(|entry| !matches!(entry.node.source.kind, PayloadNodeKind::Directory))
        .collect::<Vec<_>>();

    for entry in &directories {
        entry.validate()?;
        let path = destination_path(destination, &entry.path)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                remove_existing_node(&path)?;
                fs::create_dir(&path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&path)?;
            }
            Err(error) => return Err(error.into()),
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }

    for entry in &leaves {
        entry.validate()?;
        let path = destination_path(destination, &entry.path)?;
        remove_existing_node(&path)?;
        create_leaf(entry, cas, destination, &path)?;
    }

    for entry in &leaves {
        apply_resolved_payload_metadata(&destination_path(destination, &entry.path)?, &entry.node)?;
    }
    for entry in directories.iter().rev() {
        let path = destination_path(destination, &entry.path)?;
        apply_resolved_payload_metadata(&path, &entry.node)?;
        sync_directory(&path)?;
    }
    sync_directory(destination)?;
    u64::try_from(entries.len()).map_err(|_| {
        crate::Error::InvalidPath("payload entry count is not representable".to_string())
    })
}

fn remove_existing_node(path: &Path) -> crate::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn require_sha256_cas(cas: &CasStore) -> crate::Result<()> {
    if cas.algorithm() != HashAlgorithm::Sha256 {
        return Err(crate::Error::InvalidPath(format!(
            "generation root manifests require a SHA-256 CAS, found {}",
            cas.algorithm()
        )));
    }
    Ok(())
}

fn prepare_empty_destination(destination: &Path) -> crate::Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                return Err(crate::Error::InvalidPath(format!(
                    "generation materialization destination is not a directory: {}",
                    destination.display()
                )));
            }
            if fs::read_dir(destination)?.next().transpose()?.is_some() {
                return Err(crate::Error::ConflictError(format!(
                    "generation materialization destination is not empty: {}",
                    destination.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(destination)?;
        }
        Err(error) => return Err(error.into()),
    }
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn require_directory(destination: &Path) -> crate::Result<()> {
    let metadata = fs::symlink_metadata(destination)?;
    if metadata.file_type().is_dir() {
        Ok(())
    } else {
        Err(crate::Error::InvalidPath(format!(
            "state materialization destination is not a directory: {}",
            destination.display()
        )))
    }
}

fn materialize_entries(
    entries: &[GenerationRootEntry],
    cas: &CasStore,
    destination: &Path,
) -> crate::Result<()> {
    let directories = entries
        .iter()
        .filter(|entry| matches!(entry.node.source.kind, PayloadNodeKind::Directory))
        .collect::<Vec<_>>();
    let leaves = entries
        .iter()
        .filter(|entry| !matches!(entry.node.source.kind, PayloadNodeKind::Directory))
        .collect::<Vec<_>>();

    for entry in &directories {
        let path = destination_path(destination, &entry.path)?;
        match fs::create_dir(&path) {
            Ok(()) => fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(crate::Error::ConflictError(format!(
                    "manifest path already exists while materializing {}",
                    entry.path
                )));
            }
            Err(error) => return Err(error.into()),
        }
    }

    for entry in &leaves {
        let path = destination_path(destination, &entry.path)?;
        create_leaf(entry, cas, destination, &path)?;
    }

    // File metadata must precede directory metadata because leaf creation
    // changes parent mtimes. Deepest directories are restored first and the
    // selected-root metadata is restored by the caller last.
    for entry in &leaves {
        apply_resolved_payload_metadata(&destination_path(destination, &entry.path)?, &entry.node)?;
    }
    for entry in directories.iter().rev() {
        let path = destination_path(destination, &entry.path)?;
        apply_resolved_payload_metadata(&path, &entry.node)?;
        sync_directory(&path)?;
    }
    Ok(())
}

fn create_leaf(
    entry: &GenerationRootEntry,
    cas: &CasStore,
    destination: &Path,
    path: &Path,
) -> crate::Result<()> {
    match &entry.node.source.kind {
        PayloadNodeKind::Regular { .. } => {
            let content = entry.content.as_ref().ok_or_else(|| {
                crate::Error::InvalidPath(format!(
                    "regular manifest entry has no content authority: {}",
                    entry.path
                ))
            })?;
            let digest = content
                .sha256
                .strip_prefix("sha256:")
                .unwrap_or(&content.sha256);
            let bytes = cas.retrieve(digest).map_err(|error| {
                crate::Error::IoError(format!(
                    "failed to retrieve CAS object {digest} for {}: {error}",
                    entry.path
                ))
            })?;
            if bytes.len() as u64 != content.size {
                return Err(crate::Error::ConflictError(format!(
                    "CAS object {digest} for {} has size {}, expected {}",
                    entry.path,
                    bytes.len(),
                    content.size
                )));
            }
            let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        PayloadNodeKind::Symlink { target } => {
            std::os::unix::fs::symlink(target, path)?;
        }
        PayloadNodeKind::Hardlink { target, .. } => {
            let target = destination_path(destination, target)?;
            fs::hard_link(target, path)?;
        }
        PayloadNodeKind::BlockDevice { major, minor } => {
            create_device(path, libc::S_IFBLK, *major, *minor)?;
        }
        PayloadNodeKind::CharacterDevice { major, minor } => {
            create_device(path, libc::S_IFCHR, *major, *minor)?;
        }
        PayloadNodeKind::Fifo => create_fifo(path)?,
        PayloadNodeKind::Socket => {
            let socket = UnixListener::bind(path)?;
            drop(socket);
        }
        PayloadNodeKind::Directory => unreachable!("directories are materialized first"),
    }
    Ok(())
}

fn create_device(path: &Path, kind: libc::mode_t, major: u64, minor: u64) -> crate::Result<()> {
    let major = libc::c_uint::try_from(major).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "device major {major} is not representable at {}",
            path.display()
        ))
    })?;
    let minor = libc::c_uint::try_from(minor).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "device minor {minor} is not representable at {}",
            path.display()
        ))
    })?;
    let c_path = c_path(path)?;
    let result = unsafe { libc::mknod(c_path.as_ptr(), kind | 0o600, libc::makedev(major, minor)) };
    if result == 0 {
        Ok(())
    } else {
        Err(crate::Error::IoError(format!(
            "failed to create device {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )))
    }
}

fn create_fifo(path: &Path) -> crate::Result<()> {
    let c_path = c_path(path)?;
    let result = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    if result == 0 {
        Ok(())
    } else {
        Err(crate::Error::IoError(format!(
            "failed to create FIFO {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )))
    }
}

/// Restore the complete metadata authority of one already-created payload node.
///
/// This is shared by immutable generation reconstruction and journaled
/// mutable-root application so the two paths cannot drift on ownership,
/// permissions, timestamps, or xattrs.
pub fn apply_resolved_payload_metadata(
    path: &Path,
    node: &ResolvedPayloadNode,
) -> crate::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let uid = u32::try_from(node.uid).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "uid {} is not representable at {}",
            node.uid,
            path.display()
        ))
    })?;
    let gid = u32::try_from(node.gid).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "gid {} is not representable at {}",
            node.gid,
            path.display()
        ))
    })?;
    if metadata.uid() != uid || metadata.gid() != gid {
        let c_path = c_path(path)?;
        let result = unsafe { libc::lchown(c_path.as_ptr(), uid, gid) };
        if result != 0 {
            return Err(crate::Error::IoError(format!(
                "failed to set ownership for {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }
    }

    if matches!(node.source.kind, PayloadNodeKind::Symlink { .. }) {
        if node.source.mode & 0o7777 != 0o777 {
            return Err(crate::Error::NotImplemented(format!(
                "Linux cannot restore symlink mode {:o} at {}",
                node.source.mode & 0o7777,
                path.display()
            )));
        }
    } else {
        fs::set_permissions(path, fs::Permissions::from_mode(node.source.mode & 0o7777))?;
    }

    replace_xattrs(path, &node.source.xattrs)?;
    set_mtime(
        path,
        node.source.mtime.seconds,
        node.source.mtime.nanoseconds,
    )?;
    Ok(())
}

fn replace_xattrs(
    path: &Path,
    expected: &std::collections::BTreeMap<String, Vec<u8>>,
) -> crate::Result<()> {
    let c_path = c_path(path)?;
    let existing = list_xattr_names(&c_path, path)?;
    let expected_names = expected.keys().map(String::as_str).collect::<BTreeSet<_>>();
    for name in existing
        .iter()
        .filter(|name| !expected_names.contains(name.as_str()))
    {
        let c_name = CString::new(name.as_bytes()).expect("validated xattr name");
        let result = unsafe { libc::lremovexattr(c_path.as_ptr(), c_name.as_ptr()) };
        if result != 0 {
            return Err(xattr_error(&format!("remove {name}"), path));
        }
    }
    for (name, value) in expected {
        let c_name = CString::new(name.as_bytes()).map_err(|_| {
            crate::Error::InvalidPath(format!("xattr name contains NUL at {}", path.display()))
        })?;
        let result = unsafe {
            libc::lsetxattr(
                c_path.as_ptr(),
                c_name.as_ptr(),
                value.as_ptr().cast::<libc::c_void>(),
                value.len(),
                0,
            )
        };
        if result != 0 {
            return Err(xattr_error(&format!("set {name}"), path));
        }
    }
    Ok(())
}

fn list_xattr_names(c_path: &CString, path: &Path) -> crate::Result<Vec<String>> {
    let length = unsafe { libc::llistxattr(c_path.as_ptr(), std::ptr::null_mut(), 0) };
    if length < 0 {
        return Err(xattr_error("list", path));
    }
    let length = usize::try_from(length).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "xattr name list is too large at {}",
            path.display()
        ))
    })?;
    let mut bytes = vec![0_u8; length];
    if length != 0 {
        let actual = unsafe {
            libc::llistxattr(
                c_path.as_ptr(),
                bytes.as_mut_ptr().cast::<libc::c_char>(),
                bytes.len(),
            )
        };
        if actual < 0 {
            return Err(xattr_error("list", path));
        }
        bytes.truncate(usize::try_from(actual).map_err(|_| {
            crate::Error::InvalidPath(format!(
                "xattr name list length is invalid at {}",
                path.display()
            ))
        })?);
    }
    bytes
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| {
            std::str::from_utf8(name).map(str::to_string).map_err(|_| {
                crate::Error::NotImplemented(format!(
                    "xattr name is not UTF-8 at {}",
                    path.display()
                ))
            })
        })
        .collect()
}

fn set_mtime(path: &Path, seconds: i64, nanoseconds: u32) -> crate::Result<()> {
    let c_path = c_path(path)?;
    let seconds = libc::time_t::try_from(seconds).map_err(|_| {
        crate::Error::InvalidPath(format!(
            "mtime seconds are not representable at {}",
            path.display()
        ))
    })?;
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: seconds,
            tv_nsec: libc::c_long::from(nanoseconds),
        },
    ];
    // SAFETY: `c_path` is NUL-terminated, `times` contains two initialized
    // entries as required by `utimensat`, and both buffers outlive the call.
    let result = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c_path.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(crate::Error::IoError(format!(
            "failed to set mtime for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )))
    }
}

fn destination_path(destination: &Path, manifest_path: &str) -> crate::Result<PathBuf> {
    let relative = manifest_path.strip_prefix('/').ok_or_else(|| {
        crate::Error::InvalidPath(format!("manifest path is not absolute: {manifest_path:?}"))
    })?;
    Ok(destination.join(relative))
}

fn c_path(path: &Path) -> crate::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        crate::Error::InvalidPath(format!("filesystem path contains NUL: {}", path.display()))
    })
}

fn sync_directory(path: &Path) -> crate::Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn xattr_error(operation: &str, path: &Path) -> crate::Error {
    crate::Error::IoError(format!(
        "failed to {operation} xattrs for {}: {}",
        path.display(),
        std::io::Error::last_os_error()
    ))
}
