// conary-core/src/ccs/convert/converter/tests/manifest.rs

use super::*;
use crate::packages::traits::ProvidedCapability;
use crate::repository::dependency_model::RepositoryCapabilityKind;
use crate::repository::versioning::VersionScheme;

#[test]
fn test_conversion_options_default() {
    let options = ConversionOptions::default();
    assert_eq!(options.output_dir, PathBuf::from("./target/ccs"));
}

#[test]
fn test_converter_creation() {
    let converter = NativePackageConverter::with_defaults();
    assert_eq!(
        converter.options.output_dir,
        ConversionOptions::default().output_dir
    );
}

#[test]
fn test_build_manifest() {
    let options = ConversionOptions {
        output_dir: PathBuf::from("/tmp/test"),
    };
    let converter = NativePackageConverter::new(options);

    let metadata = make_test_metadata();

    let manifest = converter
        .build_manifest(&metadata, &Hooks::default())
        .unwrap();

    assert_eq!(manifest.package.name, "test-package");
    assert_eq!(manifest.package.version, "1.0.0");
    assert!(
        manifest.provides.binaries.is_empty(),
        "payload paths must not synthesize authoritative binary provides"
    );

    assert!(
        manifest.hooks.users.is_empty(),
        "shell text must not synthesize authoritative user hooks"
    );
}

#[test]
fn foreign_provides_bypass_legacy_manifest_and_project_as_typed_authority() {
    let converter = NativePackageConverter::with_defaults();
    let mut metadata = make_test_metadata();
    metadata.provides = vec![ProvidedCapability {
        kind: RepositoryCapabilityKind::Virtual,
        name: "kernel-uname-r".to_string(),
        version: Some("6.17.1-300.fc44.x86_64".to_string()),
        version_relation: Some(crate::repository::dependency_model::ProvideVersionRelation::Equal),
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: Default::default(),
    }];

    let manifest = converter
        .build_manifest(&metadata, &Hooks::default())
        .unwrap();

    assert!(manifest.provides.capabilities.is_empty());
    let signed = signed_native_provides(&metadata);
    assert_eq!(signed.len(), 2);
    assert_eq!(signed[0].name, "test-package");
    assert_eq!(signed[1].name, "kernel-uname-r");
    assert_eq!(
        signed[1].provider_version.as_deref(),
        Some("6.17.1-300.fc44.x86_64")
    );
    assert_eq!(signed[1].version_scheme, VersionScheme::Rpm);
}
