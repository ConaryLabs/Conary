// conary-core/src/repository/rpm_runtime.rs

//! Typed RPM package-format feature requirements.
//!
//! RPM packages declare format/runtime requirements as versioned
//! `rpmlib(Feature)` capabilities. They are satisfied by the package manager
//! implementation, not by an installable package. The upstream feature table
//! and its provided EVRs are pinned here:
//! <https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmds.cc#L954-L1063>.

use thiserror::Error;

use super::dependency_model::{
    ConditionalRequirementBehavior, RepositoryRequirementClause, RepositoryRequirementExpression,
    RepositoryRequirementGroup, RequirementArchitectureQualifier,
};
use super::versioning::{
    RepoVersionConstraint, VersionComparisonError, VersionScheme, parse_repo_constraint,
    repo_version_satisfies,
};

/// One feature in RPM's reserved `rpmlib(...)` requirement namespace.
///
/// Most variants come from RPM's `rpmlibProvides` ABI table.
/// `ShortCircuited` is deliberately absent from that table: RPM adds it to
/// binary artifacts produced without running a build phase so every package
/// manager rejects them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RpmRuntimeFeature {
    VersionedDependencies,
    CompressedFileNames,
    PayloadIsBzip2,
    PayloadIsXz,
    PayloadIsLzma,
    PayloadFilesHavePrefix,
    ExplicitPackageProvide,
    HeaderLoadSortsTags,
    ScriptletInterpreterArgs,
    PartialHardlinkSets,
    ConcurrentAccess,
    BuiltinLuaScripts,
    FileDigests,
    FileCaps,
    ScriptletExpansion,
    ShortCircuited,
    TildeInVersions,
    CaretInVersions,
    LargeFiles,
    RichDependencies,
    DynamicBuildRequires,
    PayloadIsZstd,
}

impl RpmRuntimeFeature {
    /// Parse the exact `rpmlib(Feature)` grammar.
    ///
    /// A non-`rpmlib` capability returns `None`; an unknown name inside the
    /// reserved namespace is an explicit ABI error.
    pub fn parse_capability(capability: &str) -> RpmRuntimeResult<Option<Self>> {
        let Some(feature) = capability.strip_prefix("rpmlib(") else {
            return Ok(None);
        };
        let feature =
            feature
                .strip_suffix(')')
                .ok_or_else(|| RpmRuntimeError::MalformedCapability {
                    capability: capability.to_string(),
                })?;
        let parsed = match feature {
            "VersionedDependencies" => Self::VersionedDependencies,
            "CompressedFileNames" => Self::CompressedFileNames,
            "PayloadIsBzip2" => Self::PayloadIsBzip2,
            "PayloadIsXz" => Self::PayloadIsXz,
            "PayloadIsLzma" => Self::PayloadIsLzma,
            "PayloadFilesHavePrefix" => Self::PayloadFilesHavePrefix,
            "ExplicitPackageProvide" => Self::ExplicitPackageProvide,
            "HeaderLoadSortsTags" => Self::HeaderLoadSortsTags,
            "ScriptletInterpreterArgs" => Self::ScriptletInterpreterArgs,
            "PartialHardlinkSets" => Self::PartialHardlinkSets,
            "ConcurrentAccess" => Self::ConcurrentAccess,
            "BuiltinLuaScripts" => Self::BuiltinLuaScripts,
            "FileDigests" => Self::FileDigests,
            "FileCaps" => Self::FileCaps,
            "ScriptletExpansion" => Self::ScriptletExpansion,
            "ShortCircuited" => Self::ShortCircuited,
            "TildeInVersions" => Self::TildeInVersions,
            "CaretInVersions" => Self::CaretInVersions,
            "LargeFiles" => Self::LargeFiles,
            "RichDependencies" => Self::RichDependencies,
            "DynamicBuildRequires" => Self::DynamicBuildRequires,
            "PayloadIsZstd" => Self::PayloadIsZstd,
            _ => {
                return Err(RpmRuntimeError::UnknownFeature {
                    capability: capability.to_string(),
                });
            }
        };
        Ok(Some(parsed))
    }

    #[must_use]
    pub const fn capability(self) -> &'static str {
        match self {
            Self::VersionedDependencies => "rpmlib(VersionedDependencies)",
            Self::CompressedFileNames => "rpmlib(CompressedFileNames)",
            Self::PayloadIsBzip2 => "rpmlib(PayloadIsBzip2)",
            Self::PayloadIsXz => "rpmlib(PayloadIsXz)",
            Self::PayloadIsLzma => "rpmlib(PayloadIsLzma)",
            Self::PayloadFilesHavePrefix => "rpmlib(PayloadFilesHavePrefix)",
            Self::ExplicitPackageProvide => "rpmlib(ExplicitPackageProvide)",
            Self::HeaderLoadSortsTags => "rpmlib(HeaderLoadSortsTags)",
            Self::ScriptletInterpreterArgs => "rpmlib(ScriptletInterpreterArgs)",
            Self::PartialHardlinkSets => "rpmlib(PartialHardlinkSets)",
            Self::ConcurrentAccess => "rpmlib(ConcurrentAccess)",
            Self::BuiltinLuaScripts => "rpmlib(BuiltinLuaScripts)",
            Self::FileDigests => "rpmlib(FileDigests)",
            Self::FileCaps => "rpmlib(FileCaps)",
            Self::ScriptletExpansion => "rpmlib(ScriptletExpansion)",
            Self::ShortCircuited => "rpmlib(ShortCircuited)",
            Self::TildeInVersions => "rpmlib(TildeInVersions)",
            Self::CaretInVersions => "rpmlib(CaretInVersions)",
            Self::LargeFiles => "rpmlib(LargeFiles)",
            Self::RichDependencies => "rpmlib(RichDependencies)",
            Self::DynamicBuildRequires => "rpmlib(DynamicBuildRequires)",
            Self::PayloadIsZstd => "rpmlib(PayloadIsZstd)",
        }
    }

    /// Exact feature EVR Conary currently implements.
    ///
    /// `None` is a known upstream feature whose semantics are not yet
    /// implemented. Keeping it typed makes the failure actionable without
    /// pretending an ordinary package could satisfy it.
    #[must_use]
    pub const fn supported_evr(self) -> Option<&'static str> {
        match self {
            Self::VersionedDependencies => Some("3.0.3-1"),
            Self::CompressedFileNames => Some("3.0.4-1"),
            Self::PayloadIsBzip2 => Some("3.0.5-1"),
            Self::PayloadIsXz => Some("5.2-1"),
            Self::PayloadIsLzma => Some("4.4.2-1"),
            Self::PayloadFilesHavePrefix => Some("4.0-1"),
            Self::ExplicitPackageProvide => Some("4.0-1"),
            Self::HeaderLoadSortsTags => Some("4.0.1-1"),
            Self::ScriptletInterpreterArgs => Some("4.0.3-1"),
            Self::PartialHardlinkSets => Some("4.0.4-1"),
            Self::ConcurrentAccess => None,
            Self::BuiltinLuaScripts => Some("4.2.2-1"),
            Self::FileDigests => Some("4.6.0-1"),
            Self::FileCaps => Some("4.6.1-1"),
            Self::ScriptletExpansion => Some("4.9.0-1"),
            // This is an invalid-artifact marker, not a runtime feature RPM
            // provides. `ensure_supported` rejects it before this table.
            Self::ShortCircuited => None,
            Self::TildeInVersions => Some("4.10.0-1"),
            Self::CaretInVersions => Some("4.15.0-1"),
            Self::LargeFiles => Some("4.12.0-1"),
            Self::RichDependencies => Some("4.12.0-1"),
            // Upstream adds this MISSINGOK requirement only to source RPMs
            // carrying %generate_buildrequires. It has no binary-install
            // behavior for Conary to execute.
            Self::DynamicBuildRequires => Some("4.15.0-1"),
            Self::PayloadIsZstd => Some("5.4.18-1"),
        }
    }

    /// Exact missing implementation boundary for a known unsupported feature.
    #[must_use]
    pub const fn unsupported_reason(self) -> Option<&'static str> {
        match self {
            Self::ConcurrentAccess => Some(
                "transaction scriptlets cannot query a source RPM database during Conary mutation",
            ),
            _ => None,
        }
    }

    /// Explain an RPM build marker that must never be treated as an
    /// implementable package-manager capability.
    #[must_use]
    pub const fn invalid_artifact_reason(self) -> Option<&'static str> {
        match self {
            Self::ShortCircuited => Some(
                "the binary artifact was packaged without completing an RPM build phase and RPM intentionally makes it non-installable",
            ),
            _ => None,
        }
    }
}

/// One exact package-manager feature requirement from RPM metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RpmRuntimeRequirement {
    pub feature: RpmRuntimeFeature,
    pub constraint: RepoVersionConstraint,
    pub native_constraint: Option<String>,
}

impl RpmRuntimeRequirement {
    /// Retype one RPM requirement clause when it belongs to the reserved
    /// `rpmlib(...)` namespace.
    pub fn from_clause(
        clause: &RepositoryRequirementClause,
        scheme: VersionScheme,
    ) -> RpmRuntimeResult<Option<Self>> {
        if scheme != VersionScheme::Rpm {
            return Ok(None);
        }
        let Some(feature) = RpmRuntimeFeature::parse_capability(&clause.name)? else {
            return Ok(None);
        };
        if clause.capability_kind.is_some()
            || clause.architecture_qualifier != RequirementArchitectureQualifier::Unqualified
        {
            return Err(RpmRuntimeError::QualifiedRuntimeCapability {
                capability: clause.name.clone(),
            });
        }
        let constraint = clause
            .version_constraint
            .as_deref()
            .map(|raw| parse_repo_constraint(VersionScheme::Rpm, raw))
            .transpose()?
            .unwrap_or(RepoVersionConstraint::Any);
        Ok(Some(Self {
            feature,
            constraint,
            native_constraint: clause.version_constraint.clone(),
        }))
    }

    /// Assert that Conary implements this feature at an EVR satisfying the
    /// package's exact RPM constraint.
    pub fn ensure_supported(&self) -> RpmRuntimeResult<()> {
        if let Some(reason) = self.feature.invalid_artifact_reason() {
            return Err(RpmRuntimeError::InvalidArtifactFeature {
                capability: self.feature.capability().to_string(),
                required: self
                    .native_constraint
                    .clone()
                    .unwrap_or_else(|| "<any>".to_string()),
                reason: reason.to_string(),
            });
        }
        let supported =
            self.feature
                .supported_evr()
                .ok_or_else(|| RpmRuntimeError::UnsupportedFeature {
                    capability: self.feature.capability().to_string(),
                    required: self
                        .native_constraint
                        .clone()
                        .unwrap_or_else(|| "<any>".to_string()),
                    missing: self
                        .feature
                        .unsupported_reason()
                        .unwrap_or("the feature has no declared Conary implementation")
                        .to_string(),
                })?;
        if repo_version_satisfies(VersionScheme::Rpm, supported, &self.constraint)? {
            return Ok(());
        }
        Err(RpmRuntimeError::UnsupportedVersion {
            capability: self.feature.capability().to_string(),
            required: self
                .native_constraint
                .clone()
                .unwrap_or_else(|| "<any>".to_string()),
            supported: supported.to_string(),
        })
    }
}

/// Result of replacing package-manager runtime atoms with the Boolean value
/// `true` after validating the exact requested feature EVR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimplifiedRuntimeExpression {
    /// The package-manager runtime alone satisfies the complete expression.
    Satisfied,
    /// Package SAT must evaluate the remaining source-native expression.
    Package(RepositoryRequirementExpression),
}

/// Validate and remove every typed RPM runtime atom from a native Boolean
/// expression without changing the expression's truth value.
///
/// RPM's `with` and `without` operators require one package provider. A
/// package-manager capability cannot participate in that provider identity,
/// so such a combination is rejected instead of being coerced into ordinary
/// Boolean or package-provider semantics.
pub fn simplify_runtime_requirements(
    expression: &RepositoryRequirementExpression,
    scheme: VersionScheme,
) -> RpmRuntimeResult<SimplifiedRuntimeExpression> {
    if scheme != VersionScheme::Rpm {
        return Ok(SimplifiedRuntimeExpression::Package(expression.clone()));
    }
    match reduce_runtime_expression(expression, scheme)? {
        ReducedRuntimeExpression::True => Ok(SimplifiedRuntimeExpression::Satisfied),
        ReducedRuntimeExpression::Package(expression) => {
            Ok(SimplifiedRuntimeExpression::Package(expression))
        }
    }
}

/// Apply runtime simplification to one persisted requirement-group shape.
///
/// `None` means the package-manager runtime satisfies the whole group, so no
/// package dependency may be stored or sent to SAT.
pub fn simplify_runtime_requirement_group(
    mut group: RepositoryRequirementGroup,
    scheme: VersionScheme,
) -> RpmRuntimeResult<Option<RepositoryRequirementGroup>> {
    let expression = match simplify_runtime_requirements(&group.expression, scheme)? {
        SimplifiedRuntimeExpression::Satisfied => return Ok(None),
        SimplifiedRuntimeExpression::Package(expression) => expression,
    };
    let alternatives = expression.atoms().into_iter().cloned().collect::<Vec<_>>();
    if alternatives.is_empty() {
        return Err(RpmRuntimeError::EmptyPackageExpression);
    }
    group.behavior = if expression.is_conditional() {
        ConditionalRequirementBehavior::Conditional
    } else {
        ConditionalRequirementBehavior::Hard
    };
    group.expression = expression;
    group.alternatives = alternatives;
    Ok(Some(group))
}

/// Validate every typed RPM runtime atom while preserving the source
/// expression. This is a defensive boundary for persisted expressions whose
/// ingestion path predates runtime simplification.
pub fn validate_runtime_requirements(
    expression: &RepositoryRequirementExpression,
    scheme: VersionScheme,
) -> RpmRuntimeResult<()> {
    simplify_runtime_requirements(expression, scheme).map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReducedRuntimeExpression {
    True,
    Package(RepositoryRequirementExpression),
}

fn reduce_runtime_expression(
    expression: &RepositoryRequirementExpression,
    scheme: VersionScheme,
) -> RpmRuntimeResult<ReducedRuntimeExpression> {
    use RepositoryRequirementExpression as Expression;

    match expression {
        Expression::Atom(clause) => {
            let Some(requirement) = RpmRuntimeRequirement::from_clause(clause, scheme)? else {
                return Ok(ReducedRuntimeExpression::Package(expression.clone()));
            };
            requirement.ensure_supported()?;
            Ok(ReducedRuntimeExpression::True)
        }
        Expression::And(operands) => {
            let mut reduced = Vec::with_capacity(operands.len());
            for operand in operands {
                match reduce_runtime_expression(operand, scheme)? {
                    ReducedRuntimeExpression::True => {}
                    ReducedRuntimeExpression::Package(operand) => reduced.push(operand),
                }
            }
            if reduced.is_empty() && !operands.is_empty() {
                Ok(ReducedRuntimeExpression::True)
            } else if reduced.len() == 1 {
                Ok(ReducedRuntimeExpression::Package(
                    reduced.into_iter().next().expect("length checked"),
                ))
            } else {
                Ok(ReducedRuntimeExpression::Package(Expression::And(reduced)))
            }
        }
        Expression::Or(operands) => {
            let mut reduced = Vec::with_capacity(operands.len());
            for operand in operands {
                match reduce_runtime_expression(operand, scheme)? {
                    ReducedRuntimeExpression::True => {
                        return Ok(ReducedRuntimeExpression::True);
                    }
                    ReducedRuntimeExpression::Package(operand) => reduced.push(operand),
                }
            }
            if reduced.len() == 1 {
                Ok(ReducedRuntimeExpression::Package(
                    reduced.into_iter().next().expect("length checked"),
                ))
            } else {
                Ok(ReducedRuntimeExpression::Package(Expression::Or(reduced)))
            }
        }
        Expression::If {
            requirement,
            condition,
            otherwise,
        } => reduce_if(
            reduce_runtime_expression(requirement, scheme)?,
            reduce_runtime_expression(condition, scheme)?,
            otherwise
                .as_deref()
                .map(|otherwise| reduce_runtime_expression(otherwise, scheme))
                .transpose()?,
        ),
        Expression::Unless {
            requirement,
            condition,
            otherwise,
        } => reduce_unless(
            reduce_runtime_expression(requirement, scheme)?,
            reduce_runtime_expression(condition, scheme)?,
            otherwise
                .as_deref()
                .map(|otherwise| reduce_runtime_expression(otherwise, scheme))
                .transpose()?,
        ),
        Expression::With { .. } | Expression::Without { .. } => {
            for clause in expression.atoms() {
                if RpmRuntimeRequirement::from_clause(clause, scheme)?.is_some() {
                    return Err(RpmRuntimeError::SameProviderRuntimeCapability {
                        capability: clause.name.clone(),
                    });
                }
            }
            Ok(ReducedRuntimeExpression::Package(expression.clone()))
        }
    }
}

fn reduce_if(
    requirement: ReducedRuntimeExpression,
    condition: ReducedRuntimeExpression,
    otherwise: Option<ReducedRuntimeExpression>,
) -> RpmRuntimeResult<ReducedRuntimeExpression> {
    use ReducedRuntimeExpression as Reduced;
    use RepositoryRequirementExpression as Expression;

    if matches!(condition, Reduced::True) {
        return Ok(requirement);
    }
    let Reduced::Package(condition) = condition else {
        unreachable!("true condition returned above");
    };
    match (requirement, otherwise) {
        (Reduced::True, None | Some(Reduced::True)) => Ok(Reduced::True),
        (Reduced::True, Some(Reduced::Package(otherwise))) => {
            Ok(Reduced::Package(Expression::Unless {
                requirement: Box::new(otherwise),
                condition: Box::new(condition),
                otherwise: None,
            }))
        }
        (Reduced::Package(requirement), None | Some(Reduced::True)) => {
            Ok(Reduced::Package(Expression::If {
                requirement: Box::new(requirement),
                condition: Box::new(condition),
                otherwise: None,
            }))
        }
        (Reduced::Package(requirement), Some(Reduced::Package(otherwise))) => {
            Ok(Reduced::Package(Expression::If {
                requirement: Box::new(requirement),
                condition: Box::new(condition),
                otherwise: Some(Box::new(otherwise)),
            }))
        }
    }
}

fn reduce_unless(
    requirement: ReducedRuntimeExpression,
    condition: ReducedRuntimeExpression,
    otherwise: Option<ReducedRuntimeExpression>,
) -> RpmRuntimeResult<ReducedRuntimeExpression> {
    use ReducedRuntimeExpression as Reduced;
    use RepositoryRequirementExpression as Expression;

    if matches!(condition, Reduced::True) {
        return Ok(otherwise.unwrap_or(Reduced::True));
    }
    let Reduced::Package(condition) = condition else {
        unreachable!("true condition returned above");
    };
    match (requirement, otherwise) {
        (Reduced::True, None | Some(Reduced::True)) => Ok(Reduced::True),
        (Reduced::True, Some(Reduced::Package(otherwise))) => {
            Ok(Reduced::Package(Expression::If {
                requirement: Box::new(otherwise),
                condition: Box::new(condition),
                otherwise: None,
            }))
        }
        (Reduced::Package(requirement), None | Some(Reduced::True)) => {
            Ok(Reduced::Package(Expression::Unless {
                requirement: Box::new(requirement),
                condition: Box::new(condition),
                otherwise: None,
            }))
        }
        (Reduced::Package(requirement), Some(Reduced::Package(otherwise))) => {
            Ok(Reduced::Package(Expression::Unless {
                requirement: Box::new(requirement),
                condition: Box::new(condition),
                otherwise: Some(Box::new(otherwise)),
            }))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RpmRuntimeError {
    #[error("malformed RPM runtime capability '{capability}'")]
    MalformedCapability { capability: String },
    #[error("unknown RPM runtime feature capability '{capability}'")]
    UnknownFeature { capability: String },
    #[error(
        "RPM runtime capability '{capability}' cannot carry a package capability kind or architecture qualifier"
    )]
    QualifiedRuntimeCapability { capability: String },
    #[error(
        "Conary does not implement RPM runtime capability '{capability}' required as {required}: {missing}"
    )]
    UnsupportedFeature {
        capability: String,
        required: String,
        missing: String,
    },
    #[error("RPM artifact requires invalid-artifact marker '{capability}' as {required}: {reason}")]
    InvalidArtifactFeature {
        capability: String,
        required: String,
        reason: String,
    },
    #[error(
        "RPM runtime capability '{capability}' cannot participate in same-package 'with' or 'without' provider semantics"
    )]
    SameProviderRuntimeCapability { capability: String },
    #[error("RPM runtime simplification produced an empty package requirement expression")]
    EmptyPackageExpression,
    #[error(
        "Conary implements RPM runtime capability '{capability}' at {supported}, which does not satisfy {required}"
    )]
    UnsupportedVersion {
        capability: String,
        required: String,
        supported: String,
    },
    #[error(transparent)]
    InvalidVersion(#[from] VersionComparisonError),
}

pub type RpmRuntimeResult<T> = std::result::Result<T, RpmRuntimeError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn clause(name: &str, constraint: Option<&str>) -> RepositoryRequirementClause {
        RepositoryRequirementClause {
            name: name.to_string(),
            capability_kind: None,
            version_constraint: constraint.map(str::to_string),
            architecture_qualifier: RequirementArchitectureQualifier::Unqualified,
            native_text: None,
        }
    }

    #[test]
    fn pinned_upstream_feature_catalog_round_trips_exact_names() {
        for feature in [
            RpmRuntimeFeature::VersionedDependencies,
            RpmRuntimeFeature::CompressedFileNames,
            RpmRuntimeFeature::PayloadIsBzip2,
            RpmRuntimeFeature::PayloadIsXz,
            RpmRuntimeFeature::PayloadIsLzma,
            RpmRuntimeFeature::PayloadFilesHavePrefix,
            RpmRuntimeFeature::ExplicitPackageProvide,
            RpmRuntimeFeature::HeaderLoadSortsTags,
            RpmRuntimeFeature::ScriptletInterpreterArgs,
            RpmRuntimeFeature::PartialHardlinkSets,
            RpmRuntimeFeature::ConcurrentAccess,
            RpmRuntimeFeature::BuiltinLuaScripts,
            RpmRuntimeFeature::FileDigests,
            RpmRuntimeFeature::FileCaps,
            RpmRuntimeFeature::ScriptletExpansion,
            RpmRuntimeFeature::ShortCircuited,
            RpmRuntimeFeature::TildeInVersions,
            RpmRuntimeFeature::CaretInVersions,
            RpmRuntimeFeature::LargeFiles,
            RpmRuntimeFeature::RichDependencies,
            RpmRuntimeFeature::DynamicBuildRequires,
            RpmRuntimeFeature::PayloadIsZstd,
        ] {
            assert_eq!(
                RpmRuntimeFeature::parse_capability(feature.capability()).unwrap(),
                Some(feature)
            );
        }
    }

    #[test]
    fn ordinary_capabilities_never_enter_the_runtime_namespace() {
        assert_eq!(
            RpmRuntimeFeature::parse_capability("rpmlib-helper").unwrap(),
            None
        );
        assert_eq!(
            RpmRuntimeFeature::parse_capability("libc.so.6(GLIBC_2.34)").unwrap(),
            None
        );
    }

    #[test]
    fn supported_feature_uses_exact_rpm_version_constraint() {
        let requirement = RpmRuntimeRequirement::from_clause(
            &clause("rpmlib(CompressedFileNames)", Some("<= 3.0.4-1")),
            VersionScheme::Rpm,
        )
        .unwrap()
        .unwrap();
        requirement.ensure_supported().unwrap();

        let too_new = RpmRuntimeRequirement::from_clause(
            &clause("rpmlib(CompressedFileNames)", Some(">= 99:1-1")),
            VersionScheme::Rpm,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            too_new.ensure_supported(),
            Err(RpmRuntimeError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn known_but_unimplemented_feature_fails_explicitly() {
        let requirement = RpmRuntimeRequirement::from_clause(
            &clause("rpmlib(ConcurrentAccess)", Some("<= 4.1-1")),
            VersionScheme::Rpm,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            requirement.ensure_supported(),
            Err(RpmRuntimeError::UnsupportedFeature { .. })
        ));
    }

    #[test]
    fn large_files_uses_the_pinned_upstream_evr() {
        let requirement = RpmRuntimeRequirement::from_clause(
            &clause("rpmlib(LargeFiles)", Some("<= 4.12.0-1")),
            VersionScheme::Rpm,
        )
        .unwrap()
        .unwrap();

        requirement.ensure_supported().unwrap();
        assert_eq!(
            RpmRuntimeFeature::LargeFiles.supported_evr(),
            Some("4.12.0-1")
        );
        assert!(RpmRuntimeFeature::LargeFiles.unsupported_reason().is_none());
    }

    #[test]
    fn short_circuited_marker_is_a_typed_invalid_artifact() {
        let requirement = RpmRuntimeRequirement::from_clause(
            &clause("rpmlib(ShortCircuited)", Some("<= 4.9.0-1")),
            VersionScheme::Rpm,
        )
        .unwrap()
        .unwrap();
        let error = requirement.ensure_supported().unwrap_err();
        assert!(matches!(
            error,
            RpmRuntimeError::InvalidArtifactFeature { .. }
        ));
        assert!(
            error
                .to_string()
                .contains("packaged without completing an RPM build phase")
        );
    }

    #[test]
    fn unknown_reserved_feature_never_becomes_an_ordinary_package_dependency() {
        let error = RpmRuntimeRequirement::from_clause(
            &clause("rpmlib(FutureUnknownFeature)", Some("<= 1-1")),
            VersionScheme::Rpm,
        )
        .unwrap_err();
        assert!(matches!(error, RpmRuntimeError::UnknownFeature { .. }));
    }

    fn atom(name: &str) -> RepositoryRequirementExpression {
        RepositoryRequirementExpression::Atom(clause(name, None))
    }

    fn runtime_atom() -> RepositoryRequirementExpression {
        RepositoryRequirementExpression::Atom(clause(
            "rpmlib(CompressedFileNames)",
            Some("<= 3.0.4-1"),
        ))
    }

    #[test]
    fn runtime_atom_is_true_and_never_becomes_a_package_requirement() {
        assert_eq!(
            simplify_runtime_requirements(&runtime_atom(), VersionScheme::Rpm).unwrap(),
            SimplifiedRuntimeExpression::Satisfied
        );

        let mixed = RepositoryRequirementExpression::And(vec![atom("bash"), runtime_atom()]);
        assert_eq!(
            simplify_runtime_requirements(&mixed, VersionScheme::Rpm).unwrap(),
            SimplifiedRuntimeExpression::Package(atom("bash"))
        );
    }

    #[test]
    fn runtime_atom_short_circuits_mixed_rich_or_expression() {
        let mixed = RepositoryRequirementExpression::Or(vec![atom("missing"), runtime_atom()]);
        assert_eq!(
            simplify_runtime_requirements(&mixed, VersionScheme::Rpm).unwrap(),
            SimplifiedRuntimeExpression::Satisfied
        );
    }

    #[test]
    fn runtime_condition_preserves_if_and_unless_boolean_semantics() {
        let if_expression = RepositoryRequirementExpression::If {
            requirement: Box::new(atom("selected")),
            condition: Box::new(runtime_atom()),
            otherwise: Some(Box::new(atom("unselected"))),
        };
        assert_eq!(
            simplify_runtime_requirements(&if_expression, VersionScheme::Rpm).unwrap(),
            SimplifiedRuntimeExpression::Package(atom("selected"))
        );

        let unless_expression = RepositoryRequirementExpression::Unless {
            requirement: Box::new(atom("unselected")),
            condition: Box::new(runtime_atom()),
            otherwise: Some(Box::new(atom("selected"))),
        };
        assert_eq!(
            simplify_runtime_requirements(&unless_expression, VersionScheme::Rpm).unwrap(),
            SimplifiedRuntimeExpression::Package(atom("selected"))
        );
    }

    #[test]
    fn runtime_capability_cannot_be_coerced_into_same_provider_identity() {
        let expression = RepositoryRequirementExpression::With {
            left: Box::new(atom("bash")),
            right: Box::new(runtime_atom()),
        };
        assert!(matches!(
            simplify_runtime_requirements(&expression, VersionScheme::Rpm),
            Err(RpmRuntimeError::SameProviderRuntimeCapability { .. })
        ));
    }
}
