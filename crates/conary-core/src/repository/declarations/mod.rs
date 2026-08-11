// conary-core/src/repository/declarations/mod.rs

//! Lossless, ecosystem-owned native repository declarations.
//!
//! These types are discovery inputs. They preserve source syntax and expose
//! source-specific semantics, but they do not import trust, enable a
//! repository, or persist takeover authority.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

pub mod alpm;
pub mod apt;
pub mod discovery;
pub mod dnf;
mod ini;
pub mod zypper;

#[cfg(test)]
mod contract_tests;

pub use discovery::{DiscoveredRepositoryDeclarations, discover_selected_root};

/// The exact native grammar that rejected a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarationEcosystem {
    Apt,
    Dnf,
    ZypperRepository,
    ZypperService,
    Alpm,
}

impl fmt::Display for DeclarationEcosystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Apt => "apt",
            Self::Dnf => "dnf",
            Self::ZypperRepository => "zypper-repository",
            Self::ZypperService => "zypper-service",
            Self::Alpm => "alpm",
        })
    }
}

/// Stable machine-readable declaration failure class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarationErrorKind {
    Io,
    InvalidUtf8,
    Syntax,
    UnknownAuthority,
    PathEscape,
    IncludeDepth,
}

/// A source-attributed native declaration failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarationError {
    pub ecosystem: DeclarationEcosystem,
    pub kind: DeclarationErrorKind,
    pub path: PathBuf,
    pub line: Option<usize>,
    pub message: String,
}

impl DeclarationError {
    pub(crate) fn new(
        ecosystem: DeclarationEcosystem,
        kind: DeclarationErrorKind,
        path: impl Into<PathBuf>,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            ecosystem,
            kind,
            path: path.into(),
            line,
            message: message.into(),
        }
    }

    pub(crate) fn syntax(
        ecosystem: DeclarationEcosystem,
        path: impl Into<PathBuf>,
        line: usize,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            ecosystem,
            DeclarationErrorKind::Syntax,
            path,
            Some(line),
            message,
        )
    }

    pub(crate) fn unknown(
        ecosystem: DeclarationEcosystem,
        path: impl Into<PathBuf>,
        line: usize,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            ecosystem,
            DeclarationErrorKind::UnknownAuthority,
            path,
            Some(line),
            message,
        )
    }
}

impl fmt::Display for DeclarationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} repository declaration {:?} error at {}",
            self.ecosystem,
            self.kind,
            self.path.display()
        )?;
        if let Some(line) = self.line {
            write!(formatter, ":{line}")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

impl std::error::Error for DeclarationError {}

pub type Result<T> = std::result::Result<T, DeclarationError>;

/// Exact source location for one authority-bearing declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarationLocation {
    pub path: PathBuf,
    pub line: usize,
}

impl DeclarationLocation {
    pub(crate) fn new(path: &Path, line: usize) -> Self {
        Self {
            path: path.to_path_buf(),
            line,
        }
    }
}

pub(crate) fn split_words(value: &str) -> Vec<String> {
    value.split_whitespace().map(ToOwned::to_owned).collect()
}

pub(crate) fn parse_bool(
    ecosystem: DeclarationEcosystem,
    path: &Path,
    line: usize,
    value: &str,
) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "yes" | "true" | "on" => Ok(true),
        "0" | "no" | "false" | "off" => Ok(false),
        _ => Err(DeclarationError::syntax(
            ecosystem,
            path,
            line,
            format!("invalid boolean value '{value}'"),
        )),
    }
}
