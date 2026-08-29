// crates/conary-core/src/repository/catalog/portable_integrity/tests.rs

use std::io::{Seek, SeekFrom};

use super::*;

fn open_catalog(bytes: &[u8]) -> (tempfile::TempDir, PathBuf, File, CatalogArtifactV1) {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("catalog.sqlite");
    fs::write(&path, bytes).unwrap();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let artifact = CatalogArtifactV1 {
        sha256: crate::hash::sha256(bytes),
        size: bytes.len() as u64,
    };
    (temp, path, file, artifact)
}

fn attestation(bytes: &[u8]) -> PortableManifestAttestationV1 {
    attestation_for_bytes(bytes)
}

fn manifest_fixture() -> (
    tempfile::TempDir,
    PathBuf,
    File,
    CatalogArtifactV1,
    PortableChunkManifestV1,
) {
    let mut bytes = vec![b'a'; PORTABLE_CHUNK_SIZE_V1 as usize];
    bytes.extend(vec![b'b'; PORTABLE_CHUNK_SIZE_V1 as usize]);
    bytes.extend(b"short-last-chunk");
    let (temp, path, file, artifact) = open_catalog(&bytes);
    let manifest = PortableChunkManifestV1::build(&file, &artifact).unwrap();
    (temp, path, file, artifact, manifest)
}

#[test]
fn build_round_trips_and_preserves_fd_offset() {
    let (_temp, _path, mut file, artifact, manifest) = manifest_fixture();
    file.seek(SeekFrom::Start(7)).unwrap();
    let rebuilt = PortableChunkManifestV1::build(&file, &artifact).unwrap();
    assert_eq!(file.stream_position().unwrap(), 7);
    assert_eq!(rebuilt, manifest);
    assert_eq!(manifest.catalog_size(), artifact.size);
    assert_eq!(manifest.artifact_sha256(), artifact.sha256);
    assert_eq!(manifest.chunk_size(), PORTABLE_CHUNK_SIZE_V1);
    assert_eq!(manifest.chunk_count(), 3);
    assert_eq!(manifest.chunk_range(2).unwrap().length, 16);

    let bytes = manifest.encode().unwrap();
    let expected_size = portable_manifest_size_v1(3).unwrap();
    assert_eq!(bytes.len() as u64, expected_size);
    let decoded = PortableChunkManifestV1::decode_attested(
        &bytes,
        &manifest.attestation().unwrap(),
        &artifact,
    )
    .unwrap();
    assert_eq!(decoded, manifest);
    assert_eq!(
        decoded.read_verified_chunk(&file, 2).unwrap(),
        b"short-last-chunk"
    );
}

#[test]
fn build_refuses_bytes_not_matching_expected_artifact() {
    let (_temp, path, file, artifact) = open_catalog(b"original catalog bytes");
    fs::write(&path, b"mutated! catalog bytes").unwrap();
    let error = PortableChunkManifestV1::build(&file, &artifact).unwrap_err();
    assert!(matches!(
        error,
        PortableIntegrityError::ArtifactDigestMismatch { .. }
    ));
}

#[test]
fn attestation_rejects_manifest_tamper() {
    let (_temp, _path, _file, artifact, manifest) = manifest_fixture();
    let expected = manifest.attestation().unwrap();
    let mut bytes = manifest.encode().unwrap();
    bytes[64] ^= 0x80;
    let error = PortableChunkManifestV1::decode_attested(&bytes, &expected, &artifact).unwrap_err();
    assert!(matches!(
        error,
        PortableIntegrityError::ManifestDigestMismatch { .. }
    ));
}

#[test]
fn strict_decode_rejects_truncation_and_trailing_bytes() {
    let (_temp, _path, _file, artifact, manifest) = manifest_fixture();
    let mut truncated = manifest.encode().unwrap();
    truncated.pop();
    let error =
        PortableChunkManifestV1::decode_attested(&truncated, &attestation(&truncated), &artifact)
            .unwrap_err();
    assert!(matches!(
        error,
        PortableIntegrityError::ManifestAttestationSizeMismatch { .. }
    ));

    let mut trailing = manifest.encode().unwrap();
    trailing.push(0);
    let error =
        PortableChunkManifestV1::decode_attested(&trailing, &attestation(&trailing), &artifact)
            .unwrap_err();
    assert!(matches!(
        error,
        PortableIntegrityError::ManifestAttestationSizeMismatch { .. }
    ));
}

#[test]
fn strict_decode_rejects_malformed_header_fields_and_count() {
    let (_temp, _path, _file, artifact, manifest) = manifest_fixture();
    let original = manifest.encode().unwrap();

    let mut bytes = original.clone();
    bytes[0] ^= 1;
    let error = PortableChunkManifestV1::decode_attested(&bytes, &attestation(&bytes), &artifact)
        .unwrap_err();
    assert!(matches!(error, PortableIntegrityError::InvalidMagic));

    let mut bytes = original.clone();
    bytes[8..10].copy_from_slice(&2_u16.to_le_bytes());
    let error = PortableChunkManifestV1::decode_attested(&bytes, &attestation(&bytes), &artifact)
        .unwrap_err();
    assert!(matches!(
        error,
        PortableIntegrityError::UnsupportedSchema { actual: 2, .. }
    ));

    let mut bytes = original.clone();
    bytes[10..12].copy_from_slice(&2_u16.to_le_bytes());
    let error = PortableChunkManifestV1::decode_attested(&bytes, &attestation(&bytes), &artifact)
        .unwrap_err();
    assert!(matches!(
        error,
        PortableIntegrityError::UnsupportedHashAlgorithm { actual: 2, .. }
    ));

    let mut bytes = original.clone();
    bytes[12..16].copy_from_slice(&4096_u32.to_le_bytes());
    let error = PortableChunkManifestV1::decode_attested(&bytes, &attestation(&bytes), &artifact)
        .unwrap_err();
    assert!(matches!(
        error,
        PortableIntegrityError::UnexpectedChunkSize { actual: 4096, .. }
    ));

    let mut bytes = original;
    bytes[24..32].copy_from_slice(&4_u64.to_le_bytes());
    let error = PortableChunkManifestV1::decode_attested(&bytes, &attestation(&bytes), &artifact)
        .unwrap_err();
    assert!(matches!(
        error,
        PortableIntegrityError::ChunkCountMismatch { actual: 4, .. }
    ));
}

#[test]
fn helper_arithmetic_is_exact_and_checked() {
    assert_eq!(portable_chunk_count_v1(0).unwrap(), 0);
    assert_eq!(portable_chunk_count_v1(1).unwrap(), 1);
    assert_eq!(
        portable_chunk_count_v1(u64::from(PORTABLE_CHUNK_SIZE_V1)).unwrap(),
        1
    );
    assert_eq!(
        portable_chunk_count_v1(u64::from(PORTABLE_CHUNK_SIZE_V1) + 1).unwrap(),
        2
    );
    assert!(matches!(
        portable_manifest_size_v1(u64::MAX),
        Err(PortableIntegrityError::ManifestLengthOverflow)
    ));
}

#[test]
fn chunk_hash_binds_position_and_last_chunk_length() {
    let (_temp, _path, file, _artifact, manifest) = manifest_fixture();
    let first = manifest.read_verified_chunk(&file, 0).unwrap();
    let second = manifest.read_verified_chunk(&file, 1).unwrap();

    let swapped = manifest.verify_chunk_bytes(0, &second).unwrap_err();
    assert!(matches!(
        swapped,
        PortableIntegrityError::ChunkDigestMismatch { index: 0, .. }
    ));
    assert!(matches!(
        manifest.verify_chunk_bytes(2, &first),
        Err(PortableIntegrityError::ChunkLengthMismatch { index: 2, .. })
    ));
    let mut ambiguous = b"short-last-chunk".to_vec();
    ambiguous.push(0);
    assert!(matches!(
        manifest.verify_chunk_bytes(2, &ambiguous),
        Err(PortableIntegrityError::ChunkLengthMismatch { index: 2, .. })
    ));
}

#[test]
fn verified_chunk_read_rejects_catalog_tamper() {
    let (_temp, _path, file, _artifact, manifest) = manifest_fixture();
    file.write_at(b"x", 0).unwrap();
    file.sync_all().unwrap();
    assert!(matches!(
        manifest.read_verified_chunk(&file, 0),
        Err(PortableIntegrityError::ChunkDigestMismatch { index: 0, .. })
    ));
}

#[test]
fn durable_writer_is_exclusive_and_round_trips() {
    let (temp, _catalog_path, _file, artifact, manifest) = manifest_fixture();
    let path = temp.path().join("catalog.portable-integrity");
    let expected = write_portable_chunk_manifest_v1(&path, &manifest).unwrap();
    assert_eq!(expected, manifest.attestation().unwrap());
    let decoded = read_portable_chunk_manifest_v1(&path, &expected, &artifact).unwrap();
    assert_eq!(decoded, manifest);

    let before = fs::read(&path).unwrap();
    assert!(matches!(
        write_portable_chunk_manifest_v1(&path, &manifest),
        Err(PortableIntegrityError::AlreadyExists(existing)) if existing == path
    ));
    assert_eq!(fs::read(&path).unwrap(), before);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn manifest_reader_never_follows_symlinks() {
    use std::os::unix::fs::symlink;

    let (temp, _catalog_path, _file, artifact, manifest) = manifest_fixture();
    let real = temp.path().join("real-manifest");
    let link = temp.path().join("linked-manifest");
    let expected = write_portable_chunk_manifest_v1(&real, &manifest).unwrap();
    symlink(&real, &link).unwrap();
    assert!(matches!(
        read_portable_chunk_manifest_v1(&link, &expected, &artifact),
        Err(PortableIntegrityError::NotRegularFile(path)) if path == link
    ));
}
