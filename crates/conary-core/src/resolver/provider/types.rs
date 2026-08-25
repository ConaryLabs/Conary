// conary-core/src/resolver/provider/types.rs

//! Data types for the resolver provider bridge.
//!
//! Contains the solver-facing constraint and dependency types.
//! Package identity is now represented by `PackageIdentity` from
//! `resolver::identity`.

use std::collections::HashSet;
use std::fmt;

use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryRequirementExpression, RepositoryRequirementGroup,
    RequirementArchitectureQualifier,
};
use crate::repository::rpm_runtime::RpmRuntimeRequirement;
use crate::repository::versioning::{RepoVersionConstraint, VersionScheme};
use crate::version::VersionConstraint;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConaryConstraint {
    Requested(VersionConstraint),
    /// Select one exact persisted repository package as a solver root.
    ExactRepositoryPackage(i64),
    Repository {
        scheme: VersionScheme,
        constraint: RepoVersionConstraint,
        capability_kind: Option<RepositoryCapabilityKind>,
        raw: Option<String>,
        architecture_qualifier: RequirementArchitectureQualifier,
        depending_architecture: String,
    },
    /// A package-manager runtime feature. This is a Boolean constant after
    /// exact feature-EVR validation and must never be interned as a package.
    RpmRuntime(RpmRuntimeRequirement),
    ProviderExpression {
        expression: CapabilityExpression,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SolverAtom {
    pub name: String,
    pub constraint: ConaryConstraint,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SolverExpression {
    Atom(SolverAtom),
    And(Vec<SolverExpression>),
    Or(Vec<SolverExpression>),
    Not(Box<SolverExpression>),
}

impl SolverExpression {
    pub fn atom(name: String, constraint: ConaryConstraint) -> Self {
        Self::Atom(SolverAtom { name, constraint })
    }

    pub fn atoms(&self) -> Vec<&SolverAtom> {
        let mut atoms = Vec::new();
        self.collect_atoms(&mut atoms);
        atoms
    }

    fn collect_atoms<'a>(&'a self, atoms: &mut Vec<&'a SolverAtom>) {
        match self {
            Self::Atom(atom) => atoms.push(atom),
            Self::And(operands) | Self::Or(operands) => {
                for operand in operands {
                    operand.collect_atoms(atoms);
                }
            }
            Self::Not(operand) => operand.collect_atoms(atoms),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CapabilityExpression {
    Atom {
        name: String,
        capability_kind: Option<RepositoryCapabilityKind>,
        scheme: VersionScheme,
        constraint: RepoVersionConstraint,
    },
    And(Vec<CapabilityExpression>),
    Or(Vec<CapabilityExpression>),
    Not(Box<CapabilityExpression>),
}

impl CapabilityExpression {
    pub fn collect_names(&self, known: &HashSet<String>, names: &mut HashSet<String>) {
        match self {
            Self::Atom { name, .. } => {
                if !known.contains(name.as_str()) {
                    names.insert(name.clone());
                }
            }
            Self::And(operands) | Self::Or(operands) => {
                for operand in operands {
                    operand.collect_names(known, names);
                }
            }
            Self::Not(operand) => operand.collect_names(known, names),
        }
    }

    pub fn from_repository(
        expression: &RepositoryRequirementExpression,
        scheme: VersionScheme,
    ) -> Result<Self, String> {
        use RepositoryRequirementExpression as Source;

        match expression {
            Source::Atom(clause) => {
                if RpmRuntimeRequirement::from_clause(clause, scheme)
                    .map_err(|error| error.to_string())?
                    .is_some()
                {
                    return Err(format!(
                        "RPM runtime capability '{}' cannot participate in same-package provider semantics",
                        clause.name
                    ));
                }
                Ok(Self::Atom {
                    name: clause.name.clone(),
                    capability_kind: clause.capability_kind,
                    scheme,
                    constraint: match clause.version_constraint.as_deref() {
                        Some(raw) => {
                            crate::repository::versioning::parse_repo_constraint(scheme, raw)
                                .map_err(|error| {
                                    format!(
                                        "dependency '{}' has invalid {:?} constraint '{}': {error}",
                                        clause.name, scheme, raw
                                    )
                                })?
                        }
                        None => RepoVersionConstraint::Any,
                    },
                })
            }
            Source::And(operands) => Ok(Self::And(
                operands
                    .iter()
                    .map(|operand| Self::from_repository(operand, scheme))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Source::Or(operands) => Ok(Self::Or(
                operands
                    .iter()
                    .map(|operand| Self::from_repository(operand, scheme))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Source::If {
                requirement,
                condition,
                otherwise,
            } => Self::if_expression(requirement, condition, otherwise.as_deref(), scheme),
            Source::Unless {
                requirement,
                condition,
                otherwise,
            } => Self::unless_expression(requirement, condition, otherwise.as_deref(), scheme),
            Source::With { left, right } => Ok(Self::And(vec![
                Self::from_repository(left, scheme)?,
                Self::from_repository(right, scheme)?,
            ])),
            Source::Without { left, right } => Ok(Self::And(vec![
                Self::from_repository(left, scheme)?,
                Self::Not(Box::new(Self::from_repository(right, scheme)?)),
            ])),
        }
    }

    fn if_expression(
        requirement: &RepositoryRequirementExpression,
        condition: &RepositoryRequirementExpression,
        otherwise: Option<&RepositoryRequirementExpression>,
        scheme: VersionScheme,
    ) -> Result<Self, String> {
        let requirement = Self::from_repository(requirement, scheme)?;
        let condition = Self::from_repository(condition, scheme)?;
        let first = Self::Or(vec![Self::Not(Box::new(condition.clone())), requirement]);
        Ok(match otherwise {
            Some(otherwise) => Self::And(vec![
                first,
                Self::Or(vec![condition, Self::from_repository(otherwise, scheme)?]),
            ]),
            None => first,
        })
    }

    fn unless_expression(
        requirement: &RepositoryRequirementExpression,
        condition: &RepositoryRequirementExpression,
        otherwise: Option<&RepositoryRequirementExpression>,
        scheme: VersionScheme,
    ) -> Result<Self, String> {
        let requirement = Self::from_repository(requirement, scheme)?;
        let condition = Self::from_repository(condition, scheme)?;
        let first = Self::Or(vec![condition.clone(), requirement]);
        Ok(match otherwise {
            Some(otherwise) => Self::And(vec![
                first,
                Self::Or(vec![
                    Self::Not(Box::new(condition)),
                    Self::from_repository(otherwise, scheme)?,
                ]),
            ]),
            None => first,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SolverDep {
    pub expression: SolverExpression,
    pub requirement_group: Option<RepositoryRequirementGroupIdentity>,
}

/// Persisted authority for one repository requirement group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepositoryRequirementGroupIdentity {
    pub repository_package_id: i64,
    pub repository_requirement_group_id: i64,
}

/// Exact transaction relation attached to a solver candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverRelation {
    pub scheme: VersionScheme,
    pub relation: RepositoryRequirementGroup,
}

impl SolverDep {
    pub fn single(name: String, constraint: ConaryConstraint) -> Self {
        Self {
            expression: SolverExpression::atom(name, constraint),
            requirement_group: None,
        }
    }

    pub fn as_single(&self) -> Option<(&str, &ConaryConstraint)> {
        match &self.expression {
            SolverExpression::Atom(atom) => Some((&atom.name, &atom.constraint)),
            _ => None,
        }
    }

    pub fn single_name(&self) -> Option<&str> {
        self.as_single().map(|(name, _)| name)
    }
}

impl fmt::Display for ConaryConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Requested(constraint) => write!(f, "{}", constraint),
            Self::ExactRepositoryPackage(id) => write!(f, "repository package {id}"),
            Self::Repository {
                raw, constraint, ..
            } => {
                if let Some(raw) = raw {
                    write!(f, "{}", raw)
                } else {
                    write!(f, "{:?}", constraint)
                }
            }
            Self::RpmRuntime(requirement) => {
                write!(
                    f,
                    "{} {}",
                    requirement.feature.capability(),
                    requirement.native_constraint.as_deref().unwrap_or("<any>")
                )
            }
            Self::ProviderExpression { expression } => {
                write!(f, "same-provider {expression:?}")
            }
        }
    }
}
