// crates/conary-core/src/ccs/builder/payload_preparation/tests.rs

use super::*;
use crate::packages::payload::ReopenablePayload;
use std::sync::Arc;

fn payload(path: &str, bytes: &[u8]) -> PackagePayloadFile {
    payload_with_authority(path, bytes, bytes)
}

fn payload_with_authority(
    path: &str,
    source_bytes: &[u8],
    authority_bytes: &[u8],
) -> PackagePayloadFile {
    PackagePayloadFile::new(
        path.to_string(),
        PayloadNode::regular(0o644),
        Some(PayloadContentAuthority {
            sha256: crate::hash::sha256(authority_bytes),
            size: authority_bytes.len() as u64,
        }),
        Some(ReopenablePayload::from_in_memory_bytes(Arc::<[u8]>::from(
            source_bytes.to_vec(),
        ))),
    )
    .unwrap()
}

fn stable_object_list_fixture() -> Vec<u8> {
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    (0..768 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn chunked_authority(path: &str, bytes: &[u8], chunks: Vec<ChunkReference>) -> AuthorityDocumentV3 {
    let mut authority =
        crate::ccs::v3::test_support::package_authority_with_one_file("prepared-chunks");
    let PackageKindV3::Package(package) = &mut authority.kind else {
        unreachable!()
    };
    package.files[0].path = path.to_string();
    package.files[0].node = PayloadNode::regular(0o644);
    package.files[0].content = Some(PayloadContentAuthority {
        sha256: crate::hash::sha256(bytes),
        size: bytes.len() as u64,
    });
    package.files[0].content_layout = FileContentLayoutV3::FastCdcV2020 {
        min_size: MIN_CHUNK_SIZE,
        average_size: AVG_CHUNK_SIZE,
        max_size: MAX_CHUNK_SIZE,
        chunks,
    };
    authority.components.get_mut("main").unwrap().total_size = bytes.len() as u64;
    authority
}

#[test]
fn streamed_preparation_preserves_the_stable_full_object_list() {
    let bytes = stable_object_list_fixture();
    let temp = tempfile::tempdir().unwrap();
    let prepared =
        PreparedPayloadObjectSet::prepare(&[payload("/stable-object-list", &bytes)], temp.path())
            .unwrap();
    let actual = prepared.chunks_for("/stable-object-list").unwrap().unwrap();
    let expected = [
        (
            "04bea0eadad0893ab43b0db5e503ce9e575c0a79e2f1d4a04b79956a961a15ac",
            75_896,
        ),
        (
            "29fb403e0548031d0ac6a33a4d3ecd6ae3e54862c123ab355d735d5f39c9267c",
            21_072,
        ),
        (
            "04113790af2f2a345556d84c787edebef410ea8d03f6e893ee9566487cc60ddc",
            67_367,
        ),
        (
            "795c90d7b69a8452a2867af86970553e448ac20fb6a46ad257df711df6972f94",
            52_640,
        ),
        (
            "59d3aca68160be78a15814d9b412558d8899d0886de023af25ba932fcfd3b0c4",
            71_617,
        ),
        (
            "a8985d49674f2d622157a9444aebcd1e44c02a06ab64093dd8f96171196ac49e",
            85_160,
        ),
        (
            "6aee9cde4406f81fcf860902cde51f5354b11bb447e95ead25e8206913ee5fa7",
            98_811,
        ),
        (
            "e87f4b64b96319c6763891cc36a475bb51917442e5e2c36abe7dcc3066fd007f",
            73_038,
        ),
        (
            "f2ce3db37b04cf68eda67cf51b04252bc3dc40dea2a48b0eae62453f3abb5774",
            27_502,
        ),
        (
            "65f0fbad0221ab4e2a6380893d3a540ef75f08b921a2755a27212afb7461935b",
            80_029,
        ),
        (
            "f8bac509de2f75838219c539a0fa99f0bb80dde8753a7ecc08edc1f8f5ce1314",
            78_124,
        ),
        (
            "14dd8c75fc6f005d288a5367a9d56f37403b583905110b650ec92a116417c490",
            55_176,
        ),
    ]
    .map(|(sha256, size)| ChunkReference {
        sha256: sha256.to_string(),
        size,
    });

    assert_eq!(actual, expected);
    let expected_inventory = expected
        .iter()
        .map(|reference| (reference.sha256.clone(), u64::from(reference.size)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(prepared.inventory(), &expected_inventory);

    let mut reconstructed = Vec::with_capacity(bytes.len());
    for reference in &expected {
        prepared
            .open_object(&reference.sha256)
            .unwrap()
            .read_to_end(&mut reconstructed)
            .unwrap();
    }
    assert_eq!(reconstructed, bytes);

    let authority = chunked_authority("/stable-object-list", &bytes, expected.to_vec());
    prepared.reconcile_authority(&authority).unwrap();
}

#[test]
fn projected_chunk_sequences_fail_closed_before_signing() {
    let path = "/projected-chunks";
    let bytes = stable_object_list_fixture();
    let initial_temp = tempfile::tempdir().unwrap();
    let prepared =
        PreparedPayloadObjectSet::prepare(&[payload(path, &bytes)], initial_temp.path()).unwrap();
    let chunks = prepared.chunks_for(path).unwrap().unwrap();
    let authority = chunked_authority(path, &bytes, chunks.clone());
    prepared.reconcile_authority(&authority).unwrap();

    let mut missing = chunks.clone();
    missing.remove(3);
    let mut extra = chunks.clone();
    extra.push(chunks[0].clone());
    let mut reordered = chunks.clone();
    reordered.swap(2, 3);
    let mut discontinuous = chunks.clone();
    discontinuous[0].size -= 1;
    discontinuous[1].size += 1;

    for drifted_chunks in [missing, extra, reordered, discontinuous] {
        let drifted = chunked_authority(path, &bytes, drifted_chunks);
        let temp = tempfile::tempdir().unwrap();
        assert!(
            PreparedPayloadObjectSet::prepare_for_authority(
                &[payload(path, &bytes)],
                &drifted,
                temp.path(),
            )
            .is_err()
        );
    }

    let mut wrong_layout = authority;
    let PackageKindV3::Package(package) = &mut wrong_layout.kind else {
        unreachable!()
    };
    package.files[0].content_layout = FileContentLayoutV3::WholeObject;
    assert!(prepared.reconcile_authority(&wrong_layout).is_err());
}

#[test]
fn exact_inventory_reconciliation_rejects_missing_extra_and_conflicting_size() {
    let first = "11".repeat(32);
    let second = "22".repeat(32);
    let extra = "33".repeat(32);
    let expected = BTreeMap::from([(first.clone(), 7), (second.clone(), 9)]);

    let missing = BTreeMap::from([(first.clone(), 7)]);
    let missing_error = reconcile_object_inventory(&expected, &missing)
        .unwrap_err()
        .to_string();
    assert!(missing_error.contains(&second));

    let with_extra = BTreeMap::from([(first.clone(), 7), (second.clone(), 9), (extra.clone(), 5)]);
    let extra_error = reconcile_object_inventory(&expected, &with_extra)
        .unwrap_err()
        .to_string();
    assert!(extra_error.contains(&extra));

    let conflicting = BTreeMap::from([(first.clone(), 8), (second, 9)]);
    let size_error = reconcile_object_inventory(&expected, &conflicting)
        .unwrap_err()
        .to_string();
    assert!(size_error.contains(&format!("{first}:7!=8")));
}

#[test]
fn mixed_payloads_open_once_and_count_each_physical_hash_once() {
    let temp = tempfile::tempdir().unwrap();
    let small = vec![0x31; MIN_CHUNK_SIZE as usize - 1];
    let large = (0..MIN_CHUNK_SIZE as usize * 5)
        .map(|index| (index.wrapping_mul(37) % 251) as u8)
        .collect::<Vec<_>>();
    let payloads = vec![
        payload("/small-a", &small),
        payload("/small-b", &small),
        payload("/threshold", &large),
    ];

    let prepared = PreparedPayloadObjectSet::prepare(&payloads, temp.path()).unwrap();
    let metrics = prepared.metrics();
    let source_bytes = (small.len() * 2 + large.len()) as u64;

    assert_eq!(metrics.files_examined, 3);
    assert_eq!(metrics.source_files_opened, 3);
    assert_eq!(metrics.source_bytes_read, source_bytes);
    assert_eq!(metrics.source_files_reopened, 0);
    assert_eq!(metrics.source_bytes_reread, 0);
    assert_eq!(metrics.chunk_identity_bytes_hashed, large.len() as u64);
    assert_eq!(metrics.whole_content_bytes_hashed, source_bytes);
    assert_eq!(
        metrics.crypto_bytes_hashed,
        source_bytes + large.len() as u64
    );
    assert_eq!(
        metrics.staged_object_bytes_written + metrics.staged_object_deduplicated_bytes,
        source_bytes
    );
    assert!(metrics.staged_object_deduplications >= 1);
    assert_eq!(metrics.staged_object_canonical_bytes_reread, 0);
    assert_eq!(
        metrics.staged_unique_objects,
        prepared.inventory().len() as u64
    );
    assert!(prepared.chunks_for("/small-a").unwrap().is_none());
    assert!(prepared.chunks_for("/threshold").unwrap().is_some());
}

#[test]
fn duplicate_whole_objects_are_each_authenticated_and_written_once() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = b"duplicate whole-object payload";
    let prepared = PreparedPayloadObjectSet::prepare(
        &[payload("/whole-a", bytes), payload("/whole-b", bytes)],
        temp.path(),
    )
    .unwrap();
    let metrics = prepared.metrics();

    assert_eq!(metrics.source_files_opened, 2);
    assert_eq!(metrics.source_bytes_read, bytes.len() as u64 * 2);
    assert_eq!(metrics.chunk_identity_bytes_hashed, 0);
    assert_eq!(metrics.whole_content_bytes_hashed, bytes.len() as u64 * 2);
    assert_eq!(metrics.staged_unique_objects, 1);
    assert_eq!(metrics.staged_object_deduplications, 1);
    assert_eq!(metrics.staged_object_bytes_written, bytes.len() as u64);
    assert_eq!(metrics.staged_object_deduplicated_bytes, bytes.len() as u64);
    assert_eq!(metrics.staged_object_canonical_bytes_reread, 0);
}

#[test]
fn zero_threshold_minus_one_and_threshold_use_the_exact_layout_split() {
    let temp = tempfile::tempdir().unwrap();
    let empty = Vec::new();
    let below = vec![0x42; MIN_CHUNK_SIZE as usize - 1];
    let threshold = vec![0x43; MIN_CHUNK_SIZE as usize];
    let payloads = vec![
        payload("/empty-a", &empty),
        payload("/empty-b", &empty),
        payload("/below", &below),
        payload("/threshold", &threshold),
    ];

    let prepared = PreparedPayloadObjectSet::prepare(&payloads, temp.path()).unwrap();

    assert!(prepared.chunks_for("/empty-a").unwrap().is_none());
    assert!(prepared.chunks_for("/below").unwrap().is_none());
    assert!(prepared.chunks_for("/threshold").unwrap().is_some());
    assert_eq!(prepared.metrics().source_files_opened, 4);
    assert!(prepared.metrics().staged_object_deduplications >= 1);
    assert_eq!(prepared.metrics().staged_object_canonical_bytes_reread, 0);
}

#[test]
fn repeated_large_chunks_are_hashed_once_per_occurrence_but_written_once() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = vec![0xa7; MAX_CHUNK_SIZE as usize * 3];
    let prepared =
        PreparedPayloadObjectSet::prepare(&[payload("/repeated", &bytes)], temp.path()).unwrap();
    let metrics = prepared.metrics();

    assert_eq!(metrics.source_files_opened, 1);
    assert_eq!(metrics.source_bytes_read, bytes.len() as u64);
    assert_eq!(metrics.chunks_derived, 3);
    assert_eq!(metrics.unique_chunks_derived, 1);
    assert_eq!(metrics.staged_unique_objects, 1);
    assert_eq!(metrics.staged_object_deduplications, 2);
    assert_eq!(
        metrics.staged_object_bytes_written,
        u64::from(MAX_CHUNK_SIZE)
    );
    assert_eq!(
        metrics.staged_object_deduplicated_bytes,
        u64::from(MAX_CHUNK_SIZE) * 2
    );
    assert_eq!(metrics.chunk_identity_bytes_hashed, bytes.len() as u64);
    assert_eq!(metrics.whole_content_bytes_hashed, bytes.len() as u64);
}

#[test]
fn hardlink_non_owner_is_examined_but_never_opened_or_staged() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = b"rpm hardlink owner bytes";
    let mut owner = payload("/usr/bin/owner", bytes);
    owner.node.kind = crate::payload::PayloadNodeKind::Regular {
        hardlink_identity: Some("rpm:1:7".to_string()),
    };
    let mut alias_node = owner.node.clone();
    alias_node.kind = crate::payload::PayloadNodeKind::Hardlink {
        target: "/usr/bin/owner".to_string(),
        identity: "rpm:1:7".to_string(),
    };
    let alias =
        PackagePayloadFile::new("/usr/bin/alias".to_string(), alias_node, None, None).unwrap();

    let prepared = PreparedPayloadObjectSet::prepare(&[owner, alias], temp.path()).unwrap();
    let metrics = prepared.metrics();

    assert_eq!(metrics.files_examined, 2);
    assert_eq!(metrics.source_files_opened, 1);
    assert_eq!(metrics.source_bytes_read, bytes.len() as u64);
    assert_eq!(metrics.staged_unique_objects, 1);
    assert_eq!(prepared.inventory().len(), 1);
    assert!(prepared.chunks_for("/usr/bin/alias").unwrap().is_none());
}

#[test]
fn changed_short_and_extra_sources_fail_before_returning_prepared_authority() {
    let expected = b"expected payload bytes";
    for source in [
        b"mutated!payload bytes".as_slice(),
        &expected[..expected.len() - 1],
        b"expected payload bytes!".as_slice(),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let result = PreparedPayloadObjectSet::prepare(
            &[payload_with_authority("/payload", source, expected)],
            temp.path(),
        );
        assert!(result.is_err());
    }
}

#[test]
fn duplicate_paths_fail_before_opening_payload_sources() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = b"same";
    let result = PreparedPayloadObjectSet::prepare(
        &[payload("/duplicate", bytes), payload("/duplicate", bytes)],
        temp.path(),
    );
    let error = match result {
        Ok(_) => panic!("duplicate payload paths unexpectedly prepared"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("more than once"));
}

#[test]
fn projected_authority_must_match_prepared_layout_and_inventory_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = b"hello world\n";
    let payloads = [PackagePayloadFile::new(
        "/usr/bin/hello".to_string(),
        PayloadNode::regular(0o755),
        Some(PayloadContentAuthority {
            sha256: crate::hash::sha256(bytes),
            size: bytes.len() as u64,
        }),
        Some(ReopenablePayload::from_in_memory_bytes(Arc::<[u8]>::from(
            bytes.to_vec(),
        ))),
    )
    .unwrap()];
    let prepared = PreparedPayloadObjectSet::prepare(&payloads, temp.path()).unwrap();
    let authority = crate::ccs::v3::test_support::package_authority_with_one_file("prepared");
    prepared.reconcile_authority(&authority).unwrap();

    let mut drifted = authority.clone();
    let PackageKindV3::Package(package) = &mut drifted.kind else {
        unreachable!()
    };
    package.files[0].content.as_mut().unwrap().sha256 = "0".repeat(64);
    let error = prepared
        .reconcile_authority(&drifted)
        .unwrap_err()
        .to_string();
    assert!(error.contains("disagrees with projected v3 authority"));
}
