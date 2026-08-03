// conary-core/src/packages/config_authority.rs

//! Closed source configuration declarations and transaction projection.

use crate::db::models::ConfigSource;
use crate::packages::arch::authority::AlpmConfigDeclaration;
use crate::packages::deb::authority::DebianConfigDeclaration;
use crate::packages::rpm::authority::RpmConfigDeclaration;
use serde::{Deserialize, Serialize};

/// Whether a source declaration identifies an incoming payload node.
///
/// Absence is declaration authority, never permission to synthesize a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigPayloadAssociation {
    Matched,
    Absent,
}

/// Configuration declaration authored directly in a CCS manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CcsConfigDeclaration {
    pub path: String,
    pub noreplace: bool,
    pub payload: ConfigPayloadAssociation,
}

/// Exact signed source declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceConfigDeclaration {
    Rpm(RpmConfigDeclaration),
    Debian(DebianConfigDeclaration),
    Alpm(AlpmConfigDeclaration),
    Ccs(CcsConfigDeclaration),
}

impl SourceConfigDeclaration {
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Rpm(value) => &value.path,
            Self::Debian(value) => &value.path,
            Self::Alpm(value) => &value.path,
            Self::Ccs(value) => &value.path,
        }
    }

    #[must_use]
    pub const fn payload(&self) -> ConfigPayloadAssociation {
        match self {
            Self::Rpm(value) => value.payload,
            Self::Debian(value) => value.payload,
            Self::Alpm(value) => value.payload,
            Self::Ccs(value) => value.payload,
        }
    }

    #[must_use]
    pub const fn source(&self) -> ConfigSource {
        match self {
            Self::Rpm(_) => ConfigSource::Rpm,
            Self::Debian(_) => ConfigSource::Deb,
            Self::Alpm(_) => ConfigSource::Arch,
            Self::Ccs(_) => ConfigSource::Auto,
        }
    }

    #[must_use]
    pub const fn noreplace(&self) -> bool {
        match self {
            Self::Rpm(value) => value.noreplace,
            Self::Debian(_) | Self::Alpm(_) => true,
            Self::Ccs(value) => value.noreplace,
        }
    }

    #[must_use]
    pub const fn ghost(&self) -> bool {
        matches!(self, Self::Rpm(value) if value.ghost)
    }

    #[must_use]
    pub const fn remove_on_upgrade(&self) -> bool {
        matches!(self, Self::Debian(value) if value.remove_on_upgrade)
    }

    /// Validate source semantics before they cross into transaction planning.
    pub fn validate(&self) -> crate::Result<()> {
        let path = self.path();
        if !path.starts_with('/') {
            return Err(crate::Error::InvalidPath(format!(
                "config declaration path must be absolute: {path}"
            )));
        }
        let normalized = crate::filesystem::path::sanitize_path(path)?;
        if format!("/{}", normalized.display()) != path {
            return Err(crate::Error::InvalidPath(format!(
                "config declaration path must be canonical: {path}"
            )));
        }
        match self {
            Self::Rpm(value) => {
                let expected = if value.ghost {
                    ConfigPayloadAssociation::Absent
                } else {
                    ConfigPayloadAssociation::Matched
                };
                if value.payload != expected {
                    return Err(crate::Error::ConfigError(format!(
                        "RPM config declaration {path} has contradictory payload association"
                    )));
                }
            }
            Self::Debian(value) => {
                let expected = if value.remove_on_upgrade {
                    ConfigPayloadAssociation::Absent
                } else {
                    ConfigPayloadAssociation::Matched
                };
                if value.payload != expected {
                    return Err(crate::Error::ConfigError(format!(
                        "Debian config declaration {path} has contradictory payload association"
                    )));
                }
            }
            Self::Alpm(_) => {}
            Self::Ccs(value) if value.payload == ConfigPayloadAssociation::Absent => {
                return Err(crate::Error::ConfigError(format!(
                    "direct CCS config declaration {path} cannot omit its payload"
                )));
            }
            Self::Ccs(_) => {}
        }
        Ok(())
    }
}
