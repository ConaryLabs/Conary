// conary-core/src/ccs/convert/converter/tests/metrics.rs

use super::*;

#[test]
fn conversion_metrics_expose_each_chunked_payload_pass_and_ephemeral_staging() {
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
    let metrics = result.metrics;
    let ccs_write = metrics.ccs_write;

    assert_eq!(metrics.payload_files_examined, 1);
    assert_eq!(metrics.payload_reference_bytes_read, bytes.len() as u64);
    assert_eq!(metrics.payload_reference_bytes_hashed, bytes.len() as u64);
    assert!(metrics.payload_chunks_derived > 0);
    assert!(metrics.unique_payload_chunks_derived > 0);
    assert_eq!(ccs_write.payload_files_traversed, 1);
    assert_eq!(ccs_write.payload_bytes_read, bytes.len() as u64);
    assert_eq!(ccs_write.chunk_reference_bytes_hashed, bytes.len() as u64);
    assert_eq!(
        ccs_write.reconstructed_content_bytes_hashed,
        bytes.len() as u64
    );
    assert_eq!(
        ccs_write.temporary_object_incoming_bytes_hashed,
        bytes.len() as u64
    );
    assert_eq!(ccs_write.temporary_object_bytes_written, bytes.len() as u64);
    assert_eq!(
        ccs_write.temporary_object_hits + ccs_write.temporary_object_misses,
        metrics.payload_chunks_derived
    );
    assert_eq!(ccs_write.temporary_object_file_syncs, 0);
    assert_eq!(ccs_write.temporary_object_shard_syncs, 0);
    assert!(ccs_write.archive_members_traversed > 0);
    assert!(ccs_write.archive_input_bytes >= bytes.len() as u64);
    assert!(ccs_write.ccs_output_bytes > 0);
    assert_eq!(
        ccs_write.maximum_retained_staging_bytes,
        ccs_write.archive_input_bytes + ccs_write.ccs_output_bytes
    );
}
