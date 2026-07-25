// conary-core/src/config_transaction.rs

//! Exact package-format configuration-file transaction contracts.
//!
//! Package metadata selects the contract. File identities select the action.
//! Paths, package names, distro names, and script text never select semantics.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path};

use crate::db::models::ConfigSource;
use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};

pub const GENERATION_CONFIG_TRANSACTION_SCHEMA_VERSION: u32 = 2;

/// Return whether a package payload entry participates in the exact `/etc`
/// configuration transaction contract.
///
/// Directories and special files remain ordinary payload ownership and never
/// enter the config artifact model. The parsed node kind is the sole file-type
/// authority.
#[must_use]
pub fn is_etc_config_payload(path: &str, kind: &PayloadNodeKind) -> bool {
    let Some(relative) = path.strip_prefix("/etc/") else {
        return false;
    };
    if relative.is_empty() {
        return false;
    }
    is_config_artifact_kind(kind)
}

/// Return whether a package entry can be represented by `ConfigArtifact`.
#[must_use]
pub fn is_config_artifact_kind(kind: &PayloadNodeKind) -> bool {
    matches!(
        kind,
        PayloadNodeKind::Regular { .. } | PayloadNodeKind::Symlink { .. }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigSuffix {
    RpmNew,
    RpmSave,
    RpmOrig,
    DpkgDist,
    DpkgOld,
    PacNew,
    PacSave,
    ConaryNew,
    ConarySave,
}

impl ConfigSuffix {
    pub const ALL: [Self; 9] = [
        Self::RpmNew,
        Self::RpmSave,
        Self::RpmOrig,
        Self::DpkgDist,
        Self::DpkgOld,
        Self::PacNew,
        Self::PacSave,
        Self::ConaryNew,
        Self::ConarySave,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RpmNew => ".rpmnew",
            Self::RpmSave => ".rpmsave",
            Self::RpmOrig => ".rpmorig",
            Self::DpkgDist => ".dpkg-dist",
            Self::DpkgOld => ".dpkg-old",
            Self::PacNew => ".pacnew",
            Self::PacSave => ".pacsave",
            Self::ConaryNew => ".conary-new",
            Self::ConarySave => ".conary-save",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigInstallDecision {
    Install,
    Keep,
    InstallAlternative(ConfigSuffix),
    SaveCurrentAndInstall(ConfigSuffix),
}

/// Decide an install or update from the typed source contract and exact
/// old/current/new content identities.
#[must_use]
pub fn decide_config_install(
    source: ConfigSource,
    noreplace: bool,
    old: Option<&str>,
    current: Option<&str>,
    new: &str,
) -> ConfigInstallDecision {
    if current == Some(new) || (old.is_some() && current == old) {
        return ConfigInstallDecision::Install;
    }

    if let Some(old) = old {
        if current.is_none() {
            return match source {
                // dpkg treats deletion as a local edit.
                ConfigSource::Deb if old == new => ConfigInstallDecision::Keep,
                ConfigSource::Deb => {
                    ConfigInstallDecision::InstallAlternative(ConfigSuffix::DpkgDist)
                }
                // rpm and libalpm recreate a missing packaged config.
                _ => ConfigInstallDecision::Install,
            };
        }

        if old == new {
            return ConfigInstallDecision::Keep;
        }

        return conflicting_update(source, noreplace);
    }

    if current.is_none() {
        ConfigInstallDecision::Install
    } else {
        // A package claims a config path with no prior package baseline.
        match source {
            ConfigSource::Rpm if !noreplace => {
                ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::RpmOrig)
            }
            ConfigSource::Rpm => ConfigInstallDecision::InstallAlternative(ConfigSuffix::RpmNew),
            ConfigSource::Deb => ConfigInstallDecision::InstallAlternative(ConfigSuffix::DpkgDist),
            ConfigSource::Arch => ConfigInstallDecision::InstallAlternative(ConfigSuffix::PacNew),
            ConfigSource::Auto if noreplace => {
                ConfigInstallDecision::InstallAlternative(ConfigSuffix::ConaryNew)
            }
            ConfigSource::Auto => {
                ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::ConarySave)
            }
        }
    }
}

fn conflicting_update(source: ConfigSource, noreplace: bool) -> ConfigInstallDecision {
    match source {
        ConfigSource::Rpm if !noreplace => {
            ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::RpmSave)
        }
        ConfigSource::Rpm => ConfigInstallDecision::InstallAlternative(ConfigSuffix::RpmNew),
        ConfigSource::Deb => ConfigInstallDecision::InstallAlternative(ConfigSuffix::DpkgDist),
        ConfigSource::Arch => ConfigInstallDecision::InstallAlternative(ConfigSuffix::PacNew),
        ConfigSource::Auto if noreplace => {
            ConfigInstallDecision::InstallAlternative(ConfigSuffix::ConaryNew)
        }
        ConfigSource::Auto => {
            ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::ConarySave)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigRemovalDecision {
    KeepResidual,
    Remove,
    SaveCurrent(ConfigSuffix),
    RotatePacsaveAndSaveCurrent,
}

/// Decide removal from persisted package semantics and the exact current
/// identity. `purge` is authoritative for Debian conffile residual state.
#[must_use]
pub fn decide_config_removal(
    source: ConfigSource,
    ghost: bool,
    purge: bool,
    old: Option<&str>,
    current: Option<&str>,
) -> ConfigRemovalDecision {
    if ghost || purge {
        return ConfigRemovalDecision::Remove;
    }
    if source == ConfigSource::Deb {
        return ConfigRemovalDecision::KeepResidual;
    }
    let modified = current.is_some() && current != old;
    if !modified {
        return ConfigRemovalDecision::Remove;
    }
    match source {
        ConfigSource::Rpm => ConfigRemovalDecision::SaveCurrent(ConfigSuffix::RpmSave),
        ConfigSource::Arch => ConfigRemovalDecision::RotatePacsaveAndSaveCurrent,
        ConfigSource::Auto => ConfigRemovalDecision::SaveCurrent(ConfigSuffix::ConarySave),
        ConfigSource::Deb => ConfigRemovalDecision::KeepResidual,
    }
}

/// Apply dpkg's `remove-on-upgrade` conffile rule.
///
/// The incoming package declares a path that is intentionally absent from its
/// payload. An unchanged old conffile is removed; a locally modified one is
/// preserved as `.dpkg-old`.
pub fn decide_deb_remove_on_upgrade(
    old_package_hash: Option<&str>,
    current_hash: Option<&str>,
) -> ConfigRemovalDecision {
    match current_hash {
        None => ConfigRemovalDecision::Remove,
        Some(current) if old_package_hash == Some(current) => ConfigRemovalDecision::Remove,
        Some(_) => ConfigRemovalDecision::SaveCurrent(ConfigSuffix::DpkgOld),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ConfigArtifact {
    Regular {
        sha256: String,
        content_base64: String,
        node: ResolvedPayloadNode,
    },
    Symlink {
        sha256: String,
        target: String,
        node: ResolvedPayloadNode,
    },
}

impl ConfigArtifact {
    pub fn regular(content: &[u8], node: ResolvedPayloadNode) -> crate::Result<Self> {
        if !matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
            return Err(crate::Error::InvalidPath(
                "regular config artifact requires a regular payload node".to_string(),
            ));
        }
        node.validate()
            .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
        Ok(Self::Regular {
            sha256: crate::hash::sha256(content),
            content_base64: BASE64.encode(content),
            node,
        })
    }

    pub fn symlink(target: String, node: ResolvedPayloadNode) -> crate::Result<Self> {
        let PayloadNodeKind::Symlink {
            target: node_target,
        } = &node.source.kind
        else {
            return Err(crate::Error::InvalidPath(
                "symlink config artifact requires a symlink payload node".to_string(),
            ));
        };
        if node_target != &target {
            return Err(crate::Error::InvalidPath(
                "config artifact symlink target differs from payload-node authority".to_string(),
            ));
        }
        node.validate()
            .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
        Ok(Self::Symlink {
            sha256: crate::hash::sha256(target.as_bytes()),
            target,
            node,
        })
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        match self {
            Self::Regular { sha256, .. } | Self::Symlink { sha256, .. } => sha256,
        }
    }

    #[must_use]
    pub fn node(&self) -> &ResolvedPayloadNode {
        match self {
            Self::Regular { node, .. } | Self::Symlink { node, .. } => node,
        }
    }

    pub fn regular_content(&self) -> crate::Result<Option<Vec<u8>>> {
        match self {
            Self::Regular {
                sha256,
                content_base64,
                ..
            } => {
                let content = BASE64.decode(content_base64).map_err(|error| {
                    crate::Error::ParseError(format!(
                        "invalid generation config artifact base64: {error}"
                    ))
                })?;
                let actual = crate::hash::sha256(&content);
                if &actual != sha256 {
                    return Err(crate::Error::ChecksumMismatch {
                        expected: sha256.clone(),
                        actual,
                    });
                }
                Ok(Some(content))
            }
            Self::Symlink { .. } => Ok(None),
        }
    }

    pub fn validate(&self) -> crate::Result<()> {
        match self {
            Self::Regular { node, .. } => {
                node.validate()
                    .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
                if !matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
                    return Err(crate::Error::InvalidPath(
                        "regular config artifact has non-regular payload authority".to_string(),
                    ));
                }
                self.regular_content()?;
            }
            Self::Symlink {
                sha256,
                target,
                node,
            } => {
                node.validate()
                    .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
                if !matches!(
                    &node.source.kind,
                    PayloadNodeKind::Symlink { target: node_target } if node_target == target
                ) {
                    return Err(crate::Error::InvalidPath(
                        "symlink config artifact differs from payload-node authority".to_string(),
                    ));
                }
                let actual = crate::hash::sha256(target.as_bytes());
                if &actual != sha256 {
                    return Err(crate::Error::ChecksumMismatch {
                        expected: sha256.clone(),
                        actual,
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigPackageState {
    pub source: ConfigSource,
    pub noreplace: bool,
    pub ghost: bool,
    /// Exact package-owned baseline identity. It remains available for
    /// Debian residual conffiles after their payload row has been removed.
    pub original_sha256: Option<String>,
    pub artifact: Option<ConfigArtifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigTransactionOperation {
    Install,
    RemoveOnUpgrade,
    Remove,
    Purge,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigPathTransaction {
    pub path: String,
    pub operation: ConfigTransactionOperation,
    pub before: Option<ConfigPackageState>,
    pub current: Option<ConfigArtifact>,
    pub after: Option<ConfigPackageState>,
    /// Exact pre-transaction auxiliary artifacts, keyed by absolute path.
    pub auxiliaries: Vec<(String, ConfigArtifact)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationConfigTransaction {
    pub schema_version: u32,
    pub entries: Vec<ConfigPathTransaction>,
}

impl Default for GenerationConfigTransaction {
    fn default() -> Self {
        Self {
            schema_version: GENERATION_CONFIG_TRANSACTION_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

impl GenerationConfigTransaction {
    /// Build the exact pre-transaction state used to reverse a native upgrade.
    ///
    /// The forward transaction already owns the complete primary and auxiliary
    /// artifact snapshot. Rollback strips the incoming package state and makes
    /// restoration an explicit operation instead of re-running package-format
    /// decision logic in reverse.
    pub fn restore_snapshot(&self) -> crate::Result<Self> {
        self.validate()?;
        let transaction = Self {
            schema_version: GENERATION_CONFIG_TRANSACTION_SCHEMA_VERSION,
            entries: self
                .entries
                .iter()
                .map(|entry| ConfigPathTransaction {
                    path: entry.path.clone(),
                    operation: ConfigTransactionOperation::Restore,
                    before: entry.before.clone(),
                    current: entry.current.clone(),
                    after: None,
                    auxiliaries: entry.auxiliaries.clone(),
                })
                .collect(),
        };
        transaction.validate()?;
        Ok(transaction)
    }

    /// Require this transaction to be a rollback snapshot, not forward debt.
    pub fn validate_restore_snapshot(&self) -> crate::Result<()> {
        self.validate()?;
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.operation != ConfigTransactionOperation::Restore)
        {
            return Err(crate::Error::ConfigError(format!(
                "native upgrade rollback snapshot contains non-restore operation for {}",
                entry.path
            )));
        }
        Ok(())
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.schema_version != GENERATION_CONFIG_TRANSACTION_SCHEMA_VERSION {
            return Err(crate::Error::ConfigError(format!(
                "unsupported generation config transaction schema {}; expected {}",
                self.schema_version, GENERATION_CONFIG_TRANSACTION_SCHEMA_VERSION
            )));
        }
        let mut paths = BTreeSet::new();
        for entry in &self.entries {
            if !valid_etc_path(&entry.path) {
                return Err(crate::Error::InvalidPath(format!(
                    "generation config transaction path is outside /etc: {}",
                    entry.path
                )));
            }
            if !paths.insert(entry.path.as_str()) {
                return Err(crate::Error::ConfigError(format!(
                    "generation config transaction repeats path {}",
                    entry.path
                )));
            }
            if entry.operation == ConfigTransactionOperation::Install && entry.after.is_none() {
                return Err(crate::Error::ConfigError(format!(
                    "install config transaction for {} has no after state",
                    entry.path
                )));
            }
            if entry.operation != ConfigTransactionOperation::Install && entry.after.is_some() {
                return Err(crate::Error::ConfigError(format!(
                    "generation config removal for {} has an after state",
                    entry.path
                )));
            }
            if let Some(current) = &entry.current {
                current.validate()?;
            }
            for state in [entry.before.as_ref(), entry.after.as_ref()]
                .into_iter()
                .flatten()
            {
                if let Some(artifact) = &state.artifact {
                    artifact.validate()?;
                    if state.original_sha256.as_deref() != Some(artifact.sha256()) {
                        return Err(crate::Error::ChecksumMismatch {
                            expected: state.original_sha256.clone().unwrap_or_default(),
                            actual: artifact.sha256().to_string(),
                        });
                    }
                }
            }
            if let Some(after) = &entry.after {
                if after.ghost {
                    if after.original_sha256.is_some() || after.artifact.is_some() {
                        return Err(crate::Error::ConfigError(format!(
                            "ghost config transaction for {} carries a payload identity",
                            entry.path
                        )));
                    }
                } else if after.artifact.is_none() {
                    return Err(crate::Error::ConfigError(format!(
                        "non-ghost config transaction for {} has no payload artifact",
                        entry.path
                    )));
                }
            }
            let mut auxiliary_paths = BTreeSet::new();
            for (path, artifact) in &entry.auxiliaries {
                if !valid_config_auxiliary_path(&entry.path, path) {
                    return Err(crate::Error::InvalidPath(format!(
                        "generation config auxiliary {path} does not belong to {}",
                        entry.path
                    )));
                }
                if !auxiliary_paths.insert(path.as_str()) {
                    return Err(crate::Error::ConfigError(format!(
                        "generation config transaction repeats auxiliary {path}"
                    )));
                }
                artifact.validate()?;
            }
        }
        Ok(())
    }
}

impl ConfigPathTransaction {
    /// Paths that the config engine can have mutated for this primary path.
    ///
    /// This is an exact grammar, shared by generation and mutable rollback.
    /// Numbered pacsave successors are included because libalpm rotation moves
    /// each captured entry from N to N+1.
    #[must_use]
    pub fn restore_owned_paths(&self) -> Vec<String> {
        let mut paths = BTreeSet::from([self.path.clone()]);
        for suffix in ConfigSuffix::ALL {
            paths.insert(format!("{}{}", self.path, suffix.as_str()));
        }
        let pacsave = format!("{}{}", self.path, ConfigSuffix::PacSave.as_str());
        for (path, _) in &self.auxiliaries {
            paths.insert(path.clone());
            if path == &pacsave {
                paths.insert(format!("{pacsave}.1"));
            } else if let Some(number) = path
                .strip_prefix(&(pacsave.clone() + "."))
                .and_then(|number| number.parse::<u64>().ok())
            {
                paths.insert(format!("{pacsave}.{}", number + 1));
            }
        }
        paths.into_iter().collect()
    }
}

fn valid_etc_path(path: &str) -> bool {
    let mut components = Path::new(path).components();
    matches!(components.next(), Some(Component::RootDir))
        && matches!(components.next(), Some(Component::Normal(name)) if name == "etc")
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.all(|component| matches!(component, Component::Normal(_)))
}

fn valid_config_auxiliary_path(primary: &str, candidate: &str) -> bool {
    if !valid_etc_path(candidate) {
        return false;
    }
    let suffix = candidate.strip_prefix(primary).unwrap_or_default();
    if ConfigSuffix::ALL
        .into_iter()
        .any(|known| suffix == known.as_str())
    {
        return true;
    }
    suffix
        .strip_prefix(".pacsave.")
        .and_then(|number| number.parse::<u64>().ok())
        .is_some_and(|number| number > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{PayloadIdentity, PayloadNode, PayloadTimestamp};
    use std::collections::BTreeMap;

    const OLD: &str = "old";
    const LOCAL: &str = "local";
    const NEW: &str = "new";

    fn resolved_node(kind: PayloadNodeKind, mode: u32) -> ResolvedPayloadNode {
        ResolvedPayloadNode::from_numeric_source(PayloadNode {
            kind,
            mode,
            user: PayloadIdentity::Numeric { id: 0 },
            group: PayloadIdentity::Numeric { id: 0 },
            mtime: PayloadTimestamp::UNIX_EPOCH,
            xattrs: BTreeMap::new(),
        })
        .unwrap()
    }

    fn regular(content: &[u8], mode: u32) -> ConfigArtifact {
        ConfigArtifact::regular(
            content,
            resolved_node(
                PayloadNodeKind::Regular {
                    hardlink_identity: None,
                },
                mode,
            ),
        )
        .unwrap()
    }

    #[test]
    fn alpm_three_identity_matrix_matches_documented_actions() {
        assert_eq!(
            decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(OLD), OLD),
            ConfigInstallDecision::Install
        );
        assert_eq!(
            decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(OLD), NEW),
            ConfigInstallDecision::Install
        );
        assert_eq!(
            decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(LOCAL), OLD),
            ConfigInstallDecision::Keep
        );
        assert_eq!(
            decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(NEW), NEW),
            ConfigInstallDecision::Install
        );
        assert_eq!(
            decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(LOCAL), NEW),
            ConfigInstallDecision::InstallAlternative(ConfigSuffix::PacNew)
        );
        assert_eq!(
            decide_config_install(ConfigSource::Arch, true, None, Some(LOCAL), NEW),
            ConfigInstallDecision::InstallAlternative(ConfigSuffix::PacNew)
        );
    }

    #[test]
    fn rpm_distinguishes_noreplace_save_and_first_install_backup() {
        assert_eq!(
            decide_config_install(ConfigSource::Rpm, true, Some(OLD), Some(LOCAL), NEW),
            ConfigInstallDecision::InstallAlternative(ConfigSuffix::RpmNew)
        );
        assert_eq!(
            decide_config_install(ConfigSource::Rpm, false, Some(OLD), Some(LOCAL), NEW),
            ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::RpmSave)
        );
        assert_eq!(
            decide_config_install(ConfigSource::Rpm, false, None, Some(LOCAL), NEW),
            ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::RpmOrig)
        );
    }

    #[test]
    fn dpkg_default_keeps_dual_edits_and_user_deletions() {
        assert_eq!(
            decide_config_install(ConfigSource::Deb, true, Some(OLD), Some(LOCAL), NEW),
            ConfigInstallDecision::InstallAlternative(ConfigSuffix::DpkgDist)
        );
        assert_eq!(
            decide_config_install(ConfigSource::Deb, true, Some(OLD), None, OLD),
            ConfigInstallDecision::Keep
        );
        assert_eq!(
            decide_config_install(ConfigSource::Deb, true, Some(OLD), None, NEW),
            ConfigInstallDecision::InstallAlternative(ConfigSuffix::DpkgDist)
        );
    }

    #[test]
    fn dpkg_remove_on_upgrade_removes_pristine_and_saves_modified() {
        assert_eq!(
            decide_deb_remove_on_upgrade(Some(OLD), Some(OLD)),
            ConfigRemovalDecision::Remove
        );
        assert_eq!(
            decide_deb_remove_on_upgrade(Some(OLD), Some(LOCAL)),
            ConfigRemovalDecision::SaveCurrent(ConfigSuffix::DpkgOld)
        );
        assert_eq!(
            decide_deb_remove_on_upgrade(Some(OLD), None),
            ConfigRemovalDecision::Remove
        );
    }

    #[test]
    fn removal_contracts_cover_residuals_purge_and_native_backups() {
        assert_eq!(
            decide_config_removal(ConfigSource::Deb, false, false, Some(OLD), Some(LOCAL)),
            ConfigRemovalDecision::KeepResidual
        );
        assert_eq!(
            decide_config_removal(ConfigSource::Deb, false, true, Some(OLD), Some(LOCAL)),
            ConfigRemovalDecision::Remove
        );
        assert_eq!(
            decide_config_removal(ConfigSource::Arch, false, false, Some(OLD), Some(LOCAL)),
            ConfigRemovalDecision::RotatePacsaveAndSaveCurrent
        );
    }

    #[test]
    fn durable_artifact_round_trip_validates_content_identity() {
        let artifact = regular(b"local config", 0o100640);
        let encoded = serde_json::to_string(&artifact).unwrap();
        let decoded: ConfigArtifact = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.regular_content().unwrap().unwrap(), b"local config");
        assert_eq!(decoded.node().source.mode, 0o100640);
    }

    #[test]
    fn etc_payload_contract_accepts_only_regular_files_and_symlinks() {
        assert!(is_etc_config_payload(
            "/etc/implicit.conf",
            &PayloadNodeKind::Regular {
                hardlink_identity: None
            }
        ));
        assert!(is_etc_config_payload(
            "/etc/implicit-link",
            &PayloadNodeKind::Symlink {
                target: "target".to_string()
            }
        ));
        assert!(!is_etc_config_payload(
            "/etc/directory",
            &PayloadNodeKind::Directory
        ));
        assert!(!is_etc_config_payload("/etc/fifo", &PayloadNodeKind::Fifo));
        assert!(!is_etc_config_payload(
            "/usr/lib/implicit.conf",
            &PayloadNodeKind::Regular {
                hardlink_identity: None
            }
        ));
    }

    #[test]
    fn durable_transaction_rejects_unowned_auxiliary_paths() {
        let artifact = regular(b"new", 0o100640);
        let transaction = GenerationConfigTransaction {
            entries: vec![ConfigPathTransaction {
                path: "/etc/demo.conf".to_string(),
                operation: ConfigTransactionOperation::Install,
                before: None,
                current: None,
                after: Some(ConfigPackageState {
                    source: ConfigSource::Auto,
                    noreplace: false,
                    ghost: false,
                    original_sha256: Some(artifact.sha256().to_string()),
                    artifact: Some(artifact.clone()),
                }),
                auxiliaries: vec![("/etc/other.rpmnew".to_string(), artifact)],
            }],
            ..Default::default()
        };

        assert!(transaction.validate().is_err());
    }

    #[test]
    fn restore_snapshot_is_a_v2_exact_prechange_contract() {
        let old = regular(b"local", 0o100600);
        let saved = regular(b"saved", 0o100640);
        let new = regular(b"new", 0o100644);
        let forward = GenerationConfigTransaction {
            entries: vec![ConfigPathTransaction {
                path: "/etc/demo.conf".to_string(),
                operation: ConfigTransactionOperation::Install,
                before: Some(ConfigPackageState {
                    source: ConfigSource::Arch,
                    noreplace: true,
                    ghost: false,
                    original_sha256: Some(crate::hash::sha256(b"old")),
                    artifact: None,
                }),
                current: Some(old.clone()),
                after: Some(ConfigPackageState {
                    source: ConfigSource::Arch,
                    noreplace: true,
                    ghost: false,
                    original_sha256: Some(new.sha256().to_string()),
                    artifact: Some(new),
                }),
                auxiliaries: vec![("/etc/demo.conf.pacsave".to_string(), saved.clone())],
            }],
            ..Default::default()
        };

        let restore = forward.restore_snapshot().unwrap();

        assert_eq!(restore.schema_version, 2);
        restore.validate_restore_snapshot().unwrap();
        assert_eq!(
            restore.entries[0].operation,
            ConfigTransactionOperation::Restore
        );
        assert_eq!(restore.entries[0].current.as_ref(), Some(&old));
        assert_eq!(
            restore.entries[0].auxiliaries,
            vec![("/etc/demo.conf.pacsave".to_string(), saved)]
        );
        assert!(restore.entries[0].after.is_none());
    }

    #[test]
    fn restore_owned_paths_cover_exact_suffix_grammar_and_pacsave_rotation() {
        let entry = ConfigPathTransaction {
            path: "/etc/demo.conf".to_string(),
            operation: ConfigTransactionOperation::Restore,
            before: None,
            current: None,
            after: None,
            auxiliaries: vec![
                (
                    "/etc/demo.conf.pacsave".to_string(),
                    regular(b"zero", 0o100600),
                ),
                (
                    "/etc/demo.conf.pacsave.4".to_string(),
                    regular(b"four", 0o100600),
                ),
            ],
        };

        let paths = entry.restore_owned_paths();

        for expected in [
            "/etc/demo.conf",
            "/etc/demo.conf.rpmnew",
            "/etc/demo.conf.dpkg-dist",
            "/etc/demo.conf.pacsave",
            "/etc/demo.conf.pacsave.1",
            "/etc/demo.conf.pacsave.4",
            "/etc/demo.conf.pacsave.5",
            "/etc/demo.conf.conary-save",
        ] {
            assert!(paths.iter().any(|path| path == expected), "{expected}");
        }
    }

    #[test]
    fn schema_v1_is_rejected_without_compatibility_mode() {
        let transaction = GenerationConfigTransaction {
            schema_version: 1,
            entries: Vec::new(),
        };
        assert!(transaction.validate().is_err());
    }
}
