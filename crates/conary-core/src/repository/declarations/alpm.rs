// conary-core/src/repository/declarations/alpm.rs

//! ALPM/pacman repository declaration authority.

use super::{
    DeclarationEcosystem, DeclarationError, DeclarationErrorKind, DeclarationLocation, Result,
    split_words,
};
use crate::filesystem::path::safe_join;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAX_INCLUDE_DEPTH: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AlpmSection {
    Options,
    Repository(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AlpmRepositoryOptionName {
    CacheServer,
    Server,
    SigLevel,
    Usage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AlpmDirective {
    Include {
        pattern: String,
        location: DeclarationLocation,
    },
    Architecture {
        values: Vec<String>,
        location: DeclarationLocation,
    },
    GlobalSigLevel {
        values: Vec<String>,
        location: DeclarationLocation,
    },
    RepositoryOption {
        repository: String,
        name: AlpmRepositoryOptionName,
        raw_value: String,
        values: Vec<String>,
        location: DeclarationLocation,
    },
    /// An `[options]` directive that does not affect repository declaration.
    OtherGlobal {
        key: String,
        raw_value: Option<String>,
        location: DeclarationLocation,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlpmConfigFile {
    pub path: PathBuf,
    pub source: String,
    pub directives: Vec<AlpmDirective>,
}

impl AlpmConfigFile {
    pub fn parse(path: impl Into<PathBuf>, source: &str) -> Result<Self> {
        let path = path.into();
        let mut section: Option<AlpmSection> = None;
        let mut directives = Vec::new();
        for (index, raw_line) in source.lines().enumerate() {
            let line_number = index + 1;
            match lex_line(&path, line_number, raw_line)? {
                None => {}
                Some(AlpmLine::Section(new_section)) => section = Some(new_section),
                Some(AlpmLine::Directive {
                    key,
                    value,
                    location,
                }) => apply_directive(
                    &path,
                    line_number,
                    section.as_ref(),
                    &key,
                    value,
                    location,
                    &mut directives,
                )?,
            }
        }
        Ok(Self {
            path,
            source: source.to_string(),
            directives,
        })
    }

    pub fn render_preserved(&self) -> &str {
        &self.source
    }
}

enum AlpmLine {
    Section(AlpmSection),
    Directive {
        key: String,
        value: Option<String>,
        location: DeclarationLocation,
    },
}

fn lex_line(path: &Path, line_number: usize, raw_line: &str) -> Result<Option<AlpmLine>> {
    let line = raw_line.trim_end_matches('\r').trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    if line.starts_with('[') {
        let name = line
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .ok_or_else(|| {
                DeclarationError::syntax(
                    DeclarationEcosystem::Alpm,
                    path,
                    line_number,
                    "section header must end with ']'",
                )
            })?;
        if name.is_empty() {
            return Err(DeclarationError::syntax(
                DeclarationEcosystem::Alpm,
                path,
                line_number,
                "section name is empty",
            ));
        }
        return Ok(Some(AlpmLine::Section(if name == "options" {
            AlpmSection::Options
        } else {
            AlpmSection::Repository(name.to_string())
        })));
    }
    let (key, value) = match line.split_once('=') {
        Some((key, value)) => (key.trim(), Some(value.trim().to_string())),
        None => (line, None),
    };
    if key.is_empty() {
        return Err(DeclarationError::syntax(
            DeclarationEcosystem::Alpm,
            path,
            line_number,
            "directive name is empty",
        ));
    }
    Ok(Some(AlpmLine::Directive {
        key: key.to_string(),
        value,
        location: DeclarationLocation::new(path, line_number),
    }))
}

fn apply_directive(
    path: &Path,
    line: usize,
    section: Option<&AlpmSection>,
    key: &str,
    value: Option<String>,
    location: DeclarationLocation,
    directives: &mut Vec<AlpmDirective>,
) -> Result<()> {
    if key == "Include" {
        let pattern = required_value(path, line, key, value)?;
        directives.push(AlpmDirective::Include { pattern, location });
        return Ok(());
    }
    match section {
        Some(AlpmSection::Options) => parse_global(path, line, key, value, location, directives),
        Some(AlpmSection::Repository(repository)) => {
            parse_repository(path, line, repository, key, value, location, directives)
        }
        None => Err(DeclarationError::syntax(
            DeclarationEcosystem::Alpm,
            path,
            line,
            "directive appears before the first section",
        )),
    }
}

fn parse_global(
    path: &Path,
    line: usize,
    key: &str,
    value: Option<String>,
    location: DeclarationLocation,
    directives: &mut Vec<AlpmDirective>,
) -> Result<()> {
    match key {
        "Architecture" => {
            let value = required_value(path, line, key, value)?;
            directives.push(AlpmDirective::Architecture {
                values: split_words(&value),
                location,
            });
        }
        "SigLevel" => {
            let value = required_value(path, line, key, value)?;
            let values = split_words(&value);
            validate_siglevel(path, line, &values)?;
            directives.push(AlpmDirective::GlobalSigLevel { values, location });
        }
        _ => directives.push(AlpmDirective::OtherGlobal {
            key: key.to_string(),
            raw_value: value,
            location,
        }),
    }
    Ok(())
}

fn parse_repository(
    path: &Path,
    line: usize,
    repository: &str,
    key: &str,
    value: Option<String>,
    location: DeclarationLocation,
    directives: &mut Vec<AlpmDirective>,
) -> Result<()> {
    let name = match key {
        "CacheServer" => AlpmRepositoryOptionName::CacheServer,
        "Server" => AlpmRepositoryOptionName::Server,
        "SigLevel" => AlpmRepositoryOptionName::SigLevel,
        "Usage" => AlpmRepositoryOptionName::Usage,
        _ => {
            return Err(DeclarationError::unknown(
                DeclarationEcosystem::Alpm,
                path,
                line,
                format!("unmodeled ALPM repository option '{key}'"),
            ));
        }
    };
    let raw_value = required_value(path, line, key, value)?;
    let values = split_words(&raw_value);
    match name {
        AlpmRepositoryOptionName::SigLevel => validate_siglevel(path, line, &values)?,
        AlpmRepositoryOptionName::Usage => validate_usage(path, line, &values)?,
        _ => {}
    }
    directives.push(AlpmDirective::RepositoryOption {
        repository: repository.to_string(),
        name,
        raw_value,
        values,
        location,
    });
    Ok(())
}

fn required_value(path: &Path, line: usize, key: &str, value: Option<String>) -> Result<String> {
    value.filter(|value| !value.is_empty()).ok_or_else(|| {
        DeclarationError::syntax(
            DeclarationEcosystem::Alpm,
            path,
            line,
            format!("directive '{key}' requires a value"),
        )
    })
}

fn validate_siglevel(path: &Path, line: usize, values: &[String]) -> Result<()> {
    for value in values {
        let base = value
            .strip_prefix("Package")
            .or_else(|| value.strip_prefix("Database"))
            .unwrap_or(value);
        if !matches!(
            base,
            "Never" | "Optional" | "Required" | "TrustedOnly" | "TrustAll"
        ) {
            return Err(DeclarationError::unknown(
                DeclarationEcosystem::Alpm,
                path,
                line,
                format!("unmodeled ALPM SigLevel token '{value}'"),
            ));
        }
    }
    Ok(())
}

fn validate_usage(path: &Path, line: usize, values: &[String]) -> Result<()> {
    for value in values {
        if !matches!(
            value.as_str(),
            "Sync" | "Search" | "Install" | "Upgrade" | "All"
        ) {
            return Err(DeclarationError::unknown(
                DeclarationEcosystem::Alpm,
                path,
                line,
                format!("unmodeled ALPM Usage token '{value}'"),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlpmConfigSet {
    /// Files in the exact depth-first order pacman includes them.
    pub files: Vec<AlpmConfigFile>,
    /// Effective directives in exact include-expansion order.
    pub directives: Vec<AlpmDirective>,
}

impl AlpmConfigSet {
    pub fn parse_selected_root(root: &Path, entry_path: &Path) -> Result<Self> {
        let mut files = Vec::new();
        let mut directives = Vec::new();
        let mut stack = HashSet::new();
        let mut section = None;
        parse_file_recursive(
            root,
            entry_path,
            0,
            &mut stack,
            &mut section,
            &mut files,
            &mut directives,
        )?;
        Ok(Self { files, directives })
    }
}

fn parse_file_recursive(
    root: &Path,
    guest_path: &Path,
    depth: usize,
    stack: &mut HashSet<PathBuf>,
    section: &mut Option<AlpmSection>,
    files: &mut Vec<AlpmConfigFile>,
    directives: &mut Vec<AlpmDirective>,
) -> Result<()> {
    if depth >= MAX_INCLUDE_DEPTH {
        return Err(DeclarationError::new(
            DeclarationEcosystem::Alpm,
            DeclarationErrorKind::IncludeDepth,
            guest_path,
            None,
            format!("include nesting exceeds {MAX_INCLUDE_DEPTH}"),
        ));
    }
    if !stack.insert(guest_path.to_path_buf()) {
        return Err(DeclarationError::new(
            DeclarationEcosystem::Alpm,
            DeclarationErrorKind::IncludeDepth,
            guest_path,
            None,
            "include cycle detected",
        ));
    }
    let host_path = rooted_path(root, guest_path)?;
    let bytes = fs::read(&host_path).map_err(|error| {
        DeclarationError::new(
            DeclarationEcosystem::Alpm,
            DeclarationErrorKind::Io,
            guest_path,
            None,
            error.to_string(),
        )
    })?;
    let source = String::from_utf8(bytes).map_err(|error| {
        DeclarationError::new(
            DeclarationEcosystem::Alpm,
            DeclarationErrorKind::InvalidUtf8,
            guest_path,
            None,
            error.to_string(),
        )
    })?;
    let file_index = files.len();
    files.push(AlpmConfigFile {
        path: guest_path.to_path_buf(),
        source,
        directives: Vec::new(),
    });
    let lines: Vec<String> = files[file_index]
        .source
        .lines()
        .map(ToOwned::to_owned)
        .collect();
    for (index, raw_line) in lines.iter().enumerate() {
        let line_number = index + 1;
        match lex_line(guest_path, line_number, raw_line)? {
            None => {}
            Some(AlpmLine::Section(new_section)) => *section = Some(new_section),
            Some(AlpmLine::Directive {
                key,
                value,
                location,
            }) if key == "Include" => {
                let pattern = required_value(guest_path, line_number, &key, value)?;
                let directive = AlpmDirective::Include {
                    pattern: pattern.clone(),
                    location: location.clone(),
                };
                files[file_index].directives.push(directive.clone());
                directives.push(directive);
                for included in expand_include(root, &pattern, &location)? {
                    parse_file_recursive(
                        root,
                        &included,
                        depth + 1,
                        stack,
                        section,
                        files,
                        directives,
                    )?;
                }
            }
            Some(AlpmLine::Directive {
                key,
                value,
                location,
            }) => {
                let mut parsed = Vec::new();
                apply_directive(
                    guest_path,
                    line_number,
                    section.as_ref(),
                    &key,
                    value,
                    location,
                    &mut parsed,
                )?;
                files[file_index].directives.extend(parsed.iter().cloned());
                directives.extend(parsed);
            }
        }
    }
    stack.remove(guest_path);
    Ok(())
}

fn expand_include(
    root: &Path,
    pattern: &str,
    location: &DeclarationLocation,
) -> Result<Vec<PathBuf>> {
    let guest_pattern = PathBuf::from(pattern);
    if guest_pattern
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(DeclarationError::new(
            DeclarationEcosystem::Alpm,
            DeclarationErrorKind::PathEscape,
            &location.path,
            Some(location.line),
            format!("include pattern escapes the selected root: '{pattern}'"),
        ));
    }
    let host_pattern = root.join(guest_pattern.strip_prefix("/").unwrap_or(&guest_pattern));
    let host_pattern = host_pattern.to_string_lossy();
    let entries = glob::glob(&host_pattern).map_err(|error| {
        DeclarationError::syntax(
            DeclarationEcosystem::Alpm,
            &location.path,
            location.line,
            format!("invalid include pattern '{pattern}': {error}"),
        )
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let host_path = entry.map_err(|error| {
            DeclarationError::new(
                DeclarationEcosystem::Alpm,
                DeclarationErrorKind::Io,
                &location.path,
                Some(location.line),
                error.to_string(),
            )
        })?;
        let relative = host_path.strip_prefix(root).map_err(|_| {
            DeclarationError::new(
                DeclarationEcosystem::Alpm,
                DeclarationErrorKind::PathEscape,
                &location.path,
                Some(location.line),
                format!(
                    "include match escapes selected root: {}",
                    host_path.display()
                ),
            )
        })?;
        let guest_path = Path::new("/").join(relative);
        rooted_path(root, &guest_path)?;
        paths.push(guest_path);
    }
    paths.sort();
    if paths.is_empty() {
        return Err(DeclarationError::new(
            DeclarationEcosystem::Alpm,
            DeclarationErrorKind::Io,
            &location.path,
            Some(location.line),
            format!("include pattern matched no files: '{pattern}'"),
        ));
    }
    Ok(paths)
}

fn rooted_path(root: &Path, guest_path: &Path) -> Result<PathBuf> {
    safe_join(root, guest_path).map_err(|error| {
        DeclarationError::new(
            DeclarationEcosystem::Alpm,
            DeclarationErrorKind::PathEscape,
            guest_path,
            None,
            error.to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_variables_order_usage_and_global_options() {
        let source = "# arch\n[options]\nArchitecture = auto\nColor\nSigLevel = Required DatabaseOptional\n[core]\nServer = https://mirror/$repo/os/$arch\nUsage = Sync Install\n";
        let file = AlpmConfigFile::parse("/etc/pacman.conf", source).unwrap();
        assert_eq!(file.render_preserved(), source);
        assert_eq!(file.directives.len(), 5);
    }

    #[test]
    fn unknown_repository_authority_fails_closed() {
        let error =
            AlpmConfigFile::parse("/etc/pacman.conf", "[core]\nFuture=value\n").unwrap_err();
        assert_eq!(error.kind, DeclarationErrorKind::UnknownAuthority);
    }

    #[test]
    fn selected_root_expands_includes_in_sorted_order() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc/pacman.d")).unwrap();
        fs::write(
            root.path().join("etc/pacman.conf"),
            "[options]\nInclude = /etc/pacman.d/*.conf\n",
        )
        .unwrap();
        fs::write(
            root.path().join("etc/pacman.d/b.conf"),
            "[b]\nServer=https://b\n",
        )
        .unwrap();
        fs::write(
            root.path().join("etc/pacman.d/a.conf"),
            "[a]\nServer=https://a\n",
        )
        .unwrap();
        let set =
            AlpmConfigSet::parse_selected_root(root.path(), Path::new("/etc/pacman.conf")).unwrap();
        assert_eq!(set.files.len(), 3);
        assert_eq!(set.files[1].path, Path::new("/etc/pacman.d/a.conf"));
        assert_eq!(set.files[2].path, Path::new("/etc/pacman.d/b.conf"));
    }

    #[test]
    fn included_files_inherit_and_update_the_current_section() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc/pacman.d")).unwrap();
        fs::write(
            root.path().join("etc/pacman.conf"),
            "[core]\nInclude = /etc/pacman.d/vendor.conf\nServer=https://after\n",
        )
        .unwrap();
        fs::write(
            root.path().join("etc/pacman.d/vendor.conf"),
            "Server=https://inherited\n[extra]\nServer=https://extra\n",
        )
        .unwrap();
        let set =
            AlpmConfigSet::parse_selected_root(root.path(), Path::new("/etc/pacman.conf")).unwrap();
        let repositories: Vec<_> = set
            .directives
            .iter()
            .filter_map(|directive| match directive {
                AlpmDirective::RepositoryOption { repository, .. } => Some(repository.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(repositories, ["core", "extra", "extra"]);
    }
}
