// crates/conary-core/src/ccs/convert/converter/tests/metrics.rs

use super::*;

#[test]
fn conversion_metrics_prove_one_pass_chunk_derivation_and_object_staging() {
    let temp_dir = tempfile::tempdir().unwrap();
    let metadata = make_test_metadata();
    let converter = passive_test_converter(temp_dir.path());
    let bytes = vec![0x5a; crate::ccs::chunking::MIN_CHUNK_SIZE as usize * 8];
    let files = vec![extracted_file("/usr/share/test/data", &bytes, 0o644)];

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &files,
            "rpm",
            "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        )
        .unwrap();
    let ccs_write = &result.metrics.ccs_write;
    let independent_output = std::fs::read(&result.package_path).unwrap();

    assert_eq!(ccs_write.payload_files_examined, 1);
    assert!(ccs_write.payload_chunks_derived > 0);
    assert!(ccs_write.unique_payload_chunks_derived > 0);
    assert_eq!(ccs_write.payload_source_files_opened, 1);
    assert_eq!(ccs_write.payload_source_bytes_read, bytes.len() as u64);
    assert_eq!(ccs_write.payload_source_files_reopened, 0);
    assert_eq!(ccs_write.payload_source_bytes_reread, 0);
    assert_eq!(
        ccs_write.payload_chunk_identity_bytes_hashed,
        bytes.len() as u64
    );
    assert_eq!(
        ccs_write.payload_whole_content_bytes_hashed,
        bytes.len() as u64
    );
    assert_eq!(
        ccs_write.payload_crypto_bytes_hashed,
        bytes.len() as u64 * 2
    );
    assert_eq!(
        ccs_write.staged_object_bytes_written + ccs_write.staged_object_deduplicated_bytes,
        bytes.len() as u64
    );
    assert_eq!(
        ccs_write.staged_object_deduplications + ccs_write.staged_unique_objects,
        ccs_write.payload_chunks_derived
    );
    assert_eq!(ccs_write.staged_object_canonical_bytes_reread, 0);
    assert_eq!(ccs_write.staged_object_file_syncs, 0);
    assert_eq!(ccs_write.staged_object_shard_syncs, 0);
    assert!(ccs_write.archive_members_traversed > 0);
    assert!(ccs_write.archive_input_bytes >= bytes.len() as u64);
    assert!(ccs_write.ccs_output_bytes > 0);
    assert_eq!(ccs_write.ccs_output_bytes, independent_output.len() as u64);
    assert_eq!(
        ccs_write.ccs_output_bytes_hashed,
        ccs_write.ccs_output_bytes
    );
    assert_eq!(
        ccs_write.ccs_output_sha256,
        crate::hash::sha256(&independent_output)
    );
    assert_eq!(
        ccs_write.maximum_retained_staging_bytes,
        ccs_write.archive_input_bytes + ccs_write.ccs_output_bytes
    );
}
