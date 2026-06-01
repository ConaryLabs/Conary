// conary-core/src/repository/distro.rs

//! Shared distro family and repository version-scheme inference.

use crate::ccs::legacy_scriptlets::{LegacyScriptletBundle, SourceFormat};
use crate::db::models::Repository;
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::registry::{RepositoryFormat, detect_repository_format};
use crate::repository::versioning::VersionScheme;

pub const SUPPORTED_USER_DISTROS: &[&str] = &["fedora-44", "ubuntu-26.04", "arch"];
pub const SUPPORTED_USER_DISTRO_CATALOG: &[SupportedDistro] = &[
    SupportedDistro {
        id: "fedora-44",
        display_name: "Fedora 44",
    },
    SupportedDistro {
        id: "ubuntu-26.04",
        display_name: "Ubuntu 26.04 LTS",
    },
    SupportedDistro {
        id: "arch",
        display_name: "Arch Linux (rolling)",
    },
];
pub const INTERNAL_DISTRO_FAMILIES: &[&str] = &["fedora", "ubuntu", "arch"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportedDistro {
    pub id: &'static str,
    pub display_name: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayTarget<'a> {
    pub format: &'a str,
    pub distro: &'a str,
    pub release: &'a str,
    pub arch: &'a str,
}

impl ReplayTarget<'_> {
    #[must_use]
    pub fn to_id(&self) -> String {
        replay_target_id(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayTargetOwned {
    pub format: String,
    pub distro: String,
    pub release: String,
    pub arch: String,
}

impl ReplayTargetOwned {
    #[must_use]
    pub fn as_target(&self) -> ReplayTarget<'_> {
        ReplayTarget {
            format: &self.format,
            distro: &self.distro,
            release: &self.release,
            arch: &self.arch,
        }
    }

    #[must_use]
    pub fn to_id(&self) -> String {
        self.as_target().to_id()
    }
}

/// Return the user-facing distro catalog supported by this release.
#[must_use]
pub fn supported_user_distros() -> &'static [SupportedDistro] {
    SUPPORTED_USER_DISTRO_CATALOG
}

/// Look up a user-facing supported distro by exact ID.
#[must_use]
pub fn supported_distro(id: &str) -> Option<&'static SupportedDistro> {
    let id = id.trim();
    SUPPORTED_USER_DISTRO_CATALOG
        .iter()
        .find(|distro| distro.id == id)
}

/// Infer the dependency flavor from a supported distro name or internal family
/// label.
#[must_use]
pub fn flavor_from_distro_name(name: &str) -> Option<RepositoryDependencyFlavor> {
    match name.trim().to_ascii_lowercase().as_str() {
        "fedora-44" | "fedora" => Some(RepositoryDependencyFlavor::Rpm),
        "ubuntu-26.04" | "ubuntu" => Some(RepositoryDependencyFlavor::Deb),
        "arch" => Some(RepositoryDependencyFlavor::Arch),
        _ => None,
    }
}

#[must_use]
pub fn replay_target_id(target: &ReplayTarget<'_>) -> String {
    format!(
        "{}/{}/{}/{}",
        target.format, target.distro, target.release, target.arch
    )
}

#[must_use]
pub fn replay_target_from_distro_id(distro_id: &str, arch: &str) -> Option<ReplayTargetOwned> {
    let distro_id = distro_id.trim().to_ascii_lowercase();
    let arch = arch.trim();
    if distro_id.is_empty() || arch.is_empty() {
        return None;
    }

    let (format, distro, release) = match distro_id.as_str() {
        "arch" => ("arch", "arch", "rolling"),
        "fedora" => ("rpm", "fedora", "unknown"),
        "ubuntu" => ("deb", "ubuntu", "unknown"),
        "debian" => ("deb", "debian", "unknown"),
        value => {
            if let Some(release) = value.strip_prefix("fedora-") {
                ("rpm", "fedora", release)
            } else if let Some(release) = value.strip_prefix("ubuntu-") {
                ("deb", "ubuntu", release)
            } else if let Some(release) = value.strip_prefix("debian-") {
                ("deb", "debian", release)
            } else {
                return None;
            }
        }
    };

    if release.trim().is_empty() {
        return None;
    }

    Some(ReplayTargetOwned {
        format: format.to_string(),
        distro: distro.to_string(),
        release: release.to_string(),
        arch: arch.to_string(),
    })
}

#[must_use]
pub fn source_target_from_bundle(bundle: &LegacyScriptletBundle) -> ReplayTargetOwned {
    let format = match &bundle.source_format {
        SourceFormat::Rpm => "rpm".to_string(),
        SourceFormat::Deb => "deb".to_string(),
        SourceFormat::Arch => "arch".to_string(),
        SourceFormat::Unknown(value) => value.trim().to_ascii_lowercase(),
    };
    let distro = bundle
        .source_distro
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&bundle.source_family)
        .trim()
        .to_ascii_lowercase();
    let release = bundle
        .source_release
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if distro == "arch" {
                "rolling".to_string()
            } else {
                "unknown".to_string()
            }
        });
    let arch = bundle
        .source_arch
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string();

    ReplayTargetOwned {
        format,
        distro,
        release,
        arch,
    }
}

/// Infer the dependency flavor from repository metadata shape.
#[must_use]
pub fn flavor_from_repository(repo: &Repository) -> Option<RepositoryDependencyFlavor> {
    repo.default_strategy_distro
        .as_deref()
        .and_then(flavor_from_distro_name)
        .or_else(|| flavor_from_repository_name_url(&repo.name, &repo.url))
}

/// Infer the dependency flavor from repository name and URL detection.
///
/// This preserves metadata-format detection for repository rows without
/// broadening the set of user-facing distro names accepted by
/// [`flavor_from_distro_name`].
#[must_use]
pub fn flavor_from_repository_name_url(
    name: &str,
    url: &str,
) -> Option<RepositoryDependencyFlavor> {
    match detect_repository_format(name, url) {
        RepositoryFormat::Fedora => Some(RepositoryDependencyFlavor::Rpm),
        RepositoryFormat::Debian => Some(RepositoryDependencyFlavor::Deb),
        RepositoryFormat::Arch => Some(RepositoryDependencyFlavor::Arch),
        RepositoryFormat::Json => None,
    }
}

/// Infer a version comparison scheme from a supported distro name or internal
/// family label.
#[must_use]
pub fn version_scheme_from_distro_name(name: &str) -> Option<VersionScheme> {
    flavor_from_distro_name(name).map(flavor_to_version_scheme)
}

/// Infer a version comparison scheme from repository metadata shape.
#[must_use]
pub fn version_scheme_from_repository(repo: &Repository) -> Option<VersionScheme> {
    flavor_from_repository(repo).map(flavor_to_version_scheme)
}

/// Parse a stored DB version-scheme string.
#[must_use]
pub fn version_scheme_from_db(value: Option<&str>) -> Option<VersionScheme> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "rpm" => Some(VersionScheme::Rpm),
        "debian" => Some(VersionScheme::Debian),
        "arch" => Some(VersionScheme::Arch),
        _ => None,
    }
}

/// Parse a stored DB version-scheme string, explicitly defaulting to RPM.
#[must_use]
pub fn version_scheme_or_rpm(value: Option<&str>) -> VersionScheme {
    version_scheme_from_db(value).unwrap_or(VersionScheme::Rpm)
}

/// Check whether a distro name/family label maps to a dependency flavor.
#[must_use]
pub fn flavor_matches_distro_name(name: &str, flavor: RepositoryDependencyFlavor) -> bool {
    flavor_from_distro_name(name) == Some(flavor)
}

/// Convert a dependency flavor to its version comparison scheme.
#[must_use]
pub fn flavor_to_version_scheme(flavor: RepositoryDependencyFlavor) -> VersionScheme {
    flavor.version_scheme()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_user_distro_names_map_to_flavors_and_schemes() {
        let catalog_ids: Vec<_> = supported_user_distros()
            .iter()
            .map(|distro| distro.id)
            .collect();
        assert_eq!(catalog_ids.as_slice(), SUPPORTED_USER_DISTROS);

        for (name, flavor, scheme) in [
            (
                "fedora-44",
                RepositoryDependencyFlavor::Rpm,
                VersionScheme::Rpm,
            ),
            (
                "ubuntu-26.04",
                RepositoryDependencyFlavor::Deb,
                VersionScheme::Debian,
            ),
            (
                "arch",
                RepositoryDependencyFlavor::Arch,
                VersionScheme::Arch,
            ),
        ] {
            assert_eq!(flavor_from_distro_name(name), Some(flavor));
            assert_eq!(version_scheme_from_distro_name(name), Some(scheme));
            assert!(flavor_matches_distro_name(name, flavor));
            assert_eq!(flavor_to_version_scheme(flavor), scheme);
        }
    }

    #[test]
    fn supported_distro_lookup_is_exact_and_narrow() {
        assert_eq!(
            supported_distro("fedora-44").map(|distro| distro.display_name),
            Some("Fedora 44")
        );
        assert_eq!(supported_distro("linux-mint"), None);
        assert_eq!(supported_distro("debian"), None);
    }

    #[test]
    fn internal_family_labels_map_to_flavors_and_schemes() {
        for (name, flavor, scheme) in [
            (
                "fedora",
                RepositoryDependencyFlavor::Rpm,
                VersionScheme::Rpm,
            ),
            (
                "ubuntu",
                RepositoryDependencyFlavor::Deb,
                VersionScheme::Debian,
            ),
            (
                "arch",
                RepositoryDependencyFlavor::Arch,
                VersionScheme::Arch,
            ),
        ] {
            assert_eq!(flavor_from_distro_name(name), Some(flavor));
            assert_eq!(version_scheme_from_distro_name(name), Some(scheme));
        }
    }

    #[test]
    fn replay_target_ids_normalize_known_distro_pins() {
        assert_eq!(
            replay_target_from_distro_id("fedora-44", "x86_64")
                .expect("fedora target")
                .to_id(),
            "rpm/fedora/44/x86_64"
        );
        assert_eq!(
            replay_target_from_distro_id("ubuntu-26.04", "x86_64")
                .expect("ubuntu target")
                .to_id(),
            "deb/ubuntu/26.04/x86_64"
        );
        assert_eq!(
            replay_target_from_distro_id("debian-13", "aarch64")
                .expect("debian target")
                .to_id(),
            "deb/debian/13/aarch64"
        );
        assert_eq!(
            replay_target_from_distro_id("arch", "x86_64")
                .expect("arch target")
                .to_id(),
            "arch/arch/rolling/x86_64"
        );
    }

    #[test]
    fn replay_target_for_generic_family_keeps_release_unknown() {
        assert_eq!(
            replay_target_from_distro_id("fedora", "x86_64")
                .expect("generic fedora target")
                .to_id(),
            "rpm/fedora/unknown/x86_64"
        );
    }

    #[test]
    fn unknown_distro_names_have_no_name_only_inference() {
        for name in ["nixos", "debian", "linux-mint", "ubuntu-noble"] {
            assert_eq!(flavor_from_distro_name(name), None);
            assert_eq!(version_scheme_from_distro_name(name), None);
        }
    }

    #[test]
    fn repository_inference_preserves_metadata_format_detection() {
        let fedora_repo = Repository::new("fedora-base".into(), "https://example.com".into());
        let ubuntu_repo = Repository::new(
            "custom".into(),
            "https://archive.ubuntu.com/ubuntu/dists/resolute".into(),
        );
        let arch_repo = Repository::new(
            "custom".into(),
            "https://mirror.archlinux.org/core/os/x86_64/core.db.tar.gz".into(),
        );
        let unknown_repo = Repository::new("custom".into(), "https://example.com/repo".into());

        assert_eq!(
            flavor_from_repository(&fedora_repo),
            Some(RepositoryDependencyFlavor::Rpm)
        );
        assert_eq!(
            flavor_from_repository(&ubuntu_repo),
            Some(RepositoryDependencyFlavor::Deb)
        );
        assert_eq!(
            flavor_from_repository(&arch_repo),
            Some(RepositoryDependencyFlavor::Arch)
        );
        assert_eq!(flavor_from_repository(&unknown_repo), None);

        assert_eq!(
            version_scheme_from_repository(&ubuntu_repo),
            Some(VersionScheme::Debian)
        );
    }

    #[test]
    fn repository_inference_prefers_explicit_strategy_distro() {
        let mut repo = Repository::new("custom".into(), "https://example.com/repo".into());
        repo.default_strategy_distro = Some("arch".to_string());

        assert_eq!(
            flavor_from_repository(&repo),
            Some(RepositoryDependencyFlavor::Arch)
        );
        assert_eq!(
            version_scheme_from_repository(&repo),
            Some(VersionScheme::Arch)
        );
    }

    #[test]
    fn explicit_db_version_scheme_strings_parse_with_explicit_rpm_fallback() {
        assert_eq!(
            version_scheme_from_db(Some("rpm")),
            Some(VersionScheme::Rpm)
        );
        assert_eq!(
            version_scheme_from_db(Some("debian")),
            Some(VersionScheme::Debian)
        );
        assert_eq!(
            version_scheme_from_db(Some("arch")),
            Some(VersionScheme::Arch)
        );
        assert_eq!(version_scheme_from_db(Some("bogus")), None);
        assert_eq!(version_scheme_from_db(None), None);
        assert_eq!(version_scheme_or_rpm(Some("bogus")), VersionScheme::Rpm);
        assert_eq!(version_scheme_or_rpm(None), VersionScheme::Rpm);
    }
}
