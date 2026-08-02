// conary-core/src/scriptlet/process/device_projection.rs

//! Controlled selected-root device projection for lifecycle execution.

use crate::error::{Error, Result, ScriptletFailureKind};
use nix::errno::Errno;
use nix::fcntl::{OFlag, open, openat};
use nix::sys::stat::{Mode, SFlag, fstat, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use std::os::fd::OwnedFd;
use std::path::Path;

#[derive(Debug)]
pub(super) struct TargetDeviceProjection {
    _mountpoint: TargetDeviceMountpoint,
}

#[derive(Debug)]
struct TargetDeviceMountpoint {
    root_fd: OwnedFd,
    dev_fd: OwnedFd,
    created_dev: bool,
    created_null: bool,
}

impl TargetDeviceProjection {
    pub(super) fn stage(root: &Path) -> Result<Self> {
        let source_fd = open(
            "/dev/null",
            OFlag::O_RDWR | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| projection_error(root, format!("cannot open host /dev/null: {error}")))?;
        let source_stat = fstat(&source_fd).map_err(|error| {
            projection_error(root, format!("cannot inspect host /dev/null: {error}"))
        })?;
        let source_kind = SFlag::from_bits_truncate(source_stat.st_mode);
        if source_kind != SFlag::S_IFCHR
            || libc::major(source_stat.st_rdev) != 1
            || libc::minor(source_stat.st_rdev) != 3
        {
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
        let mut mountpoint = TargetDeviceMountpoint {
            root_fd,
            dev_fd,
            created_dev,
            created_null: false,
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
        mountpoint.created_null = created_null;
        let target_kind = SFlag::from_bits_truncate(
            fstat(&target_fd)
                .map_err(|error| {
                    projection_error(root, format!("cannot inspect /dev/null: {error}"))
                })?
                .st_mode,
        );
        if target_kind != SFlag::S_IFREG && target_kind != SFlag::S_IFCHR {
            return Err(projection_error(
                root,
                "existing /dev/null is neither a regular mountpoint nor a character device",
            ));
        }

        Ok(Self {
            _mountpoint: mountpoint,
        })
    }
}

impl Drop for TargetDeviceMountpoint {
    fn drop(&mut self) {
        if self.created_null {
            let _ = unlinkat(&self.dev_fd, "null", UnlinkatFlags::NoRemoveDir);
        }
        if self.created_dev {
            let _ = unlinkat(&self.root_fd, "dev", UnlinkatFlags::RemoveDir);
        }
    }
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
    use super::TargetDeviceProjection;
    use std::fs;

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
}
