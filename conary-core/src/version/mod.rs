// conary-core/src/version/mod.rs

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
                Error::VersionParse(format!("Invalid epoch in version '{}': {}", s, e))
            })?
        };

        let (version, release) = if let Some(dash_pos) = rest.find('-') {
            let (v, r) = rest.split_at(dash_pos);
            (v.to_string(), Some(r[1..].to_string()))
        } else {
            (rest.to_string(), None)
        };

        if version.is_empty() {
            return Err(Error::VersionParse(format!(
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

    /// Compare version strings using RPM's `rpmvercmp` algorithm.
    ///
    /// The algorithm splits each string into alternating runs of digits and
    /// non-digit characters (skipping separators like `.` and `-`). Digit
    /// runs are compared numerically (with leading zeros stripped), alpha
    /// runs are compared lexicographically, and digit runs always sort
    /// after alpha runs.
    fn compare_version_strings(a: &str, b: &str) -> Ordering {
        let segments_a = Self::split_version_segments(a);
        let segments_b = Self::split_version_segments(b);

        for i in 0..segments_a.len().max(segments_b.len()) {
            let seg_a = segments_a.get(i);
            let seg_b = segments_b.get(i);

            match (seg_a, seg_b) {
                (None, None) => return Ordering::Equal,
                (Some(_), None) => return Ordering::Greater,
                (None, Some(_)) => return Ordering::Less,
                (Some(sa), Some(sb)) => {
                    let a_is_num = sa.chars().all(|c| c.is_ascii_digit());
                    let b_is_num = sb.chars().all(|c| c.is_ascii_digit());

                    match (a_is_num, b_is_num) {
                        // Both numeric: compare as numbers
                        (true, true) => {
                            let a_trimmed = sa.trim_start_matches('0');
                            let b_trimmed = sb.trim_start_matches('0');
                            match a_trimmed.len().cmp(&b_trimmed.len()) {
                                Ordering::Equal => match a_trimmed.cmp(b_trimmed) {
                                    Ordering::Equal => continue,
                                    ord => return ord,
                                },
                                ord => return ord,
                            }
                        }
                        // Digits always beat alphas in RPM
                        (true, false) => return Ordering::Greater,
                        (false, true) => return Ordering::Less,
                        // Both alpha: lexicographic
                        (false, false) => match sa.cmp(sb) {
                            Ordering::Equal => continue,
                            ord => return ord,
                        },
                    }
                }
            }
        }

        Ordering::Equal
    }

    /// Split a version string into alternating runs of digits and non-digits,
    /// skipping separator characters (`.`, `-`, `_`).
    fn split_version_segments(s: &str) -> Vec<&str> {
        let mut segments = Vec::new();
        let mut i = 0;
        let bytes = s.as_bytes();

        while i < bytes.len() {
            // Skip separators
            if bytes[i] == b'.' || bytes[i] == b'-' || bytes[i] == b'_' {
                i += 1;
                continue;
            }

            let start = i;
            if bytes[i].is_ascii_digit() {
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            } else {
                while i < bytes.len()
                    && !bytes[i].is_ascii_digit()
                    && bytes[i] != b'.'
                    && bytes[i] != b'-'
                    && bytes[i] != b'_'
                {
                    i += 1;
                }
            }
            segments.push(&s[start..i]);
        }

        segments
    }

    /// Compare two RPM versions.
    ///
    /// Delegates to `repository::versioning::compare_repo_versions` when either
    /// version string contains `~` or `^` (tilde/caret pre-release and snapshot
    /// markers).  For versions without those characters, uses the faster inline
    /// segment comparison.
    pub fn compare(&self, other: &RpmVersion) -> Ordering {
        // First compare epochs
        match self.epoch.cmp(&other.epoch) {
            Ordering::Equal => {}
            ord => return ord,
        }

        // If either side contains tilde or caret, delegate to the
        // tilde/caret-aware comparator in repository::versioning which
        // implements full RPM semantics for ~pre-release and ^snapshot.
        let a_has_special = self.version.contains('~')
            || self.version.contains('^')
            || self.release.as_ref().is_some_and(|r| r.contains('~') || r.contains('^'));
        let b_has_special = other.version.contains('~')
            || other.version.contains('^')
            || other.release.as_ref().is_some_and(|r| r.contains('~') || r.contains('^'));

        if a_has_special || b_has_special {
            // Reconstruct the version-release strings (without epoch, which
            // we already compared above).
            let a_str = match &self.release {
                Some(rel) => format!("{}-{}", self.version, rel),
                None => self.version.clone(),
            };
            let b_str = match &other.release {
                Some(rel) => format!("{}-{}", other.version, rel),
                None => other.version.clone(),
            };
            return crate::repository::versioning::compare_repo_versions(
                crate::repository::versioning::VersionScheme::Rpm,
                &a_str,
                &b_str,
            )
            .unwrap_or(Ordering::Equal);
        }

        // Fast path: no tilde/caret, use inline segment comparison
        match Self::compare_version_strings(&self.version, &other.version) {
            Ordering::Equal => {}
            ord => return ord,
        }

        // Finally compare releases using numeric-aware comparison
        match (&self.release, &other.release) {
            (Some(a), Some(b)) => Self::compare_version_strings(a, b),
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
        }
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
    /// It should only be called for packages whose version scheme is RPM or
    /// legacy (no stored scheme). For Debian or Arch versions, use the
    /// scheme-aware comparison in `repository::versioning` instead.
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

    #[test]
    fn test_exact_match_normalizes_epoch_and_release() {
        // "1.2.3" (no epoch/release) should match "0:1.2.3" (explicit epoch 0)
        let c = VersionConstraint::parse("= 1.2.3").unwrap();
        let v = RpmVersion::parse("0:1.2.3").unwrap();
        assert!(c.satisfies(&v));

        // "0:1.2.3" constraint should match "1.2.3" (no epoch)
        let c = VersionConstraint::parse("= 0:1.2.3").unwrap();
        let v = RpmVersion::parse("1.2.3").unwrap();
        assert!(c.satisfies(&v));

        // Release None vs Some should not match
        let c = VersionConstraint::parse("= 1.2.3").unwrap();
        let v = RpmVersion::parse("1.2.3-1.fc43").unwrap();
        assert!(!c.satisfies(&v));
    }

    #[test]
    fn test_rpmvercmp_digits_beat_alpha() {
        // Digit segments always sort after alpha segments in RPM
        let v1 = RpmVersion::parse("1.0a").unwrap();
        let v2 = RpmVersion::parse("1.01").unwrap();
        assert_eq!(v1.compare(&v2), Ordering::Less);
    }

    #[test]
    fn test_rpmvercmp_leading_zeros() {
        let v1 = RpmVersion::parse("1.001").unwrap();
        let v2 = RpmVersion::parse("1.1").unwrap();
        assert_eq!(v1.compare(&v2), Ordering::Equal);
    }

    #[test]
    fn test_rpmvercmp_mixed_alpha_numeric() {
        let v1 = RpmVersion::parse("2.0.1a").unwrap();
        let v2 = RpmVersion::parse("2.0.1b").unwrap();
        assert!(v1 < v2);
    }

    // --- is_compatible_with tests ---

    #[test]
    fn test_compatible_any_with_anything() {
        let any = VersionConstraint::Any;
        let exact = VersionConstraint::parse("= 1.0").unwrap();
        let range = VersionConstraint::parse("> 1.0").unwrap();
        assert!(any.is_compatible_with(&exact));
        assert!(any.is_compatible_with(&range));
        assert!(any.is_compatible_with(&VersionConstraint::Any));
    }

    #[test]
    fn test_compatible_exact_vs_exact_same() {
        let c1 = VersionConstraint::parse("= 1.0").unwrap();
        let c2 = VersionConstraint::parse("= 1.0").unwrap();
        assert!(c1.is_compatible_with(&c2));
    }

    #[test]
    fn test_compatible_exact_vs_exact_different() {
        let c1 = VersionConstraint::parse("= 1.0").unwrap();
        let c2 = VersionConstraint::parse("= 2.0").unwrap();
        assert!(!c1.is_compatible_with(&c2));
    }

    #[test]
    fn test_compatible_exact_vs_range_satisfies() {
        // 1.5 satisfies >= 1.0 and < 2.0
        let exact = VersionConstraint::parse("= 1.5").unwrap();
        let ge = VersionConstraint::parse(">= 1.0").unwrap();
        let lt = VersionConstraint::parse("< 2.0").unwrap();
        assert!(exact.is_compatible_with(&ge));
        assert!(exact.is_compatible_with(&lt));
        assert!(ge.is_compatible_with(&exact));
    }

    #[test]
    fn test_compatible_exact_vs_range_does_not_satisfy() {
        // 0.5 does not satisfy >= 1.0
        let exact = VersionConstraint::parse("= 0.5").unwrap();
        let ge = VersionConstraint::parse(">= 1.0").unwrap();
        assert!(!exact.is_compatible_with(&ge));
        assert!(!ge.is_compatible_with(&exact));
    }

    #[test]
    fn test_compatible_same_direction_ranges() {
        // Both GT — always overlap
        let c1 = VersionConstraint::parse("> 1.0").unwrap();
        let c2 = VersionConstraint::parse("> 3.0").unwrap();
        assert!(c1.is_compatible_with(&c2));

        // Both LT — always overlap
        let c3 = VersionConstraint::parse("< 2.0").unwrap();
        let c4 = VersionConstraint::parse("< 5.0").unwrap();
        assert!(c3.is_compatible_with(&c4));

        // GE + GE
        let c5 = VersionConstraint::parse(">= 1.0").unwrap();
        let c6 = VersionConstraint::parse(">= 2.0").unwrap();
        assert!(c5.is_compatible_with(&c6));
    }

    #[test]
    fn test_compatible_opposite_ranges_overlapping() {
        // > 1.0 and < 3.0 — overlap in (1.0, 3.0)
        let c1 = VersionConstraint::parse("> 1.0").unwrap();
        let c2 = VersionConstraint::parse("< 3.0").unwrap();
        assert!(c1.is_compatible_with(&c2));

        // >= 1.0 and <= 2.0 — overlap at [1.0, 2.0]
        let c3 = VersionConstraint::parse(">= 1.0").unwrap();
        let c4 = VersionConstraint::parse("<= 2.0").unwrap();
        assert!(c3.is_compatible_with(&c4));

        // >= 2.0 and <= 2.0 — single-point overlap at 2.0
        let c5 = VersionConstraint::parse(">= 2.0").unwrap();
        let c6 = VersionConstraint::parse("<= 2.0").unwrap();
        assert!(c5.is_compatible_with(&c6));
    }

    #[test]
    fn test_compatible_opposite_ranges_non_overlapping() {
        // > 3.0 and < 1.0 — no overlap
        let c1 = VersionConstraint::parse("> 3.0").unwrap();
        let c2 = VersionConstraint::parse("< 1.0").unwrap();
        assert!(!c1.is_compatible_with(&c2));

        // >= 3.0 and <= 2.0 — no overlap
        let c3 = VersionConstraint::parse(">= 3.0").unwrap();
        let c4 = VersionConstraint::parse("<= 2.0").unwrap();
        assert!(!c3.is_compatible_with(&c4));

        // > 2.0 and < 2.0 — touching but not overlapping (strict)
        let c5 = VersionConstraint::parse("> 2.0").unwrap();
        let c6 = VersionConstraint::parse("< 2.0").unwrap();
        assert!(!c5.is_compatible_with(&c6));
    }

    #[test]
    fn test_compatible_not_equal() {
        let ne = VersionConstraint::parse("!= 1.0").unwrap();
        let exact_same = VersionConstraint::parse("= 1.0").unwrap();
        let exact_diff = VersionConstraint::parse("= 2.0").unwrap();
        let range = VersionConstraint::parse("> 1.0").unwrap();

        // NotEqual vs different Exact — compatible (2.0 satisfies != 1.0)
        assert!(ne.is_compatible_with(&exact_diff));
        // NotEqual vs range — compatible
        assert!(ne.is_compatible_with(&range));
        // NotEqual vs Exact(same): the Exact arm fires first (range.satisfies(v)),
        // and != 1.0 does NOT satisfy version 1.0, so false.
        assert!(!ne.is_compatible_with(&exact_same));
    }

    #[test]
    fn test_compatible_and_constraint() {
        // And(>= 1.0, < 2.0) is compatible with > 1.5
        let and_c = VersionConstraint::parse(">= 1.0, < 2.0").unwrap();
        let gt = VersionConstraint::parse("> 1.5").unwrap();
        assert!(and_c.is_compatible_with(&gt));

        // And(>= 1.0, < 2.0) is NOT compatible with > 3.0
        let gt_high = VersionConstraint::parse("> 3.0").unwrap();
        assert!(!and_c.is_compatible_with(&gt_high));
    }

    #[test]
    fn test_rpm_version_four_component() {
        let v1 = RpmVersion::parse("1.2.3.4").unwrap();
        let v2 = RpmVersion::parse("1.2.3.5").unwrap();
        assert_eq!(v1.compare(&v2), Ordering::Less);

        let v3 = RpmVersion::parse("1.2.3").unwrap();
        let v4 = RpmVersion::parse("1.2.3.4").unwrap();
        assert_eq!(v3.compare(&v4), Ordering::Less);
    }

    #[test]
    fn test_exact_constraint_uses_rpmvercmp_semantics() {
        // "1.001-1" should match "1.1-1" under Exact because rpmvercmp
        // treats leading zeros as insignificant in numeric segments.
        let c = VersionConstraint::parse("= 1.001-1").unwrap();
        let v = RpmVersion::parse("1.1-1").unwrap();
        assert!(
            c.satisfies(&v),
            "Exact constraint should use rpmvercmp: 1.001-1 == 1.1-1"
        );

        // And the reverse direction
        let c2 = VersionConstraint::parse("= 1.1-1").unwrap();
        let v2 = RpmVersion::parse("1.001-1").unwrap();
        assert!(
            c2.satisfies(&v2),
            "Exact constraint should use rpmvercmp: 1.1-1 == 1.001-1"
        );
    }
}
