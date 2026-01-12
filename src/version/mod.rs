// src/version/mod.rs

//! Version handling and constraint satisfaction for package dependencies
//!
//! This module provides version parsing and comparison for RPM-style versions,
//! including support for epoch:version-release format and version constraints.

use crate::error::{Error, Result};
use semver::Version;
use std::cmp::Ordering;
use std::fmt;

/// A parsed RPM version with epoch, version, and release components
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RpmVersion {
    pub epoch: u64,
    pub version: String,
    pub release: Option<String>,
}

impl RpmVersion {
    /// Parse an RPM version string
    ///
    /// Format: [epoch:]version[-release]
    /// Examples:
    /// - "1.2.3" → epoch=0, version="1.2.3", release=None
    /// - "2:1.2.3" → epoch=2, version="1.2.3", release=None
    /// - "1.2.3-4.el8" → epoch=0, version="1.2.3", release=Some("4.el8")
    /// - "1:2.3.4-5.el8" → epoch=1, version="2.3.4", release=Some("5.el8")
    pub fn parse(s: &str) -> Result<Self> {
        let (epoch_str, rest) = if let Some(colon_pos) = s.find(':') {
            let (e, r) = s.split_at(colon_pos);
            (e, &r[1..]) // Skip the colon
        } else {
            ("0", s)
        };

        let epoch = if epoch_str.is_empty() {
            0 // Empty epoch (e.g., ":1.0.0") defaults to 0
        } else {
            epoch_str.parse::<u64>().map_err(|e| {
                Error::InitError(format!("Invalid epoch in version '{}': {}", s, e))
            })?
        };

        let (version, release) = if let Some(dash_pos) = rest.find('-') {
            let (v, r) = rest.split_at(dash_pos);
            (v.to_string(), Some(r[1..].to_string()))
        } else {
            (rest.to_string(), None)
        };

        if version.is_empty() {
            return Err(Error::InitError(format!(
                "Empty version component in '{}'",
                s
            )));
        }

        Ok(Self {
            epoch,
            version,
            release,
        })
    }

    /// Convert to a semver::Version for comparison
    ///
    /// RPM versions may not be semver-compliant, so we normalize them:
    /// - If version parses as semver, use it directly
    /// - Otherwise, try to extract major.minor.patch from version string
    fn to_semver(&self) -> Result<Version> {
        // First try to parse version directly
        if let Ok(v) = Version::parse(&self.version) {
            return Ok(v);
        }

        // Try to extract numbers and create a semver-compliant version
        let parts: Vec<&str> = self.version.split('.').collect();
        let major = parts.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

        Ok(Version::new(major, minor, patch))
    }

    /// Compare two RPM versions
    pub fn compare(&self, other: &RpmVersion) -> Ordering {
        // First compare epochs
        match self.epoch.cmp(&other.epoch) {
            Ordering::Equal => {}
            ord => return ord,
        }

        // Then compare versions using semver if possible
        match (self.to_semver(), other.to_semver()) {
            (Ok(v1), Ok(v2)) => match v1.cmp(&v2) {
                Ordering::Equal => {}
                ord => return ord,
            },
            _ => {
                // Fall back to string comparison if semver parsing fails
                match self.version.cmp(&other.version) {
                    Ordering::Equal => {}
                    ord => return ord,
                }
            }
        }

        // Finally compare releases (lexicographically)
        self.release.cmp(&other.release)
    }
}

impl fmt::Display for RpmVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.epoch > 0 {
            write!(f, "{}:", self.epoch)?;
        }
        write!(f, "{}", self.version)?;
        if let Some(ref release) = self.release {
            write!(f, "-{}", release)?;
        }
        Ok(())
    }
}

impl Ord for RpmVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.compare(other)
    }
}

impl PartialOrd for RpmVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Version constraint operators
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionConstraint {
    /// Any version is acceptable
    Any,
    /// Exact version match
    Exact(RpmVersion),
    /// Greater than
    GreaterThan(RpmVersion),
    /// Greater than or equal
    GreaterOrEqual(RpmVersion),
    /// Less than
    LessThan(RpmVersion),
    /// Less than or equal
    LessOrEqual(RpmVersion),
    /// Not equal
    NotEqual(RpmVersion),
    /// Both constraints must be satisfied (for ranges like ">= 1.0, < 2.0")
    And(Box<VersionConstraint>, Box<VersionConstraint>),
}

impl VersionConstraint {
    /// Parse a version constraint string
    ///
    /// Examples:
    /// - ">= 1.2.3" → GreaterOrEqual(1.2.3)
    /// - "< 2.0.0" → LessThan(2.0.0)
    /// - "= 1.5.0" → Exact(1.5.0)
    /// - "> 1.0" → GreaterThan(1.0)
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();

        if s.is_empty() || s == "*" {
            return Ok(VersionConstraint::Any);
        }

        // Check for compound constraints (e.g., ">= 1.0, < 2.0")
        if s.contains(',') {
            let parts: Vec<&str> = s.split(',').map(|p| p.trim()).collect();
            if parts.len() == 2 {
                let left = Self::parse(parts[0])?;
                let right = Self::parse(parts[1])?;
                return Ok(VersionConstraint::And(Box::new(left), Box::new(right)));
            }
        }

        // Parse single constraint
        if let Some(rest) = s.strip_prefix(">=") {
            let version = RpmVersion::parse(rest.trim())?;
            Ok(VersionConstraint::GreaterOrEqual(version))
        } else if let Some(rest) = s.strip_prefix("<=") {
            let version = RpmVersion::parse(rest.trim())?;
            Ok(VersionConstraint::LessOrEqual(version))
        } else if let Some(rest) = s.strip_prefix("!=") {
            let version = RpmVersion::parse(rest.trim())?;
            Ok(VersionConstraint::NotEqual(version))
        } else if let Some(rest) = s.strip_prefix('>') {
            let version = RpmVersion::parse(rest.trim())?;
            Ok(VersionConstraint::GreaterThan(version))
        } else if let Some(rest) = s.strip_prefix('<') {
            let version = RpmVersion::parse(rest.trim())?;
            Ok(VersionConstraint::LessThan(version))
        } else if let Some(rest) = s.strip_prefix('=') {
            let version = RpmVersion::parse(rest.trim())?;
            Ok(VersionConstraint::Exact(version))
        } else {
            // No operator means exact match
            let version = RpmVersion::parse(s)?;
            Ok(VersionConstraint::Exact(version))
        }
    }

    /// Check if a version satisfies this constraint
    pub fn satisfies(&self, version: &RpmVersion) -> bool {
        match self {
            VersionConstraint::Any => true,
            VersionConstraint::Exact(v) => version == v,
            VersionConstraint::GreaterThan(v) => version > v,
            VersionConstraint::GreaterOrEqual(v) => version >= v,
            VersionConstraint::LessThan(v) => version < v,
            VersionConstraint::LessOrEqual(v) => version <= v,
            VersionConstraint::NotEqual(v) => version != v,
            VersionConstraint::And(left, right) => {
                left.satisfies(version) && right.satisfies(version)
            }
        }
    }

    /// Check if two constraints are compatible (can be satisfied simultaneously)
    pub fn is_compatible_with(&self, other: &VersionConstraint) -> bool {
        // This is a simplified check - could be more sophisticated
        match (self, other) {
            (VersionConstraint::Any, _) | (_, VersionConstraint::Any) => true,
            (VersionConstraint::Exact(v1), VersionConstraint::Exact(v2)) => v1 == v2,
            _ => {
                // For complex cases, we'd need to check if there exists any version
                // that satisfies both constraints. For now, assume compatible.
                true
            }
        }
    }
}

impl fmt::Display for VersionConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionConstraint::Any => write!(f, "*"),
            VersionConstraint::Exact(v) => write!(f, "= {}", v),
            VersionConstraint::GreaterThan(v) => write!(f, "> {}", v),
            VersionConstraint::GreaterOrEqual(v) => write!(f, ">= {}", v),
            VersionConstraint::LessThan(v) => write!(f, "< {}", v),
            VersionConstraint::LessOrEqual(v) => write!(f, "<= {}", v),
            VersionConstraint::NotEqual(v) => write!(f, "!= {}", v),
            VersionConstraint::And(left, right) => write!(f, "{}, {}", left, right),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpm_version_parse_simple() {
        let v = RpmVersion::parse("1.2.3").unwrap();
        assert_eq!(v.epoch, 0);
        assert_eq!(v.version, "1.2.3");
        assert_eq!(v.release, None);
    }

    #[test]
    fn test_rpm_version_parse_with_epoch() {
        let v = RpmVersion::parse("2:1.2.3").unwrap();
        assert_eq!(v.epoch, 2);
        assert_eq!(v.version, "1.2.3");
        assert_eq!(v.release, None);
    }

    #[test]
    fn test_rpm_version_parse_with_release() {
        let v = RpmVersion::parse("1.2.3-4.el8").unwrap();
        assert_eq!(v.epoch, 0);
        assert_eq!(v.version, "1.2.3");
        assert_eq!(v.release, Some("4.el8".to_string()));
    }

    #[test]
    fn test_rpm_version_parse_full() {
        let v = RpmVersion::parse("1:2.3.4-5.el8").unwrap();
        assert_eq!(v.epoch, 1);
        assert_eq!(v.version, "2.3.4");
        assert_eq!(v.release, Some("5.el8".to_string()));
    }

    #[test]
    fn test_rpm_version_compare_epochs() {
        let v1 = RpmVersion::parse("1:1.0.0").unwrap();
        let v2 = RpmVersion::parse("0:2.0.0").unwrap();
        assert!(v1 > v2); // Higher epoch wins even with lower version
    }

    #[test]
    fn test_rpm_version_compare_versions() {
        let v1 = RpmVersion::parse("1.2.3").unwrap();
        let v2 = RpmVersion::parse("1.2.4").unwrap();
        assert!(v1 < v2);
    }

    #[test]
    fn test_rpm_version_compare_releases() {
        let v1 = RpmVersion::parse("1.2.3-1").unwrap();
        let v2 = RpmVersion::parse("1.2.3-2").unwrap();
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_constraint_parse_exact() {
        let c = VersionConstraint::parse("1.2.3").unwrap();
        let v = RpmVersion::parse("1.2.3").unwrap();
        assert!(c.satisfies(&v));
    }

    #[test]
    fn test_version_constraint_parse_greater_or_equal() {
        let c = VersionConstraint::parse(">= 1.2.0").unwrap();
        let v1 = RpmVersion::parse("1.2.0").unwrap();
        let v2 = RpmVersion::parse("1.3.0").unwrap();
        let v3 = RpmVersion::parse("1.1.0").unwrap();

        assert!(c.satisfies(&v1));
        assert!(c.satisfies(&v2));
        assert!(!c.satisfies(&v3));
    }

    #[test]
    fn test_version_constraint_parse_less_than() {
        let c = VersionConstraint::parse("< 2.0.0").unwrap();
        let v1 = RpmVersion::parse("1.9.9").unwrap();
        let v2 = RpmVersion::parse("2.0.0").unwrap();

        assert!(c.satisfies(&v1));
        assert!(!c.satisfies(&v2));
    }

    #[test]
    fn test_version_constraint_and() {
        let c = VersionConstraint::parse(">= 1.0.0, < 2.0.0").unwrap();
        let v1 = RpmVersion::parse("1.5.0").unwrap();
        let v2 = RpmVersion::parse("2.0.0").unwrap();
        let v3 = RpmVersion::parse("0.9.0").unwrap();

        assert!(c.satisfies(&v1));
        assert!(!c.satisfies(&v2));
        assert!(!c.satisfies(&v3));
    }

    #[test]
    fn test_version_constraint_any() {
        let c = VersionConstraint::parse("*").unwrap();
        let v = RpmVersion::parse("99.99.99").unwrap();
        assert!(c.satisfies(&v));
    }

    #[test]
    fn test_rpm_version_parse_empty_epoch() {
        // Some packages have versions like ":1.02.208-2.fc43" with empty epoch
        let v = RpmVersion::parse(":1.02.208-2.fc43").unwrap();
        assert_eq!(v.epoch, 0);
        assert_eq!(v.version, "1.02.208");
        assert_eq!(v.release, Some("2.fc43".to_string()));
    }

    #[test]
    fn test_rpm_version_display() {
        let v1 = RpmVersion::parse("1.2.3").unwrap();
        assert_eq!(v1.to_string(), "1.2.3");

        let v2 = RpmVersion::parse("2:1.2.3-4.el8").unwrap();
        assert_eq!(v2.to_string(), "2:1.2.3-4.el8");
    }

    #[test]
    fn test_version_constraint_display() {
        let c1 = VersionConstraint::parse(">= 1.2.0").unwrap();
        assert_eq!(c1.to_string(), ">= 1.2.0");

        let c2 = VersionConstraint::parse(">= 1.0.0, < 2.0.0").unwrap();
        assert_eq!(c2.to_string(), ">= 1.0.0, < 2.0.0");
    }
}
