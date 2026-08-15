// conary-core/src/packages/installed_inventory.rs

//! Coherent installed native-package inventory authority.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::repository::dependency_model::{ProvidedCapability, RepositoryRequirementGroup};

use super::{InstalledFileInfo, InstalledPackageIdentity, SystemPackageManager};

/// Exact native install reason at inventory capture time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeInstallReason {
    Explicit,
    Dependency,
}

/// Exact digest state retained for one ALPM backup declaration.
///
/// Pacman normally persists an MD5 digest. A deliberately empty local-db
/// field and the `(null)` spelling emitted when libalpm has no digest are
/// distinct native states and must remain distinct in the inventory token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum PacmanInstalledBackupDigest {
    Md5(String),
    Empty,
    Unavailable,
}

/// Source-specific configuration declaration retained by an installed manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "manager", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InstalledConfigAuthority {
    Rpm {
        config: bool,
        noreplace: bool,
        missing_ok: bool,
        ghost: bool,
    },
    Dpkg {
        original_md5: Option<String>,
        obsolete: bool,
        remove_on_upgrade: bool,
    },
    Pacman {
        original_digest: PacmanInstalledBackupDigest,
    },
    Eopkg {
        permanent: bool,
    },
}

/// RPM lifecycle slot names retained in an installed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RpmInstalledLifecycleSlot {
    PreInstall,
    PostInstall,
    PreRemove,
    PostRemove,
    PreTransaction,
    PostTransaction,
    PreUntransaction,
    PostUntransaction,
    Verify,
    Trigger,
    FileTrigger,
    TransactionFileTrigger,
}

/// Debian control members retained below `/var/lib/dpkg/info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DpkgInstalledControlMember {
    Config,
    PreInstall,
    PostInstall,
    PreRemove,
    PostRemove,
    Triggers,
    Templates,
}

/// ALPM lifecycle records retained in the local database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PacmanInstalledLifecycleKind {
    InstallScript,
    Mtree,
}

/// Digest authority for source-specific installed lifecycle state.
///
/// Adoption does not execute these records. Their exact byte digest and size
/// participate in the snapshot token so a native lifecycle change cannot pass
/// pre-commit revalidation unnoticed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "manager", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InstalledLifecycleAuthority {
    Rpm {
        slot: RpmInstalledLifecycleSlot,
        sha256: String,
        size: u64,
    },
    Dpkg {
        member: DpkgInstalledControlMember,
        sha256: String,
        size: u64,
    },
    Pacman {
        kind: PacmanInstalledLifecycleKind,
        sha256: String,
        size: u64,
    },
    Eopkg {
        component: String,
        sha256: String,
        size: u64,
    },
}

/// One normalized native ownership declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledInventoryFile {
    pub file: InstalledFileInfo,
    pub config: Option<InstalledConfigAuthority>,
}

/// All installed authority for one exact native package identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledInventoryPackage {
    pub identity: InstalledPackageIdentity,
    pub description: Option<String>,
    pub install_reason: NativeInstallReason,
    pub files: Vec<InstalledInventoryFile>,
    pub requirements: Vec<RepositoryRequirementGroup>,
    pub provides: Vec<ProvidedCapability>,
    pub lifecycle: Vec<InstalledLifecycleAuthority>,
}

/// Fixed-work observations emitted by one backend acquisition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledInventoryWork {
    /// Native-manager child processes spawned for the complete inventory.
    pub manager_processes: u64,
    /// Logical whole-database reads or directory traversals.
    pub database_bulk_reads: u64,
    /// Native ownership path records decoded.
    pub ownership_records: u64,
}

/// Content identity of every mutation-relevant installed-manager field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstalledInventoryToken(String);

impl InstalledInventoryToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One coherent installed native-package inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledInventorySnapshot {
    manager: SystemPackageManager,
    token: InstalledInventoryToken,
    packages: BTreeMap<String, InstalledInventoryPackage>,
    owners_by_path: BTreeMap<String, Vec<String>>,
    work: InstalledInventoryWork,
}

#[derive(Serialize)]
struct TokenAuthority<'a> {
    version: u32,
    manager: SystemPackageManager,
    packages: &'a BTreeMap<String, InstalledInventoryPackage>,
    owners_by_path: &'a BTreeMap<String, Vec<String>>,
}

impl InstalledInventorySnapshot {
    pub const TOKEN_VERSION: u32 = 1;

    /// Acquire one complete inventory from the selected backend.
    pub fn capture(manager: SystemPackageManager) -> Result<Self> {
        match manager {
            SystemPackageManager::Rpm => super::rpm_query::query_installed_inventory(),
            SystemPackageManager::Dpkg => super::dpkg_query::query_installed_inventory(),
            SystemPackageManager::Pacman => super::pacman_query::query_installed_inventory(),
            SystemPackageManager::Eopkg => {
                super::eopkg::query::InstalledEopkgDatabase::load()?.inventory_snapshot()
            }
            SystemPackageManager::Unknown => Err(Error::ConfigError(
                "cannot capture an installed inventory for unknown native authority".to_string(),
            )),
        }
    }

    pub fn from_packages(
        manager: SystemPackageManager,
        packages: Vec<InstalledInventoryPackage>,
        work: InstalledInventoryWork,
    ) -> Result<Self> {
        if !manager.is_available() {
            return Err(Error::ConfigError(
                "installed inventory requires a concrete native package manager".to_string(),
            ));
        }
        let mut by_selector = BTreeMap::new();
        let mut owners = BTreeMap::<String, BTreeSet<String>>::new();
        for mut package in packages {
            package.identity.validate()?;
            if SystemPackageManager::from_version_scheme(package.identity.version_scheme())
                != Some(manager)
            {
                return Err(Error::ConflictError(format!(
                    "installed inventory manager '{}' disagrees with package identity '{}'",
                    manager.cli_name(),
                    package.identity.selector()
                )));
            }
            normalize_package(&mut package)?;
            let selector = package.identity.selector().to_string();
            for file in &package.files {
                owners
                    .entry(file.file.path.clone())
                    .or_default()
                    .insert(selector.clone());
            }
            if by_selector.insert(selector.clone(), package).is_some() {
                return Err(Error::ConflictError(format!(
                    "installed inventory repeated exact selector '{selector}'"
                )));
            }
        }
        let owners_by_path = owners
            .into_iter()
            .map(|(path, owners)| (path, owners.into_iter().collect()))
            .collect::<BTreeMap<_, _>>();
        let token_bytes = serde_json::to_vec(&TokenAuthority {
            version: Self::TOKEN_VERSION,
            manager,
            packages: &by_selector,
            owners_by_path: &owners_by_path,
        })
        .map_err(|error| {
            Error::ParseError(format!(
                "failed to encode installed inventory token: {error}"
            ))
        })?;
        let token =
            InstalledInventoryToken(format!("sha256:{}", crate::hash::sha256(&token_bytes)));
        Ok(Self {
            manager,
            token,
            packages: by_selector,
            owners_by_path,
            work,
        })
    }

    pub const fn manager(&self) -> SystemPackageManager {
        self.manager
    }

    pub fn token(&self) -> &InstalledInventoryToken {
        &self.token
    }

    pub fn packages(&self) -> impl ExactSizeIterator<Item = &InstalledInventoryPackage> {
        self.packages.values()
    }

    pub fn package(&self, selector: &str) -> Option<&InstalledInventoryPackage> {
        self.packages.get(selector)
    }

    pub fn owners_by_path(&self) -> &BTreeMap<String, Vec<String>> {
        &self.owners_by_path
    }

    pub const fn work(&self) -> InstalledInventoryWork {
        self.work
    }

    pub fn require_unchanged(&self, current: &Self) -> Result<()> {
        if self.manager != current.manager || self.token != current.token {
            return Err(Error::ConflictError(format!(
                "native {} inventory changed during adoption (planned {}, current {})",
                self.manager.display_name(),
                self.token.as_str(),
                current.token.as_str()
            )));
        }
        Ok(())
    }
}

fn normalize_package(package: &mut InstalledInventoryPackage) -> Result<()> {
    let selector = package.identity.selector();
    let mut paths = BTreeSet::new();
    for file in &mut package.files {
        file.file.path = super::archive_utils::normalize_path(&file.file.path)
            .map_err(|error| Error::ParseError(error.to_string()))?;
        if !paths.insert(file.file.path.clone()) {
            return Err(Error::ConflictError(format!(
                "installed package '{selector}' repeats owned path '{}'",
                file.file.path
            )));
        }
    }
    package
        .files
        .sort_by(|left, right| left.file.path.cmp(&right.file.path));
    package.requirements.sort_by(|left, right| {
        serde_json::to_string(left)
            .expect("requirement serialization is infallible")
            .cmp(&serde_json::to_string(right).expect("requirement serialization is infallible"))
    });
    package.provides.sort_by(|left, right| {
        serde_json::to_string(left)
            .expect("provide serialization is infallible")
            .cmp(&serde_json::to_string(right).expect("provide serialization is infallible"))
    });
    package.lifecycle.sort_by(|left, right| {
        serde_json::to_string(left)
            .expect("lifecycle serialization is infallible")
            .cmp(&serde_json::to_string(right).expect("lifecycle serialization is infallible"))
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::InstalledFileAbsencePolicy;
    use crate::repository::dependency_model::DebianMultiArch;

    fn package(selector: &str, architecture: &str, path: &str) -> InstalledInventoryPackage {
        InstalledInventoryPackage {
            identity: InstalledPackageIdentity::dpkg(
                selector,
                "libfixture",
                "1.0-1",
                architecture,
                DebianMultiArch::Same,
            )
            .unwrap(),
            description: None,
            install_reason: NativeInstallReason::Dependency,
            files: vec![InstalledInventoryFile {
                file: InstalledFileInfo {
                    path: path.to_string(),
                    size: 0,
                    mode: 0,
                    digest: None,
                    user: None,
                    group: None,
                    link_target: None,
                    mtime: None,
                    absence_policy: InstalledFileAbsencePolicy::Required,
                },
                config: None,
            }],
            requirements: Vec::new(),
            provides: Vec::new(),
            lifecycle: Vec::new(),
        }
    }

    #[test]
    fn snapshot_preserves_multiarch_and_every_shared_path_owner() {
        let snapshot = InstalledInventorySnapshot::from_packages(
            SystemPackageManager::Dpkg,
            vec![
                package("libfixture:amd64", "amd64", "/usr/share/doc/fixture"),
                package("libfixture:i386", "i386", "/usr/share/doc/fixture"),
            ],
            InstalledInventoryWork::default(),
        )
        .unwrap();

        assert_eq!(snapshot.packages().len(), 2);
        assert_eq!(
            snapshot.owners_by_path()["/usr/share/doc/fixture"],
            ["libfixture:amd64", "libfixture:i386"]
        );
    }

    #[test]
    fn token_is_order_independent_but_changes_with_native_reason() {
        let amd64 = package("libfixture:amd64", "amd64", "/usr/lib/fixture");
        let i386 = package("libfixture:i386", "i386", "/usr/lib32/fixture");
        let first = InstalledInventorySnapshot::from_packages(
            SystemPackageManager::Dpkg,
            vec![amd64.clone(), i386.clone()],
            InstalledInventoryWork {
                manager_processes: 99,
                database_bulk_reads: 99,
                ownership_records: 2,
            },
        )
        .unwrap();
        let reordered = InstalledInventorySnapshot::from_packages(
            SystemPackageManager::Dpkg,
            vec![i386, amd64.clone()],
            InstalledInventoryWork::default(),
        )
        .unwrap();
        assert_eq!(first.token(), reordered.token());

        let mut changed = amd64;
        changed.install_reason = NativeInstallReason::Explicit;
        let current = InstalledInventorySnapshot::from_packages(
            SystemPackageManager::Dpkg,
            vec![changed],
            InstalledInventoryWork::default(),
        )
        .unwrap();
        assert!(first.require_unchanged(&current).is_err());
    }

    #[test]
    fn duplicate_normalized_path_is_rejected() {
        let mut duplicate = package("libfixture:amd64", "amd64", "/usr/lib/fixture");
        duplicate.files.push(InstalledInventoryFile {
            file: InstalledFileInfo {
                path: "usr/lib/fixture".to_string(),
                size: 0,
                mode: 0,
                digest: None,
                user: None,
                group: None,
                link_target: None,
                mtime: None,
                absence_policy: InstalledFileAbsencePolicy::Required,
            },
            config: None,
        });
        let error = InstalledInventorySnapshot::from_packages(
            SystemPackageManager::Dpkg,
            vec![duplicate],
            InstalledInventoryWork::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("repeats owned path"));
    }
}
