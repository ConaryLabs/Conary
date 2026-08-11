// conary-core/src/repository/declarations/zypper.rs

//! libzypp `.repo` and `.service` declaration authority.

use super::ini::IniDocument;
use super::{
    DeclarationEcosystem, DeclarationError, DeclarationErrorKind, DeclarationLocation, Result,
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZypperRepositoryOptionName {
    Name,
    Enabled,
    Priority,
    Path,
    Type,
    Autorefresh,
    Gpgcheck,
    RepoGpgcheck,
    PkgGpgcheck,
    Keeppackages,
    Service,
    Proxy,
    Baseurl,
    Gpgkey,
    Mirrorlist,
    Metalink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "kebab-case")]
pub enum ZypperRepositoryOption {
    Known {
        name: ZypperRepositoryOptionName,
        raw_value: String,
        location: DeclarationLocation,
    },
    /// libzypp preserves and re-emits unknown repository keys.
    Uninterpreted {
        key: String,
        raw_value: String,
        location: DeclarationLocation,
    },
}

impl ZypperRepositoryOption {
    fn known(&self) -> Option<(ZypperRepositoryOptionName, &str, &DeclarationLocation)> {
        match self {
            Self::Known {
                name,
                raw_value,
                location,
            } => Some((*name, raw_value, location)),
            Self::Uninterpreted { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZypperRepository {
    pub alias: String,
    pub options: Vec<ZypperRepositoryOption>,
    pub location: DeclarationLocation,
}

impl ZypperRepository {
    pub fn has_uninterpreted_authority(&self) -> bool {
        self.options
            .iter()
            .any(|option| matches!(option, ZypperRepositoryOption::Uninterpreted { .. }))
    }

    pub fn explicit_enabled(&self) -> Result<Option<bool>> {
        Ok(self
            .options
            .iter()
            .rev()
            .filter_map(ZypperRepositoryOption::known)
            .find(|(name, _, _)| *name == ZypperRepositoryOptionName::Enabled)
            .map(|(_, value, _)| zypper_true(value)))
    }

    pub fn require_interpreted_authority(&self) -> Result<()> {
        if let Some(ZypperRepositoryOption::Uninterpreted { key, location, .. }) = self
            .options
            .iter()
            .find(|option| matches!(option, ZypperRepositoryOption::Uninterpreted { .. }))
        {
            return Err(DeclarationError::unknown(
                DeclarationEcosystem::ZypperRepository,
                &location.path,
                location.line,
                format!("unmodeled libzypp repository option '{key}'"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZypperRepoDocument {
    pub path: PathBuf,
    pub source: String,
    pub repositories: Vec<ZypperRepository>,
}

impl ZypperRepoDocument {
    pub fn parse(path: impl Into<PathBuf>, source: &str) -> Result<Self> {
        let path = path.into();
        let ini = IniDocument::parse(DeclarationEcosystem::ZypperRepository, &path, source)?;
        let mut repositories: Vec<ZypperRepository> = Vec::new();
        for section in ini.sections {
            let repository = if let Some(index) = repositories
                .iter()
                .position(|repository| repository.alias == section.name)
            {
                &mut repositories[index]
            } else {
                repositories.push(ZypperRepository {
                    alias: section.name,
                    options: Vec::new(),
                    location: section.location,
                });
                repositories.last_mut().expect("repository was pushed")
            };
            for entry in section.entries {
                let option = match repository_option_name(&entry.key) {
                    Some(name) => ZypperRepositoryOption::Known {
                        name,
                        raw_value: entry.value,
                        location: entry.location,
                    },
                    None => ZypperRepositoryOption::Uninterpreted {
                        key: entry.key,
                        raw_value: entry.value,
                        location: entry.location,
                    },
                };
                repository.options.push(option);
            }
        }
        for repository in &repositories {
            validate_repository(repository)?;
        }
        Ok(Self {
            path: ini.path,
            source: ini.source,
            repositories,
        })
    }

    pub fn render_preserved(&self) -> &str {
        &self.source
    }
}

fn validate_repository(repository: &ZypperRepository) -> Result<()> {
    for (name, value, location) in repository
        .options
        .iter()
        .filter_map(ZypperRepositoryOption::known)
    {
        if name == ZypperRepositoryOptionName::Path
            && Path::new(value)
                .components()
                .any(|component| component == Component::ParentDir)
        {
            return Err(DeclarationError::new(
                DeclarationEcosystem::ZypperRepository,
                DeclarationErrorKind::PathEscape,
                &location.path,
                Some(location.line),
                format!("repository path contains a parent component: '{value}'"),
            ));
        }
        if name == ZypperRepositoryOptionName::Priority {
            value.parse::<u32>().map_err(|_| {
                DeclarationError::syntax(
                    DeclarationEcosystem::ZypperRepository,
                    &location.path,
                    location.line,
                    format!("invalid priority value '{value}'"),
                )
            })?;
        }
    }
    Ok(())
}

fn repository_option_name(key: &str) -> Option<ZypperRepositoryOptionName> {
    Some(match key {
        "name" => ZypperRepositoryOptionName::Name,
        "enabled" => ZypperRepositoryOptionName::Enabled,
        "priority" => ZypperRepositoryOptionName::Priority,
        "path" => ZypperRepositoryOptionName::Path,
        "type" => ZypperRepositoryOptionName::Type,
        "autorefresh" => ZypperRepositoryOptionName::Autorefresh,
        "gpgcheck" => ZypperRepositoryOptionName::Gpgcheck,
        "repo_gpgcheck" => ZypperRepositoryOptionName::RepoGpgcheck,
        "pkg_gpgcheck" => ZypperRepositoryOptionName::PkgGpgcheck,
        "keeppackages" => ZypperRepositoryOptionName::Keeppackages,
        "service" => ZypperRepositoryOptionName::Service,
        "proxy" => ZypperRepositoryOptionName::Proxy,
        "baseurl" => ZypperRepositoryOptionName::Baseurl,
        "gpgkey" => ZypperRepositoryOptionName::Gpgkey,
        "mirrorlist" => ZypperRepositoryOptionName::Mirrorlist,
        "metalink" => ZypperRepositoryOptionName::Metalink,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZypperServiceOptionName {
    Name,
    Url,
    Enabled,
    Autorefresh,
    Type,
    TtlSec,
    LrfDat,
    ReposToEnable,
    ReposToDisable,
    RepositoryUrl { index: u32 },
    RepositoryEnabled { index: u32 },
    RepositoryAutorefresh { index: u32 },
    RepositoryPriority { index: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZypperServiceOption {
    pub name: ZypperServiceOptionName,
    pub raw_value: String,
    pub location: DeclarationLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZypperService {
    pub alias: String,
    pub options: Vec<ZypperServiceOption>,
    pub location: DeclarationLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZypperServiceDocument {
    pub path: PathBuf,
    pub source: String,
    pub services: Vec<ZypperService>,
}

impl ZypperServiceDocument {
    pub fn parse(path: impl Into<PathBuf>, source: &str) -> Result<Self> {
        let path = path.into();
        let ini = IniDocument::parse(DeclarationEcosystem::ZypperService, &path, source)?;
        let mut services: Vec<ZypperService> = Vec::new();
        for section in ini.sections {
            let service = if let Some(index) = services
                .iter()
                .position(|service| service.alias == section.name)
            {
                &mut services[index]
            } else {
                services.push(ZypperService {
                    alias: section.name,
                    options: Vec::new(),
                    location: section.location,
                });
                services.last_mut().expect("service was pushed")
            };
            for entry in section.entries {
                let name = service_option_name(&path, &entry.key, entry.location.line)?;
                if matches!(
                    name,
                    ZypperServiceOptionName::TtlSec
                        | ZypperServiceOptionName::RepositoryPriority { .. }
                ) {
                    entry.value.parse::<u64>().map_err(|_| {
                        DeclarationError::syntax(
                            DeclarationEcosystem::ZypperService,
                            &entry.location.path,
                            entry.location.line,
                            format!("invalid numeric value '{}'", entry.value),
                        )
                    })?;
                }
                service.options.push(ZypperServiceOption {
                    name,
                    raw_value: entry.value,
                    location: entry.location,
                });
            }
        }
        Ok(Self {
            path: ini.path,
            source: ini.source,
            services,
        })
    }

    pub fn render_preserved(&self) -> &str {
        &self.source
    }
}

fn service_option_name(path: &Path, key: &str, line: usize) -> Result<ZypperServiceOptionName> {
    let fixed = match key {
        "name" => Some(ZypperServiceOptionName::Name),
        "url" => Some(ZypperServiceOptionName::Url),
        "enabled" => Some(ZypperServiceOptionName::Enabled),
        "autorefresh" => Some(ZypperServiceOptionName::Autorefresh),
        "type" => Some(ZypperServiceOptionName::Type),
        "ttl_sec" => Some(ZypperServiceOptionName::TtlSec),
        "lrf_dat" => Some(ZypperServiceOptionName::LrfDat),
        "repostoenable" => Some(ZypperServiceOptionName::ReposToEnable),
        "repostodisable" => Some(ZypperServiceOptionName::ReposToDisable),
        _ => None,
    };
    if let Some(name) = fixed {
        return Ok(name);
    }
    if let Some(dynamic) = key.strip_prefix("repo_") {
        let (index, suffix) = dynamic
            .split_once('_')
            .map_or((dynamic, ""), |(index, suffix)| (index, suffix));
        if let Ok(index) = index.parse::<u32>() {
            let name = match suffix {
                "" => Some(ZypperServiceOptionName::RepositoryUrl { index }),
                "enabled" => Some(ZypperServiceOptionName::RepositoryEnabled { index }),
                "autorefresh" => Some(ZypperServiceOptionName::RepositoryAutorefresh { index }),
                "priority" => Some(ZypperServiceOptionName::RepositoryPriority { index }),
                _ => None,
            };
            if let Some(name) = name {
                return Ok(name);
            }
        }
    }
    Err(DeclarationError::unknown(
        DeclarationEcosystem::ZypperService,
        path,
        line,
        format!("unmodeled libzypp service option '{key}'"),
    ))
}

fn zypper_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "yes" | "true" | "always" | "on" | "+"
    ) || value.trim().parse::<i64>().is_ok_and(|number| number != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_preserves_repeatable_urls_and_upstream_extras() {
        let source = "# vendor\n[vendor]\nenabled=0\nbaseurl=https://one\nbaseurl=https://two\nx-vendor=value\n";
        let document = ZypperRepoDocument::parse("/etc/zypp/repos.d/vendor.repo", source).unwrap();
        assert_eq!(document.render_preserved(), source);
        assert_eq!(
            document.repositories[0].explicit_enabled().unwrap(),
            Some(false)
        );
        assert!(document.repositories[0].has_uninterpreted_authority());
    }

    #[test]
    fn service_models_dynamic_repository_fields() {
        let source = "[vendor]\nurl=https://service\nrepo_1=https://repo\nrepo_1_enabled=yes\n";
        let document =
            ZypperServiceDocument::parse("/etc/zypp/services.d/vendor.service", source).unwrap();
        assert_eq!(document.render_preserved(), source);
        assert!(matches!(
            document.services[0].options[2].name,
            ZypperServiceOptionName::RepositoryEnabled { index: 1 }
        ));
    }

    #[test]
    fn unknown_service_authority_fails_closed() {
        let error = ZypperServiceDocument::parse(
            "/etc/zypp/services.d/vendor.service",
            "[vendor]\nfuture=value\n",
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            super::super::DeclarationErrorKind::UnknownAuthority
        );
    }
}
