// conary-core/src/db/models/repository/source.rs

//! Repository source identity, policy, trust, validation, and persistence.

mod policy;

pub use policy::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream,
    RepositoryPolicyScope, RepositorySourcePolicy, RepositoryUpdateMode,
};

use crate::error::{Error, Result};
use crate::repository::supported_profiles::{ProfilePackageFormat, SupportedProfile};
use crate::repository::{RepositoryFormat, RepositoryParserConfig, RepositoryTrustPolicy};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityAdvisorySupport {
    Unknown,
    Unsupported,
    Supported,
}

impl SecurityAdvisorySupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::Supported => "supported",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "supported" => Self::Supported,
            "unsupported" => Self::Unsupported,
            _ => Self::Unknown,
        }
    }

    pub fn is_supported(self) -> bool {
        self == Self::Supported
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RepositoryOwnership {
    #[default]
    Operator,
    RemiConfig,
    NativeProjection,
    PackageProjection,
}

impl RepositoryOwnership {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::RemiConfig => "remi-config",
            Self::NativeProjection => "native-projection",
            Self::PackageProjection => "package-projection",
        }
    }

    fn from_db(value: &str) -> std::result::Result<Self, String> {
        match value {
            "operator" => Ok(Self::Operator),
            "remi-config" => Ok(Self::RemiConfig),
            "native-projection" => Ok(Self::NativeProjection),
            "package-projection" => Ok(Self::PackageProjection),
            other => Err(format!("unknown persisted repository ownership '{other}'")),
        }
    }
}

/// Repository represents a remote package source
#[derive(Debug, Clone)]
pub struct Repository {
    pub id: Option<i64>,
    pub name: String,
    /// Metadata URL (for repomd.xml, package lists, signatures)
    pub url: String,
    /// Content URL for package downloads (reference mirror)
    /// If None, uses the same URL as metadata
    pub content_url: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    /// Exact ecosystem-native authority used to authenticate metadata and
    /// package payloads. Native repositories require a matching policy.
    pub trust_policy: Option<RepositoryTrustPolicy>,
    pub metadata_expire: i32,
    pub last_sync: Option<String>,
    pub created_at: Option<String>,
    /// Default resolution strategy for packages without explicit routing entries
    /// Values: "binary", "remi", or None
    pub default_strategy: Option<String>,
    /// For "remi" strategy: the Remi server endpoint URL
    pub default_strategy_endpoint: Option<String>,
    /// Exact public source profile served by this repository. Values are
    /// profile IDs, never family names, route aliases, or package formats.
    pub source_profile: Option<String>,
    /// Whether TUF trust verification is enabled for this repository
    pub tuf_enabled: bool,
    /// Current verified TUF root metadata version
    pub tuf_root_version: Option<i64>,
    /// URL for fetching TUF root metadata (if different from repo URL)
    pub tuf_root_url: Option<String>,
    /// Whether this source publishes security-advisory metadata Conary can trust.
    pub security_advisory_support: SecurityAdvisorySupport,
    /// Exact package-manager metadata grammar used by this source.
    pub package_format: RepositoryFormat,
    /// Exact typed construction data for the package-manager metadata parser.
    pub parser_config: Option<RepositoryParserConfig>,
    /// Authority that owns the repository definition.
    pub managed_by: RepositoryOwnership,
    /// Exact native source policy; absent for Remi, static, JSON, and unspecified sources.
    pub source_policy: Option<RepositorySourcePolicy>,
    /// Opaque exact identity of this repository inside its source policy.
    pub repository_identity: Option<String>,
    /// Revision-31 drift binding over the declared stream inputs.
    pub stream_binding_sha256: Option<String>,
    /// Exact authenticated metadata root required by a pin policy.
    pub pinned_snapshot: Option<AuthenticatedSnapshotIdentity>,
}

impl Repository {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "r.id, r.name, r.url, r.content_url, r.enabled, r.priority, \
         r.trust_policy_json, r.metadata_expire, r.last_sync, r.created_at, \
         r.default_strategy, r.default_strategy_endpoint, r.source_profile, \
         r.tuf_enabled, r.tuf_root_version, r.tuf_root_url, r.security_advisory_support, \
         r.package_format, r.parser_config_json, r.managed_by, r.repository_identity, \
         r.stream_binding_sha256, \
         sp.id, sp.source_identity, sp.scope_kind, sp.scope_identity, sp.ecosystem, \
         sp.version_scheme, sp.stream_kind, sp.stream_identity, sp.update_mode, pin.snapshot_sha256";
    const FROM: &'static str = "repositories r \
         LEFT JOIN repository_source_policies sp ON sp.id = r.source_policy_id \
         LEFT JOIN repository_source_pins pin ON pin.repository_id = r.id";

    /// Create a new Repository
    pub fn new(name: String, url: String) -> Self {
        Self {
            id: None,
            name,
            url,
            content_url: None,
            enabled: true,
            priority: 0,
            trust_policy: None,
            metadata_expire: 3600, // Default: 1 hour
            last_sync: None,
            created_at: None,
            default_strategy: None,
            default_strategy_endpoint: None,
            source_profile: None,
            tuf_enabled: false,
            tuf_root_version: None,
            tuf_root_url: None,
            security_advisory_support: SecurityAdvisorySupport::Unknown,
            package_format: RepositoryFormat::Unspecified,
            parser_config: None,
            managed_by: RepositoryOwnership::Operator,
            source_policy: None,
            repository_identity: None,
            stream_binding_sha256: None,
            pinned_snapshot: None,
        }
    }

    /// Create a new Repository with a content mirror (reference mirror pattern)
    pub fn with_content_mirror(name: String, metadata_url: String, content_url: String) -> Self {
        let mut repo = Self::new(name, metadata_url);
        repo.content_url = Some(content_url);
        repo
    }

    /// Get the effective URL for downloading content
    /// Returns content_url if set, otherwise falls back to url
    pub fn effective_content_url(&self) -> &str {
        self.content_url.as_deref().unwrap_or(&self.url)
    }

    pub fn set_parser_config(&mut self, config: RepositoryParserConfig) -> Result<()> {
        config.validate()?;
        self.package_format = config.format();
        self.parser_config = Some(config);
        Ok(())
    }

    pub fn set_trust_policy(&mut self, policy: RepositoryTrustPolicy) -> Result<()> {
        policy.validate()?;
        if self.package_format != RepositoryFormat::Unspecified
            && policy.format() != self.package_format
        {
            return Err(Error::ConfigError(format!(
                "repository '{}' parser format '{}' cannot use '{}' trust policy",
                self.name,
                self.package_format.as_str(),
                policy.format().as_str()
            )));
        }
        self.trust_policy = Some(policy);
        Ok(())
    }

    pub fn set_native_source_policy(
        &mut self,
        mut policy: RepositorySourcePolicy,
        repository_identity: impl Into<String>,
        pinned_snapshot: Option<AuthenticatedSnapshotIdentity>,
    ) -> Result<()> {
        let repository_identity = repository_identity.into();
        policy::validate_identity(&repository_identity, "repository identity")?;
        policy.validate()?;
        if let RepositoryPolicyScope::Repository { identity } = &policy.scope
            && identity != &repository_identity
        {
            return Err(Error::ConfigError(format!(
                "repository-scope policy identity '{identity}' does not match repository identity '{repository_identity}'"
            )));
        }
        match (policy.update_mode, &pinned_snapshot) {
            (RepositoryUpdateMode::Follow, None) | (RepositoryUpdateMode::Pin, Some(_)) => {}
            (RepositoryUpdateMode::Follow, Some(_)) => {
                return Err(Error::ConfigError(
                    "follow policy cannot declare a pinned snapshot".to_string(),
                ));
            }
            (RepositoryUpdateMode::Pin, None) => {
                return Err(Error::ConfigError(
                    "pin policy requires an authenticated snapshot SHA-256".to_string(),
                ));
            }
        }
        let parser = self.require_parser_config()?;
        let trust = self.require_trust_policy()?;
        if policy.ecosystem.repository_format() != self.package_format {
            return Err(Error::ConfigError(format!(
                "native source ecosystem '{}' conflicts with repository parser '{}'",
                policy.ecosystem.as_str(),
                self.package_format.as_str()
            )));
        }
        let binding = policy::calculate_stream_binding(
            &policy,
            &repository_identity,
            &self.url,
            self.content_url.as_deref(),
            parser,
            trust,
        )?;
        policy.id = self.source_policy.as_ref().and_then(|existing| {
            let mut candidate = policy.clone();
            candidate.id = existing.id;
            (candidate == *existing).then_some(existing.id).flatten()
        });
        self.source_policy = Some(policy);
        self.repository_identity = Some(repository_identity);
        self.stream_binding_sha256 = Some(binding);
        self.pinned_snapshot = pinned_snapshot;
        Ok(())
    }

    pub fn require_source_policy(&self) -> Result<&RepositorySourcePolicy> {
        let policy = self.source_policy.as_ref().ok_or_else(|| {
            Error::ConfigError(format!(
                "repository '{}' has no exact native source policy",
                self.name
            ))
        })?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate_stream_binding(&self) -> Result<()> {
        let policy = self.require_source_policy()?;
        let repository_identity = self.repository_identity.as_deref().ok_or_else(|| {
            Error::ConfigError(format!(
                "repository '{}' has no exact repository identity",
                self.name
            ))
        })?;
        let persisted = self.stream_binding_sha256.as_deref().ok_or_else(|| {
            Error::ConfigError(format!(
                "repository '{}' has no native stream binding",
                self.name
            ))
        })?;
        policy::validate_sha256(persisted, "native source stream binding")?;
        let current = policy::calculate_stream_binding(
            policy,
            repository_identity,
            &self.url,
            self.content_url.as_deref(),
            self.require_parser_config()?,
            self.require_trust_policy()?,
        )?;
        if current != persisted {
            return Err(Error::ConfigError(format!(
                "repository '{}' native stream inputs changed; explicit re-enrollment is required",
                self.name
            )));
        }
        Ok(())
    }

    /// Validate a newly authenticated native metadata root without persisting
    /// it as mutable repository state. Immutable Remi catalogs own revision
    /// identity; native pin policy remains the only repository-level check.
    pub fn validate_authenticated_snapshot(
        &self,
        candidate: &AuthenticatedSnapshotIdentity,
    ) -> Result<()> {
        self.validate_stream_binding()?;
        let policy = self.require_source_policy()?;
        if policy.update_mode == RepositoryUpdateMode::Pin {
            let pinned = self.pinned_snapshot.as_ref().ok_or_else(|| {
                Error::ConfigError(format!(
                    "repository '{}' has a pin policy without a member pin",
                    self.name
                ))
            })?;
            if pinned != candidate {
                return Err(Error::TrustError(format!(
                    "repository '{}' authenticated snapshot {} does not match pinned snapshot {}",
                    self.name,
                    candidate.sha256(),
                    pinned.sha256()
                )));
            }
        }
        Ok(())
    }

    pub fn require_trust_policy(&self) -> Result<&RepositoryTrustPolicy> {
        let policy = self.trust_policy.as_ref().ok_or_else(|| {
            Error::ConfigError(format!(
                "repository '{}' has no ecosystem-native trust policy",
                self.name
            ))
        })?;
        policy.validate()?;
        if policy.format() != self.package_format {
            return Err(Error::ConfigError(format!(
                "repository '{}' trust policy is '{}' but its parser format is '{}'",
                self.name,
                policy.format().as_str(),
                self.package_format.as_str()
            )));
        }
        Ok(policy)
    }

    pub fn require_parser_config(&self) -> Result<&RepositoryParserConfig> {
        let config = self.parser_config.as_ref().ok_or_else(|| {
            Error::InitError(format!(
                "repository '{}' has no typed parser configuration",
                self.name
            ))
        })?;
        config.validate()?;
        if config.format() != self.package_format {
            return Err(Error::InitError(format!(
                "repository '{}' parser configuration is '{}' but its format projection is '{}'",
                self.name,
                config.format().as_str(),
                self.package_format.as_str()
            )));
        }
        Ok(config)
    }

    /// Return the exact public source profile served by this repository.
    pub fn require_source_profile(&self) -> Result<&'static SupportedProfile> {
        let profile_id = self.source_profile.as_deref().ok_or_else(|| {
            Error::ConfigError(format!(
                "repository '{}' has no exact source profile",
                self.name
            ))
        })?;
        let profile = crate::repository::supported_profiles::profile_by_public_id(profile_id)
            .ok_or_else(|| {
                Error::ConfigError(format!(
                    "repository '{}' declares unsupported source profile '{}'",
                    self.name, profile_id
                ))
            })?;

        let compatible = matches!(
            (self.package_format, profile.package_format()),
            (RepositoryFormat::Fedora, ProfilePackageFormat::Rpm)
                | (RepositoryFormat::Debian, ProfilePackageFormat::Deb)
                | (RepositoryFormat::Arch, ProfilePackageFormat::Arch)
                | (RepositoryFormat::Eopkg, ProfilePackageFormat::Eopkg)
                | (RepositoryFormat::Json | RepositoryFormat::Unspecified, _)
        );
        if !compatible {
            return Err(Error::ConfigError(format!(
                "repository '{}' parser format '{}' conflicts with source profile '{}' format '{}'",
                self.name,
                self.package_format.as_str(),
                profile.id(),
                profile.package_format().as_str()
            )));
        }
        Ok(profile)
    }

    /// Return the exact profile authority for dependency resolution.
    ///
    /// Native CCS repositories are already bound by their signed repository
    /// identity. A distro-specific CCS repository may additionally carry the
    /// exact source profile preserved by converted packages; a distro-neutral
    /// CCS repository has no profile. Every foreign-metadata repository must
    /// name one exact supported profile.
    pub fn resolution_source_profile(&self) -> Result<Option<&'static SupportedProfile>> {
        if self.default_strategy.as_deref() == Some("static") {
            if self.package_format != RepositoryFormat::Unspecified || self.parser_config.is_some()
            {
                return Err(Error::ConfigError(format!(
                    "static CCS repository '{}' cannot declare a foreign package parser",
                    self.name
                )));
            }
            return self
                .source_profile
                .as_deref()
                .map(|_| self.require_source_profile())
                .transpose();
        }

        self.require_source_profile().map(Some)
    }

    /// Return the exact source identity used by dependency resolution.
    ///
    /// Native repositories derive this from their persisted, stream-bound
    /// source policy. Static/Remi repositories may use an exact public feed
    /// profile as a source identity projection, but the public profile catalog
    /// never gates native repository eligibility.
    pub fn resolution_source_identity(&self) -> Result<Option<&str>> {
        if let Some(policy) = self.source_policy.as_ref() {
            policy.validate()?;
            return Ok(Some(policy.source_identity.as_str()));
        }

        self.resolution_source_profile()
            .map(|profile| profile.map(SupportedProfile::id))
    }

    fn parser_config_json(&self) -> Result<Option<String>> {
        self.parser_config
            .as_ref()
            .map(RepositoryParserConfig::to_json)
            .transpose()
    }

    fn trust_policy_json(&self) -> Result<Option<String>> {
        self.trust_policy
            .as_ref()
            .map(RepositoryTrustPolicy::to_json)
            .transpose()
    }

    fn validate_parser_contract(&self) -> Result<()> {
        match self.default_strategy.as_deref() {
            None | Some("binary") | Some("static") => {}
            Some("remi") => {
                if self.default_strategy_endpoint.is_none() {
                    return Err(Error::ConfigError(format!(
                        "repository '{}' declares Remi resolution without an endpoint",
                        self.name
                    )));
                }
                let profile = self.source_profile.as_deref().ok_or_else(|| {
                    Error::ConfigError(format!(
                        "repository '{}' declares Remi resolution without a public profile",
                        self.name
                    ))
                })?;
                if crate::repository::supported_profiles::profile_by_public_id(profile).is_none() {
                    return Err(Error::ConfigError(format!(
                        "repository '{}' declares unsupported Remi profile '{}'",
                        self.name, profile
                    )));
                }
            }
            Some(other) => {
                return Err(Error::ConfigError(format!(
                    "repository '{}' declares unsupported default strategy '{}'",
                    self.name, other
                )));
            }
        }

        if self.source_profile.is_some() {
            self.require_source_profile()?;
        }

        match (&self.parser_config, self.package_format) {
            (None, RepositoryFormat::Unspecified) => self.validate_no_native_source_policy(),
            (None, format) => Err(Error::InitError(format!(
                "repository '{}' declares format '{}' without typed parser configuration",
                self.name,
                format.as_str()
            ))),
            (
                Some(_),
                RepositoryFormat::Arch
                | RepositoryFormat::Debian
                | RepositoryFormat::Fedora
                | RepositoryFormat::Eopkg,
            ) => {
                self.require_parser_config()?;
                self.require_trust_policy()?;
                let policy = self.require_source_policy()?;
                match (policy.update_mode, &self.pinned_snapshot) {
                    (RepositoryUpdateMode::Follow, None) | (RepositoryUpdateMode::Pin, Some(_)) => {
                    }
                    (RepositoryUpdateMode::Follow, Some(_)) => {
                        return Err(Error::ConfigError(format!(
                            "repository '{}' has a follow policy with a member pin",
                            self.name
                        )));
                    }
                    (RepositoryUpdateMode::Pin, None) => {
                        return Err(Error::ConfigError(format!(
                            "repository '{}' has a pin policy without a member pin",
                            self.name
                        )));
                    }
                }
                self.validate_stream_binding()
            }
            (Some(_), RepositoryFormat::Json) => {
                self.require_parser_config()?;
                if self.trust_policy.is_some() {
                    return Err(Error::ConfigError(format!(
                        "repository '{}' uses Conary JSON metadata; native package trust policy is \
                         not applicable",
                        self.name
                    )));
                }
                self.validate_no_native_source_policy()
            }
            (Some(_), RepositoryFormat::Unspecified) => {
                unreachable!("typed parser configuration cannot project to unspecified format")
            }
        }
    }

    fn validate_no_native_source_policy(&self) -> Result<()> {
        if self.source_policy.is_some()
            || self.repository_identity.is_some()
            || self.stream_binding_sha256.is_some()
            || self.pinned_snapshot.is_some()
        {
            return Err(Error::ConfigError(format!(
                "repository '{}' is not a native metadata source and cannot declare native source policy state",
                self.name
            )));
        }
        Ok(())
    }

    /// Insert this repository into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.validate_parser_contract()?;
        if Self::find_by_name(conn, &self.name)?.is_some() {
            return Err(Error::ConflictError(format!(
                "repository '{}' already exists",
                self.name
            )));
        }
        let parser_config_json = self.parser_config_json()?;
        let trust_policy_json = self.trust_policy_json()?;
        let source_policy_id = self.ensure_source_policy(conn)?;
        let inserted = conn.execute(
            "INSERT INTO repositories (name, url, content_url, enabled, priority, trust_policy_json, metadata_expire, default_strategy, default_strategy_endpoint, source_profile, tuf_enabled, tuf_root_version, tuf_root_url, security_advisory_support, package_format, parser_config_json, managed_by, source_policy_id, repository_identity, stream_binding_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
             ON CONFLICT(name) DO NOTHING",
            params![
                &self.name,
                &self.url,
                &self.content_url,
                self.enabled as i32,
                &self.priority,
                trust_policy_json,
                &self.metadata_expire,
                &self.default_strategy,
                &self.default_strategy_endpoint,
                &self.source_profile,
                self.tuf_enabled as i32,
                &self.tuf_root_version,
                &self.tuf_root_url,
                self.security_advisory_support.as_str(),
                self.package_format.as_str(),
                parser_config_json,
                self.managed_by.as_str(),
                source_policy_id,
                &self.repository_identity,
                &self.stream_binding_sha256,
            ],
        )?;
        if inserted == 0 {
            return Err(Error::ConflictError(format!(
                "repository '{}' already exists",
                self.name
            )));
        }

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        if let Some(pin) = &self.pinned_snapshot {
            conn.execute(
                "INSERT INTO repository_source_pins (repository_id, snapshot_sha256) VALUES (?1, ?2)",
                params![id, pin.sha256()],
            )?;
        }
        Ok(id)
    }

    fn ensure_source_policy(&mut self, conn: &Connection) -> Result<Option<i64>> {
        let Some(policy) = self.source_policy.as_mut() else {
            return Ok(None);
        };
        policy.validate()?;
        let existing = conn
            .query_row(
                "SELECT id, source_identity, scope_kind, scope_identity, ecosystem, version_scheme,
                        stream_kind, stream_identity, update_mode
                 FROM repository_source_policies
                 WHERE source_identity = ?1 AND scope_kind = ?2 AND scope_identity = ?3",
                params![
                    &policy.source_identity,
                    policy.scope.kind(),
                    policy.scope.identity()
                ],
                |row| source_policy_from_row(row, 0),
            )
            .optional()?;
        if let Some(existing) = existing {
            let mut expected = policy.clone();
            expected.id = existing.id;
            if existing != expected {
                return Err(Error::ConflictError(format!(
                    "native source policy scope '{}:{}' already exists with different authority",
                    policy.scope.kind(),
                    policy.scope.identity()
                )));
            }
            if matches!(policy.scope, RepositoryPolicyScope::Repository { .. }) {
                let members: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM repositories WHERE source_policy_id = ?1",
                    [existing.id.expect("persisted policy has an id")],
                    |row| row.get(0),
                )?;
                if members != 0 {
                    return Err(Error::ConflictError(format!(
                        "repository-scope native source policy '{}' already has a member",
                        policy.scope.identity()
                    )));
                }
            }
            policy.id = existing.id;
            return Ok(existing.id);
        }

        conn.execute(
            "INSERT INTO repository_source_policies
             (source_identity, scope_kind, scope_identity, ecosystem, version_scheme,
              stream_kind, stream_identity, update_mode)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &policy.source_identity,
                policy.scope.kind(),
                policy.scope.identity(),
                policy.ecosystem.as_str(),
                policy.version_scheme.as_str(),
                policy.stream.kind(),
                policy.stream.identity(),
                policy.update_mode.as_str(),
            ],
        )?;
        let id = conn.last_insert_rowid();
        policy.id = Some(id);
        Ok(Some(id))
    }

    /// Find a repository by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM {} WHERE r.id = ?1",
            Self::COLUMNS,
            Self::FROM
        );
        let mut stmt = conn.prepare(&sql)?;
        let repo = stmt.query_row([id], Self::from_row).optional()?;
        Ok(repo)
    }

    /// Find a repository by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM {} WHERE r.name = ?1",
            Self::COLUMNS,
            Self::FROM
        );
        let mut stmt = conn.prepare(&sql)?;
        let repo = stmt.query_row([name], Self::from_row).optional()?;
        Ok(repo)
    }

    /// List all repositories
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM {} ORDER BY r.priority DESC, r.name",
            Self::COLUMNS,
            Self::FROM
        );
        let mut stmt = conn.prepare(&sql)?;
        let repos = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(repos)
    }

    /// List enabled repositories
    pub fn list_enabled(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM {} WHERE r.enabled = 1 ORDER BY r.priority DESC, r.name",
            Self::COLUMNS,
            Self::FROM
        );
        let mut stmt = conn.prepare(&sql)?;
        let repos = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(repos)
    }

    /// Update repository metadata
    pub fn update(&self, conn: &Connection) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::MissingId("Cannot update repository without ID".to_string())
        })?;
        self.validate_parser_contract()?;
        let persisted_enrollment = conn
            .query_row(
                "SELECT repository.source_policy_id,
                        repository.repository_identity,
                        repository.stream_binding_sha256,
                        pin.snapshot_sha256
                 FROM repositories repository
                 LEFT JOIN repository_source_pins pin ON pin.repository_id = repository.id
                 WHERE repository.id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("repository ID {id} does not exist")))?;
        let requested_enrollment = (
            self.source_policy.as_ref().and_then(|policy| policy.id),
            self.repository_identity.clone(),
            self.stream_binding_sha256.clone(),
            self.pinned_snapshot
                .as_ref()
                .map(|snapshot| snapshot.sha256().to_string()),
        );
        if requested_enrollment != persisted_enrollment {
            return Err(Error::ConfigError(format!(
                "repository '{}' native source enrollment changed; explicit re-enrollment is required",
                self.name
            )));
        }
        let parser_config_json = self.parser_config_json()?;
        let trust_policy_json = self.trust_policy_json()?;

        conn.execute(
            "UPDATE repositories SET name = ?1, url = ?2, content_url = ?3, enabled = ?4, priority = ?5,
             trust_policy_json = ?6, metadata_expire = ?7, last_sync = ?8,
             default_strategy = ?9, default_strategy_endpoint = ?10, source_profile = ?11,
             tuf_enabled = ?12, tuf_root_version = ?13, tuf_root_url = ?14,
             security_advisory_support = ?15, package_format = ?16, parser_config_json = ?17,
             managed_by = ?18, repository_identity = ?19, stream_binding_sha256 = ?20
             WHERE id = ?21",
            params![
                &self.name,
                &self.url,
                &self.content_url,
                self.enabled as i32,
                &self.priority,
                trust_policy_json,
                &self.metadata_expire,
                &self.last_sync,
                &self.default_strategy,
                &self.default_strategy_endpoint,
                &self.source_profile,
                self.tuf_enabled as i32,
                &self.tuf_root_version,
                &self.tuf_root_url,
                self.security_advisory_support.as_str(),
                self.package_format.as_str(),
                parser_config_json,
                self.managed_by.as_str(),
                &self.repository_identity,
                &self.stream_binding_sha256,
                id,
            ],
        )?;

        Ok(())
    }

    /// Delete a repository by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        let source_policy_id = conn
            .query_row(
                "SELECT source_policy_id FROM repositories WHERE id = ?1",
                [id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();
        conn.execute("DELETE FROM repositories WHERE id = ?1", [id])?;
        if let Some(source_policy_id) = source_policy_id {
            conn.execute(
                "DELETE FROM repository_source_policies
                 WHERE id = ?1
                   AND NOT EXISTS (
                       SELECT 1 FROM repositories WHERE source_policy_id = ?1
                   )",
                [source_policy_id],
            )?;
        }
        Ok(())
    }

    /// Convert a database row to a Repository
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let package_format_value = row.get::<_, String>(17)?;
        let package_format = RepositoryFormat::from_db(&package_format_value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                17,
                rusqlite::types::Type::Text,
                Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    error.to_string(),
                )),
            )
        })?;
        let parser_config = row
            .get::<_, Option<String>>(18)?
            .map(|value| RepositoryParserConfig::from_json(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    18,
                    rusqlite::types::Type::Text,
                    Box::new(io::Error::new(
                        io::ErrorKind::InvalidData,
                        error.to_string(),
                    )),
                )
            })?;
        let trust_policy = row
            .get::<_, Option<String>>(6)?
            .map(|value| RepositoryTrustPolicy::from_json(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(io::Error::new(
                        io::ErrorKind::InvalidData,
                        error.to_string(),
                    )),
                )
            })?;
        let ownership_value = row.get::<_, String>(19)?;
        let managed_by = RepositoryOwnership::from_db(&ownership_value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                19,
                rusqlite::types::Type::Text,
                Box::new(io::Error::new(io::ErrorKind::InvalidData, error)),
            )
        })?;
        let source_policy = if row.get::<_, Option<i64>>(22)?.is_some() {
            Some(source_policy_from_row(row, 22)?)
        } else {
            None
        };
        let pinned_snapshot = row
            .get::<_, Option<String>>(31)?
            .map(AuthenticatedSnapshotIdentity::from_sha256)
            .transpose()
            .map_err(|error| row_conversion_error(31, error.to_string()))?;
        let repository = Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            url: row.get(2)?,
            content_url: row.get(3)?,
            enabled: row.get::<_, i32>(4)? != 0,
            priority: row.get(5)?,
            trust_policy,
            metadata_expire: row.get(7)?,
            last_sync: row.get(8)?,
            created_at: row.get(9)?,
            default_strategy: row.get(10)?,
            default_strategy_endpoint: row.get(11)?,
            source_profile: row.get(12)?,
            tuf_enabled: row.get::<_, i32>(13)? != 0,
            tuf_root_version: row.get(14)?,
            tuf_root_url: row.get(15)?,
            security_advisory_support: SecurityAdvisorySupport::from_db(
                row.get::<_, String>(16)?.as_str(),
            ),
            package_format,
            parser_config,
            managed_by,
            source_policy,
            repository_identity: row.get(20)?,
            stream_binding_sha256: row.get(21)?,
            pinned_snapshot,
        };
        repository
            .validate_parser_contract()
            .map_err(|error| row_conversion_error(20, error.to_string()))?;
        Ok(repository)
    }
}

fn source_policy_from_row(
    row: &Row<'_>,
    offset: usize,
) -> rusqlite::Result<RepositorySourcePolicy> {
    let scope =
        RepositoryPolicyScope::from_db(&row.get::<_, String>(offset + 2)?, row.get(offset + 3)?)
            .map_err(|error| row_conversion_error(offset + 2, error))?;
    let ecosystem = NativeSourceEcosystem::from_db(&row.get::<_, String>(offset + 4)?)
        .map_err(|error| row_conversion_error(offset + 4, error))?;
    let version_scheme = row
        .get::<_, String>(offset + 5)?
        .parse()
        .map_err(|error: String| row_conversion_error(offset + 5, error))?;
    let stream =
        NativeSourceStream::from_db(&row.get::<_, String>(offset + 6)?, row.get(offset + 7)?)
            .map_err(|error| row_conversion_error(offset + 6, error))?;
    let update_mode = RepositoryUpdateMode::from_db(&row.get::<_, String>(offset + 8)?)
        .map_err(|error| row_conversion_error(offset + 8, error))?;
    let policy = RepositorySourcePolicy {
        id: Some(row.get(offset)?),
        source_identity: row.get(offset + 1)?,
        scope,
        ecosystem,
        version_scheme,
        stream,
        update_mode,
    };
    policy
        .validate()
        .map_err(|error| row_conversion_error(offset + 1, error.to_string()))?;
    Ok(policy)
}

fn row_conversion_error(index: usize, error: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(io::Error::new(io::ErrorKind::InvalidData, error)),
    )
}
