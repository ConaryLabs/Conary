// conary-core/src/repository/versioning.rs

//! Parsed, ecosystem-native repository version comparison.
//!
//! Version strings are validated by the grammar that owns them before they
//! reach an upstream comparator. Invalid values and cross-scheme comparisons
//! are data errors; they never acquire an ordering through fallback logic.

use crate::db::models::RepositoryPackage;
use crate::repository::dependency_model::ProvideVersionRelation;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::str::FromStr;
use thiserror::Error;

mod rpm_dependency;

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
    /// Solus eopkg/PISI version ordering.
    Eopkg,
}

impl VersionScheme {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conary => "conary",
            Self::Rpm => "rpm",
            Self::Debian => "debian",
            Self::Arch => "arch",
            Self::Eopkg => "eopkg",
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
            "eopkg" => Ok(Self::Eopkg),
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
    #[error("invalid CCS package release '{release}': {reason}")]
    InvalidPackageRelease { release: String, reason: String },
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
    Eopkg(EopkgVersionConstraint),
}

/// Independent eopkg/PISI version and integer-release bounds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EopkgVersionConstraint {
    pub version: Option<String>,
    pub version_from: Option<String>,
    pub version_to: Option<String>,
    pub release: Option<u64>,
    pub release_from: Option<u64>,
    pub release_to: Option<u64>,
}

impl EopkgVersionConstraint {
    pub fn validate(&self) -> VersionResult<()> {
        for value in [
            self.version.as_deref(),
            self.version_from.as_deref(),
            self.version_to.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            super::eopkg_version::validate_upstream(value)?;
        }
        if self.version.is_some() && (self.version_from.is_some() || self.version_to.is_some()) {
            return Err(VersionComparisonError::InvalidConstraint {
                scheme: "eopkg",
                constraint: self.to_string(),
                reason: "exact version cannot be combined with version range bounds".to_string(),
            });
        }
        if self.release.is_some() && (self.release_from.is_some() || self.release_to.is_some()) {
            return Err(VersionComparisonError::InvalidConstraint {
                scheme: "eopkg",
                constraint: self.to_string(),
                reason: "exact release cannot be combined with release range bounds".to_string(),
            });
        }
        Ok(())
    }
}

impl std::fmt::Display for EopkgVersionConstraint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        serde_json::to_string(self)
            .map_err(|_| std::fmt::Error)
            .and_then(|value| formatter.write_str(&format!("eopkg:{value}")))
    }
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
        VersionScheme::Eopkg => super::eopkg_version::validate(raw),
    }
}

/// Validate an RPM dependency EVR boundary.
///
/// RPM dependency records deliberately admit a broader version and release
/// alphabet than package identity fields. Keep that source grammar separate
/// so native capability authority is not narrowed to the package-header
/// `VERSION`/`RELEASE` grammar.
pub fn validate_rpm_dependency_boundary(raw: &str) -> VersionResult<()> {
    rpm_dependency::validate(raw).map_err(|reason| VersionComparisonError::InvalidVersion {
        scheme: VersionScheme::Rpm.as_str(),
        version: raw.to_string(),
        reason,
    })
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
        VersionScheme::Eopkg => super::eopkg_version::compare(a, b),
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
    if scheme == VersionScheme::Eopkg {
        let json = trimmed.strip_prefix("eopkg:").ok_or_else(|| {
            VersionComparisonError::InvalidConstraint {
                scheme: "eopkg",
                constraint: raw.to_string(),
                reason: "expected typed 'eopkg:<json>' constraint".to_string(),
            }
        })?;
        let constraint: EopkgVersionConstraint = serde_json::from_str(json).map_err(|error| {
            VersionComparisonError::InvalidConstraint {
                scheme: "eopkg",
                constraint: raw.to_string(),
                reason: error.to_string(),
            }
        })?;
        constraint.validate()?;
        return Ok(RepoVersionConstraint::Eopkg(constraint));
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
    let validation = if scheme == VersionScheme::Rpm {
        validate_rpm_dependency_boundary(version)
    } else {
        validate_repo_version(scheme, version)
    };
    validation.map_err(|error| VersionComparisonError::InvalidConstraint {
        scheme: scheme.as_str(),
        constraint: raw.to_string(),
        reason: error.to_string(),
    })?;

    Ok(operator.build(version.to_string()))
}

pub fn repo_version_satisfies(
    scheme: VersionScheme,
    version: &str,
    constraint: &RepoVersionConstraint,
) -> VersionResult<bool> {
    if let RepoVersionConstraint::Eopkg(constraint) = constraint {
        if scheme != VersionScheme::Eopkg {
            return Err(VersionComparisonError::InvalidConstraint {
                scheme: scheme.as_str(),
                constraint: constraint.to_string(),
                reason: "eopkg constraint used with another version scheme".to_string(),
            });
        }
        let candidate = super::eopkg_version::parse(version)?;
        let compare =
            |expected: &str| super::eopkg_version::compare_upstream(&candidate.version, expected);
        if constraint
            .version
            .as_deref()
            .is_some_and(|expected| compare(expected) != Ok(Ordering::Equal))
            || constraint
                .version_from
                .as_deref()
                .is_some_and(|expected| compare(expected) == Ok(Ordering::Less))
            || constraint
                .version_to
                .as_deref()
                .is_some_and(|expected| compare(expected) == Ok(Ordering::Greater))
            || constraint
                .release
                .is_some_and(|value| candidate.release != value)
            || constraint
                .release_from
                .is_some_and(|value| candidate.release < value)
            || constraint
                .release_to
                .is_some_and(|value| candidate.release > value)
        {
            return Ok(false);
        }
        return Ok(true);
    }
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
        RepoVersionConstraint::Eopkg(_) => unreachable!("handled above"),
    };
    let ordering = if scheme == VersionScheme::Rpm {
        validate_repo_version(scheme, version)?;
        validate_rpm_dependency_boundary(expected)?;
        rpm_dependency_boundary_cmp(version, expected)?
    } else {
        compare_repo_versions(scheme, version, expected)?
    };
    Ok(match constraint {
        RepoVersionConstraint::Any => unreachable!("handled above"),
        RepoVersionConstraint::Exact(_) => ordering == Ordering::Equal,
        RepoVersionConstraint::GreaterThan(_) => ordering == Ordering::Greater,
        RepoVersionConstraint::GreaterOrEqual(_) => ordering != Ordering::Less,
        RepoVersionConstraint::LessThan(_) => ordering == Ordering::Less,
        RepoVersionConstraint::LessOrEqual(_) => ordering != Ordering::Greater,
        RepoVersionConstraint::NotEqual(_) => ordering != Ordering::Equal,
        RepoVersionConstraint::Eopkg(_) => unreachable!("handled above"),
    })
}

/// Match one source-native provided range against a requirement range.
///
/// RPM uses `rpmdsCompare`: a versioned provide is a range, not a point. Its
/// comparison therefore asks whether the provide and requirement ranges
/// overlap. Other supported formats currently permit only exact versioned
/// provides.
pub fn provided_range_matches_requirement(
    scheme: VersionScheme,
    provide_relation: Option<ProvideVersionRelation>,
    provide_version: Option<&str>,
    requirement: &RepoVersionConstraint,
) -> VersionResult<bool> {
    if provide_relation.is_some() != provide_version.is_some() {
        return Err(VersionComparisonError::InvalidConstraint {
            scheme: scheme.as_str(),
            constraint: format!(
                "provider relation {:?} with boundary {:?}",
                provide_relation, provide_version
            ),
            reason: "a versioned provide must carry both a relation and boundary".to_string(),
        });
    }

    let Some(provide_relation) = provide_relation else {
        return Ok(match scheme {
            // rpm's existence-test rule overlaps a same-name versioned range.
            VersionScheme::Rpm => true,
            VersionScheme::Conary
            | VersionScheme::Debian
            | VersionScheme::Arch
            | VersionScheme::Eopkg => {
                matches!(requirement, RepoVersionConstraint::Any)
            }
        });
    };
    let provide_version = provide_version.expect("relation and version pairing checked above");
    if scheme == VersionScheme::Rpm {
        validate_rpm_dependency_boundary(provide_version)?;
    } else {
        validate_repo_version(scheme, provide_version)?;
    }

    if matches!(requirement, RepoVersionConstraint::Any) {
        return Ok(true);
    }
    if scheme == VersionScheme::Rpm {
        return rpm_provided_range_overlaps_requirement(
            provide_relation,
            provide_version,
            requirement,
        );
    }
    if provide_relation != ProvideVersionRelation::Equal {
        return Err(VersionComparisonError::InvalidConstraint {
            scheme: scheme.as_str(),
            constraint: format!("{} {provide_version}", provide_relation.as_str()),
            reason: "this source format only permits exact versioned provides".to_string(),
        });
    }
    repo_version_satisfies(scheme, provide_version, requirement)
}

fn rpm_provided_range_overlaps_requirement(
    provide_relation: ProvideVersionRelation,
    provide_version: &str,
    requirement: &RepoVersionConstraint,
) -> VersionResult<bool> {
    let requirement_relation = match requirement {
        RepoVersionConstraint::Exact(_) => ProvideVersionRelation::Equal,
        RepoVersionConstraint::GreaterThan(_) => ProvideVersionRelation::GreaterThan,
        RepoVersionConstraint::GreaterOrEqual(_) => ProvideVersionRelation::GreaterOrEqual,
        RepoVersionConstraint::LessThan(_) => ProvideVersionRelation::LessThan,
        RepoVersionConstraint::LessOrEqual(_) => ProvideVersionRelation::LessOrEqual,
        RepoVersionConstraint::Any => return Ok(true),
        RepoVersionConstraint::NotEqual(version) => {
            return Err(VersionComparisonError::InvalidConstraint {
                scheme: VersionScheme::Rpm.as_str(),
                constraint: format!("!= {version}"),
                reason: "RPM dependency ranges do not define a not-equal relation".to_string(),
            });
        }
        RepoVersionConstraint::Eopkg(_) => {
            return Err(VersionComparisonError::InvalidConstraint {
                scheme: VersionScheme::Rpm.as_str(),
                constraint: "eopkg".to_string(),
                reason: "eopkg constraint cannot participate in RPM range overlap".to_string(),
            });
        }
    };
    let requirement_version = constraint_boundary(requirement)
        .expect("all non-Any requirement relations above have a boundary");
    validate_rpm_dependency_boundary(requirement_version)?;

    let ordering = rpm_dependency_boundary_cmp(provide_version, requirement_version)?;

    let provide =
        rpm_dependency::parse(provide_version).expect("validated RPM provided capability boundary");
    let required =
        rpm_dependency::parse(requirement_version).expect("validated RPM requirement boundary");

    // rpmverOverlap treats a missing release as an intentionally partial EVR:
    // equality on the side without a release overlaps any release at the same
    // epoch and version.
    if ordering == Ordering::Equal
        && ((provide.release.is_some()
            && required.release.is_none()
            && relation_includes_equal(requirement_relation))
            || (required.release.is_some()
                && provide.release.is_none()
                && relation_includes_equal(provide_relation)))
    {
        return Ok(true);
    }

    Ok(match ordering {
        Ordering::Less => {
            relation_includes_greater(provide_relation)
                || relation_includes_less(requirement_relation)
        }
        Ordering::Greater => {
            relation_includes_less(provide_relation)
                || relation_includes_greater(requirement_relation)
        }
        Ordering::Equal => {
            (relation_includes_equal(provide_relation)
                && relation_includes_equal(requirement_relation))
                || (relation_includes_less(provide_relation)
                    && relation_includes_less(requirement_relation))
                || (relation_includes_greater(provide_relation)
                    && relation_includes_greater(requirement_relation))
        }
    })
}

fn constraint_boundary(constraint: &RepoVersionConstraint) -> Option<&str> {
    match constraint {
        RepoVersionConstraint::Any => None,
        RepoVersionConstraint::Exact(version)
        | RepoVersionConstraint::GreaterThan(version)
        | RepoVersionConstraint::GreaterOrEqual(version)
        | RepoVersionConstraint::LessThan(version)
        | RepoVersionConstraint::LessOrEqual(version)
        | RepoVersionConstraint::NotEqual(version) => Some(version),
        RepoVersionConstraint::Eopkg(_) => None,
    }
}

fn rpm_dependency_boundary_cmp(left: &str, right: &str) -> VersionResult<Ordering> {
    rpm_dependency::compare(left, right).map_err(|reason| {
        VersionComparisonError::InvalidConstraint {
            scheme: VersionScheme::Rpm.as_str(),
            constraint: format!("{left:?} compared with {right:?}"),
            reason,
        }
    })
}

const fn relation_includes_less(relation: ProvideVersionRelation) -> bool {
    matches!(
        relation,
        ProvideVersionRelation::LessThan | ProvideVersionRelation::LessOrEqual
    )
}

const fn relation_includes_equal(relation: ProvideVersionRelation) -> bool {
    matches!(
        relation,
        ProvideVersionRelation::Equal
            | ProvideVersionRelation::LessOrEqual
            | ProvideVersionRelation::GreaterOrEqual
    )
}

const fn relation_includes_greater(relation: ProvideVersionRelation) -> bool {
    matches!(
        relation,
        ProvideVersionRelation::GreaterThan | ProvideVersionRelation::GreaterOrEqual
    )
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
    compare_package_identities(
        a_scheme,
        &a.version,
        non_empty_release(&a.package_release),
        b_scheme,
        &b.version,
        non_empty_release(&b.package_release),
    )
}

/// Validate the monotonically increasing release number of one signed CCS
/// build. Release `0` is reserved for sources that have no CCS envelope.
pub fn validate_package_release(release: &str) -> VersionResult<u64> {
    if release.is_empty() {
        return Err(VersionComparisonError::InvalidPackageRelease {
            release: release.to_string(),
            reason: "release is empty".to_string(),
        });
    }
    if !release.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(VersionComparisonError::InvalidPackageRelease {
            release: release.to_string(),
            reason: "release must be an unsigned decimal integer".to_string(),
        });
    }
    let value =
        release
            .parse::<u64>()
            .map_err(|_| VersionComparisonError::InvalidPackageRelease {
                release: release.to_string(),
                reason: "release exceeds the supported integer range".to_string(),
            })?;
    if value == 0 {
        return Err(VersionComparisonError::InvalidPackageRelease {
            release: release.to_string(),
            reason: "release must be greater than zero".to_string(),
        });
    }
    Ok(value)
}

/// Compare complete package identities. The source-native version algebra
/// orders versions first; the signed CCS build release breaks equal-version
/// ties. A source artifact without a CCS envelope has implicit release zero.
pub fn compare_package_identities(
    a_scheme: VersionScheme,
    a_version: &str,
    a_release: Option<&str>,
    b_scheme: VersionScheme,
    b_version: &str,
    b_release: Option<&str>,
) -> VersionResult<Ordering> {
    let version = compare_mixed_repo_versions(a_scheme, a_version, b_scheme, b_version)?;
    if version != Ordering::Equal {
        return Ok(version);
    }
    let a_release = a_release
        .map(validate_package_release)
        .transpose()?
        .unwrap_or(0);
    let b_release = b_release
        .map(validate_package_release)
        .transpose()?
        .unwrap_or(0);
    Ok(a_release.cmp(&b_release))
}

fn non_empty_release(release: &str) -> Option<&str> {
    (!release.is_empty()).then_some(release)
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
    fn rpm_dependency_boundaries_keep_their_broader_source_grammar() {
        let cpe = "cpe%3A%2Fo%3Aopensuse%3Aopensuse%3A20260811";

        assert!(validate_repo_version(VersionScheme::Rpm, cpe).is_err());
        validate_rpm_dependency_boundary(cpe).unwrap();
        let requirement = parse_repo_constraint(VersionScheme::Rpm, &format!("= {cpe}"))
            .expect("RPM dependency constraint");
        assert!(
            provided_range_matches_requirement(
                VersionScheme::Rpm,
                Some(ProvideVersionRelation::Equal),
                Some(cpe),
                &requirement,
            )
            .unwrap()
        );
        validate_rpm_dependency_boundary("pattern-basis-addon").unwrap();

        for invalid in ["", " 1", "1 ", "1 2", "1-", "bad:1", "1:2:3"] {
            assert!(
                validate_rpm_dependency_boundary(invalid).is_err(),
                "RPM dependency boundary admitted {invalid:?}"
            );
        }
    }

    #[test]
    fn rpm_provided_ranges_use_upstream_overlap_semantics() {
        use ProvideVersionRelation::{Equal, GreaterOrEqual, GreaterThan};

        for (provide_relation, provide_version, requirement, expected) in [
            (
                GreaterOrEqual,
                "2",
                RepoVersionConstraint::LessThan("3".to_string()),
                true,
            ),
            (
                GreaterOrEqual,
                "3",
                RepoVersionConstraint::LessThan("2".to_string()),
                false,
            ),
            (
                GreaterThan,
                "2",
                RepoVersionConstraint::LessThan("2".to_string()),
                false,
            ),
            (
                GreaterOrEqual,
                "2",
                RepoVersionConstraint::LessOrEqual("2".to_string()),
                true,
            ),
            (
                GreaterThan,
                "2",
                RepoVersionConstraint::LessOrEqual("2".to_string()),
                false,
            ),
            (
                Equal,
                "1.0",
                RepoVersionConstraint::Exact("1.0-9".to_string()),
                true,
            ),
        ] {
            assert_eq!(
                provided_range_matches_requirement(
                    VersionScheme::Rpm,
                    Some(provide_relation),
                    Some(provide_version),
                    &requirement,
                )
                .unwrap(),
                expected,
                "{provide_relation:?} {provide_version} versus {requirement:?}"
            );
        }
        assert!(
            provided_range_matches_requirement(
                VersionScheme::Rpm,
                None,
                None,
                &RepoVersionConstraint::GreaterOrEqual("999".to_string()),
            )
            .unwrap()
        );
    }

    #[test]
    fn non_rpm_provides_require_exact_version_authority() {
        assert!(
            provided_range_matches_requirement(
                VersionScheme::Debian,
                Some(ProvideVersionRelation::Equal),
                Some("2.0"),
                &RepoVersionConstraint::GreaterOrEqual("1.0".to_string()),
            )
            .unwrap()
        );
        assert!(
            provided_range_matches_requirement(
                VersionScheme::Debian,
                Some(ProvideVersionRelation::GreaterOrEqual),
                Some("2.0"),
                &RepoVersionConstraint::GreaterOrEqual("1.0".to_string()),
            )
            .is_err()
        );
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

    #[test]
    fn ccs_release_is_numeric_positive_and_orders_equal_versions() {
        for invalid in ["", "0", "-1", "1.0", " 1", "1 "] {
            assert!(validate_package_release(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(validate_package_release("12").unwrap(), 12);
        assert_eq!(
            compare_package_identities(
                VersionScheme::Conary,
                "1.2.3",
                Some("10"),
                VersionScheme::Conary,
                "1.2.3",
                Some("2"),
            )
            .unwrap(),
            Ordering::Greater
        );
    }
}
