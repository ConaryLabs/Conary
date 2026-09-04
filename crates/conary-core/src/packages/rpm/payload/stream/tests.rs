// conary-core/src/packages/rpm/payload/stream/tests.rs

use super::super::header::DeclaredDigest;
use super::*;
use std::io;

fn header_record(mode: u32, size: u64, link_target: Option<&str>) -> HeaderRecord {
    HeaderRecord {
        path: "/usr/lib/.build-id/fixture".to_string(),
        path_kind: super::super::header::HeaderPathKind::Deployable,
        mode,
        user: "root".to_string(),
        group: "root".to_string(),
        mtime: 0,
        size,
        ghost: false,
        digest: None,
        link_target: link_target.map(str::to_string),
        caps: None,
        ima_signature: None,
        device: 0,
        inode: 0,
        rdev: 0,
        nlink: None,
    }
}

fn regular_header_record(
    size: u64,
    algorithm: RpmFileDigestAlgorithm,
    digest: &str,
) -> HeaderRecord {
    let mut record = header_record(libc::S_IFREG | 0o644, size, None);
    record.digest = Some(DeclaredDigest {
        algorithm,
        hex: digest.to_string(),
    });
    record
}

struct ZeroReader {
    max_requested: usize,
}

impl Read for ZeroReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.max_requested = self.max_requested.max(buffer.len());
        buffer.fill(0);
        Ok(buffer.len())
    }
}

struct StopWriter;

impl Write for StopWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("injected bounded sink stop"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn stripped_large_member_uses_u64_and_fixed_buffers() {
    let mut reader = ZeroReader { max_requested: 0 };
    let error = copy_exact_payload(
        &mut reader,
        &mut StopWriter,
        u64::from(u32::MAX) + 1,
        RpmFileDigestAlgorithm::Sha2_256,
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("injected bounded sink stop"));
    assert!(reader.max_requested <= PAYLOAD_IO_BUFFER_SIZE);
}

#[test]
fn sole_spool_copy_computes_every_pinned_digest_and_reuses_sha256() {
    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    for (algorithm, expected) in [
        (
            RpmFileDigestAlgorithm::Md5,
            "900150983cd24fb0d6963f7d28e17f72",
        ),
        (
            RpmFileDigestAlgorithm::Sha1,
            "a9993e364706816aba3e25717850c26c9cd0d89d",
        ),
        (
            RpmFileDigestAlgorithm::Sha2_224,
            "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7",
        ),
        (RpmFileDigestAlgorithm::Sha2_256, SHA256_ABC),
        (
            RpmFileDigestAlgorithm::Sha2_384,
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
                 8086072ba1e7cc2358baeca134c825a7",
        ),
        (
            RpmFileDigestAlgorithm::Sha2_512,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                 2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        ),
        (
            RpmFileDigestAlgorithm::Sha3_256,
            "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532",
        ),
        (
            RpmFileDigestAlgorithm::Sha3_512,
            "b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712\
                 e10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0",
        ),
    ] {
        let expected = expected.replace(char::is_whitespace, "");
        let mut output = Vec::new();
        let (computed, crc) = copy_exact_payload(
            &mut io::Cursor::new(b"abc"),
            &mut output,
            3,
            algorithm,
            false,
        )
        .unwrap();

        assert_eq!(output, b"abc");
        assert_eq!(computed.sha256, SHA256_ABC);
        assert_eq!(computed.declared.algorithm, algorithm);
        assert_eq!(computed.declared.hex, expected);
        assert_eq!(
            computed.bytes_hashed,
            if algorithm == RpmFileDigestAlgorithm::Sha2_256 {
                3
            } else {
                6
            }
        );
        assert_eq!(crc, None, "non-CRC entries must not accumulate CRC work");

        let valid = regular_header_record(3, algorithm, &expected);
        super::super::require_regular_content(&valid, 3, &computed).unwrap();

        let mismatch = regular_header_record(3, algorithm, &"0".repeat(expected.len()));
        let error = super::super::require_regular_content(&mismatch, 3, &computed).unwrap_err();
        assert!(
            error.to_string().contains("file digest mismatch"),
            "{error}"
        );
    }

    let computed = ComputedRegularContent {
        sha256: SHA256_ABC.to_string(),
        declared: ComputedFileDigest {
            algorithm: RpmFileDigestAlgorithm::Sha2_256,
            hex: SHA256_ABC.to_string(),
        },
        bytes_hashed: 3,
    };
    let sha3_header = regular_header_record(3, RpmFileDigestAlgorithm::Sha3_256, SHA256_ABC);
    let error = super::super::require_regular_content(&sha3_header, 3, &computed).unwrap_err();
    assert!(
        error.to_string().contains("digest algorithm")
            && !error.to_string().contains("digest mismatch"),
        "equal-length SHA2-256/SHA3-256 identities must fail by typed algorithm: {error}"
    );
}

#[test]
fn bounded_spool_copy_rejects_short_input_and_sink_failure() {
    let error = copy_exact_payload(
        &mut io::Cursor::new(b"ab"),
        &mut Vec::new(),
        3,
        RpmFileDigestAlgorithm::Sha2_256,
        false,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("read RPM payload bytes"),
        "{error}"
    );

    let error = copy_exact_payload(
        &mut io::Cursor::new(b"abc"),
        &mut StopWriter,
        3,
        RpmFileDigestAlgorithm::Sha2_256,
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("injected bounded sink stop"));
}

#[test]
fn payload_reader_rejects_extra_bytes_after_stripped_trailer() {
    let mut bytes = b"07070Xffffffff".to_vec();
    bytes.extend_from_slice(&[0, 0]);
    bytes.push(b'x');
    let records = [];
    let spool = PayloadSpool::new(0).unwrap();
    let error = RpmPayloadReader::new(
        Box::new(io::Cursor::new(bytes)),
        &records,
        &spool,
        CCS_BUDGET.archive_decode_bounds().unwrap(),
    )
    .read_all()
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("nonzero bytes after the CPIO trailer")
    );
}

#[test]
fn regular_member_rejects_crc_mismatch_and_spool_create_collision() {
    let digest = crate::hash::sha256(b"abc");
    let records = [regular_header_record(
        3,
        RpmFileDigestAlgorithm::Sha2_256,
        &digest,
    )];
    let spool = PayloadSpool::new(3).unwrap();
    let mut reader = RpmPayloadReader::new(
        Box::new(io::Cursor::new(b"abc\0")),
        &records,
        &spool,
        CCS_BUDGET.archive_decode_bounds().unwrap(),
    );
    let error = reader.read_member_content(0, 3, Some(0)).unwrap_err();
    assert!(error.to_string().contains("CPIO CRC mismatch"), "{error}");

    let spool = PayloadSpool::new(3).unwrap();
    let collision = spool.indexed_path(0);
    std::fs::write(&collision, b"preexisting authority").unwrap();
    let mut reader = RpmPayloadReader::new(
        Box::new(io::Cursor::new(b"abc")),
        &records,
        &spool,
        CCS_BUDGET.archive_decode_bounds().unwrap(),
    );
    let error = reader.read_member_content(0, 3, None).unwrap_err();
    let crate::Error::Io(error) = error else {
        panic!("expected typed I/O collision failure");
    };
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(collision).unwrap(), b"preexisting authority");
}

#[test]
fn repeated_member_index_fails_before_a_second_spool_open() {
    let digest = crate::hash::sha256(b"abc");
    let records = [regular_header_record(
        3,
        RpmFileDigestAlgorithm::Sha2_256,
        &digest,
    )];
    let spool = PayloadSpool::new(3).unwrap();
    let mut reader = RpmPayloadReader::new(
        Box::new(io::Cursor::new(b"abc\0")),
        &records,
        &spool,
        CCS_BUDGET.archive_decode_bounds().unwrap(),
    );
    reader.read_member_content(0, 3, None).unwrap();

    let error = reader.read_member_content(0, 3, None).unwrap_err();

    assert!(error.to_string().contains("repeats header path"), "{error}");
    assert_eq!(std::fs::read(spool.indexed_path(0)).unwrap(), b"abc");
}

#[test]
fn rpm_cpio_root_spellings_map_only_to_the_header_root_anchor() {
    for name in ["", "./", "/", ".//"] {
        assert_eq!(
            rpm_cpio_header_path(name).unwrap(),
            "/",
            "name was {name:?}"
        );
    }
    for name in [".", "//", "./.", "../", "./../root"] {
        assert!(
            rpm_cpio_header_path(name).is_err(),
            "{name:?} must not alias the RPM root anchor"
        );
    }
}

#[test]
fn rpm_cpio_paths_follow_upstream_prefix_and_exact_match_grammar() {
    for name in [
        "usr/share/fixture",
        "./usr/share/fixture",
        "/usr/share/fixture",
    ] {
        assert_eq!(
            rpm_cpio_header_path(name).unwrap(),
            "/usr/share/fixture",
            "name was {name:?}"
        );
    }
    for name in ["usr//share/fixture", "usr/./share/fixture", "usr/../etc"] {
        assert!(
            rpm_cpio_header_path(name).is_err(),
            "{name:?} must not normalize into different header authority"
        );
    }
}

#[test]
fn lzma_alone_payload_decoder_round_trips_bytes() {
    let input = b"RPM lzma-alone payload";
    let options = liblzma::stream::LzmaOptions::new_preset(6).unwrap();
    let stream = liblzma::stream::Stream::new_lzma_encoder(&options).unwrap();
    let mut encoder = liblzma::read::XzEncoder::new_stream(input.as_slice(), stream);
    let mut encoded = Vec::new();
    encoder.read_to_end(&mut encoded).unwrap();

    let mut decoded = Vec::new();
    payload_decoder(
        Box::new(io::Cursor::new(encoded)),
        RpmPayloadCompressor::Lzma,
    )
    .unwrap()
    .read_to_end(&mut decoded)
    .unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn symlink_payload_comparison_rejects_bytes_that_differ_from_filelinktos() {
    let error = compare_exact_payload(
        &mut io::Cursor::new(b"../../expecteD-target"),
        b"../../expected-target",
        "/usr/lib/.build-id/fixture",
        false,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("payload target differs from FILELINKTOS")
    );
}

#[test]
fn symlink_size_and_other_nonregular_payload_content_remain_rejected() {
    let target = b"../../expected-target";
    let symlink = [header_record(
        libc::S_IFLNK | 0o777,
        target.len() as u64,
        Some(std::str::from_utf8(target).unwrap()),
    )];
    let spool = PayloadSpool::new(0).unwrap();
    let mut reader = RpmPayloadReader::new(
        Box::new(io::Cursor::new(target)),
        &symlink,
        &spool,
        CCS_BUDGET.archive_decode_bounds().unwrap(),
    );
    let error = reader
        .read_member_content(0, target.len() as u64 - 1, None)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("size disagrees with FILELINKTOS"),
        "{error}"
    );

    let directory = [header_record(libc::S_IFDIR | 0o755, 0, None)];
    let mut reader = RpmPayloadReader::new(
        Box::new(io::Cursor::new(b"x")),
        &directory,
        &spool,
        CCS_BUDGET.archive_decode_bounds().unwrap(),
    );
    let error = reader.read_member_content(0, 1, None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("non-regular RPM node /usr/lib/.build-id/fixture carries 1 payload bytes"),
        "{error}"
    );
}
