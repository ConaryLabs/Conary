// apps/conary/src/commands/ccs/templates.rs

use anyhow::Result;
use conary_core::ccs::CcsManifest;
use conary_core::ccs::v2::PackageKindTagV2;

use super::CcsInitTemplate;

pub fn build_manifest(
    template: Option<CcsInitTemplate>,
    name: &str,
    version: &str,
) -> Result<CcsManifest> {
    match template {
        Some(CcsInitTemplate::MinimalFile) => minimal_file_manifest(name, version),
        None => Ok(CcsManifest::new_minimal(name, version)),
    }
}

fn minimal_file_manifest(name: &str, version: &str) -> Result<CcsManifest> {
    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.package.release = Some("1".to_string());
    manifest.package.kind = Some(PackageKindTagV2::Package);
    manifest.package.description = format!("{name} package");
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
        assert_eq!(manifest.package.release.as_deref(), Some("1"));
        assert_eq!(manifest.package.kind, Some(PackageKindTagV2::Package));
    }
}
