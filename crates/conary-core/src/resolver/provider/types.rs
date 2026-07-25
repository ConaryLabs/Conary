// conary-core/src/resolver/provider/types.rs

//! Data types for the resolver provider bridge.
//!
//! Contains the solver-facing constraint and dependency types.
//! Package identity is now represented by `PackageIdentity` from
//! `resolver::identity`.

use std::collections::HashSet;
use std::fmt;

use crate::repository::dependency_model::{
    RepositoryRequirementExpression, RepositoryRequirementGroup,
};
use crate::repository::versioning::{RepoVersionConstraint, VersionScheme};
use crate::version::VersionConstraint;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConaryConstraint {
    Requested(VersionConstraint),
    Repository {
        scheme: VersionScheme,
        constraint: RepoVersionConstraint,
        raw: Option<String>,
    },
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
            Source::Atom(clause) => Ok(Self::Atom {
                name: clause.name.clone(),
                scheme,
                constraint: match clause.version_constraint.as_deref() {
                    Some(raw) => crate::repository::versioning::parse_repo_constraint(scheme, raw)
                        .map_err(|error| {
                            format!(
                                "dependency '{}' has invalid {:?} constraint '{}': {error}",
                                clause.name, scheme, raw
                            )
                        })?,
                    None => RepoVersionConstraint::Any,
                },
            }),
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
            Self::Repository {
                raw, constraint, ..
            } => {
                if let Some(raw) = raw {
                    write!(f, "{}", raw)
                } else {
                    write!(f, "{:?}", constraint)
                }
            }
            Self::ProviderExpression { expression } => {
                write!(f, "same-provider {expression:?}")
            }
        }
    }
}
