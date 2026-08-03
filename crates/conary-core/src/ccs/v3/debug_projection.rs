// conary-core/src/ccs/v3/debug_projection.rs

use super::schema::{AuthorityDocumentV3, ConfigSemanticsV3, PackageDataV3, PackageKindV3};
use anyhow::{Result, bail};
use std::collections::BTreeMap;

pub fn validate_debug_toml_projection(
    authority: &AuthorityDocumentV3,
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    validate_config_projection(authority, manifest)?;
    validate_file_capability_projection(authority, manifest)?;
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
            "v3 debug TOML contains unsupported install authority fields: {}",
            unsupported.join(", ")
        );
    }
    Ok(())
}

fn validate_file_capability_projection(
    authority: &AuthorityDocumentV3,
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    let debug =
        super::file_capabilities::canonicalize_file_capabilities(&manifest.file_capabilities)?;
    if debug != authority.file_capabilities {
        bail!("debug TOML file capability projection does not match signed authority");
    }
    Ok(())
}

fn validate_requirement_projection(
    authority: &AuthorityDocumentV3,
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
    authority: &AuthorityDocumentV3,
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Result<()> {
    let debug_config = debug_config_projection(manifest);
    let signed_config = match &authority.kind {
        PackageKindV3::Package(package) => signed_config_projection(package),
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

    if let PackageKindV3::Package(package) = &authority.kind {
        for (path, semantics) in &debug_config {
            let signed_file = package.files.iter().find(|file| file.path == *path);
            if semantics.ghost || semantics.remove_on_upgrade {
                if signed_file.is_some() {
                    bail!(
                        "debug TOML config path {path} must be absent from signed file authority"
                    );
                }
            } else {
                let Some(file) = signed_file else {
                    bail!("debug TOML config path {path} missing from signed file authority");
                };
                if file.config != Some(*semantics) {
                    bail!(
                        "debug TOML config path {path} semantics do not match signed file authority"
                    );
                }
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
) -> BTreeMap<String, ConfigSemanticsV3> {
    manifest
        .config
        .files
        .iter()
        .map(|entry| {
            (
                entry.path.clone(),
                ConfigSemanticsV3 {
                    noreplace: entry.noreplace,
                    ghost: entry.ghost,
                    remove_on_upgrade: entry.remove_on_upgrade,
                },
            )
        })
        .collect()
}

fn signed_config_projection(package: &PackageDataV3) -> BTreeMap<String, ConfigSemanticsV3> {
    package
        .config
        .iter()
        .map(|entry| (entry.path.clone(), entry.semantics))
        .collect()
}

fn validate_lifecycle_projection(
    authority: &AuthorityDocumentV3,
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

fn joined_keys(map: &BTreeMap<String, ConfigSemanticsV3>) -> String {
    map.keys().cloned().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use crate::ccs::v3::schema::*;

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

[[config.files]]
path = "/etc/conary-example/config.toml"
noreplace = true
ghost = false
remove_on_upgrade = false
"#;
        let authority = AuthorityDocumentV3::package_for_tests("demo");
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
        let mut authority = AuthorityDocumentV3::package_for_tests("demo");
        authority.lifecycle.services = vec![LifecycleServiceV3 {
            name: "conary-example.service".to_string(),
            action: LifecycleServiceActionV3::Restart,
            reversible: None,
        }];
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::validate_debug_toml_projection(&authority, &manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(error.to_string().contains("conary-example.service"));
        assert!(error.to_string().contains("other.service"));
    }

    #[test]
    fn rejects_debug_file_capability_mismatch_with_signed_authority() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "demo package"

[[file_capabilities]]
path = "/usr/bin/hello"
capabilities = ["cap_net_raw"]
permitted = true
effective = true
inheritable = false
"#;
        let mut authority = AuthorityDocumentV3::package_for_tests("demo");
        authority.file_capabilities = vec![crate::ccs::manifest::FileCapability {
            path: "/usr/bin/hello".to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        }];
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::validate_debug_toml_projection(&authority, &manifest).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("file capability projection does not match signed authority")
        );
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
        let mut authority = AuthorityDocumentV3::package_for_tests("demo");
        authority.lifecycle.services = vec![LifecycleServiceV3 {
            name: "conary-example.service".to_string(),
            action: LifecycleServiceActionV3::Restart,
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
        let authority = AuthorityDocumentV3::package_for_tests("demo");
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
