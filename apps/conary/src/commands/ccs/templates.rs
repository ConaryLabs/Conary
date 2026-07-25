// apps/conary/src/commands/ccs/templates.rs

use anyhow::Result;
use conary_core::ccs::CcsManifest;
use conary_core::ccs::manifest::{Service, ServiceAction};
use conary_core::ccs::v2::PackageKindTagV2;

use super::CcsInitTemplate;

pub fn build_manifest(
    template: Option<CcsInitTemplate>,
    name: &str,
    version: &str,
) -> Result<CcsManifest> {
    match template {
        Some(CcsInitTemplate::MinimalFile) => minimal_file_manifest(name, version),
        Some(CcsInitTemplate::ConfigNoreplace) => config_noreplace_manifest(name, version),
        Some(CcsInitTemplate::Service) => service_manifest(name, version),
        None => Ok(CcsManifest::new_minimal(name, version)),
    }
}

fn minimal_file_manifest(name: &str, version: &str) -> Result<CcsManifest> {
    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.package.release = "1".to_string();
    manifest.package.kind = PackageKindTagV2::Package;
    manifest.package.description = format!("{name} package");
    Ok(manifest)
}

fn config_noreplace_manifest(name: &str, version: &str) -> Result<CcsManifest> {
    let mut manifest = minimal_file_manifest(name, version)?;
    manifest.config.files = vec!["/etc/conary-example/config.toml".to_string()];
    manifest.config.noreplace = true;
    Ok(manifest)
}

fn service_manifest(name: &str, version: &str) -> Result<CcsManifest> {
    let mut manifest = minimal_file_manifest(name, version)?;
    manifest.hooks.services.push(Service {
        name: "conary-example.service".to_string(),
        action: ServiceAction::Restart,
        reversible: None,
    });
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_file_template_writes_v2_identity_fields() {
        let manifest = build_manifest(Some(CcsInitTemplate::MinimalFile), "hello", "0.1.0")
            .expect("template manifest");

        assert_eq!(manifest.package.name, "hello");
        assert_eq!(manifest.package.version, "0.1.0");
        assert_eq!(manifest.package.release.as_str(), "1");
        assert_eq!(manifest.package.kind, PackageKindTagV2::Package);
    }

    #[test]
    fn config_noreplace_template_declares_config_authority() {
        let manifest = build_manifest(Some(CcsInitTemplate::ConfigNoreplace), "demo", "0.1.0")
            .expect("template manifest");

        assert_eq!(manifest.package.release.as_str(), "1");
        assert_eq!(manifest.package.kind, PackageKindTagV2::Package);
        assert_eq!(
            manifest.config.files,
            vec!["/etc/conary-example/config.toml".to_string()]
        );
        assert!(manifest.config.noreplace);
        assert!(manifest.hooks.services.is_empty());
        assert!(manifest.hooks.systemd.is_empty());
    }

    #[test]
    fn service_template_declares_lifecycle_without_scripts() {
        let manifest = build_manifest(Some(CcsInitTemplate::Service), "demo", "0.1.0")
            .expect("template manifest");

        assert_eq!(manifest.package.release.as_str(), "1");
        assert_eq!(manifest.package.kind, PackageKindTagV2::Package);
        assert_eq!(manifest.hooks.services.len(), 1);
        assert_eq!(manifest.hooks.services[0].name, "conary-example.service");
        assert!(matches!(
            manifest.hooks.services[0].action,
            conary_core::ccs::manifest::ServiceAction::Restart
        ));
        assert!(manifest.hooks.post_install.is_none());
        assert!(manifest.hooks.pre_remove.is_none());
    }
}
