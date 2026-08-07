// conary-core/src/packages/installed_identity.rs

//! Typed identity for one exact native package-manager database record.

use crate::error::{Error, Result};
use crate::repository::dependency_model::SourcePackageFormat;
use crate::repository::versioning::VersionScheme;
use serde::{Deserialize, Serialize};

/// Exact identity and query selector for one installed native package record.
///
/// The selector is emitted by the owning package manager and is persisted so
/// adoption, refresh, and takeover never have to reconstruct or guess a native
/// query target from a Conary trove name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "manager", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InstalledPackageIdentity {
    Rpm {
        selector: String,
        name: String,
        epoch: Option<u64>,
        version: String,
        release: String,
        architecture: String,
    },
    Dpkg {
        selector: String,
        name: String,
        version: String,
        architecture: String,
    },
    Pacman {
        selector: String,
        name: String,
        version: String,
        architecture: String,
    },
}

impl InstalledPackageIdentity {
    /// Exact source package format that owns this native identity.
    pub const fn source_package_format(&self) -> SourcePackageFormat {
        match self {
            Self::Rpm { .. } => SourcePackageFormat::Rpm,
            Self::Dpkg { .. } => SourcePackageFormat::Debian,
            Self::Pacman { .. } => SourcePackageFormat::Alpm,
        }
    }

    /// Exact version grammar owned by this native package-manager identity.
    pub const fn version_scheme(&self) -> VersionScheme {
        self.source_package_format().version_scheme()
    }

    pub fn rpm(
        selector: impl Into<String>,
        name: impl Into<String>,
        epoch: Option<u64>,
        version: impl Into<String>,
        release: impl Into<String>,
        architecture: impl Into<String>,
    ) -> Result<Self> {
        Self::validated(Self::Rpm {
            selector: selector.into(),
            name: name.into(),
            epoch,
            version: version.into(),
            release: release.into(),
            architecture: architecture.into(),
        })
    }

    pub fn dpkg(
        selector: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        architecture: impl Into<String>,
    ) -> Result<Self> {
        Self::validated(Self::Dpkg {
            selector: selector.into(),
            name: name.into(),
            version: version.into(),
            architecture: architecture.into(),
        })
    }

    pub fn pacman(
        selector: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        architecture: impl Into<String>,
    ) -> Result<Self> {
        Self::validated(Self::Pacman {
            selector: selector.into(),
            name: name.into(),
            version: version.into(),
            architecture: architecture.into(),
        })
    }

    pub fn validate(&self) -> Result<()> {
        let required: Vec<(&str, &str)> = match self {
            Self::Rpm {
                selector,
                name,
                version,
                release,
                architecture,
                ..
            } => vec![
                ("selector", selector.as_str()),
                ("name", name.as_str()),
                ("version", version.as_str()),
                ("release", release.as_str()),
                ("architecture", architecture.as_str()),
            ],
            Self::Dpkg {
                selector,
                name,
                version,
                architecture,
            }
            | Self::Pacman {
                selector,
                name,
                version,
                architecture,
            } => vec![
                ("selector", selector.as_str()),
                ("name", name.as_str()),
                ("version", version.as_str()),
                ("architecture", architecture.as_str()),
            ],
        };
        if let Some((field, _)) = required.iter().find(|(_, value)| value.trim().is_empty()) {
            return Err(Error::ParseError(format!(
                "installed native package identity has an empty {field}"
            )));
        }
        if self
            .selector()
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(Error::ParseError(
                "installed native package selector contains whitespace or control characters"
                    .to_string(),
            ));
        }
        let selector_matches_fields = match self {
            Self::Rpm {
                selector,
                name,
                epoch,
                version,
                release,
                architecture,
            } => {
                let epoch = epoch.map(|value| format!("{value}:")).unwrap_or_default();
                *selector == format!("{name}-{epoch}{version}-{release}.{architecture}")
            }
            Self::Dpkg {
                selector,
                name,
                architecture,
                ..
            } => selector == name || *selector == format!("{name}:{architecture}"),
            Self::Pacman { selector, name, .. } => selector == name,
        };
        if !selector_matches_fields {
            return Err(Error::ParseError(format!(
                "installed native package selector {:?} disagrees with its typed identity fields",
                self.selector()
            )));
        }
        Ok(())
    }

    pub fn selector(&self) -> &str {
        match self {
            Self::Rpm { selector, .. }
            | Self::Dpkg { selector, .. }
            | Self::Pacman { selector, .. } => selector,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Rpm { name, .. } | Self::Dpkg { name, .. } | Self::Pacman { name, .. } => name,
        }
    }

    pub fn version(&self) -> String {
        match self {
            Self::Rpm {
                epoch,
                version,
                release,
                ..
            } => {
                let mut value = String::new();
                if let Some(epoch) = epoch
                    && *epoch > 0
                {
                    value.push_str(&format!("{epoch}:"));
                }
                value.push_str(version);
                value.push('-');
                value.push_str(release);
                value
            }
            Self::Dpkg { version, .. } | Self::Pacman { version, .. } => version.clone(),
        }
    }

    pub fn architecture(&self) -> &str {
        match self {
            Self::Rpm { architecture, .. }
            | Self::Dpkg { architecture, .. }
            | Self::Pacman { architecture, .. } => architecture,
        }
    }

    /// Selector used by the native manager's install-reason authority.
    ///
    /// DNF records reason at the name/architecture package level, while dpkg
    /// and pacman expose the same exact selector used for their local record.
    pub fn install_reason_selector(&self) -> String {
        match self {
            Self::Rpm {
                name, architecture, ..
            } => format!("{name}.{architecture}"),
            Self::Dpkg { selector, .. } | Self::Pacman { selector, .. } => selector.clone(),
        }
    }

    pub fn same_name_and_architecture(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
            && self.name() == other.name()
            && self.architecture() == other.architecture()
    }

    fn validated(identity: Self) -> Result<Self> {
        identity.validate()?;
        Ok(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_identity_preserves_exact_selector_and_epoch_version() {
        let identity = InstalledPackageIdentity::rpm(
            "perl-4:5.40.0-1.fc44.x86_64",
            "perl",
            Some(4),
            "5.40.0",
            "1.fc44",
            "x86_64",
        )
        .unwrap();

        assert_eq!(identity.selector(), "perl-4:5.40.0-1.fc44.x86_64");
        assert_eq!(identity.version(), "4:5.40.0-1.fc44");

        let explicit_zero = InstalledPackageIdentity::rpm(
            "fixture-0:1-1.x86_64",
            "fixture",
            Some(0),
            "1",
            "1",
            "x86_64",
        )
        .unwrap();
        assert_eq!(explicit_zero.version(), "1-1");
    }

    #[test]
    fn dpkg_identity_preserves_multiarch_selector() {
        let identity =
            InstalledPackageIdentity::dpkg("libc6:i386", "libc6", "2.41-12", "i386").unwrap();

        assert_eq!(identity.selector(), "libc6:i386");
        assert_eq!(identity.name(), "libc6");
    }

    #[test]
    fn identity_rejects_empty_or_whitespace_selector() {
        assert!(InstalledPackageIdentity::pacman("", "bash", "5.3-1", "x86_64").is_err());
        assert!(
            InstalledPackageIdentity::pacman("bad selector", "bash", "5.3-1", "x86_64").is_err()
        );
    }

    #[test]
    fn identity_rejects_selectors_that_disagree_with_typed_fields() {
        assert!(
            InstalledPackageIdentity::rpm("other-1-1.x86_64", "fixture", None, "1", "1", "x86_64")
                .is_err()
        );
        assert!(InstalledPackageIdentity::dpkg("libc6:amd64", "libc6", "2.41-12", "i386").is_err());
        assert!(InstalledPackageIdentity::pacman("bash-debug", "bash", "5.3-1", "x86_64").is_err());
    }

    #[test]
    fn persisted_identity_rejects_unknown_fields() {
        let error = serde_json::from_str::<InstalledPackageIdentity>(
            r#"{"manager":"pacman","selector":"bash","name":"bash","version":"5.3-1","architecture":"x86_64","legacy":true}"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field"));
    }
}
