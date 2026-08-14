// conary-core/src/repository/declarations/dnf.rs

//! DNF5/YUM `.repo` declaration authority.

use super::ini::IniDocument;
use super::{DeclarationEcosystem, DeclarationError, DeclarationLocation, Result, parse_bool};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnfOptionName {
    Name,
    Enabled,
    Cachedir,
    Baseurl,
    Mirrorlist,
    Metalink,
    Type,
    Mediaid,
    Gpgkey,
    Excludepkgs,
    Exclude,
    Includepkgs,
    Fastestmirror,
    Proxy,
    ProxyUsername,
    ProxyPassword,
    ProxyAuthMethod,
    Username,
    Password,
    ProtectedPackages,
    PkgGpgcheck,
    Gpgcheck,
    RepoGpgcheck,
    Enablegroups,
    Retries,
    Bandwidth,
    Minrate,
    IpResolve,
    Throttle,
    Timeout,
    MaxParallelDownloads,
    MaxDownloadsPerMirror,
    MetadataExpire,
    Cost,
    Priority,
    ModuleHotfixes,
    Sslcacert,
    Sslverify,
    Sslclientcert,
    Sslclientkey,
    ProxySslcacert,
    ProxySslverify,
    ProxySslclientcert,
    ProxySslclientkey,
    Deltarpm,
    DeltarpmPercentage,
    SkipIfUnavailable,
    EnabledMetadata,
    UserAgent,
    Countme,
    BuildCache,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnfOption {
    pub name: DnfOptionName,
    pub raw_value: String,
    pub location: DeclarationLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnfRepository {
    pub id: String,
    pub options: Vec<DnfOption>,
    pub location: DeclarationLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnfEndpointKind {
    Metalink,
    Mirrorlist,
    Baseurl,
}

impl DnfRepository {
    /// DNF defaults to enabled when the declaration has no explicit value.
    pub fn explicit_enabled(&self) -> Result<Option<bool>> {
        let option = self
            .options
            .iter()
            .rev()
            .find(|option| option.name == DnfOptionName::Enabled);
        option
            .map(|option| {
                parse_bool(
                    DeclarationEcosystem::Dnf,
                    &option.location.path,
                    option.location.line,
                    &option.raw_value,
                )
            })
            .transpose()
    }

    pub fn endpoint_values(&self) -> impl Iterator<Item = &DnfOption> {
        self.options.iter().filter(|option| {
            matches!(
                option.name,
                DnfOptionName::Baseurl | DnfOptionName::Mirrorlist | DnfOptionName::Metalink
            )
        })
    }

    /// Mirrors DNF5's endpoint precedence: metalink, mirrorlist, then baseurl.
    pub fn effective_endpoint(&self) -> Option<(DnfEndpointKind, &str)> {
        [
            (DnfEndpointKind::Metalink, DnfOptionName::Metalink),
            (DnfEndpointKind::Mirrorlist, DnfOptionName::Mirrorlist),
            (DnfEndpointKind::Baseurl, DnfOptionName::Baseurl),
        ]
        .into_iter()
        .find_map(|(kind, name)| {
            self.options
                .iter()
                .rev()
                .find(|option| option.name == name && !option.raw_value.trim().is_empty())
                .and_then(|option| {
                    let value = if kind == DnfEndpointKind::Baseurl {
                        option.raw_value.split_whitespace().next()
                    } else {
                        Some(option.raw_value.trim())
                    }?;
                    Some((kind, value))
                })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnfRepoDocument {
    pub path: PathBuf,
    pub source: String,
    pub repositories: Vec<DnfRepository>,
}

impl DnfRepoDocument {
    pub fn parse(path: impl Into<PathBuf>, source: &str) -> Result<Self> {
        let path = path.into();
        let ini = IniDocument::parse(DeclarationEcosystem::Dnf, &path, source)?;
        let mut repositories: Vec<DnfRepository> = Vec::new();
        for section in ini.sections {
            let target = if let Some(index) = repositories
                .iter()
                .position(|repository| repository.id == section.name)
            {
                &mut repositories[index]
            } else {
                repositories.push(DnfRepository {
                    id: section.name,
                    options: Vec::new(),
                    location: section.location,
                });
                repositories.last_mut().expect("repository was pushed")
            };
            for entry in section.entries {
                target.options.push(DnfOption {
                    name: option_name(&path, &entry.key, entry.location.line)?,
                    raw_value: entry.value,
                    location: entry.location,
                });
            }
        }
        for repository in &repositories {
            validate_booleans(repository)?;
            validate_numeric(repository, DnfOptionName::Priority, "priority")?;
            validate_numeric(repository, DnfOptionName::Cost, "cost")?;
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

fn validate_numeric(repository: &DnfRepository, name: DnfOptionName, label: &str) -> Result<()> {
    for option in repository
        .options
        .iter()
        .filter(|option| option.name == name)
    {
        option.raw_value.parse::<i32>().map_err(|_| {
            DeclarationError::syntax(
                DeclarationEcosystem::Dnf,
                &option.location.path,
                option.location.line,
                format!("invalid {label} value '{}'", option.raw_value),
            )
        })?;
    }
    Ok(())
}

fn validate_booleans(repository: &DnfRepository) -> Result<()> {
    for option in &repository.options {
        if matches!(
            option.name,
            DnfOptionName::Enabled
                | DnfOptionName::Fastestmirror
                | DnfOptionName::PkgGpgcheck
                | DnfOptionName::Gpgcheck
                | DnfOptionName::RepoGpgcheck
                | DnfOptionName::Enablegroups
                | DnfOptionName::ModuleHotfixes
                | DnfOptionName::Sslverify
                | DnfOptionName::ProxySslverify
                | DnfOptionName::Deltarpm
                | DnfOptionName::SkipIfUnavailable
                | DnfOptionName::Countme
                | DnfOptionName::BuildCache
        ) {
            parse_bool(
                DeclarationEcosystem::Dnf,
                &option.location.path,
                option.location.line,
                &option.raw_value,
            )?;
        }
    }
    Ok(())
}

fn option_name(path: &Path, key: &str, line: usize) -> Result<DnfOptionName> {
    let name = match key {
        "name" => DnfOptionName::Name,
        "enabled" => DnfOptionName::Enabled,
        "cachedir" => DnfOptionName::Cachedir,
        "baseurl" => DnfOptionName::Baseurl,
        "mirrorlist" => DnfOptionName::Mirrorlist,
        "metalink" => DnfOptionName::Metalink,
        "type" => DnfOptionName::Type,
        "mediaid" => DnfOptionName::Mediaid,
        "gpgkey" => DnfOptionName::Gpgkey,
        "excludepkgs" => DnfOptionName::Excludepkgs,
        "exclude" => DnfOptionName::Exclude,
        "includepkgs" => DnfOptionName::Includepkgs,
        "fastestmirror" => DnfOptionName::Fastestmirror,
        "proxy" => DnfOptionName::Proxy,
        "proxy_username" => DnfOptionName::ProxyUsername,
        "proxy_password" => DnfOptionName::ProxyPassword,
        "proxy_auth_method" => DnfOptionName::ProxyAuthMethod,
        "username" => DnfOptionName::Username,
        "password" => DnfOptionName::Password,
        "protected_packages" => DnfOptionName::ProtectedPackages,
        "pkg_gpgcheck" => DnfOptionName::PkgGpgcheck,
        "gpgcheck" => DnfOptionName::Gpgcheck,
        "repo_gpgcheck" => DnfOptionName::RepoGpgcheck,
        "enablegroups" => DnfOptionName::Enablegroups,
        "retries" => DnfOptionName::Retries,
        "bandwidth" => DnfOptionName::Bandwidth,
        "minrate" => DnfOptionName::Minrate,
        "ip_resolve" => DnfOptionName::IpResolve,
        "throttle" => DnfOptionName::Throttle,
        "timeout" => DnfOptionName::Timeout,
        "max_parallel_downloads" => DnfOptionName::MaxParallelDownloads,
        "max_downloads_per_mirror" => DnfOptionName::MaxDownloadsPerMirror,
        "metadata_expire" => DnfOptionName::MetadataExpire,
        "cost" => DnfOptionName::Cost,
        "priority" => DnfOptionName::Priority,
        "module_hotfixes" => DnfOptionName::ModuleHotfixes,
        "sslcacert" => DnfOptionName::Sslcacert,
        "sslverify" => DnfOptionName::Sslverify,
        "sslclientcert" => DnfOptionName::Sslclientcert,
        "sslclientkey" => DnfOptionName::Sslclientkey,
        "proxy_sslcacert" => DnfOptionName::ProxySslcacert,
        "proxy_sslverify" => DnfOptionName::ProxySslverify,
        "proxy_sslclientcert" => DnfOptionName::ProxySslclientcert,
        "proxy_sslclientkey" => DnfOptionName::ProxySslclientkey,
        "deltarpm" => DnfOptionName::Deltarpm,
        "deltarpm_percentage" => DnfOptionName::DeltarpmPercentage,
        "skip_if_unavailable" => DnfOptionName::SkipIfUnavailable,
        "enabled_metadata" => DnfOptionName::EnabledMetadata,
        "user_agent" => DnfOptionName::UserAgent,
        "countme" => DnfOptionName::Countme,
        "build_cache" => DnfOptionName::BuildCache,
        _ => {
            return Err(DeclarationError::unknown(
                DeclarationEcosystem::Dnf,
                path,
                line,
                format!("unmodeled DNF repository option '{key}'"),
            ));
        }
    };
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_comments_continuations_duplicates_and_disabled_state() {
        let source = "# vendor\n[vendor]\nname = Vendor $releasever\nenabled = 0\nbaseurl = https://one.example/$basearch\n https://two.example/$basearch\nexclude = old-*\nexclude = broken-*\npriority = 20\n";
        let document = DnfRepoDocument::parse("/etc/yum.repos.d/vendor.repo", source).unwrap();
        assert_eq!(document.render_preserved(), source);
        assert_eq!(document.repositories.len(), 1);
        assert_eq!(
            document.repositories[0].explicit_enabled().unwrap(),
            Some(false)
        );
        assert_eq!(
            document.repositories[0]
                .options
                .iter()
                .filter(|option| matches!(option.name, DnfOptionName::Exclude))
                .count(),
            2
        );
        assert!(document.repositories[0].options[2].raw_value.contains('\n'));
        assert_eq!(
            document.repositories[0].effective_endpoint(),
            Some((DnfEndpointKind::Baseurl, "https://one.example/$basearch"))
        );
    }

    #[test]
    fn unknown_option_fails_closed() {
        let error =
            DnfRepoDocument::parse("/etc/yum.repos.d/a.repo", "[a]\nfuture=1\n").unwrap_err();
        assert_eq!(
            error.kind,
            super::super::DeclarationErrorKind::UnknownAuthority
        );
    }

    #[test]
    fn endpoint_precedence_matches_dnf5() {
        let document = DnfRepoDocument::parse(
            "/etc/yum.repos.d/a.repo",
            "[a]\nbaseurl=https://a\nmetalink=https://m\n",
        )
        .unwrap();
        assert_eq!(
            document.repositories[0].effective_endpoint().unwrap().0,
            DnfEndpointKind::Metalink
        );
    }
}
