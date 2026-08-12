// conary-core/src/db/models/repository/source/policy.rs

//! Typed native source identity, stream, and authenticated-snapshot policy.

use crate::error::{Error, Result};
pub use crate::repository::parsers::AuthenticatedSnapshotIdentity;
use crate::repository::versioning::VersionScheme;
use crate::repository::{
    ArchKeyringTrust, ArchSignatureRequirement, ArchTrustLevel, OpenPgpTrustRoot, RepositoryFormat,
    RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};
use serde::Serialize;

pub const STREAM_BINDING_SCHEMA_REVISION: u32 = 31;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeSourceEcosystem {
    Rpm,
    Deb,
    Alpm,
}

impl NativeSourceEcosystem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Alpm => "alpm",
        }
    }

    pub const fn version_scheme(self) -> VersionScheme {
        match self {
            Self::Rpm => VersionScheme::Rpm,
            Self::Deb => VersionScheme::Debian,
            Self::Alpm => VersionScheme::Arch,
        }
    }

    pub const fn repository_format(self) -> RepositoryFormat {
        match self {
            Self::Rpm => RepositoryFormat::Fedora,
            Self::Deb => RepositoryFormat::Debian,
            Self::Alpm => RepositoryFormat::Arch,
        }
    }

    pub fn from_repository_format(format: RepositoryFormat) -> Result<Self> {
        match format {
            RepositoryFormat::Fedora => Ok(Self::Rpm),
            RepositoryFormat::Debian => Ok(Self::Deb),
            RepositoryFormat::Arch => Ok(Self::Alpm),
            RepositoryFormat::Json | RepositoryFormat::Unspecified => Err(Error::ConfigError(
                format!("'{}' is not a native repository ecosystem", format.as_str()),
            )),
        }
    }

    pub(crate) fn from_db(value: &str) -> std::result::Result<Self, String> {
        match value {
            "rpm" => Ok(Self::Rpm),
            "deb" => Ok(Self::Deb),
            "alpm" => Ok(Self::Alpm),
            other => Err(format!(
                "unknown persisted native source ecosystem '{other}'"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryPolicyScope {
    Repository { identity: String },
    Group { identity: String },
}

impl RepositoryPolicyScope {
    pub fn repository(identity: impl Into<String>) -> Result<Self> {
        let identity = identity.into();
        validate_identity(&identity, "repository policy scope")?;
        Ok(Self::Repository { identity })
    }

    pub fn group(identity: impl Into<String>) -> Result<Self> {
        let identity = identity.into();
        validate_identity(&identity, "repository policy group")?;
        Ok(Self::Group { identity })
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Repository { .. } => "repository",
            Self::Group { .. } => "group",
        }
    }

    pub fn identity(&self) -> &str {
        match self {
            Self::Repository { identity } | Self::Group { identity } => identity,
        }
    }

    pub(crate) fn from_db(kind: &str, identity: String) -> std::result::Result<Self, String> {
        validate_identity(&identity, "persisted repository policy scope")
            .map_err(|error| error.to_string())?;
        match kind {
            "repository" => Ok(Self::Repository { identity }),
            "group" => Ok(Self::Group { identity }),
            other => Err(format!(
                "unknown persisted repository policy scope '{other}'"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeSourceStream {
    Release { identity: String },
    Channel { identity: String },
    Rolling { identity: String },
}

impl NativeSourceStream {
    pub fn release(identity: impl Into<String>) -> Result<Self> {
        Self::new("release", identity.into())
    }

    pub fn channel(identity: impl Into<String>) -> Result<Self> {
        Self::new("channel", identity.into())
    }

    pub fn rolling(identity: impl Into<String>) -> Result<Self> {
        Self::new("rolling", identity.into())
    }

    fn new(kind: &str, identity: String) -> Result<Self> {
        validate_identity(&identity, "native source stream")?;
        match kind {
            "release" => Ok(Self::Release { identity }),
            "channel" => Ok(Self::Channel { identity }),
            "rolling" => Ok(Self::Rolling { identity }),
            _ => unreachable!("closed native source stream constructor"),
        }
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Release { .. } => "release",
            Self::Channel { .. } => "channel",
            Self::Rolling { .. } => "rolling",
        }
    }

    pub fn identity(&self) -> &str {
        match self {
            Self::Release { identity }
            | Self::Channel { identity }
            | Self::Rolling { identity } => identity,
        }
    }

    pub(crate) fn from_db(kind: &str, identity: String) -> std::result::Result<Self, String> {
        validate_identity(&identity, "persisted native source stream")
            .map_err(|error| error.to_string())?;
        match kind {
            "release" => Ok(Self::Release { identity }),
            "channel" => Ok(Self::Channel { identity }),
            "rolling" => Ok(Self::Rolling { identity }),
            other => Err(format!("unknown persisted native source stream '{other}'")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryUpdateMode {
    Follow,
    Pin,
}

impl RepositoryUpdateMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Pin => "pin",
        }
    }

    pub(crate) fn from_db(value: &str) -> std::result::Result<Self, String> {
        match value {
            "follow" => Ok(Self::Follow),
            "pin" => Ok(Self::Pin),
            other => Err(format!(
                "unknown persisted repository update mode '{other}'"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositorySourcePolicy {
    pub(crate) id: Option<i64>,
    pub source_identity: String,
    pub scope: RepositoryPolicyScope,
    pub ecosystem: NativeSourceEcosystem,
    pub version_scheme: VersionScheme,
    pub stream: NativeSourceStream,
    pub update_mode: RepositoryUpdateMode,
}

impl RepositorySourcePolicy {
    pub fn new(
        source_identity: impl Into<String>,
        scope: RepositoryPolicyScope,
        ecosystem: NativeSourceEcosystem,
        stream: NativeSourceStream,
        update_mode: RepositoryUpdateMode,
    ) -> Result<Self> {
        let policy = Self {
            id: None,
            source_identity: source_identity.into(),
            scope,
            ecosystem,
            version_scheme: ecosystem.version_scheme(),
            stream,
            update_mode,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<()> {
        validate_identity(&self.source_identity, "native source identity")?;
        validate_identity(self.scope.identity(), "repository policy scope")?;
        validate_identity(self.stream.identity(), "native source stream")?;
        if self.version_scheme != self.ecosystem.version_scheme() {
            return Err(Error::ConfigError(format!(
                "native source ecosystem '{}' cannot use '{}' version ordering",
                self.ecosystem.as_str(),
                self.version_scheme.as_str()
            )));
        }
        Ok(())
    }
}

pub(crate) fn validate_identity(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.trim() != value
        || !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must contain 1 to 255 printable ASCII characters without surrounding whitespace"
        )));
    }
    Ok(())
}

pub(crate) fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} SHA-256 must be exactly 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

#[derive(Serialize)]
struct StreamBindingDocument<'a> {
    schema_revision: u32,
    source_identity: &'a str,
    repository_identity: &'a str,
    stream_kind: &'a str,
    stream_identity: &'a str,
    metadata_url: &'a str,
    content_url: Option<&'a str>,
    parser: &'a RepositoryParserConfig,
    snapshot_authority: SnapshotAuthority<'a>,
}

#[derive(Serialize)]
#[serde(tag = "ecosystem", rename_all = "kebab-case")]
enum SnapshotAuthority<'a> {
    Debian {
        release_keys: &'a [OpenPgpTrustRoot],
    },
    Rpm {
        metadata: &'a RpmMetadataAuthority,
    },
    Arch {
        keyring: &'a ArchKeyringTrust,
        database_signature: ArchSignatureRequirement,
        trust: ArchTrustLevel,
    },
}

pub(crate) fn calculate_stream_binding(
    policy: &RepositorySourcePolicy,
    repository_identity: &str,
    metadata_url: &str,
    content_url: Option<&str>,
    parser: &RepositoryParserConfig,
    trust: &RepositoryTrustPolicy,
) -> Result<String> {
    policy.validate()?;
    validate_identity(repository_identity, "repository identity")?;
    parser.validate()?;
    trust.validate()?;
    if parser.format() != policy.ecosystem.repository_format()
        || trust.format() != policy.ecosystem.repository_format()
    {
        return Err(Error::ConfigError(
            "native source ecosystem, parser, and trust policy must match".to_string(),
        ));
    }

    let snapshot_authority = match trust {
        RepositoryTrustPolicy::Debian { release_keys } => {
            SnapshotAuthority::Debian { release_keys }
        }
        RepositoryTrustPolicy::Rpm { metadata, .. } => SnapshotAuthority::Rpm { metadata },
        RepositoryTrustPolicy::Arch { keyring, sig_level } => SnapshotAuthority::Arch {
            keyring,
            database_signature: sig_level.database,
            trust: sig_level.trust,
        },
    };
    let document = StreamBindingDocument {
        schema_revision: STREAM_BINDING_SCHEMA_REVISION,
        source_identity: &policy.source_identity,
        repository_identity,
        stream_kind: policy.stream.kind(),
        stream_identity: policy.stream.identity(),
        metadata_url,
        content_url,
        parser,
        snapshot_authority,
    };
    let encoded = serde_json::to_vec(&document).map_err(|error| {
        Error::ParseError(format!(
            "failed to encode native source stream binding: {error}"
        ))
    })?;
    Ok(crate::hash::sha256(&encoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_distribution_names_are_valid_typed_source_identities() {
        for (source_identity, ecosystem) in [
            ("linux-mint:22", NativeSourceEcosystem::Deb),
            ("mx-linux:23", NativeSourceEcosystem::Deb),
            ("manjaro:stable", NativeSourceEcosystem::Alpm),
            ("cachyos:rolling", NativeSourceEcosystem::Alpm),
            ("opensuse:tumbleweed", NativeSourceEcosystem::Rpm),
            ("artix:rolling", NativeSourceEcosystem::Alpm),
        ] {
            let policy = RepositorySourcePolicy::new(
                source_identity,
                RepositoryPolicyScope::repository(format!("{source_identity}:core")).unwrap(),
                ecosystem,
                NativeSourceStream::rolling("current").unwrap(),
                RepositoryUpdateMode::Follow,
            )
            .unwrap();

            assert_eq!(policy.source_identity, source_identity);
            assert_eq!(policy.version_scheme, ecosystem.version_scheme());
        }
    }
}
