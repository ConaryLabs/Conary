// conary-core/src/scriptlet/rpm_runtime/target.rs
//! Symlink-aware path resolution confined to an RPM install target.

use anyhow::{Result, bail};
use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};

const MAX_ROOT_SYMLINKS: usize = 64;

pub(super) fn resolve_guest_path(root: &Path, cwd: &str, guest: &str) -> Result<PathBuf> {
    if guest.contains('\0') {
        bail!("RpmTargetPathError: path contains NUL");
    }
    let mut pending = VecDeque::new();
    if !guest.starts_with('/') {
        pending.extend(guest_components(Path::new(cwd))?);
    }
    pending.extend(guest_components(Path::new(guest))?);

    let mut resolved = root.to_path_buf();
    let mut symlinks = 0usize;
    while let Some(component) = pending.pop_front() {
        match component {
            GuestComponent::Root => resolved = root.to_path_buf(),
            GuestComponent::Parent => {
                if resolved != root {
                    resolved.pop();
                }
            }
            GuestComponent::Normal(component) => {
                let candidate = resolved.join(component);
                let metadata = match std::fs::symlink_metadata(&candidate) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        resolved = candidate;
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                if !metadata.file_type().is_symlink() {
                    resolved = candidate;
                    continue;
                }
                symlinks += 1;
                if symlinks > MAX_ROOT_SYMLINKS {
                    bail!("RpmTargetPathError: symlink depth exceeds {MAX_ROOT_SYMLINKS}");
                }
                let target = std::fs::read_link(&candidate)?;
                if target.is_absolute() {
                    resolved = root.to_path_buf();
                }
                let mut target_components = guest_components(&target)?;
                target_components.append(&mut pending);
                pending = target_components;
            }
        }
    }
    Ok(resolved)
}

pub(super) fn guest_path(root: &Path, cwd: &str, guest: &str) -> Result<String> {
    let host = resolve_guest_path(root, cwd, guest)?;
    let relative = host.strip_prefix(root).map_err(|_| {
        anyhow::anyhow!(
            "RpmTargetPathError: resolved path {} escaped target {}",
            host.display(),
            root.display()
        )
    })?;
    let rendered = relative.to_string_lossy();
    Ok(if rendered.is_empty() {
        "/".to_string()
    } else {
        format!("/{rendered}")
    })
}

enum GuestComponent {
    Root,
    Parent,
    Normal(std::ffi::OsString),
}

fn guest_components(path: &Path) -> Result<VecDeque<GuestComponent>> {
    let mut components = VecDeque::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => {
                bail!("RpmTargetPathError: platform path prefixes are invalid")
            }
            Component::RootDir => components.push_back(GuestComponent::Root),
            Component::CurDir => {}
            Component::ParentDir => components.push_back(GuestComponent::Parent),
            Component::Normal(component) => {
                components.push_back(GuestComponent::Normal(component.to_os_string()));
            }
        }
    }
    Ok(components)
}

#[cfg(test)]
mod tests {
    use super::{guest_path, resolve_guest_path};

    #[test]
    fn confines_parent_components_and_absolute_symlink_targets() {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir_all(root.path().join("usr/lib")).expect("directories");
        std::os::unix::fs::symlink("/usr/lib", root.path().join("lib")).expect("symlink");

        assert_eq!(
            resolve_guest_path(root.path(), "/", "/lib/demo").expect("path"),
            root.path().join("usr/lib/demo")
        );
        assert_eq!(
            guest_path(root.path(), "/usr/lib", "../../../etc/demo").expect("guest"),
            "/etc/demo"
        );
    }
}
