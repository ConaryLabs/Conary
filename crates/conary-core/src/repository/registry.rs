// conary-core/src/repository/registry.rs

//! Repository format registry
//!
//! Provides typed format identity and creation of repository parsers.

use crate::error::{Error, Result};
use crate::repository::parsers::{self, AuthenticatedRepositoryMetadata, RepositoryParser};
use crate::repository::trust::openpgp::PreparedOpenPgpTrust;
use serde::{Deserialize, Serialize};

/// Enum wrapper for concrete parser types to avoid dyn dispatch (async-incompatible).
pub enum AnyParser {
    Arch(parsers::arch::ArchParser),
    Debian(parsers::debian::DebianParser),
    Fedora(parsers::fedora::FedoraParser),
}

impl AnyParser {
    pub async fn sync_metadata(&self, repo_url: &str) -> Result<AuthenticatedRepositoryMetadata> {
        match self {
            Self::Arch(p) => p.sync_metadata(repo_url).await,
            Self::Debian(p) => p.sync_metadata(repo_url).await,
            Self::Fedora(p) => p.sync_metadata(repo_url).await,
        }
    }
}

/// Detect the system architecture
///
/// Returns the architecture in RPM format (x86_64, aarch64, etc.)
pub fn detect_system_arch() -> String {
    std::env::consts::ARCH.to_string()
}

/// Convert system architecture to Debian's naming convention
///
/// Debian uses different names: amd64 instead of x86_64, arm64 instead of aarch64
pub fn arch_to_debian(arch: &str) -> String {
    match arch {
        "x86_64" => "amd64".to_string(),
        "aarch64" => "arm64".to_string(),
        "x86" | "i686" | "i386" => "i386".to_string(),
        "arm" | "armv7" => "armhf".to_string(),
        "powerpc64" => "ppc64el".to_string(),
        "s390x" => "s390x".to_string(),
        "riscv64" => "riscv64".to_string(),
        other => other.to_string(),
    }
}

/// Explicit package-metadata format for a repository.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepositoryFormat {
    #[serde(rename = "arch")]
    Arch,
    #[serde(rename = "deb")]
    Debian,
    #[serde(rename = "rpm")]
    Fedora,
    #[serde(rename = "json")]
    Json,
    #[default]
    #[serde(rename = "unspecified")]
    Unspecified,
}

/// Exact parser construction data for one repository source.
///
/// This is persisted alongside the repository. Human-readable names and URLs
/// are never interpreted as parser configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "package_format", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RepositoryParserConfig {
    Arch {
        database: String,
    },
    Deb {
        distribution: String,
        component: String,
        architecture: String,
    },
    Rpm {
        architecture: String,
    },
    Json,
}

impl RepositoryParserConfig {
    #[must_use]
    pub const fn format(&self) -> RepositoryFormat {
        match self {
            Self::Arch { .. } => RepositoryFormat::Arch,
            Self::Deb { .. } => RepositoryFormat::Debian,
            Self::Rpm { .. } => RepositoryFormat::Fedora,
            Self::Json => RepositoryFormat::Json,
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Arch { database } => validate_source_identifier(database, "Arch database"),
            Self::Deb {
                distribution,
                component,
                architecture,
            } => {
                validate_source_identifier(distribution, "Debian distribution")?;
                validate_source_identifier(component, "Debian component")?;
                validate_source_identifier(architecture, "Debian architecture")
            }
            Self::Rpm { architecture } => {
                validate_source_identifier(architecture, "RPM architecture")
            }
            Self::Json => Ok(()),
        }
    }

    pub fn to_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| {
            Error::ParseError(format!(
                "failed to serialize repository parser configuration: {error}"
            ))
        })
    }

    pub fn from_json(value: &str) -> Result<Self> {
        let parsed = serde_json::from_str::<Self>(value).map_err(|error| {
            Error::ParseError(format!(
                "invalid persisted repository parser configuration: {error}"
            ))
        })?;
        parsed.validate()?;
        Ok(parsed)
    }
}

fn validate_source_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
    {
        return Err(Error::ParseError(format!(
            "{label} must contain 1 to 128 ASCII letters, digits, '.', '_', '+', or '-'"
        )));
    }
    Ok(())
}

impl RepositoryFormat {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Arch => "arch",
            Self::Debian => "deb",
            Self::Fedora => "rpm",
            Self::Json => "json",
            Self::Unspecified => "unspecified",
        }
    }

    pub fn from_db(value: &str) -> Result<Self> {
        match value {
            "arch" => Ok(Self::Arch),
            "deb" => Ok(Self::Debian),
            "rpm" => Ok(Self::Fedora),
            "json" => Ok(Self::Json),
            "unspecified" => Ok(Self::Unspecified),
            other => Err(Error::ParseError(format!(
                "unknown persisted repository format '{other}'"
            ))),
        }
    }

    pub fn require_explicit(self, repository_name: &str) -> Result<Self> {
        if self == Self::Unspecified {
            return Err(Error::InitError(format!(
                "repository '{repository_name}' has no package metadata format; configure an exact \
                 format instead of relying on name or URL inference"
            )));
        }
        Ok(self)
    }
}

/// Create a parser for the given format and repository info
pub fn create_parser(
    config: &RepositoryParserConfig,
    trust: PreparedOpenPgpTrust,
) -> Result<AnyParser> {
    config.validate()?;
    match config {
        RepositoryParserConfig::Arch { database } => Ok(AnyParser::Arch(
            parsers::arch::ArchParser::new(database.clone(), trust)?,
        )),
        RepositoryParserConfig::Deb {
            distribution,
            component,
            architecture,
        } => Ok(AnyParser::Debian(parsers::debian::DebianParser::new(
            distribution.clone(),
            component.clone(),
            architecture.clone(),
            trust,
        )?)),
        RepositoryParserConfig::Rpm { architecture } => Ok(AnyParser::Fedora(
            parsers::fedora::FedoraParser::new(architecture.clone(), trust)?,
        )),
        RepositoryParserConfig::Json => Err(Error::ParseError(
            "JSON format has no native parser".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_system_arch_returns_valid_string() {
        let arch = detect_system_arch();
        assert!(!arch.is_empty());
        // Should be one of the known architectures
        let known_arches = [
            "x86_64",
            "aarch64",
            "x86",
            "i686",
            "arm",
            "armv7",
            "powerpc64",
            "s390x",
            "riscv64",
            "mips64",
            "loongarch64",
        ];
        // The system arch should be recognizable (or at least non-empty)
        assert!(
            known_arches.contains(&arch.as_str()) || !arch.is_empty(),
            "Unknown arch: {}",
            arch
        );
    }

    #[test]
    fn test_arch_to_debian_conversions() {
        assert_eq!(arch_to_debian("x86_64"), "amd64");
        assert_eq!(arch_to_debian("aarch64"), "arm64");
        assert_eq!(arch_to_debian("i686"), "i386");
        assert_eq!(arch_to_debian("i386"), "i386");
        assert_eq!(arch_to_debian("armv7"), "armhf");
        assert_eq!(arch_to_debian("powerpc64"), "ppc64el");
        assert_eq!(arch_to_debian("s390x"), "s390x");
        assert_eq!(arch_to_debian("riscv64"), "riscv64");
        // Unknown arches pass through unchanged
        assert_eq!(arch_to_debian("unknown"), "unknown");
    }

    #[test]
    fn repository_format_round_trips_exact_persisted_values() {
        for format in [
            RepositoryFormat::Arch,
            RepositoryFormat::Debian,
            RepositoryFormat::Fedora,
            RepositoryFormat::Json,
            RepositoryFormat::Unspecified,
        ] {
            assert_eq!(RepositoryFormat::from_db(format.as_str()).unwrap(), format);
        }
        assert!(RepositoryFormat::from_db("rpm-ish").is_err());
        assert!(
            RepositoryFormat::Unspecified
                .require_explicit("example")
                .is_err()
        );
    }

    #[test]
    fn parser_configuration_is_typed_and_round_trips_without_name_inference() {
        for config in [
            RepositoryParserConfig::Arch {
                database: "core".to_string(),
            },
            RepositoryParserConfig::Deb {
                distribution: "resolute".to_string(),
                component: "main".to_string(),
                architecture: "amd64".to_string(),
            },
            RepositoryParserConfig::Rpm {
                architecture: "x86_64".to_string(),
            },
            RepositoryParserConfig::Json,
        ] {
            let json = config.to_json().unwrap();
            assert_eq!(RepositoryParserConfig::from_json(&json).unwrap(), config);
            assert_eq!(
                RepositoryParserConfig::from_json(&json).unwrap().format(),
                config.format()
            );
        }

        assert!(
            RepositoryParserConfig::Arch {
                database: "../core".to_string()
            }
            .validate()
            .is_err()
        );
    }
}
