// conary-core/src/scriptlet/lifecycle_bridge/staging.rs

use crate::error::{Error, Result, ScriptletFailureKind};
use nix::errno::Errno;
use nix::fcntl::{OFlag, open, openat};
use nix::sys::stat::{Mode, fstat, mkdirat};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

pub(super) struct BridgeTargetStaging {
    root: PathBuf,
    created: Vec<CreatedPath>,
    cleaned: bool,
}

struct CreatedPath {
    path: PathBuf,
    device: u64,
    inode: u64,
    directory: bool,
}

impl BridgeTargetStaging {
    pub(super) fn new(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root).map_err(staging_error)?;
        if root == Path::new("/") {
            return Err(staging_contract_error(
                "lifecycle bridge cannot stage against the host root",
            ));
        }
        Ok(Self {
            root,
            created: Vec::new(),
            cleaned: false,
        })
    }

    pub(super) fn prepare(&mut self, target: &Path) -> Result<PathBuf> {
        let requested = validated_target(&self.root, target)?;
        match fs::symlink_metadata(&requested) {
            Ok(_) => return resolve_existing(&self.root, &requested),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(staging_error(error)),
        }
        let resolved = resolve_missing_target(&self.root, &requested)?;
        self.create_missing_target(&resolved)?;
        Ok(resolved)
    }

    pub(super) fn cleanup(&mut self) -> Result<()> {
        if self.cleaned {
            return Ok(());
        }
        self.cleaned = true;
        let mut first_error = None;
        for created in self.created.iter().rev() {
            let result = cleanup_created(created);
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    fn create_missing_target(&mut self, target: &Path) -> Result<()> {
        let relative = target.strip_prefix(&self.root).map_err(|_| {
            staging_contract_error(format!(
                "resolved lifecycle bridge target {} escaped {}",
                target.display(),
                self.root.display()
            ))
        })?;
        let components = relative
            .components()
            .map(|component| match component {
                Component::Normal(value) => Ok(value.to_os_string()),
                _ => Err(staging_contract_error("bridge target is not canonical")),
            })
            .collect::<Result<Vec<_>>>()?;
        let (file_name, directories) = components
            .split_last()
            .ok_or_else(|| staging_contract_error("bridge target has no file name"))?;
        let mut relative_directory = PathBuf::new();
        let mut directory_fd = open(
            &self.root,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(staging_error)?;

        for component in directories {
            relative_directory.push(component);
            let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
            directory_fd = match openat(&directory_fd, component.as_os_str(), flags, Mode::empty())
            {
                Ok(fd) => fd,
                Err(Errno::ENOENT) => {
                    mkdirat(
                        &directory_fd,
                        component.as_os_str(),
                        Mode::from_bits_truncate(0o755),
                    )
                    .map_err(staging_error)?;
                    let fd = openat(&directory_fd, component.as_os_str(), flags, Mode::empty())
                        .map_err(staging_error)?;
                    let stat = fstat(&fd).map_err(staging_error)?;
                    self.created.push(CreatedPath {
                        path: self.root.join(&relative_directory),
                        device: stat.st_dev,
                        inode: stat.st_ino,
                        directory: true,
                    });
                    fd
                }
                Err(error) => return Err(staging_error(error)),
            };
        }

        let file_fd = openat(
            &directory_fd,
            file_name.as_os_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(staging_error)?;
        let stat = fstat(&file_fd).map_err(staging_error)?;
        self.created.push(CreatedPath {
            path: target.to_path_buf(),
            device: stat.st_dev,
            inode: stat.st_ino,
            directory: false,
        });
        Ok(())
    }
}

impl Drop for BridgeTargetStaging {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn validated_target(root: &Path, target: &Path) -> Result<PathBuf> {
    if !target.is_absolute() {
        return Err(staging_contract_error(format!(
            "lifecycle bridge target must be absolute: {}",
            target.display()
        )));
    }
    let mut relative = PathBuf::new();
    for component in target.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(value) => relative.push(value),
            _ => {
                return Err(staging_contract_error(format!(
                    "lifecycle bridge target is not canonical: {}",
                    target.display()
                )));
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(staging_contract_error(
            "lifecycle bridge target cannot be the selected-root directory",
        ));
    }
    Ok(root.join(relative))
}

fn resolve_existing(root: &Path, requested: &Path) -> Result<PathBuf> {
    let resolved = fs::canonicalize(requested).map_err(staging_error)?;
    ensure_in_root(root, &resolved)?;
    let metadata = fs::metadata(&resolved).map_err(staging_error)?;
    if !metadata.file_type().is_file() {
        return Err(staging_contract_error(format!(
            "lifecycle bridge target {} is not a regular file",
            requested.display()
        )));
    }
    Ok(resolved)
}

fn resolve_missing_target(root: &Path, requested: &Path) -> Result<PathBuf> {
    let mut ancestor = requested.to_path_buf();
    let mut missing = Vec::<OsString>::new();
    loop {
        match fs::symlink_metadata(&ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor.file_name().ok_or_else(|| {
                    staging_contract_error("bridge target has no existing ancestor")
                })?;
                missing.push(name.to_os_string());
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| {
                        staging_contract_error("bridge target has no existing ancestor")
                    })?
                    .to_path_buf();
            }
            Err(error) => return Err(staging_error(error)),
        }
    }
    let mut resolved = fs::canonicalize(&ancestor).map_err(staging_error)?;
    ensure_in_root(root, &resolved)?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    ensure_in_root(root, &resolved)?;
    Ok(resolved)
}

fn ensure_in_root(root: &Path, path: &Path) -> Result<()> {
    if path == root || path.starts_with(root) {
        Ok(())
    } else {
        Err(staging_contract_error(format!(
            "lifecycle bridge target {} escapes selected root {}",
            path.display(),
            root.display()
        )))
    }
}

fn cleanup_created(created: &CreatedPath) -> Result<()> {
    let metadata = match fs::symlink_metadata(&created.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(staging_error(error)),
    };
    if metadata.dev() != created.device || metadata.ino() != created.inode {
        return Err(staging_contract_error(format!(
            "lifecycle bridge staging path {} changed before cleanup",
            created.path.display()
        )));
    }
    if created.directory {
        fs::remove_dir(&created.path).map_err(staging_error)
    } else {
        fs::remove_file(&created.path).map_err(staging_error)
    }
}

fn staging_error(error: impl std::fmt::Display) -> Error {
    Error::scriptlet(
        ScriptletFailureKind::ProcessSetupFailed,
        format!("lifecycle bridge staging failed: {error}"),
    )
}

fn staging_contract_error(message: impl Into<String>) -> Error {
    Error::scriptlet(ScriptletFailureKind::ContractViolation, message.into())
}
