// conary-core/src/repository/versioning/rpm_dependency.rs

//! RPM dependency EVR grammar and `rpmdsCompare` component ordering.

use std::cmp::Ordering;

/// RPM's parsed dependency boundary.
///
/// Unlike package `VERSION` and `RELEASE` fields, dependency EVRs found in
/// installed headers can retain multiple hyphens. RPM 4.20's `parseEVR()`
/// treats the final hyphen as the release separator and leaves earlier
/// hyphens in the version component:
/// https://github.com/rpm-software-management/rpm/blob/c8dc5ea575a2e9c1488036d12f4b75f6a5a49120/rpmio/rpmver.c#L24-L57
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RpmDependencyEvr<'a> {
    epoch: Option<u64>,
    version: &'a str,
    pub(super) release: Option<&'a str>,
}

pub(super) fn validate(raw: &str) -> Result<(), String> {
    parse(raw).map(|_| ())
}

pub(super) fn parse(raw: &str) -> Result<RpmDependencyEvr<'_>, String> {
    if raw.is_empty() {
        return Err("version is empty".to_string());
    }
    if raw
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("whitespace and control characters are not allowed".to_string());
    }
    if let Some(character) = raw.chars().find(|character| {
        character.is_ascii()
            && !(character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '.' | '_' | '+' | '%' | '{' | '}' | '~' | '^' | '-' | ':'
                ))
    }) {
        return Err(format!(
            "character {character:?} is outside RPM's dependency EVR grammar"
        ));
    }

    let (epoch, version_release) = match raw.split_once(':') {
        Some((epoch, rest)) => {
            if rest.contains(':') {
                return Err("multiple epoch separators are not allowed".to_string());
            }
            if epoch.is_empty() || !epoch.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("epoch must be a non-empty decimal integer".to_string());
            }
            let epoch = epoch
                .parse::<u64>()
                .map_err(|_| "epoch exceeds the supported integer range".to_string())?;
            (Some(epoch), rest)
        }
        None => (None, raw),
    };
    let (version, release) = version_release
        .rsplit_once('-')
        .map_or((version_release, None), |(version, release)| {
            (version, Some(release))
        });
    if version.is_empty() {
        return Err("version component is empty".to_string());
    }
    if release.is_some_and(str::is_empty) {
        return Err("release component is empty".to_string());
    }

    Ok(RpmDependencyEvr {
        epoch,
        version,
        release,
    })
}

pub(super) fn compare(left: &str, right: &str) -> Result<Ordering, String> {
    let left = parse(left)?;
    let right = parse(right)?;
    let epoch = match (left.epoch, right.epoch) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(left), None) if left > 0 => Ordering::Greater,
        (None, Some(right)) if right > 0 => Ordering::Less,
        _ => Ordering::Equal,
    };
    if epoch != Ordering::Equal {
        return Ok(epoch);
    }
    let version = rpm_version::rpm_evr_compare(left.version, right.version);
    if version != Ordering::Equal {
        return Ok(version);
    }
    match (left.release, right.release) {
        (Some(left), Some(right)) => Ok(rpm_version::rpm_evr_compare(left, right)),
        _ => Ok(Ordering::Equal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_evr_uses_rpm_last_hyphen_split() {
        let parsed = parse("pattern-basis-addon").unwrap();

        assert_eq!(parsed.epoch, None);
        assert_eq!(parsed.version, "pattern-basis");
        assert_eq!(parsed.release, Some("addon"));
    }

    #[test]
    fn dependency_evr_preserves_encoded_source_boundaries() {
        let cpe = parse("cpe%3A%2Fo%3Aopensuse%3Aopensuse%3A20260811").unwrap();

        assert_eq!(cpe.version, "cpe%3A%2Fo%3Aopensuse%3Aopensuse%3A20260811");
        assert_eq!(cpe.release, None);
    }

    #[test]
    fn dependency_evr_rejects_unrepresentable_boundaries() {
        for invalid in ["", " 1", "1 ", "1 2", "1-", "bad:1", "1:2:3", "1/2"] {
            assert!(parse(invalid).is_err(), "accepted {invalid:?}");
        }
    }
}
