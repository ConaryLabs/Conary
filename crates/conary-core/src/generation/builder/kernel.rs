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
    boot_root: &Path,
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
            if regular_file_exists(&path.join("vmlinuz"))
                || solus_kernel_path(boot_root, &release).is_some()
            {
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

/// Resolve Solus's exact kernel ABI pairing from a module release.
///
/// A module release such as `7.1.7-351.current` maps to the kernel artifact
/// `com.solus-project.current.7.1.7-351`. The module directory remains release
/// authority; a boot artifact without a matching module release is never a
/// candidate.
pub(super) fn solus_kernel_path(boot_root: &Path, release: &str) -> Option<PathBuf> {
    let (version_release, flavor) = release.rsplit_once('.')?;
    if version_release.is_empty()
        || flavor.is_empty()
        || !flavor
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }
    let path = boot_root.join(format!("com.solus-project.{flavor}.{version_release}"));
    regular_file_exists(&path).then_some(path)
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

pub(super) fn system_root_for_boot_root(boot_root: &Path) -> crate::Result<PathBuf> {
    if !boot_root.is_absolute() || !boot_root.file_name().is_some_and(|name| name == "boot") {
        return Err(crate::Error::InvalidPath(format!(
            "generation boot root {} must be an absolute path naming the target root's boot directory",
            boot_root.display()
        )));
    }

    let canonical_boot = std::fs::canonicalize(boot_root).map_err(|error| {
        crate::Error::InvalidPath(format!(
            "generation boot root {} cannot be resolved: {error}",
            boot_root.display()
        ))
    })?;
    let system_root = canonical_boot.parent().ok_or_else(|| {
        crate::Error::InvalidPath(format!(
            "generation boot root {} has no target system root",
            boot_root.display()
        ))
    })?;
    if boot_root != Path::new("/boot") && system_root == Path::new("/") {
        return Err(crate::Error::InvalidPath(format!(
            "custom generation boot root {} resolves to the live system /boot",
            boot_root.display()
        )));
    }

    Ok(system_root.to_path_buf())
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

    #[test]
    fn solus_kernel_requires_exact_module_release_pair() {
        let root = tempfile::tempdir().unwrap();
        let boot = root.path().join("boot");
        let modules = root.path().join("lib/modules/7.1.7-351.current");
        std::fs::create_dir_all(&boot).unwrap();
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(boot.join("com.solus-project.current.7.1.7-351"), b"kernel").unwrap();
        std::fs::write(
            boot.join("com.solus-project.current.6.18.21-333"),
            b"stale kernel",
        )
        .unwrap();

        let mut releases = Vec::new();
        collect_module_kernel_releases(root.path(), &boot, &mut releases).unwrap();

        assert_eq!(releases, ["7.1.7-351.current"]);
        assert_eq!(
            solus_kernel_path(&boot, &releases[0]),
            Some(boot.join("com.solus-project.current.7.1.7-351"))
        );
    }

    #[test]
    fn boot_root_never_defaults_an_ambiguous_path_to_the_live_system() {
        let root = tempfile::tempdir().unwrap();

        let error = system_root_for_boot_root(&root.path().join("boot-assets"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("naming the target root's boot directory"));
    }

    #[cfg(unix)]
    #[test]
    fn custom_boot_root_cannot_resolve_through_a_symlink_to_live_boot() {
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/", root.path().join("target")).unwrap();
        let disguised_live_boot = root.path().join("target/boot");

        let error = system_root_for_boot_root(&disguised_live_boot)
            .unwrap_err()
            .to_string();

        assert!(error.contains("resolves to the live system /boot"));
    }
}
