// src/commands/ccs/payload_paths.rs

//! CCS payload path normalization and symlink safety.

use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::packages::traits::{ExtractedFile, PackageFormat};
use std::collections::{HashMap, HashSet};
use std::path::{Component as PathComponent, Path, PathBuf};

pub(super) fn sanitize_package_relative_path(path: &str) -> Result<PathBuf> {
    let candidate = path.strip_prefix('/').unwrap_or(path);
    let mut normalized = PathBuf::new();

    for component in Path::new(candidate).components() {
        match component {
            PathComponent::CurDir => {}
            PathComponent::Normal(part) => normalized.push(part),
            PathComponent::ParentDir => {
                anyhow::bail!("path traversal detected in package path: {path}")
            }
            PathComponent::RootDir | PathComponent::Prefix(_) => {
                anyhow::bail!("invalid package path component in {path}")
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        anyhow::bail!("empty package path is not allowed");
    }

    Ok(normalized)
}

fn deployed_mode(mode: i32) -> (i32, bool) {
    let stripped = mode & !0o6000;
    (stripped, stripped != mode)
}

fn is_symlink_mode(mode: i32) -> bool {
    (mode & 0o170000) == 0o120000
}

fn is_extracted_symlink(file: &ExtractedFile) -> bool {
    is_symlink_mode(file.mode) || file.symlink_target.is_some()
}

fn symlink_target_for_file(file: &ExtractedFile) -> Result<String> {
    if let Some(target) = &file.symlink_target {
        return Ok(target.clone());
    }

    String::from_utf8(file.content.clone()).context("invalid symlink target in package payload")
}

struct DeploymentFile {
    file: ExtractedFile,
    relative_path: PathBuf,
    symlink_target: Option<String>,
}

fn standard_usrmerge_target(component: &str) -> Option<&'static str> {
    match component {
        "bin" => Some("usr/bin"),
        "sbin" => Some("usr/sbin"),
        "lib" => Some("usr/lib"),
        "lib64" => Some("usr/lib64"),
        _ => None,
    }
}

fn rewrite_standard_usrmerge_root_symlink(relative_path: &Path) -> Result<PathBuf> {
    let mut components = relative_path.components();
    let Some(first) = components.next() else {
        return Ok(relative_path.to_path_buf());
    };
    let first = first.as_os_str().to_str().ok_or_else(|| {
        anyhow::anyhow!(
            "invalid non-UTF8 package path component in {}",
            relative_path.display()
        )
    })?;
    let Some(target) = standard_usrmerge_target(first) else {
        return Ok(relative_path.to_path_buf());
    };

    // Only rewrite descendants such as bin/foo. A package entry that targets
    // the root symlink itself should still collide/fail rather than replace it.
    let remaining: Vec<_> = components.collect();
    if remaining.is_empty() {
        return Ok(relative_path.to_path_buf());
    }

    let mut rewritten = PathBuf::from(target);
    for component in remaining {
        rewritten.push(component.as_os_str());
    }
    Ok(rewritten)
}

fn deployment_path_to_package_path(relative_path: &Path) -> Result<String> {
    let path = relative_path.to_str().ok_or_else(|| {
        anyhow::anyhow!(
            "invalid non-UTF8 package path after normalization: {}",
            relative_path.display()
        )
    })?;
    Ok(format!("/{path}"))
}

pub(crate) fn normalize_ccs_package_path(root_path: &Path, package_path: &str) -> Result<String> {
    let relative_path = package_deployment_relative_path(root_path, package_path)?;
    deployment_path_to_package_path(&relative_path)
}

fn package_deployment_relative_path(_root_path: &Path, package_path: &str) -> Result<PathBuf> {
    let relative_path = sanitize_package_relative_path(package_path)?;
    rewrite_standard_usrmerge_root_symlink(&relative_path)
}

fn identical_regular_deployment(existing: &ExtractedFile, current: &ExtractedFile) -> bool {
    !is_extracted_symlink(existing)
        && !is_extracted_symlink(current)
        && existing.sha256 == current.sha256
        && existing.size == current.size
        && deployed_mode(existing.mode).0 == deployed_mode(current.mode).0
        && existing.content == current.content
}

fn find_symlink_blocker(
    root_path: &Path,
    relative_path: &Path,
    created_symlinks: &HashSet<PathBuf>,
    include_self: bool,
) -> Result<Option<PathBuf>> {
    let mut prefix = PathBuf::new();
    let mut components = relative_path.components().peekable();

    while let Some(component) = components.next() {
        prefix.push(component.as_os_str());

        let is_self = components.peek().is_none();
        if is_self && !include_self {
            break;
        }

        if created_symlinks.contains(&prefix) {
            return Ok(Some(prefix));
        }

        match std::fs::symlink_metadata(root_path.join(&prefix)) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(Some(prefix)),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to inspect {}", root_path.join(&prefix).display())
                });
            }
        }
    }

    Ok(None)
}

fn ensure_no_symlink_ancestor(
    root_path: &Path,
    relative_path: &Path,
    created_symlinks: &HashSet<PathBuf>,
    include_self: bool,
) -> Result<()> {
    if let Some(blocker) =
        find_symlink_blocker(root_path, relative_path, created_symlinks, include_self)?
    {
        return Err(anyhow::Error::new(conary_core::Error::PathTraversal(
            format!(
                "package path {} resolves through symlink {}",
                relative_path.display(),
                blocker.display()
            ),
        )));
    }

    Ok(())
}

pub(crate) fn validate_ccs_payload_paths(
    root_path: &Path,
    ccs_pkg: &CcsPackage,
    selected_component_names: &[String],
) -> Result<()> {
    let selected_component_set: HashSet<&str> = selected_component_names
        .iter()
        .map(String::as_str)
        .collect();
    let selected_file_entries: Vec<_> = ccs_pkg
        .file_entries()
        .iter()
        .filter(|file| selected_component_set.contains(file.component.as_str()))
        .cloned()
        .collect();
    let selected_paths: HashSet<&str> = selected_file_entries
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let extracted_files: Vec<_> = if selected_paths.is_empty() {
        Vec::new()
    } else {
        ccs_pkg
            .extract_file_contents()?
            .into_iter()
            .filter(|file| selected_paths.contains(file.path.as_str()))
            .collect()
    };
    if extracted_files.is_empty() && !selected_file_entries.is_empty() {
        anyhow::bail!(
            "No files matched the selected components: {}",
            selected_component_names.join(", ")
        );
    }

    normalize_ccs_extracted_files(root_path, extracted_files)?;

    Ok(())
}

pub(crate) fn normalize_ccs_extracted_files(
    root_path: &Path,
    extracted_files: Vec<ExtractedFile>,
) -> Result<Vec<ExtractedFile>> {
    let mut deployment_files: Vec<DeploymentFile> = Vec::new();
    let mut seen_indexes: HashMap<PathBuf, usize> = HashMap::new();
    for file in extracted_files {
        let relative_path = package_deployment_relative_path(root_path, &file.path)?;
        let current_is_symlink = is_extracted_symlink(&file);
        let symlink_target = if current_is_symlink {
            Some(symlink_target_for_file(&file)?)
        } else {
            None
        };
        if let Some(existing_index) = seen_indexes.get(&relative_path).copied() {
            let existing = &deployment_files[existing_index].file;
            let existing_is_symlink = is_extracted_symlink(existing);
            if existing_is_symlink || current_is_symlink {
                anyhow::bail!(
                    "symlink deployment path collision detected for {}",
                    relative_path.display()
                );
            }
            if identical_regular_deployment(existing, &file) {
                continue;
            }
            anyhow::bail!(
                "duplicate deployment path detected for {}",
                relative_path.display()
            );
        }
        seen_indexes.insert(relative_path.clone(), deployment_files.len());
        deployment_files.push(DeploymentFile {
            file,
            relative_path,
            symlink_target,
        });
    }

    let created_symlinks: HashSet<PathBuf> = deployment_files
        .iter()
        .filter(|deployment| deployment.symlink_target.is_some())
        .map(|deployment| deployment.relative_path.clone())
        .collect();
    for deployment in &deployment_files {
        let current_is_symlink = deployment.symlink_target.is_some();
        ensure_no_symlink_ancestor(
            root_path,
            &deployment.relative_path,
            &created_symlinks,
            !current_is_symlink,
        )?;
    }

    let mut normalized = Vec::with_capacity(deployment_files.len());
    for mut deployment in deployment_files {
        deployment.file.path = deployment_path_to_package_path(&deployment.relative_path)?;
        if deployment.symlink_target.is_some() {
            deployment.file.symlink_target = deployment.symlink_target;
        } else {
            deployment.file.mode = deployed_mode(deployment.file.mode).0;
        }
        normalized.push(deployment.file);
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::sanitize_package_relative_path;
    use std::path::PathBuf;

    #[test]
    fn sanitize_rejects_path_traversal() {
        let err = sanitize_package_relative_path("../../etc/shadow").unwrap_err();
        assert!(err.to_string().contains("path traversal"));

        let err = sanitize_package_relative_path("/usr/../../../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn sanitize_accepts_normal_paths() {
        assert_eq!(
            sanitize_package_relative_path("/usr/bin/hello").unwrap(),
            PathBuf::from("usr/bin/hello")
        );
        assert_eq!(
            sanitize_package_relative_path("usr/lib/libfoo.so").unwrap(),
            PathBuf::from("usr/lib/libfoo.so")
        );
        assert_eq!(
            sanitize_package_relative_path("/usr/./bin/./hello").unwrap(),
            PathBuf::from("usr/bin/hello")
        );
    }

    #[test]
    fn sanitize_rejects_empty_path() {
        let err = sanitize_package_relative_path("").unwrap_err();
        assert!(err.to_string().contains("empty package path"));
    }
}
