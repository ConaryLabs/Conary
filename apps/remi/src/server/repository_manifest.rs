// apps/remi/src/server/repository_manifest.rs

//! Typed, declarative package-source authority for Remi.

use anyhow::{Context, Result, bail};
use conary_core::db::models::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
    RepositoryOwnership, RepositoryPolicyScope, RepositorySourcePolicy, RepositoryUpdateMode,
    SecurityAdvisorySupport,
};
use conary_core::repository::supported_profiles::{ProfilePackageFormat, profile_by_public_id};
use conary_core::repository::{
    OpenPgpTrustRoot, RepositoryFormat, RepositoryParserConfig, RepositoryTrustPolicy,
    RpmMetadataAuthority,
};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

pub const REPOSITORY_MANIFEST_SCHEMA: u32 = 3;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryManifest {
    pub schema_version: u32,
    #[serde(default)]
    pub repositories: Vec<RepositoryDefinition>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryDefinition {
    pub name: String,
    pub url: String,
    pub content_url: Option<String>,
    pub parser: RepositoryParserConfig,
    pub profile: String,
    pub source_identity: String,
    pub repository_identity: String,
    pub stream_kind: String,
    pub stream_identity: String,
    pub policy_group: Option<String>,
    pub update_mode: String,
    pub pinned_snapshot_sha256: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub trust: RepositoryTrustPolicy,
    pub metadata_expire_seconds: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileResult {
    pub inserted: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

impl RepositoryManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read repository manifest {}", path.display()))?;
        let manifest: Self = toml::from_str(&content)
            .with_context(|| format!("failed to parse repository manifest {}", path.display()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != REPOSITORY_MANIFEST_SCHEMA {
            bail!(
                "repository manifest schema {} is unsupported; expected {}",
                self.schema_version,
                REPOSITORY_MANIFEST_SCHEMA
            );
        }

        let mut names = BTreeSet::new();
        for repository in &self.repositories {
            repository.validate()?;
            if !names.insert(repository.name.as_str()) {
                bail!(
                    "repository manifest contains duplicate name '{}'",
                    repository.name
                );
            }
        }
        Ok(())
    }

    pub fn reconcile(&self, conn: &mut Connection) -> Result<ReconcileResult> {
        self.validate()?;
        let tx = conn.transaction()?;
        let mut result = ReconcileResult::default();
        let desired_names = self
            .repositories
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<BTreeSet<_>>();

        for definition in &self.repositories {
            match Repository::find_by_name(&tx, &definition.name)? {
                Some(mut existing) => {
                    if existing.managed_by != RepositoryOwnership::RemiConfig {
                        bail!(
                            "repository '{}' is operator-owned and cannot be replaced by the Remi \
                             repository manifest",
                            definition.name
                        );
                    }

                    let source_changed = definition.source_differs(&existing)?;
                    let changed = definition.differs(&existing)?;
                    if source_changed {
                        let repository_id = existing.id.context("repository row has no ID")?;
                        Repository::delete(&tx, repository_id)?;
                        let mut replacement = definition.to_repository()?;
                        replacement.insert(&tx)?;
                        result.updated += 1;
                    } else if changed {
                        definition.apply_to(&mut existing)?;
                        existing.update(&tx)?;
                        result.updated += 1;
                    } else {
                        result.unchanged += 1;
                    }
                }
                None => {
                    let mut repository = definition.to_repository()?;
                    repository.insert(&tx)?;
                    result.inserted += 1;
                }
            }
        }

        for repository in Repository::list_all(&tx)? {
            if repository.managed_by == RepositoryOwnership::RemiConfig
                && !desired_names.contains(repository.name.as_str())
            {
                Repository::delete(&tx, repository.id.context("repository row has no ID")?)?;
                result.removed += 1;
            }
        }

        tx.commit()?;
        Ok(result)
    }

    pub fn verify_reconciled(&self, conn: &Connection) -> Result<()> {
        self.validate()?;
        let desired_names = self
            .repositories
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<BTreeSet<_>>();

        for definition in &self.repositories {
            let repository =
                Repository::find_by_name(conn, &definition.name)?.with_context(|| {
                    format!("configured repository '{}' is missing", definition.name)
                })?;
            if repository.managed_by != RepositoryOwnership::RemiConfig {
                bail!(
                    "configured repository '{}' is not owned by the Remi manifest",
                    definition.name
                );
            }
            if definition.differs(&repository)? {
                bail!(
                    "configured repository '{}' does not match the installed manifest",
                    definition.name
                );
            }
        }

        for repository in Repository::list_all(conn)? {
            if repository.managed_by == RepositoryOwnership::RemiConfig
                && !desired_names.contains(repository.name.as_str())
            {
                bail!(
                    "repository '{}' is Remi-config-owned but absent from the installed manifest",
                    repository.name
                );
            }
        }
        Ok(())
    }
}

impl RepositoryDefinition {
    fn validate(&self) -> Result<()> {
        if self.name.is_empty() || self.name.len() > 128 {
            bail!("repository name must contain 1 to 128 characters");
        }
        if self.name.trim() != self.name {
            bail!("repository '{}' has surrounding whitespace", self.name);
        }
        validate_https_url(&self.url, &format!("repository '{}'.url", self.name))?;
        if let Some(content_url) = self.content_url.as_deref() {
            validate_https_url(
                content_url,
                &format!("repository '{}'.content_url", self.name),
            )?;
        }
        if self.metadata_expire_seconds < 0 {
            bail!(
                "repository '{}'.metadata_expire_seconds must not be negative",
                self.name
            );
        }
        self.parser.validate()?;
        self.trust.validate()?;
        if let RepositoryTrustPolicy::Rpm {
            metadata: RpmMetadataAuthority::Metalink { url },
            ..
        } = &self.trust
        {
            validate_https_url(url, &format!("repository '{}' RPM metalink", self.name))?;
        }
        if let RepositoryTrustPolicy::Arch { keyring, .. } = &self.trust {
            validate_https_url(
                &keyring.url,
                &format!("repository '{}' Arch keyring", self.name),
            )?;
        }
        if self.trust.format() != self.parser.format() {
            bail!(
                "repository '{}' trust policy '{}' conflicts with parser format '{}'",
                self.name,
                self.trust.format().as_str(),
                self.parser.format().as_str()
            );
        }
        self.native_policy()?;
        for root in trust_roots(&self.trust) {
            validate_https_url(
                &root.url,
                &format!(
                    "repository '{}' trust root '{}'",
                    self.name, root.fingerprint
                ),
            )?;
        }

        let profile = profile_by_public_id(&self.profile).with_context(|| {
            format!(
                "repository '{}' names unsupported profile '{}'",
                self.name, self.profile
            )
        })?;
        let expected = match profile.package_format() {
            ProfilePackageFormat::Rpm => RepositoryFormat::Fedora,
            ProfilePackageFormat::Deb => RepositoryFormat::Debian,
            ProfilePackageFormat::Arch => RepositoryFormat::Arch,
        };
        if self.parser.format() != expected {
            bail!(
                "repository '{}' format '{}' conflicts with profile '{}' format '{}'",
                self.name,
                self.parser.format().as_str(),
                self.profile,
                expected.as_str()
            );
        }
        Ok(())
    }

    fn to_repository(&self) -> Result<Repository> {
        let mut repository = Repository::new(self.name.clone(), self.url.clone());
        self.apply_to(&mut repository)?;
        Ok(repository)
    }

    fn apply_to(&self, repository: &mut Repository) -> Result<()> {
        let authenticated_snapshot = repository.authenticated_snapshot.clone();
        repository.url.clone_from(&self.url);
        repository.content_url.clone_from(&self.content_url);
        repository.enabled = self.enabled;
        repository.priority = self.priority;
        repository.trust_policy = Some(self.trust.clone());
        repository.metadata_expire = self.metadata_expire_seconds;
        repository.default_strategy = None;
        repository.default_strategy_endpoint = None;
        repository.source_profile = Some(self.profile.clone());
        repository.tuf_enabled = false;
        repository.tuf_root_version = None;
        repository.tuf_root_url = None;
        repository.security_advisory_support = SecurityAdvisorySupport::Unknown;
        repository.package_format = self.parser.format();
        repository.parser_config = Some(self.parser.clone());
        repository.managed_by = RepositoryOwnership::RemiConfig;
        let (policy, pinned_snapshot) = self.native_policy()?;
        repository.set_native_source_policy(
            policy,
            self.repository_identity.clone(),
            pinned_snapshot,
        )?;
        repository.authenticated_snapshot = authenticated_snapshot;
        Ok(())
    }

    fn source_differs(&self, repository: &Repository) -> Result<bool> {
        let expected = self.to_repository()?;
        Ok(repository.url != self.url
            || repository.content_url != self.content_url
            || repository.parser_config.as_ref() != Some(&self.parser)
            || repository.trust_policy.as_ref() != Some(&self.trust)
            || repository.source_profile.as_deref() != Some(self.profile.as_str())
            || repository.repository_identity.as_deref() != Some(self.repository_identity.as_str())
            || !same_source_policy(
                repository.source_policy.as_ref(),
                expected.source_policy.as_ref(),
            )
            || repository.stream_binding_sha256 != expected.stream_binding_sha256)
    }

    fn differs(&self, repository: &Repository) -> Result<bool> {
        Ok(self.source_differs(repository)?
            || repository.enabled != self.enabled
            || repository.priority != self.priority
            || repository.metadata_expire != self.metadata_expire_seconds
            || repository.default_strategy.is_some()
            || repository.default_strategy_endpoint.is_some()
            || repository.tuf_enabled
            || repository.tuf_root_version.is_some()
            || repository.tuf_root_url.is_some()
            || repository.security_advisory_support != SecurityAdvisorySupport::Unknown
            || repository.managed_by != RepositoryOwnership::RemiConfig)
    }

    fn native_policy(
        &self,
    ) -> Result<(
        RepositorySourcePolicy,
        Option<AuthenticatedSnapshotIdentity>,
    )> {
        let ecosystem = NativeSourceEcosystem::from_repository_format(self.parser.format())?;
        let stream = match self.stream_kind.as_str() {
            "release" => NativeSourceStream::release(self.stream_identity.clone())?,
            "channel" => NativeSourceStream::channel(self.stream_identity.clone())?,
            "rolling" => NativeSourceStream::rolling(self.stream_identity.clone())?,
            other => bail!(
                "repository '{}' has unknown stream kind '{other}'",
                self.name
            ),
        };
        let scope = match &self.policy_group {
            Some(group) => RepositoryPolicyScope::group(group.clone())?,
            None => RepositoryPolicyScope::repository(self.repository_identity.clone())?,
        };
        let (update_mode, pinned_snapshot) = match (
            self.update_mode.as_str(),
            self.pinned_snapshot_sha256.as_deref(),
        ) {
            ("follow", None) => (RepositoryUpdateMode::Follow, None),
            ("pin", Some(sha256)) => (
                RepositoryUpdateMode::Pin,
                Some(AuthenticatedSnapshotIdentity::from_sha256(sha256)?),
            ),
            ("follow", Some(_)) => bail!(
                "repository '{}' follow policy cannot declare pinned_snapshot_sha256",
                self.name
            ),
            ("pin", None) => bail!(
                "repository '{}' pin policy requires pinned_snapshot_sha256",
                self.name
            ),
            (other, _) => bail!(
                "repository '{}' has unknown update mode '{other}'",
                self.name
            ),
        };
        Ok((
            RepositorySourcePolicy::new(
                self.source_identity.clone(),
                scope,
                ecosystem,
                stream,
                update_mode,
            )?,
            pinned_snapshot,
        ))
    }
}

fn same_source_policy(
    left: Option<&RepositorySourcePolicy>,
    right: Option<&RepositorySourcePolicy>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.source_identity == right.source_identity
                && left.scope == right.scope
                && left.ecosystem == right.ecosystem
                && left.version_scheme == right.version_scheme
                && left.stream == right.stream
                && left.update_mode == right.update_mode
        }
        (None, None) => true,
        _ => false,
    }
}

fn trust_roots(policy: &RepositoryTrustPolicy) -> Vec<&OpenPgpTrustRoot> {
    match policy {
        RepositoryTrustPolicy::Debian { release_keys } => release_keys.iter().collect(),
        RepositoryTrustPolicy::Rpm {
            metadata,
            package_keys,
        } => match metadata {
            RpmMetadataAuthority::OpenPgp { keys } => keys.iter().chain(package_keys).collect(),
            RpmMetadataAuthority::Metalink { .. } => package_keys.iter().collect(),
        },
        RepositoryTrustPolicy::Arch { .. } => Vec::new(),
    }
}

fn validate_https_url(value: &str, field: &str) -> Result<()> {
    let parsed = url::Url::parse(value).with_context(|| format!("{field} is not a valid URL"))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        bail!("{field} must be an absolute HTTPS URL");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        (temp_file, conn)
    }

    fn manifest() -> RepositoryManifest {
        RepositoryManifest {
            schema_version: REPOSITORY_MANIFEST_SCHEMA,
            repositories: vec![RepositoryDefinition {
                name: "opaque-source".to_string(),
                url: "https://packages.example.test/repository".to_string(),
                content_url: None,
                parser: RepositoryParserConfig::Rpm {
                    architecture: "x86_64".to_string(),
                },
                profile: "fedora-44".to_string(),
                source_identity: "fedora-project".to_string(),
                repository_identity: "fedora-44-everything-x86_64".to_string(),
                stream_kind: "release".to_string(),
                stream_identity: "44".to_string(),
                policy_group: None,
                update_mode: "follow".to_string(),
                pinned_snapshot_sha256: None,
                enabled: true,
                priority: 100,
                trust: RepositoryTrustPolicy::Rpm {
                    metadata: RpmMetadataAuthority::OpenPgp {
                        keys: vec![OpenPgpTrustRoot {
                            url: "https://keys.example.test/rpm-metadata.asc".to_string(),
                            fingerprint: "A".repeat(40),
                        }],
                    },
                    package_keys: vec![OpenPgpTrustRoot {
                        url: "https://keys.example.test/rpm-package.asc".to_string(),
                        fingerprint: "B".repeat(40),
                    }],
                },
                metadata_expire_seconds: 21_600,
            }],
        }
    }

    #[test]
    fn exact_format_and_profile_are_required() {
        let mut value = manifest();
        value.repositories[0].parser = RepositoryParserConfig::Rpm {
            architecture: "../host".to_string(),
        };
        assert!(value.validate().is_err());

        value.repositories[0].parser = RepositoryParserConfig::Arch {
            database: "core".to_string(),
        };
        assert!(value.validate().is_err());

        value.repositories[0].parser = RepositoryParserConfig::Rpm {
            architecture: "x86_64".to_string(),
        };
        value.repositories[0].profile = "fedora".to_string();
        assert!(value.validate().is_err());
    }

    #[test]
    fn reconcile_is_idempotent_and_config_owned() {
        let (_temp, mut conn) = create_test_db();
        let desired = manifest();

        assert_eq!(
            desired.reconcile(&mut conn).unwrap(),
            ReconcileResult {
                inserted: 1,
                ..ReconcileResult::default()
            }
        );
        assert_eq!(
            desired.reconcile(&mut conn).unwrap(),
            ReconcileResult {
                unchanged: 1,
                ..ReconcileResult::default()
            }
        );

        let repository = Repository::find_by_name(&conn, "opaque-source")
            .unwrap()
            .unwrap();
        assert_eq!(repository.package_format, RepositoryFormat::Fedora);
        assert_eq!(repository.managed_by, RepositoryOwnership::RemiConfig);
        assert_eq!(repository.source_profile.as_deref(), Some("fedora-44"));
    }

    #[test]
    fn reconcile_does_not_take_over_operator_repository() {
        let (_temp, mut conn) = create_test_db();
        let mut definition = manifest().repositories.remove(0);
        definition.url = "https://operator.example.test/repository".to_string();
        let mut existing = definition.to_repository().unwrap();
        existing.managed_by = RepositoryOwnership::Operator;
        existing.insert(&conn).unwrap();

        let error = manifest().reconcile(&mut conn).unwrap_err().to_string();
        assert!(error.contains("operator-owned"));
        assert_eq!(
            Repository::find_by_name(&conn, "opaque-source")
                .unwrap()
                .unwrap()
                .url,
            "https://operator.example.test/repository"
        );
    }
}
