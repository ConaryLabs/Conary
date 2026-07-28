// conary-core/src/packages/deb/payload/tests.rs

use super::*;
use std::io::Cursor;
use tar::{Builder, EntryType, Header as TarHeader, HeaderMode};

fn append(
    builder: &mut Builder<Vec<u8>>,
    path: &str,
    entry_type: EntryType,
    mode: u32,
    content: &[u8],
    link_name: Option<&str>,
) {
    let mut header = TarHeader::new_gnu();
    header.set_entry_type(entry_type);
    header.set_mode(mode);
    header.set_uid(42);
    header.set_gid(24);
    header.set_mtime(123);
    header.set_size(content.len() as u64);
    header.set_username("source-user").unwrap();
    header.set_groupname("source-group").unwrap();
    if let Some(link_name) = link_name {
        header.set_link_name(link_name).unwrap();
    }
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(content))
        .unwrap();
}

fn finish(builder: Builder<Vec<u8>>) -> Vec<u8> {
    builder.into_inner().unwrap()
}

fn bounds_with(
    max_archive_entries: u64,
    max_payload_object_bytes: u64,
    max_total_payload_bytes: u64,
) -> ArchiveDecodeBounds {
    ArchiveDecodeBounds {
        max_archive_entries,
        max_payload_object_bytes,
        max_total_payload_bytes,
        ..CCS_BUDGET.archive_decode_bounds().unwrap()
    }
}

#[test]
fn data_tar_entry_count_over_budget_is_typed() {
    let mut builder = Builder::new(Vec::new());
    append(
        &mut builder,
        "usr/share/one",
        EntryType::Regular,
        0o644,
        b"one",
        None,
    );
    append(
        &mut builder,
        "usr/share/two",
        EntryType::Regular,
        0o644,
        b"two",
        None,
    );
    let archive = finish(builder);
    let bounds = bounds_with(1, 3, 6);

    let error = parse_reader(Box::new(Cursor::new(archive)), None, &bounds).unwrap_err();
    let Error::Budget(error) = error else {
        panic!("expected typed entry-count refusal");
    };
    assert_eq!(
        error.dimension,
        crate::ccs::BudgetDimension::ArchiveEntryCount
    );
    assert_eq!(error.observed, 2);
    assert_eq!(error.limit, 1);
}

#[test]
fn data_tar_cumulative_payload_over_budget_is_typed() {
    let mut builder = Builder::new(Vec::new());
    append(
        &mut builder,
        "usr/share/one",
        EntryType::Regular,
        0o644,
        b"one",
        None,
    );
    append(
        &mut builder,
        "usr/share/two",
        EntryType::Regular,
        0o644,
        b"two",
        None,
    );
    let archive = finish(builder);
    let bounds = bounds_with(8, 3, 5);

    let error = parse_reader(Box::new(Cursor::new(archive)), None, &bounds).unwrap_err();
    let Error::Budget(error) = error else {
        panic!("expected typed cumulative-payload refusal");
    };
    assert_eq!(
        error.dimension,
        crate::ccs::BudgetDimension::TotalPayloadBytes
    );
    assert_eq!(error.observed, 6);
    assert_eq!(error.limit, 5);
}

fn vm_hwm_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.split_ascii_whitespace().next())
        .and_then(|value| value.parse().ok())
}

#[test]
fn streams_zstd_tar_past_the_retired_two_gib_ceiling() {
    const DECLARED: u64 = 2 * 1024 * 1024 * 1024 + 64 * 1024;
    const ZERO_CHUNK_SIZE: usize = 1024 * 1024;

    let before_kib = vm_hwm_kib();
    let mut header = TarHeader::new_gnu();
    header
        .set_path("usr/share/issue-133/large-zero-payload")
        .unwrap();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(DECLARED);
    header.set_cksum();
    let mut compressed = zstd::stream::encode_all(Cursor::new(header.as_bytes()), 1).unwrap();
    let zero_chunk = vec![0_u8; ZERO_CHUNK_SIZE];
    let zero_frame = zstd::stream::encode_all(Cursor::new(&zero_chunk), 1).unwrap();
    for _ in 0..(DECLARED / ZERO_CHUNK_SIZE as u64) {
        compressed.extend_from_slice(&zero_frame);
    }
    let remainder = usize::try_from(DECLARED % ZERO_CHUNK_SIZE as u64).unwrap();
    if remainder != 0 {
        compressed.extend_from_slice(
            &zstd::stream::encode_all(Cursor::new(&zero_chunk[..remainder]), 1).unwrap(),
        );
    }
    compressed.extend_from_slice(
        &zstd::stream::encode_all(Cursor::new(&zero_chunk[..TAR_BLOCK_SIZE * 2]), 1).unwrap(),
    );
    let after_build_kib = vm_hwm_kib();

    assert!(
        compressed.len() < 1024 * 1024,
        "fixture must stay tiny: {} bytes",
        compressed.len()
    );
    let parsed = parse_package_files(&compressed).unwrap();
    let after_parse_kib = vm_hwm_kib();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].content.as_ref().unwrap().size, DECLARED);
    eprintln!(
        "issue133_large_archive compressed_bytes={} VmHWM_before_kib={} \
         VmHWM_after_build_kib={} VmHWM_after_parse_kib={}",
        compressed.len(),
        before_kib.unwrap_or(0),
        after_build_kib.unwrap_or(0),
        after_parse_kib.unwrap_or(0),
    );
}

fn raw_regular_header(declared: u64) -> Vec<u8> {
    let mut header = TarHeader::new_gnu();
    header.set_path("usr/share/declared").unwrap();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(declared);
    header.set_cksum();
    header.as_bytes().to_vec()
}

#[test]
fn data_tar_declared_size_mismatch_is_typed_in_both_directions() {
    let mut shorter = raw_regular_header(3);
    shorter.extend_from_slice(b"ab");
    let error = parse_package_files(&shorter).unwrap_err();
    assert!(matches!(&error, Error::ParseError(_)));
    assert!(error.to_string().contains("regular payload"), "{error}");

    let mut longer = raw_regular_header(3);
    longer.extend_from_slice(b"abcd");
    longer.resize(TAR_BLOCK_SIZE * 2, 0);
    longer.extend_from_slice(&[0; TAR_BLOCK_SIZE * 2]);
    let error = parse_package_files(&longer).unwrap_err();
    assert!(matches!(&error, Error::ParseError(_)));
    assert!(
        error
            .to_string()
            .contains("carries nonzero bytes in its padding"),
        "{error}"
    );
}

#[test]
fn accepts_one_exact_archive_root_directory_as_container_metadata() {
    let mut builder = Builder::new(Vec::new());
    append(&mut builder, "./", EntryType::Directory, 0o755, &[], None);
    append(
        &mut builder,
        "./usr/bin/tool",
        EntryType::Regular,
        0o755,
        b"tool",
        None,
    );

    let parsed = parse_package_files(&finish(builder)).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].path, "/usr/bin/tool");
    assert_eq!(parsed[0].content.as_ref().unwrap().size, 4);
}

#[test]
fn archive_root_must_be_one_zero_sized_directory() {
    let mut duplicate = Builder::new(Vec::new());
    append(&mut duplicate, "./", EntryType::Directory, 0o755, &[], None);
    append(&mut duplicate, "./", EntryType::Directory, 0o755, &[], None);
    let error = parse_package_files(&finish(duplicate)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("duplicate archive root directory"),
        "{error}"
    );

    let mut regular = Builder::new(Vec::new());
    append(
        &mut regular,
        ".",
        EntryType::Regular,
        0o755,
        b"not-a-directory",
        None,
    );
    let error = parse_package_files(&finish(regular)).unwrap_err();
    assert!(error.to_string().contains("is not a directory"), "{error}");
}

#[test]
fn parses_exact_posix_nodes_and_hardlink_identity() {
    let mut builder = Builder::new(Vec::new());
    builder.mode(HeaderMode::Complete);
    append(
        &mut builder,
        "usr/bin",
        EntryType::Directory,
        0o755,
        &[],
        None,
    );
    append(
        &mut builder,
        "usr/bin/tool",
        EntryType::Regular,
        0o4750,
        b"abc",
        None,
    );
    append(
        &mut builder,
        "usr/bin/tool-copy",
        EntryType::Link,
        0,
        &[],
        Some("usr/bin/tool"),
    );
    append(
        &mut builder,
        "usr/bin/tool-link",
        EntryType::Symlink,
        0o777,
        &[],
        Some("tool"),
    );
    let archive = finish(builder);

    let metadata = parse_package_files(&archive).unwrap();
    assert_eq!(metadata.len(), 4);
    assert!(matches!(metadata[0].node.kind, PayloadNodeKind::Directory));
    assert_eq!(metadata[0].node.mode, libc::S_IFDIR | 0o755);
    assert_eq!(metadata[1].node.user, PayloadIdentity::Numeric { id: 42 });
    assert_eq!(metadata[1].node.group, PayloadIdentity::Numeric { id: 24 });
    assert_eq!(metadata[1].node.mode, libc::S_IFREG | 0o4750);
    assert_eq!(
        metadata[1].node.kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some("path:/usr/bin/tool".to_string())
        }
    );
    assert_eq!(
        metadata[2].node.kind,
        PayloadNodeKind::Hardlink {
            target: "/usr/bin/tool".to_string(),
            identity: "path:/usr/bin/tool".to_string(),
        }
    );
    assert_eq!(metadata[2].node.mode, metadata[1].node.mode);
    assert_eq!(
        metadata[3].node.kind,
        PayloadNodeKind::Symlink {
            target: "tool".to_string()
        }
    );
    assert!(metadata[0].content.is_none());
    assert_eq!(metadata[1].content.as_ref().unwrap().size, 3);
    assert!(metadata[2].content.is_none());
    assert!(metadata[3].content.is_none());

    let extracted = extract_files(&archive).unwrap();
    assert!(extracted[0].content.is_empty());
    assert_eq!(extracted[1].content, b"abc");
    assert!(extracted[2].content.is_empty());
    assert!(extracted[3].content.is_empty());
    assert_eq!(extracted[1].content_authority, metadata[1].content);
}

#[test]
fn preserves_negative_mtime_and_large_numeric_ids() {
    let mut header = TarHeader::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o640);
    header.set_uid(1u64 << 40);
    header.set_gid((1u64 << 40) + 1);
    header.set_size(0);
    header.as_old_mut().mtime.fill(0xff);
    header.as_old_mut().mtime[11] = 0xd6;
    header.set_cksum();

    let mut builder = Builder::new(Vec::new());
    builder
        .append_data(&mut header, "var/lib/exact", Cursor::new([]))
        .unwrap();
    let parsed = parse_package_files(&finish(builder)).unwrap();

    assert_eq!(
        parsed[0].node.user,
        PayloadIdentity::Numeric { id: 1u64 << 40 }
    );
    assert_eq!(
        parsed[0].node.group,
        PayloadIdentity::Numeric {
            id: (1u64 << 40) + 1
        }
    );
    assert_eq!(parsed[0].node.mtime.seconds, -42);
}

#[test]
fn rejects_pax_instead_of_silently_applying_it() {
    let mut header = TarHeader::new_gnu();
    header.set_entry_type(EntryType::XHeader);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(0);
    header.set_cksum();
    let mut builder = Builder::new(Vec::new());
    builder
        .append_data(&mut header, "pax", Cursor::new([]))
        .unwrap();

    let error = parse_package_files(&finish(builder)).unwrap_err();
    assert!(error.to_string().contains("unsupported PAX"));
}

#[test]
fn resolves_forward_hardlink_chains_to_one_canonical_anchor() {
    let mut builder = Builder::new(Vec::new());
    append(
        &mut builder,
        "usr/bin/tool-copy-2",
        EntryType::Link,
        0,
        &[],
        Some("usr/bin/tool-copy-1"),
    );
    append(
        &mut builder,
        "usr/bin/tool-copy-1",
        EntryType::Link,
        0,
        &[],
        Some("usr/bin/tool"),
    );
    append(
        &mut builder,
        "usr/bin/tool",
        EntryType::Regular,
        0o755,
        b"tool",
        None,
    );

    let parsed = parse_package_files(&finish(builder)).unwrap();
    assert_eq!(
        parsed[0].node.kind,
        PayloadNodeKind::Hardlink {
            target: "/usr/bin/tool".to_string(),
            identity: "path:/usr/bin/tool".to_string(),
        }
    );
    assert_eq!(parsed[0].node.mode, libc::S_IFREG | 0o755);
    assert_eq!(parsed[0].node.user, PayloadIdentity::Numeric { id: 42 });
    assert_eq!(parsed[1].node, parsed[0].node);
    assert_eq!(
        parsed[2].node.kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some("path:/usr/bin/tool".to_string())
        }
    );
    assert_eq!(parsed[2].content.as_ref().unwrap().size, 4);
}

#[test]
fn rejects_hardlink_cycles_with_the_exact_cycle() {
    let mut builder = Builder::new(Vec::new());
    append(
        &mut builder,
        "usr/bin/a",
        EntryType::Link,
        0,
        &[],
        Some("usr/bin/b"),
    );
    append(
        &mut builder,
        "usr/bin/b",
        EntryType::Link,
        0,
        &[],
        Some("usr/bin/a"),
    );

    let error = parse_package_files(&finish(builder)).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("hardlink cycle detected"));
    assert!(message.contains("\"/usr/bin/a\" -> \"/usr/bin/b\" -> \"/usr/bin/a\""));
}

#[test]
fn rejects_hardlinks_whose_target_is_absent_from_the_archive() {
    let mut builder = Builder::new(Vec::new());
    append(
        &mut builder,
        "usr/bin/tool-copy",
        EntryType::Link,
        0,
        &[],
        Some("usr/bin/missing"),
    );

    let error = parse_package_files(&finish(builder)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("targets missing payload path \"/usr/bin/missing\"")
    );
}
