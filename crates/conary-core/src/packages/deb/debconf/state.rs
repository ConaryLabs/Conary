// conary-core/src/packages/deb/debconf/state.rs

//! Serializable debconf templates, questions, and shared ownership state.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfTemplate {
    pub name: String,
    pub template_type: String,
    /// Lower-case imported fields plus exact `DATA` field names. Continuation
    /// data uses the upstream `extended_<field>` representation.
    pub fields: BTreeMap<String, Vec<u8>>,
    /// Question names that currently reference this template.
    pub owners: BTreeSet<String>,
}

impl DebconfTemplate {
    pub fn default_value(&self) -> &[u8] {
        self.fields
            .get("default")
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfQuestion {
    pub name: String,
    pub template: String,
    /// Package/confmodule owners. Removing the final owner removes the
    /// question and releases its ownership of the template.
    pub owners: BTreeSet<String>,
    /// Explicit answer. Absence means the template default is effective.
    pub value: Option<Vec<u8>>,
    pub seen: bool,
    /// Boolean flags other than the first-class `seen` flag.
    pub flags: BTreeMap<String, bool>,
    pub substitutions: BTreeMap<String, Vec<u8>>,
}

impl DebconfQuestion {
    pub fn flag(&self, name: &str) -> bool {
        match name {
            "seen" => self.seen,
            "isdefault" => !self.seen,
            other => self.flags.get(other).copied().unwrap_or(false),
        }
    }

    pub fn set_flag(&mut self, name: &str, value: bool) {
        match name {
            "seen" => self.seen = value,
            "isdefault" => self.seen = !value,
            other => {
                self.flags.insert(other.to_string(), value);
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfState {
    pub templates: BTreeMap<String, DebconfTemplate>,
    pub questions: BTreeMap<String, DebconfQuestion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebconfStateError {
    MissingTemplate(String),
    MissingTemplateQuestion(String),
    MissingQuestion(String),
    InvalidTemplateType(String),
}

impl fmt::Display for DebconfStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTemplate(name) => write!(formatter, "template {name:?} does not exist"),
            Self::MissingTemplateQuestion(name) => {
                write!(formatter, "template question {name:?} does not exist")
            }
            Self::MissingQuestion(name) => write!(formatter, "question {name:?} does not exist"),
            Self::InvalidTemplateType(name) => {
                write!(formatter, "template {name:?} has no type")
            }
        }
    }
}

impl std::error::Error for DebconfStateError {}

impl DebconfState {
    pub fn effective_value(&self, question: &str) -> Result<Vec<u8>, DebconfStateError> {
        let question = self
            .questions
            .get(question)
            .ok_or_else(|| DebconfStateError::MissingQuestion(question.to_string()))?;
        if let Some(value) = &question.value {
            return Ok(value.clone());
        }
        let template = self
            .templates
            .get(&question.template)
            .ok_or_else(|| DebconfStateError::MissingTemplate(question.template.clone()))?;
        Ok(template.default_value().to_vec())
    }

    pub fn question_type(&self, question: &str) -> Result<&str, DebconfStateError> {
        let question = self
            .questions
            .get(question)
            .ok_or_else(|| DebconfStateError::MissingQuestion(question.to_string()))?;
        self.templates
            .get(&question.template)
            .map(|template| template.template_type.as_str())
            .ok_or_else(|| DebconfStateError::MissingTemplate(question.template.clone()))
    }

    pub fn metaget(&self, question_name: &str, field: &str) -> Option<Vec<u8>> {
        let question = self.questions.get(question_name)?;
        let field = field.to_lowercase();
        match field.as_str() {
            "name" => Some(question.name.as_bytes().to_vec()),
            "owners" => Some(
                question
                    .owners
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into_bytes(),
            ),
            "template" => Some(question.template.as_bytes().to_vec()),
            "value" => self.effective_value(question_name).ok(),
            "seen" => Some(
                if question.seen {
                    b"true".as_slice()
                } else {
                    b"false".as_slice()
                }
                .to_vec(),
            ),
            "type" => self
                .templates
                .get(&question.template)
                .map(|template| template.template_type.as_bytes().to_vec()),
            _ => self.question_template_field(question, &field),
        }
    }

    pub fn expanded_field(&self, question_name: &str, field: &str) -> Option<Vec<u8>> {
        let question = self.questions.get(question_name)?;
        self.question_template_field(question, field)
    }

    fn question_template_field(&self, question: &DebconfQuestion, field: &str) -> Option<Vec<u8>> {
        let template = self.templates.get(&question.template)?;
        let value = template.fields.get(field).cloned().unwrap_or_default();
        if matches!(field, "description" | "extended_description" | "choices")
            || field.starts_with("description-")
            || field.starts_with("extended_description-")
            || field.starts_with("choices-")
        {
            Some(expand_substitutions(&value, &question.substitutions))
        } else {
            Some(value)
        }
    }

    pub(crate) fn install_template(
        &mut self,
        owner: &str,
        name: String,
        template_type: String,
        fields: BTreeMap<String, Vec<u8>>,
    ) -> Result<(), DebconfStateError> {
        if template_type.is_empty() {
            return Err(DebconfStateError::InvalidTemplateType(name));
        }

        if let Some(question) = self.questions.get_mut(&name) {
            question.owners.insert(owner.to_string());
        } else {
            let mut owners = BTreeSet::new();
            owners.insert(owner.to_string());
            self.questions.insert(
                name.clone(),
                DebconfQuestion {
                    name: name.clone(),
                    template: name.clone(),
                    owners,
                    value: None,
                    seen: false,
                    flags: BTreeMap::new(),
                    substitutions: BTreeMap::new(),
                },
            );
        }

        let mut template_owners = self
            .templates
            .get(&name)
            .map(|template| template.owners.clone())
            .unwrap_or_default();
        template_owners.insert(name.clone());
        self.templates.insert(
            name.clone(),
            DebconfTemplate {
                name: name.clone(),
                template_type,
                fields,
                owners: template_owners,
            },
        );
        self.set_question_template(&name, &name)
    }

    pub(crate) fn create_data_template(
        &mut self,
        owner: &str,
        name: String,
        template_type: String,
    ) -> Result<(), DebconfStateError> {
        self.install_template(owner, name, template_type, BTreeMap::new())
    }

    pub(crate) fn register(
        &mut self,
        template_question: &str,
        question_name: &str,
        owner: &str,
    ) -> Result<(), DebconfStateError> {
        let source_question = self.questions.get(template_question).ok_or_else(|| {
            DebconfStateError::MissingTemplateQuestion(template_question.to_string())
        })?;
        let template_name = source_question.template.clone();
        if !self.templates.contains_key(&template_name) {
            return Err(DebconfStateError::MissingTemplate(template_name));
        }

        if let Some(question) = self.questions.get_mut(question_name) {
            question.owners.insert(owner.to_string());
        } else {
            let mut owners = BTreeSet::new();
            owners.insert(owner.to_string());
            self.questions.insert(
                question_name.to_string(),
                DebconfQuestion {
                    name: question_name.to_string(),
                    template: template_name.clone(),
                    owners,
                    value: None,
                    seen: false,
                    flags: BTreeMap::new(),
                    substitutions: BTreeMap::new(),
                },
            );
        }
        self.set_question_template(question_name, &template_name)
    }

    fn set_question_template(
        &mut self,
        question_name: &str,
        template_name: &str,
    ) -> Result<(), DebconfStateError> {
        if !self.templates.contains_key(template_name) {
            return Err(DebconfStateError::MissingTemplate(
                template_name.to_string(),
            ));
        }
        let old_template = self
            .questions
            .get(question_name)
            .ok_or_else(|| DebconfStateError::MissingQuestion(question_name.to_string()))?
            .template
            .clone();
        if old_template == template_name {
            if let Some(template) = self.templates.get_mut(template_name) {
                template.owners.insert(question_name.to_string());
            }
            return Ok(());
        }

        if let Some(template) = self.templates.get_mut(&old_template) {
            template.owners.remove(question_name);
        }
        self.remove_unowned_template(&old_template);
        self.questions
            .get_mut(question_name)
            .expect("question checked above")
            .template = template_name.to_string();
        self.templates
            .get_mut(template_name)
            .expect("template checked above")
            .owners
            .insert(question_name.to_string());
        Ok(())
    }

    pub(crate) fn remove_question_owner(&mut self, question_name: &str, owner: &str) {
        let Some(question) = self.questions.get_mut(question_name) else {
            return;
        };
        question.owners.remove(owner);
        if !question.owners.is_empty() {
            return;
        }
        let template_name = question.template.clone();
        self.questions.remove(question_name);
        if let Some(template) = self.templates.get_mut(&template_name) {
            template.owners.remove(question_name);
        }
        self.remove_unowned_template(&template_name);
    }

    pub(crate) fn purge_owner(&mut self, owner: &str) {
        let names = self.questions.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.remove_question_owner(&name, owner);
        }
    }

    fn remove_unowned_template(&mut self, template_name: &str) {
        if self
            .templates
            .get(template_name)
            .is_some_and(|template| template.owners.is_empty())
        {
            self.templates.remove(template_name);
        }
    }
}

fn expand_substitutions(raw: &[u8], substitutions: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut output = Vec::with_capacity(raw.len());
    let mut cursor = 0;
    while cursor < raw.len() {
        let Some(relative_start) = raw[cursor..].windows(2).position(|pair| pair == b"${") else {
            output.extend_from_slice(&raw[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        let Some(relative_end) = raw[start + 2..].iter().position(|byte| *byte == b'}') else {
            output.extend_from_slice(&raw[cursor..]);
            break;
        };
        let end = start + 2 + relative_end;
        let key = &raw[start + 2..end];
        if key.is_empty() || key.contains(&b'{') {
            output.extend_from_slice(&raw[cursor..=end]);
            cursor = end + 1;
            continue;
        }
        if start > cursor && raw[start - 1] == b'\\' {
            output.extend_from_slice(&raw[cursor..start - 1]);
            output.extend_from_slice(&raw[start..=end]);
        } else {
            output.extend_from_slice(&raw[cursor..start]);
            if let Ok(key) = std::str::from_utf8(key)
                && let Some(value) = substitutions.get(key)
            {
                output.extend_from_slice(value);
            }
        }
        cursor = end + 1;
    }
    output
}
