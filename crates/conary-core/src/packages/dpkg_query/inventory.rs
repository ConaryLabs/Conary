// conary-core/src/packages/dpkg_query/inventory.rs

//! Fixed-work dpkg installed-inventory acquisition.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
#[cfg(test)]
use crate::packages::install_reason::parse_package_name_lines;
use crate::packages::install_reason::query_package_names;
use crate::packages::{
    DpkgInstalledControlMember, InstalledConfigAuthority, InstalledFileAbsencePolicy,
    InstalledFileInfo, InstalledInventoryFile, InstalledInventoryPackage,
    InstalledInventorySnapshot, InstalledInventoryWork, InstalledLifecycleAuthority,
    NativeInstallReason, SystemPackageManager,
};
use crate::repository::dependency_model::RepositoryRequirementKind;
use crate::repository::requirement::parse_native_requirement;
use crate::repository::versioning::VersionScheme;

const INVENTORY_FORMAT: &str = "${binary:Package}\x1e${Package}\x1e${Version}\x1e${Architecture}\x1e${Description}\x1e${Maintainer}\x1e${Homepage}\x1e${Section}\x1e${Priority}\x1e${Installed-Size}\x1e${Multi-Arch}\x1e${db:Status-Status}\x1e${Pre-Depends}\x1e${Depends}\x1e${Provides}\x1e${Conffiles}\x1f";

/// Capture installed dpkg identity and control fields in one query, then scan
/// the info directory once for ownership and lifecycle records.
pub fn query_installed_inventory() -> Result<InstalledInventorySnapshot> {
    let packages = crate::packages::query_common::run_query_command(
        "dpkg-query",
        &["-W", "-f", INVENTORY_FORMAT],
    )?;
    let manual = query_package_names("APT", "apt-mark", &["showmanual"])
        .map_err(|error| Error::InitError(error.to_string()))?;
    capture_from_authority(&packages, &manual, Path::new("/var/lib/dpkg/info"), 2)
}

fn capture_from_authority(
    output: &str,
    manual: &HashSet<String>,
    info_root: &Path,
    manager_processes: u64,
) -> Result<InstalledInventorySnapshot> {
    let info = read_info_directory(info_root)?;
    let mut packages = Vec::new();
    let mut ownership_records = 0_u64;
    for (record_index, record) in output
        .split('\x1f')
        .filter(|record| !record.is_empty())
        .enumerate()
    {
        let fields = record.split('\x1e').collect::<Vec<_>>();
        if fields.len() != 16 {
            return Err(Error::ParseError(format!(
                "dpkg bulk inventory record {} has {} fields; expected 16",
                record_index + 1,
                fields.len()
            )));
        }
        if fields[11] != "installed" {
            continue;
        }
        let identity_record = format!("{}\x1f", fields[..11].join("\x1e"));
        let mut identities = super::parse_package_query_records(&identity_record)?;
        let package_record = identities.pop().ok_or_else(|| {
            Error::ParseError(format!(
                "dpkg bulk inventory record {} produced no installed identity",
                record_index + 1
            ))
        })?;
        let identity = package_record.identity;
        let selector = identity.selector();
        let package_files = project_files(selector, &info, &mut ownership_records)?;
        let config = parse_conffiles(fields[15])?;
        let mut files = package_files
            .into_iter()
            .map(|file| {
                let authority = config.get(&file.path).cloned();
                InstalledInventoryFile {
                    file,
                    config: authority,
                }
            })
            .collect::<Vec<_>>();
        let known_paths = files
            .iter()
            .map(|file| file.file.path.clone())
            .collect::<BTreeSet<_>>();
        for (path, authority) in &config {
            if !known_paths.contains(path) {
                ownership_records = ownership_records.checked_add(1).ok_or_else(|| {
                    Error::ParseError("dpkg ownership record count overflowed u64".to_string())
                })?;
                files.push(InstalledInventoryFile {
                    file: InstalledFileInfo {
                        path: path.clone(),
                        size: 0,
                        mode: i32::try_from(libc::S_IFREG | 0o644)
                            .expect("portable dpkg discovery mode fits i32"),
                        digest: match authority {
                            InstalledConfigAuthority::Dpkg { md5, .. } => Some(md5.clone()),
                            _ => unreachable!("dpkg config parser emits dpkg authority"),
                        },
                        user: None,
                        group: None,
                        link_target: None,
                        mtime: None,
                        absence_policy: InstalledFileAbsencePolicy::Required,
                    },
                    config: Some(authority.clone()),
                });
            }
        }
        let mut requirements = Vec::new();
        for (kind, field) in [
            (RepositoryRequirementKind::PreDepends, fields[12]),
            (RepositoryRequirementKind::Depends, fields[13]),
        ] {
            for native in super::split_debian_requirement_field(field)? {
                requirements.push(
                    parse_native_requirement(kind, VersionScheme::Debian, native)
                        .map_err(Error::ParseError)?,
                );
            }
        }
        let provides_record = format!(
            "{}\x1e{}\x1e{}\x1e{}",
            identity.selector(),
            identity.name(),
            identity.version(),
            fields[14]
        );
        let provides = super::parse_dpkg_provide_record(&identity, &provides_record)?;
        let lifecycle = project_lifecycle(selector, &info)?;
        let is_explicit = manual.contains(&identity.install_reason_selector());
        packages.push(InstalledInventoryPackage {
            identity,
            description: package_record.info.description,
            install_reason: if is_explicit {
                NativeInstallReason::Explicit
            } else {
                NativeInstallReason::Dependency
            },
            files,
            requirements,
            provides,
            lifecycle,
        });
    }
    InstalledInventorySnapshot::from_packages(
        SystemPackageManager::Dpkg,
        packages,
        InstalledInventoryWork {
            manager_processes,
            database_bulk_reads: 2,
            ownership_records,
        },
    )
}

fn read_info_directory(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(root).map_err(|error| {
        Error::IoError(format!(
            "failed to traverse dpkg info directory {}: {error}",
            root.display()
        ))
    })? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().into_string().map_err(|_| {
            Error::ParseError("dpkg info directory contains a non-UTF-8 filename".to_string())
        })?;
        files.insert(name, fs::read(entry.path())?);
    }
    Ok(files)
}

fn project_files(
    selector: &str,
    info: &BTreeMap<String, Vec<u8>>,
    ownership_records: &mut u64,
) -> Result<Vec<InstalledFileInfo>> {
    let list_name = format!("{selector}.list");
    let list = info.get(&list_name).ok_or_else(|| {
        Error::NotFound(format!(
            "installed dpkg selector {selector:?} has no exact {list_name:?} ownership record"
        ))
    })?;
    let list = std::str::from_utf8(list).map_err(|error| {
        Error::ParseError(format!(
            "dpkg ownership record {list_name:?} is not UTF-8: {error}"
        ))
    })?;
    let digests = match info.get(&format!("{selector}.md5sums")) {
        Some(bytes) => super::parse_md5sums(std::str::from_utf8(bytes).map_err(|error| {
            Error::ParseError(format!(
                "dpkg md5sums record for {selector:?} is not UTF-8: {error}"
            ))
        })?)?,
        None => std::collections::HashMap::new(),
    };
    list.lines()
        .enumerate()
        .filter_map(|(index, path)| {
            if path.is_empty() {
                return Some(Err(Error::ParseError(format!(
                    "dpkg ownership record {list_name:?} has an empty path at line {}",
                    index + 1
                ))));
            }
            // dpkg's installed `.list` grammar may carry `/.` as the root
            // anchor. It is not a deployable package payload node.
            if matches!(path, "/" | "/.") {
                return None;
            }
            Some((|| {
                *ownership_records = ownership_records.checked_add(1).ok_or_else(|| {
                    Error::ParseError("dpkg ownership record count overflowed u64".to_string())
                })?;
                let path = crate::packages::archive_utils::normalize_path(path)
                    .map_err(|error| Error::ParseError(error.to_string()))?;
                let digest_path = path.strip_prefix('/').unwrap_or(&path);
                let digest = digests.get(digest_path).cloned();
                Ok(InstalledFileInfo {
                    path,
                    size: 0,
                    mode: 0,
                    digest,
                    user: None,
                    group: None,
                    link_target: None,
                    mtime: None,
                    absence_policy: InstalledFileAbsencePolicy::Required,
                })
            })())
        })
        .collect()
}

fn parse_conffiles(field: &str) -> Result<BTreeMap<String, InstalledConfigAuthority>> {
    let mut config = BTreeMap::new();
    for (index, line) in field.lines().filter(|line| !line.is_empty()).enumerate() {
        let value = line.trim();
        let (value, obsolete) = value
            .strip_suffix(" obsolete")
            .map(|value| (value, true))
            .unwrap_or((value, false));
        let (path, md5) = value.rsplit_once(' ').ok_or_else(|| {
            Error::ParseError(format!(
                "dpkg Conffiles record {} has no path/digest separator",
                index + 1
            ))
        })?;
        if md5.len() != 32 || !md5.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::ParseError(format!(
                "dpkg Conffiles record {} has an invalid MD5 digest",
                index + 1
            )));
        }
        let path = crate::packages::archive_utils::normalize_path(path)
            .map_err(|error| Error::ParseError(error.to_string()))?;
        if config
            .insert(
                path.clone(),
                InstalledConfigAuthority::Dpkg {
                    md5: md5.to_string(),
                    obsolete,
                },
            )
            .is_some()
        {
            return Err(Error::ConflictError(format!(
                "dpkg Conffiles repeats path {path:?}"
            )));
        }
    }
    Ok(config)
}

fn project_lifecycle(
    selector: &str,
    info: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<InstalledLifecycleAuthority>> {
    [
        ("config", DpkgInstalledControlMember::Config),
        ("preinst", DpkgInstalledControlMember::PreInstall),
        ("postinst", DpkgInstalledControlMember::PostInstall),
        ("prerm", DpkgInstalledControlMember::PreRemove),
        ("postrm", DpkgInstalledControlMember::PostRemove),
        ("triggers", DpkgInstalledControlMember::Triggers),
        ("templates", DpkgInstalledControlMember::Templates),
    ]
    .into_iter()
    .filter_map(|(extension, member)| {
        info.get(&format!("{selector}.{extension}")).map(|bytes| {
            InstalledLifecycleAuthority::Dpkg {
                member,
                sha256: format!("sha256:{}", crate::hash::sha256(bytes)),
                size: bytes.len() as u64,
            }
        })
    })
    .map(Ok)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(reason_version: &str) -> String {
        format!(
            "fixture:amd64\x1efixture\x1e{reason_version}\x1eamd64\x1edescription\x1emaintainer\x1e\x1eutils\x1eoptional\x1e42\x1esame\x1einstalled\x1elibc6 (>= 2.42)\x1ezlib1g | libz1\x1efixture-abi (= 1)\x1e /etc/fixture.conf 0123456789abcdef0123456789abcdef\x1f"
        )
    }

    fn info_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("fixture:amd64.list"),
            "/.\n/etc/fixture.conf\n/usr/bin/fixture\n",
        )
        .unwrap();
        fs::write(
            root.path().join("fixture:amd64.md5sums"),
            "fedcba9876543210fedcba9876543210  usr/bin/fixture\n",
        )
        .unwrap();
        fs::write(
            root.path().join("fixture:amd64.postinst"),
            "#!/bin/sh\nexit 0\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn dpkg_inventory_uses_two_fixed_manager_processes_and_two_bulk_reads() {
        let root = info_root();
        let manual = parse_package_name_lines("APT", "fixture:amd64\n").unwrap();
        let snapshot = capture_from_authority(&output("1.0-1"), &manual, root.path(), 2).unwrap();
        let package = snapshot.package("fixture:amd64").unwrap();

        assert_eq!(package.install_reason, NativeInstallReason::Explicit);
        assert_eq!(package.files.len(), 2);
        assert!(package.files[0].config.is_some());
        assert_eq!(package.requirements.len(), 2);
        assert_eq!(package.provides.len(), 2);
        assert_eq!(package.lifecycle.len(), 1);
        assert_eq!(
            snapshot.work(),
            InstalledInventoryWork {
                manager_processes: 2,
                database_bulk_reads: 2,
                ownership_records: 2,
            }
        );
    }

    #[test]
    fn dpkg_version_reason_config_and_lifecycle_change_token() {
        let root = info_root();
        let explicit = parse_package_name_lines("APT", "fixture:amd64\n").unwrap();
        let first = capture_from_authority(&output("1.0-1"), &explicit, root.path(), 2).unwrap();
        fs::write(
            root.path().join("fixture:amd64.postinst"),
            "#!/bin/sh\nchanged\n",
        )
        .unwrap();
        let second =
            capture_from_authority(&output("1.0-2"), &HashSet::new(), root.path(), 2).unwrap();

        assert_ne!(first.token(), second.token());
        assert_eq!(
            second.package("fixture:amd64").unwrap().install_reason,
            NativeInstallReason::Dependency
        );
    }
}
