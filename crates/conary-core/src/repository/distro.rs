// conary-core/src/repository/distro.rs

//! Shared distro family and repository version-scheme inference.

use crate::ccs::legacy_scriptlets::{LegacyScriptletBundle, SourceFormat};
use crate::db::models::Repository;
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::registry::{RepositoryFormat, detect_repository_format};
use crate::repository::versioning::VersionScheme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedDistro {
    pub id: String,
    pub display_name: String,
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
pub fn supported_user_distros() -> Vec<SupportedDistro> {
    crate::repository::supported_profiles::public_profiles()
        .iter()
        .map(|profile| SupportedDistro {
            id: profile.id().to_string(),
            display_name: profile.display_name().to_string(),
        })
        .collect()
}

/// Look up a user-facing supported distro by exact ID.
#[must_use]
pub fn supported_distro(id: &str) -> Option<SupportedDistro> {
    crate::repository::supported_profiles::profile_by_public_id(id).map(|profile| SupportedDistro {
        id: profile.id().to_string(),
        display_name: profile.display_name().to_string(),
    })
}

/// Infer the dependency flavor from a supported distro name or internal family
/// label.
#[must_use]
pub fn flavor_from_distro_name(name: &str) -> Option<RepositoryDependencyFlavor> {
    crate::repository::supported_profiles::dependency_flavor_for_name(name)
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
    crate::repository::supported_profiles::replay_target_for_public_id(distro_id, arch)
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
    crate::repository::supported_profiles::version_scheme_for_name(name)
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
            .into_iter()
            .map(|distro| distro.id)
            .collect();
        assert_eq!(catalog_ids, vec!["fedora-44", "ubuntu-26.04", "arch"]);

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
            Some("Fedora 44".to_string())
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
    fn replay_target_only_accepts_public_profile_ids() {
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
            replay_target_from_distro_id("arch", "x86_64")
                .expect("arch target")
                .to_id(),
            "arch/arch/rolling/x86_64"
        );
    }

    #[test]
    fn replay_target_rejects_non_public_legacy_normalization() {
        for name in ["fedora", "ubuntu", "debian", "debian-13", "linux-mint"] {
            assert_eq!(replay_target_from_distro_id(name, "x86_64"), None, "{name}");
        }
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
