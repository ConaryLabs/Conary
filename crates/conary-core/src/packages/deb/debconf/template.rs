// conary-core/src/packages/deb/debconf/template.rs

//! Exact import bridge from parsed Debian template records.

use super::DebconfState;
use crate::packages::native_abi::{DebconfTemplateField, DebconfTemplateRecord};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebconfTemplateLoadErrorKind {
    Open,
    Parse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebconfTemplateLoadError {
    pub kind: DebconfTemplateLoadErrorKind,
    pub message: Vec<u8>,
}

impl DebconfTemplateLoadError {
    pub fn open(message: impl Into<Vec<u8>>) -> Self {
        Self {
            kind: DebconfTemplateLoadErrorKind::Open,
            message: message.into(),
        }
    }

    pub fn parse(message: impl Into<Vec<u8>>) -> Self {
        Self {
            kind: DebconfTemplateLoadErrorKind::Parse,
            message: message.into(),
        }
    }
}

impl fmt::Display for DebconfTemplateLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?}: {}",
            self.kind,
            String::from_utf8_lossy(&self.message)
        )
    }
}

impl std::error::Error for DebconfTemplateLoadError {}

/// Typed boundary for `X_LOADTEMPLATEFILE`. Implementations may read a file,
/// CAS object, package member, or test fixture; the protocol engine itself
/// performs no I/O.
pub trait DebconfTemplateLoader {
    fn load(&mut self, path: &[u8])
    -> Result<Vec<DebconfTemplateRecord>, DebconfTemplateLoadError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebconfTemplateImportError {
    pub record: usize,
    pub message: String,
}

impl fmt::Display for DebconfTemplateImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "template record {}: {}",
            self.record, self.message
        )
    }
}

impl std::error::Error for DebconfTemplateImportError {}

impl DebconfState {
    /// Import records transactionally using the exact `Template.pm::load`
    /// continuation folding and shared-owner transitions.
    pub fn import_template_records(
        &mut self,
        owner: &str,
        records: &[DebconfTemplateRecord],
    ) -> Result<(), DebconfTemplateImportError> {
        let prepared = records
            .iter()
            .enumerate()
            .map(|(index, record)| prepare_record(index + 1, record))
            .collect::<Result<Vec<_>, _>>()?;

        let mut next = self.clone();
        for record in prepared {
            let PreparedRecord {
                index,
                name,
                template_type,
                fields,
            } = record;
            next.install_template(owner, name, template_type, fields)
                .map_err(|error| DebconfTemplateImportError {
                    record: index,
                    message: error.to_string(),
                })?;
        }
        *self = next;
        Ok(())
    }
}

struct PreparedRecord {
    index: usize,
    name: String,
    template_type: String,
    fields: BTreeMap<String, Vec<u8>>,
}

fn prepare_record(
    index: usize,
    record: &DebconfTemplateRecord,
) -> Result<PreparedRecord, DebconfTemplateImportError> {
    if record.template.is_empty() {
        return invalid(index, "Template value is empty");
    }
    if record.template_type.is_empty() {
        return invalid(index, "Type value is empty");
    }

    let mut seen = BTreeSet::new();
    let mut fields = BTreeMap::new();
    for field in &record.fields {
        validate_field(index, field)?;
        let name = field.name.to_ascii_lowercase();
        if !seen.insert(name.clone()) {
            return invalid(index, format!("duplicate field {:?}", field.name));
        }
        let (value, extended) = fold_field(field);
        fields.insert(name.clone(), value);
        if !extended.is_empty() {
            fields.insert(format!("extended_{name}"), extended);
        }
    }

    let imported_name = fields
        .get("template")
        .map(|value| trim_end_ascii(value))
        .ok_or_else(|| DebconfTemplateImportError {
            record: index,
            message: "missing Template field".to_string(),
        })?;
    if imported_name != record.template.as_bytes() {
        return invalid(
            index,
            "typed Template value does not match its source field",
        );
    }
    let imported_type = fields
        .get("type")
        .map(|value| trim_end_ascii(value))
        .ok_or_else(|| DebconfTemplateImportError {
            record: index,
            message: "missing Type field".to_string(),
        })?;
    if imported_type != record.template_type.as_bytes() {
        return invalid(index, "typed Type value does not match its source field");
    }

    fields.remove("template");
    fields.remove("type");
    Ok(PreparedRecord {
        index,
        name: record.template.clone(),
        template_type: record.template_type.clone(),
        fields,
    })
}

fn validate_field(
    index: usize,
    field: &DebconfTemplateField,
) -> Result<(), DebconfTemplateImportError> {
    if field.name.is_empty()
        || !field
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'@' | b'.'))
    {
        return invalid(index, format!("invalid field name {:?}", field.name));
    }
    if field
        .continuation_lines
        .iter()
        .any(|line| !line.starts_with(char::is_whitespace))
    {
        return invalid(
            index,
            format!("field {:?} has an invalid continuation", field.name),
        );
    }
    Ok(())
}

fn fold_field(field: &DebconfTemplateField) -> (Vec<u8>, Vec<u8>) {
    let value = field
        .value
        .trim_end_matches(char::is_whitespace)
        .as_bytes()
        .to_vec();
    let mut extended = String::new();
    for line in &field.continuation_lines {
        if line.chars().all(char::is_whitespace) {
            continue;
        }
        let mut characters = line.chars();
        let _leading = characters.next();
        let rest = characters.as_str();
        if rest == "." {
            extended.push_str("\n\n");
        } else if rest.starts_with(char::is_whitespace) {
            let bit = rest.trim_end_matches(char::is_whitespace);
            if !extended.is_empty() && !extended.ends_with(['\n', ' ']) {
                extended.push('\n');
            }
            extended.push_str(bit);
            extended.push('\n');
        } else {
            let bit = rest.trim_end_matches(char::is_whitespace);
            if !extended.is_empty() && !extended.ends_with(['\n', ' ']) {
                extended.push(' ');
            }
            extended.push_str(bit);
        }
    }
    while extended.ends_with('\n') {
        extended.pop();
    }
    (value, extended.into_bytes())
}

fn trim_end_ascii(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |index| index + 1);
    &bytes[..end]
}

fn invalid<T>(record: usize, message: impl Into<String>) -> Result<T, DebconfTemplateImportError> {
    Err(DebconfTemplateImportError {
        record,
        message: message.into(),
    })
}
