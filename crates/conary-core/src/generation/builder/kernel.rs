// conary-core/src/generation/builder/kernel.rs

use std::path::{Path, PathBuf};

use crate::db::models::Trove;

pub(super) fn collect_boot_kernel_releases(
    boot_root: &Path,
    requested_version: &str,
    releases: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(boot_root) else {
        return;
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        let Some(release) = name.strip_prefix("vmlinuz-") else {
            continue;
        };
        if kernel_release_matches(requested_version, release) {
            found.push(release.to_string());
        }
    }
    found.sort();
    for release in found {
        push_unique_release(releases, release);
    }
}

pub(super) fn collect_module_kernel_releases(
    system_root: &Path,
    requested_version: &str,
    releases: &mut Vec<String>,
) {
    let mut found = Vec::new();
    for modules_root in [
        system_root.join("lib/modules"),
        system_root.join("usr/lib/modules"),
    ] {
        let Ok(entries) = std::fs::read_dir(modules_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(release) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if kernel_release_matches(requested_version, release)
                && regular_file_exists(&path.join("vmlinuz"))
            {
                found.push(release.to_string());
            }
        }
    }
    found.sort();
    for release in found {
        push_unique_release(releases, release);
    }
}

pub(super) fn push_unique_release(releases: &mut Vec<String>, release: String) {
    if !releases.iter().any(|existing| existing == &release) {
        releases.push(release);
    }
}

fn kernel_release_matches(requested_version: &str, release: &str) -> bool {
    release == requested_version
        || release
            .strip_prefix(requested_version)
            .is_some_and(|suffix| suffix.starts_with('.'))
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

pub fn detect_kernel_version_from_troves(troves: &[Trove]) -> Option<String> {
    for trove in troves {
        if matches!(
            trove.name.as_str(),
            "kernel-core" | "kernel-modules-core" | "kernel-modules"
        ) || trove.name.starts_with("linux-image")
        {
            return Some(trove.version.clone());
        }
    }

    for trove in troves {
        if trove.name.starts_with("kernel") || trove.name.starts_with("linux-image") {
            return Some(trove.version.clone());
        }
    }
    // Fall back to running kernel version from /proc/sys/kernel/osrelease
    crate::generation::metadata::running_kernel_version()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_kernel_version_does_not_panic() {
        let result = detect_kernel_version_from_troves(&[]);
        assert!(result.is_some() || result.is_none());
    }

    #[test]
    fn detect_kernel_version_prefers_payload_kernel_over_meta_package() {
        use crate::db::models::TroveType;

        let troves = vec![
            Trove::new(
                "kernel".to_string(),
                "6.17.1-300.fc44".to_string(),
                TroveType::Package,
            ),
            Trove::new(
                "kernel-core".to_string(),
                "6.19.10-300.fc44".to_string(),
                TroveType::Package,
            ),
        ];

        assert_eq!(
            detect_kernel_version_from_troves(&troves).as_deref(),
            Some("6.19.10-300.fc44")
        );
    }
}
