// conary-core/src/generation/builder/initramfs.rs

use std::path::Path;

use super::kernel::{kernel_module_dir, regular_file_exists};

pub(super) const CONARY_DRACUT_MODULE_SETUP: &str =
    include_str!("../../../../../packaging/dracut/90conary/module-setup.sh");
const CONARY_DRACUT_INIT: &str =
    include_str!("../../../../../packaging/dracut/90conary/conary-init.sh");
const CONARY_DRACUT_GENERATOR: &str =
    include_str!("../../../../../packaging/dracut/90conary/conary-generator.sh");
pub(super) const RUNTIME_DRACUT_ADD_MODULES: &str = "conary";
pub(super) const RUNTIME_DRACUT_OMIT_MODULES: &str = "systemd";

pub(super) fn generate_runtime_initramfs(
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
    system_root: &Path,
    release: &str,
    initramfs: &Path,
) -> crate::Result<()> {
    let Some(parent) = initramfs.parent() else {
        return Err(crate::error::Error::InvalidPath(format!(
            "initramfs destination has no parent: {}",
            initramfs.display()
        )));
    };
    std::fs::create_dir_all(parent)?;
    ensure_initramfs_tool_available(cpio, "cpio")?;
    ensure_kernel_module_metadata(depmod, system_root, release)?;
    let (runtime_module_dir, _module_dir_arg) =
        kernel_module_dir(system_root, release).ok_or_else(|| {
            crate::error::Error::NotFound(format!(
                "missing kernel module directory for {release}; expected lib/modules/{release} or usr/lib/modules/{release}"
            ))
        })?;

    let modules_workspace = tempfile::Builder::new()
        .prefix("conary-dracut-")
        .tempdir()
        .map_err(|e| {
            crate::error::Error::IoError(format!("failed to create dracut workspace: {e}"))
        })?;
    prepare_dracut_workspace(modules_workspace.path())?;
    let module_dir = modules_workspace.path().join("modules.d/90conary");
    std::fs::create_dir_all(&module_dir)?;
    write_dracut_module_file(
        &module_dir.join("module-setup.sh"),
        CONARY_DRACUT_MODULE_SETUP,
    )?;
    write_dracut_module_file(&module_dir.join("conary-init.sh"), CONARY_DRACUT_INIT)?;
    write_dracut_module_file(
        &module_dir.join("conary-generator.sh"),
        CONARY_DRACUT_GENERATOR,
    )?;

    let output = std::process::Command::new(dracut)
        .env("dracutbasedir", modules_workspace.path())
        .arg("--force")
        .arg("--no-hostonly")
        // Force dracut's shell init path. The default systemd module alone
        // creates a partial initramfs without the initrd systemd contract.
        .arg("--omit")
        .arg(RUNTIME_DRACUT_OMIT_MODULES)
        .arg("--add")
        .arg(RUNTIME_DRACUT_ADD_MODULES)
        .arg("--sysroot")
        .arg(system_root)
        .arg("--kmoddir")
        .arg(&runtime_module_dir)
        .arg(initramfs)
        .arg(release)
        .output()
        .map_err(|e| {
            crate::error::Error::NotFound(format!(
                "failed to run dracut to generate {} for {release}: {e}",
                initramfs.display()
            ))
        })?;

    if !output.status.success() {
        return Err(crate::error::Error::IoError(format!(
            "dracut failed to generate {} for {release} with status {}:\nstdout:\n{}\nstderr:\n{}",
            initramfs.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

fn ensure_initramfs_tool_available(tool: &Path, name: &str) -> crate::Result<()> {
    match std::process::Command::new(tool).arg("--version").output() {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(crate::error::Error::NotFound(format!(
                "missing required initramfs tool {name} at {}; source images that build runtime generations must include the initramfs toolchain because dracut emits initramfs archives through {name}",
                tool.display()
            )))
        }
        Err(e) => Err(crate::error::Error::IoError(format!(
            "failed to check required initramfs tool {name} at {}: {e}",
            tool.display()
        ))),
    }
}

fn prepare_dracut_workspace(workspace: &Path) -> crate::Result<()> {
    let modules_dir = workspace.join("modules.d");
    std::fs::create_dir_all(&modules_dir)?;

    let system_dracut = Path::new("/usr/lib/dracut");
    if !system_dracut.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(system_dracut)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "modules.d" {
            continue;
        }
        link_or_copy_dracut_entry(&entry.path(), &workspace.join(name))?;
    }

    let system_modules = system_dracut.join("modules.d");
    if system_modules.is_dir() {
        for entry in std::fs::read_dir(system_modules)? {
            let entry = entry?;
            link_or_copy_dracut_entry(&entry.path(), &modules_dir.join(entry.file_name()))?;
        }
    }

    Ok(())
}

fn link_or_copy_dracut_entry(source: &Path, dest: &Path) -> crate::Result<()> {
    if dest.exists() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, dest)?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        if source.is_file() {
            std::fs::copy(source, dest)?;
        }
        Ok(())
    }
}

fn ensure_kernel_module_metadata(
    depmod: &Path,
    system_root: &Path,
    release: &str,
) -> crate::Result<()> {
    let (module_dir, module_dir_arg) = kernel_module_dir(system_root, release).ok_or_else(|| {
        crate::error::Error::NotFound(format!(
            "missing kernel module directory for {release}; expected lib/modules/{release} or usr/lib/modules/{release}"
        ))
    })?;
    let modules_dep = module_dir.join("modules.dep");
    if regular_file_exists(&modules_dep) {
        return Ok(());
    }

    let output = std::process::Command::new(depmod)
        .arg("-b")
        .arg(system_root)
        .arg("-m")
        .arg(module_dir_arg)
        .arg(release)
        .output()
        .map_err(|e| {
            crate::error::Error::NotFound(format!(
                "failed to run depmod for kernel {release} under {}: {e}",
                system_root.display()
            ))
        })?;

    if !output.status.success() {
        return Err(crate::error::Error::IoError(format!(
            "depmod failed for kernel {release} under {} with status {}:\nstdout:\n{}\nstderr:\n{}",
            system_root.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    if !regular_file_exists(&modules_dep) {
        return Err(crate::error::Error::NotFound(format!(
            "depmod completed but did not create {}",
            modules_dep.display()
        )));
    }

    Ok(())
}

fn write_dracut_module_file(path: &Path, contents: &str) -> crate::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
