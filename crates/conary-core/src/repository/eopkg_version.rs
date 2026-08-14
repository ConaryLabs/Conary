// conary-core/src/repository/eopkg_version.rs

//! Exact eopkg/PISI version grammar and ordering.
//!
//! This is a direct typed transcription of `pisi/version.py` at upstream
//! commit `59e423ea39088cd46f1903b1094127d8fda28d73`. Package update selection is
//! separately release-number based; this comparator owns dependency ranges.

use super::versioning::{VersionComparisonError, VersionResult};
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct VersionItem {
    number: u64,
    trailing: Option<char>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ParsedUpstreamVersion {
    base: Vec<VersionItem>,
    suffix_rank: i8,
    suffix: Vec<VersionItem>,
}

fn invalid(raw: &str, reason: impl Into<String>) -> VersionComparisonError {
    VersionComparisonError::InvalidVersion {
        scheme: "eopkg",
        version: raw.to_string(),
        reason: reason.into(),
    }
}

fn parse_item(raw_version: &str, item: &str) -> VersionResult<VersionItem> {
    if item.is_empty() {
        return Err(invalid(raw_version, "version item is empty"));
    }
    if let Ok(number) = item.parse::<u64>() {
        return Ok(VersionItem {
            number,
            trailing: None,
        });
    }

    let trailing = item
        .chars()
        .next_back()
        .ok_or_else(|| invalid(raw_version, "version item is empty"))?;
    let prefix_len = item.len() - trailing.len_utf8();
    let number = item[..prefix_len].parse::<u64>().map_err(|_| {
        invalid(
            raw_version,
            format!(
                "version item '{item}' is neither an integer nor an integer plus one character"
            ),
        )
    })?;
    Ok(VersionItem {
        number,
        trailing: Some(trailing),
    })
}

fn parse_items(raw_version: &str, value: &str) -> VersionResult<Vec<VersionItem>> {
    value
        .split('.')
        .map(|item| parse_item(raw_version, item))
        .collect()
}

fn parse_upstream(raw: &str) -> VersionResult<ParsedUpstreamVersion> {
    if raw.is_empty() {
        return Err(invalid(raw, "version is empty"));
    }

    let (base_raw, suffix_raw) = raw
        .split_once('_')
        .map_or((raw, None), |(base, suffix)| (base, Some(suffix)));
    let base = parse_items(raw, base_raw)?;
    let Some(suffix_raw) = suffix_raw else {
        return Ok(ParsedUpstreamVersion {
            base,
            suffix_rank: 0,
            suffix: vec![VersionItem {
                number: 0,
                trailing: None,
            }],
        });
    };

    const KEYWORDS: [(&str, i8); 6] = [
        ("alpha", -5),
        ("beta", -4),
        ("pre", -3),
        ("rc", -2),
        ("m", -1),
        ("p", 1),
    ];
    if ("a"..="s").contains(&suffix_raw) {
        if let Some((keyword, rank)) = KEYWORDS
            .iter()
            .find(|(keyword, _)| suffix_raw.starts_with(keyword))
        {
            let remainder = &suffix_raw[keyword.len()..];
            return Ok(ParsedUpstreamVersion {
                base,
                suffix_rank: *rank,
                suffix: parse_items(raw, remainder)?,
            });
        }

        // This is intentionally not permissive guessing. Upstream maps an
        // unrecognized alphabetic suffix in this lexical range to its own
        // no-suffix tuple, and conformance must preserve that behavior.
        return Ok(ParsedUpstreamVersion {
            base,
            suffix_rank: 0,
            suffix: vec![VersionItem {
                number: 0,
                trailing: None,
            }],
        });
    }

    Ok(ParsedUpstreamVersion {
        base,
        suffix_rank: 0,
        suffix: parse_items(raw, suffix_raw)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EopkgVersion {
    pub(crate) version: String,
    pub(crate) release: u64,
}

pub(crate) fn parse(raw: &str) -> VersionResult<EopkgVersion> {
    let (version, release) = raw.rsplit_once('-').ok_or_else(|| {
        invalid(
            raw,
            "package identity must be '<version>-<integer-release>'",
        )
    })?;
    parse_upstream(version)?;
    let release = release
        .parse::<u64>()
        .map_err(|_| invalid(raw, "package release is not an unsigned integer"))?;
    Ok(EopkgVersion {
        version: version.to_string(),
        release,
    })
}

pub(crate) fn validate_upstream(raw: &str) -> VersionResult<()> {
    parse_upstream(raw).map(|_| ())
}

pub(crate) fn compare_upstream(left: &str, right: &str) -> VersionResult<Ordering> {
    Ok(parse_upstream(left)?.cmp(&parse_upstream(right)?))
}

pub(super) fn validate(raw: &str) -> VersionResult<()> {
    parse(raw).map(|_| ())
}

pub(super) fn compare(left: &str, right: &str) -> VersionResult<Ordering> {
    let left = parse(left)?;
    let right = parse(right)?;
    // eopkg's upgrade authority compares release numbers; version is a
    // deterministic tiebreak only for malformed duplicate-release corpora.
    match left.release.cmp(&right.release) {
        Ordering::Equal => compare_upstream(&left.version, &right.version),
        ordering => Ok(ordering),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_order_matches_pisi_contract() {
        let ordered = [
            "1_alpha1", "1_beta1", "1_pre1", "1_rc1", "1_m1", "1", "1_p1",
        ];
        for pair in ordered.windows(2) {
            assert_eq!(compare_upstream(pair[0], pair[1]).unwrap(), Ordering::Less);
        }
    }

    #[test]
    fn base_and_numeric_suffix_items_are_lexicographic() {
        assert_eq!(compare_upstream("1.2", "1.10").unwrap(), Ordering::Less);
        assert_eq!(compare_upstream("1.2a", "1.2b").unwrap(), Ordering::Less);
        assert_eq!(compare_upstream("1_1.2", "1_1.10").unwrap(), Ordering::Less);
    }

    #[test]
    fn invalid_items_fail_closed() {
        for value in ["", ".1", "1.", "one", "1_alpha", "1_1_two"] {
            assert!(validate_upstream(value).is_err(), "{value}");
        }
    }

    #[test]
    fn upstream_unrecognized_alphabetic_suffix_behavior_is_preserved() {
        assert_eq!(compare_upstream("1_gamma1", "1").unwrap(), Ordering::Equal);
    }

    #[test]
    fn package_order_is_release_authority() {
        assert_eq!(compare("99-12", "1-13").unwrap(), Ordering::Less);
        assert_eq!(parse("1.8.2-13").unwrap().release, 13);
    }
}
