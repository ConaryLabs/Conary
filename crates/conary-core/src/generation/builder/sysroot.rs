// conary-core/src/generation/builder/sysroot.rs

use std::path::Path;

use super::{FileEntryRef, SymlinkEntryRef, runtime_inputs};
use crate::generation::metadata::ROOT_SYMLINKS;

pub(super) fn materialize_runtime_generation_sysroot(
    runtime_inputs: &runtime_inputs::RuntimeGenerationInputs,
    objects_dir: &Path,
    artifact_root: &Path,
) -> crate::Result<tempfile::TempDir> {
    let sysroot = tempfile::Builder::new()
        .prefix(".generation-sysroot-")
        .tempdir_in(artifact_root)
        .map_err(|e| {
            crate::error::Error::IoError(format!(
                "failed to create temporary generation sysroot under {}: {e}",
                artifact_root.display()
            ))
        })?;

    for file in &runtime_inputs.file_refs {
        materialize_runtime_regular_file(sysroot.path(), objects_dir, file)?;
    }
    for symlink in &runtime_inputs.symlink_refs {
        materialize_runtime_symlink(sysroot.path(), symlink)?;
    }
    materialize_root_symlinks(sysroot.path())?;
    materialize_runtime_sysroot_base_dirs(sysroot.path())?;

    Ok(sysroot)
}

fn materialize_runtime_regular_file(
    sysroot: &Path,
    objects_dir: &Path,
    file: &FileEntryRef,
) -> crate::Result<()> {
    let rel_path = relative_runtime_path(&file.path)?;
    let dest = sysroot.join(rel_path);
    let source = crate::filesystem::object_path(objects_dir, &file.sha256_hash)?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match std::fs::hard_link(&source, &dest) {
        Ok(()) => Ok(()),
        Err(_) => std::fs::copy(&source, &dest)
            .map(|_| ())
            .map_err(crate::error::Error::Io),
    }
}

fn materialize_runtime_symlink(sysroot: &Path, symlink: &SymlinkEntryRef) -> crate::Result<()> {
    let rel_path = relative_runtime_path(&symlink.path)?;
    let dest = sysroot.join(rel_path);

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if dest.exists() || dest.is_symlink() {
        std::fs::remove_file(&dest)?;
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&symlink.target, &dest)?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        Err(crate::error::Error::NotImplemented(
            "runtime generation sysroot materialization requires Unix symlinks".to_string(),
        ))
    }
}

fn materialize_root_symlinks(sysroot: &Path) -> crate::Result<()> {
    for (link, target) in ROOT_SYMLINKS {
        let dest = sysroot.join(link);
        if dest.exists() || dest.is_symlink() {
            continue;
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, dest)?;
        }

        #[cfg(not(unix))]
        {
            return Err(crate::error::Error::NotImplemented(
                "runtime generation sysroot materialization requires Unix symlinks".to_string(),
            ));
        }
    }
    Ok(())
}

fn materialize_runtime_sysroot_base_dirs(sysroot: &Path) -> crate::Result<()> {
    for dir in ["dev", "proc", "run", "sys", "tmp", "var", "var/tmp"] {
        std::fs::create_dir_all(sysroot.join(dir))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        for dir in ["tmp", "var/tmp"] {
            std::fs::set_permissions(sysroot.join(dir), std::fs::Permissions::from_mode(0o1777))?;
        }
    }

    Ok(())
}

fn relative_runtime_path(path: &str) -> crate::Result<&Path> {
    let rel = path.strip_prefix('/').ok_or_else(|| {
        crate::error::Error::InvalidPath(format!(
            "runtime generation path must be absolute: {path}"
        ))
    })?;
    if rel.is_empty() || rel.split('/').any(|component| component == "..") {
        return Err(crate::error::Error::InvalidPath(format!(
            "runtime generation path escapes root: {path}"
        )));
    }
    Ok(Path::new(rel))
}

pub(super) fn runtime_generation_architecture() -> crate::Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64"),
        "aarch64" => Err(crate::error::Error::NotImplemented(
            "aarch64 generation export boot assets are reserved but not implemented".to_string(),
        )),
        "riscv64" => Err(crate::error::Error::NotImplemented(
            "riscv64 generation export boot assets are reserved but not implemented".to_string(),
        )),
        other => Err(crate::error::Error::NotImplemented(format!(
            "unsupported runtime architecture for generation export: {other}"
        ))),
    }
}
