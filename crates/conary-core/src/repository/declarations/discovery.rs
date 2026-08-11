// conary-core/src/repository/declarations/discovery.rs

//! Selected-root discovery for native repository declarations.

use super::alpm::AlpmConfigSet;
use super::apt::{AptSourceDocument, AptSyntax};
use super::dnf::DnfRepoDocument;
use super::zypper::{ZypperRepoDocument, ZypperServiceDocument};
use super::{DeclarationEcosystem, DeclarationError, DeclarationErrorKind, Result};
use crate::filesystem::path::safe_join;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const APT_DIRECTORY: &str = "/etc/apt/sources.list.d";
const DNF_DIRECTORIES: &[&str] = &[
    "/etc/yum.repos.d",
    "/etc/distro.repos.d",
    "/usr/share/dnf5/repos.d",
];
const ZYPPER_REPOSITORY_DIRECTORY: &str = "/etc/zypp/repos.d";
const ZYPPER_SERVICE_DIRECTORY: &str = "/etc/zypp/services.d";
const ALPM_CONFIG: &str = "/etc/pacman.conf";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveredRepositoryDeclarations {
    pub root: PathBuf,
    pub apt: Vec<AptSourceDocument>,
    pub dnf: Vec<DnfRepoDocument>,
    pub zypper_repositories: Vec<ZypperRepoDocument>,
    pub zypper_services: Vec<ZypperServiceDocument>,
    pub alpm: Option<AlpmConfigSet>,
}

/// Discover only inside an explicit selected root; no native package-manager
/// binary or database is read or invoked.
pub fn discover_selected_root(root: impl AsRef<Path>) -> Result<DiscoveredRepositoryDeclarations> {
    let root = root.as_ref();
    let apt = discover_apt(root)?;
    let dnf = discover_dnf(root)?;
    let zypper_repositories = discover_zypper_repositories(root)?;
    let zypper_services = discover_zypper_services(root)?;
    let alpm = if selected_exists(root, Path::new(ALPM_CONFIG), DeclarationEcosystem::Alpm)? {
        Some(AlpmConfigSet::parse_selected_root(
            root,
            Path::new(ALPM_CONFIG),
        )?)
    } else {
        None
    };
    Ok(DiscoveredRepositoryDeclarations {
        root: root.to_path_buf(),
        apt,
        dnf,
        zypper_repositories,
        zypper_services,
        alpm,
    })
}

fn discover_apt(root: &Path) -> Result<Vec<AptSourceDocument>> {
    let mut paths = Vec::new();
    let primary = PathBuf::from("/etc/apt/sources.list");
    if selected_exists(root, &primary, DeclarationEcosystem::Apt)? {
        paths.push((primary, AptSyntax::OneLine));
    }
    for path in directory_files(root, Path::new(APT_DIRECTORY), DeclarationEcosystem::Apt)? {
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !apt_filename_is_valid(filename) {
            continue;
        }
        let syntax = if filename.ends_with(".list") {
            AptSyntax::OneLine
        } else {
            AptSyntax::Deb822
        };
        paths.push((path, syntax));
    }
    paths
        .into_iter()
        .map(|(path, syntax)| {
            let source = read_utf8(root, &path, DeclarationEcosystem::Apt)?;
            AptSourceDocument::parse(path, syntax, &source)
        })
        .collect()
}

fn discover_dnf(root: &Path) -> Result<Vec<DnfRepoDocument>> {
    let mut documents = Vec::new();
    for directory in DNF_DIRECTORIES {
        for path in directory_files(root, Path::new(directory), DeclarationEcosystem::Dnf)? {
            if path.extension().and_then(|value| value.to_str()) != Some("repo") {
                continue;
            }
            let source = read_utf8(root, &path, DeclarationEcosystem::Dnf)?;
            documents.push(DnfRepoDocument::parse(path, &source)?);
        }
    }
    Ok(documents)
}

fn discover_zypper_repositories(root: &Path) -> Result<Vec<ZypperRepoDocument>> {
    directory_files(
        root,
        Path::new(ZYPPER_REPOSITORY_DIRECTORY),
        DeclarationEcosystem::ZypperRepository,
    )?
    .into_iter()
    .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("repo"))
    .map(|path| {
        let source = read_utf8(root, &path, DeclarationEcosystem::ZypperRepository)?;
        let document = ZypperRepoDocument::parse(path, &source)?;
        for repository in &document.repositories {
            repository.require_interpreted_authority()?;
        }
        Ok(document)
    })
    .collect()
}

fn discover_zypper_services(root: &Path) -> Result<Vec<ZypperServiceDocument>> {
    directory_files(
        root,
        Path::new(ZYPPER_SERVICE_DIRECTORY),
        DeclarationEcosystem::ZypperService,
    )?
    .into_iter()
    .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("service"))
    .map(|path| {
        let source = read_utf8(root, &path, DeclarationEcosystem::ZypperService)?;
        ZypperServiceDocument::parse(path, &source)
    })
    .collect()
}

fn directory_files(
    root: &Path,
    guest_directory: &Path,
    ecosystem: DeclarationEcosystem,
) -> Result<Vec<PathBuf>> {
    let host_directory = rooted_path(root, guest_directory, ecosystem)?;
    let entries = match fs::read_dir(&host_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(ecosystem, guest_directory, error)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error(ecosystem, guest_directory, error))?;
        let guest_path = guest_directory.join(entry.file_name());
        let host_path = rooted_path(root, &guest_path, ecosystem)?;
        if host_path
            .metadata()
            .map_err(|error| io_error(ecosystem, &guest_path, error))?
            .is_file()
        {
            files.push(guest_path);
        }
    }
    files.sort();
    Ok(files)
}

fn apt_filename_is_valid(filename: &str) -> bool {
    (filename.ends_with(".list") || filename.ends_with(".sources"))
        && filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn selected_exists(
    root: &Path,
    guest_path: &Path,
    ecosystem: DeclarationEcosystem,
) -> Result<bool> {
    let host_path = rooted_path(root, guest_path, ecosystem)?;
    match host_path.metadata() {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(ecosystem, guest_path, error)),
    }
}

fn read_utf8(root: &Path, guest_path: &Path, ecosystem: DeclarationEcosystem) -> Result<String> {
    let host_path = rooted_path(root, guest_path, ecosystem)?;
    let bytes = fs::read(host_path).map_err(|error| io_error(ecosystem, guest_path, error))?;
    String::from_utf8(bytes).map_err(|error| {
        DeclarationError::new(
            ecosystem,
            DeclarationErrorKind::InvalidUtf8,
            guest_path,
            None,
            error.to_string(),
        )
    })
}

fn rooted_path(root: &Path, guest_path: &Path, ecosystem: DeclarationEcosystem) -> Result<PathBuf> {
    safe_join(root, guest_path).map_err(|error| {
        DeclarationError::new(
            ecosystem,
            DeclarationErrorKind::PathEscape,
            guest_path,
            None,
            error.to_string(),
        )
    })
}

fn io_error(
    ecosystem: DeclarationEcosystem,
    path: &Path,
    error: std::io::Error,
) -> DeclarationError {
    DeclarationError::new(
        ecosystem,
        DeclarationErrorKind::Io,
        path,
        None,
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_each_ecosystem_only_below_selected_root() {
        let root = tempfile::tempdir().unwrap();
        let paths = [
            "etc/apt/sources.list.d",
            "etc/yum.repos.d",
            "etc/zypp/repos.d",
            "etc/zypp/services.d",
        ];
        for path in paths {
            fs::create_dir_all(root.path().join(path)).unwrap();
        }
        fs::write(
            root.path().join("etc/apt/sources.list.d/vendor.sources"),
            "Types: deb\nURIs: https://apt.example\nSuites: stable\nComponents: main\n",
        )
        .unwrap();
        fs::write(
            root.path().join("etc/yum.repos.d/vendor.repo"),
            "[vendor]\nbaseurl=https://rpm.example\n",
        )
        .unwrap();
        fs::write(
            root.path().join("etc/zypp/repos.d/vendor.repo"),
            "[vendor]\nbaseurl=https://zypp.example\n",
        )
        .unwrap();
        fs::write(
            root.path().join("etc/zypp/services.d/vendor.service"),
            "[vendor]\nurl=https://service.example\n",
        )
        .unwrap();
        fs::write(
            root.path().join("etc/pacman.conf"),
            "[core]\nServer=https://alpm.example/$repo/$arch\n",
        )
        .unwrap();

        let declarations = discover_selected_root(root.path()).unwrap();
        assert_eq!(declarations.apt.len(), 1);
        assert_eq!(declarations.dnf.len(), 1);
        assert_eq!(declarations.zypper_repositories.len(), 1);
        assert_eq!(declarations.zypper_services.len(), 1);
        assert_eq!(declarations.alpm.unwrap().files.len(), 1);
    }

    #[test]
    fn apt_ignores_names_upstream_would_ignore() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc/apt/sources.list.d")).unwrap();
        fs::write(
            root.path().join("etc/apt/sources.list.d/vendor!.list"),
            "deb https://outside.example stable main\n",
        )
        .unwrap();
        assert!(discover_selected_root(root.path()).unwrap().apt.is_empty());
    }

    #[test]
    fn invalid_utf8_is_source_attributed() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc/yum.repos.d")).unwrap();
        fs::write(
            root.path().join("etc/yum.repos.d/vendor.repo"),
            [b'[', 0xff, b']'],
        )
        .unwrap();
        let error = discover_selected_root(root.path()).unwrap_err();
        assert_eq!(error.ecosystem, DeclarationEcosystem::Dnf);
        assert_eq!(error.kind, DeclarationErrorKind::InvalidUtf8);
        assert_eq!(error.path, Path::new("/etc/yum.repos.d/vendor.repo"));
    }

    #[test]
    fn zypper_extras_are_preserved_but_authoritative_discovery_refuses_them() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc/zypp/repos.d")).unwrap();
        fs::write(
            root.path().join("etc/zypp/repos.d/vendor.repo"),
            "[vendor]\nbaseurl=https://repo.example\nx-vendor=opaque\n",
        )
        .unwrap();
        let error = discover_selected_root(root.path()).unwrap_err();
        assert_eq!(error.ecosystem, DeclarationEcosystem::ZypperRepository);
        assert_eq!(error.kind, DeclarationErrorKind::UnknownAuthority);
        assert_eq!(error.line, Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_selected_root_symlink_escape() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc/apt")).unwrap();
        symlink("/etc", root.path().join("etc/apt/sources.list.d")).unwrap();
        let error = discover_selected_root(root.path()).unwrap_err();
        assert_eq!(error.kind, DeclarationErrorKind::PathEscape);
    }
}
