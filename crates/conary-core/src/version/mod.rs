// crates/conary-core/src/version/mod.rs

//! Version handling and constraint satisfaction for package dependencies
//!
//! This module provides version parsing and comparison for RPM-style versions,
//! including support for epoch:version-release format and version constraints.

use crate::error::{Error, Result};
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
        crate::repository::versioning::validate_repo_version(
            crate::repository::versioning::VersionScheme::Rpm,
            s,
        )
        .map_err(|error| Error::VersionParse(error.to_string()))?;

        let (epoch_str, rest) = if let Some(colon_pos) = s.find(':') {
            let (e, r) = s.split_at(colon_pos);
            (e, &r[1..]) // Skip the colon
        } else {
            ("0", s)
        };

        let epoch = epoch_str.parse::<u64>().map_err(|error| {
            Error::VersionParse(format!("Invalid epoch in version '{s}': {error}"))
        })?;

        let (version, release) = if let Some(dash_pos) = rest.find('-') {
            let (v, r) = rest.split_at(dash_pos);
            (v.to_string(), Some(r[1..].to_string()))
        } else {
            (rest.to_string(), None)
        };

        Ok(Self {
            epoch,
            version,
            release,
        })
    }

    /// Compare two already-validated RPM versions with the upstream comparator.
    pub fn compare(&self, other: &RpmVersion) -> Ordering {
        rpm_version::rpm_evr_compare(&self.to_string(), &other.to_string())
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

        // Check for compound constraints (e.g., ">= 1.0, < 2.0, != 1.5")
        if s.contains(',') {
            let parts: Vec<&str> = s.split(',').map(|p| p.trim()).collect();
            if parts.len() >= 2 {
                let mut result = Self::parse(parts[0])?;
                for part in &parts[1..] {
                    let right = Self::parse(part)?;
                    result = VersionConstraint::And(Box::new(result), Box::new(right));
                }
                return Ok(result);
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

    /// Check if a version satisfies this constraint.
    ///
    /// NOTE: This uses RPM-style version comparison (`RpmVersion::compare`).
    /// It should only be called for packages whose persisted version scheme is
    /// RPM. For Debian or Arch versions, use the scheme-aware comparison in
    /// `repository::versioning` instead.
    pub fn satisfies(&self, version: &RpmVersion) -> bool {
        match self {
            VersionConstraint::Any => true,
            VersionConstraint::Exact(v) => version.compare(v).is_eq(),
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
        match (self, other) {
            // Any is always compatible with everything
            (VersionConstraint::Any, _) | (_, VersionConstraint::Any) => true,

            // Two exact constraints: only compatible if they name the same version
            (VersionConstraint::Exact(v1), VersionConstraint::Exact(v2)) => v1.compare(v2).is_eq(),

            // Exact vs range: compatible if the exact version satisfies the range
            (VersionConstraint::Exact(v), range) | (range, VersionConstraint::Exact(v)) => {
                range.satisfies(v)
            }

            // NotEqual is compatible with everything except Exact of same version
            // (handled above); always compatible with ranges and other NotEquals.
            (VersionConstraint::NotEqual(_), _) | (_, VersionConstraint::NotEqual(_)) => true,

            // Two same-direction ranges are always compatible (their intersection is
            // non-empty: e.g. "> 1.0" and "> 2.0" share the region (2.0, ∞)).
            (VersionConstraint::GreaterThan(_), VersionConstraint::GreaterThan(_))
            | (VersionConstraint::GreaterThan(_), VersionConstraint::GreaterOrEqual(_))
            | (VersionConstraint::GreaterOrEqual(_), VersionConstraint::GreaterThan(_))
            | (VersionConstraint::GreaterOrEqual(_), VersionConstraint::GreaterOrEqual(_))
            | (VersionConstraint::LessThan(_), VersionConstraint::LessThan(_))
            | (VersionConstraint::LessThan(_), VersionConstraint::LessOrEqual(_))
            | (VersionConstraint::LessOrEqual(_), VersionConstraint::LessThan(_))
            | (VersionConstraint::LessOrEqual(_), VersionConstraint::LessOrEqual(_)) => true,

            // Opposite-direction ranges: check that the intervals overlap.
            //
            // (> lo) and (< hi): need hi > lo (strict gap between bounds)
            (VersionConstraint::GreaterThan(lo), VersionConstraint::LessThan(hi))
            | (VersionConstraint::LessThan(hi), VersionConstraint::GreaterThan(lo)) => hi > lo,

            // (>= lo) and (< hi): need hi > lo
            (VersionConstraint::GreaterOrEqual(lo), VersionConstraint::LessThan(hi))
            | (VersionConstraint::LessThan(hi), VersionConstraint::GreaterOrEqual(lo)) => hi > lo,

            // (> lo) and (<= hi): need hi > lo
            (VersionConstraint::GreaterThan(lo), VersionConstraint::LessOrEqual(hi))
            | (VersionConstraint::LessOrEqual(hi), VersionConstraint::GreaterThan(lo)) => hi > lo,

            // (>= lo) and (<= hi): need hi >= lo (single-point overlap is OK)
            (VersionConstraint::GreaterOrEqual(lo), VersionConstraint::LessOrEqual(hi))
            | (VersionConstraint::LessOrEqual(hi), VersionConstraint::GreaterOrEqual(lo)) => {
                hi >= lo
            }

            // And(l, r) + other: both sub-constraints must be compatible with other
            (VersionConstraint::And(l, r), other) | (other, VersionConstraint::And(l, r)) => {
                l.is_compatible_with(other) && r.is_compatible_with(other)
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
mod tests;
