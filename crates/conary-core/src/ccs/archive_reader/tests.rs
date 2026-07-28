// conary-core/src/ccs/archive_reader/tests.rs

use super::*;
use crate::ccs::builder::write_v2_ccs_package_from_bounded_memory_for_tests;
use crate::ccs::signing::SigningKeyPair;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;
use tar::Builder;

fn current_package() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("current.ccs");
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("current");
    let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
    write_v2_ccs_package_from_bounded_memory_for_tests(
        &authority,
        &payloads,
        &path,
        &SigningKeyPair::generate(),
        None,
        None,
        None,
    )
    .unwrap();
    (temp, path)
}

fn append<W: Write>(builder: &mut Builder<W>, path: &str, bytes: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, path, bytes).unwrap();
}

fn entry(path: &str, content: Vec<u8>) -> (String, Vec<u8>) {
    (path.to_string(), content)
}

fn archive_of(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let encoder = GzEncoder::new(&mut bytes, Compression::default());
        let mut builder = Builder::new(encoder);
        for (path, content) in entries {
            append(&mut builder, path, content);
        }
        builder.into_inner().unwrap().finish().unwrap();
    }
    bytes
}

#[test]
fn inspection_is_explicitly_untrusted_and_v2_only() {
    let (_temp, path) = current_package();
    let archive = inspect_untrusted_ccs_archive(File::open(&path).unwrap()).unwrap();

    assert_eq!(archive.v2_authority.identity.name, "current");
    assert_eq!(archive.manifest.package.name, "current");
    assert!(archive.signature_raw.is_some());
    assert_eq!(archive.census.files, 1);
    assert!(has_current_ccs_archive_contract(path).unwrap());
}

#[test]
fn current_contract_detection_rejects_v1() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("v1.ccs");
    let mut manifest = Vec::new();
    ciborium::into_writer(
        &serde_json::json!({
            "format_version": 1,
            "name": "retired-v1-fixture",
        }),
        &mut manifest,
    )
    .unwrap();
    std::fs::write(&path, archive_of(&[entry("MANIFEST", manifest)])).unwrap();

    let error = inspect_untrusted_ccs_archive(File::open(&path).unwrap()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("CCS v1 archive authority is unsupported")
    );
    let error = has_current_ccs_archive_contract(path).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("CCS v1 archive authority is unsupported")
    );
}

#[test]
fn inspection_rejects_noncanonical_object_paths() {
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("bad-object");
    let bytes = archive_of(&[
        entry("MANIFEST", authority.to_cbor().unwrap()),
        entry("objects/not-a-digest", b"payload".to_vec()),
    ]);

    let error = inspect_untrusted_ccs_archive(std::io::Cursor::new(bytes)).unwrap_err();
    assert!(error.to_string().contains("invalid CCS object path"));
}

#[test]
fn inspection_rejects_duplicate_manifest_authority() {
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("duplicate");
    let raw = authority.to_cbor().unwrap();
    let bytes = archive_of(&[entry("MANIFEST", raw.clone()), entry("./MANIFEST", raw)]);

    let error = inspect_untrusted_ccs_archive(std::io::Cursor::new(bytes)).unwrap_err();
    assert!(error.to_string().contains("duplicate MANIFEST"));
}

#[test]
fn inspection_requires_authority_before_every_other_archived_file() {
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("ordered");
    let bytes = archive_of(&[
        entry("MANIFEST.sig", b"{}".to_vec()),
        entry("MANIFEST", authority.to_cbor().unwrap()),
    ]);

    let error = inspect_untrusted_ccs_archive(std::io::Cursor::new(bytes)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("must carry its MANIFEST authority before"),
        "{error:#}"
    );
}

#[test]
fn inspection_rejects_unsigned_and_oversized_payload_objects() {
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("payload-bounds");
    let unsigned = b"not in signed authority".to_vec();
    let hash = crate::hash::sha256(&unsigned);
    let bytes = archive_of(&[
        entry("MANIFEST", authority.to_cbor().unwrap()),
        entry(&format!("objects/{}/{}", &hash[..2], &hash[2..]), unsigned),
    ]);

    let error = inspect_untrusted_ccs_archive(std::io::Cursor::new(bytes)).unwrap_err();
    assert!(
        error.to_string().contains("total-payload-bytes")
            || error.to_string().contains("payload-object-count"),
        "{error:#}"
    );
}

#[test]
fn inspection_rejects_a_hostile_declared_authority_length_before_allocation() {
    // The declared tar length is refused against the derived decoder-memory
    // ceiling, so no allocation proportional to the declaration happens.
    let ceiling = CCS_BUDGET.max_authority_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_size(ceiling + 1);
    header.set_mode(0o644);
    header.set_cksum();

    let error = CCS_BUDGET
        .admit_control_bytes(
            BudgetDimension::AuthorityBytes,
            "MANIFEST",
            header.size().unwrap(),
            ceiling,
        )
        .unwrap_err();
    assert_eq!(error.dimension, BudgetDimension::AuthorityBytes);
    assert_eq!(error.limit, ceiling);
}
