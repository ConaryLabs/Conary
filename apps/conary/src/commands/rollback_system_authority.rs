// apps/conary/src/commands/rollback_system_authority.rs

//! Exact normalized native-system authority retained across rollback.

use anyhow::{Result, bail};
use conary_core::ccs::native_transaction::{DebPackageState, NativePackageIdentity};
use conary_core::db::models::NativeLifecycleResidualState;
use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod runtime_projections;

use runtime_projections::RuntimeProjections;
pub(crate) use runtime_projections::{
    ConfigRuntimeSnapshot, DerivedRuntimeSnapshot, InstalledNativeRuntimeSnapshot,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RollbackSystemAuthority {
    pub alternative_groups: Vec<AlternativeGroupSnapshot>,
    pub alternative_slaves: Vec<AlternativeSlaveSnapshot>,
    pub alternative_choices: Vec<AlternativeChoiceSnapshot>,
    pub alternative_choice_slaves: Vec<AlternativeChoiceSlaveSnapshot>,
    pub debconf_templates: Vec<DebconfTemplateSnapshot>,
    pub debconf_template_fields: Vec<DebconfTemplateFieldSnapshot>,
    pub debconf_questions: Vec<DebconfQuestionSnapshot>,
    pub debconf_question_owners: Vec<DebconfQuestionOwnerSnapshot>,
    pub debconf_flags: Vec<DebconfFlagSnapshot>,
    pub debconf_substitutions: Vec<DebconfSubstitutionSnapshot>,
    pub native_lifecycle_residual_states: Vec<NativeLifecycleResidualSnapshot>,
    pub installed_native_runtime: Vec<InstalledNativeRuntimeSnapshot>,
    pub config_runtime: Vec<ConfigRuntimeSnapshot>,
    pub derived_runtime: Vec<DerivedRuntimeSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AlternativeGroupSnapshot {
    pub name: String,
    pub mode: String,
    pub master_link: String,
    pub selected_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AlternativeSlaveSnapshot {
    pub group_name: String,
    pub name: String,
    pub link: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AlternativeChoiceSnapshot {
    pub group_name: String,
    pub path: String,
    pub priority: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AlternativeChoiceSlaveSnapshot {
    pub group_name: String,
    pub choice_path: String,
    pub slave_name: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DebconfTemplateSnapshot {
    pub name: String,
    pub template_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DebconfTemplateFieldSnapshot {
    pub template_name: String,
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DebconfQuestionSnapshot {
    pub name: String,
    pub template_name: String,
    pub value: Option<Vec<u8>>,
    pub seen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DebconfQuestionOwnerSnapshot {
    pub question_name: String,
    pub owner: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DebconfFlagSnapshot {
    pub question_name: String,
    pub name: String,
    pub value: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DebconfSubstitutionSnapshot {
    pub question_name: String,
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeLifecycleResidualSnapshot {
    pub identity_digest: String,
    pub source_format: String,
    pub source_package: String,
    pub source_version: String,
    pub source_arch: Option<String>,
    pub lifecycle_state: String,
    pub pending_triggers: Vec<String>,
    pub awaited_packages: Vec<NativePackageIdentity>,
    pub bundle_toml: String,
    pub updated_at: String,
}

impl RollbackSystemAuthority {
    pub(crate) fn capture(conn: &Connection) -> Result<Self> {
        let runtime = RuntimeProjections::capture(conn)?;
        let authority = Self {
            alternative_groups: query_rows(
                conn,
                "SELECT name, mode, master_link, selected_path
                 FROM debian_alternative_groups ORDER BY name",
                |row| {
                    Ok(AlternativeGroupSnapshot {
                        name: row.get(0)?,
                        mode: row.get(1)?,
                        master_link: row.get(2)?,
                        selected_path: row.get(3)?,
                    })
                },
            )?,
            alternative_slaves: query_rows(
                conn,
                "SELECT group_name, name, link
                 FROM debian_alternative_slaves ORDER BY group_name, name",
                |row| {
                    Ok(AlternativeSlaveSnapshot {
                        group_name: row.get(0)?,
                        name: row.get(1)?,
                        link: row.get(2)?,
                    })
                },
            )?,
            alternative_choices: query_rows(
                conn,
                "SELECT group_name, path, priority
                 FROM debian_alternative_choices ORDER BY group_name, path",
                |row| {
                    Ok(AlternativeChoiceSnapshot {
                        group_name: row.get(0)?,
                        path: row.get(1)?,
                        priority: row.get(2)?,
                    })
                },
            )?,
            alternative_choice_slaves: query_rows(
                conn,
                "SELECT group_name, choice_path, slave_name, target
                 FROM debian_alternative_choice_slaves
                 ORDER BY group_name, choice_path, slave_name",
                |row| {
                    Ok(AlternativeChoiceSlaveSnapshot {
                        group_name: row.get(0)?,
                        choice_path: row.get(1)?,
                        slave_name: row.get(2)?,
                        target: row.get(3)?,
                    })
                },
            )?,
            debconf_templates: query_rows(
                conn,
                "SELECT name, template_type FROM debian_debconf_templates ORDER BY name",
                |row| {
                    Ok(DebconfTemplateSnapshot {
                        name: row.get(0)?,
                        template_type: row.get(1)?,
                    })
                },
            )?,
            debconf_template_fields: query_rows(
                conn,
                "SELECT template_name, name, value
                 FROM debian_debconf_template_fields ORDER BY template_name, name",
                |row| {
                    Ok(DebconfTemplateFieldSnapshot {
                        template_name: row.get(0)?,
                        name: row.get(1)?,
                        value: row.get(2)?,
                    })
                },
            )?,
            debconf_questions: query_rows(
                conn,
                "SELECT name, template_name, value, seen
                 FROM debian_debconf_questions ORDER BY name",
                |row| {
                    Ok(DebconfQuestionSnapshot {
                        name: row.get(0)?,
                        template_name: row.get(1)?,
                        value: row.get(2)?,
                        seen: exact_bool(row.get(3)?, "debconf question seen")?,
                    })
                },
            )?,
            debconf_question_owners: query_rows(
                conn,
                "SELECT question_name, owner
                 FROM debian_debconf_question_owners ORDER BY question_name, owner",
                |row| {
                    Ok(DebconfQuestionOwnerSnapshot {
                        question_name: row.get(0)?,
                        owner: row.get(1)?,
                    })
                },
            )?,
            debconf_flags: query_rows(
                conn,
                "SELECT question_name, name, value
                 FROM debian_debconf_flags ORDER BY question_name, name",
                |row| {
                    Ok(DebconfFlagSnapshot {
                        question_name: row.get(0)?,
                        name: row.get(1)?,
                        value: exact_bool(row.get(2)?, "debconf flag value")?,
                    })
                },
            )?,
            debconf_substitutions: query_rows(
                conn,
                "SELECT question_name, name, value
                 FROM debian_debconf_substitutions ORDER BY question_name, name",
                |row| {
                    Ok(DebconfSubstitutionSnapshot {
                        question_name: row.get(0)?,
                        name: row.get(1)?,
                        value: row.get(2)?,
                    })
                },
            )?,
            native_lifecycle_residual_states: NativeLifecycleResidualState::find_all(conn)?
                .into_iter()
                .map(|state| {
                    state.bundle()?;
                    Ok(NativeLifecycleResidualSnapshot {
                        identity_digest: state.identity_digest,
                        source_format: state.source_format,
                        source_package: state.source_package,
                        source_version: state.source_version,
                        source_arch: state.source_arch,
                        lifecycle_state: state.lifecycle_state.as_str().to_string(),
                        pending_triggers: state.pending_triggers,
                        awaited_packages: state.awaited_packages,
                        bundle_toml: state.bundle_toml,
                        updated_at: state.updated_at,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            installed_native_runtime: runtime.installed_native,
            config_runtime: runtime.config,
            derived_runtime: runtime.derived,
        };
        authority.validate()?;
        Ok(authority)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let groups = unique_keys(
            "alternative group",
            self.alternative_groups.iter().map(|row| row.name.as_str()),
        )?;
        for group in &self.alternative_groups {
            require_text("alternative mode", &group.mode)?;
            if !matches!(group.mode.as_str(), "auto" | "manual") {
                bail!(
                    "rollback system authority has unknown alternative mode '{}'",
                    group.mode
                );
            }
            require_text("alternative master link", &group.master_link)?;
            require_text("alternative selected path", &group.selected_path)?;
        }
        let slave_keys = unique_keys(
            "alternative slave",
            self.alternative_slaves
                .iter()
                .map(|row| (row.group_name.as_str(), row.name.as_str())),
        )?;
        for slave in &self.alternative_slaves {
            require_member("alternative slave", &slave.group_name, &groups)?;
            require_text("alternative slave link", &slave.link)?;
        }
        let choice_keys = unique_keys(
            "alternative choice",
            self.alternative_choices
                .iter()
                .map(|row| (row.group_name.as_str(), row.path.as_str())),
        )?;
        for choice in &self.alternative_choices {
            require_member("alternative choice", &choice.group_name, &groups)?;
        }
        unique_keys(
            "alternative choice slave",
            self.alternative_choice_slaves.iter().map(|row| {
                (
                    row.group_name.as_str(),
                    row.choice_path.as_str(),
                    row.slave_name.as_str(),
                )
            }),
        )?;
        for row in &self.alternative_choice_slaves {
            if !choice_keys.contains(&(row.group_name.as_str(), row.choice_path.as_str())) {
                bail!("rollback system authority has choice-slave without its choice");
            }
            if !slave_keys.contains(&(row.group_name.as_str(), row.slave_name.as_str())) {
                bail!("rollback system authority has choice-slave without its slave");
            }
            require_text("alternative choice-slave target", &row.target)?;
        }

        let templates = unique_keys(
            "debconf template",
            self.debconf_templates.iter().map(|row| row.name.as_str()),
        )?;
        for template in &self.debconf_templates {
            require_text("debconf template type", &template.template_type)?;
        }
        unique_keys(
            "debconf template field",
            self.debconf_template_fields
                .iter()
                .map(|row| (row.template_name.as_str(), row.name.as_str())),
        )?;
        for field in &self.debconf_template_fields {
            require_member("debconf template field", &field.template_name, &templates)?;
        }
        let questions = unique_keys(
            "debconf question",
            self.debconf_questions.iter().map(|row| row.name.as_str()),
        )?;
        for question in &self.debconf_questions {
            require_member("debconf question", &question.template_name, &templates)?;
        }
        unique_keys(
            "debconf question owner",
            self.debconf_question_owners
                .iter()
                .map(|row| (row.question_name.as_str(), row.owner.as_str())),
        )?;
        for owner in &self.debconf_question_owners {
            require_member("debconf question owner", &owner.question_name, &questions)?;
        }
        unique_keys(
            "debconf flag",
            self.debconf_flags
                .iter()
                .map(|row| (row.question_name.as_str(), row.name.as_str())),
        )?;
        for flag in &self.debconf_flags {
            require_member("debconf flag", &flag.question_name, &questions)?;
        }
        unique_keys(
            "debconf substitution",
            self.debconf_substitutions
                .iter()
                .map(|row| (row.question_name.as_str(), row.name.as_str())),
        )?;
        for substitution in &self.debconf_substitutions {
            require_member(
                "debconf substitution",
                &substitution.question_name,
                &questions,
            )?;
        }
        unique_keys(
            "native lifecycle residual identity",
            self.native_lifecycle_residual_states
                .iter()
                .map(|row| row.identity_digest.as_str()),
        )?;
        for residual in &self.native_lifecycle_residual_states {
            residual.as_model()?.bundle()?;
        }
        runtime_projections::validate(
            &self.installed_native_runtime,
            &self.config_runtime,
            &self.derived_runtime,
        )?;
        Ok(())
    }

    pub(crate) fn restore(&self, tx: &Transaction<'_>) -> Result<()> {
        self.validate()?;
        tx.execute_batch(
            "DELETE FROM debian_alternative_choice_slaves;
             DELETE FROM debian_alternative_choices;
             DELETE FROM debian_alternative_slaves;
             DELETE FROM debian_alternative_groups;
             DELETE FROM debian_debconf_substitutions;
             DELETE FROM debian_debconf_flags;
             DELETE FROM debian_debconf_question_owners;
             DELETE FROM debian_debconf_questions;
             DELETE FROM debian_debconf_template_fields;
             DELETE FROM debian_debconf_templates;
             DELETE FROM native_lifecycle_residual_states;",
        )?;
        for row in &self.alternative_groups {
            tx.execute(
                "INSERT INTO debian_alternative_groups
                 (name, mode, master_link, selected_path) VALUES (?1, ?2, ?3, ?4)",
                params![&row.name, &row.mode, &row.master_link, &row.selected_path],
            )?;
        }
        for row in &self.alternative_slaves {
            tx.execute(
                "INSERT INTO debian_alternative_slaves
                 (group_name, name, link) VALUES (?1, ?2, ?3)",
                params![&row.group_name, &row.name, &row.link],
            )?;
        }
        for row in &self.alternative_choices {
            tx.execute(
                "INSERT INTO debian_alternative_choices
                 (group_name, path, priority) VALUES (?1, ?2, ?3)",
                params![&row.group_name, &row.path, row.priority],
            )?;
        }
        for row in &self.alternative_choice_slaves {
            tx.execute(
                "INSERT INTO debian_alternative_choice_slaves
                 (group_name, choice_path, slave_name, target) VALUES (?1, ?2, ?3, ?4)",
                params![
                    &row.group_name,
                    &row.choice_path,
                    &row.slave_name,
                    &row.target
                ],
            )?;
        }
        for row in &self.debconf_templates {
            tx.execute(
                "INSERT INTO debian_debconf_templates (name, template_type) VALUES (?1, ?2)",
                params![&row.name, &row.template_type],
            )?;
        }
        for row in &self.debconf_template_fields {
            tx.execute(
                "INSERT INTO debian_debconf_template_fields
                 (template_name, name, value) VALUES (?1, ?2, ?3)",
                params![&row.template_name, &row.name, &row.value],
            )?;
        }
        for row in &self.debconf_questions {
            tx.execute(
                "INSERT INTO debian_debconf_questions
                 (name, template_name, value, seen) VALUES (?1, ?2, ?3, ?4)",
                params![&row.name, &row.template_name, &row.value, row.seen as i64],
            )?;
        }
        for row in &self.debconf_question_owners {
            tx.execute(
                "INSERT INTO debian_debconf_question_owners
                 (question_name, owner) VALUES (?1, ?2)",
                params![&row.question_name, &row.owner],
            )?;
        }
        for row in &self.debconf_flags {
            tx.execute(
                "INSERT INTO debian_debconf_flags
                 (question_name, name, value) VALUES (?1, ?2, ?3)",
                params![&row.question_name, &row.name, row.value as i64],
            )?;
        }
        for row in &self.debconf_substitutions {
            tx.execute(
                "INSERT INTO debian_debconf_substitutions
                 (question_name, name, value) VALUES (?1, ?2, ?3)",
                params![&row.question_name, &row.name, &row.value],
            )?;
        }
        for row in &self.native_lifecycle_residual_states {
            tx.execute(
                "INSERT INTO native_lifecycle_residual_states (
                    identity_digest, source_format, source_package, source_version,
                    source_arch, lifecycle_state, pending_triggers_json,
                    awaited_packages_json, bundle_toml, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    &row.identity_digest,
                    &row.source_format,
                    &row.source_package,
                    &row.source_version,
                    &row.source_arch,
                    &row.lifecycle_state,
                    serde_json::to_string(&row.pending_triggers)?,
                    serde_json::to_string(&row.awaited_packages)?,
                    &row.bundle_toml,
                    &row.updated_at,
                ],
            )?;
        }
        runtime_projections::restore(
            tx,
            &self.installed_native_runtime,
            &self.config_runtime,
            &self.derived_runtime,
        )?;
        Ok(())
    }
}

impl NativeLifecycleResidualSnapshot {
    fn as_model(&self) -> Result<NativeLifecycleResidualState> {
        Ok(NativeLifecycleResidualState {
            identity_digest: self.identity_digest.clone(),
            source_format: self.source_format.clone(),
            source_package: self.source_package.clone(),
            source_version: self.source_version.clone(),
            source_arch: self.source_arch.clone(),
            lifecycle_state: DebPackageState::parse(&self.lifecycle_state)?,
            pending_triggers: self.pending_triggers.clone(),
            awaited_packages: self.awaited_packages.clone(),
            bundle_toml: self.bundle_toml.clone(),
            updated_at: self.updated_at.clone(),
        })
    }
}

fn query_rows<T>(
    conn: &Connection,
    sql: &str,
    map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    Ok(conn
        .prepare(sql)?
        .query_map([], map)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn exact_bool(value: i64, context: &str) -> rusqlite::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            format!("{context} has non-boolean value {value}").into(),
        )),
    }
}

fn require_text(context: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("rollback system authority has empty {context}");
    }
    Ok(())
}

fn unique_keys<T: Ord>(context: &str, values: impl IntoIterator<Item = T>) -> Result<BTreeSet<T>> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            bail!("rollback system authority repeats {context}");
        }
    }
    Ok(seen)
}

fn require_member<'a>(context: &str, value: &'a str, owners: &BTreeSet<&'a str>) -> Result<()> {
    require_text(context, value)?;
    if !owners.contains(value) {
        bail!("rollback system authority {context} references missing owner '{value}'");
    }
    Ok(())
}

#[cfg(test)]
#[path = "rollback_system_authority/tests.rs"]
mod tests;
