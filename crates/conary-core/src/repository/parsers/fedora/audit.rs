// conary-core/src/repository/parsers/fedora/audit.rs

//! Exact audit for RPM repositories that omit `filelists.xml`.

use crate::error::{Error, Result};
use crate::repository::dependency_model::RepositoryRequirementExpression;

/// Refuse one typed positive requirement that cannot be satisfied from
/// primary.xml's generator-filtered file provides.
pub(in crate::repository) fn require_primary_file_providers(
    repo_url: &str,
    package_name: &str,
    package_version: &str,
    expression: &RepositoryRequirementExpression,
    provided: &mut impl FnMut(&str) -> Result<bool>,
) -> Result<()> {
    if may_hold_without_filelists(expression, provided)? {
        return Ok(());
    }
    let mut paths = Vec::new();
    for clause in expression.atoms() {
        if clause.name.starts_with('/') && !provided(&clause.name)? {
            paths.push(clause.name.as_str());
        }
    }
    Err(Error::ParseError(format!(
        "repository {repo_url} package {package_name} {package_version} requires path '{}', but its signed \
         repomd.xml publishes no filelists record. primary.xml carries only the \
         generator-filtered file set, so this path has no repository provider. The \
         repository must publish filelists.xml.",
        paths.join("', '")
    )))
}

fn may_hold_without_filelists(
    expression: &RepositoryRequirementExpression,
    provided: &mut impl FnMut(&str) -> Result<bool>,
) -> Result<bool> {
    match expression {
        RepositoryRequirementExpression::Atom(clause) => {
            Ok(!clause.name.starts_with('/') || provided(&clause.name)?)
        }
        RepositoryRequirementExpression::And(operands) => {
            for operand in operands {
                if !may_hold_without_filelists(operand, provided)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        RepositoryRequirementExpression::Or(operands) => {
            for operand in operands {
                if may_hold_without_filelists(operand, provided)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        RepositoryRequirementExpression::If {
            requirement,
            condition,
            otherwise,
        } => Ok((may_hold_without_filelists(condition, provided)?
            && may_hold_without_filelists(requirement, provided)?)
            || match otherwise {
                Some(otherwise) => may_hold_without_filelists(otherwise, provided)?,
                None => true,
            }),
        RepositoryRequirementExpression::Unless {
            requirement,
            condition,
            otherwise,
        } => Ok(may_hold_without_filelists(requirement, provided)?
            || (may_hold_without_filelists(condition, provided)?
                && match otherwise {
                    Some(otherwise) => may_hold_without_filelists(otherwise, provided)?,
                    None => true,
                })),
        RepositoryRequirementExpression::With { left, right } => {
            Ok(may_hold_without_filelists(left, provided)?
                && may_hold_without_filelists(right, provided)?)
        }
        RepositoryRequirementExpression::Without { left, .. } => {
            may_hold_without_filelists(left, provided)
        }
    }
}
