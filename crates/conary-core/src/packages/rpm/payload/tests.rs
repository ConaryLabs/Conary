// crates/conary-core/src/packages/rpm/payload/tests.rs

use super::digest::{ComputedFileDigest, ComputedRegularContent};
use super::header::DeclaredDigest;
use super::stream::{PayloadMember, RegularPayloadEvidence};
use super::{
    HeaderRecord, RpmFileDigestAlgorithm, apply_capability_operation, digest_hex, parse,
    parse_file_capabilities, project_records,
};
use crate::packages::payload::ReopenablePayload;
use crate::payload::PayloadNodeKind;
use rpm::IndexTag;
use std::io::Cursor;

#[test]
fn sha1_file_digest_uses_rpm_algorithm_code_two_semantics() {
    assert_eq!(
        digest_hex(RpmFileDigestAlgorithm::Sha1, b"abc"),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
}

#[test]
fn sha1_file_digest_header_projects_payload_without_md5_fallback() {
    let content = b"SHA-1 RPM payload fixture\n";
    let mut builder =
        rpm::PackageBuilder::new("sha1-fixture", "1", "MIT", "noarch", "SHA-1 fixture");
    builder
        .with_file_contents(
            content,
            rpm::FileOptions::new("/usr/share/sha1-fixture/data"),
        )
        .unwrap();
    let package = builder.build().unwrap();
    let header_start = package.metadata.get_package_segment_offsets().header as usize;
    let mut bytes = Vec::new();
    package.write(&mut bytes).unwrap();

    let algorithm_offset =
        main_header_value_offset(&bytes, header_start, IndexTag::RPMTAG_FILEDIGESTALGO);
    bytes[algorithm_offset..algorithm_offset + 4].copy_from_slice(&2_u32.to_be_bytes());
    let digest_offset =
        main_header_value_offset(&bytes, header_start, IndexTag::RPMTAG_FILEDIGESTS);
    let sha1 = digest_hex(RpmFileDigestAlgorithm::Sha1, content);
    bytes[digest_offset..digest_offset + sha1.len()].copy_from_slice(sha1.as_bytes());
    bytes[digest_offset + sha1.len()] = 0;

    let patched = rpm::Package::parse(&mut Cursor::new(bytes)).unwrap();
    let decompressed = crate::packages::parse_metrics::ReadCounter::default();
    let (payload, metrics) = super::parse_stream(
        &patched,
        Box::new(Cursor::new(patched.payload.clone())),
        &decompressed,
    )
    .unwrap();
    let files = payload.to_extracted_in_memory().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "/usr/share/sha1-fixture/data");
    assert_eq!(files[0].content, content);
    assert_eq!(metrics.payload_spool_bytes_reread, 0);
    assert_eq!(metrics.payload_spool_file_reopens, 0);
    assert_eq!(metrics.payload_bytes_hashed, content.len() as u64 * 2);
}

#[test]
fn projection_consumes_computed_evidence_without_reopening_spool_source() {
    let content = b"projection evidence";
    let sha256 = crate::hash::sha256(content);
    let record = HeaderRecord {
        path: "/usr/share/fixture".to_string(),
        path_kind: super::header::HeaderPathKind::Deployable,
        mode: libc::S_IFREG | 0o644,
        user: "root".to_string(),
        group: "root".to_string(),
        mtime: 1,
        size: content.len() as u64,
        ghost: false,
        digest: Some(DeclaredDigest {
            algorithm: RpmFileDigestAlgorithm::Sha2_256,
            hex: sha256.clone(),
        }),
        link_target: None,
        caps: None,
        ima_signature: None,
        device: 1,
        inode: 0,
        rdev: 0,
        nlink: Some(1),
    };
    let temp = tempfile::tempdir().unwrap();
    let absent_source = temp.path().join("must-not-be-opened");
    let members = vec![PayloadMember {
        header_index: 0,
        archive_position: 0,
        content_size: content.len() as u64,
        regular: Some(RegularPayloadEvidence {
            computed: ComputedRegularContent {
                sha256: sha256.clone(),
                declared: ComputedFileDigest {
                    algorithm: RpmFileDigestAlgorithm::Sha2_256,
                    hex: sha256.clone(),
                },
                bytes_hashed: content.len() as u64,
            },
            source: ReopenablePayload::from_path(&absent_source),
        }),
    }];

    let payload = project_records(&[record], members).unwrap();

    assert_eq!(payload.files().len(), 1);
    assert_eq!(
        payload.files()[0]
            .content_authority
            .as_ref()
            .unwrap()
            .sha256,
        sha256
    );
    assert_eq!(
        payload.files()[0].source().unwrap().path(),
        Some(absent_source.as_path())
    );
    assert!(!absent_source.exists());
}

#[test]
fn source_validated_rpm_symlink_projects_without_regular_content_authority() {
    let path = "/usr/lib/.build-id/a7/232a5f6ed485eb65b89f39caa2da4dfe8288b5";
    let target = "../../../../usr/bin/curl";
    let mut builder =
        rpm::PackageBuilder::new("symlink-fixture", "1", "MIT", "x86_64", "symlink fixture");
    builder
        .with_symlink(rpm::FileOptions::symlink(path, target))
        .unwrap();
    let package = builder.build().unwrap();

    let payload = parse(&package).unwrap();
    assert_eq!(payload.files().len(), 1);
    let file = &payload.files()[0];
    assert_eq!(file.path, path);
    assert_eq!(
        file.node.kind,
        PayloadNodeKind::Symlink {
            target: target.to_string()
        }
    );
    assert!(file.content_authority.is_none());
    assert!(file.source().is_none());
    assert!(file.to_extracted_in_memory().unwrap().content.is_empty());
}

#[test]
fn rpm_root_anchor_is_consumed_without_becoming_payload_authority() {
    let records = [HeaderRecord {
        path: "/".to_string(),
        path_kind: super::header::HeaderPathKind::RootAnchor,
        mode: libc::S_IFDIR | 0o555,
        user: "root".to_string(),
        group: "root".to_string(),
        mtime: 1,
        size: 0,
        ghost: false,
        digest: None,
        link_target: None,
        caps: None,
        ima_signature: None,
        device: 1,
        inode: 1,
        rdev: 0,
        nlink: Some(1),
    }];
    let members = vec![super::stream::PayloadMember {
        header_index: 0,
        archive_position: 0,
        content_size: 0,
        regular: None,
    }];

    let payload = project_records(&records, members).unwrap();

    assert!(
        payload.files().is_empty(),
        "the selected root is a container boundary, not RPM payload mutation authority"
    );
}

#[test]
fn pinned_fedora_2ping_projects_rpm_hardlink_transaction_owner() {
    const FIXTURE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rpm/2ping-4.5.1-24.fc44.noarch.rpm"
    ));
    assert_eq!(
        crate::hash::sha256(FIXTURE),
        "cf48a9380416daf02e934cbddd15d5356b3b6ea6b5f0824187074b66dd0fe14a"
    );
    let package = rpm::Package::parse(&mut Cursor::new(FIXTURE)).unwrap();
    let records = super::header::header_records(&package).unwrap();
    let first = records
        .iter()
        .find(|record| record.path == "/usr/bin/2ping")
        .unwrap();
    let owner = records
        .iter()
        .find(|record| record.path == "/usr/bin/2ping6")
        .unwrap();
    assert_ne!(first.ima_signature, owner.ima_signature);

    let payload = parse(&package).unwrap();
    let files = payload
        .files()
        .iter()
        .filter(|file| file.path == "/usr/bin/2ping" || file.path == "/usr/bin/2ping6")
        .collect::<Vec<_>>();

    assert_eq!(files.len(), 2);
    let projected_owner = files
        .iter()
        .find(|file| file.path == "/usr/bin/2ping6")
        .unwrap();
    assert_eq!(
        projected_owner.node.kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some("rpm:1:1".to_string())
        }
    );
    assert_eq!(projected_owner.node.mode, owner.mode);
    assert_eq!(projected_owner.node.mtime.seconds, i64::from(owner.mtime));
    assert_eq!(
        projected_owner.node.xattrs.get("security.ima").unwrap(),
        &hex::decode(owner.ima_signature.as_ref().unwrap()).unwrap()
    );
    assert_eq!(
        projected_owner.content_authority.as_ref().unwrap(),
        &crate::payload::PayloadContentAuthority {
            sha256: "581a92b326b83518a80d3d35411e6d0ed1380969306a39d042d5922dd45528aa".to_string(),
            size: 188
        }
    );
    let linked = files
        .iter()
        .find(|file| file.path == "/usr/bin/2ping")
        .unwrap();
    assert_eq!(
        linked.node.kind,
        PayloadNodeKind::Hardlink {
            target: "/usr/bin/2ping6".to_string(),
            identity: "rpm:1:1".to_string()
        }
    );
    assert_eq!(linked.node.mode, projected_owner.node.mode);
    assert_eq!(linked.node.user, projected_owner.node.user);
    assert_eq!(linked.node.group, projected_owner.node.group);
    assert_eq!(linked.node.mtime, projected_owner.node.mtime);
    assert_eq!(linked.node.xattrs, projected_owner.node.xattrs);
}

fn main_header_value_offset(bytes: &[u8], header_start: usize, wanted: IndexTag) -> usize {
    let entry_count = u32::from_be_bytes(
        bytes[header_start + 8..header_start + 12]
            .try_into()
            .unwrap(),
    ) as usize;
    let store_start = header_start + 16 + entry_count * 16;
    for index in 0..entry_count {
        let entry_start = header_start + 16 + index * 16;
        let tag = u32::from_be_bytes(bytes[entry_start..entry_start + 4].try_into().unwrap());
        if tag == wanted as u32 {
            let offset =
                i32::from_be_bytes(bytes[entry_start + 8..entry_start + 12].try_into().unwrap());
            return store_start + usize::try_from(offset).unwrap();
        }
    }
    panic!("missing {wanted} in fixture header");
}

#[test]
fn capability_text_projects_exact_kernel_xattr() {
    let encoded = parse_file_capabilities("cap_net_bind_service,cap_net_raw=ep", "/usr/bin/server")
        .expect("parse capability");
    assert_eq!(encoded.len(), 20);
    assert_eq!(
        u32::from_le_bytes(encoded[0..4].try_into().unwrap()),
        0x0200_0001
    );
    assert_eq!(
        u32::from_le_bytes(encoded[4..8].try_into().unwrap()),
        (1 << 10) | (1 << 13)
    );
}

#[test]
fn capability_operations_apply_in_order() {
    let mut value = 0_u64;
    apply_capability_operation(&mut value, 0b11, '=', true);
    apply_capability_operation(&mut value, 0b01, '-', true);
    apply_capability_operation(&mut value, 0b100, '+', true);
    assert_eq!(value, 0b110);
}

#[test]
fn capability_text_rejects_nonrepresentable_effective_subset() {
    let error = parse_file_capabilities("cap_net_bind_service=p cap_net_raw=e", "/usr/bin/server")
        .expect_err("effective subset must fail");
    assert!(error.to_string().contains("not representable"));
}
