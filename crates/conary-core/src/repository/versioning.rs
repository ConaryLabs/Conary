// conary-core/src/repository/versioning.rs

//! Parsed, ecosystem-native repository version comparison.
//!
//! Version strings are validated by the grammar that owns them before they
//! reach an upstream comparator. Invalid values and cross-scheme comparisons
//! are data errors; they never acquire an ordering through fallback logic.

use crate::db::models::RepositoryPackage;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::str::FromStr;
use thiserror::Error;

/// Which package ecosystem owns a version string's grammar and ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionScheme {
    /// Conary-authored CCS versions use Semantic Versioning 2.0.0.
    Conary,
    /// RPM epoch-version-release ordering.
    Rpm,
    /// Debian dpkg epoch-upstream-revision ordering.
    Debian,
    /// Arch Linux ALPM epoch-pkgver-pkgrel ordering.
    Arch,
}

impl VersionScheme {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conary => "conary",
            Self::Rpm => "rpm",
            Self::Debian => "debian",
            Self::Arch => "arch",
        }
    }
}

impl FromStr for VersionScheme {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "conary" => Ok(Self::Conary),
            "rpm" => Ok(Self::Rpm),
            "debian" => Ok(Self::Debian),
            "arch" => Ok(Self::Arch),
            other => Err(format!(
                "unsupported persisted native version scheme '{other}'"
            )),
        }
    }
}

/// A failure to parse or compare a package version under its declared scheme.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VersionComparisonError {
    #[error("invalid {scheme} version '{version}': {reason}")]
    InvalidVersion {
        scheme: &'static str,
        version: String,
        reason: String,
    },
    #[error("cannot compare {left} and {right} version schemes")]
    SchemeMismatch {
        left: &'static str,
        right: &'static str,
    },
    #[error("invalid {scheme} version constraint '{constraint}': {reason}")]
    InvalidConstraint {
        scheme: &'static str,
        constraint: String,
        reason: String,
    },
}

pub type VersionResult<T> = std::result::Result<T, VersionComparisonError>;

/// A validated version string paired with its owning comparison scheme.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepositoryVersion {
    pub raw: String,
    pub scheme: VersionScheme,
}

impl RepositoryVersion {
    pub fn new(raw: String, scheme: VersionScheme) -> VersionResult<Self> {
        validate_repo_version(scheme, &raw)?;
        Ok(Self { raw, scheme })
    }

    pub fn compare(&self, other: &Self) -> VersionResult<Ordering> {
        compare_mixed_repo_versions(self.scheme, &self.raw, other.scheme, &other.raw)
    }

    pub fn satisfies(&self, constraint: &RepoVersionConstraint) -> VersionResult<bool> {
        repo_version_satisfies(self.scheme, &self.raw, constraint)
    }
}

/// A version constraint in its enclosing repository scheme.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RepoVersionConstraint {
    Any,
    Exact(String),
    GreaterThan(String),
    GreaterOrEqual(String),
    LessThan(String),
    LessOrEqual(String),
    NotEqual(String),
}

/// Validate one version against the exact grammar selected by `scheme`.
pub fn validate_repo_version(scheme: VersionScheme, raw: &str) -> VersionResult<()> {
    let invalid = |reason: String| VersionComparisonError::InvalidVersion {
        scheme: scheme.as_str(),
        version: raw.to_string(),
        reason,
    };

    match scheme {
        VersionScheme::Conary => semver::Version::parse(raw)
            .map(|_| ())
            .map_err(|error| invalid(error.to_string())),
        VersionScheme::Rpm => validate_rpm_evr(raw).map_err(invalid),
        VersionScheme::Debian => debversion::Version::from_str(raw)
            .map(|_| ())
            .map_err(|error| invalid(error.to_string())),
        VersionScheme::Arch => alpm_types::Version::from_str(raw)
            .map(|_| ())
            .map_err(|error| invalid(error.to_string())),
    }
}

/// Compare two versions using only the parser and comparator owned by `scheme`.
pub fn compare_repo_versions(scheme: VersionScheme, a: &str, b: &str) -> VersionResult<Ordering> {
    match scheme {
        VersionScheme::Conary => {
            let a = parse_semver(a)?;
            let b = parse_semver(b)?;
            Ok(a.cmp_precedence(&b))
        }
        VersionScheme::Rpm => {
            validate_repo_version(scheme, a)?;
            validate_repo_version(scheme, b)?;
            Ok(rpm_version::rpm_evr_compare(a, b))
        }
        VersionScheme::Debian => {
            let a = parse_debian(a)?;
            let b = parse_debian(b)?;
            Ok(a.cmp(&b))
        }
        VersionScheme::Arch => {
            let a = parse_arch(a)?;
            let b = parse_arch(b)?;
            Ok(a.cmp(&b))
        }
    }
}

pub fn compare_mixed_repo_versions(
    a_scheme: VersionScheme,
    a: &str,
    b_scheme: VersionScheme,
    b: &str,
) -> VersionResult<Ordering> {
    if a_scheme != b_scheme {
        return Err(VersionComparisonError::SchemeMismatch {
            left: a_scheme.as_str(),
            right: b_scheme.as_str(),
        });
    }
    compare_repo_versions(a_scheme, a, b)
}

/// Parse and validate a version constraint under its declared scheme.
pub fn parse_repo_constraint(
    scheme: VersionScheme,
    raw: &str,
) -> VersionResult<RepoVersionConstraint> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(RepoVersionConstraint::Any);
    }

    let operators = [
        (">=", ConstraintOperator::GreaterOrEqual),
        ("<=", ConstraintOperator::LessOrEqual),
        ("<<", ConstraintOperator::LessThan),
        (">>", ConstraintOperator::GreaterThan),
        ("!=", ConstraintOperator::NotEqual),
        (">", ConstraintOperator::GreaterThan),
        ("<", ConstraintOperator::LessThan),
        ("=", ConstraintOperator::Exact),
    ];
    let (operator, version) = operators
        .iter()
        .find_map(|(token, operator)| {
            trimmed
                .strip_prefix(token)
                .map(|version| (*operator, version.trim()))
        })
        .unwrap_or((ConstraintOperator::Exact, trimmed));

    if version.is_empty() {
        return Err(VersionComparisonError::InvalidConstraint {
            scheme: scheme.as_str(),
            constraint: raw.to_string(),
            reason: "missing version operand".to_string(),
        });
    }
    validate_repo_version(scheme, version).map_err(|error| {
        VersionComparisonError::InvalidConstraint {
            scheme: scheme.as_str(),
            constraint: raw.to_string(),
            reason: error.to_string(),
        }
    })?;

    Ok(operator.build(version.to_string()))
}

pub fn repo_version_satisfies(
    scheme: VersionScheme,
    version: &str,
    constraint: &RepoVersionConstraint,
) -> VersionResult<bool> {
    let expected = match constraint {
        RepoVersionConstraint::Any => {
            validate_repo_version(scheme, version)?;
            return Ok(true);
        }
        RepoVersionConstraint::Exact(expected)
        | RepoVersionConstraint::GreaterThan(expected)
        | RepoVersionConstraint::GreaterOrEqual(expected)
        | RepoVersionConstraint::LessThan(expected)
        | RepoVersionConstraint::LessOrEqual(expected)
        | RepoVersionConstraint::NotEqual(expected) => expected,
    };
    let ordering = compare_repo_versions(scheme, version, expected)?;
    Ok(match constraint {
        RepoVersionConstraint::Any => unreachable!("handled above"),
        RepoVersionConstraint::Exact(_) => ordering == Ordering::Equal,
        RepoVersionConstraint::GreaterThan(_) => ordering == Ordering::Greater,
        RepoVersionConstraint::GreaterOrEqual(_) => ordering != Ordering::Less,
        RepoVersionConstraint::LessThan(_) => ordering == Ordering::Less,
        RepoVersionConstraint::LessOrEqual(_) => ordering != Ordering::Greater,
        RepoVersionConstraint::NotEqual(_) => ordering != Ordering::Equal,
    })
}

/// Resolve the persisted scheme authority for a repository package.
pub fn resolve_package_version_scheme(pkg: &RepositoryPackage) -> VersionScheme {
    pkg.version_scheme
}

pub fn compare_repo_package_versions(
    a: &RepositoryPackage,
    b: &RepositoryPackage,
) -> VersionResult<Ordering> {
    let a_scheme = resolve_package_version_scheme(a);
    let b_scheme = resolve_package_version_scheme(b);
    compare_mixed_repo_versions(a_scheme, &a.version, b_scheme, &b.version)
}

#[derive(Clone, Copy)]
enum ConstraintOperator {
    Exact,
    GreaterThan,
    GreaterOrEqual,
    LessThan,
    LessOrEqual,
    NotEqual,
}

impl ConstraintOperator {
    fn build(self, version: String) -> RepoVersionConstraint {
        match self {
            Self::Exact => RepoVersionConstraint::Exact(version),
            Self::GreaterThan => RepoVersionConstraint::GreaterThan(version),
            Self::GreaterOrEqual => RepoVersionConstraint::GreaterOrEqual(version),
            Self::LessThan => RepoVersionConstraint::LessThan(version),
            Self::LessOrEqual => RepoVersionConstraint::LessOrEqual(version),
            Self::NotEqual => RepoVersionConstraint::NotEqual(version),
        }
    }
}

fn parse_semver(raw: &str) -> VersionResult<semver::Version> {
    semver::Version::parse(raw).map_err(|error| VersionComparisonError::InvalidVersion {
        scheme: VersionScheme::Conary.as_str(),
        version: raw.to_string(),
        reason: error.to_string(),
    })
}

fn parse_debian(raw: &str) -> VersionResult<debversion::Version> {
    debversion::Version::from_str(raw).map_err(|error| VersionComparisonError::InvalidVersion {
        scheme: VersionScheme::Debian.as_str(),
        version: raw.to_string(),
        reason: error.to_string(),
    })
}

fn parse_arch(raw: &str) -> VersionResult<alpm_types::Version> {
    alpm_types::Version::from_str(raw).map_err(|error| VersionComparisonError::InvalidVersion {
        scheme: VersionScheme::Arch.as_str(),
        version: raw.to_string(),
        reason: error.to_string(),
    })
}

/// Validate RPM's documented `[epoch:]version[-release]` grammar before
/// calling `rpm-version`, whose comparator intentionally accepts arbitrary
/// strings.
fn validate_rpm_evr(raw: &str) -> std::result::Result<(), String> {
    if raw.is_empty() {
        return Err("version is empty".to_string());
    }
    if raw.trim() != raw {
        return Err("leading or trailing whitespace is not allowed".to_string());
    }

    let (epoch, version_release) = match raw.split_once(':') {
        Some((epoch, rest)) => {
            if epoch.is_empty() || !epoch.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("epoch must be a non-empty decimal integer".to_string());
            }
            epoch
                .parse::<u64>()
                .map_err(|_| "epoch exceeds the supported integer range".to_string())?;
            (Some(epoch), rest)
        }
        None => (None, raw),
    };
    let _ = epoch;

    if version_release.contains(':') {
        return Err("multiple epoch separators are not allowed".to_string());
    }
    let (version, release) = match version_release.split_once('-') {
        Some((version, release)) => {
            if release.contains('-') {
                return Err("release must not contain '-'".to_string());
            }
            (version, Some(release))
        }
        None => (version_release, None),
    };
    validate_rpm_component(version, "version")?;
    if let Some(release) = release {
        validate_rpm_component(release, "release")?;
    }
    Ok(())
}

fn validate_rpm_component(component: &str, name: &str) -> std::result::Result<(), String> {
    if component.is_empty() {
        return Err(format!("{name} component is empty"));
    }
    if !component.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'~' | b'^')
    }) {
        return Err(format!(
            "{name} contains characters outside RPM's version grammar"
        ));
    }
    if !component.bytes().any(|byte| byte.is_ascii_alphanumeric()) {
        return Err(format!("{name} must contain an alphanumeric character"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compare(scheme: VersionScheme, a: &str, b: &str) -> Ordering {
        compare_repo_versions(scheme, a, b).expect("valid versions")
    }

    #[test]
    fn upstream_comparators_preserve_native_ordering() {
        assert_eq!(
            compare(VersionScheme::Rpm, "1.2.3-2.fc44", "1.2.3-1.fc44"),
            Ordering::Greater
        );
        assert_eq!(
            compare(VersionScheme::Debian, "1.0", "1.0~beta1"),
            Ordering::Greater
        );
        assert_eq!(
            compare(VersionScheme::Arch, "1:1.0-2", "1.0-3"),
            Ordering::Greater
        );
        assert_eq!(
            compare(VersionScheme::Conary, "1.0.0-alpha.1", "1.0.0"),
            Ordering::Less
        );
        assert_eq!(
            compare(VersionScheme::Conary, "1.0.0+build.1", "1.0.0+build.2"),
            Ordering::Equal
        );
    }

    #[test]
    fn rpm_upstream_tilde_caret_and_epoch_corpus() {
        for (a, b, expected) in [
            ("1.0~rc1", "1.0", Ordering::Less),
            ("1.0^git1", "1.0", Ordering::Greater),
            ("1.0^git1", "1.1", Ordering::Less),
            ("2:1.0", "1:2.0", Ordering::Greater),
            ("1.0~alpha", "1.0~beta", Ordering::Less),
            ("1.0^git1", "1.0^git2", Ordering::Less),
            ("1.0~rc1", "1.0^git1", Ordering::Less),
        ] {
            assert_eq!(compare(VersionScheme::Rpm, a, b), expected);
        }
    }

    #[test]
    fn arch_upstream_special_version_corpus() {
        assert_eq!(
            compare(VersionScheme::Arch, "1.0~rc1", "1.0"),
            Ordering::Greater
        );
        assert_eq!(
            compare(VersionScheme::Arch, "1.0^git1", "1.0"),
            Ordering::Greater
        );
    }

    #[test]
    fn rejects_cross_scheme_comparison_with_typed_error() {
        assert!(matches!(
            compare_mixed_repo_versions(VersionScheme::Debian, "1.0", VersionScheme::Arch, "1.0-1"),
            Err(VersionComparisonError::SchemeMismatch { .. })
        ));
    }

    #[test]
    fn rejects_invalid_versions_instead_of_assigning_fallback_order() {
        for (scheme, raw) in [
            (VersionScheme::Conary, "1.0"),
            (VersionScheme::Rpm, "broken:1.0"),
            (VersionScheme::Rpm, "1.0-"),
            (VersionScheme::Debian, "1.0 bad"),
            (VersionScheme::Arch, "1:"),
        ] {
            assert!(
                validate_repo_version(scheme, raw).is_err(),
                "{scheme:?} accepted {raw:?}"
            );
        }
    }

    #[test]
    fn constraints_are_validated_and_use_native_ordering() {
        let debian =
            parse_repo_constraint(VersionScheme::Debian, ">= 1.0~beta1").expect("constraint");
        assert!(repo_version_satisfies(VersionScheme::Debian, "1.0", &debian).unwrap());
        assert!(!repo_version_satisfies(VersionScheme::Debian, "0.9", &debian).unwrap());

        let arch = parse_repo_constraint(VersionScheme::Arch, ">= 1:1.0-1").expect("constraint");
        assert!(repo_version_satisfies(VersionScheme::Arch, "1:1.0-2", &arch).unwrap());
        assert!(!repo_version_satisfies(VersionScheme::Arch, "1.0-9", &arch).unwrap());

        assert!(parse_repo_constraint(VersionScheme::Rpm, ">= broken:1.0").is_err());
    }

    #[test]
    fn validated_repository_version_compares_and_satisfies() {
        let a = RepositoryVersion::new("1.2.3-2.fc44".to_string(), VersionScheme::Rpm).unwrap();
        let b = RepositoryVersion::new("1.2.3-1.fc44".to_string(), VersionScheme::Rpm).unwrap();
        assert_eq!(a.compare(&b).unwrap(), Ordering::Greater);

        let version = RepositoryVersion::new("1.0".to_string(), VersionScheme::Debian).unwrap();
        let constraint =
            parse_repo_constraint(VersionScheme::Debian, ">= 0.9").expect("constraint");
        assert!(version.satisfies(&constraint).unwrap());
    }
}
