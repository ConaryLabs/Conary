// conary-core/src/repository/versioning.rs

//! Native repository version comparison.
//!
//! This module is intentionally separate from `crate::version` so repository-native
//! RPM, Debian, and Arch semantics do not bleed into Conary's older internal
//! versioning substrate.

use crate::db::models::{Repository, RepositoryPackage};
use crate::repository::registry::{RepositoryFormat, detect_repository_format};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Which native version comparison algorithm to use.
///
/// Each distro ecosystem has its own version string format and comparison
/// rules.  This enum selects the correct algorithm so that versions are
/// never compared across incompatible schemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VersionScheme {
    /// RPM-based version ordering (epoch:version-release, segment-based).
    Rpm,
    /// Debian dpkg version ordering (epoch:upstream-revision, tilde semantics).
    Debian,
    /// Arch Linux / ALPM version ordering (epoch:pkgver-pkgrel).
    Arch,
}

/// A version string paired with its comparison scheme.
///
/// Carrying the scheme alongside the string prevents accidental cross-scheme
/// comparison and makes it explicit which algorithm governs ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepositoryVersion {
    /// The raw version string (e.g. `"1:1.2.3-2.fc43"`).
    pub raw: String,
    /// Which comparison scheme applies to this version.
    pub scheme: VersionScheme,
}

impl RepositoryVersion {
    /// Create a new repository version.
    #[must_use]
    pub fn new(raw: String, scheme: VersionScheme) -> Self {
        Self { raw, scheme }
    }

    /// Compare with another version.  Returns `None` if the schemes differ.
    #[must_use]
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        compare_mixed_repo_versions(self.scheme, &self.raw, other.scheme, &other.raw)
    }

    /// Check whether this version satisfies a constraint.
    #[must_use]
    pub fn satisfies(&self, constraint: &RepoVersionConstraint) -> bool {
        repo_version_satisfies(self.scheme, &self.raw, constraint)
    }
}

/// A version constraint in native repository format.
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

pub fn compare_repo_versions(scheme: VersionScheme, a: &str, b: &str) -> Option<Ordering> {
    Some(match scheme {
        VersionScheme::Rpm => compare_rpm_like_versions(a, b),
        VersionScheme::Debian => compare_debian_versions(a, b),
        VersionScheme::Arch => compare_arch_versions(a, b),
    })
}

pub fn compare_mixed_repo_versions(
    a_scheme: VersionScheme,
    a: &str,
    b_scheme: VersionScheme,
    b: &str,
) -> Option<Ordering> {
    (a_scheme == b_scheme)
        .then(|| compare_repo_versions(a_scheme, a, b))
        .flatten()
}

pub fn parse_repo_constraint(_scheme: VersionScheme, raw: &str) -> Option<RepoVersionConstraint> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(RepoVersionConstraint::Any);
    }

    for (op, ctor) in [
        (
            ">=",
            RepoVersionConstraint::GreaterOrEqual as fn(String) -> RepoVersionConstraint,
        ),
        ("<=", RepoVersionConstraint::LessOrEqual),
        ("<<", RepoVersionConstraint::LessThan),
        (">>", RepoVersionConstraint::GreaterThan),
        ("!=", RepoVersionConstraint::NotEqual),
        (">", RepoVersionConstraint::GreaterThan),
        ("<", RepoVersionConstraint::LessThan),
        ("=", RepoVersionConstraint::Exact),
    ] {
        if let Some(rest) = raw.strip_prefix(op) {
            let version = rest.trim();
            if version.is_empty() {
                return None;
            }
            return Some(ctor(version.to_string()));
        }
    }

    Some(RepoVersionConstraint::Exact(raw.to_string()))
}

pub fn repo_version_satisfies(
    scheme: VersionScheme,
    version: &str,
    constraint: &RepoVersionConstraint,
) -> bool {
    match constraint {
        RepoVersionConstraint::Any => true,
        RepoVersionConstraint::Exact(expected) => {
            compare_repo_versions(scheme, version, expected) == Some(Ordering::Equal)
        }
        RepoVersionConstraint::GreaterThan(expected) => {
            compare_repo_versions(scheme, version, expected) == Some(Ordering::Greater)
        }
        RepoVersionConstraint::GreaterOrEqual(expected) => matches!(
            compare_repo_versions(scheme, version, expected),
            Some(Ordering::Greater | Ordering::Equal)
        ),
        RepoVersionConstraint::LessThan(expected) => {
            compare_repo_versions(scheme, version, expected) == Some(Ordering::Less)
        }
        RepoVersionConstraint::LessOrEqual(expected) => matches!(
            compare_repo_versions(scheme, version, expected),
            Some(Ordering::Less | Ordering::Equal)
        ),
        RepoVersionConstraint::NotEqual(expected) => {
            compare_repo_versions(scheme, version, expected) != Some(Ordering::Equal)
        }
    }
}

pub fn infer_version_scheme(repo: &Repository) -> Option<VersionScheme> {
    match detect_repository_format(&repo.name, &repo.url) {
        RepositoryFormat::Fedora => Some(VersionScheme::Rpm),
        RepositoryFormat::Debian => Some(VersionScheme::Debian),
        RepositoryFormat::Arch => Some(VersionScheme::Arch),
        RepositoryFormat::Json => None,
    }
}

pub fn compare_repo_package_versions(
    a: &RepositoryPackage,
    a_repo: &Repository,
    b: &RepositoryPackage,
    b_repo: &Repository,
) -> Option<Ordering> {
    compare_mixed_repo_versions(
        infer_version_scheme(a_repo)?,
        &a.version,
        infer_version_scheme(b_repo)?,
        &b.version,
    )
}

/// Split a version part (version or release) into tilde/caret components.
///
/// RPM and ALPM define two special separators:
/// - `~` (tilde): sorts *before* the base version.  Used for pre-releases:
///   `1.0~rc1 < 1.0`.
/// - `^` (caret): sorts *after* the base version but before the next higher
///   version.  Used for post-release snapshots: `1.0^git1 > 1.0` but
///   `1.0^git1 < 1.1`.
///
/// The returned vector contains `(separator, text)` pairs.  The first element
/// always has separator `None` (the base).  Subsequent elements carry the
/// separator that preceded them.
fn split_tilde_caret(version: &str) -> Vec<(Option<char>, &str)> {
    let mut parts: Vec<(Option<char>, &str)> = Vec::new();
    let mut start = 0;
    let mut pending_sep: Option<char> = None;

    for (i, ch) in version.char_indices() {
        if ch == '~' || ch == '^' {
            parts.push((pending_sep, &version[start..i]));
            pending_sep = Some(ch);
            start = i + 1;
        }
    }

    // Add the final segment (or the whole string if no separators found)
    parts.push((pending_sep, &version[start..]));

    parts
}

/// Compare two tilde/caret-aware version part sequences segment by segment.
///
/// Ordering rules for special separators:
/// - Tilde (`~`) sorts before everything, including end-of-string.
/// - Caret (`^`) sorts after end-of-string but before any regular segment.
fn compare_tilde_caret_parts(
    a_parts: &[(Option<char>, &str)],
    b_parts: &[(Option<char>, &str)],
    flavor: SegmentFlavor,
) -> Ordering {
    let max_len = a_parts.len().max(b_parts.len());

    for i in 0..max_len {
        match (a_parts.get(i), b_parts.get(i)) {
            (None, None) => return Ordering::Equal,
            (None, Some((Some('~'), _))) => {
                // b has a tilde part, a has nothing.  Tilde sorts before
                // everything, so "nothing" (end) is greater than tilde.
                return Ordering::Greater;
            }
            (None, Some((Some('^'), _))) => {
                // b has a caret part, a has nothing.  Caret sorts after
                // end-of-string? No: end-of-string means "this IS the
                // version", caret means "version + snapshot".  So a (bare)
                // is less than b (with caret).
                // Wait -- RPM semantics: 1.0 < 1.0^git1.
                // If a ran out and b has ^, a is Less.
                return Ordering::Less;
            }
            (None, Some(_)) => {
                // b has more regular segments, a ran out
                return Ordering::Less;
            }
            (Some((Some('~'), _)), None) => {
                // a has a tilde part, b has nothing.  Tilde sorts before everything.
                return Ordering::Less;
            }
            (Some((Some('^'), _)), None) => {
                // a has a caret part, b has nothing.  1.0^git1 > 1.0.
                return Ordering::Greater;
            }
            (Some(_), None) => {
                return Ordering::Greater;
            }
            (Some((a_sep, a_text)), Some((b_sep, b_text))) => {
                // Compare separators first (for i > 0)
                if i > 0 {
                    let ord = match (a_sep, b_sep) {
                        (Some('~'), Some('~')) => Ordering::Equal,
                        (Some('~'), _) => Ordering::Less,
                        (_, Some('~')) => Ordering::Greater,
                        (Some('^'), Some('^')) => Ordering::Equal,
                        (Some('^'), _) => Ordering::Less,
                        (_, Some('^')) => Ordering::Greater,
                        _ => Ordering::Equal,
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }

                // Compare the text within this segment
                let ord = compare_segmented(a_text, b_text, flavor);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }

    Ordering::Equal
}

fn compare_rpm_like_versions(a: &str, b: &str) -> Ordering {
    let (a_epoch, a_rest) = split_epoch(a);
    let (b_epoch, b_rest) = split_epoch(b);
    match a_epoch.cmp(&b_epoch) {
        Ordering::Equal => {}
        ord => return ord,
    }

    let (a_version, a_release) = split_release(a_rest);
    let (b_version, b_release) = split_release(b_rest);

    // Handle tilde/caret in the version part
    let a_tc = split_tilde_caret(a_version);
    let b_tc = split_tilde_caret(b_version);
    match compare_tilde_caret_parts(&a_tc, &b_tc, SegmentFlavor::RpmLike) {
        Ordering::Equal => {
            // Also handle tilde/caret in the release part (e.g. 1.0-1~rc1)
            match (a_release, b_release) {
                (Some(a_rel), Some(b_rel)) => {
                    let a_rel_tc = split_tilde_caret(a_rel);
                    let b_rel_tc = split_tilde_caret(b_rel);
                    compare_tilde_caret_parts(&a_rel_tc, &b_rel_tc, SegmentFlavor::RpmLike)
                }
                (Some(_), None) => Ordering::Greater,
                (None, Some(_)) => Ordering::Less,
                (None, None) => Ordering::Equal,
            }
        }
        ord => ord,
    }
}

fn compare_arch_versions(a: &str, b: &str) -> Ordering {
    let (a_epoch, a_rest) = split_epoch(a);
    let (b_epoch, b_rest) = split_epoch(b);
    match a_epoch.cmp(&b_epoch) {
        Ordering::Equal => {}
        ord => return ord,
    }

    let (a_version, a_release) = split_release(a_rest);
    let (b_version, b_release) = split_release(b_rest);

    // Handle tilde/caret in the version part
    let a_tc = split_tilde_caret(a_version);
    let b_tc = split_tilde_caret(b_version);
    match compare_tilde_caret_parts(&a_tc, &b_tc, SegmentFlavor::Arch) {
        Ordering::Equal => {
            // Also handle tilde/caret in the release/pkgrel part
            match (a_release, b_release) {
                (Some(a_rel), Some(b_rel)) => {
                    let a_rel_tc = split_tilde_caret(a_rel);
                    let b_rel_tc = split_tilde_caret(b_rel);
                    compare_tilde_caret_parts(&a_rel_tc, &b_rel_tc, SegmentFlavor::Arch)
                }
                (Some(_), None) => Ordering::Greater,
                (None, Some(_)) => Ordering::Less,
                (None, None) => Ordering::Equal,
            }
        }
        ord => ord,
    }
}

fn compare_debian_versions(a: &str, b: &str) -> Ordering {
    let (a_epoch, a_rest) = split_epoch(a);
    let (b_epoch, b_rest) = split_epoch(b);
    match a_epoch.cmp(&b_epoch) {
        Ordering::Equal => {}
        ord => return ord,
    }

    let (a_upstream, a_revision) = split_debian_revision(a_rest);
    let (b_upstream, b_revision) = split_debian_revision(b_rest);
    match compare_debian_part(a_upstream, b_upstream) {
        Ordering::Equal => compare_debian_part(a_revision, b_revision),
        ord => ord,
    }
}

fn split_epoch(version: &str) -> (u64, &str) {
    if let Some((epoch, rest)) = version.split_once(':') {
        return (epoch.parse::<u64>().unwrap_or(0), rest);
    }
    (0, version)
}

fn split_release(version: &str) -> (&str, Option<&str>) {
    if let Some((pkgver, release)) = version.rsplit_once('-') {
        (pkgver, Some(release))
    } else {
        (version, None)
    }
}

fn split_debian_revision(version: &str) -> (&str, &str) {
    if let Some((upstream, revision)) = version.rsplit_once('-') {
        (upstream, revision)
    } else {
        (version, "0")
    }
}

#[derive(Clone, Copy)]
enum SegmentFlavor {
    RpmLike,
    Arch,
}

fn compare_segmented(a: &str, b: &str, flavor: SegmentFlavor) -> Ordering {
    let a_segments = split_segments(a, flavor);
    let b_segments = split_segments(b, flavor);

    for i in 0..a_segments.len().max(b_segments.len()) {
        match (a_segments.get(i), b_segments.get(i)) {
            (None, None) => return Ordering::Equal,
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (Some(sa), Some(sb)) => {
                let a_is_num = sa.chars().all(|c| c.is_ascii_digit());
                let b_is_num = sb.chars().all(|c| c.is_ascii_digit());
                match (a_is_num, b_is_num) {
                    (true, true) => {
                        let ord = compare_numeric_strings(sa, sb);
                        if ord != Ordering::Equal {
                            return ord;
                        }
                    }
                    (true, false) => return Ordering::Greater,
                    (false, true) => return Ordering::Less,
                    (false, false) => {
                        let ord = sa.cmp(sb);
                        if ord != Ordering::Equal {
                            return ord;
                        }
                    }
                }
            }
        }
    }

    Ordering::Equal
}

fn split_segments(version: &str, flavor: SegmentFlavor) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut i = 0;
    let bytes = version.as_bytes();

    while i < bytes.len() {
        if is_segment_separator(bytes[i], flavor) {
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
                && !is_segment_separator(bytes[i], flavor)
            {
                i += 1;
            }
        }
        segments.push(&version[start..i]);
    }

    segments
}

fn is_segment_separator(byte: u8, flavor: SegmentFlavor) -> bool {
    match flavor {
        SegmentFlavor::RpmLike => matches!(byte, b'.' | b'-' | b'_'),
        SegmentFlavor::Arch => matches!(byte, b'.' | b'-' | b'_' | b'+'),
    }
}

fn compare_numeric_strings(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    match a.len().cmp(&b.len()) {
        Ordering::Equal => a.cmp(b),
        ord => ord,
    }
}

fn compare_debian_part(a: &str, b: &str) -> Ordering {
    let mut a = a;
    let mut b = b;

    while !a.is_empty() || !b.is_empty() {
        let (a_non_digit, a_rest) = take_non_digits(a);
        let (b_non_digit, b_rest) = take_non_digits(b);
        let non_digit_ord = compare_debian_non_digits(a_non_digit, b_non_digit);
        if non_digit_ord != Ordering::Equal {
            return non_digit_ord;
        }

        let (a_digits, next_a) = take_digits(a_rest);
        let (b_digits, next_b) = take_digits(b_rest);
        let digit_ord = compare_numeric_strings(a_digits, b_digits);
        if digit_ord != Ordering::Equal {
            return digit_ord;
        }

        a = next_a;
        b = next_b;
    }

    Ordering::Equal
}

fn take_non_digits(s: &str) -> (&str, &str) {
    let idx = s
        .char_indices()
        .find(|(_, ch)| ch.is_ascii_digit())
        .map(|(idx, _)| idx)
        .unwrap_or(s.len());
    s.split_at(idx)
}

fn take_digits(s: &str) -> (&str, &str) {
    let idx = s
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .map(|(idx, _)| idx)
        .unwrap_or(s.len());
    s.split_at(idx)
}

fn compare_debian_non_digits(a: &str, b: &str) -> Ordering {
    let mut a_chars = a.chars();
    let mut b_chars = b.chars();

    loop {
        let ord = debian_char_order(a_chars.next()).cmp(&debian_char_order(b_chars.next()));
        if ord != Ordering::Equal {
            return ord;
        }

        if a_chars.as_str().is_empty() && b_chars.as_str().is_empty() {
            return Ordering::Equal;
        }
    }
}

fn debian_char_order(ch: Option<char>) -> i32 {
    match ch {
        Some('~') => -1,
        None => 0,
        Some(ch) if ch.is_ascii_alphabetic() => ch as i32,
        Some(ch) => ch as i32 + 256,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn compares_rpm_versions_natively() {
        assert_eq!(
            compare_repo_versions(VersionScheme::Rpm, "1.2.3-2.fc43", "1.2.3-1.fc43"),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn compares_debian_versions_natively() {
        assert_eq!(
            compare_repo_versions(VersionScheme::Debian, "1.0", "1.0~beta1"),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn compares_arch_versions_natively() {
        assert_eq!(
            compare_repo_versions(VersionScheme::Arch, "1:1.0-2", "1.0-3"),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn rejects_cross_scheme_comparison() {
        assert_eq!(
            compare_mixed_repo_versions(VersionScheme::Debian, "1.0", VersionScheme::Arch, "1.0-1"),
            None
        );
    }

    #[test]
    fn debian_constraints_use_native_ordering() {
        let constraint =
            parse_repo_constraint(VersionScheme::Debian, ">= 1.0~beta1").expect("constraint");
        assert!(repo_version_satisfies(
            VersionScheme::Debian,
            "1.0",
            &constraint
        ));
        assert!(!repo_version_satisfies(
            VersionScheme::Debian,
            "0.9",
            &constraint
        ));
    }

    #[test]
    fn arch_constraints_respect_epoch() {
        let constraint =
            parse_repo_constraint(VersionScheme::Arch, ">= 1:1.0-1").expect("constraint");
        assert!(repo_version_satisfies(
            VersionScheme::Arch,
            "1:1.0-2",
            &constraint
        ));
        assert!(!repo_version_satisfies(
            VersionScheme::Arch,
            "1.0-9",
            &constraint
        ));
    }

    #[test]
    fn repository_version_same_scheme_compare() {
        let a = RepositoryVersion::new("1.2.3-2.fc43".to_string(), VersionScheme::Rpm);
        let b = RepositoryVersion::new("1.2.3-1.fc43".to_string(), VersionScheme::Rpm);
        assert_eq!(a.compare(&b), Some(Ordering::Greater));
    }

    #[test]
    fn repository_version_cross_scheme_returns_none() {
        let rpm = RepositoryVersion::new("1.0".to_string(), VersionScheme::Rpm);
        let deb = RepositoryVersion::new("1.0".to_string(), VersionScheme::Debian);
        assert_eq!(rpm.compare(&deb), None);
    }

    #[test]
    fn repository_version_satisfies_constraint() {
        let v = RepositoryVersion::new("1.0".to_string(), VersionScheme::Debian);
        let constraint =
            parse_repo_constraint(VersionScheme::Debian, ">= 0.9").expect("constraint");
        assert!(v.satisfies(&constraint));
    }

    // -- RPM tilde/caret tests --

    #[test]
    fn rpm_tilde_sorts_before_release() {
        // 1.0~rc1 < 1.0
        assert_eq!(
            compare_repo_versions(VersionScheme::Rpm, "1.0~rc1", "1.0"),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn rpm_caret_sorts_after_release() {
        // 1.0^git1 > 1.0
        assert_eq!(
            compare_repo_versions(VersionScheme::Rpm, "1.0^git1", "1.0"),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn rpm_caret_sorts_before_next_version() {
        // 1.0^git1 < 1.1
        assert_eq!(
            compare_repo_versions(VersionScheme::Rpm, "1.0^git1", "1.1"),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn rpm_epoch_overrides_version() {
        // 2:1.0 > 1:2.0
        assert_eq!(
            compare_repo_versions(VersionScheme::Rpm, "2:1.0", "1:2.0"),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn rpm_tilde_earlier_than_tilde() {
        // 1.0~alpha < 1.0~beta (alpha < beta lexically)
        assert_eq!(
            compare_repo_versions(VersionScheme::Rpm, "1.0~alpha", "1.0~beta"),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn rpm_caret_vs_caret() {
        // 1.0^git1 < 1.0^git2
        assert_eq!(
            compare_repo_versions(VersionScheme::Rpm, "1.0^git1", "1.0^git2"),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn rpm_tilde_before_caret() {
        // 1.0~rc1 < 1.0^git1 (tilde sorts before everything)
        assert_eq!(
            compare_repo_versions(VersionScheme::Rpm, "1.0~rc1", "1.0^git1"),
            Some(Ordering::Less)
        );
    }

    // -- ALPM tilde/caret tests (same semantics) --

    #[test]
    fn arch_tilde_sorts_before_release() {
        assert_eq!(
            compare_repo_versions(VersionScheme::Arch, "1.0~rc1", "1.0"),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn arch_caret_sorts_after_release() {
        assert_eq!(
            compare_repo_versions(VersionScheme::Arch, "1.0^git1", "1.0"),
            Some(Ordering::Greater)
        );
    }
}
