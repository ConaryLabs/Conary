// conary-core/src/repository/declarations/ini.rs

//! Private lossless INI lexer shared by the distinct DNF and libzypp models.

use super::{DeclarationEcosystem, DeclarationError, DeclarationLocation, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IniDocument {
    pub source: String,
    pub path: PathBuf,
    pub sections: Vec<IniSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IniSection {
    pub name: String,
    pub location: DeclarationLocation,
    pub entries: Vec<IniEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IniEntry {
    pub key: String,
    pub value: String,
    pub location: DeclarationLocation,
}

impl IniDocument {
    pub fn parse(ecosystem: DeclarationEcosystem, path: &Path, source: &str) -> Result<Self> {
        let mut sections: Vec<IniSection> = Vec::new();
        let mut current_section: Option<usize> = None;
        let mut current_entry: Option<(usize, usize)> = None;

        for (index, raw_line) in source.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim_end_matches('\r');
            let trimmed = line.trim();
            let zypper = matches!(
                ecosystem,
                DeclarationEcosystem::ZypperRepository | DeclarationEcosystem::ZypperService
            );
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with(';')
                || (zypper && (trimmed.starts_with('#') || trimmed.starts_with(';')))
            {
                if !zypper {
                    current_entry = None;
                }
                continue;
            }

            if trimmed.is_empty() {
                if !zypper {
                    current_entry = None;
                }
                continue;
            }

            if trimmed.starts_with('[') {
                let closing = trimmed.find(']').ok_or_else(|| {
                    DeclarationError::syntax(
                        ecosystem,
                        path,
                        line_number,
                        "section header is missing ']'",
                    )
                })?;
                let name = &trimmed[1..closing];
                if name.is_empty() {
                    return Err(DeclarationError::syntax(
                        ecosystem,
                        path,
                        line_number,
                        "section name is empty",
                    ));
                }
                let trailing = trimmed[closing + 1..].trim();
                if !trailing.is_empty() && !trailing.starts_with('#') && !trailing.starts_with(';')
                {
                    return Err(DeclarationError::syntax(
                        ecosystem,
                        path,
                        line_number,
                        "non-comment text follows section header",
                    ));
                }
                sections.push(IniSection {
                    name: name.to_string(),
                    location: DeclarationLocation::new(path, line_number),
                    entries: Vec::new(),
                });
                current_section = Some(sections.len() - 1);
                current_entry = None;
                continue;
            }

            let zypper_url_continuation = ecosystem == DeclarationEcosystem::ZypperRepository
                && !trimmed.contains('=')
                && current_entry.is_some_and(|(section_index, entry_index)| {
                    matches!(
                        sections[section_index].entries[entry_index].key.as_str(),
                        "baseurl" | "gpgkey" | "mirrorlist" | "metalink"
                    )
                });
            if (!zypper && (line.starts_with(' ') || line.starts_with('\t')))
                || zypper_url_continuation
            {
                let (section_index, entry_index) = current_entry.ok_or_else(|| {
                    DeclarationError::syntax(
                        ecosystem,
                        path,
                        line_number,
                        "continuation line has no preceding option",
                    )
                })?;
                sections[section_index].entries[entry_index]
                    .value
                    .push_str(&format!("\n{trimmed}"));
                continue;
            }

            let section_index = current_section.ok_or_else(|| {
                DeclarationError::syntax(
                    ecosystem,
                    path,
                    line_number,
                    "option appears before the first section",
                )
            })?;
            let (key, value) = line.split_once('=').ok_or_else(|| {
                DeclarationError::syntax(ecosystem, path, line_number, "option is missing '='")
            })?;
            let key = key.trim_end();
            if key.is_empty() {
                return Err(DeclarationError::syntax(
                    ecosystem,
                    path,
                    line_number,
                    "option name is empty",
                ));
            }
            sections[section_index].entries.push(IniEntry {
                key: key.to_string(),
                value: trim_ini_value(value, ecosystem == DeclarationEcosystem::Dnf),
                location: DeclarationLocation::new(path, line_number),
            });
            current_entry = Some((section_index, sections[section_index].entries.len() - 1));
        }

        Ok(Self {
            source: source.to_string(),
            path: path.to_path_buf(),
            sections,
        })
    }
}

fn trim_ini_value(value: &str, strip_quotes: bool) -> String {
    let value = value.trim();
    if strip_quotes && value.len() >= 2 {
        let first = value.as_bytes()[0];
        let last = value.as_bytes()[value.len() - 1];
        if matches!(first, b'\'' | b'"') && first == last {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}
