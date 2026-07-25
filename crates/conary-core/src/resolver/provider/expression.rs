// conary-core/src/resolver/provider/expression.rs

//! Exact compilation of native Boolean dependency expressions for resolvo.

use std::collections::HashSet;

use resolvo::{
    Condition, ConditionId, ConditionalRequirement, LogicalOperator, Requirement, VersionSetId,
};

use crate::error::{Error, Result};
use crate::repository::versioning::{RepoVersionConstraint, VersionScheme};

use super::ConaryProvider;
#[cfg(test)]
use super::types::ConaryConstraint;
use super::types::{SolverAtom, SolverExpression};

const FALSE_REQUIREMENT_NAME: &str = "\0conary:false";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Literal {
    atom: SolverAtom,
    positive: bool,
}

#[derive(Debug, Clone)]
enum NormalExpression {
    Literal(Literal),
    And(Vec<NormalExpression>),
    Or(Vec<NormalExpression>),
}

impl ConaryProvider<'_> {
    pub(super) fn compile_dependency_requirements(&mut self) -> Result<()> {
        let dependencies = std::mem::take(&mut self.dependencies);
        self.compiled_dependencies.clear();

        for (solvable, deps) in &dependencies {
            let mut requirements = Vec::new();
            for dep in deps {
                requirements.extend(self.compile_expression(&dep.expression)?);
            }
            self.compiled_dependencies.insert(*solvable, requirements);
        }

        self.dependencies = dependencies;
        Ok(())
    }

    pub(crate) fn compile_root_requirements(
        &mut self,
        expressions: &[SolverExpression],
    ) -> Result<Vec<ConditionalRequirement>> {
        let requirements = expressions
            .iter()
            .map(|expression| self.compile_expression(expression))
            .collect::<Result<Vec<_>>>()
            .map(|requirements| requirements.into_iter().flatten().collect())?;
        self.validate_solver_invariants()?;
        Ok(requirements)
    }

    fn compile_expression(
        &mut self,
        expression: &SolverExpression,
    ) -> Result<Vec<ConditionalRequirement>> {
        let normal = to_normal_form(expression, false);
        let clauses = to_cnf(&normal);
        let mut requirements = Vec::with_capacity(clauses.len());

        for clause in clauses {
            let Some(clause) = normalize_clause(clause) else {
                continue;
            };
            requirements.push(self.compile_clause(&clause)?);
        }

        Ok(requirements)
    }

    fn compile_clause(&mut self, clause: &[Literal]) -> Result<ConditionalRequirement> {
        let mut positive = Vec::new();
        let mut negative = Vec::new();

        for literal in clause {
            self.intern_constraint(&literal.atom.name, &literal.atom.constraint)?;
            let version_set = self.version_set_id(&literal.atom).ok_or_else(|| {
                Error::ResolutionError(format!(
                    "dependency atom '{}' was not interned",
                    literal.atom.name
                ))
            })?;
            if literal.positive {
                positive.push(version_set);
            } else {
                negative.push(version_set);
            }
        }

        positive.sort_by_key(|id| id.0);
        positive.dedup();
        negative.sort_by_key(|id| id.0);
        negative.dedup();

        let requirement = match positive.as_slice() {
            [] => Requirement::Single(self.intern_false_requirement()?),
            [single] => Requirement::Single(*single),
            alternatives => {
                let union = self
                    .find_union_id(alternatives)
                    .map_or_else(|| self.intern_version_set_union(alternatives.to_vec()), Ok)?;
                Requirement::Union(union)
            }
        };

        let condition = self.intern_conjunction(&negative)?;
        Ok(ConditionalRequirement {
            condition,
            requirement,
        })
    }

    fn version_set_id(&self, atom: &SolverAtom) -> Option<VersionSetId> {
        let name = self.name_to_id.get(&atom.name)?;
        self.version_set_cache
            .get(&(name.0, atom.constraint.clone()))
            .copied()
    }

    fn intern_false_requirement(&mut self) -> Result<VersionSetId> {
        let name = self.intern_name(FALSE_REQUIREMENT_NAME)?;
        self.intern_repo_version_set(
            name,
            VersionScheme::Conary,
            RepoVersionConstraint::Any,
            None,
        )
    }

    fn intern_conjunction(&mut self, sets: &[VersionSetId]) -> Result<Option<ConditionId>> {
        let mut conditions = sets
            .iter()
            .map(|set| self.intern_condition(Condition::Requirement(*set)))
            .collect::<Result<Vec<_>>>()?
            .into_iter();
        let Some(mut combined) = conditions.next() else {
            return Ok(None);
        };
        for condition in conditions {
            combined = self.intern_condition(Condition::Binary(
                LogicalOperator::And,
                combined,
                condition,
            ))?;
        }
        Ok(Some(combined))
    }

    fn intern_condition(&mut self, condition: Condition) -> Result<ConditionId> {
        if let Some(existing) = self.condition_cache.get(&condition) {
            return Ok(*existing);
        }
        let index = Self::pool_u32(self.conditions.len(), "condition")?;
        let id = ConditionId::new(index);
        self.conditions.push(condition.clone());
        self.condition_cache.insert(condition, id);
        Ok(id)
    }
}

fn to_normal_form(expression: &SolverExpression, negated: bool) -> NormalExpression {
    match expression {
        SolverExpression::Atom(atom) => NormalExpression::Literal(Literal {
            atom: atom.clone(),
            positive: !negated,
        }),
        SolverExpression::And(operands) => {
            let operands = operands
                .iter()
                .map(|operand| to_normal_form(operand, negated))
                .collect();
            if negated {
                NormalExpression::Or(operands)
            } else {
                NormalExpression::And(operands)
            }
        }
        SolverExpression::Or(operands) => {
            let operands = operands
                .iter()
                .map(|operand| to_normal_form(operand, negated))
                .collect();
            if negated {
                NormalExpression::And(operands)
            } else {
                NormalExpression::Or(operands)
            }
        }
        SolverExpression::Not(operand) => to_normal_form(operand, !negated),
    }
}

fn to_cnf(expression: &NormalExpression) -> Vec<Vec<Literal>> {
    match expression {
        NormalExpression::Literal(literal) => vec![vec![literal.clone()]],
        NormalExpression::And(operands) => operands.iter().flat_map(to_cnf).collect(),
        NormalExpression::Or(operands) => {
            let mut operands = operands.iter();
            let Some(first) = operands.next() else {
                return vec![Vec::new()];
            };
            operands.fold(to_cnf(first), |left, right| distribute(left, to_cnf(right)))
        }
    }
}

fn distribute(left: Vec<Vec<Literal>>, right: Vec<Vec<Literal>>) -> Vec<Vec<Literal>> {
    if left.is_empty() || right.is_empty() {
        return Vec::new();
    }
    left.into_iter()
        .flat_map(|left_clause| {
            right.iter().cloned().map(move |right_clause| {
                let mut clause = left_clause.clone();
                clause.extend(right_clause);
                clause
            })
        })
        .collect()
}

fn normalize_clause(clause: Vec<Literal>) -> Option<Vec<Literal>> {
    let mut positive = HashSet::new();
    let mut negative = HashSet::new();
    let mut normalized = Vec::new();

    for literal in clause {
        let opposite = if literal.positive {
            &negative
        } else {
            &positive
        };
        if opposite.contains(&literal.atom) {
            return None;
        }
        let same = if literal.positive {
            &mut positive
        } else {
            &mut negative
        };
        if same.insert(literal.atom.clone()) {
            normalized.push(literal);
        }
    }
    Some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(name: &str) -> SolverExpression {
        SolverExpression::atom(
            name.to_string(),
            ConaryConstraint::Repository {
                scheme: VersionScheme::Rpm,
                constraint: RepoVersionConstraint::Any,
                raw: None,
            },
        )
    }

    #[test]
    fn converts_nested_boolean_expression_to_exact_cnf() {
        let expression = SolverExpression::Or(vec![
            atom("a"),
            SolverExpression::And(vec![atom("b"), atom("c")]),
        ]);
        let cnf = to_cnf(&to_normal_form(&expression, false));
        assert_eq!(cnf.len(), 2);
        assert_eq!(cnf[0].len(), 2);
        assert_eq!(cnf[1].len(), 2);
    }

    #[test]
    fn removes_tautological_clauses() {
        let expression =
            SolverExpression::Or(vec![atom("a"), SolverExpression::Not(Box::new(atom("a")))]);
        let cnf = to_cnf(&to_normal_form(&expression, false));
        assert!(normalize_clause(cnf[0].clone()).is_none());
    }
}
