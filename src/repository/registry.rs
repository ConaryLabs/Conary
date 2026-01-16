// src/repository/registry.rs

//! Repository format registry and detection
//!
//! Provides centralized detection and creation of repository parsers.

use crate::error::{Error, Result};
use crate::repository::parsers::{self, RepositoryParser};

/// Detected repository format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryFormat {
    Arch,
    Debian,
    Fedora,
    Json,
}

/// Detect repository format based on name and URL
pub fn detect_repository_format(name: &str, url: &str) -> RepositoryFormat {
    let name_lower = name.to_lowercase();
    let url_lower = url.to_lowercase();

    // Check for Arch Linux indicators
    if name_lower.contains("arch")
        || url_lower.contains("archlinux")
        || url_lower.contains("pkgbuild")
        || url_lower.contains(".db.tar")
    {
        return RepositoryFormat::Arch;
    }

    // Check for Fedora indicators
    if name_lower.contains("fedora")
        || url_lower.contains("fedora")
        || url_lower.contains("/repodata/")
    {
        return RepositoryFormat::Fedora;
    }

    // Check for Debian/Ubuntu indicators
    if name_lower.contains("debian")
        || name_lower.contains("ubuntu")
        || url_lower.contains("debian")
        || url_lower.contains("ubuntu")
        || url_lower.contains("/dists/")
    {
        return RepositoryFormat::Debian;
    }

    // Default to JSON format
    RepositoryFormat::Json
}

/// Create a parser for the given format and repository info
pub fn create_parser(
    format: RepositoryFormat,
    repo_name: &str,
    _repo_url: &str,
) -> Result<Box<dyn RepositoryParser>> {
    match format {
        RepositoryFormat::Arch => {
            let name = if let Some(suffix) = repo_name.strip_prefix("arch-") {
                suffix.to_string()
            } else {
                "core".to_string()
            };
            Ok(Box::new(parsers::arch::ArchParser::new(name)))
        }
        RepositoryFormat::Debian => {
            let distribution = if let Some(suffix) = repo_name.strip_prefix("ubuntu-") {
                suffix.to_string()
            } else if let Some(suffix) = repo_name.strip_prefix("debian-") {
                suffix.to_string()
            } else {
                "noble".to_string()
            };

            Ok(Box::new(parsers::debian::DebianParser::new(
                distribution,
                "main".to_string(),
                "amd64".to_string(),
            )))
        }
        RepositoryFormat::Fedora => {
            Ok(Box::new(parsers::fedora::FedoraParser::new("x86_64".to_string())))
        }
        RepositoryFormat::Json => {
            Err(Error::ParseError("JSON format has no native parser".to_string()))
        }
    }
}
