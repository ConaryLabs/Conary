// conary-core/src/packages/pacman_query/inventory.rs

//! Coherent ALPM local-database inventory acquisition.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};
use crate::packages::{
    InstalledConfigAuthority, InstalledFileAbsencePolicy, InstalledFileInfo,
    InstalledInventoryFile, InstalledInventoryPackage, InstalledInventorySnapshot,
    InstalledInventoryWork, InstalledLifecycleAuthority, InstalledPackageIdentity,
    NativeInstallReason, PacmanInstalledBackupDigest, PacmanInstalledLifecycleKind,
    SystemPackageManager,
};
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation, ProvidedCapability,
    RepositoryCapabilityKind, RepositoryRequirementKind, SourcePackageFormat,
};
use crate::repository::package_relation::parse_arch_provide;
use crate::repository::requirement::parse_native_requirement;
use crate::repository::versioning::VersionScheme;

/// Capture every installed ALPM record with one database-root traversal.
pub fn query_installed_inventory() -> Result<InstalledInventorySnapshot> {
    let output = Command::new("pacman-conf")
        .arg("DBPath")
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| Error::InitError(format!("Failed to run pacman-conf DBPath: {error}")))?;
    if !output.status.success() {
        return Err(Error::InitError(format!(
            "pacman-conf DBPath failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::ParseError(format!("pacman-conf output is not UTF-8: {error}")))?;
    let values = stdout.lines().collect::<Vec<_>>();
    if values.len() != 1 || values[0].is_empty() || !Path::new(values[0]).is_absolute() {
        return Err(Error::ParseError(
            "pacman-conf DBPath must emit one absolute database path".to_string(),
        ));
    }
    capture_local_root(&Path::new(values[0]).join("local"), 1)
}

fn capture_local_root(root: &Path, manager_processes: u64) -> Result<InstalledInventorySnapshot> {
    let mut packages = Vec::new();
    let mut ownership_records = 0_u64;
    let entries = fs::read_dir(root).map_err(|error| {
        Error::IoError(format!(
            "failed to traverse ALPM local database {}: {error}",
            root.display()
        ))
    })?;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let directory = entry.path();
        let desc = parse_sections(&fs::read(directory.join("desc"))?, "desc")?;
        let files = parse_sections(&fs::read(directory.join("files"))?, "files")?;
        let package = project_package(&directory, &desc, &files, &mut ownership_records)?;
        packages.push(package);
    }
    InstalledInventorySnapshot::from_packages(
        SystemPackageManager::Pacman,
        packages,
        InstalledInventoryWork {
            manager_processes,
            database_bulk_reads: 1,
            ownership_records,
        },
    )
}

fn parse_sections(bytes: &[u8], document: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let input = std::str::from_utf8(bytes).map_err(|error| {
        Error::ParseError(format!("ALPM local {document} is not UTF-8: {error}"))
    })?;
    let mut fields = BTreeMap::new();
    let mut lines = input.lines().peekable();
    while let Some(line) = lines.next() {
        if line.is_empty() {
            continue;
        }
        let key = line
            .strip_prefix('%')
            .and_then(|line| line.strip_suffix('%'))
            .filter(|key| {
                !key.is_empty()
                    && key
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
            })
            .ok_or_else(|| {
                Error::ParseError(format!(
                    "ALPM local {document} has malformed field marker {line:?}"
                ))
            })?;
        let mut values = Vec::new();
        while let Some(value) = lines.peek().copied() {
            if value.is_empty() {
                lines.next();
                break;
            }
            if value.starts_with('%') && value.ends_with('%') {
                return Err(Error::ParseError(format!(
                    "ALPM local {document} field %{key}% has no blank terminator"
                )));
            }
            values.push(lines.next().expect("peeked value").to_string());
        }
        if fields.insert(key.to_string(), values).is_some() {
            return Err(Error::ParseError(format!(
                "ALPM local {document} repeats field %{key}%"
            )));
        }
    }
    Ok(fields)
}

fn one<'a>(fields: &'a BTreeMap<String, Vec<String>>, key: &str) -> Result<&'a str> {
    let values = fields
        .get(key)
        .ok_or_else(|| Error::ParseError(format!("ALPM local database omitted %{key}%")))?;
    if values.len() != 1 || values[0].is_empty() {
        return Err(Error::ParseError(format!(
            "ALPM local database %{key}% must contain one non-empty value"
        )));
    }
    Ok(&values[0])
}

fn many<'a>(fields: &'a BTreeMap<String, Vec<String>>, key: &str) -> &'a [String] {
    fields.get(key).map(Vec::as_slice).unwrap_or_default()
}

fn project_package(
    directory: &Path,
    desc: &BTreeMap<String, Vec<String>>,
    files: &BTreeMap<String, Vec<String>>,
    ownership_records: &mut u64,
) -> Result<InstalledInventoryPackage> {
    let name = one(desc, "NAME")?;
    let version = one(desc, "VERSION")?;
    let architecture = one(desc, "ARCH")?;
    let identity = InstalledPackageIdentity::pacman(name, name, version, architecture)?;
    let reason = match desc.get("REASON").map(Vec::as_slice) {
        None | Some([]) => NativeInstallReason::Explicit,
        Some([value]) if value == "0" => NativeInstallReason::Explicit,
        Some([value]) if value == "1" => NativeInstallReason::Dependency,
        Some(values) => {
            return Err(Error::ParseError(format!(
                "ALPM local package {name:?} has unsupported %REASON% values {values:?}"
            )));
        }
    };
    let backup = parse_backup(many(files, "BACKUP"))?;
    let mut seen_paths = BTreeSet::new();
    let inventory_files = many(files, "FILES")
        .iter()
        .map(|raw_path| {
            *ownership_records = ownership_records.checked_add(1).ok_or_else(|| {
                Error::ParseError("ALPM ownership record count overflowed u64".to_string())
            })?;
            let directory = raw_path.ends_with('/');
            let path = crate::packages::archive_utils::normalize_path(raw_path)
                .map_err(|error| Error::ParseError(error.to_string()))?;
            if !seen_paths.insert(path.clone()) {
                return Err(Error::ConflictError(format!(
                    "ALPM package {name:?} repeats owned path {path:?}"
                )));
            }
            Ok(InstalledInventoryFile {
                file: InstalledFileInfo {
                    path: path.clone(),
                    size: 0,
                    mode: i32::try_from(if directory {
                        libc::S_IFDIR | 0o755
                    } else {
                        libc::S_IFREG | 0o644
                    })
                    .expect("portable ALPM discovery mode fits i32"),
                    digest: None,
                    user: None,
                    group: None,
                    link_target: None,
                    mtime: None,
                    absence_policy: InstalledFileAbsencePolicy::Required,
                },
                config: backup
                    .get(&path)
                    .map(|digest| InstalledConfigAuthority::Pacman {
                        original_digest: digest.clone(),
                    }),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if let Some(path) = backup.keys().find(|path| !seen_paths.contains(*path)) {
        return Err(Error::ParseError(format!(
            "ALPM package {name:?} declares backup path {path:?} outside %FILES%"
        )));
    }

    let requirements = many(desc, "DEPENDS")
        .iter()
        .map(|requirement| {
            parse_native_requirement(
                RepositoryRequirementKind::Depends,
                VersionScheme::Arch,
                requirement,
            )
            .map_err(Error::ParseError)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut provides = vec![ProvidedCapability {
        kind: RepositoryCapabilityKind::PackageName,
        name: name.to_string(),
        version: Some(version.to_string()),
        version_relation: Some(ProvideVersionRelation::Equal),
        version_scheme: VersionScheme::Arch,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    for (record_index, native) in many(desc, "PROVIDES").iter().enumerate() {
        let parsed = parse_arch_provide(native, name).map_err(Error::ParseError)?;
        provides.push(ProvidedCapability {
            kind: parsed.kind,
            name: parsed.name,
            version_relation: parsed
                .version
                .as_ref()
                .map(|_| ProvideVersionRelation::Equal),
            version: parsed.version,
            version_scheme: VersionScheme::Arch,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::SourceDeclared {
                format: SourcePackageFormat::Alpm,
                record_index: u32::try_from(record_index).map_err(|_| {
                    Error::ParseError("ALPM provide record index exceeds u32".to_string())
                })?,
            },
        });
    }
    for provide in &provides {
        provide.validate()?;
    }
    let lifecycle = [
        ("install", PacmanInstalledLifecycleKind::InstallScript),
        ("mtree", PacmanInstalledLifecycleKind::Mtree),
    ]
    .into_iter()
    .filter_map(|(file, kind)| {
        let path = directory.join(file);
        match fs::read(&path) {
            Ok(bytes) => Some(Ok(InstalledLifecycleAuthority::Pacman {
                kind,
                sha256: format!("sha256:{}", crate::hash::sha256(&bytes)),
                size: bytes.len() as u64,
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(Err(Error::IoError(format!(
                "failed to read ALPM lifecycle record {}: {error}",
                path.display()
            )))),
        }
    })
    .collect::<Result<Vec<_>>>()?;

    Ok(InstalledInventoryPackage {
        identity,
        description: desc.get("DESC").and_then(|values| values.first()).cloned(),
        install_reason: reason,
        files: inventory_files,
        requirements,
        provides,
        lifecycle,
    })
}

fn parse_backup(records: &[String]) -> Result<BTreeMap<String, PacmanInstalledBackupDigest>> {
    let mut backup = BTreeMap::new();
    for record in records {
        let (path, digest) = record.split_once('\t').ok_or_else(|| {
            Error::ParseError("ALPM %BACKUP% record has no path/digest separator".to_string())
        })?;
        let path = crate::packages::archive_utils::normalize_path(path)
            .map_err(|error| Error::ParseError(error.to_string()))?;
        // libalpm 7.1 normally stores the MD5 computed after extraction. If
        // extraction is skipped by NoExtract (or digest computation otherwise
        // fails), backup->hash remains NULL and glibc's `%s` serialization in
        // _alpm_local_db_write() persists the exact `(null)` spelling:
        // https://gitlab.archlinux.org/pacman/pacman/-/blob/54d94116164b0b2202c6061c4a59c6f3e70820d8/lib/libalpm/be_local.c#L1119-1125
        let digest = match digest {
            "" => PacmanInstalledBackupDigest::Empty,
            "(null)" => PacmanInstalledBackupDigest::Unavailable,
            value if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
                PacmanInstalledBackupDigest::Md5(value.to_string())
            }
            _ => {
                return Err(Error::ParseError(format!(
                    "ALPM backup path {path:?} has an invalid digest state"
                )));
            }
        };
        if backup.insert(path.clone(), digest).is_some() {
            return Err(Error::ConflictError(format!(
                "ALPM database repeats backup path {path:?}"
            )));
        }
    }
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package(root: &Path, suffix: &str, reason: &str) {
        let package = root.join(format!("fixture-{suffix}"));
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("desc"),
            format!(
                "%NAME%\nfixture\n\n%VERSION%\n{suffix}\n\n%ARCH%\nx86_64\n\n%DESC%\nfixture package\n\n%REASON%\n{reason}\n\n%DEPENDS%\nglibc>=2.42\n\n%PROVIDES%\nfixture-abi=1\n\n"
            ),
        )
        .unwrap();
        fs::write(
            package.join("files"),
            "%FILES%\netc/fixture.conf\nusr/\nusr/bin/fixture\n\n%BACKUP%\netc/fixture.conf\t0123456789abcdef0123456789abcdef\n\n",
        )
        .unwrap();
        fs::write(package.join("install"), b"post_install() { :; }\n").unwrap();
    }

    #[test]
    fn alpm_inventory_uses_one_fixed_database_traversal() {
        let temp = tempfile::tempdir().unwrap();
        write_package(temp.path(), "1.0-1", "0");

        let snapshot = capture_local_root(temp.path(), 1).unwrap();
        let package = snapshot.package("fixture").unwrap();
        assert_eq!(package.install_reason, NativeInstallReason::Explicit);
        assert_eq!(package.files.len(), 3);
        assert!(package.files[0].config.is_some());
        assert_eq!(package.requirements.len(), 1);
        assert_eq!(package.provides.len(), 2);
        assert_eq!(package.lifecycle.len(), 1);
        assert_eq!(
            snapshot.work(),
            InstalledInventoryWork {
                manager_processes: 1,
                database_bulk_reads: 1,
                ownership_records: 3,
            }
        );
    }

    #[test]
    fn alpm_reason_and_lifecycle_bytes_change_snapshot_token() {
        let explicit_root = tempfile::tempdir().unwrap();
        write_package(explicit_root.path(), "1.0-1", "0");
        let dependency_root = tempfile::tempdir().unwrap();
        write_package(dependency_root.path(), "1.0-1", "1");
        fs::write(
            dependency_root.path().join("fixture-1.0-1/install"),
            b"post_install() { changed; }\n",
        )
        .unwrap();

        let explicit = capture_local_root(explicit_root.path(), 1).unwrap();
        let dependency = capture_local_root(dependency_root.path(), 1).unwrap();
        assert_ne!(explicit.token(), dependency.token());
        assert_eq!(
            dependency.package("fixture").unwrap().install_reason,
            NativeInstallReason::Dependency
        );
    }

    #[test]
    fn absent_alpm_reason_is_the_explicit_default() {
        let temp = tempfile::tempdir().unwrap();
        write_package(temp.path(), "1.0-1", "0");
        let desc = temp.path().join("fixture-1.0-1/desc");
        let content = fs::read_to_string(&desc).unwrap();
        fs::write(&desc, content.replace("%REASON%\n0\n\n", "")).unwrap();

        let snapshot = capture_local_root(temp.path(), 1).unwrap();
        assert_eq!(
            snapshot.package("fixture").unwrap().install_reason,
            NativeInstallReason::Explicit
        );
    }

    #[test]
    fn absent_alpm_backup_digest_remains_exact_config_authority() {
        let temp = tempfile::tempdir().unwrap();
        write_package(temp.path(), "1.0-1", "0");
        let files = temp.path().join("fixture-1.0-1/files");
        let content = fs::read_to_string(&files).unwrap();
        fs::write(
            &files,
            content.replace(
                "etc/fixture.conf\t0123456789abcdef0123456789abcdef",
                "etc/fixture.conf\t",
            ),
        )
        .unwrap();

        let snapshot = capture_local_root(temp.path(), 1).unwrap();
        assert!(matches!(
            snapshot.package("fixture").unwrap().files[0].config,
            Some(InstalledConfigAuthority::Pacman {
                original_digest: PacmanInstalledBackupDigest::Empty
            })
        ));
    }

    #[test]
    fn unavailable_alpm_backup_digest_is_distinct_snapshot_authority() {
        let empty_root = tempfile::tempdir().unwrap();
        write_package(empty_root.path(), "1.0-1", "0");
        let empty_files = empty_root.path().join("fixture-1.0-1/files");
        let empty_content = fs::read_to_string(&empty_files).unwrap();
        fs::write(
            &empty_files,
            empty_content.replace(
                "etc/fixture.conf\t0123456789abcdef0123456789abcdef",
                "etc/fixture.conf\t",
            ),
        )
        .unwrap();

        let unavailable_root = tempfile::tempdir().unwrap();
        write_package(unavailable_root.path(), "1.0-1", "0");
        let unavailable_files = unavailable_root.path().join("fixture-1.0-1/files");
        let unavailable_content = fs::read_to_string(&unavailable_files).unwrap();
        fs::write(
            &unavailable_files,
            unavailable_content.replace(
                "etc/fixture.conf\t0123456789abcdef0123456789abcdef",
                "etc/fixture.conf\t(null)",
            ),
        )
        .unwrap();

        let empty = capture_local_root(empty_root.path(), 1).unwrap();
        let unavailable = capture_local_root(unavailable_root.path(), 1).unwrap();

        assert!(matches!(
            unavailable.package("fixture").unwrap().files[0].config,
            Some(InstalledConfigAuthority::Pacman {
                original_digest: PacmanInstalledBackupDigest::Unavailable
            })
        ));
        assert_ne!(empty.token(), unavailable.token());
    }
}
