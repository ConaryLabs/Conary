// crates/conary-core/src/ccs/package/tests.rs

use super::*;
use crate::ccs::VerifiedCcsArchive;
use crate::ccs::builder::{CcsBuilder, write_signed_current_ccs_package};
use crate::ccs::signing::SigningKeyPair;
use crate::filesystem::CasStore;
use crate::payload::PayloadNodeKind;
use flate2::Compression;
use gzp::ZWriter;
use std::fs::{self, File};
use std::io::Read;
use tar::{Archive, Builder};
use tempfile::TempDir;

fn build_test_package() -> (TempDir, std::path::PathBuf, SigningKeyPair) {
    let temp = tempfile::tempdir().unwrap();
    let source_dir = temp.path().join("src");
    fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
    fs::write(source_dir.join("usr/bin/hello"), b"hello world\n").unwrap();

    let manifest = CcsManifest::parse(
        r#"
[package]
name = "test-package"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "test fixture"
license = "MIT"

[package.platform]
arch = "noarch"
"#,
    )
    .unwrap();

    let result = CcsBuilder::new(manifest, &source_dir)
        .unwrap()
        .build()
        .unwrap();
    let package_path = temp.path().join("test-package.ccs");
    let signing_key = SigningKeyPair::generate();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, false).unwrap();
    (temp, package_path, signing_key)
}

fn verify_test_package(path: &Path, signing_key: &SigningKeyPair) -> VerifiedCcsArchive {
    crate::ccs::verify::verify_package(
        path,
        &crate::ccs::verify::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
    )
    .unwrap()
}

fn truncate_first_object(source_path: &Path, output_path: &Path) {
    let source_file = File::open(source_path).unwrap();
    let decoder = crate::ccs::archive_framing::MgzipDecoder::new(source_file);
    let mut archive = Archive::new(decoder);

    let output_file = File::create(output_path).unwrap();
    let encoder = gzp::par::compress::ParCompressBuilder::<gzp::deflate::Mgzip>::new()
        .buffer_size(crate::ccs::CCS_BUDGET.archive_compression_block_bytes)
        .unwrap()
        .num_threads(1)
        .unwrap()
        .compression_level(Compression::default())
        .from_writer(output_file);
    let mut builder = Builder::new(encoder);
    let mut truncated = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        let mut header = entry.header().clone();
        if !truncated
            && header.entry_type().is_file()
            && path.starts_with("objects")
            && !bytes.is_empty()
        {
            bytes.truncate(bytes.len() / 2);
            header.set_size(bytes.len() as u64);
            header.set_cksum();
            truncated = true;
        }
        builder
            .append_data(&mut header, &path, bytes.as_slice())
            .unwrap();
    }
    assert!(truncated, "test package must contain an object payload");
    builder.finish().unwrap();
    let mut encoder = builder.into_inner().unwrap();
    encoder.finish().unwrap();
}

#[test]
fn test_symlink_hash_consistency() {
    // Verify we use consistent symlink hashing
    let target = "/usr/lib/libfoo.so.1";
    let hash = CasStore::compute_symlink_hash(target);
    assert_eq!(hash.len(), 64);
}

#[cfg(unix)]
#[test]
fn test_extract_preserves_symlink_target() {
    let temp = tempfile::tempdir().unwrap();
    let source_dir = temp.path().join("src");
    fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
    fs::write(source_dir.join("usr/bin/bash"), b"bash\n").unwrap();
    std::os::unix::fs::symlink("bash", source_dir.join("usr/bin/sh")).unwrap();

    let manifest = CcsManifest::parse(
        r#"
[package]
name = "symlink-package"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "symlink fixture"
license = "MIT"

[package.platform]
arch = "noarch"
"#,
    )
    .unwrap();

    let result = CcsBuilder::new(manifest, &source_dir)
        .unwrap()
        .build()
        .unwrap();
    let package_path = temp.path().join("symlink-package.ccs");
    let signing_key = SigningKeyPair::generate();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, false).unwrap();

    let verification = verify_test_package(&package_path, &signing_key);
    let package =
        CcsPackage::from_verified_archive(package_path.to_str().unwrap(), &verification).unwrap();
    let files = package.extract_file_contents().unwrap();
    let sh = files
        .iter()
        .find(|file| file.path == "/usr/bin/sh")
        .expect("expected /usr/bin/sh symlink");

    assert_eq!(
        sh.node.kind,
        PayloadNodeKind::Symlink {
            target: "bash".to_string()
        }
    );
}

#[test]
fn signed_v3_file_capabilities_round_trip_to_verified_install_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let source_dir = temp.path().join("src");
    fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
    fs::write(source_dir.join("usr/bin/server"), b"#!/bin/sh\n").unwrap();
    let manifest = CcsManifest::parse(
        r#"
[package]
name = "file-capable"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "file capability fixture"
license = "MIT"

[package.platform]
arch = "noarch"

[[file_capabilities]]
path = "/usr/bin/server"
capabilities = ["cap_net_bind_service"]
permitted = true
effective = true
inheritable = false
"#,
    )
    .unwrap();
    let result = CcsBuilder::new(manifest, &source_dir)
        .unwrap()
        .build()
        .unwrap();
    let package_path = temp.path().join("file-capable.ccs");
    let signing_key = SigningKeyPair::generate();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, false).unwrap();

    let verification = verify_test_package(&package_path, &signing_key);
    let package =
        CcsPackage::from_verified_archive(package_path.to_str().unwrap(), &verification).unwrap();

    assert_eq!(verification.authority().file_capabilities.len(), 1);
    assert_eq!(
        package.manifest().file_capabilities,
        verification.authority().file_capabilities
    );
    assert_eq!(
        package.manifest().file_capabilities[0]
            .to_setcap_spec()
            .unwrap(),
        "cap_net_bind_service=ep"
    );
}

#[test]
fn test_extract_rejects_truncated_content() {
    let (_temp, package_path, signing_key) = build_test_package();
    let corrupted_path = package_path.with_file_name("truncated.ccs");
    truncate_first_object(&package_path, &corrupted_path);

    let err = crate::ccs::verify::verify_package(
        &corrupted_path,
        &crate::ccs::verify::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
    )
    .unwrap_err();
    let err = format!("{err:#}");
    assert!(
        err.contains("object path hash mismatch")
            || err.contains("payload hash mismatch")
            || err.contains("payload size mismatch")
            || err.contains("signed authority requires"),
        "{err}"
    );
}

#[test]
fn current_writer_rejects_declared_size_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("size-lie.ccs");
    let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("size-lie");
    let crate::ccs::v3::schema::PackageKindV3::Package(data) = &mut authority.kind else {
        panic!("fixture must be a package");
    };
    data.files[0].content.as_mut().unwrap().size += 1;
    authority.components.get_mut("main").unwrap().total_size += 1;
    let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
    let signing_key = SigningKeyPair::generate();

    let error = crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority,
        &payloads,
        &package_path,
        &signing_key,
        None,
        None,
        None,
    )
    .unwrap_err();
    let error = format!("{error:#}");
    assert!(
        error.contains("authenticate and stage whole payload")
            && error.contains("payload size mismatch while staging"),
        "{error}"
    );
}

#[test]
fn v3_packages_do_not_reconstruct_missing_authority_from_projections() {
    let authority = crate::ccs::v3::test_support::package_authority_with_one_file("adapter-v3");
    let package = CcsPackage::from_v3_authority_for_tests(authority.clone(), None, None).unwrap();
    assert_eq!(package.manifest().package.name, "adapter-v3");
    assert!(package.v3_authority().is_some());
}

#[test]
fn v3_positive_dependency_kinds_reach_package_format_without_string_guessing() {
    use crate::ccs::v3::schema::{DependencyKindV3, ProvidedCapabilityV3};
    use crate::repository::dependency_model::{
        RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementGroup,
        RepositoryRequirementKind,
    };

    let requirement = |name: &str, capability_kind| {
        RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            RepositoryRequirementClause {
                name: name.to_string(),
                capability_kind: Some(capability_kind),
                version_constraint: None,
                architecture_qualifier: Default::default(),
                native_text: None,
            },
        )
    };

    let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("typed-v3");
    authority.requirements = vec![
        requirement("web-server", RepositoryCapabilityKind::Virtual),
        requirement("soname(libssl.so.3)", RepositoryCapabilityKind::Soname),
        requirement("binary(sh)", RepositoryCapabilityKind::Generic),
    ];
    authority.provided_capabilities = vec![
        ProvidedCapabilityV3 {
            kind: DependencyKindV3::Capability,
            name: "http-client".to_string(),
            provider_version: None,
            version_relation: None,
            version_scheme: crate::repository::versioning::VersionScheme::Conary,
            architecture_qualifier: Default::default(),
            provenance: crate::repository::dependency_model::CapabilityProvenance::AuthorDeclared,
            target: None,
            component: None,
        },
        ProvidedCapabilityV3 {
            kind: DependencyKindV3::PkgConfig,
            name: "typed-v3".to_string(),
            provider_version: None,
            version_relation: None,
            version_scheme: crate::repository::versioning::VersionScheme::Conary,
            architecture_qualifier: Default::default(),
            provenance: crate::repository::dependency_model::CapabilityProvenance::AuthorDeclared,
            target: None,
            component: None,
        },
    ];

    let package = CcsPackage::from_v3_authority_for_tests(authority.clone(), None, None).unwrap();
    let resolution_capabilities = package.resolution_capabilities().unwrap();
    let provides = resolution_capabilities
        .iter()
        .map(|provide| provide.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(package.requirements(), authority.requirements.as_slice());
    assert_eq!(provides, vec!["typed-v3", "http-client", "typed-v3"]);
    assert!(package.manifest().provides.capabilities.is_empty());
    assert!(package.manifest().provides.pkgconfig.is_empty());
}

#[test]
fn v3_projection_preserves_attestation_metadata() {
    let authority = crate::ccs::v3::test_support::package_authority_with_one_file("attested-v3");
    let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("publish");
    let envelope = crate::ccs::attestation::test_support::sample_envelope_for_tests(&key);
    let package =
        CcsPackage::from_v3_authority_for_tests(authority, Some(envelope.clone()), None).unwrap();
    let provenance = package.manifest().provenance.as_ref().unwrap();
    assert_eq!(provenance.build_attestation.as_ref(), Some(&envelope));
}

#[test]
fn parse_rejects_native_v3_and_verified_parse_accepts_after_verification() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("adapter-v3.ccs");
    let authority = crate::ccs::v3::test_support::package_authority_with_one_file("adapter-v3");
    let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
    let key = crate::ccs::signing::SigningKeyPair::generate();
    crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority, &payloads, &path, &key, None, None, None,
    )
    .unwrap();

    let plain_error = CcsPackage::parse(path.to_str().unwrap()).unwrap_err();
    assert!(
        plain_error
            .to_string()
            .contains("requires a VerifiedCcsArchive capability")
    );

    let verification = crate::ccs::verify::verify_package(
        &path,
        &crate::ccs::verify::TrustPolicy::strict(vec![key.public_key_base64()]),
    )
    .unwrap();
    let package = CcsPackage::from_verified_archive(path.to_str().unwrap(), &verification).unwrap();
    assert!(package.v3_authority().is_some());

    let untrusted_key = crate::ccs::signing::SigningKeyPair::generate();
    let verified_error = crate::ccs::verify::verify_package(
        &path,
        &crate::ccs::verify::TrustPolicy::strict(vec![untrusted_key.public_key_base64()]),
    )
    .unwrap_err();
    assert!(format!("{verified_error:#}").contains("not trusted"));
}
