// conary-core/src/repository/parsers/fedora/audit.rs

//! Disk-backed audit for RPM repositories that omit `filelists.xml`.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryRequirementExpression, RepositoryRequirementKind,
};
use crate::repository::parsers::PackageMetadata;

/// Primary file provides and path requirements retained outside process RSS.
pub(super) struct PrimaryFileAudit {
    connection: Connection,
}

impl PrimaryFileAudit {
    pub(super) fn create(work_directory: &Path) -> Result<Self> {
        let connection = Connection::open(work_directory.join("rpm-primary-file-audit.sqlite"))?;
        connection.execute_batch(
            "PRAGMA journal_mode = DELETE;
             PRAGMA synchronous = OFF;
             PRAGMA temp_store = FILE;
             PRAGMA trusted_schema = OFF;
             CREATE TABLE provided_paths (
                 path TEXT PRIMARY KEY
             ) STRICT, WITHOUT ROWID;
             CREATE TABLE path_requirements (
                 ordinal INTEGER PRIMARY KEY,
                 package_name TEXT NOT NULL,
                 package_version TEXT NOT NULL,
                 expression_json TEXT NOT NULL
             ) STRICT;",
        )?;
        Ok(Self { connection })
    }

    pub(super) fn package(&mut self, package: &PackageMetadata) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for provide in &package.provides {
            if provide.kind == RepositoryCapabilityKind::File {
                transaction.execute(
                    "INSERT OR IGNORE INTO provided_paths (path) VALUES (?1)",
                    [&provide.name],
                )?;
            }
        }
        for group in &package.requirements {
            if !matches!(
                group.kind,
                RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
            ) || !group
                .expression
                .atoms()
                .iter()
                .any(|atom| atom.name.starts_with('/'))
            {
                continue;
            }
            transaction.execute(
                "INSERT INTO path_requirements (
                     package_name, package_version, expression_json
                 ) VALUES (?1, ?2, ?3)",
                params![
                    &package.name,
                    &package.version,
                    serde_json::to_string(&group.expression)?,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn finish(self, repo_url: &str) -> Result<()> {
        let mut previous = 0_i64;
        loop {
            let requirement = self
                .connection
                .query_row(
                    "SELECT ordinal, package_name, package_version, expression_json
                     FROM path_requirements WHERE ordinal > ?1
                     ORDER BY ordinal LIMIT 1",
                    [previous],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?;
            let Some((ordinal, name, version, expression_json)) = requirement else {
                break;
            };
            previous = ordinal;
            let expression: RepositoryRequirementExpression =
                serde_json::from_str(&expression_json)?;
            let mut provided = |path: &str| -> Result<bool> {
                Ok(self
                    .connection
                    .query_row(
                        "SELECT 1 FROM provided_paths WHERE path = ?1",
                        [path],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some())
            };
            if may_hold_without_filelists(&expression, &mut provided)? {
                continue;
            }
            let mut paths = Vec::new();
            for clause in expression.atoms() {
                if clause.name.starts_with('/') && !provided(&clause.name)? {
                    paths.push(clause.name.as_str());
                }
            }
            return Err(Error::ParseError(format!(
                "repository {repo_url} package {name} {version} requires path '{}', but its signed \
                 repomd.xml publishes no filelists record. primary.xml carries only the \
                 generator-filtered file set, so this path has no repository provider. The \
                 repository must publish filelists.xml.",
                paths.join("', '")
            )));
        }
        Ok(())
    }
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
