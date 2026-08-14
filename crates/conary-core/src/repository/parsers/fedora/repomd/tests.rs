// conary-core/src/repository/parsers/fedora/repomd/tests.rs

use super::*;

const PRIMARY_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PRIMARY_OPEN_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const FILELISTS_SHA: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const FILELISTS_OPEN_SHA: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

/// The record shape createrepo_c writes, including the `<open-checksum>` and
/// `<open-size>` pair it emits for a compressed document.
fn repomd_with(records: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<repomd xmlns="http://linux.duke.edu/metadata/repo">
  <revision>1776864872</revision>
{records}
</repomd>
"#
    )
}

fn primary_record() -> String {
    format!(
        r#"  <data type="primary">
    <checksum type="sha256">{PRIMARY_SHA}</checksum>
    <open-checksum type="sha256">{PRIMARY_OPEN_SHA}</open-checksum>
    <location href="repodata/primary.xml.zst"/>
    <timestamp>1776864872</timestamp>
    <size>15678154</size>
    <open-size>185300395</open-size>
  </data>"#
    )
}

fn filelists_record() -> String {
    format!(
        r#"  <data type="filelists">
    <checksum type="sha256">{FILELISTS_SHA}</checksum>
    <open-checksum type="sha256">{FILELISTS_OPEN_SHA}</open-checksum>
    <location href="repodata/filelists.xml.zst"/>
    <timestamp>1776864872</timestamp>
    <size>46935343</size>
    <open-size>829071915</open-size>
  </data>"#
    )
}

fn other_record() -> String {
    format!(
        r#"  <data type="filelists_db">
    <checksum type="sha256">{FILELISTS_SHA}</checksum>
    <location href="repodata/filelists.sqlite.zst"/>
    <size>62143089</size>
    <open-size>398798848</open-size>
  </data>"#
    )
}

#[test]
fn both_documents_conary_reads_are_admitted_with_their_signed_identity() {
    let index = parse_repomd(&repomd_with(&format!(
        "{}\n{}\n{}",
        primary_record(),
        other_record(),
        filelists_record()
    )))
    .unwrap();

    assert_eq!(
        index.primary,
        RepoMdRecord {
            document: RepoMdDocument::Primary,
            href: "repodata/primary.xml.zst".to_string(),
            sha256: PRIMARY_SHA.to_string(),
            size: 15_678_154,
            open_size: Some(185_300_395),
        }
    );
    assert_eq!(
        index.filelists,
        Some(RepoMdRecord {
            document: RepoMdDocument::Filelists,
            href: "repodata/filelists.xml.zst".to_string(),
            sha256: FILELISTS_SHA.to_string(),
            size: 46_935_343,
            open_size: Some(829_071_915),
        })
    );
}

/// `<open-checksum>` describes the decompressed bytes, never the served ones.
/// Reading it as the record's identity would authenticate the document against
/// the wrong digest.
#[test]
fn the_open_checksum_is_never_read_as_the_served_identity() {
    let index = parse_repomd(&repomd_with(&primary_record())).unwrap();

    assert_eq!(index.primary.sha256, PRIMARY_SHA);
    assert_ne!(index.primary.sha256, PRIMARY_OPEN_SHA);
}

#[test]
fn a_repository_without_a_filelists_record_is_parsed_without_one() {
    let index = parse_repomd(&repomd_with(&format!(
        "{}\n{}",
        primary_record(),
        other_record()
    )))
    .unwrap();

    assert!(index.filelists.is_none());
}

#[test]
fn a_repeated_record_for_a_document_conary_reads_is_refused() {
    for (label, records) in [
        (
            "primary",
            format!("{}\n{}", primary_record(), primary_record()),
        ),
        (
            "filelists",
            format!(
                "{}\n{}\n{}",
                primary_record(),
                filelists_record(),
                filelists_record()
            ),
        ),
    ] {
        let error = parse_repomd(&repomd_with(&records)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("repomd.xml repeats {label} metadata records")),
            "{label}: {error}"
        );
    }
}

#[test]
fn a_filelists_record_holds_the_same_identity_discipline_primary_holds() {
    let cases = [
        (
            filelists_record().replace(
                &format!(r#"<checksum type="sha256">{FILELISTS_SHA}</checksum>"#),
                "",
            ),
            "requires exact sha256 identity",
        ),
        (
            filelists_record().replace("sha256", "sha1"),
            "requires exact sha256 identity",
        ),
        (
            filelists_record().replace(FILELISTS_SHA, "not-a-digest"),
            "SHA256 must be exactly 64 lowercase hexadecimal digits",
        ),
        (
            filelists_record().replace("<size>46935343</size>", ""),
            "repomd.xml filelists record has no compressed size",
        ),
        (
            filelists_record().replace("<size>46935343</size>", "<size>huge</size>"),
            "repomd.xml filelists compressed size is invalid",
        ),
        (
            filelists_record().replace(
                r#"<location href="repodata/filelists.xml.zst"/>"#,
                r#"<location href="../../etc/passwd"/>"#,
            ),
            "path traversal",
        ),
        (
            filelists_record().replace(r#"<location href="repodata/filelists.xml.zst"/>"#, ""),
            "Could not find filelists data location in repomd.xml",
        ),
    ];

    for (record, expected) in cases {
        let error =
            parse_repomd(&repomd_with(&format!("{}\n{record}", primary_record()))).unwrap_err();
        assert!(error.to_string().contains(expected), "{expected}: {error}");
    }
}

#[test]
fn a_missing_primary_record_is_refused() {
    let error = parse_repomd(&repomd_with(&filelists_record())).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("Could not find primary data location in repomd.xml"),
        "{error}"
    );
}

#[test]
fn served_bytes_must_match_the_signed_size_and_digest() {
    let record = RepoMdRecord {
        document: RepoMdDocument::Filelists,
        href: "repodata/filelists.xml".to_string(),
        sha256: crate::hash::sha256(b"filelists"),
        size: 9,
        open_size: None,
    };

    record.verify_served_bytes(b"filelists").unwrap();

    let short = record.verify_served_bytes(b"filelist").unwrap_err();
    assert!(
        short
            .to_string()
            .contains("authenticates filelists metadata as 9 bytes but the repository served 8"),
        "{short}"
    );

    let wrong = record.verify_served_bytes(b"FILELISTS").unwrap_err();
    assert!(
        wrong
            .to_string()
            .contains("RPM filelists metadata identity mismatch"),
        "{wrong}"
    );
}

#[test]
fn the_decompressed_ceiling_comes_from_the_signed_record() {
    let compressed = RepoMdRecord {
        document: RepoMdDocument::Filelists,
        href: "repodata/filelists.xml.zst".to_string(),
        sha256: FILELISTS_SHA.to_string(),
        size: 46_935_343,
        open_size: Some(829_071_915),
    };
    assert_eq!(
        compressed.authenticated_open_size(true).unwrap(),
        829_071_915
    );

    let uncompressed = RepoMdRecord {
        open_size: None,
        ..compressed.clone()
    };
    assert_eq!(
        uncompressed.authenticated_open_size(false).unwrap(),
        46_935_343
    );

    let error = uncompressed.authenticated_open_size(true).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("serves a compressed document without an <open-size>"),
        "{error}"
    );
}
