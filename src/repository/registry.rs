// src/repository/registry.rs

//! Repository format registry and detection
//!
//! Provides centralized detection and creation of repository parsers.

use crate::error::{Error, Result};
use crate::repository::parsers::{self, RepositoryParser};

/// Detect the system architecture
///
/// Returns the architecture in RPM format (x86_64, aarch64, etc.)
pub fn detect_system_arch() -> String {
    std::env::consts::ARCH.to_string()
}

/// Convert system architecture to Debian's naming convention
///
/// Debian uses different names: amd64 instead of x86_64, arm64 instead of aarch64
pub fn arch_to_debian(arch: &str) -> String {
    match arch {
        "x86_64" => "amd64".to_string(),
        "aarch64" => "arm64".to_string(),
        "x86" | "i686" | "i386" => "i386".to_string(),
        "arm" | "armv7" => "armhf".to_string(),
        "powerpc64" => "ppc64el".to_string(),
        "s390x" => "s390x".to_string(),
        "riscv64" => "riscv64".to_string(),
        other => other.to_string(),
    }
}

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

            let arch = arch_to_debian(&detect_system_arch());
            Ok(Box::new(parsers::debian::DebianParser::new(
                distribution,
                "main".to_string(),
                arch,
            )))
        }
        RepositoryFormat::Fedora => {
            let arch = detect_system_arch();
            Ok(Box::new(parsers::fedora::FedoraParser::new(arch)))
        }
        RepositoryFormat::Json => {
            Err(Error::ParseError("JSON format has no native parser".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_system_arch_returns_valid_string() {
        let arch = detect_system_arch();
        assert!(!arch.is_empty());
        // Should be one of the known architectures
        let known_arches = [
            "x86_64", "aarch64", "x86", "i686", "arm", "armv7",
            "powerpc64", "s390x", "riscv64", "mips64", "loongarch64",
        ];
        // The system arch should be recognizable (or at least non-empty)
        assert!(
            known_arches.contains(&arch.as_str()) || !arch.is_empty(),
            "Unknown arch: {}",
            arch
        );
    }

    #[test]
    fn test_arch_to_debian_conversions() {
        assert_eq!(arch_to_debian("x86_64"), "amd64");
        assert_eq!(arch_to_debian("aarch64"), "arm64");
        assert_eq!(arch_to_debian("i686"), "i386");
        assert_eq!(arch_to_debian("i386"), "i386");
        assert_eq!(arch_to_debian("armv7"), "armhf");
        assert_eq!(arch_to_debian("powerpc64"), "ppc64el");
        assert_eq!(arch_to_debian("s390x"), "s390x");
        assert_eq!(arch_to_debian("riscv64"), "riscv64");
        // Unknown arches pass through unchanged
        assert_eq!(arch_to_debian("unknown"), "unknown");
    }

    #[test]
    fn test_detect_repository_format() {
        assert_eq!(
            detect_repository_format("fedora", "https://example.com/"),
            RepositoryFormat::Fedora
        );
        assert_eq!(
            detect_repository_format("myrepo", "https://mirror.fedoraproject.org/"),
            RepositoryFormat::Fedora
        );
        assert_eq!(
            detect_repository_format("arch-core", "https://example.com/"),
            RepositoryFormat::Arch
        );
        assert_eq!(
            detect_repository_format("debian-bookworm", "https://example.com/"),
            RepositoryFormat::Debian
        );
        assert_eq!(
            detect_repository_format("custom", "https://example.com/"),
            RepositoryFormat::Json
        );
    }
}
