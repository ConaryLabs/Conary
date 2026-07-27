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
use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, ProvideVersionRelation, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use crate::repository::versioning::{RepoVersionConstraint, VersionScheme, parse_repo_constraint};

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
    match (flags, version) {
        (None, None) if epoch.is_none() && release.is_none() => Ok(String::new()),
        (Some(flags), Some(version)) => {
            let operator = rpm_flags_to_op(flags).ok_or_else(|| {
                Error::ParseError(format!(
                    "unsupported RPM relation flags '{flags}' for {name}"
                ))
            })?;
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
            Ok(format!("{operator} {evr}"))
        }
        _ => Err(Error::ParseError(format!(
            "malformed RPM relation entry for {name}: comparison flags and version must appear together"
        ))),
    }
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
    constraint: &str,
) -> Result<Option<RepositoryRequirementGroup>> {
    let native_text = if constraint.is_empty() {
        name.to_string()
    } else {
        format!("{name} {constraint}")
    };
    let expression = crate::repository::rpm_dependency::parse_rpm_dependency(
        RepositoryRequirementKind::Depends,
        &native_text,
    )
    .map_err(Error::ParseError)?;
    let clauses = expression.atoms().into_iter().cloned().collect::<Vec<_>>();
    let behavior = if expression.is_conditional() {
        ConditionalRequirementBehavior::Conditional
    } else {
        ConditionalRequirementBehavior::Hard
    };
    let first = clauses.first().cloned().ok_or_else(|| {
        Error::ParseError(format!(
            "RPM dependency produced no atomic requirements: {native_text}"
        ))
    })?;
    let mut group = RepositoryRequirementGroup::simple(RepositoryRequirementKind::Depends, first)
        .with_behavior(behavior)
        .with_expression(expression)
        .with_native_text(native_text);
    group.alternatives = clauses;
    crate::repository::rpm_runtime::simplify_runtime_requirement_group(
        group,
        crate::repository::versioning::VersionScheme::Rpm,
    )
    .map_err(|error| Error::ParseError(error.to_string()))
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
