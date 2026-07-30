// apps/conary/src/commands/install/conversion/tests/dependencies.rs

use super::*;

#[test]
fn default_dependency_passes_reach_kernel_initramfs_toolchain() {
    assert_eq!(DEFAULT_CCS_DEPENDENCY_PASSES, 2);
}

fn verified_rpm_ccs(name: &str, version: &str, architecture: &str) -> CcsPackage {
    let temp_dir = tempfile::tempdir().unwrap();
    let signing_key = SigningKeyPair::generate().with_key_id("dependency-identity");
    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.package.version_scheme = conary_core::repository::versioning::VersionScheme::Rpm;
    manifest.package.platform.as_mut().unwrap().arch = Some(architecture.to_string());
    let package_path =
        write_runtime_signed_ccs_package(temp_dir.path(), name, manifest, &signing_key);
    let verified = conary_core::ccs::verify::verify_package(
        &package_path,
        &conary_core::ccs::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
    )
    .unwrap();
    CcsPackage::from_verified_archive(package_path.to_str().unwrap(), &verified).unwrap()
}

fn selected_rpm_dependency() -> SatPackage {
    SatPackage {
        name: "dbus-broker".to_string(),
        version: "37-8.fc44".to_string(),
        package_release: None,
        architecture: Some("x86_64".to_string()),
        version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
        repo_package_id: Some(41),
        repository_id: Some(7),
        repository_name: Some("supported-host-fedora-44".to_string()),
        installed_trove_id: None,
        source: SatSource::Repository,
    }
}

fn selected_remi_provenance() -> RepositoryInstallProvenance {
    RepositoryInstallProvenance {
        repository_id: 7,
        source_profile: Some("fedora-44".to_string()),
        version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
        source_kind: RepositorySourceKind::Remi,
    }
}

#[test]
fn verified_remi_ccs_matches_exact_selected_source_identity() {
    let package = verified_rpm_ccs("dbus-broker", "37-8.fc44", "x86_64");
    let selected = selected_rpm_dependency();

    validate_selected_repository_ccs_identity(
        &package,
        &selected,
        Some(&selected_remi_provenance()),
    )
    .expect("signed CCS identity must match its exact selected Remi row");
}

#[test]
fn verified_remi_ccs_rejects_any_selected_identity_drift() {
    let package = verified_rpm_ccs("dbus-broker", "37-8.fc44", "x86_64");
    let provenance = selected_remi_provenance();

    let mut mismatches = Vec::new();

    let mut name = selected_rpm_dependency();
    name.name = "dbus-daemon".to_string();
    mismatches.push(name);

    let mut version = selected_rpm_dependency();
    version.version = "37-9.fc44".to_string();
    mismatches.push(version);

    let mut architecture = selected_rpm_dependency();
    architecture.architecture = Some("aarch64".to_string());
    mismatches.push(architecture);

    let mut scheme = selected_rpm_dependency();
    scheme.version_scheme = conary_core::repository::versioning::VersionScheme::Debian;
    mismatches.push(scheme);

    let mut repository = selected_rpm_dependency();
    repository.repository_id = Some(8);
    mismatches.push(repository);

    for selected in mismatches {
        let error =
            validate_selected_repository_ccs_identity(&package, &selected, Some(&provenance))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match SAT-selected repository row"),
            "identity drift must fail before mutation: {error:#}"
        );
    }
}

#[test]
fn explicit_static_ccs_release_must_match_selected_row() {
    let package = verified_rpm_ccs("dbus-broker", "37-8.fc44", "x86_64");
    let mut selected = selected_rpm_dependency();
    selected.package_release = Some("2".to_string());

    let error = validate_selected_repository_ccs_identity(
        &package,
        &selected,
        Some(&selected_remi_provenance()),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("package release"),
        "an explicitly selected CCS release must remain exact: {error:#}"
    );
}
