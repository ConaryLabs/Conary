// conary-core/src/repository/declarations/eopkg.rs

//! Lossless eopkg repository-order declaration grammar.

use super::{DeclarationEcosystem, DeclarationError, DeclarationLocation, Result};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EopkgRepositoryDocument {
    pub path: PathBuf,
    pub source: String,
    pub repositories: Vec<EopkgRepository>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EopkgRepository {
    pub name: String,
    pub index_url: String,
    pub enabled: bool,
    pub media: String,
    pub location: DeclarationLocation,
}

#[derive(Default)]
struct PendingRepository {
    name: Option<String>,
    index_url: Option<String>,
    status: Option<String>,
    media: Option<String>,
    line: usize,
}

impl EopkgRepositoryDocument {
    pub fn parse(path: impl Into<PathBuf>, source: &str) -> Result<Self> {
        let path = path.into();
        let mut reader = Reader::from_str(source);
        reader.config_mut().trim_text(true);
        let mut stack = Vec::new();
        let mut pending: Option<PendingRepository> = None;
        let mut repositories = Vec::new();
        let mut saw_root = false;
        loop {
            let event = reader.read_event().map_err(|error| {
                DeclarationError::syntax(
                    DeclarationEcosystem::Eopkg,
                    &path,
                    line_at(source, reader.buffer_position()),
                    error.to_string(),
                )
            })?;
            match event {
                Event::Start(start) => {
                    let tag = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                    if stack.is_empty() {
                        if saw_root || tag != "REPOS" {
                            return Err(DeclarationError::unknown(
                                DeclarationEcosystem::Eopkg,
                                &path,
                                line_at(source, reader.buffer_position()),
                                "eopkg repository document requires one REPOS root",
                            ));
                        }
                        saw_root = true;
                    } else if stack == ["REPOS"] && tag == "Repo" {
                        pending = Some(PendingRepository {
                            line: line_at(source, reader.buffer_position()),
                            ..PendingRepository::default()
                        });
                    } else if !(stack == ["REPOS", "Repo"]
                        && matches!(tag.as_str(), "Name" | "Url" | "Status" | "Media"))
                    {
                        return Err(DeclarationError::unknown(
                            DeclarationEcosystem::Eopkg,
                            &path,
                            line_at(source, reader.buffer_position()),
                            format!("unknown authority-bearing eopkg element '{tag}'"),
                        ));
                    }
                    stack.push(tag);
                }
                Event::Text(text) => {
                    let Some(field) = stack.last() else { continue };
                    let value = text
                        .decode()
                        .map_err(|error| {
                            DeclarationError::syntax(
                                DeclarationEcosystem::Eopkg,
                                &path,
                                line_at(source, reader.buffer_position()),
                                error.to_string(),
                            )
                        })?
                        .trim()
                        .to_string();
                    let Some(repository) = pending.as_mut() else {
                        continue;
                    };
                    match field.as_str() {
                        "Name" => repository.name = Some(value),
                        "Url" => repository.index_url = Some(value),
                        "Status" => repository.status = Some(value),
                        "Media" => repository.media = Some(value),
                        _ => {}
                    }
                }
                Event::End(end) => {
                    let tag = String::from_utf8_lossy(end.local_name().as_ref()).into_owned();
                    if stack.pop().as_deref() != Some(tag.as_str()) {
                        return Err(DeclarationError::syntax(
                            DeclarationEcosystem::Eopkg,
                            &path,
                            line_at(source, reader.buffer_position()),
                            "mismatched eopkg repository element",
                        ));
                    }
                    if tag == "Repo" {
                        repositories.push(finish_repository(&path, pending.take().unwrap())?);
                    }
                }
                Event::Eof => break,
                Event::Decl(_) | Event::Comment(_) | Event::CData(_) => {}
                _ => {}
            }
        }
        if !saw_root || !stack.is_empty() {
            return Err(DeclarationError::syntax(
                DeclarationEcosystem::Eopkg,
                &path,
                1,
                "incomplete eopkg repository document",
            ));
        }
        Ok(Self {
            path,
            source: source.to_string(),
            repositories,
        })
    }

    #[must_use]
    pub fn render_preserved(&self) -> &str {
        &self.source
    }
}

fn finish_repository(path: &Path, pending: PendingRepository) -> Result<EopkgRepository> {
    let required = |value: Option<String>, field: &str| {
        value.filter(|value| !value.is_empty()).ok_or_else(|| {
            DeclarationError::syntax(
                DeclarationEcosystem::Eopkg,
                path,
                pending.line,
                format!("eopkg Repo requires non-empty {field}"),
            )
        })
    };
    let status = required(pending.status, "Status")?;
    let enabled = match status.as_str() {
        "active" => true,
        "inactive" => false,
        _ => {
            return Err(DeclarationError::unknown(
                DeclarationEcosystem::Eopkg,
                path,
                pending.line,
                format!("unsupported eopkg repository status '{status}'"),
            ));
        }
    };
    Ok(EopkgRepository {
        name: required(pending.name, "Name")?,
        index_url: required(pending.index_url, "Url")?,
        enabled,
        media: required(pending.media, "Media")?,
        location: DeclarationLocation::new(path, pending.line),
    })
}

fn line_at(source: &str, position: u64) -> usize {
    let end = usize::try_from(position)
        .unwrap_or(source.len())
        .min(source.len());
    source[..end].bytes().filter(|byte| *byte == b'\n').count() + 1
}
