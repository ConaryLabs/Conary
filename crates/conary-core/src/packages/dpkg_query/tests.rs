// crates/conary-core/src/packages/dpkg_query/tests.rs

use super::*;

fn dpkg_identity(selector: &str, architecture: &str) -> InstalledPackageIdentity {
    InstalledPackageIdentity::dpkg(
        selector,
        "fixture",
        "1.0-1",
        architecture,
        DebianMultiArch::Same,
    )
    .unwrap()
}

#[test]
fn test_is_dpkg_available() {
    // This test just ensures the function runs without panic
    let _ = is_dpkg_available();
}

#[test]
fn test_installed_dpkg_info_version() {
    let info = InstalledDpkgInfo {
        name: "test".to_string(),
        version: "1.0.0-1ubuntu1".to_string(),
        arch: "amd64".to_string(),
        description: None,
        maintainer: None,
        homepage: None,
        section: None,
        priority: None,
        installed_size: None,
    };

    assert_eq!(info.full_version(), "1.0.0-1ubuntu1");
    assert_eq!(info.version_only(), "1.0.0-1ubuntu1");
}

#[test]
fn batch_removal_targets_exact_multiarch_stanzas_and_info_records() {
    let temp = tempfile::TempDir::new().unwrap();
    let status = temp.path().join("status");
    let info = temp.path().join("info");
    std::fs::create_dir(&info).unwrap();
    std::fs::write(
            &status,
            "Package: fixture\nArchitecture: amd64\nStatus: install ok installed\n\nPackage: fixture\nArchitecture: i386\nStatus: install ok installed\n\nPackage: retained\nArchitecture: amd64\nStatus: install ok installed\n\n",
        )
        .unwrap();
    for name in [
        "fixture:amd64.list",
        "fixture:amd64.postinst",
        "fixture:i386.list",
        "retained.list",
    ] {
        std::fs::write(info.join(name), b"fixture").unwrap();
    }

    remove_dpkg_records_at(&[dpkg_identity("fixture:amd64", "amd64")], &status, &info).unwrap();

    let remaining = std::fs::read_to_string(status).unwrap();
    assert!(
        !remaining
            .contains("Architecture: amd64\nStatus: install ok installed\n\nPackage: fixture")
    );
    assert!(remaining.contains("Package: fixture\nArchitecture: i386"));
    assert!(remaining.contains("Package: retained\nArchitecture: amd64"));
    assert!(!info.join("fixture:amd64.list").exists());
    assert!(!info.join("fixture:amd64.postinst").exists());
    assert!(info.join("fixture:i386.list").exists());
    assert!(info.join("retained.list").exists());
}

#[test]
fn batch_removal_rejects_a_missing_exact_variant_without_rewriting_status() {
    let targets = BTreeSet::from([("fixture".to_string(), "arm64".to_string())]);
    let status = "Package: fixture\nArchitecture: amd64\n\n";

    let error = filter_dpkg_status(status, &targets).unwrap_err();

    assert!(error.to_string().contains("fixture:arm64"));
}

#[test]
fn committed_status_removal_is_not_rejected_by_ancillary_cleanup_failure() {
    let temp = tempfile::TempDir::new().unwrap();
    let status = temp.path().join("status");
    let missing_info = temp.path().join("missing-info");
    std::fs::write(
        &status,
        "Package: fixture\nArchitecture: amd64\nStatus: install ok installed\n\n",
    )
    .unwrap();

    remove_dpkg_records_at(
        &[dpkg_identity("fixture:amd64", "amd64")],
        &status,
        &missing_info,
    )
    .unwrap();

    assert!(std::fs::read_to_string(status).unwrap().is_empty());
}

#[test]
fn package_query_records_preserve_multiline_descriptions_and_variants() {
    let output = "fixture:amd64\x1efixture\x1e1.2.3\x1eamd64\x1efirst line\nsecond line\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1esame\x1f\
                      fixture:arm64\x1efixture\x1e1.2.3\x1earm64\x1edescription\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1e\x1f";
    let records = parse_package_query_records(output).unwrap();

    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].info.description.as_deref(),
        Some("first line\nsecond line")
    );
    assert_eq!(records[1].info.arch, "arm64");
    assert_eq!(
        records[0].identity.debian_multi_arch(),
        Some(DebianMultiArch::Same)
    );
    assert_eq!(
        records[1].identity.debian_multi_arch(),
        Some(DebianMultiArch::No)
    );
    assert_eq!(records[0].identity.selector(), "fixture:amd64");
    assert_eq!(records[1].identity.selector(), "fixture:arm64");
}

#[test]
fn package_inventory_rejects_malformed_or_duplicate_records() {
    assert!(parse_package_query_records("missing-fields\x1f").is_err());

    let record = "fixture:amd64\x1efixture\x1e1.2.3\x1eamd64\x1edescription\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1eno\x1f";
    assert!(parse_package_query_records(&format!("{record}{record}")).is_err());

    let unknown_multi_arch = "fixture:amd64\x1efixture\x1e1.2.3\x1eamd64\x1edescription\x1emaintainer\x1ehttps://example.invalid\x1eutils\x1eoptional\x1e42\x1esometimes\x1f";
    let error = parse_package_query_records(unknown_multi_arch).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid Debian Multi-Arch value")
    );
}

#[test]
fn installed_provides_preserve_debian_versions_and_architecture_qualifiers() {
    let identity = InstalledPackageIdentity::dpkg(
        "fixture:amd64",
        "fixture",
        "2:1.4-3",
        "amd64",
        DebianMultiArch::Foreign,
    )
    .unwrap();
    let provides = parse_dpkg_provide_record(
        &identity,
        "fixture:amd64\x1efixture\x1e2:1.4-3\x1efixture (= 1.0), mail-api:any (= 2), helper:arm64",
    )
    .unwrap();

    assert_eq!(provides.len(), 4);
    assert_eq!(provides[0].provenance, CapabilityProvenance::ExactIdentity);
    assert_eq!(provides[1].name, "fixture");
    assert_eq!(provides[1].kind, RepositoryCapabilityKind::PackageName);
    assert_eq!(provides[1].version.as_deref(), Some("1.0"));
    assert_eq!(provides[2].name, "mail-api");
    assert_eq!(provides[2].kind, RepositoryCapabilityKind::Virtual);
    assert_eq!(provides[2].version.as_deref(), Some("2"));
    assert_eq!(
        provides[2].architecture_qualifier,
        ProvideArchitectureQualifier::Any
    );
    assert_eq!(provides[3].name, "helper");
    assert_eq!(
        provides[3].architecture_qualifier,
        ProvideArchitectureQualifier::Exact("arm64".to_string())
    );
}

#[test]
fn installed_provides_reject_identity_drift() {
    let identity = InstalledPackageIdentity::dpkg(
        "fixture:amd64",
        "fixture",
        "1",
        "amd64",
        DebianMultiArch::No,
    )
    .unwrap();
    assert!(
        parse_dpkg_provide_record(&identity, "fixture:amd64\x1efixture\x1e2\x1email-api").is_err()
    );
}

#[test]
fn md5sums_parser_is_exact_and_rejects_malformed_records() {
    let parsed = parse_md5sums("d41d8cd98f00b204e9800998ecf8427e  usr/share/fixture\n").unwrap();
    assert_eq!(
        parsed.get("usr/share/fixture").map(String::as_str),
        Some("d41d8cd98f00b204e9800998ecf8427e")
    );

    assert!(parse_md5sums("not-a-record\n").is_err());
    assert!(
        parse_md5sums(
            "d41d8cd98f00b204e9800998ecf8427e  usr/share/fixture\n\
                 d41d8cd98f00b204e9800998ecf8427e  usr/share/fixture\n"
        )
        .is_err()
    );
}
