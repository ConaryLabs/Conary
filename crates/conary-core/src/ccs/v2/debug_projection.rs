// conary-core/src/ccs/v2/debug_projection.rs

use super::schema::{AuthorityDocumentV2, ConfigPolicyV2, PackageDataV2, PackageKindV2};
use anyhow::{Result, bail};
use std::collections::BTreeMap;

pub fn validate_debug_toml_projection(
    authority: &AuthorityDocumentV2,
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    validate_config_projection(authority, manifest)?;
    validate_requirement_projection(authority, manifest)?;
    validate_lifecycle_projection(authority, manifest)?;
    Ok(())
}

pub(crate) fn reject_unsupported_debug_toml_install_authority(
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    let mut unsupported = Vec::new();
    if !manifest.components.rules.is_empty() || !manifest.components.files.is_empty() {
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

fn validate_requirement_projection(
    authority: &AuthorityDocumentV2,
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    if super::authoring::project_requirements(manifest) != authority.requirements {
        bail!("debug TOML requirement projection does not match signed authority");
    }
    if manifest.relations != authority.relations {
        bail!("debug TOML relation projection does not match signed authority");
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
    let debug = super::lifecycle::authority_from_manifest(manifest);
    if debug != authority.lifecycle {
        bail!(
            "debug TOML lifecycle projection mismatch: debug {debug:?}, signed {:?}",
            authority.lifecycle
        );
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
version_scheme = "conary"
release = "1"
kind = "package"
description = "demo package"

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
version_scheme = "conary"
release = "1"
kind = "package"
description = "demo package"

[[hooks.services]]
name = "other.service"
action = "restart"
"#;
        let mut authority = AuthorityDocumentV2::package_for_tests("demo");
        authority.lifecycle.services = vec![LifecycleServiceV2 {
            name: "conary-example.service".to_string(),
            action: LifecycleServiceActionV2::Restart,
            reversible: None,
        }];
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
version_scheme = "conary"
release = "1"
kind = "package"
description = "demo package"
"#;
        let mut authority = AuthorityDocumentV2::package_for_tests("demo");
        authority.lifecycle.services = vec![LifecycleServiceV2 {
            name: "conary-example.service".to_string(),
            action: LifecycleServiceActionV2::Restart,
            reversible: None,
        }];
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
version_scheme = "conary"
release = "1"
kind = "package"
description = "demo package"
"#;
        let authority = AuthorityDocumentV2::package_for_tests("demo");
        let mut manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();
        manifest
            .components
            .files
            .insert("/usr/bin/demo".to_string(), "runtime".to_string());

        let error = super::reject_unsupported_debug_toml_install_authority(&manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(error.to_string().contains("component overrides"));
        super::validate_debug_toml_projection(&authority, &manifest).unwrap();
    }
}
