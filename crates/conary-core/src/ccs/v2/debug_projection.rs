// conary-core/src/ccs/v2/debug_projection.rs

use super::schema::{
    AuthorityDocumentV2, ConfigPolicyV2, LifecycleAuthorityV2, PackageDataV2, PackageKindV2,
};
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};

pub fn validate_debug_toml_projection(
    authority: &AuthorityDocumentV2,
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    validate_config_projection(authority, manifest)?;
    validate_lifecycle_projection(authority, manifest)?;
    Ok(())
}

pub(crate) fn reject_unsupported_debug_toml_install_authority(
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    let mut unsupported = Vec::new();
    if !manifest.requires.packages.is_empty() || !manifest.requires.capabilities.is_empty() {
        unsupported.push("dependencies");
    }
    if manifest.hooks.has_script_hooks() {
        unsupported.push("script hooks");
    }
    if manifest.scriptlets.has_capability_declarations() {
        unsupported.push("scriptlet capabilities");
    }
    if manifest.legacy_scriptlets.is_some() {
        unsupported.push("legacy scriptlets");
    }
    if !manifest.components.overrides.is_empty() || !manifest.components.files.is_empty() {
        unsupported.push("component overrides");
    }
    if !unsupported.is_empty() {
        bail!(
            "v2 debug TOML contains unsupported install authority fields: {}",
            unsupported.join(", ")
        );
    }
    Ok(())
}

fn validate_config_projection(
    authority: &AuthorityDocumentV2,
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    let debug_config = debug_config_projection(manifest);
    let signed_config = match &authority.kind {
        PackageKindV2::Package(package) => signed_config_projection(package),
        _ if debug_config.is_empty() => BTreeMap::new(),
        _ => bail!("debug TOML config projection mismatch for non-package authority"),
    };

    if debug_config != signed_config {
        bail!(
            "debug TOML config projection mismatch: debug TOML [{}], signed [{}]",
            joined_keys(&debug_config),
            joined_keys(&signed_config)
        );
    }

    if let PackageKindV2::Package(package) = &authority.kind {
        for (path, policy) in &debug_config {
            let Some(file) = package.files.iter().find(|file| file.path == *path) else {
                bail!("debug TOML config path {path} missing from signed file authority");
            };
            if file.config != Some(*policy) {
                bail!("debug TOML config path {path} policy does not match signed file authority");
            }
        }
        for file in &package.files {
            if file.config.is_some() && !debug_config.contains_key(&file.path) {
                bail!(
                    "debug TOML config projection missing signed file authority path {}",
                    file.path
                );
            }
        }
    }

    Ok(())
}

fn debug_config_projection(
    manifest: &crate::ccs::manifest::CcsManifest,
) -> BTreeMap<String, ConfigPolicyV2> {
    let policy = if manifest.config.noreplace {
        ConfigPolicyV2::NoReplace
    } else {
        ConfigPolicyV2::Replace
    };
    manifest
        .config
        .files
        .iter()
        .map(|path| (path.clone(), policy))
        .collect()
}

fn signed_config_projection(package: &PackageDataV2) -> BTreeMap<String, ConfigPolicyV2> {
    package
        .config
        .iter()
        .map(|entry| (entry.path.clone(), entry.policy))
        .collect()
}

fn validate_lifecycle_projection(
    authority: &AuthorityDocumentV2,
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    let debug = debug_lifecycle_projection(manifest);
    compare_lifecycle_category("services", debug.services, &authority.lifecycle.services)?;
    compare_lifecycle_category("tmpfiles", debug.tmpfiles, &authority.lifecycle.tmpfiles)?;
    compare_lifecycle_category("sysctl", debug.sysctl, &authority.lifecycle.sysctl)?;
    compare_lifecycle_category("users", debug.users, &authority.lifecycle.users)?;
    compare_lifecycle_category("groups", debug.groups, &authority.lifecycle.groups)?;
    compare_lifecycle_category(
        "directories",
        debug.directories,
        &authority.lifecycle.directories,
    )?;
    compare_lifecycle_category(
        "alternatives",
        debug.alternatives,
        &authority.lifecycle.alternatives,
    )?;
    Ok(())
}

fn debug_lifecycle_projection(
    manifest: &crate::ccs::manifest::CcsManifest,
) -> LifecycleAuthorityV2 {
    LifecycleAuthorityV2 {
        services: manifest
            .hooks
            .services
            .iter()
            .map(|service| service.name.clone())
            .chain(
                manifest
                    .hooks
                    .systemd
                    .iter()
                    .map(|service| service.unit.clone()),
            )
            .collect(),
        tmpfiles: manifest
            .hooks
            .tmpfiles
            .iter()
            .map(|entry| entry.path.clone())
            .collect(),
        sysctl: manifest
            .hooks
            .sysctl
            .iter()
            .map(|entry| entry.key.clone())
            .collect(),
        users: manifest
            .hooks
            .users
            .iter()
            .map(|entry| entry.name.clone())
            .collect(),
        groups: manifest
            .hooks
            .groups
            .iter()
            .map(|entry| entry.name.clone())
            .collect(),
        directories: manifest
            .hooks
            .directories
            .iter()
            .map(|entry| entry.path.clone())
            .collect(),
        alternatives: manifest
            .hooks
            .alternatives
            .iter()
            .map(|entry| entry.name.clone())
            .collect(),
    }
}

fn compare_lifecycle_category(
    category: &str,
    debug_entries: Vec<String>,
    signed_entries: &[String],
) -> Result<()> {
    let debug = debug_entries
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let signed = signed_entries
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if debug != signed {
        let mismatch = debug
            .symmetric_difference(&signed)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        bail!("debug TOML lifecycle.{category} projection mismatch: {mismatch}");
    }
    Ok(())
}

fn joined_keys(map: &BTreeMap<String, ConfigPolicyV2>) -> String {
    map.keys().cloned().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use crate::ccs::v2::schema::*;

    #[test]
    fn rejects_debug_config_missing_from_signed_authority() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
description = "demo package"
release = "1"
kind = "package"

[config]
files = ["/etc/conary-example/config.toml"]
noreplace = true
"#;
        let authority = AuthorityDocumentV2::package_for_tests("demo");
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::validate_debug_toml_projection(&authority, &manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(
            error
                .to_string()
                .contains("/etc/conary-example/config.toml")
        );
    }

    #[test]
    fn rejects_debug_lifecycle_mismatch_with_signed_authority() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
description = "demo package"
release = "1"
kind = "package"

[[hooks.services]]
name = "other.service"
action = "restart"
"#;
        let mut authority = AuthorityDocumentV2::package_for_tests("demo");
        authority.lifecycle.services = vec!["conary-example.service".to_string()];
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::validate_debug_toml_projection(&authority, &manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(error.to_string().contains("conary-example.service"));
        assert!(error.to_string().contains("other.service"));
    }

    #[test]
    fn rejects_signed_lifecycle_missing_from_debug_toml() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
description = "demo package"
release = "1"
kind = "package"
"#;
        let mut authority = AuthorityDocumentV2::package_for_tests("demo");
        authority.lifecycle.services = vec!["conary-example.service".to_string()];
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::validate_debug_toml_projection(&authority, &manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(error.to_string().contains("conary-example.service"));
    }

    #[test]
    fn still_rejects_unsupported_debug_toml_install_authority() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
description = "demo package"
release = "1"
kind = "package"

[[requires.packages]]
name = "openssl"
version = ">=3.0"
"#;
        let authority = AuthorityDocumentV2::package_for_tests("demo");
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::reject_unsupported_debug_toml_install_authority(&manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(error.to_string().contains("dependencies"));
        super::validate_debug_toml_projection(&authority, &manifest).unwrap();
    }
}
