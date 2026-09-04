// conary-core/src/packages/arch/payload/tests.rs

use super::*;
use crate::packages::{ExtractedFile, PackagePayload};
use std::io::Cursor;
use tar::{Archive, Builder, Header};

fn header(
    path: &str,
    entry_type: EntryType,
    mode: u32,
    uid: u64,
    gid: u64,
    mtime: u64,
    size: u64,
) -> Header {
    let mut header = Header::new_gnu();
    header.set_path(path).unwrap();
    header.set_entry_type(entry_type);
    header.set_mode(mode);
    header.set_uid(uid);
    header.set_gid(gid);
    header.set_mtime(mtime);
    header.set_size(size);
    header
}

fn append(builder: &mut Builder<Vec<u8>>, mut header: Header, content: &[u8]) {
    header.set_cksum();
    builder.append(&header, content).unwrap();
}

fn pax_sparse_v1_archive(
    logical_path: &'static str,
    logical_size: &'static [u8],
    map: &[u8],
    extent_bytes: &[u8],
) -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());
    builder
        .append_pax_extensions([
            ("GNU.sparse.major", b"1".as_slice()),
            ("GNU.sparse.minor", b"0".as_slice()),
            ("GNU.sparse.name", logical_path.as_bytes()),
            ("GNU.sparse.realsize", logical_size),
        ])
        .unwrap();
    let mut encoded = map.to_vec();
    encoded.resize(encoded.len().div_ceil(512) * 512, 0);
    encoded.extend_from_slice(extent_bytes);
    append(
        &mut builder,
        header(
            "GNUSparseFile.123/sparse.bin",
            EntryType::Regular,
            0o640,
            12,
            34,
            56,
            encoded.len() as u64,
        ),
        &encoded,
    );
    builder.into_inner().unwrap()
}

fn parse_archive(bytes: Vec<u8>) -> Result<Vec<ExtractedFile>> {
    let mut archive = Archive::new(Cursor::new(bytes));
    let mut parsed = Vec::new();
    let spool = PayloadSpool::new(0)?;
    let bounds = crate::ccs::CCS_BUDGET.archive_decode_bounds()?;
    for (index, entry) in archive
        .entries()
        .map_err(|error| Error::ParseError(error.to_string()))?
        .enumerate()
    {
        let mut entry = entry.map_err(|error| Error::ParseError(error.to_string()))?;
        parsed.push(parse_entry(&mut entry, &spool, index, &bounds)?);
    }
    PackagePayload::new(resolve_hardlinks(parsed)?).to_extracted_in_memory()
}

#[test]
fn declared_payload_copy_rejects_shorter_and_longer_bodies() {
    let mut output = Vec::new();
    let error =
        copy_declared_payload(&mut Cursor::new(b"ab"), &mut output, 3, "/short").unwrap_err();
    assert!(matches!(&error, Error::ParseError(_)));
    assert!(error.to_string().contains("declares 3 bytes but yields 2"));

    output.clear();
    let error =
        copy_declared_payload(&mut Cursor::new(b"abcd"), &mut output, 3, "/long").unwrap_err();
    assert!(matches!(&error, Error::ParseError(_)));
    assert!(
        error
            .to_string()
            .contains("declares 3 bytes but yields at least 4")
    );
}

#[test]
fn parses_gnu_pax_sparse_v1_as_the_logical_path_and_content() {
    let bytes = pax_sparse_v1_archive(
        "usr/share/example/sparse.bin",
        b"16",
        b"3\n0\n3\n13\n3\n16\n0\n",
        b"abcxyz",
    );
    let bounds = crate::ccs::CCS_BUDGET.archive_decode_bounds().unwrap();
    let mut archive = Archive::new(Cursor::new(bytes.clone()));
    let mut entry = archive.entries().unwrap().next().unwrap().unwrap();
    assert_eq!(declared_spool_bytes(&mut entry, &bounds).unwrap(), 16);

    let files = parse_archive(bytes).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "/usr/share/example/sparse.bin");
    assert_eq!(files[0].content_authority.as_ref().unwrap().size, 16);
    assert_eq!(files[0].content, b"abc\0\0\0\0\0\0\0\0\0\0xyz");
}

#[test]
fn rejects_overlapping_gnu_pax_sparse_v1_extents() {
    let bytes = pax_sparse_v1_archive(
        "usr/share/example/sparse.bin",
        b"5",
        b"2\n0\n4\n3\n2\n",
        b"abcdef",
    );
    let error = parse_archive(bytes).expect_err("overlapping sparse extents must fail closed");
    assert!(
        error.to_string().contains("overlapping, out of order"),
        "{error}"
    );
}

#[test]
fn rejects_gnu_pax_sparse_v1_extent_count_over_the_structural_budget() {
    let bytes = pax_sparse_v1_archive(
        "usr/share/example/sparse.bin",
        b"16",
        b"3\n0\n3\n13\n3\n16\n0\n",
        b"abcxyz",
    );
    let mut bounds = crate::ccs::CCS_BUDGET.archive_decode_bounds().unwrap();
    bounds.max_payload_references = 2;
    let mut archive = Archive::new(Cursor::new(bytes));
    let mut entry = archive.entries().unwrap().next().unwrap().unwrap();
    let error = match parse_entry(&mut entry, &PayloadSpool::new(0).unwrap(), 0, &bounds) {
        Ok(_) => panic!("sparse extent count over the structural budget must fail closed"),
        Err(error) => error,
    };
    let Error::Budget(error) = error else {
        panic!("expected typed payload-reference budget refusal");
    };
    assert_eq!(
        error.dimension,
        crate::ccs::BudgetDimension::PayloadReferenceCount
    );
    assert_eq!(error.observed, 3);
    assert_eq!(error.limit, 2);
}

#[test]
fn parses_exact_posix_nodes_and_pax_authority() {
    let mut builder = Builder::new(Vec::new());
    builder
        .append_pax_extensions([
            ("mtime", b"-1.25".as_slice()),
            ("SCHILY.xattr.user.conary", b"exact".as_slice()),
            // SCHILY and LIBARCHIVE families for the same xattr with
            // byte-identical values (4 bytes: [1, 2, 3, 4]).
            // LIBARCHIVE uses unpadded base64 per the writer grammar.
            (
                "SCHILY.xattr.security.capability",
                b"\x01\x02\x03\x04".as_slice(),
            ),
            (
                "LIBARCHIVE.xattr.security%2Ecapability",
                b"AQIDBA".as_slice(),
            ),
        ])
        .unwrap();
    append(
        &mut builder,
        header("usr/bin/tool", EntryType::Regular, 0o4750, 123, 456, 99, 5),
        b"hello",
    );

    let mut symlink = header(
        "usr/bin/tool-link",
        EntryType::Symlink,
        0o777,
        321,
        654,
        43,
        0,
    );
    symlink.set_link_name("tool").unwrap();
    append(&mut builder, symlink, &[]);

    let mut character = header("dev/example", EntryType::Char, 0o600, 0, 0, 44, 0);
    character.set_device_major(10).unwrap();
    character.set_device_minor(200).unwrap();
    append(&mut builder, character, &[]);

    append(
        &mut builder,
        header("run/example", EntryType::Fifo, 0o620, 20, 30, 45, 0),
        &[],
    );

    let files = parse_archive(builder.into_inner().unwrap()).unwrap();
    assert_eq!(files.len(), 4);

    let regular = &files[0];
    assert_eq!(regular.path, "/usr/bin/tool");
    assert_eq!(
        regular.node.kind,
        PayloadNodeKind::Regular {
            hardlink_identity: None
        }
    );
    assert_eq!(regular.node.mode, libc::S_IFREG | 0o4750);
    assert_eq!(regular.node.user, PayloadIdentity::Numeric { id: 123 });
    assert_eq!(regular.node.group, PayloadIdentity::Numeric { id: 456 });
    assert_eq!(
        regular.node.mtime,
        PayloadTimestamp {
            seconds: -2,
            nanoseconds: 750_000_000,
        }
    );
    assert_eq!(
        regular.node.xattrs.get("user.conary"),
        Some(&b"exact".to_vec())
    );
    assert_eq!(
        regular.node.xattrs.get("security.capability"),
        Some(&vec![1, 2, 3, 4])
    );
    assert_eq!(regular.content, b"hello");
    assert_eq!(
        regular.content_authority,
        Some(PayloadContentAuthority {
            sha256: crate::hash::sha256(b"hello"),
            size: 5,
        })
    );

    assert_eq!(
        files[1].node.kind,
        PayloadNodeKind::Symlink {
            target: "tool".to_string()
        }
    );
    assert_eq!(files[1].node.mode, libc::S_IFLNK | 0o777);
    assert!(files[1].content_authority.is_none());
    assert_eq!(
        files[2].node.kind,
        PayloadNodeKind::CharacterDevice {
            major: 10,
            minor: 200
        }
    );
    assert_eq!(files[2].node.mode, libc::S_IFCHR | 0o600);
    assert_eq!(files[3].node.kind, PayloadNodeKind::Fifo);
    assert_eq!(files[3].node.mode, libc::S_IFIFO | 0o620);
}

#[test]
fn resolves_forward_hardlink_chains_to_one_canonical_anchor() {
    let mut builder = Builder::new(Vec::new());
    let mut second = header("usr/bin/second", EntryType::Link, 0, 0, 0, 0, 0);
    second.set_link_name("usr/bin/first").unwrap();
    append(&mut builder, second, &[]);
    let mut first = header("usr/bin/first", EntryType::Link, 0, 0, 0, 0, 0);
    first.set_link_name("usr/bin/anchor").unwrap();
    append(&mut builder, first, &[]);
    append(
        &mut builder,
        header("usr/bin/anchor", EntryType::Regular, 0o755, 10, 20, 30, 7),
        b"payload",
    );

    let files = parse_archive(builder.into_inner().unwrap()).unwrap();
    let identity = "path:/usr/bin/anchor".to_string();
    for file in &files[..2] {
        assert_eq!(
            file.node.kind,
            PayloadNodeKind::Hardlink {
                target: "/usr/bin/anchor".to_string(),
                identity: identity.clone(),
            }
        );
        assert!(file.content.is_empty());
        assert!(file.content_authority.is_none());
    }
    assert_eq!(
        files[2].node.kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some(identity),
        }
    );
    assert_eq!(files[2].content, b"payload");
}

#[test]
fn rejects_hardlinks_whose_target_is_absent_from_the_archive() {
    let mut builder = Builder::new(Vec::new());
    let mut link = header("usr/bin/link", EntryType::Link, 0, 0, 0, 0, 0);
    link.set_link_name("usr/bin/missing").unwrap();
    append(&mut builder, link, &[]);

    let error = parse_archive(builder.into_inner().unwrap())
        .expect_err("missing hardlink targets must not be guessed");
    assert!(
        error
            .to_string()
            .contains("targets missing payload node /usr/bin/missing"),
        "{error}"
    );
}

#[test]
fn rejects_hardlink_cycles_with_the_exact_cycle() {
    let mut builder = Builder::new(Vec::new());
    let mut first = header("usr/bin/a", EntryType::Link, 0, 0, 0, 0, 0);
    first.set_link_name("usr/bin/b").unwrap();
    append(&mut builder, first, &[]);
    let mut second = header("usr/bin/b", EntryType::Link, 0, 0, 0, 0, 0);
    second.set_link_name("usr/bin/a").unwrap();
    append(&mut builder, second, &[]);

    let error = parse_archive(builder.into_inner().unwrap())
        .expect_err("hardlink cycles must not be flattened");
    assert!(
        error
            .to_string()
            .contains("hardlink cycle starts at /usr/bin/a"),
        "{error}"
    );
}
