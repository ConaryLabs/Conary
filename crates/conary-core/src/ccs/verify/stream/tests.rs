// conary-core/src/ccs/verify/stream/tests.rs

use super::*;
use crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests;
use crate::ccs::signing::SigningKeyPair;
use flate2::Compression;
use flate2::write::GzEncoder;
use gzp::ZWriter;
use gzp::deflate::Mgzip;
use gzp::par::compress::ParCompressBuilder;
use std::fs::File;
use std::io::Write;
use tar::{Archive, Builder, EntryType, Header};

#[derive(Clone)]
struct TestEntry {
    path: String,
    entry_type: EntryType,
    content: Vec<u8>,
}

fn fixture() -> (
    tempfile::TempDir,
    Vec<TestEntry>,
    TrustPolicy,
    std::path::PathBuf,
) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("base.ccs");
    let authority = crate::ccs::v3::test_support::package_authority_with_one_file("stream");
    let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
    let signer = SigningKeyPair::generate();
    write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority, &payloads, &path, &signer, None, None, None,
    )
    .unwrap();
    let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);

    let mut archive = Archive::new(crate::ccs::archive_framing::MgzipDecoder::new(
        File::open(&path).unwrap(),
    ));
    let entries = archive
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let path = std::str::from_utf8(entry.path_bytes().as_ref())
                .unwrap()
                .to_string();
            let entry_type = entry.header().entry_type();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();
            TestEntry {
                path,
                entry_type,
                content,
            }
        })
        .collect();
    (temp, entries, policy, path)
}

fn chunked_fixture() -> (
    tempfile::TempDir,
    Vec<TestEntry>,
    SigningKeyPair,
    std::path::PathBuf,
    Vec<u8>,
) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("chunked.ccs");
    let bytes = (0..(crate::ccs::chunking::MAX_CHUNK_SIZE as usize * 20))
        .map(|index| ((index * 31 + index / 251) % 256) as u8)
        .collect::<Vec<_>>();
    let chunk_references = crate::ccs::chunking::Chunker::new()
        .chunk_bytes(&bytes)
        .iter()
        .map(crate::ccs::chunking::Chunk::reference)
        .collect::<Vec<_>>();
    assert!(chunk_references.len() > 2, "fixture must span chunks");

    let mut authority =
        crate::ccs::v3::test_support::package_authority_with_one_file("chunked-stream");
    let PackageKindV3::Package(package) = &mut authority.kind else {
        unreachable!()
    };
    let file = &mut package.files[0];
    file.content = Some(crate::payload::PayloadContentAuthority {
        sha256: crate::hash::sha256(&bytes),
        size: bytes.len() as u64,
    });
    file.content_layout = FileContentLayoutV3::FastCdcV2020 {
        min_size: crate::ccs::chunking::MIN_CHUNK_SIZE,
        average_size: crate::ccs::chunking::AVG_CHUNK_SIZE,
        max_size: crate::ccs::chunking::MAX_CHUNK_SIZE,
        chunks: chunk_references,
    };
    let mut copy = file.clone();
    copy.path = "/usr/bin/hello-copy".to_string();
    let first_path = file.path.clone();
    let copy_path = copy.path.clone();
    package.files.push(copy);
    let component = authority.components.get_mut("main").unwrap();
    component.file_count = 2;
    component.total_size = (bytes.len() as u64) * 2;

    let payloads = BTreeMap::from([(first_path, bytes.clone()), (copy_path, bytes.clone())]);
    let signer = SigningKeyPair::generate();
    write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority, &payloads, &path, &signer, None, None, None,
    )
    .unwrap();
    let entries = read_entries(&path);
    (temp, entries, signer, path, bytes)
}

fn read_entries(path: &Path) -> Vec<TestEntry> {
    let mut archive = Archive::new(crate::ccs::archive_framing::MgzipDecoder::new(
        File::open(path).unwrap(),
    ));
    archive
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let path = std::str::from_utf8(entry.path_bytes().as_ref())
                .unwrap()
                .to_string();
            let entry_type = entry.header().entry_type();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();
            TestEntry {
                path,
                entry_type,
                content,
            }
        })
        .collect()
}

fn resign_authority(
    entries: &mut [TestEntry],
    signer: &SigningKeyPair,
    mutate: impl FnOnce(&mut AuthorityDocumentV3),
) {
    let manifest = entries
        .iter_mut()
        .find(|entry| entry.path == "MANIFEST")
        .unwrap();
    let mut authority = CCS_BUDGET.decode_authority(&manifest.content).unwrap();
    mutate(&mut authority);
    manifest.content = authority.to_cbor().unwrap();
    let signature = serde_json::to_vec_pretty(&signer.sign(&manifest.content)).unwrap();
    entries
        .iter_mut()
        .find(|entry| entry.path == "MANIFEST.sig")
        .unwrap()
        .content = signature;
}

fn write_fixture(path: &Path, entries: &[TestEntry]) {
    let encoder = ParCompressBuilder::<Mgzip>::new()
        .buffer_size(crate::ccs::CCS_BUDGET.archive_compression_block_bytes)
        .unwrap()
        .num_threads(1)
        .unwrap()
        .compression_level(Compression::default())
        .from_writer(File::create(path).unwrap());
    let mut archive = Builder::new(encoder);
    for entry in entries {
        let mut header = Header::new_gnu();
        header.set_path(&entry.path).unwrap();
        header.set_entry_type(entry.entry_type);
        header.set_mode(if entry.entry_type.is_dir() {
            0o755
        } else {
            0o644
        });
        header.set_size(entry.content.len() as u64);
        header.set_cksum();
        archive
            .append_data(&mut header, &entry.path, entry.content.as_slice())
            .unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap();
}

#[test]
fn chunked_archive_emits_unique_objects_and_reconstructs_signed_whole_file() {
    let (_temp, entries, signer, path, bytes) = chunked_fixture();
    let manifest = entries
        .iter()
        .find(|entry| entry.path == "MANIFEST")
        .unwrap();
    let authority = CCS_BUDGET.decode_authority(&manifest.content).unwrap();
    let PackageKindV3::Package(package) = &authority.kind else {
        unreachable!()
    };
    let FileContentLayoutV3::FastCdcV2020 { chunks, .. } = &package.files[0].content_layout else {
        panic!("fixture must carry signed chunks")
    };
    let unique = chunks
        .iter()
        .map(|chunk| chunk.sha256.as_str())
        .collect::<BTreeSet<_>>();
    let archived_paths = entries
        .iter()
        .filter(|entry| entry.path.starts_with("objects/") && entry.entry_type.is_file())
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(archived_paths.len(), unique.len());
    assert!(
        archived_paths.windows(2).all(|pair| pair[0] < pair[1]),
        "layout objects must have one deterministic canonical order: {archived_paths:?}"
    );
    assert!(!unique.contains(crate::hash::sha256(&bytes).as_str()));

    let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
    let verified = verify_archive(
        &path,
        &policy,
        super::super::object_sink::ObjectDestination::Spool,
        1,
    )
    .unwrap();
    assert_eq!(verified.payload.files().len(), 2);
    for file in verified.payload.files() {
        let mut reconstructed = Vec::new();
        file.open_content()
            .unwrap()
            .read_to_end(&mut reconstructed)
            .unwrap();
        assert_eq!(reconstructed, bytes);
    }
}

#[test]
fn chunked_verification_rejects_reordered_authority_and_wrong_whole_digest() {
    let (temp, entries, signer, _, _) = chunked_fixture();
    let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);

    let mut reordered = entries.clone();
    resign_authority(&mut reordered, &signer, |authority| {
        let PackageKindV3::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        let FileContentLayoutV3::FastCdcV2020 { chunks, .. } = &mut package.files[0].content_layout
        else {
            unreachable!()
        };
        chunks.swap(0, 1);
    });
    let path = temp.path().join("reordered-chunks.ccs");
    write_fixture(&path, &reordered);
    let error = error_text(&path, &policy);
    assert!(
        error.contains("reconstructed") || error.contains("streamed chunk"),
        "{error}"
    );

    let mut wrong_whole = entries;
    resign_authority(&mut wrong_whole, &signer, |authority| {
        let PackageKindV3::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        package.files[0].content.as_mut().unwrap().sha256 = crate::hash::sha256(b"wrong");
    });
    let path = temp.path().join("wrong-whole-digest.ccs");
    write_fixture(&path, &wrong_whole);
    assert!(error_text(&path, &policy).contains("signed whole-file authority"));
}

fn error_text(path: &Path, policy: &TrustPolicy) -> String {
    match verify_archive(
        path,
        policy,
        super::super::object_sink::ObjectDestination::Spool,
        1,
    ) {
        Ok(_) => panic!("mutated archive unexpectedly verified"),
        Err(error) => format!("{error:#}"),
    }
}

fn decoded_tar(path: &Path) -> Vec<u8> {
    let mut decoded = Vec::new();
    crate::ccs::archive_framing::MgzipDecoder::new(File::open(path).unwrap())
        .read_to_end(&mut decoded)
        .unwrap();
    decoded
}

fn write_mgzip_payload(path: &Path, payload: &[u8]) {
    let mut encoder = ParCompressBuilder::<Mgzip>::new()
        .buffer_size(crate::ccs::CCS_BUDGET.archive_compression_block_bytes)
        .unwrap()
        .num_threads(1)
        .unwrap()
        .compression_level(Compression::default())
        .from_writer(File::create(path).unwrap());
    encoder.write_all(payload).unwrap();
    encoder.finish().unwrap();
}

#[test]
fn rejects_noncanonical_paths_and_nested_object_directories() {
    let (temp, entries, policy, _) = fixture();
    let error = canonical_path("./MANIFEST").unwrap_err();
    assert!(format!("{error:#}").contains("noncanonical CCS archive path"));

    let object_index = entries
        .iter()
        .position(|entry| entry.path.starts_with("objects/") && !entry.entry_type.is_dir())
        .unwrap();
    let mut nested = entries;
    nested.insert(
        object_index,
        TestEntry {
            path: "objects/ab/cd".to_string(),
            entry_type: EntryType::Directory,
            content: Vec::new(),
        },
    );
    let path = temp.path().join("nested-dir.ccs");
    write_fixture(&path, &nested);
    assert!(error_text(&path, &policy).contains("unknown CCS archive directory"));
}

#[test]
fn rejects_metadata_after_objects_and_duplicate_metadata() {
    let (temp, entries, policy, _) = fixture();
    let signature = entries
        .iter()
        .find(|entry| entry.path == "MANIFEST.sig")
        .unwrap()
        .clone();
    let object_index = entries
        .iter()
        .position(|entry| entry.path.starts_with("objects/") && !entry.entry_type.is_dir())
        .unwrap();
    let mut reordered = entries.clone();
    reordered.insert(object_index + 1, signature);
    let path = temp.path().join("metadata-after.ccs");
    write_fixture(&path, &reordered);
    assert!(error_text(&path, &policy).contains("appears after payload objects"));

    let directory = entries
        .iter()
        .find(|entry| entry.entry_type.is_dir())
        .unwrap()
        .clone();
    let mut directory_after = entries.clone();
    directory_after.insert(object_index + 1, directory);
    let path = temp.path().join("directory-after.ccs");
    write_fixture(&path, &directory_after);
    assert!(error_text(&path, &policy).contains("appears after payload objects"));

    let manifest = entries
        .iter()
        .find(|entry| entry.path == "MANIFEST")
        .unwrap()
        .clone();
    let mut duplicate = entries;
    duplicate.insert(1, manifest);
    let path = temp.path().join("duplicate-metadata.ccs");
    write_fixture(&path, &duplicate);
    assert!(error_text(&path, &policy).contains("duplicate MANIFEST entries"));
}

#[test]
fn rejects_unsigned_missing_duplicate_and_wrong_size_objects() {
    let (temp, entries, policy, _) = fixture();
    let object = entries
        .iter()
        .find(|entry| entry.path.starts_with("objects/") && !entry.entry_type.is_dir())
        .unwrap()
        .clone();

    let unsigned_bytes = b"unsigned".to_vec();
    let unsigned_hash = crate::hash::sha256(&unsigned_bytes);
    let mut unsigned = entries.clone();
    unsigned.push(TestEntry {
        path: format!("objects/{}/{}", &unsigned_hash[..2], &unsigned_hash[2..]),
        entry_type: EntryType::Regular,
        content: unsigned_bytes,
    });
    let path = temp.path().join("unsigned.ccs");
    write_fixture(&path, &unsigned);
    assert!(error_text(&path, &policy).contains("carries unsigned object"));

    let missing = entries
        .iter()
        .filter(|entry| entry.path != object.path)
        .cloned()
        .collect::<Vec<_>>();
    let path = temp.path().join("missing.ccs");
    write_fixture(&path, &missing);
    assert!(error_text(&path, &policy).contains("disagrees with archived set"));

    let mut duplicate = entries.clone();
    duplicate.push(object.clone());
    let path = temp.path().join("duplicate.ccs");
    write_fixture(&path, &duplicate);
    assert!(error_text(&path, &policy).contains("duplicate object"));

    let mut wrong_size = entries;
    wrong_size
        .iter_mut()
        .find(|entry| entry.path == object.path)
        .unwrap()
        .content
        .pop();
    let path = temp.path().join("wrong-size.ccs");
    write_fixture(&path, &wrong_size);
    assert!(error_text(&path, &policy).contains("signed authority requires"));
}

#[test]
fn rejects_truncated_archive_and_aggregate_metadata_without_allocation() {
    let (temp, _, policy, base) = fixture();
    let mut bytes = Vec::new();
    File::open(base).unwrap().read_to_end(&mut bytes).unwrap();
    let truncated = temp.path().join("truncated.ccs");
    let mut output = File::create(&truncated).unwrap();
    output.write_all(&bytes[..bytes.len() / 2]).unwrap();
    assert!(
        verify_archive(
            &truncated,
            &policy,
            super::super::object_sink::ObjectDestination::Spool,
            1,
        )
        .is_err()
    );

    // The aggregate control-document ceiling is derived from this
    // package's own census, so padding is refused without allocation.
    let authority = crate::ccs::v3::test_support::package_authority_with_one_file("stream");
    let census = crate::ccs::v3::authority_census(&authority).unwrap();
    let ceiling = CCS_BUDGET.metadata_bytes_ceiling(&census).unwrap();
    let mut state = MetadataState {
        census: Some(census),
        bytes_read: ceiling,
        ..MetadataState::default()
    };
    let error = reserve_metadata_budget(&mut state, 1).unwrap_err();
    assert!(format!("{error:#}").contains("metadata-bytes"), "{error:#}");
    assert_eq!(state.bytes_read, ceiling);
}

#[test]
fn rejects_missing_extra_and_nonzero_tar_terminator_bytes() {
    let (temp, _, policy, base) = fixture();
    let raw = decoded_tar(&base);
    assert!(raw.len() >= 1024);
    assert!(raw[raw.len() - 1024..].iter().all(|byte| *byte == 0));

    let missing = temp.path().join("missing-terminator.ccs");
    write_mgzip_payload(&missing, &raw[..raw.len() - 512]);
    assert!(error_text(&missing, &policy).contains("noncanonical length"));

    let extra = temp.path().join("extra-terminator.ccs");
    let mut extra_raw = raw.clone();
    extra_raw.extend_from_slice(&[0_u8; 512]);
    write_mgzip_payload(&extra, &extra_raw);
    assert!(error_text(&extra, &policy).contains("terminator/padding exceeds"));

    let nonzero = temp.path().join("nonzero-terminator.ccs");
    let mut nonzero_raw = raw;
    *nonzero_raw.last_mut().unwrap() = 1;
    write_mgzip_payload(&nonzero, &nonzero_raw);
    assert!(error_text(&nonzero, &policy).contains("non-zero data"));
}

#[test]
fn rejects_appended_tar_data_and_noncanonical_gzip_member() {
    let (temp, _, policy, base) = fixture();

    let appended_tar = temp.path().join("appended-tar.ccs");
    let mut raw = decoded_tar(&base);
    raw.extend_from_slice(b"unsigned appended tar bytes");
    write_mgzip_payload(&appended_tar, &raw);
    assert!(error_text(&appended_tar, &policy).contains("non-zero data"));

    let appended_gzip = temp.path().join("appended-gzip.ccs");
    let mut bytes = Vec::new();
    File::open(&base).unwrap().read_to_end(&mut bytes).unwrap();
    let mut member = GzEncoder::new(Vec::new(), Compression::default());
    member.write_all(b"second member").unwrap();
    bytes.extend(member.finish().unwrap());
    std::fs::write(&appended_gzip, bytes).unwrap();
    assert!(error_text(&appended_gzip, &policy).contains("noncanonical MGZIP header"));
}

#[test]
fn reordered_and_substituted_mgzip_blocks_fail_before_archive_authority_escapes() {
    let (temp, _, signer, base, _) = chunked_fixture();
    let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
    let bytes = std::fs::read(base).unwrap();
    let mut ranges = Vec::new();
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let frame_bytes =
            u32::from_le_bytes(bytes[offset + 16..offset + 20].try_into().unwrap()) as usize;
        ranges.push(offset..offset + frame_bytes);
        offset += frame_bytes;
    }
    assert_eq!(offset, bytes.len());
    assert!(
        ranges.len() >= 2,
        "fixture must span canonical MGZIP blocks"
    );

    let reordered = temp.path().join("reordered-mgzip.ccs");
    let mut reordered_bytes = Vec::with_capacity(bytes.len());
    reordered_bytes.extend_from_slice(&bytes[ranges[1].clone()]);
    reordered_bytes.extend_from_slice(&bytes[ranges[0].clone()]);
    for range in &ranges[2..] {
        reordered_bytes.extend_from_slice(&bytes[range.clone()]);
    }
    std::fs::write(&reordered, reordered_bytes).unwrap();
    assert!(!error_text(&reordered, &policy).is_empty());

    let substituted = temp.path().join("substituted-mgzip.ccs");
    let mut substituted_bytes = Vec::with_capacity(bytes.len());
    substituted_bytes.extend_from_slice(&bytes[ranges[0].clone()]);
    substituted_bytes.extend_from_slice(&bytes[ranges[0].clone()]);
    for range in &ranges[2..] {
        substituted_bytes.extend_from_slice(&bytes[range.clone()]);
    }
    std::fs::write(&substituted, substituted_bytes).unwrap();
    assert!(!error_text(&substituted, &policy).is_empty());
}

#[test]
fn signed_object_authority_uses_lowercase_sha256_and_u64_sizes() {
    let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("stream");
    {
        let PackageKindV3::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        package.files[0].content.as_mut().unwrap().size = u64::from(u32::MAX) + 1;
    }
    let (objects, _) = expected_objects(&authority).unwrap();
    assert_eq!(
        objects.values().copied().collect::<Vec<_>>(),
        vec![u64::from(u32::MAX) + 1]
    );

    let PackageKindV3::Package(package) = &mut authority.kind else {
        unreachable!()
    };
    package.files[0].content.as_mut().unwrap().sha256 = "A".repeat(64);
    let error = expected_objects(&authority).unwrap_err();
    assert!(format!("{error:#}").contains("canonical lowercase SHA-256"));
}
