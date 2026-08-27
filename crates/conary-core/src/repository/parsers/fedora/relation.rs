// conary-core/src/repository/parsers/fedora/relation.rs

//! Exact RPM repository relation and provide grammar.
//!
//! RPM applies the same optional `<operator> <EVR>` grammar to `Provides` as
//! to the other dependency tags:
//! <https://rpm.org/docs/latest/manual/spec.html#dependencies>.
//! Do not narrow provides to equality here. Provider/requirement matching must
//! compare the resulting ranges for overlap, as upstream `rpmdsCompare()` does:
//! <https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmds.cc#L858-L901>.
//! The boundary-overlap rules live in upstream `rpmverOverlap()`:
//! <https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/rpmio/rpmver.cc#L59-L106>.

use crate::error::{Error, Result};
use crate::repository::dependency_model::{ProvideVersionRelation, RepositoryRequirementGroup};
use crate::repository::versioning::{RepoVersionConstraint, VersionScheme, parse_repo_constraint};
use rpm::DependencyFlags;

#[derive(Debug)]
pub(super) struct RpmProvideConstraint {
    pub(super) native_constraint: String,
    pub(super) version: Option<String>,
    pub(super) version_relation: Option<ProvideVersionRelation>,
}

/// Convert RPM primary-metadata flags to their exact native operator.
fn rpm_flags_to_op(flags: &str) -> Option<&'static str> {
    match flags {
        "GE" => Some(">="),
        "LE" => Some("<="),
        "EQ" => Some("="),
        "LT" => Some("<"),
        "GT" => Some(">"),
        _ => None,
    }
}

fn rpm_flags_to_dependency_flags(name: &str, flags: Option<&str>) -> Result<DependencyFlags> {
    match flags {
        None => Ok(DependencyFlags::empty()),
        Some("GE") => Ok(DependencyFlags::GREATER | DependencyFlags::EQUAL),
        Some("LE") => Ok(DependencyFlags::LESS | DependencyFlags::EQUAL),
        Some("EQ") => Ok(DependencyFlags::EQUAL),
        Some("LT") => Ok(DependencyFlags::LESS),
        Some("GT") => Ok(DependencyFlags::GREATER),
        Some(flags) => Err(Error::ParseError(format!(
            "unsupported RPM relation flags '{flags}' for {name}"
        ))),
    }
}

fn rpm_pre_to_dependency_flags(name: &str, pre: Option<&str>) -> Result<DependencyFlags> {
    match pre {
        None => Ok(DependencyFlags::empty()),
        Some("1") => Ok(DependencyFlags::PREREQ),
        Some(value) => Err(Error::ParseError(format!(
            "unsupported RPM pre marker '{value}' for {name}; createrepo_c emits exactly '1'"
        ))),
    }
}

fn rpm_evr_native_text(
    name: &str,
    flags: Option<&str>,
    epoch: Option<&str>,
    version: Option<&str>,
    release: Option<&str>,
) -> Result<String> {
    match (flags, version) {
        (None, None) if epoch.is_none() && release.is_none() => Ok(String::new()),
        (Some(_), Some(version)) => {
            let mut evr = String::new();
            if let Some(epoch) = epoch
                && epoch != "0"
            {
                evr.push_str(epoch);
                evr.push(':');
            }
            evr.push_str(version);
            if let Some(release) = release
                && !release.is_empty()
            {
                evr.push('-');
                evr.push_str(release);
            }
            Ok(evr)
        }
        _ => Err(Error::ParseError(format!(
            "malformed RPM relation entry for {name}: comparison flags and version must appear together"
        ))),
    }
}

pub(super) fn rpm_relation_native_text(
    name: &str,
    flags: Option<&str>,
    epoch: Option<&str>,
    version: Option<&str>,
    release: Option<&str>,
) -> Result<String> {
    let constraint = rpm_constraint_native_text(name, flags, epoch, version, release)?;
    if constraint.is_empty() {
        Ok(name.to_string())
    } else {
        Ok(format!("{name} {constraint}"))
    }
}

pub(super) fn rpm_constraint_native_text(
    name: &str,
    flags: Option<&str>,
    epoch: Option<&str>,
    version: Option<&str>,
    release: Option<&str>,
) -> Result<String> {
    let Some(flags) = flags else {
        return rpm_evr_native_text(name, None, epoch, version, release);
    };
    let operator = rpm_flags_to_op(flags).ok_or_else(|| {
        Error::ParseError(format!(
            "unsupported RPM relation flags '{flags}' for {name}"
        ))
    })?;
    let evr = rpm_evr_native_text(name, Some(flags), epoch, version, release)?;
    Ok(format!(
        "{operator} {}",
        crate::repository::rpm_dependency::canonicalize_source_rpm_evr(&evr)
            .map_err(Error::ParseError)?
    ))
}

pub(super) fn rpm_provide_constraint(
    name: &str,
    flags: Option<&str>,
    epoch: Option<&str>,
    version: Option<&str>,
    release: Option<&str>,
) -> Result<RpmProvideConstraint> {
    let native_constraint = rpm_constraint_native_text(name, flags, epoch, version, release)?;
    let (version_relation, version) =
        match parse_repo_constraint(VersionScheme::Rpm, &native_constraint).map_err(|error| {
            Error::ParseError(format!(
                "invalid RPM provide '{name} {native_constraint}': {error}"
            ))
        })? {
            RepoVersionConstraint::Any => (None, None),
            RepoVersionConstraint::Exact(version) => {
                (Some(ProvideVersionRelation::Equal), Some(version))
            }
            RepoVersionConstraint::GreaterThan(version) => {
                (Some(ProvideVersionRelation::GreaterThan), Some(version))
            }
            RepoVersionConstraint::GreaterOrEqual(version) => {
                (Some(ProvideVersionRelation::GreaterOrEqual), Some(version))
            }
            RepoVersionConstraint::LessThan(version) => {
                (Some(ProvideVersionRelation::LessThan), Some(version))
            }
            RepoVersionConstraint::LessOrEqual(version) => {
                (Some(ProvideVersionRelation::LessOrEqual), Some(version))
            }
            RepoVersionConstraint::NotEqual(_) => {
                return Err(Error::ParseError(format!(
                    "RPM provide '{name}' uses the unsupported '!=' relation"
                )));
            }
            RepoVersionConstraint::Eopkg(_) => unreachable!("RPM parser selected RPM scheme"),
        };
    Ok(RpmProvideConstraint {
        native_constraint,
        version,
        version_relation,
    })
}

/// Build a typed requirement group from one exact RPM primary-metadata entry.
pub(super) fn rpm_require_to_group(
    name: &str,
    flags: Option<&str>,
    epoch: Option<&str>,
    version: Option<&str>,
    release: Option<&str>,
    pre: Option<&str>,
) -> Result<Option<RepositoryRequirementGroup>> {
    let dependency_flags =
        rpm_flags_to_dependency_flags(name, flags)? | rpm_pre_to_dependency_flags(name, pre)?;
    let evr = rpm_evr_native_text(name, flags, epoch, version, release)?;
    crate::packages::rpm::decode_rpm_requirement(name, &evr, dependency_flags)
}

/// Build one exact RPM weak relation from primary metadata.
pub(super) fn rpm_weak_require_to_group(
    kind: crate::repository::dependency_model::RepositoryRequirementKind,
    name: &str,
    flags: Option<&str>,
    epoch: Option<&str>,
    version: Option<&str>,
    release: Option<&str>,
) -> Result<RepositoryRequirementGroup> {
    if !matches!(
        kind,
        crate::repository::dependency_model::RepositoryRequirementKind::Recommends
            | crate::repository::dependency_model::RepositoryRequirementKind::Suggests
            | crate::repository::dependency_model::RepositoryRequirementKind::Supplements
            | crate::repository::dependency_model::RepositoryRequirementKind::Enhances
    ) {
        return Err(Error::InternalError(format!(
            "{kind:?} is not an RPM weak relation"
        )));
    }
    let native_text = rpm_relation_native_text(name, flags, epoch, version, release)?;
    crate::repository::requirement::parse_source_rpm_requirement(kind, &native_text)
        .map_err(Error::ParseError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relation_preserves_epoch_version_and_release() {
        assert_eq!(
            rpm_constraint_native_text(
                "openssl-libs",
                Some("GE"),
                Some("2"),
                Some("3.2.0"),
                Some("4.fc44"),
            )
            .unwrap(),
            ">= 2:3.2.0-4.fc44"
        );
    }

    #[test]
    fn requirement_canonicalizes_zero_epochs_through_shared_rpm_decoder() {
        for epoch in ["", "0"] {
            let requirement = rpm_require_to_group(
                "device-mapper-libs",
                Some("EQ"),
                Some(epoch),
                Some("1.02.212"),
                Some("2.fc44"),
                None,
            )
            .unwrap()
            .unwrap();

            assert_eq!(
                requirement.alternatives[0].version_constraint.as_deref(),
                Some("= 1.02.212-2.fc44")
            );
            assert_eq!(
                requirement.native_text.as_deref(),
                Some("device-mapper-libs = 1.02.212-2.fc44")
            );
        }
    }

    #[test]
    fn requirement_rejects_non_decimal_epoch_through_shared_rpm_decoder() {
        let error = rpm_require_to_group(
            "device-mapper-libs",
            Some("EQ"),
            Some("invalid"),
            Some("1.02.212"),
            Some("2.fc44"),
            None,
        )
        .unwrap_err();

        assert!(error.to_string().contains("epoch"));
    }

    #[test]
    fn requirement_preserves_createrepo_prerequisite_marker() {
        let requirement = rpm_require_to_group("setup", None, None, None, None, Some("1"))
            .unwrap()
            .unwrap();

        assert_eq!(
            requirement.kind,
            crate::repository::dependency_model::RepositoryRequirementKind::PreDepends
        );
    }

    #[test]
    fn requirement_rejects_noncanonical_prerequisite_marker() {
        let error =
            rpm_require_to_group("setup", None, None, None, None, Some("true")).unwrap_err();

        assert!(error.to_string().contains("pre marker"));
    }

    #[test]
    fn provide_preserves_every_upstream_rpm_range_operator() {
        for (flags, expected_text, expected_relation) in [
            ("EQ", "= 2-1", ProvideVersionRelation::Equal),
            ("GE", ">= 2-1", ProvideVersionRelation::GreaterOrEqual),
            ("LE", "<= 2-1", ProvideVersionRelation::LessOrEqual),
            ("GT", "> 2-1", ProvideVersionRelation::GreaterThan),
            ("LT", "< 2-1", ProvideVersionRelation::LessThan),
        ] {
            let parsed =
                rpm_provide_constraint("mail-api", Some(flags), Some("0"), Some("2"), Some("1"))
                    .unwrap();
            assert_eq!(parsed.native_constraint, expected_text);
            assert_eq!(parsed.version.as_deref(), Some("2-1"));
            assert_eq!(parsed.version_relation, Some(expected_relation));
        }
    }

    #[test]
    fn unversioned_provide_is_an_existence_range() {
        let parsed = rpm_provide_constraint("mail-api", None, None, None, None).unwrap();
        assert!(parsed.native_constraint.is_empty());
        assert_eq!(parsed.version, None);
        assert_eq!(parsed.version_relation, None);
    }
}
