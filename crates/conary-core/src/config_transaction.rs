// conary-core/src/config_transaction.rs

//! Exact package-format configuration-file transaction contracts.
//!
//! Package metadata selects the contract. File identities select the action.
//! Paths, package names, distro names, and script text never select semantics.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path};

use crate::db::models::ConfigSource;
use crate::payload::{PayloadContentAuthority, PayloadNodeKind, ResolvedPayloadNode};

pub const GENERATION_CONFIG_TRANSACTION_SCHEMA_VERSION: u32 = 5;

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
    EopkgNew,
    ConaryNew,
    ConarySave,
}

impl ConfigSuffix {
    pub const ALL: [Self; 10] = [
        Self::RpmNew,
        Self::RpmSave,
        Self::RpmOrig,
        Self::DpkgDist,
        Self::DpkgOld,
        Self::PacNew,
        Self::PacSave,
        Self::EopkgNew,
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
            Self::EopkgNew => ".newconfig",
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
            ConfigSource::Eopkg => {
                ConfigInstallDecision::InstallAlternative(ConfigSuffix::EopkgNew)
            }
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
        ConfigSource::Eopkg => ConfigInstallDecision::InstallAlternative(ConfigSuffix::EopkgNew),
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

/// Source-owned transition from a packaged config artifact to declaration-only
/// ownership. RPM and libalpm deliberately disagree here, so the incoming
/// declaration must select the contract before either filesystem planner runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDematerializationContract {
    /// RPM `rpmfilesDecideFate()` skips an incoming `%ghost` and preserves
    /// whatever is currently on disk.
    RpmGhost,
    /// libalpm removes an old backup file omitted from the incoming file list,
    /// saving a locally modified primary as `.pacsave`.
    AlpmBackupWithoutPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDematerializationDecision {
    PreserveCurrent,
    Remove,
    RotatePacsaveAndSaveCurrent,
}

/// Resolve the exact source contract for a materialized-to-declaration-only
/// update. Other source/flag combinations have no upstream transition and
/// fail before mutation instead of inheriting removal behavior accidentally.
pub fn config_dematerialization_contract(
    source: ConfigSource,
    incoming_ghost: bool,
) -> crate::Result<ConfigDematerializationContract> {
    match (source, incoming_ghost) {
        (ConfigSource::Rpm, true) => Ok(ConfigDematerializationContract::RpmGhost),
        (ConfigSource::Arch, false) => {
            Ok(ConfigDematerializationContract::AlpmBackupWithoutPayload)
        }
        _ => Err(crate::Error::ConfigError(format!(
            "{source} has no materialized-to-declaration-only config contract with ghost={incoming_ghost}"
        ))),
    }
}

/// Decide the primary and auxiliary filesystem outcome for one exact
/// dematerialization contract and the old/current content identities.
#[must_use]
pub fn decide_config_dematerialization(
    contract: ConfigDematerializationContract,
    old: Option<&str>,
    current: Option<&str>,
) -> ConfigDematerializationDecision {
    match contract {
        ConfigDematerializationContract::RpmGhost => {
            ConfigDematerializationDecision::PreserveCurrent
        }
        ConfigDematerializationContract::AlpmBackupWithoutPayload => {
            if current.is_some() && current != old {
                ConfigDematerializationDecision::RotatePacsaveAndSaveCurrent
            } else {
                ConfigDematerializationDecision::Remove
            }
        }
    }
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
        ConfigSource::Eopkg => ConfigRemovalDecision::KeepResidual,
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
        content: PayloadContentAuthority,
        node: ResolvedPayloadNode,
    },
    Symlink {
        sha256: String,
        target: String,
        node: ResolvedPayloadNode,
    },
}

impl ConfigArtifact {
    pub fn regular(
        content: PayloadContentAuthority,
        node: ResolvedPayloadNode,
    ) -> crate::Result<Self> {
        if !matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
            return Err(crate::Error::InvalidPath(
                "regular config artifact requires a regular payload node".to_string(),
            ));
        }
        node.validate()
            .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
        content
            .validate()
            .map_err(|error| crate::Error::ConfigError(error.to_string()))?;
        Ok(Self::Regular { content, node })
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
            Self::Regular { content, .. } => &content.sha256,
            Self::Symlink { sha256, .. } => sha256,
        }
    }

    #[must_use]
    pub fn content_authority(&self) -> Option<&PayloadContentAuthority> {
        match self {
            Self::Regular { content, .. } => Some(content),
            Self::Symlink { .. } => None,
        }
    }

    #[must_use]
    pub fn node(&self) -> &ResolvedPayloadNode {
        match self {
            Self::Regular { node, .. } | Self::Symlink { node, .. } => node,
        }
    }

    pub fn validate(&self) -> crate::Result<()> {
        match self {
            Self::Regular { content, node } => {
                node.validate()
                    .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
                if !matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
                    return Err(crate::Error::InvalidPath(
                        "regular config artifact has non-regular payload authority".to_string(),
                    ));
                }
                content
                    .validate()
                    .map_err(|error| crate::Error::ConfigError(error.to_string()))?;
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
#[serde(deny_unknown_fields)]
pub struct ConfigPackageState {
    pub source: ConfigSource,
    pub noreplace: bool,
    pub ghost: bool,
    /// Whether the declaration identifies an installed payload artifact.
    pub materialized: bool,
    /// Exact package-owned baseline identity. It remains available for
    /// Debian residual conffiles after their payload row has been removed.
    pub original_sha256: Option<String>,
    pub artifact: Option<ConfigArtifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigTransactionOperation {
    Install,
    /// Remove the prior package artifact while retaining an exact incoming
    /// declaration that has no matching payload entry.
    Dematerialize,
    RemoveOnUpgrade,
    Remove,
    Purge,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
    /// Exact CAS-backed regular content referenced by this durable transaction.
    pub fn regular_content_authorities(&self) -> impl Iterator<Item = &PayloadContentAuthority> {
        self.entries
            .iter()
            .flat_map(|entry| {
                entry
                    .before
                    .iter()
                    .filter_map(|state| state.artifact.as_ref())
                    .chain(entry.current.iter())
                    .chain(
                        entry
                            .after
                            .iter()
                            .filter_map(|state| state.artifact.as_ref()),
                    )
                    .chain(entry.auxiliaries.iter().map(|(_, artifact)| artifact))
            })
            .filter_map(ConfigArtifact::content_authority)
    }

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
            if matches!(
                entry.operation,
                ConfigTransactionOperation::Install | ConfigTransactionOperation::Dematerialize
            ) && entry.after.is_none()
            {
                return Err(crate::Error::ConfigError(format!(
                    "generation config {:?} for {} has no after state",
                    entry.operation, entry.path
                )));
            }
            if !matches!(
                entry.operation,
                ConfigTransactionOperation::Install | ConfigTransactionOperation::Dematerialize
            ) && entry.after.is_some()
            {
                return Err(crate::Error::ConfigError(format!(
                    "generation config removal for {} has an after state",
                    entry.path
                )));
            }
            if matches!(
                entry.operation,
                ConfigTransactionOperation::Dematerialize
                    | ConfigTransactionOperation::Remove
                    | ConfigTransactionOperation::Purge
            ) && entry.before.is_none()
            {
                return Err(crate::Error::ConfigError(format!(
                    "generation config {:?} for {} has no exact prior package state",
                    entry.operation, entry.path
                )));
            }
            if entry.operation == ConfigTransactionOperation::Dematerialize {
                let before = entry.before.as_ref().expect("checked above");
                let after = entry.after.as_ref().expect("checked above");
                if !before.materialized || after.materialized {
                    return Err(crate::Error::ConfigError(format!(
                        "generation config dematerialization for {} must replace a materialized artifact with a declaration-only state",
                        entry.path
                    )));
                }
                if before.source != after.source {
                    return Err(crate::Error::ConfigError(format!(
                        "generation config dematerialization for {} changes source authority from {} to {}",
                        entry.path, before.source, after.source
                    )));
                }
                config_dematerialization_contract(after.source, after.ghost)?;
            }
            if entry.operation == ConfigTransactionOperation::RemoveOnUpgrade {
                let Some(before) = entry.before.as_ref() else {
                    return Err(crate::Error::ConfigError(format!(
                        "remove-on-upgrade config transaction for {} has no exact prior package state",
                        entry.path
                    )));
                };
                if before.source != ConfigSource::Deb {
                    return Err(crate::Error::ConfigError(format!(
                        "remove-on-upgrade config transaction for {} has non-Debian prior source {}",
                        entry.path, before.source
                    )));
                }
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
            for (position, state) in [
                ("before", entry.before.as_ref()),
                ("after", entry.after.as_ref()),
            ] {
                let Some(state) = state else {
                    continue;
                };
                if state.ghost {
                    if state.original_sha256.is_some() || state.artifact.is_some() {
                        return Err(crate::Error::ConfigError(format!(
                            "ghost config transaction {position} state for {} carries a payload identity",
                            entry.path,
                        )));
                    }
                } else if !state.materialized {
                    if state.original_sha256.is_some() || state.artifact.is_some() {
                        return Err(crate::Error::ConfigError(format!(
                            "non-materialized config transaction {position} state for {} carries a payload identity",
                            entry.path,
                        )));
                    }
                } else {
                    let hash = state.original_sha256.as_deref().ok_or_else(|| {
                        crate::Error::ConfigError(format!(
                            "non-ghost config transaction {position} state for {} has no exact package SHA-256",
                            entry.path
                        ))
                    })?;
                    validate_canonical_sha256(
                        hash,
                        &format!(
                            "config transaction {position} package identity for {}",
                            entry.path
                        ),
                    )?;
                    if position == "after" && state.artifact.is_none() {
                        return Err(crate::Error::ConfigError(format!(
                            "non-ghost config transaction for {} has no payload artifact",
                            entry.path
                        )));
                    }
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
                .and_then(parse_canonical_positive_decimal)
                .and_then(|number| number.checked_add(1))
            {
                paths.insert(format!("{pacsave}.{number}"));
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
        .and_then(parse_canonical_positive_decimal)
        .is_some()
}

/// Parse libalpm's canonical positive decimal `.pacsave.N` suffix.
///
/// This rejects lookalike spellings such as `00`, `01`, and `+1`.
#[must_use]
pub fn parse_canonical_positive_decimal(value: &str) -> Option<u64> {
    let number = value.parse::<u64>().ok()?;
    (number > 0 && number.to_string() == value).then_some(number)
}

fn validate_canonical_sha256(value: &str, context: &str) -> crate::Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(crate::Error::ConfigError(format!(
            "{context} must be a lowercase raw 64-hex SHA-256 digest"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_transaction/tests.rs"]
mod tests;
