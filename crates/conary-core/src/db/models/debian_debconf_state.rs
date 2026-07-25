// conary-core/src/db/models/debian_debconf_state.rs

//! Normalized persistence for Debian debconf's byte-exact shared state.

use crate::packages::deb::debconf::{DebconfQuestion, DebconfState, DebconfTemplate};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};

const SAVEPOINT: &str = "conary_debian_debconf_replace";

/// Current shared debconf database owned by Conary.
pub struct DebianDebconfState;

impl DebianDebconfState {
    /// Load and validate the complete normalized debconf state.
    pub fn load(conn: &Connection) -> Result<DebconfState> {
        let mut state = DebconfState::default();
        load_templates(conn, &mut state)?;
        load_template_fields(conn, &mut state)?;
        load_questions(conn, &mut state)?;
        load_question_owners(conn, &mut state)?;
        load_flags(conn, &mut state)?;
        load_substitutions(conn, &mut state)?;
        rebuild_template_owners(&mut state)?;
        validate_state(&state)?;
        Ok(state)
    }

    /// Atomically replace the complete shared debconf state.
    pub fn replace(conn: &Connection, state: &DebconfState) -> Result<()> {
        validate_state(state)?;
        conn.execute_batch(&format!("SAVEPOINT {SAVEPOINT}"))?;
        let result = replace_inner(conn, state);
        match result {
            Ok(()) => {
                conn.execute_batch(&format!("RELEASE SAVEPOINT {SAVEPOINT}"))?;
                Ok(())
            }
            Err(error) => {
                let _ = conn.execute_batch(&format!(
                    "ROLLBACK TO SAVEPOINT {SAVEPOINT}; RELEASE SAVEPOINT {SAVEPOINT}"
                ));
                Err(error)
            }
        }
    }
}

fn load_templates(conn: &Connection, state: &mut DebconfState) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT name, template_type
         FROM debian_debconf_templates
         ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (name, template_type) = row?;
        if state
            .templates
            .insert(
                name.clone(),
                DebconfTemplate {
                    name,
                    template_type,
                    fields: BTreeMap::new(),
                    owners: BTreeSet::new(),
                },
            )
            .is_some()
        {
            bail!("debconf database repeats a template");
        }
    }
    Ok(())
}

fn load_template_fields(conn: &Connection, state: &mut DebconfState) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT template_name, name, value
         FROM debian_debconf_template_fields
         ORDER BY template_name, name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    for row in rows {
        let (template_name, name, value) = row?;
        let template = state.templates.get_mut(&template_name).with_context(|| {
            format!("debconf field refers to missing template {template_name:?}")
        })?;
        if template.fields.insert(name.clone(), value).is_some() {
            bail!("debconf template {template_name:?} repeats field {name:?}");
        }
    }
    Ok(())
}

fn load_questions(conn: &Connection, state: &mut DebconfState) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT name, template_name, value, seen
         FROM debian_debconf_questions
         ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<Vec<u8>>>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (name, template, value, seen) = row?;
        let seen = decode_boolean(seen, "question seen")?;
        if !state.templates.contains_key(&template) {
            bail!("debconf question {name:?} refers to missing template {template:?}");
        }
        if state
            .questions
            .insert(
                name.clone(),
                DebconfQuestion {
                    name,
                    template,
                    owners: BTreeSet::new(),
                    value,
                    seen,
                    flags: BTreeMap::new(),
                    substitutions: BTreeMap::new(),
                },
            )
            .is_some()
        {
            bail!("debconf database repeats a question");
        }
    }
    Ok(())
}

fn load_question_owners(conn: &Connection, state: &mut DebconfState) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT question_name, owner
         FROM debian_debconf_question_owners
         ORDER BY question_name, owner",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (question_name, owner) = row?;
        let question = state.questions.get_mut(&question_name).with_context(|| {
            format!("debconf owner refers to missing question {question_name:?}")
        })?;
        if !question.owners.insert(owner.clone()) {
            bail!("debconf question {question_name:?} repeats owner {owner:?}");
        }
    }
    Ok(())
}

fn load_flags(conn: &Connection, state: &mut DebconfState) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT question_name, name, value
         FROM debian_debconf_flags
         ORDER BY question_name, name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (question_name, name, value) = row?;
        let value = decode_boolean(value, "question flag")?;
        let question = state.questions.get_mut(&question_name).with_context(|| {
            format!("debconf flag refers to missing question {question_name:?}")
        })?;
        if question.flags.insert(name.clone(), value).is_some() {
            bail!("debconf question {question_name:?} repeats flag {name:?}");
        }
    }
    Ok(())
}

fn load_substitutions(conn: &Connection, state: &mut DebconfState) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT question_name, name, value
         FROM debian_debconf_substitutions
         ORDER BY question_name, name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    for row in rows {
        let (question_name, name, value) = row?;
        let question = state.questions.get_mut(&question_name).with_context(|| {
            format!("debconf substitution refers to missing question {question_name:?}")
        })?;
        if question.substitutions.insert(name.clone(), value).is_some() {
            bail!("debconf question {question_name:?} repeats substitution {name:?}");
        }
    }
    Ok(())
}

fn rebuild_template_owners(state: &mut DebconfState) -> Result<()> {
    for (name, question) in &state.questions {
        state
            .templates
            .get_mut(&question.template)
            .with_context(|| {
                format!(
                    "debconf question {name:?} refers to missing template {:?}",
                    question.template
                )
            })?
            .owners
            .insert(name.clone());
    }
    Ok(())
}

fn validate_state(state: &DebconfState) -> Result<()> {
    let mut expected_template_owners = state
        .templates
        .keys()
        .map(|name| (name.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for (name, template) in &state.templates {
        if name.is_empty() || template.name != *name {
            bail!("debconf template key and identity must be equal and non-empty");
        }
        if template.template_type.is_empty() {
            bail!("debconf template {name:?} has no type");
        }
        if template.fields.keys().any(String::is_empty) {
            bail!("debconf template {name:?} has an empty field name");
        }
    }
    for (name, question) in &state.questions {
        if name.is_empty() || question.name != *name {
            bail!("debconf question key and identity must be equal and non-empty");
        }
        if question.owners.is_empty() || question.owners.iter().any(String::is_empty) {
            bail!("debconf question {name:?} must have a non-empty owner set");
        }
        let owners = expected_template_owners
            .get_mut(&question.template)
            .with_context(|| {
                format!(
                    "debconf question {name:?} refers to missing template {:?}",
                    question.template
                )
            })?;
        owners.insert(name.clone());
        if question
            .flags
            .keys()
            .any(|flag| flag.is_empty() || matches!(flag.as_str(), "seen" | "isdefault"))
        {
            bail!("debconf question {name:?} has an invalid stored flag");
        }
        if question.substitutions.keys().any(String::is_empty) {
            bail!("debconf question {name:?} has an empty substitution name");
        }
    }
    for (name, expected) in expected_template_owners {
        let observed = &state
            .templates
            .get(&name)
            .expect("template came from the same map")
            .owners;
        if observed != &expected {
            bail!("debconf template {name:?} owner set does not match its referencing questions");
        }
        if expected.is_empty() {
            bail!("debconf template {name:?} is unowned");
        }
    }
    Ok(())
}

fn replace_inner(conn: &Connection, state: &DebconfState) -> Result<()> {
    for table in [
        "debian_debconf_substitutions",
        "debian_debconf_flags",
        "debian_debconf_question_owners",
        "debian_debconf_questions",
        "debian_debconf_template_fields",
        "debian_debconf_templates",
    ] {
        conn.execute(&format!("DELETE FROM {table}"), [])?;
    }
    for template in state.templates.values() {
        conn.execute(
            "INSERT INTO debian_debconf_templates (name, template_type)
             VALUES (?1, ?2)",
            params![&template.name, &template.template_type],
        )?;
        for (name, value) in &template.fields {
            conn.execute(
                "INSERT INTO debian_debconf_template_fields
                    (template_name, name, value)
                 VALUES (?1, ?2, ?3)",
                params![&template.name, name, value],
            )?;
        }
    }
    for question in state.questions.values() {
        conn.execute(
            "INSERT INTO debian_debconf_questions
                (name, template_name, value, seen)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                &question.name,
                &question.template,
                &question.value,
                i64::from(question.seen)
            ],
        )?;
        for owner in &question.owners {
            conn.execute(
                "INSERT INTO debian_debconf_question_owners
                    (question_name, owner)
                 VALUES (?1, ?2)",
                params![&question.name, owner],
            )?;
        }
        for (name, value) in &question.flags {
            conn.execute(
                "INSERT INTO debian_debconf_flags
                    (question_name, name, value)
                 VALUES (?1, ?2, ?3)",
                params![&question.name, name, i64::from(*value)],
            )?;
        }
        for (name, value) in &question.substitutions {
            conn.execute(
                "INSERT INTO debian_debconf_substitutions
                    (question_name, name, value)
                 VALUES (?1, ?2, ?3)",
                params![&question.name, name, value],
            )?;
        }
    }
    Ok(())
}

fn decode_boolean(value: i64, field: &str) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => bail!("debconf {field} value {value} is not boolean"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        conn
    }

    fn state() -> DebconfState {
        let question = DebconfQuestion {
            name: "demo/answer".to_string(),
            template: "demo/answer".to_string(),
            owners: BTreeSet::from(["demo".to_string(), "shared".to_string()]),
            value: Some(b"two words\nand bytes \xff".to_vec()),
            seen: true,
            flags: BTreeMap::from([("backup".to_string(), true)]),
            substitutions: BTreeMap::from([("ITEM".to_string(), b"value \0".to_vec())]),
        };
        DebconfState {
            templates: BTreeMap::from([(
                "demo/answer".to_string(),
                DebconfTemplate {
                    name: "demo/answer".to_string(),
                    template_type: "string".to_string(),
                    fields: BTreeMap::from([
                        ("default".to_string(), b"default".to_vec()),
                        ("description".to_string(), b"Choose ${ITEM}".to_vec()),
                    ]),
                    owners: BTreeSet::from(["demo/answer".to_string()]),
                },
            )]),
            questions: BTreeMap::from([("demo/answer".to_string(), question)]),
        }
    }

    #[test]
    fn normalized_state_round_trips_every_byte_and_owner() {
        let conn = connection();
        let expected = state();
        DebianDebconfState::replace(&conn, &expected).unwrap();
        assert_eq!(DebianDebconfState::load(&conn).unwrap(), expected);
    }

    #[test]
    fn replacement_is_complete_and_idempotent() {
        let conn = connection();
        let expected = state();
        DebianDebconfState::replace(&conn, &expected).unwrap();
        DebianDebconfState::replace(&conn, &expected).unwrap();
        DebianDebconfState::replace(&conn, &DebconfState::default()).unwrap();
        assert_eq!(
            DebianDebconfState::load(&conn).unwrap(),
            DebconfState::default()
        );
    }

    #[test]
    fn invalid_owner_projection_never_reaches_storage() {
        let conn = connection();
        let mut invalid = state();
        invalid
            .templates
            .get_mut("demo/answer")
            .unwrap()
            .owners
            .clear();
        assert!(DebianDebconfState::replace(&conn, &invalid).is_err());
        assert_eq!(
            DebianDebconfState::load(&conn).unwrap(),
            DebconfState::default()
        );
    }

    #[test]
    fn tampered_rows_fail_closed() {
        let conn = connection();
        let expected = state();
        DebianDebconfState::replace(&conn, &expected).unwrap();
        conn.execute(
            "UPDATE debian_debconf_questions SET seen = 2 WHERE name = 'demo/answer'",
            [],
        )
        .unwrap_err();
        conn.execute(
            "DELETE FROM debian_debconf_question_owners
             WHERE question_name = 'demo/answer'",
            [],
        )
        .unwrap();
        assert!(DebianDebconfState::load(&conn).is_err());
    }
}
