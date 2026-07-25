// src/commands/ccs/payload_paths.rs

//! CCS payload path normalization and symlink safety.

use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::manifest::FileCapability;
use conary_core::packages::traits::{ExtractedFile, PackageFormat};
use conary_core::payload::PayloadNodeKind;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Component as PathComponent, Path, PathBuf};

const MAX_EXISTING_SYMLINK_DEPTH: usize = 40;

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

fn deployed_mode(mode: u32) -> (u32, bool) {
    let stripped = mode & !0o6000;
    (stripped, stripped != mode)
}

fn is_extracted_symlink(file: &ExtractedFile) -> bool {
    matches!(file.node.kind, PayloadNodeKind::Symlink { .. })
}

fn symlink_target_for_file(file: &ExtractedFile) -> Result<String> {
    if let PayloadNodeKind::Symlink { target } = &file.node.kind {
        return Ok(target.clone());
    }
    anyhow::bail!("payload node {} is not a symlink", file.path)
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

pub(crate) fn normalize_ccs_file_capabilities(
    root_path: &Path,
    file_capabilities: &[FileCapability],
) -> Result<Vec<FileCapability>> {
    file_capabilities
        .iter()
        .map(|capability| {
            capability.validate()?;
            let mut normalized = capability.clone();
            normalized.path = normalize_ccs_package_path(root_path, &capability.path)?;
            Ok(normalized)
        })
        .collect()
}

fn package_deployment_relative_path(root_path: &Path, package_path: &str) -> Result<PathBuf> {
    let relative_path = sanitize_package_relative_path(package_path)?;
    let relative_path = rewrite_standard_usrmerge_root_symlink(&relative_path)?;
    resolve_existing_symlink_ancestors(root_path, &relative_path)
}

fn resolve_existing_symlink_ancestors(root_path: &Path, relative_path: &Path) -> Result<PathBuf> {
    let Some(file_name) = relative_path.file_name() else {
        return Ok(relative_path.to_path_buf());
    };
    let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
    let resolved_parent = resolve_existing_root_relative_path(root_path, parent, relative_path)?;
    Ok(resolved_parent.join(file_name))
}

fn resolve_existing_root_relative_path(
    root_path: &Path,
    relative_path: &Path,
    package_path: &Path,
) -> Result<PathBuf> {
    let mut pending: VecDeque<OsString> = relative_path
        .components()
        .filter_map(|component| match component {
            PathComponent::Normal(part) => Some(part.to_os_string()),
            _ => None,
        })
        .collect();
    let mut resolved = PathBuf::new();
    let mut symlink_depth = 0;

    while let Some(component) = pending.pop_front() {
        resolved.push(&component);
        let candidate = root_path.join(&resolved);
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                symlink_depth += 1;
                if symlink_depth > MAX_EXISTING_SYMLINK_DEPTH {
                    return Err(path_traversal_error(format!(
                        "package path {} resolves through a symlink chain deeper than \
                         {MAX_EXISTING_SYMLINK_DEPTH} at {}",
                        package_path.display(),
                        resolved.display()
                    )));
                }

                let blocker = resolved.clone();
                let target = std::fs::read_link(&candidate)
                    .with_context(|| format!("failed to read symlink {}", candidate.display()))?;
                resolved.pop();
                let (mut redirected, target_relative) = if target.is_absolute() {
                    let canonical_root = std::fs::canonicalize(root_path).with_context(|| {
                        format!(
                            "failed to resolve install root {} while validating package path {}",
                            root_path.display(),
                            package_path.display()
                        )
                    })?;
                    let target_relative = target.strip_prefix(&canonical_root).map_err(|_| {
                        path_traversal_error(format!(
                            "package path {} escapes the install root through symlink {} -> {}",
                            package_path.display(),
                            blocker.display(),
                            target.display()
                        ))
                    })?;
                    (PathBuf::new(), target_relative)
                } else {
                    (resolved.clone(), target.as_path())
                };
                append_root_relative_symlink_target(
                    &mut redirected,
                    target_relative,
                    &target,
                    package_path,
                    &blocker,
                )?;
                redirected.extend(pending);
                pending = redirected
                    .components()
                    .filter_map(|component| match component {
                        PathComponent::Normal(part) => Some(part.to_os_string()),
                        _ => None,
                    })
                    .collect();
                resolved.clear();
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to inspect {}", candidate.display()));
            }
        }
    }

    Ok(resolved)
}

fn append_root_relative_symlink_target(
    resolved: &mut PathBuf,
    target_relative: &Path,
    target_display: &Path,
    package_path: &Path,
    blocker: &Path,
) -> Result<()> {
    for component in target_relative.components() {
        match component {
            PathComponent::CurDir => {}
            PathComponent::Normal(part) => resolved.push(part),
            PathComponent::ParentDir => {
                if !resolved.pop() {
                    return Err(path_traversal_error(format!(
                        "package path {} escapes the install root through symlink {} -> {}",
                        package_path.display(),
                        blocker.display(),
                        target_display.display()
                    )));
                }
            }
            PathComponent::RootDir | PathComponent::Prefix(_) => {
                return Err(path_traversal_error(format!(
                    "package path {} resolves through invalid symlink {} -> {}",
                    package_path.display(),
                    blocker.display(),
                    target_display.display()
                )));
            }
        }
    }

    Ok(())
}

fn path_traversal_error(message: String) -> anyhow::Error {
    anyhow::Error::new(conary_core::Error::PathTraversal(message))
}

fn identical_regular_deployment(existing: &ExtractedFile, current: &ExtractedFile) -> bool {
    if !matches!(existing.node.kind, PayloadNodeKind::Regular { .. })
        || !matches!(current.node.kind, PayloadNodeKind::Regular { .. })
    {
        return false;
    }
    let mut existing_node = existing.node.clone();
    let mut current_node = current.node.clone();
    existing_node.mode = deployed_mode(existing_node.mode).0;
    current_node.mode = deployed_mode(current_node.mode).0;
    existing.content_authority == current.content_authority
        && existing_node == current_node
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
        if deployment.symlink_target.is_none() {
            deployment.file.node.mode = deployed_mode(deployment.file.node.mode).0;
        }
        normalized.push(deployment.file);
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_ccs_extracted_files, normalize_ccs_file_capabilities, normalize_ccs_package_path,
        sanitize_package_relative_path,
    };
    use conary_core::ccs::manifest::FileCapability;
    use conary_core::packages::traits::ExtractedFile;
    use conary_core::payload::{PayloadContentAuthority, PayloadNode};
    use std::path::PathBuf;

    fn file_capability(path: &str) -> FileCapability {
        FileCapability {
            path: path.to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        }
    }

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

    #[test]
    fn normalize_file_capabilities_follows_usrmerge_payload_paths() {
        let root = tempfile::tempdir().unwrap();
        let normalized = normalize_ccs_file_capabilities(
            root.path(),
            &[
                file_capability("/bin/demo"),
                file_capability("/sbin/admin-tool"),
                file_capability("/usr/bin/kept"),
            ],
        )
        .unwrap();

        assert_eq!(normalized[0].path, "/usr/bin/demo");
        assert_eq!(normalized[1].path, "/usr/sbin/admin-tool");
        assert_eq!(normalized[2].path, "/usr/bin/kept");
        assert_eq!(
            normalized[0].capabilities,
            vec!["cap_net_bind_service".to_string()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn normalize_payload_follows_safe_existing_relative_symlink_ancestor() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr/lib")).unwrap();
        std::os::unix::fs::symlink("lib", root.path().join("usr/lib64")).unwrap();

        let normalized = normalize_ccs_extracted_files(
            root.path(),
            vec![ExtractedFile {
                path: "/usr/lib64/libform.so.6".to_string(),
                node: PayloadNode::regular(0o644),
                content: b"fixture".to_vec(),
                content_authority: Some(PayloadContentAuthority {
                    sha256: conary_core::hash::sha256(b"fixture"),
                    size: 7,
                }),
            }],
        )
        .unwrap();

        assert_eq!(normalized[0].path, "/usr/lib/libform.so.6");
    }

    #[cfg(unix)]
    #[test]
    fn normalize_payload_follows_safe_existing_absolute_symlink_ancestor_inside_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr/lib")).unwrap();
        std::os::unix::fs::symlink(root.path().join("usr/lib"), root.path().join("usr/lib64"))
            .unwrap();

        assert_eq!(
            normalize_ccs_package_path(root.path(), "/usr/lib64/libform.so.6").unwrap(),
            "/usr/lib/libform.so.6"
        );
    }

    #[cfg(unix)]
    #[test]
    fn normalize_payload_rejects_absolute_symlink_ancestor_outside_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("usr/lib64")).unwrap();

        let err = normalize_ccs_package_path(root.path(), "/usr/lib64/libform.so.6").unwrap_err();

        assert!(err.to_string().contains("escapes the install root"));
        assert!(err.to_string().contains("usr/lib64"));
    }

    #[cfg(unix)]
    #[test]
    fn normalize_payload_rejects_existing_symlink_ancestor_that_escapes_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr")).unwrap();
        std::os::unix::fs::symlink("../../outside", root.path().join("usr/lib64")).unwrap();

        let err = normalize_ccs_package_path(root.path(), "/usr/lib64/libform.so.6").unwrap_err();

        assert!(err.to_string().contains("escapes the install root"));
        assert!(err.to_string().contains("usr/lib64"));
    }

    #[cfg(unix)]
    #[test]
    fn normalize_payload_rejects_multi_hop_existing_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr")).unwrap();
        std::fs::create_dir_all(root.path().join("opt")).unwrap();
        std::os::unix::fs::symlink("../opt/lib64", root.path().join("usr/lib64")).unwrap();
        std::os::unix::fs::symlink("../../outside", root.path().join("opt/lib64")).unwrap();

        let err = normalize_ccs_package_path(root.path(), "/usr/lib64/libform.so.6").unwrap_err();

        assert!(err.to_string().contains("escapes the install root"));
        assert!(err.to_string().contains("opt/lib64"));
    }

    #[cfg(unix)]
    #[test]
    fn normalize_payload_rejects_existing_symlink_loop() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("usr")).unwrap();
        std::os::unix::fs::symlink("lib", root.path().join("usr/lib64")).unwrap();
        std::os::unix::fs::symlink("lib64", root.path().join("usr/lib")).unwrap();

        let err = normalize_ccs_package_path(root.path(), "/usr/lib64/libform.so.6").unwrap_err();

        assert!(
            err.to_string().contains("symlink chain deeper than 40"),
            "unexpected error: {err:#}"
        );
    }
}
