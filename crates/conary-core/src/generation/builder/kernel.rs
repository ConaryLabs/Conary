// conary-core/src/generation/builder/kernel.rs

use std::path::{Path, PathBuf};

pub(super) fn collect_boot_kernel_releases(
    boot_root: &Path,
    releases: &mut Vec<String>,
) -> crate::Result<()> {
    let entries = match std::fs::read_dir(boot_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            crate::Error::InvalidPath(format!(
                "boot artifact name under {} is not UTF-8",
                boot_root.display()
            ))
        })?;
        let Some(release) = name.strip_prefix("vmlinuz-") else {
            continue;
        };
        validate_kernel_release(release)?;
        found.push(release.to_string());
    }
    let entries = std::fs::read_dir(boot_root)?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| {
            crate::Error::InvalidPath(format!(
                "boot artifact name under {} is not UTF-8",
                boot_root.display()
            ))
        })?;
        let Some(release) = name
            .strip_prefix("initramfs-")
            .and_then(|name| name.strip_suffix(".img"))
        else {
            continue;
        };
        validate_kernel_release(release)?;
        found.push(release.to_string());
    }
    found.sort();
    for release in found {
        push_unique_release(releases, release);
    }
    Ok(())
}

pub(super) fn collect_module_kernel_releases(
    system_root: &Path,
    releases: &mut Vec<String>,
) -> crate::Result<()> {
    let mut found = Vec::new();
    for modules_root in [
        system_root.join("lib/modules"),
        system_root.join("usr/lib/modules"),
    ] {
        let entries = match std::fs::read_dir(&modules_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let release = entry.file_name().into_string().map_err(|_| {
                crate::Error::InvalidPath(format!(
                    "kernel module release under {} is not UTF-8",
                    modules_root.display()
                ))
            })?;
            validate_kernel_release(&release)?;
            if regular_file_exists(&path.join("vmlinuz")) {
                found.push(release);
            }
        }
    }
    found.sort();
    for release in found {
        push_unique_release(releases, release);
    }
    Ok(())
}

fn validate_kernel_release(release: &str) -> crate::Result<()> {
    if release.is_empty() || release.contains(['/', '\\', '\0']) {
        return Err(crate::Error::InvalidPath(format!(
            "kernel release {release:?} is invalid"
        )));
    }
    Ok(())
}

pub(super) fn push_unique_release(releases: &mut Vec<String>, release: String) {
    if !releases.iter().any(|existing| existing == &release) {
        releases.push(release);
    }
}

pub(super) fn module_kernel_path(system_root: &Path, release: &str) -> Option<PathBuf> {
    kernel_module_dir(system_root, release)
        .map(|(module_dir, _module_dir_arg)| module_dir.join("vmlinuz"))
        .filter(|path| regular_file_exists(path))
}

pub(super) fn kernel_module_dir(
    system_root: &Path,
    release: &str,
) -> Option<(PathBuf, &'static str)> {
    [
        (
            system_root.join("lib/modules").join(release),
            "/lib/modules",
        ),
        (
            system_root.join("usr/lib/modules").join(release),
            "/usr/lib/modules",
        ),
    ]
    .into_iter()
    .find(|(path, _module_dir_arg)| path.is_dir())
}

pub(super) fn regular_file_exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

pub(super) fn system_root_for_boot_root(boot_root: &Path) -> PathBuf {
    if boot_root.file_name().is_some_and(|name| name == "boot") {
        return boot_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
    }

    PathBuf::from("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_candidates_come_only_from_exact_artifact_paths() {
        let root = tempfile::tempdir().unwrap();
        let boot = root.path().join("boot");
        std::fs::create_dir_all(&boot).unwrap();
        std::fs::write(boot.join("vmlinuz-6.19.10"), b"kernel").unwrap();
        std::fs::write(boot.join("initramfs-6.20.0.img"), b"initramfs").unwrap();

        let mut releases = Vec::new();
        collect_boot_kernel_releases(&boot, &mut releases).unwrap();

        assert_eq!(releases, ["6.19.10", "6.20.0"]);
    }
}
