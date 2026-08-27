// conary-core/src/repository/requirement.rs

//! Validation for the single typed package-requirement authority.

use super::dependency_model::{
    ConditionalRequirementBehavior, RepositoryRequirementExpression, RepositoryRequirementGroup,
    RepositoryRequirementKind, RequirementArchitectureQualifier,
};
use super::package_relation::validate_native_relation;
use super::versioning::{VersionScheme, parse_repo_constraint};

/// Parse one source-native positive requirement entry.
pub fn parse_native_requirement(
    kind: RepositoryRequirementKind,
    scheme: VersionScheme,
    native_text: &str,
) -> Result<RepositoryRequirementGroup, String> {
    if kind.is_negative_relation() {
        return Err(format!("{kind:?} is not a positive package requirement"));
    }
    let native_text = native_text.trim();
    if native_text.is_empty() {
        return Err("package requirement is empty".to_string());
    }

    let expression = match scheme {
        VersionScheme::Rpm => super::rpm_dependency::parse_rpm_dependency(kind, native_text)?,
        VersionScheme::Debian => {
            let operands = native_text
                .split('|')
                .map(str::trim)
                .map(super::package_relation::parse_debian_atom)
                .collect::<Result<Vec<_>, _>>()?;
            if operands.len() == 1 {
                RepositoryRequirementExpression::Atom(operands.into_iter().next().unwrap())
            } else {
                RepositoryRequirementExpression::Or(
                    operands
                        .into_iter()
                        .map(RepositoryRequirementExpression::Atom)
                        .collect(),
                )
            }
        }
        VersionScheme::Arch => {
            RepositoryRequirementExpression::Atom(if kind == RepositoryRequirementKind::Depends {
                super::package_relation::parse_arch_runtime_atom(native_text)?
            } else {
                super::package_relation::parse_arch_atom(native_text)?
            })
        }
        VersionScheme::Conary | VersionScheme::Eopkg => RepositoryRequirementExpression::Atom(
            super::package_relation::parse_inline_atom(native_text, scheme)?,
        ),
    };
    build_positive_requirement(kind, scheme, native_text, expression)
}

/// Parse an authenticated RPM source requirement before it enters canonical
/// repository authority. This is the sole boundary that accepts RPM's empty
/// serialized epoch spelling.
pub(crate) fn parse_source_rpm_requirement(
    kind: RepositoryRequirementKind,
    native_text: &str,
) -> Result<RepositoryRequirementGroup, String> {
    if kind.is_negative_relation() {
        return Err(format!("{kind:?} is not a positive package requirement"));
    }
    let native_text = native_text.trim();
    if native_text.is_empty() {
        return Err("package requirement is empty".to_string());
    }
    let expression = super::rpm_dependency::parse_source_rpm_dependency(kind, native_text)?;
    build_positive_requirement(kind, VersionScheme::Rpm, native_text, expression)
}

fn build_positive_requirement(
    kind: RepositoryRequirementKind,
    scheme: VersionScheme,
    native_text: &str,
    expression: RepositoryRequirementExpression,
) -> Result<RepositoryRequirementGroup, String> {
    let alternatives = expression.atoms().into_iter().cloned().collect::<Vec<_>>();
    let first = alternatives
        .first()
        .cloned()
        .ok_or_else(|| format!("package requirement produced no atoms: {native_text}"))?;
    let mut group = RepositoryRequirementGroup::simple(kind, first)
        .with_behavior(if expression.is_conditional() {
            ConditionalRequirementBehavior::Conditional
        } else {
            ConditionalRequirementBehavior::Hard
        })
        .with_expression(expression)
        .with_native_text(native_text.to_string());
    group.alternatives = alternatives;
    validate_requirement_group(&group, scheme)?;
    Ok(group)
}

/// Validate one exact positive or negative package requirement group.
///
/// `alternatives` is an index of the expression's atoms, never a second
/// authority. The expression and its source version scheme own all resolution
/// semantics.
pub fn validate_requirement_group(
    group: &RepositoryRequirementGroup,
    scheme: VersionScheme,
) -> Result<(), String> {
    if group.kind.is_negative_relation() {
        return validate_native_relation(group, scheme);
    }

    let atoms = group.expression.atoms();
    if atoms.is_empty() {
        return Err("package requirement expression has no atoms".to_string());
    }
    if atoms.len() != group.alternatives.len()
        || atoms
            .iter()
            .zip(&group.alternatives)
            .any(|(expression, indexed)| *expression != indexed)
    {
        return Err(
            "package requirement clause index disagrees with its authoritative expression"
                .to_string(),
        );
    }
    let expected_behavior = if group.expression.is_conditional() {
        ConditionalRequirementBehavior::Conditional
    } else {
        ConditionalRequirementBehavior::Hard
    };
    if group.behavior != expected_behavior {
        return Err(
            "package requirement behavior disagrees with its authoritative expression".to_string(),
        );
    }
    validate_positive_expression_shape(&group.expression, scheme)?;
    for clause in atoms {
        if clause.name.is_empty() || clause.name.chars().any(char::is_whitespace) {
            return Err(format!(
                "package requirement has invalid capability name '{}'",
                clause.name
            ));
        }
        if let Some(raw) = clause.version_constraint.as_deref() {
            parse_repo_constraint(scheme, raw).map_err(|error| {
                format!(
                    "package requirement '{}' has invalid {} constraint '{}': {error}",
                    clause.name,
                    scheme.as_str(),
                    raw
                )
            })?;
        }
        match (&clause.architecture_qualifier, scheme) {
            (RequirementArchitectureQualifier::Unqualified, _) => {}
            (
                RequirementArchitectureQualifier::Any
                | RequirementArchitectureQualifier::Native
                | RequirementArchitectureQualifier::Exact(_),
                VersionScheme::Debian,
            ) => {}
            (qualifier, _) => {
                return Err(format!(
                    "package requirement '{}' uses Debian architecture qualifier {qualifier:?} with {} versioning",
                    clause.name,
                    scheme.as_str()
                ));
            }
        }
    }
    Ok(())
}

fn validate_positive_expression_shape(
    expression: &RepositoryRequirementExpression,
    scheme: VersionScheme,
) -> Result<(), String> {
    match scheme {
        VersionScheme::Rpm | VersionScheme::Conary | VersionScheme::Eopkg => Ok(()),
        VersionScheme::Debian => {
            let valid = match expression {
                RepositoryRequirementExpression::Atom(_) => true,
                RepositoryRequirementExpression::Or(operands) => operands
                    .iter()
                    .all(|operand| matches!(operand, RepositoryRequirementExpression::Atom(_))),
                _ => false,
            };
            if valid {
                Ok(())
            } else {
                Err(
                    "Debian positive requirements permit only one atom or a flat alternative list"
                        .to_string(),
                )
            }
        }
        VersionScheme::Arch => {
            if matches!(expression, RepositoryRequirementExpression::Atom(_)) {
                Ok(())
            } else {
                Err("Arch positive requirements require one exact relation atom".to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::dependency_model::{
        RepositoryRequirementClause, RepositoryRequirementKind,
    };
    use crate::repository::rpm_dependency::parse_rpm_dependency;

    #[test]
    fn accepts_full_rpm_boolean_requirement() {
        let expression = parse_rpm_dependency(
            RepositoryRequirementKind::Depends,
            "((feature-a with feature-b) if engine else (fallback without incompatible))",
        )
        .unwrap();
        let alternatives = expression.atoms().into_iter().cloned().collect::<Vec<_>>();
        let mut group = RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            alternatives[0].clone(),
        )
        .with_behavior(ConditionalRequirementBehavior::Conditional)
        .with_expression(expression);
        group.alternatives = alternatives;

        validate_requirement_group(&group, VersionScheme::Rpm).unwrap();
    }

    #[test]
    fn source_rpm_empty_epochs_canonicalize_inside_nested_rich_atoms() {
        let exact = "(nvidia-query-resource-opengl-libs(x86-32) = :1.0.0-23.fc44 if libGL(x86-32))";
        let requirement =
            parse_source_rpm_requirement(RepositoryRequirementKind::Suggests, exact).unwrap();
        assert_eq!(requirement.native_text.as_deref(), Some(exact));
        assert_eq!(
            requirement
                .alternatives
                .iter()
                .map(|clause| (clause.name.as_str(), clause.version_constraint.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "nvidia-query-resource-opengl-libs(x86-32)",
                    Some("= 1.0.0-23.fc44")
                ),
                ("libGL(x86-32)", None),
            ]
        );
        assert!(matches!(
            requirement.expression,
            RepositoryRequirementExpression::If { .. }
        ));

        let nested = parse_source_rpm_requirement(
            RepositoryRequirementKind::Depends,
            "((a = :1-1 and b = 0:1-1) if c = 2:1-1)",
        )
        .unwrap();
        assert_eq!(
            nested
                .alternatives
                .iter()
                .map(|clause| clause.version_constraint.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("= 1-1"), Some("= 0:1-1"), Some("= 2:1-1")]
        );
    }

    #[test]
    fn source_rpm_epoch_decoding_keeps_canonical_and_source_grammars_separate() {
        for malformed in [
            "pkg = :",
            "pkg = ::1",
            "pkg = :1:2",
            "pkg = nondecimal:1",
            "(pkg = :1 if)",
        ] {
            assert!(
                parse_source_rpm_requirement(RepositoryRequirementKind::Depends, malformed)
                    .is_err(),
                "{malformed} must fail closed"
            );
        }
        for canonical_input in ["pkg = :1", "(pkg = :1 if condition)"] {
            assert!(
                parse_native_requirement(
                    RepositoryRequirementKind::Depends,
                    VersionScheme::Rpm,
                    canonical_input,
                )
                .is_err(),
                "canonical input must reject RPM source spelling: {canonical_input}"
            );
        }

        let omitted = super::parse_repo_constraint(VersionScheme::Rpm, "= 1-1").unwrap();
        let explicit_zero = super::parse_repo_constraint(VersionScheme::Rpm, "= 0:1-1").unwrap();
        for candidate in ["1-1", "0:1-1"] {
            assert!(
                crate::repository::versioning::repo_version_satisfies(
                    VersionScheme::Rpm,
                    candidate,
                    &omitted,
                )
                .unwrap()
            );
            assert!(
                crate::repository::versioning::repo_version_satisfies(
                    VersionScheme::Rpm,
                    candidate,
                    &explicit_zero,
                )
                .unwrap()
            );
        }
    }

    #[test]
    fn rejects_clause_index_drift() {
        let mut group = RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            RepositoryRequirementClause::name_only("runtime".to_string()),
        );
        group.alternatives[0].name = "different".to_string();

        assert!(
            validate_requirement_group(&group, VersionScheme::Rpm)
                .unwrap_err()
                .contains("clause index")
        );
    }
}
