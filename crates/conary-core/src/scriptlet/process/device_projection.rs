// conary-core/src/scriptlet/process/device_projection.rs

//! Controlled selected-root device projection for lifecycle execution.

use crate::error::{Error, Result, ScriptletFailureKind};
use nix::errno::Errno;
use nix::fcntl::{OFlag, open, openat};
use nix::sys::stat::{Mode, SFlag, fstat, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

// Stable Linux UAPI values from <linux/mount.h>. The libc crate does not
// expose these names on every static target that Conary supports.
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 0x0000_0004;
const MOVE_MOUNT_T_EMPTY_PATH: libc::c_uint = 0x0000_0040;

#[derive(Debug)]
pub(super) struct TargetDeviceProjection {
    _mountpoint: TargetDeviceMountpoint,
    plan: TargetDeviceProjectionPlan,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TargetDeviceProjectionPlan {
    root: FileIdentity,
    dev: FileIdentity,
    target: FileIdentity,
}

#[derive(Debug)]
struct TargetDeviceMountpoint {
    root_fd: OwnedFd,
    dev_fd: OwnedFd,
    created_dev: Option<FileIdentity>,
    created_null: Option<FileIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: libc::dev_t,
    inode: libc::ino_t,
}

impl TargetDeviceProjection {
    pub(super) fn stage(root: &Path) -> Result<Self> {
        let source_fd = open(
            "/dev/null",
            OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| projection_error(root, format!("cannot open host /dev/null: {error}")))?;
        let source_stat = fstat(&source_fd).map_err(|error| {
            projection_error(root, format!("cannot inspect host /dev/null: {error}"))
        })?;
        if !is_linux_null_device(&source_stat) {
            return Err(projection_error(
                root,
                "host /dev/null is not the Linux null character device (1:3)",
            ));
        }

        let root_fd = open(
            root,
            OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| projection_error(root, format!("cannot open root: {error}")))?;
        let root_identity = file_identity(&root_fd).map_err(|error| {
            projection_error(root, format!("cannot inspect selected root: {error}"))
        })?;

        let (dev_fd, created_dev) = match openat(
            &root_fd,
            "dev",
            OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => (fd, false),
            Err(Errno::ENOENT) => {
                mkdirat(&root_fd, "dev", Mode::from_bits_truncate(0o755)).map_err(|error| {
                    projection_error(root, format!("cannot create /dev: {error}"))
                })?;
                let fd = match openat(
                    &root_fd,
                    "dev",
                    OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(fd) => fd,
                    Err(error) => {
                        let _ = unlinkat(&root_fd, "dev", UnlinkatFlags::RemoveDir);
                        return Err(projection_error(
                            root,
                            format!("cannot reopen created /dev: {error}"),
                        ));
                    }
                };
                (fd, true)
            }
            Err(error) => {
                return Err(projection_error(
                    root,
                    format!("cannot open /dev without following links: {error}"),
                ));
            }
        };
        let created_dev = created_dev
            .then(|| file_identity(&dev_fd))
            .transpose()
            .map_err(|error| {
                projection_error(root, format!("cannot inspect created /dev: {error}"))
            })?;
        let dev_identity = file_identity(&dev_fd).map_err(|error| {
            projection_error(root, format!("cannot inspect selected-root /dev: {error}"))
        })?;
        let mut mountpoint = TargetDeviceMountpoint {
            root_fd,
            dev_fd,
            created_dev,
            created_null: None,
        };

        let (target_fd, created_null) = match openat(
            &mountpoint.dev_fd,
            "null",
            OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => (fd, false),
            Err(Errno::ENOENT) => {
                let fd = openat(
                    &mountpoint.dev_fd,
                    "null",
                    OFlag::O_WRONLY
                        | OFlag::O_CREAT
                        | OFlag::O_EXCL
                        | OFlag::O_NOFOLLOW
                        | OFlag::O_CLOEXEC,
                    Mode::from_bits_truncate(0o600),
                )
                .map_err(|error| {
                    projection_error(root, format!("cannot create /dev/null mountpoint: {error}"))
                })?;
                (fd, true)
            }
            Err(error) => {
                return Err(projection_error(
                    root,
                    format!("cannot open /dev/null without following links: {error}"),
                ));
            }
        };
        let target_stat = fstat(&target_fd).map_err(|error| {
            projection_error(root, format!("cannot inspect /dev/null: {error}"))
        })?;
        let target_kind = SFlag::from_bits_truncate(target_stat.st_mode);
        if target_kind != SFlag::S_IFREG && target_kind != SFlag::S_IFCHR {
            return Err(projection_error(
                root,
                "existing /dev/null is neither a regular mountpoint nor a character device",
            ));
        }
        if created_null {
            mountpoint.created_null = Some(FileIdentity::from_stat(&target_stat));
        }

        Ok(Self {
            _mountpoint: mountpoint,
            plan: TargetDeviceProjectionPlan {
                root: root_identity,
                dev: dev_identity,
                target: FileIdentity::from_stat(&target_stat),
            },
        })
    }

    pub(super) fn plan(&self) -> TargetDeviceProjectionPlan {
        self.plan
    }
}

pub(super) fn attach_device_projection(
    root: &Path,
    plan: TargetDeviceProjectionPlan,
) -> io::Result<()> {
    let source = open(
        "/dev/null",
        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let source_stat = fstat(&source).map_err(io::Error::from)?;
    if !is_linux_null_device(&source_stat) {
        return Err(io::Error::other(
            "host /dev/null is not the Linux null character device (1:3)",
        ));
    }

    let root_fd = open(
        root,
        OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    require_identity(&root_fd, plan.root, "selected root changed after staging")?;
    let dev_fd = openat(
        &root_fd,
        "dev",
        OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    require_identity(
        &dev_fd,
        plan.dev,
        "selected-root /dev changed after staging",
    )?;
    let target = openat(
        &dev_fd,
        "null",
        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    require_identity(
        &target,
        plan.target,
        "selected-root /dev/null changed after staging",
    )?;

    let empty = c"";
    // Safety: open_tree returns a new descriptor or -1 and reads only the
    // supplied empty C string. Both input descriptors remain owned by the
    // pre-exec closure.
    let tree_fd = unsafe {
        libc::syscall(
            libc::SYS_open_tree,
            source.as_raw_fd(),
            empty.as_ptr(),
            libc::OPEN_TREE_CLONE | libc::OPEN_TREE_CLOEXEC | libc::AT_EMPTY_PATH as libc::c_uint,
        )
    };
    if tree_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // Safety: the successful syscall returned a new owned descriptor.
    let tree_fd = unsafe { OwnedFd::from_raw_fd(tree_fd as libc::c_int) };

    // Safety: move_mount reads only the supplied empty C strings and attaches
    // the detached tree to the exact target descriptor. The descriptor owners
    // outlive the syscall.
    let result = unsafe {
        libc::syscall(
            libc::SYS_move_mount,
            tree_fd.as_raw_fd(),
            empty.as_ptr(),
            target.as_raw_fd(),
            empty.as_ptr(),
            MOVE_MOUNT_F_EMPTY_PATH | MOVE_MOUNT_T_EMPTY_PATH,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }

    require_identity(
        &root_fd,
        plan.root,
        "selected root changed during projection",
    )?;
    let current_dev = openat(
        &root_fd,
        "dev",
        OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    require_identity(
        &current_dev,
        plan.dev,
        "selected-root /dev changed during projection",
    )?;
    let current_target = openat(
        &current_dev,
        "null",
        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let current_stat = fstat(&current_target).map_err(io::Error::from)?;
    if !is_linux_null_device(&current_stat) {
        return Err(io::Error::other(
            "selected-root /dev/null does not expose the projected Linux null device",
        ));
    }
    Ok(())
}

impl Drop for TargetDeviceMountpoint {
    fn drop(&mut self) {
        if self
            .created_null
            .is_some_and(|identity| entry_has_identity(&self.dev_fd, "null", false, identity))
        {
            let _ = unlinkat(&self.dev_fd, "null", UnlinkatFlags::NoRemoveDir);
        }
        if self
            .created_dev
            .is_some_and(|identity| entry_has_identity(&self.root_fd, "dev", true, identity))
        {
            let _ = unlinkat(&self.root_fd, "dev", UnlinkatFlags::RemoveDir);
        }
    }
}

impl FileIdentity {
    fn from_stat(stat: &libc::stat) -> Self {
        Self {
            device: stat.st_dev,
            inode: stat.st_ino,
        }
    }
}

fn file_identity(fd: &OwnedFd) -> nix::Result<FileIdentity> {
    fstat(fd).map(|stat| FileIdentity::from_stat(&stat))
}

fn require_identity(fd: &OwnedFd, expected: FileIdentity, detail: &str) -> io::Result<()> {
    let actual = file_identity(fd).map_err(io::Error::from)?;
    if actual != expected {
        return Err(io::Error::other(detail));
    }
    Ok(())
}

fn is_linux_null_device(stat: &libc::stat) -> bool {
    SFlag::from_bits_truncate(stat.st_mode) == SFlag::S_IFCHR
        && libc::major(stat.st_rdev) == 1
        && libc::minor(stat.st_rdev) == 3
}

fn entry_has_identity(
    parent: &OwnedFd,
    name: &str,
    directory: bool,
    expected: FileIdentity,
) -> bool {
    let mut flags = OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    if directory {
        flags |= OFlag::O_DIRECTORY;
    }
    openat(parent, name, flags, Mode::empty())
        .and_then(|fd| file_identity(&fd))
        .is_ok_and(|identity| identity == expected)
}

fn projection_error(root: &Path, detail: impl std::fmt::Display) -> Error {
    Error::scriptlet(
        ScriptletFailureKind::SandboxSetupUnavailable,
        format!(
            "cannot prepare controlled /dev/null projection for selected root {}: {detail}",
            root.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::{TargetDeviceProjection, attach_device_projection};
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn removes_only_mountpoints_created_for_the_projection() {
        let empty_root = tempfile::tempdir().unwrap();
        {
            let _projection = TargetDeviceProjection::stage(empty_root.path()).unwrap();
            assert!(empty_root.path().join("dev/null").is_file());
        }
        assert!(!empty_root.path().join("dev").exists());

        let populated_root = tempfile::tempdir().unwrap();
        fs::create_dir(populated_root.path().join("dev")).unwrap();
        fs::write(
            populated_root.path().join("dev/null"),
            b"existing mountpoint",
        )
        .unwrap();
        {
            let _projection = TargetDeviceProjection::stage(populated_root.path()).unwrap();
        }
        assert_eq!(
            fs::read(populated_root.path().join("dev/null")).unwrap(),
            b"existing mountpoint"
        );
    }

    #[test]
    fn refuses_selected_root_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("dev")).unwrap();

        let error = TargetDeviceProjection::stage(root.path())
            .expect_err("device projection must reject a selected-root /dev symlink");

        assert!(
            error
                .to_string()
                .contains("controlled /dev/null projection"),
            "unexpected error: {error}"
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), b"outside sentinel\n").unwrap();
        fs::create_dir(root.path().join("dev")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("dev/null")).unwrap();

        let error = TargetDeviceProjection::stage(root.path())
            .expect_err("device projection must reject a selected-root /dev/null symlink");

        assert!(
            error
                .to_string()
                .contains("controlled /dev/null projection"),
            "unexpected error: {error}"
        );
        assert_eq!(fs::read(outside.path()).unwrap(), b"outside sentinel\n");
    }

    #[test]
    fn pins_mount_target_and_preserves_replacement_during_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), b"outside sentinel\n").unwrap();

        let projection = TargetDeviceProjection::stage(root.path()).unwrap();
        let pinned_identity = projection.plan.target;
        let dev = root.path().join("dev");
        fs::rename(dev.join("null"), dev.join("original-null")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dev.join("null")).unwrap();

        let current_metadata = fs::metadata(dev.join("null")).unwrap();
        assert_ne!(pinned_identity.inode, current_metadata.ino());

        drop(projection);

        assert!(dev.join("null").is_symlink());
        assert_eq!(fs::read(outside.path()).unwrap(), b"outside sentinel\n");
    }

    #[test]
    fn refuses_replaced_mount_target_before_projection() {
        let root = tempfile::tempdir().unwrap();
        let projection = TargetDeviceProjection::stage(root.path()).unwrap();
        let plan = projection.plan();
        let dev = root.path().join("dev");
        fs::rename(dev.join("null"), dev.join("original-null")).unwrap();
        fs::write(dev.join("null"), b"replacement").unwrap();

        let error = attach_device_projection(root.path(), plan)
            .expect_err("projection must reject a target replaced after staging");

        assert_eq!(
            error.to_string(),
            "selected-root /dev/null changed after staging"
        );
    }
}
