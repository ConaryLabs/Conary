// conary-core/src/db/models/repository.rs

//! Repository and RepositoryPackage models - remote package sources

use crate::error::{Error, Result};
use crate::repository::dependency_model::DebianMultiArch;
use crate::repository::supported_profiles::{ProfilePackageFormat, SupportedProfile};
use crate::repository::versioning::VersionScheme;
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
}

impl RepositoryOwnership {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::RemiConfig => "remi-config",
        }
    }

    fn from_db(value: &str) -> std::result::Result<Self, String> {
        match value {
            "operator" => Ok(Self::Operator),
            "remi-config" => Ok(Self::RemiConfig),
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
}

impl Repository {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, name, url, content_url, enabled, priority, \
         trust_policy_json, metadata_expire, last_sync, created_at, \
         default_strategy, default_strategy_endpoint, source_profile, \
         tuf_enabled, tuf_root_version, tuf_root_url, security_advisory_support, \
         package_format, parser_config_json, managed_by";

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

        match (&self.parser_config, self.package_format) {
            (None, RepositoryFormat::Unspecified) => Ok(()),
            (None, format) => Err(Error::InitError(format!(
                "repository '{}' declares format '{}' without typed parser configuration",
                self.name,
                format.as_str()
            ))),
            (
                Some(_),
                RepositoryFormat::Arch | RepositoryFormat::Debian | RepositoryFormat::Fedora,
            ) => {
                self.require_parser_config()?;
                self.require_source_profile()?;
                match &self.trust_policy {
                    Some(_) => self.require_trust_policy().map(|_| ()),
                    None if !self.enabled => Ok(()),
                    None => self.require_trust_policy().map(|_| ()),
                }
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
                Ok(())
            }
            (Some(_), RepositoryFormat::Unspecified) => {
                unreachable!("typed parser configuration cannot project to unspecified format")
            }
        }
    }

    /// Insert this repository into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.validate_parser_contract()?;
        let parser_config_json = self.parser_config_json()?;
        let trust_policy_json = self.trust_policy_json()?;
        let inserted = conn.execute(
            "INSERT INTO repositories (name, url, content_url, enabled, priority, trust_policy_json, metadata_expire, default_strategy, default_strategy_endpoint, source_profile, tuf_enabled, tuf_root_version, tuf_root_url, security_advisory_support, package_format, parser_config_json, managed_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
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
        Ok(id)
    }

    /// Find a repository by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM repositories WHERE id = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        let repo = stmt.query_row([id], Self::from_row).optional()?;
        Ok(repo)
    }

    /// Find a repository by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM repositories WHERE name = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        let repo = stmt.query_row([name], Self::from_row).optional()?;
        Ok(repo)
    }

    /// List all repositories
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM repositories ORDER BY priority DESC, name",
            Self::COLUMNS
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
            "SELECT {} FROM repositories WHERE enabled = 1 ORDER BY priority DESC, name",
            Self::COLUMNS
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
        let parser_config_json = self.parser_config_json()?;
        let trust_policy_json = self.trust_policy_json()?;

        conn.execute(
            "UPDATE repositories SET name = ?1, url = ?2, content_url = ?3, enabled = ?4, priority = ?5,
             trust_policy_json = ?6, metadata_expire = ?7, last_sync = ?8,
             default_strategy = ?9, default_strategy_endpoint = ?10, source_profile = ?11,
             tuf_enabled = ?12, tuf_root_version = ?13, tuf_root_url = ?14,
             security_advisory_support = ?15, package_format = ?16, parser_config_json = ?17,
             managed_by = ?18
             WHERE id = ?19",
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
                id,
            ],
        )?;

        Ok(())
    }

    /// Delete a repository by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM repositories WHERE id = ?1", [id])?;
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
        Ok(Self {
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
        })
    }
}

/// RepositoryPackage represents a package available from a repository
#[derive(Debug, Clone)]
pub struct RepositoryPackage {
    pub id: Option<i64>,
    pub repository_id: i64,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    /// Exact Debian `Multi-Arch` behavior; absent for non-Debian packages.
    pub debian_multi_arch: Option<DebianMultiArch>,
    pub description: Option<String>,
    pub checksum: String,
    pub size: i64,
    pub download_url: String,
    pub metadata: Option<String>,
    pub synced_at: Option<String>,
    /// Whether this update is a security update
    pub is_security_update: bool,
    /// Severity level: critical, important, moderate, low
    pub severity: Option<String>,
    /// Comma-separated list of CVE IDs (e.g., "CVE-2024-1234,CVE-2024-5678")
    pub cve_ids: Option<String>,
    /// Advisory ID (e.g., "RHSA-2024:1234", "DSA-5678-1")
    pub advisory_id: Option<String>,
    /// URL to the advisory
    pub advisory_url: Option<String>,
    /// Exact public source-profile identity this package came from.
    pub source_profile: Option<String>,
    /// Exact native version grammar and comparison authority.
    pub version_scheme: VersionScheme,
    /// Cross-distro canonical identity for this package.
    pub canonical_id: Option<i64>,
}

impl RepositoryPackage {
    /// Column list for SELECT queries.
    pub const COLUMNS: &'static str = "id, repository_id, name, version, package_release, architecture, debian_multi_arch, description, \
         checksum, size, download_url, metadata, synced_at, \
         is_security_update, severity, cve_ids, advisory_id, advisory_url, \
         source_profile, version_scheme, canonical_id";

    /// Column list for SELECT queries with table alias prefix (rp.).
    pub const COLUMNS_PREFIXED: &'static str = "rp.id, rp.repository_id, rp.name, rp.version, \
         rp.package_release, rp.architecture, rp.debian_multi_arch, rp.description, rp.checksum, rp.size, rp.download_url, \
         rp.metadata, rp.synced_at, rp.is_security_update, \
         rp.severity, rp.cve_ids, rp.advisory_id, rp.advisory_url, rp.source_profile, \
         rp.version_scheme, rp.canonical_id";

    /// INSERT SQL shared by `batch_insert` and `batch_insert_with_ids`.
    const BATCH_INSERT_SQL: &'static str = "\
         INSERT INTO repository_packages \
         (repository_id, name, version, package_release, architecture, debian_multi_arch, description, checksum, size, \
          download_url, metadata, is_security_update, severity, cve_ids, \
          advisory_id, advisory_url, source_profile, version_scheme, canonical_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)";

    /// Create a new RepositoryPackage
    pub fn new(
        repository_id: i64,
        name: String,
        version: String,
        version_scheme: VersionScheme,
        checksum: String,
        size: i64,
        download_url: String,
    ) -> Self {
        Self {
            id: None,
            repository_id,
            name,
            version,
            package_release: String::new(),
            architecture: None,
            debian_multi_arch: (version_scheme == VersionScheme::Debian)
                .then_some(DebianMultiArch::No),
            description: None,
            checksum,
            size,
            download_url,
            metadata: None,
            synced_at: None,
            is_security_update: false,
            severity: None,
            cve_ids: None,
            advisory_id: None,
            advisory_url: None,
            source_profile: None,
            version_scheme,
            canonical_id: None,
        }
    }

    /// Insert this repository package into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO repository_packages
             (repository_id, name, version, package_release, architecture, debian_multi_arch, description, checksum, size, download_url, metadata,
              is_security_update, severity, cve_ids, advisory_id, advisory_url, source_profile, version_scheme, canonical_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                &self.repository_id,
                &self.name,
                &self.version,
                &self.package_release,
                &self.architecture,
                self.debian_multi_arch.map(DebianMultiArch::as_str),
                &self.description,
                &self.checksum,
                &self.size,
                &self.download_url,
                &self.metadata,
                self.is_security_update as i32,
                &self.severity,
                &self.cve_ids,
                &self.advisory_id,
                &self.advisory_url,
                &self.source_profile,
                self.version_scheme.as_str(),
                &self.canonical_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a repository package by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages WHERE id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let pkg = stmt.query_row([id], Self::from_row).optional()?;
        Ok(pkg)
    }

    /// Find repository packages by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages WHERE name = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Find repository packages by repository ID
    pub fn find_by_repository(conn: &Connection, repository_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages WHERE repository_id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([repository_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Search repository packages by pattern (name or description)
    ///
    /// The pattern is escaped for SQL LIKE to prevent `%` and `_` in user
    /// input from acting as wildcards. Uses `\` as the LIKE escape character.
    pub fn search(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        // Escape SQL LIKE wildcards in user input
        let escaped = pattern
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let search_pattern = format!("%{escaped}%");
        let sql = format!(
            "SELECT {} FROM repository_packages \
             WHERE name LIKE ?1 ESCAPE '\\' OR description LIKE ?1 ESCAPE '\\' \
             ORDER BY name, version",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([&search_pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Delete all packages for a repository (used when syncing)
    pub fn delete_by_repository(conn: &Connection, repository_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_packages WHERE repository_id = ?1",
            [repository_id],
        )?;
        Ok(())
    }

    /// Delete a specific package by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM repository_packages WHERE id = ?1", [id])?;
        Ok(())
    }

    /// List all packages in all enabled repositories
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages rp \
             JOIN repositories r ON rp.repository_id = r.id \
             WHERE r.enabled = 1 \
             ORDER BY rp.name, rp.version",
            Self::COLUMNS_PREFIXED
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Find packages in enabled repositories whose metadata JSON contains `dependency_name`
    /// as a literal substring.  This is a coarse pre-filter; callers must re-check the
    /// parsed provides list for an exact match.
    pub fn find_in_enabled_repos_with_metadata_like(
        conn: &Connection,
        dependency_name: &str,
    ) -> Result<Vec<Self>> {
        let pattern = format!("%{dependency_name}%");
        let sql = format!(
            "SELECT {} FROM repository_packages rp \
             JOIN repositories r ON rp.repository_id = r.id \
             WHERE r.enabled = 1 AND rp.metadata LIKE ?1",
            Self::COLUMNS_PREFIXED
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([&pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Find a specific package by name and version in enabled repositories
    pub fn find_by_name_version(
        conn: &Connection,
        name: &str,
        version: &str,
    ) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages rp \
             JOIN repositories r ON rp.repository_id = r.id \
             WHERE r.enabled = 1 AND rp.name = ?1 AND rp.version = ?2",
            Self::COLUMNS_PREFIXED
        );
        let mut stmt = conn.prepare(&sql)?;
        let pkg = stmt.query_row([name, version], Self::from_row).optional()?;
        Ok(pkg)
    }

    /// Get repository name for this package
    pub fn get_repository_name(&self, conn: &Connection) -> Result<String> {
        if let Some(repo) = Repository::find_by_id(conn, self.repository_id)? {
            Ok(repo.name)
        } else {
            Ok("unknown".to_string())
        }
    }

    /// Format size as human-readable string
    pub fn size_human(&self) -> String {
        super::format_size(self.size)
    }

    /// Convert a database row to a RepositoryPackage
    pub fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            repository_id: row.get(1)?,
            name: row.get(2)?,
            version: row.get(3)?,
            package_release: row.get(4)?,
            architecture: row.get(5)?,
            debian_multi_arch: debian_multi_arch_from_row(row, 6)?,
            description: row.get(7)?,
            checksum: row.get(8)?,
            size: row.get(9)?,
            download_url: row.get(10)?,
            metadata: row.get(11)?,
            synced_at: row.get(12)?,
            is_security_update: row.get::<_, i32>(13)? != 0,
            severity: row.get(14)?,
            cve_ids: row.get(15)?,
            advisory_id: row.get(16)?,
            advisory_url: row.get(17)?,
            source_profile: row.get(18)?,
            version_scheme: version_scheme_from_row(row, 19)?,
            canonical_id: row.get(20)?,
        })
    }

    /// Find all security updates available
    pub fn find_security_updates(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM repository_packages rp \
             JOIN repositories r ON rp.repository_id = r.id \
             WHERE r.enabled = 1 AND rp.is_security_update = 1 \
             ORDER BY CASE rp.severity \
                 WHEN 'critical' THEN 1 \
                 WHEN 'important' THEN 2 \
                 WHEN 'moderate' THEN 3 \
                 WHEN 'low' THEN 4 \
                 ELSE 5 \
             END, rp.name",
            Self::COLUMNS_PREFIXED
        );
        let mut stmt = conn.prepare(&sql)?;
        let packages = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(packages)
    }

    /// Batch insert multiple repository packages efficiently
    ///
    /// Uses a prepared statement within a transaction for much better performance
    /// than individual inserts. For 77k packages, this reduces sync time from
    /// ~5 minutes to ~10 seconds.
    ///
    /// Caller must wrap this in a transaction for atomicity.
    pub fn batch_insert(conn: &Connection, packages: &[Self]) -> Result<usize> {
        if packages.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(Self::BATCH_INSERT_SQL)?;

        for pkg in packages {
            Self::execute_batch_row(&mut stmt, pkg)?;
        }

        Ok(packages.len())
    }

    /// Batch insert repository packages and populate their generated IDs.
    pub fn batch_insert_with_ids(conn: &Connection, packages: &mut [Self]) -> Result<usize> {
        if packages.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(Self::BATCH_INSERT_SQL)?;

        for pkg in packages.iter_mut() {
            Self::execute_batch_row(&mut stmt, pkg)?;
            pkg.id = Some(conn.last_insert_rowid());
        }

        Ok(packages.len())
    }

    /// Execute a single batch INSERT row with the shared parameter list.
    fn execute_batch_row(stmt: &mut rusqlite::CachedStatement<'_>, pkg: &Self) -> Result<()> {
        stmt.execute(params![
            &pkg.repository_id,
            &pkg.name,
            &pkg.version,
            &pkg.package_release,
            &pkg.architecture,
            pkg.debian_multi_arch.map(DebianMultiArch::as_str),
            &pkg.description,
            &pkg.checksum,
            &pkg.size,
            &pkg.download_url,
            &pkg.metadata,
            pkg.is_security_update as i32,
            &pkg.severity,
            &pkg.cve_ids,
            &pkg.advisory_id,
            &pkg.advisory_url,
            &pkg.source_profile,
            pkg.version_scheme.as_str(),
            &pkg.canonical_id,
        ])?;
        Ok(())
    }
}

pub(crate) fn version_scheme_from_row(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<VersionScheme> {
    let raw = row.get::<_, String>(index)?;
    raw.parse().map_err(|error: String| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(io::Error::new(io::ErrorKind::InvalidData, error)),
        )
    })
}

fn debian_multi_arch_from_row(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<DebianMultiArch>> {
    let Some(raw) = row.get::<_, Option<String>>(index)? else {
        return Ok(None);
    };
    DebianMultiArch::parse_exact(&raw)
        .map(Some)
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(io::Error::new(io::ErrorKind::InvalidData, error)),
            )
        })
}

#[cfg(test)]
#[path = "repository/tests.rs"]
mod tests;
