// conary-core/src/repository/declarations/apt.rs

//! APT `sources.list` and deb822 `.sources` declaration authority.

use super::{
    DeclarationEcosystem, DeclarationError, DeclarationLocation, Result, parse_bool, split_words,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AptSyntax {
    OneLine,
    Deb822,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AptSourceKind {
    Deb,
    DebSrc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AptOptionOperation {
    Set,
    Add,
    Remove,
}

/// Current options consumed by APT's source-list and Debian meta-index code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AptOptionName {
    Architectures,
    Languages,
    Targets,
    Trusted,
    CheckValidUntil,
    ValidUntilMin,
    ValidUntilMax,
    CheckDate,
    DateMaxFuture,
    Snapshot,
    SignedBy,
    PDiffs,
    ByHash,
    Include,
    Exclude,
    AllowInsecure,
    AllowWeak,
    AllowDowngradeToInsecure,
    InReleasePath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AptOption {
    pub name: AptOptionName,
    pub operation: AptOptionOperation,
    pub raw_value: String,
    pub values: Vec<String>,
    pub location: DeclarationLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AptAnnotation {
    pub name: String,
    pub raw_value: String,
    pub location: DeclarationLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AptSourceEntry {
    pub kinds: Vec<AptSourceKind>,
    pub enabled: bool,
    pub uris: Vec<String>,
    pub suites: Vec<String>,
    pub components: Vec<String>,
    pub options: Vec<AptOption>,
    pub annotations: Vec<AptAnnotation>,
    pub location: DeclarationLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AptSourceDocument {
    pub path: PathBuf,
    pub syntax: AptSyntax,
    /// Exact bytes after UTF-8 validation. Rendering returns this unchanged.
    pub source: String,
    pub entries: Vec<AptSourceEntry>,
}

impl AptSourceDocument {
    pub fn parse(path: impl Into<PathBuf>, syntax: AptSyntax, source: &str) -> Result<Self> {
        let path = path.into();
        let entries = match syntax {
            AptSyntax::OneLine => parse_one_line(&path, source)?,
            AptSyntax::Deb822 => parse_deb822(&path, source)?,
        };
        Ok(Self {
            path,
            syntax,
            source: source.to_string(),
            entries,
        })
    }

    pub fn render_preserved(&self) -> &str {
        &self.source
    }
}

fn parse_one_line(path: &Path, source: &str) -> Result<Vec<AptSourceEntry>> {
    let mut entries = Vec::new();
    for (index, raw_line) in source.lines().enumerate() {
        let line_number = index + 1;
        let mut line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }

        let mut enabled = true;
        if let Some(comment) = line.strip_prefix('#') {
            let candidate = comment.trim_start();
            if candidate.starts_with("deb ")
                || candidate.starts_with("deb\t")
                || candidate.starts_with("deb-src ")
                || candidate.starts_with("deb-src\t")
            {
                enabled = false;
                line = candidate;
            } else {
                continue;
            }
        }
        let line = strip_one_line_comment(line).trim();
        if line.is_empty() {
            continue;
        }

        let (kind_text, mut remainder) = take_word(line).ok_or_else(|| {
            DeclarationError::syntax(
                DeclarationEcosystem::Apt,
                path,
                line_number,
                "source entry has no type",
            )
        })?;
        let kind = parse_kind(path, line_number, kind_text)?;
        remainder = remainder.trim_start();

        let mut options = Vec::new();
        if remainder.starts_with('[') {
            let closing = find_closing_bracket(remainder).ok_or_else(|| {
                DeclarationError::syntax(
                    DeclarationEcosystem::Apt,
                    path,
                    line_number,
                    "option block is missing ']'",
                )
            })?;
            let option_block = &remainder[1..closing];
            for token in shell_words(option_block, path, line_number)? {
                options.push(parse_one_line_option(path, line_number, &token)?);
            }
            remainder = remainder[closing + 1..].trim_start();
        }

        let words = shell_words(remainder, path, line_number)?;
        if words.len() < 2 {
            return Err(DeclarationError::syntax(
                DeclarationEcosystem::Apt,
                path,
                line_number,
                "source entry requires URI and suite",
            ));
        }
        let suite = words[1].clone();
        let components = words[2..].to_vec();
        validate_components(path, line_number, &suite, &components)?;
        entries.push(AptSourceEntry {
            kinds: vec![kind],
            enabled,
            uris: vec![words[0].clone()],
            suites: vec![suite],
            components,
            options,
            annotations: Vec::new(),
            location: DeclarationLocation::new(path, line_number),
        });
    }
    Ok(entries)
}

fn parse_deb822(path: &Path, source: &str) -> Result<Vec<AptSourceEntry>> {
    let stanzas = deb822_fields(path, source)?;
    let mut entries = Vec::new();
    for fields in stanzas {
        let line = fields.first().map_or(1, |field| field.location.line);
        let mut kinds = None;
        let mut uris = None;
        let mut suites = None;
        let mut components = Vec::new();
        let mut enabled = true;
        let mut options = Vec::new();
        let mut annotations = Vec::new();

        for field in fields {
            match field.name.as_str() {
                "Types" => {
                    kinds = Some(
                        split_words(&field.value)
                            .into_iter()
                            .map(|value| parse_kind(path, field.location.line, &value))
                            .collect::<Result<Vec<_>>>()?,
                    );
                }
                "URIs" => uris = Some(split_words(&field.value)),
                "Suites" => suites = Some(split_words(&field.value)),
                "Components" => components = split_words(&field.value),
                "Enabled" => {
                    enabled = parse_bool(
                        DeclarationEcosystem::Apt,
                        path,
                        field.location.line,
                        &field.value,
                    )?;
                }
                "Description" => annotations.push(AptAnnotation {
                    name: field.name,
                    raw_value: field.value,
                    location: field.location,
                }),
                name if name.starts_with("X-") => annotations.push(AptAnnotation {
                    name: field.name,
                    raw_value: field.value,
                    location: field.location,
                }),
                _ => options.push(parse_deb822_option(path, field)?),
            }
        }

        let kinds = required_field(path, line, "Types", kinds)?;
        let uris = required_field(path, line, "URIs", uris)?;
        let suites = required_field(path, line, "Suites", suites)?;
        if kinds.is_empty() || uris.is_empty() || suites.is_empty() {
            return Err(DeclarationError::syntax(
                DeclarationEcosystem::Apt,
                path,
                line,
                "Types, URIs, and Suites must each contain at least one value",
            ));
        }
        for suite in &suites {
            validate_components(path, line, suite, &components)?;
        }
        entries.push(AptSourceEntry {
            kinds,
            enabled,
            uris,
            suites,
            components,
            options,
            annotations,
            location: DeclarationLocation::new(path, line),
        });
    }
    Ok(entries)
}

#[derive(Debug)]
struct Deb822Field {
    name: String,
    value: String,
    location: DeclarationLocation,
}

fn deb822_fields(path: &Path, source: &str) -> Result<Vec<Vec<Deb822Field>>> {
    let mut stanzas = Vec::new();
    let mut current = Vec::<Deb822Field>::new();
    for (index, raw_line) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            if !current.is_empty() {
                stanzas.push(std::mem::take(&mut current));
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            let field = current.last_mut().ok_or_else(|| {
                DeclarationError::syntax(
                    DeclarationEcosystem::Apt,
                    path,
                    line_number,
                    "continuation line has no field",
                )
            })?;
            field.value.push('\n');
            field.value.push_str(line.trim_start());
            continue;
        }
        let (name, value) = line.split_once(':').ok_or_else(|| {
            DeclarationError::syntax(
                DeclarationEcosystem::Apt,
                path,
                line_number,
                "deb822 field is missing ':'",
            )
        })?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(DeclarationError::syntax(
                DeclarationEcosystem::Apt,
                path,
                line_number,
                format!("invalid deb822 field name '{name}'"),
            ));
        }
        current.push(Deb822Field {
            name: name.to_string(),
            value: value.trim_start().to_string(),
            location: DeclarationLocation::new(path, line_number),
        });
    }
    if !current.is_empty() {
        stanzas.push(current);
    }
    Ok(stanzas)
}

fn parse_kind(path: &Path, line: usize, value: &str) -> Result<AptSourceKind> {
    match value {
        "deb" => Ok(AptSourceKind::Deb),
        "deb-src" => Ok(AptSourceKind::DebSrc),
        _ => Err(DeclarationError::unknown(
            DeclarationEcosystem::Apt,
            path,
            line,
            format!("unknown APT source type '{value}'"),
        )),
    }
}

fn parse_one_line_option(path: &Path, line: usize, token: &str) -> Result<AptOption> {
    let (name, value) = token.split_once('=').ok_or_else(|| {
        DeclarationError::syntax(
            DeclarationEcosystem::Apt,
            path,
            line,
            format!("APT option '{token}' has no value separator"),
        )
    })?;
    if name.is_empty() || value.is_empty() {
        return Err(DeclarationError::syntax(
            DeclarationEcosystem::Apt,
            path,
            line,
            format!("APT option '{token}' requires a key and value"),
        ));
    }
    let option_name = apt_option_name(path, line, name, true)?;
    Ok(AptOption {
        name: option_name,
        operation: AptOptionOperation::Set,
        raw_value: value.to_string(),
        values: value.split(',').map(ToOwned::to_owned).collect(),
        location: DeclarationLocation::new(path, line),
    })
}

fn parse_deb822_option(path: &Path, field: Deb822Field) -> Result<AptOption> {
    let (base, operation) = if let Some(base) = field.name.strip_suffix("-Add") {
        (base, AptOptionOperation::Add)
    } else if let Some(base) = field.name.strip_suffix("-Remove") {
        (base, AptOptionOperation::Remove)
    } else {
        (field.name.as_str(), AptOptionOperation::Set)
    };
    let name = apt_option_name(path, field.location.line, base, false)?;
    validate_option_operation(path, field.location.line, &name, operation, &field.name)?;
    let values = if name == AptOptionName::SignedBy {
        vec![field.value.clone()]
    } else {
        split_words(&field.value)
    };
    Ok(AptOption {
        name,
        operation,
        raw_value: field.value,
        values,
        location: field.location,
    })
}

fn apt_option_name(path: &Path, line: usize, name: &str, one_line: bool) -> Result<AptOptionName> {
    let canonical = if one_line {
        match name {
            "arch" => "Architectures",
            "lang" => "Languages",
            "target" => "Targets",
            "trusted" => "Trusted",
            "check-valid-until" => "Check-Valid-Until",
            "valid-until-min" => "Valid-Until-Min",
            "valid-until-max" => "Valid-Until-Max",
            "check-date" => "Check-Date",
            "date-max-future" => "Date-Max-Future",
            "snapshot" => "Snapshot",
            "signed-by" => "Signed-By",
            "pdiffs" => "PDiffs",
            "by-hash" => "By-Hash",
            "include" => "Include",
            "exclude" => "Exclude",
            "allow-insecure" => "Allow-Insecure",
            "allow-weak" => "Allow-Weak",
            "allow-downgrade-to-insecure" => "Allow-Downgrade-To-Insecure",
            "inrelease-path" => "InRelease-Path",
            other => other,
        }
    } else {
        name
    };
    let option = match canonical {
        "Architectures" => AptOptionName::Architectures,
        "Languages" => AptOptionName::Languages,
        "Targets" => AptOptionName::Targets,
        "Trusted" => AptOptionName::Trusted,
        "Check-Valid-Until" => AptOptionName::CheckValidUntil,
        "Valid-Until-Min" => AptOptionName::ValidUntilMin,
        "Valid-Until-Max" => AptOptionName::ValidUntilMax,
        "Check-Date" => AptOptionName::CheckDate,
        "Date-Max-Future" => AptOptionName::DateMaxFuture,
        "Snapshot" => AptOptionName::Snapshot,
        "Signed-By" => AptOptionName::SignedBy,
        "PDiffs" => AptOptionName::PDiffs,
        "By-Hash" => AptOptionName::ByHash,
        "Include" => AptOptionName::Include,
        "Exclude" => AptOptionName::Exclude,
        "Allow-Insecure" if one_line => AptOptionName::AllowInsecure,
        "Allow-Weak" if one_line => AptOptionName::AllowWeak,
        "Allow-Downgrade-To-Insecure" if one_line => AptOptionName::AllowDowngradeToInsecure,
        "InRelease-Path" if one_line => AptOptionName::InReleasePath,
        _ => {
            return Err(DeclarationError::unknown(
                DeclarationEcosystem::Apt,
                path,
                line,
                format!("unmodeled APT source option '{name}'"),
            ));
        }
    };
    Ok(option)
}

fn validate_option_operation(
    path: &Path,
    line: usize,
    option: &AptOptionName,
    operation: AptOptionOperation,
    source_name: &str,
) -> Result<()> {
    if operation != AptOptionOperation::Set
        && !matches!(
            option,
            AptOptionName::Architectures | AptOptionName::Languages | AptOptionName::Targets
        )
    {
        return Err(DeclarationError::syntax(
            DeclarationEcosystem::Apt,
            path,
            line,
            format!("APT option '{source_name}' does not support add/remove semantics"),
        ));
    }
    Ok(())
}

fn validate_components(path: &Path, line: usize, suite: &str, components: &[String]) -> Result<()> {
    if suite.ends_with('/') && !components.is_empty() {
        return Err(DeclarationError::syntax(
            DeclarationEcosystem::Apt,
            path,
            line,
            "an absolute suite ending in '/' cannot declare components",
        ));
    }
    if !suite.ends_with('/') && components.is_empty() {
        return Err(DeclarationError::syntax(
            DeclarationEcosystem::Apt,
            path,
            line,
            "a distribution suite requires at least one component",
        ));
    }
    Ok(())
}

fn required_field<T>(path: &Path, line: usize, name: &str, value: Option<T>) -> Result<T> {
    value.ok_or_else(|| {
        DeclarationError::syntax(
            DeclarationEcosystem::Apt,
            path,
            line,
            format!("deb822 stanza is missing {name}"),
        )
    })
}

fn strip_one_line_comment(line: &str) -> &str {
    let mut bracket_depth = 0usize;
    for (index, character) in line.char_indices() {
        match character {
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '#' if bracket_depth == 0 => return &line[..index],
            _ => {}
        }
    }
    line
}

fn find_closing_bracket(value: &str) -> Option<usize> {
    let mut quote = None;
    for (index, character) in value.char_indices().skip(1) {
        match character {
            '\'' | '"' if quote == Some(character) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(character),
            ']' if quote.is_none() => return Some(index),
            _ => {}
        }
    }
    None
}

fn take_word(value: &str) -> Option<(&str, &str)> {
    let end = value.find(char::is_whitespace).unwrap_or(value.len());
    (end != 0).then_some((&value[..end], &value[end..]))
}

fn shell_words(value: &str, path: &Path, line: usize) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in value.chars() {
        match character {
            '\'' | '"' if quote == Some(character) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(character),
            c if c.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            other => current.push(other),
        }
    }
    if quote.is_some() {
        return Err(DeclarationError::syntax(
            DeclarationEcosystem::Apt,
            path,
            line,
            "unterminated quoted word",
        ));
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_preserves_disabled_and_ordered_authority() {
        let source = "# heading\n# deb [arch=amd64 signed-by=/keys/vendor.gpg] https://example.test stable main\ndeb-src https://example.test stable main contrib # tail\n";
        let document =
            AptSourceDocument::parse("/etc/apt/sources.list", AptSyntax::OneLine, source).unwrap();
        assert_eq!(document.render_preserved(), source);
        assert_eq!(document.entries.len(), 2);
        assert!(!document.entries[0].enabled);
        assert_eq!(
            document.entries[0].options[0].name,
            AptOptionName::Architectures
        );
        assert_eq!(document.entries[1].kinds, vec![AptSourceKind::DebSrc]);
        assert_eq!(document.entries[1].components, ["main", "contrib"]);
    }

    #[test]
    fn deb822_preserves_disabled_multi_value_and_embedded_key() {
        let source = "Types: deb deb-src\nURIs: https://deb.example.test\nSuites: stable testing\nComponents: main contrib\nEnabled: no\nArchitectures: amd64 arm64\nSigned-By:\n -----BEGIN PGP PUBLIC KEY BLOCK-----\n .\n body\nX-Repolib-Name: Vendor\n";
        let document = AptSourceDocument::parse(
            "/etc/apt/sources.list.d/vendor.sources",
            AptSyntax::Deb822,
            source,
        )
        .unwrap();
        let entry = &document.entries[0];
        assert!(!entry.enabled);
        assert_eq!(entry.uris, ["https://deb.example.test"]);
        assert_eq!(entry.suites, ["stable", "testing"]);
        assert_eq!(entry.annotations[0].name, "X-Repolib-Name");
        assert_eq!(document.render_preserved(), source);
    }

    #[test]
    fn unknown_authority_fails_closed() {
        let error = AptSourceDocument::parse(
            "/etc/apt/sources.list",
            AptSyntax::OneLine,
            "deb [future-authority=yes] https://example.test stable main\n",
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            super::super::DeclarationErrorKind::UnknownAuthority
        );
        assert_eq!(error.line, Some(1));
    }

    #[test]
    fn one_line_does_not_invent_deb822_add_remove_operations() {
        let error = AptSourceDocument::parse(
            "/etc/apt/sources.list",
            AptSyntax::OneLine,
            "deb [arch+=arm64] https://example.test stable main\n",
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            super::super::DeclarationErrorKind::UnknownAuthority
        );
    }
}
