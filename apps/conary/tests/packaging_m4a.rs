// apps/conary/tests/packaging_m4a.rs

mod common;

use conary_core::ccs::builder::write_v2_ccs_package;
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::ccs::v2::schema::{
    AuthorityDocumentV2, ComponentAuthorityV2, ConflictPolicyV2, FORMAT_VERSION_V2,
    FileAuthorityV2, FileTypeV2, LifecycleAuthorityV2, PackageDataV2, PackageIdentityV2,
    PackageKindTagV2, PackageKindV2, ProvenanceAuthorityV2,
};
use conary_core::ccs::verify::{TrustPolicy, verify_package};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tar::Builder;

#[test]
fn v2_package_verification_rejects_unsigned_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("unsigned-v2.ccs");
    write_unsigned_v2_package(&package_path);

    let error = verify_package(&package_path, &TrustPolicy::strict(Vec::new())).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("MANIFEST.sig") || message.contains("not signed"),
        "unexpected unsigned v2 verification error: {message}"
    );
}

#[test]
fn v2_install_refuses_allow_unsigned_bypass() {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("unsigned-v2.ccs");
    let db_path = temp.path().join("conary.db");
    let root = temp.path().join("root");
    fs::create_dir_all(&root).unwrap();
    write_unsigned_v2_package(&package_path);

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("ccs")
        .arg("install")
        .arg(&package_path)
        .arg("--db-path")
        .arg(&db_path)
        .arg("--root")
        .arg(&root)
        .arg("--sandbox")
        .arg("never")
        .arg("--dry-run")
        .arg("--allow-unsigned")
        .output()
        .expect("run conary ccs install");

    assert_failure_contains(&output, &["native CCS v2", "signature verification"]);
}

#[test]
fn v2_install_uses_verified_parse_after_signature_check() {
    let work = tempfile::tempdir().unwrap();
    let package_path = work.path().join("signed-v2.ccs");
    let policy_path = work.path().join("trust-policy.toml");
    let root = work.path().join("root");
    let (_db_temp, db_path) = common::setup_command_test_db();
    fs::create_dir_all(&root).unwrap();
    let signer = SigningKeyPair::generate().with_key_id("publish");
    write_signed_v2_package(&package_path, &signer);
    fs::write(
        &policy_path,
        format!(
            "trusted_keys = [\"{}\"]\nallow_unsigned = false\n",
            signer.public_key_base64()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("ccs")
        .arg("install")
        .arg(&package_path)
        .arg("--db-path")
        .arg(&db_path)
        .arg("--root")
        .arg(&root)
        .arg("--sandbox")
        .arg("never")
        .arg("--dry-run")
        .arg("--no-deps")
        .arg("--policy")
        .arg(&policy_path)
        .output()
        .expect("run conary ccs install");

    assert_success(&output);
    assert_stdout_contains(&output, "Package: signed-v2 v1.0.0");
}

fn write_unsigned_v2_package(package_path: &Path) {
    let authority = unsigned_v2_authority();
    write_raw_v2_manifest_only(package_path, &authority);
}

fn write_signed_v2_package(package_path: &Path, signer: &SigningKeyPair) {
    let authority = signed_v2_authority();
    let payloads = BTreeMap::from([("/usr/bin/hello".to_string(), b"hello world\n".to_vec())]);
    write_v2_ccs_package(
        &authority,
        &payloads,
        package_path,
        signer,
        None,
        None,
        None,
    )
    .unwrap();
}

fn unsigned_v2_authority() -> AuthorityDocumentV2 {
    v2_authority("unsigned-v2")
}

fn signed_v2_authority() -> AuthorityDocumentV2 {
    v2_authority("signed-v2")
}

fn v2_authority(name: &str) -> AuthorityDocumentV2 {
    let file = FileAuthorityV2 {
        path: "/usr/bin/hello".to_string(),
        sha256: conary_core::hash::sha256(b"hello world\n"),
        size: 12,
        file_type: FileTypeV2::Regular,
        mode: 0o755,
        owner: "root".to_string(),
        group: "root".to_string(),
        component: "main".to_string(),
        symlink_target: None,
        config: None,
        conflict: ConflictPolicyV2::Error,
    };
    AuthorityDocumentV2 {
        format_version: FORMAT_VERSION_V2,
        identity: PackageIdentityV2 {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            platform: Some("linux".to_string()),
            kind: PackageKindTagV2::Package,
        },
        kind: PackageKindV2::Package(PackageDataV2 {
            files: vec![file],
            config: Vec::new(),
            policy: Default::default(),
        }),
        provides: Vec::new(),
        requires: Vec::new(),
        components: BTreeMap::from([(
            "main".to_string(),
            ComponentAuthorityV2 {
                name: "main".to_string(),
                default: true,
                file_count: 1,
                total_size: 12,
            },
        )]),
        lifecycle: LifecycleAuthorityV2::default(),
        provenance: ProvenanceAuthorityV2 {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("hermetic".to_string()),
            build_input_identity: Some("sha256:build-input".to_string()),
            hermetic_evidence_hash: Some("sha256:evidence".to_string()),
            foreign_conversion_boundary_hash: None,
        },
        debug_toml_sha256: None,
    }
}

fn write_raw_v2_manifest_only(package_path: &Path, authority: &AuthorityDocumentV2) {
    let manifest_cbor = authority.to_cbor().unwrap();
    let output = fs::File::create(package_path).unwrap();
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_cbor.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "MANIFEST", manifest_cbor.as_slice())
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap();
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command to succeed\n{}",
        output_text(output)
    );
}

fn assert_stdout_contains(output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(needle),
        "expected stdout to contain {needle:?}\n{}",
        output_text(output)
    );
}

fn assert_failure_contains(output: &Output, needles: &[&str]) {
    assert!(
        !output.status.success(),
        "expected command to fail\n{}",
        output_text(output)
    );
    let combined = output_text(output);
    for needle in needles {
        assert!(
            combined.contains(needle),
            "expected output to contain {needle:?}\n{combined}"
        );
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
