// conary-core/src/repository/parsers/fedora/filelists/tests.rs

use super::*;
use crate::db::models::Repository;
use crate::repository::dependency_model::{CapabilityProvenance, SourcePackageFormat};
use crate::repository::parsers::fedora::tests::parser;

const CONSUMER_PKGID: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PROVIDER_PKGID: &str = "2222222222222222222222222222222222222222222222222222222222222222";

/// A primary.xml carrying exactly what createrepo_c's selection rule keeps:
/// `crypto-policies` requires a `/usr/lib` path, and `systemd` publishes only
/// its `bin/` paths here.
fn primary_document() -> String {
    format!(
        r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>crypto-policies</name>
    <arch>noarch</arch>
    <version epoch="0" ver="20260101" rel="1.fc44"/>
    <checksum type="sha256">{CONSUMER_PKGID}</checksum>
    <size package="1024"/>
    <location href="Packages/c/crypto-policies-20260101-1.fc44.noarch.rpm"/>
    <format>
      <rpm:requires>
        <rpm:entry name="/usr/lib/systemd/systemd-update-helper"/>
      </rpm:requires>
    </format>
  </package>
  <package type="rpm">
    <name>systemd</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="258" rel="4.fc44"/>
    <checksum type="sha256">{PROVIDER_PKGID}</checksum>
    <size package="2048"/>
    <location href="Packages/s/systemd-258-4.fc44.x86_64.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="systemd" flags="EQ" epoch="0" ver="258" rel="4.fc44"/>
      </rpm:provides>
      <file>/usr/bin/systemctl</file>
    </format>
  </package>
</metadata>
"#
    )
}

/// The same two packages as the generator writes them with the primary filter
/// off: every owned path, including the `/usr/lib` entry primary drops and the
/// `/usr/bin` entry primary keeps.
fn filelists_document() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<filelists xmlns="http://linux.duke.edu/metadata/filelists" packages="2">
<package pkgid="{CONSUMER_PKGID}" name="crypto-policies" arch="noarch">
  <version epoch="0" ver="20260101" rel="1.fc44"/>
  <file type="dir">/usr/share/crypto-policies</file>
  <file>/usr/share/crypto-policies/DEFAULT/opensslcnf.txt</file>
</package>
<package pkgid="{PROVIDER_PKGID}" name="systemd" arch="x86_64">
  <version epoch="0" ver="258" rel="4.fc44"/>
  <file>/usr/bin/systemctl</file>
  <file>/usr/lib/systemd/systemd-update-helper</file>
  <file type="dir">/usr/lib/systemd/system</file>
  <file type="ghost">/usr/lib/systemd/system/default.target</file>
</package>
</filelists>
"#
    )
}

fn parse_primary() -> Vec<PackageMetadata> {
    parser()
        .parse_primary_xml(&primary_document(), "https://repo.test/fedora")
        .unwrap()
}

fn file_capabilities(package: &PackageMetadata) -> Vec<&str> {
    package
        .provides
        .iter()
        .filter(|provide| provide.kind == RepositoryCapabilityKind::File)
        .map(|provide| provide.name.as_str())
        .collect()
}

fn ingest(packages: &mut [PackageMetadata], document: &str) -> Result<FilelistsIngest> {
    ingest_filelists(
        packages,
        document.as_bytes(),
        "https://repo.test/filelists.xml",
    )
}

#[test]
fn filelists_publishes_every_owned_path_the_primary_filter_drops() {
    let mut packages = parse_primary();
    assert_eq!(file_capabilities(&packages[1]), vec!["/usr/bin/systemctl"]);

    let ingest = ingest(&mut packages, &filelists_document()).unwrap();

    assert_eq!(ingest.records, 2);
    assert_eq!(
        file_capabilities(&packages[0]),
        vec![
            "/usr/share/crypto-policies",
            "/usr/share/crypto-policies/DEFAULT/opensslcnf.txt",
        ]
    );
    // Directory and %ghost records are package-owned paths, exactly as they
    // are in primary.xml, so the generator's `type` attribute does not gate
    // the projection.
    assert_eq!(
        file_capabilities(&packages[1]),
        vec![
            "/usr/bin/systemctl",
            "/usr/lib/systemd/systemd-update-helper",
            "/usr/lib/systemd/system",
            "/usr/lib/systemd/system/default.target",
        ]
    );
}

#[test]
fn filelists_paths_carry_the_same_provenance_primary_paths_carry() {
    let mut packages = parse_primary();
    ingest(&mut packages, &filelists_document()).unwrap();

    let filelists_only = packages[1]
        .provides
        .iter()
        .find(|provide| provide.name == "/usr/lib/systemd/systemd-update-helper")
        .expect("filelists-only path");
    let primary_selected = packages[1]
        .provides
        .iter()
        .find(|provide| provide.name == "/usr/bin/systemctl")
        .expect("primary-selected path");

    for provide in [filelists_only, primary_selected] {
        assert_eq!(provide.kind, RepositoryCapabilityKind::File);
        assert!(provide.version.is_none());
        assert!(provide.version_relation.is_none());
        assert_eq!(
            provide.provenance,
            CapabilityProvenance::SourceDerivedFile {
                format: SourcePackageFormat::Rpm,
                source_path: provide.name.clone(),
            }
        );
    }
}

#[test]
fn a_path_both_documents_publish_becomes_exactly_one_capability() {
    let mut packages = parse_primary();
    let ingest = ingest(&mut packages, &filelists_document()).unwrap();

    assert_eq!(ingest.files_already_known, 1);
    assert_eq!(
        packages[1]
            .provides
            .iter()
            .filter(|provide| provide.name == "/usr/bin/systemctl")
            .count(),
        1
    );
    assert_eq!(ingest.files_added, 5);
}

#[test]
fn repeated_filelists_ingest_cannot_duplicate_a_capability() {
    let mut packages = parse_primary();
    ingest(&mut packages, &filelists_document()).unwrap();
    let after_first = file_capabilities(&packages[1]).len();

    let second = ingest(&mut packages, &filelists_document()).unwrap();

    assert_eq!(second.files_added, 0);
    assert_eq!(file_capabilities(&packages[1]).len(), after_first);
}

/// The exact issue #285 failure: a `/usr/lib` file dependency has no
/// repository provider from the filtered primary set, and resolves once the
/// signed filelists document is ingested.
#[test]
fn a_file_dependency_outside_the_primary_filter_resolves_after_filelists_ingest() {
    fn solve(packages: Vec<PackageMetadata>) -> crate::resolver::sat::SatResolution {
        let (_temp, conn) = crate::db::testing::create_test_db();
        let mut repository = Repository::new(
            "fedora-44".to_string(),
            "https://repo.test/fedora".to_string(),
        );
        repository.source_profile = Some("fedora-44".to_string());
        let repository_id = repository.insert(&conn).unwrap();

        let rows = packages
            .into_iter()
            .map(|package| {
                crate::repository::sync::synced_package_row(
                    repository_id,
                    "fedora-44",
                    "https://repo.test/fedora",
                    None,
                    package,
                )
            })
            .collect::<Vec<_>>();
        crate::repository::sync::persist_synced_package_rows(&conn, repository_id, rows).unwrap();

        crate::resolver::sat::solve_install_with_policy(
            &conn,
            &[(
                "crypto-policies".to_string(),
                crate::version::VersionConstraint::Any,
            )],
            &crate::repository::resolution_policy::ResolutionPolicy::new().with_mixing(
                crate::repository::resolution_policy::DependencyMixingPolicy::Permissive,
            ),
        )
        .unwrap()
    }

    let primary_only = solve(parse_primary());
    assert!(
        primary_only.conflict_message.is_some(),
        "the filtered primary file set alone must not resolve a /usr/lib path dependency"
    );

    let mut packages = parse_primary();
    ingest(&mut packages, &filelists_document()).unwrap();
    let with_filelists = solve(packages);

    assert!(
        with_filelists.conflict_message.is_none(),
        "{with_filelists:?}"
    );
    let installed = with_filelists
        .install_order
        .iter()
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    assert!(installed.contains(&"crypto-policies"));
    assert!(installed.contains(&"systemd"));
}

// ---------------------------------------------------------------------------
// Join between the two signed documents
// ---------------------------------------------------------------------------

#[test]
fn filelists_record_for_an_unpublished_package_is_refused() {
    let mut packages = parse_primary();
    let document = filelists_document().replace(
        &format!(r#"pkgid="{PROVIDER_PKGID}""#),
        &format!(r#"pkgid="{}""#, "3".repeat(64)),
    );

    let error = ingest(&mut packages, &document).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("which the signed primary.xml does not publish"),
        "{error}"
    );
}

#[test]
fn a_package_with_no_filelists_record_is_refused() {
    let mut packages = parse_primary();
    let document = format!(
        r#"<filelists packages="1">
<package pkgid="{PROVIDER_PKGID}" name="systemd" arch="x86_64">
  <version epoch="0" ver="258" rel="4.fc44"/>
  <file>/usr/bin/systemctl</file>
</package>
</filelists>"#
    );

    let error = ingest(&mut packages, &document).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("publishes no file record for package crypto-policies"),
        "{error}"
    );
}

#[test]
fn filelists_that_disagrees_with_primary_identity_is_refused() {
    for (field, from, to) in [
        ("name", r#"name="systemd""#, r#"name="systemd-udev""#),
        ("architecture", r#"arch="x86_64""#, r#"arch="aarch64""#),
        (
            "version",
            r#"ver="258" rel="4.fc44""#,
            r#"ver="259" rel="4.fc44""#,
        ),
    ] {
        let mut packages = parse_primary();
        let document = filelists_document().replacen(from, to, 1);

        let error = ingest(&mut packages, &document).unwrap_err();

        assert!(
            error.to_string().contains(&format!(
                "disagree on pkgid {PROVIDER_PKGID}: filelists {field}"
            )),
            "{field}: {error}"
        );
    }
}

#[test]
fn filelists_that_miscounts_its_own_records_is_refused() {
    let mut packages = parse_primary();
    let document = filelists_document().replace(r#"packages="2""#, r#"packages="3""#);

    let error = ingest(&mut packages, &document).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("declares 3 packages but carries 2 package records"),
        "{error}"
    );
}

#[test]
fn filelists_package_without_a_version_is_refused() {
    let mut packages = parse_primary();
    let document =
        filelists_document().replace(r#"<version epoch="0" ver="258" rel="4.fc44"/>"#, "");

    let error = ingest(&mut packages, &document).unwrap_err();

    assert!(error.to_string().contains("carries no version"), "{error}");
}

// ---------------------------------------------------------------------------
// The `<file>` grammar is one owner shared with primary.xml
// ---------------------------------------------------------------------------

/// Everything primary.xml's file grammar admits, filelists.xml admits, and
/// everything one refuses the other refuses. The documents are written by one
/// generator function, so one parser owner has to admit exactly one language.
#[test]
fn both_documents_admit_exactly_the_same_file_record_language() {
    let admitted = [
        ("<file>/usr/bin/bash</file>", "/usr/bin/bash"),
        ("<file type=\"ghost\">/usr/bin/sh</file>", "/usr/bin/sh"),
        ("<file type=\"dir\">/usr/share/doc</file>", "/usr/share/doc"),
        (
            "<file>/opt/provider&amp;selected</file>",
            "/opt/provider&selected",
        ),
        (
            "<file>/usr<![CDATA[/local/bin/cdata]]></file>",
            "/usr/local/bin/cdata",
        ),
        (
            "<file>/usr/bin/trailing-space </file>",
            "/usr/bin/trailing-space ",
        ),
    ];

    for (record, expected) in admitted {
        let from_primary = primary_file_paths(record);
        let from_filelists = filelists_file_paths(record).unwrap();
        assert_eq!(from_primary, vec![expected], "primary: {record}");
        assert_eq!(from_filelists, vec![expected], "filelists: {record}");
    }

    let refused = [
        (
            "<file>/usr<unexpected/>/bin/bash</file>",
            "cannot contain nested elements",
        ),
        (
            "<file>/usr<file>/bin/bash</file></file>",
            "cannot contain nested elements",
        ),
        ("<file/>", "must contain an absolute path"),
        ("<file></file>", "must contain an absolute path"),
        ("<file>usr/bin/bash</file>", "must contain an absolute path"),
    ];

    for (record, expected) in refused {
        let primary_error = parser()
            .parse_primary_xml(&primary_with_file_records(record), "https://repo.test")
            .unwrap_err()
            .to_string();
        let filelists_error = filelists_file_paths(record).unwrap_err().to_string();
        assert!(primary_error.contains(expected), "primary: {primary_error}");
        assert!(
            filelists_error.contains(expected),
            "filelists: {filelists_error}"
        );
    }
}

fn primary_with_file_records(records: &str) -> String {
    format!(
        r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>systemd</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="258" rel="4.fc44"/>
    <checksum type="sha256">{PROVIDER_PKGID}</checksum>
    <size package="2048"/>
    <location href="Packages/s/systemd-258-4.fc44.x86_64.rpm"/>
    <format>
      {records}
    </format>
  </package>
</metadata>
"#
    )
}

fn primary_file_paths(records: &str) -> Vec<String> {
    let packages = parser()
        .parse_primary_xml(&primary_with_file_records(records), "https://repo.test")
        .unwrap();
    file_capabilities(&packages[0])
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

fn filelists_file_paths(records: &str) -> Result<Vec<String>> {
    let mut packages = parser()
        .parse_primary_xml(&primary_with_file_records(""), "https://repo.test")
        .unwrap();
    let document = format!(
        r#"<filelists packages="1">
<package pkgid="{PROVIDER_PKGID}" name="systemd" arch="x86_64">
  <version epoch="0" ver="258" rel="4.fc44"/>
  {records}
</package>
</filelists>"#
    );
    ingest(&mut packages, &document)?;
    Ok(file_capabilities(&packages[0])
        .into_iter()
        .map(ToString::to_string)
        .collect())
}

// ---------------------------------------------------------------------------
// Decoding one verified document
// ---------------------------------------------------------------------------

fn gzip(content: &str) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(content.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn a_compressed_filelists_document_is_ingested_without_materializing_it() {
    let mut packages = parse_primary();
    let document = filelists_document();
    let compressed = gzip(&document);

    let ingest = ingest_verified_filelists(
        &mut packages,
        &compressed,
        document.len() as u64,
        "https://repo.test/filelists.xml.gz",
    )
    .unwrap();

    assert_eq!(ingest.records, 2);
    assert!(
        file_capabilities(&packages[1]).contains(&"/usr/lib/systemd/systemd-update-helper"),
        "{:?}",
        file_capabilities(&packages[1])
    );
}

#[test]
fn a_document_longer_than_the_signed_decompressed_length_is_refused() {
    let mut packages = parse_primary();
    let document = filelists_document();
    let compressed = gzip(&document);

    let error = ingest_verified_filelists(
        &mut packages,
        &compressed,
        document.len() as u64 - 1,
        "https://repo.test/filelists.xml.gz",
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("decodes to more than the"),
        "{error}"
    );
}

#[test]
fn a_document_shorter_than_the_signed_decompressed_length_is_refused() {
    let mut packages = parse_primary();
    let document = filelists_document();
    let compressed = gzip(&document);

    let error = ingest_verified_filelists(
        &mut packages,
        &compressed,
        document.len() as u64 + 1,
        "https://repo.test/filelists.xml.gz",
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("decompressed bytes but https://repo.test/filelists.xml.gz decoded to"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// Failing closed when the repository publishes no filelists record
// ---------------------------------------------------------------------------

#[test]
fn a_path_requirement_without_a_filelists_record_refuses_the_repository() {
    let packages = parse_primary();

    let error = require_no_filelists_dependent_requirements(&packages, "https://repo.test/fedora")
        .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("https://repo.test/fedora"), "{message}");
    assert!(message.contains("crypto-policies"), "{message}");
    assert!(
        message.contains("/usr/lib/systemd/systemd-update-helper"),
        "{message}"
    );
    assert!(message.contains("filelists"), "{message}");
}

#[test]
fn a_path_requirement_the_primary_file_set_provides_is_not_filelists_dependent() {
    let mut packages = parse_primary();
    packages[0].requirements = requirement_groups("/usr/bin/systemctl");

    require_no_filelists_dependent_requirements(&packages, "https://repo.test/fedora").unwrap();
}

#[test]
fn a_capability_requirement_is_never_filelists_dependent() {
    let mut packages = parse_primary();
    packages[0].requirements = requirement_groups("systemd");

    require_no_filelists_dependent_requirements(&packages, "https://repo.test/fedora").unwrap();
}

#[test]
fn an_alternative_beside_an_unprovided_path_is_not_filelists_dependent() {
    let mut packages = parse_primary();
    packages[0].requirements = requirement_groups("(/usr/lib/systemd/systemd or systemd)");

    require_no_filelists_dependent_requirements(&packages, "https://repo.test/fedora").unwrap();
}

#[test]
fn a_conditional_path_requirement_is_not_filelists_dependent() {
    let mut packages = parse_primary();
    packages[0].requirements =
        requirement_groups("(/usr/lib/systemd/systemd if /usr/lib/systemd/systemd-update-helper)");

    require_no_filelists_dependent_requirements(&packages, "https://repo.test/fedora").unwrap();
}

/// A flattened alternatives view cannot tell a conjunction from a
/// disjunction, so a mandatory path conjunct beside a satisfiable capability
/// looks resolvable while it is not. The decision reads the typed expression.
#[test]
fn a_mandatory_path_conjunct_refuses_the_repository() {
    let mut packages = parse_primary();
    packages[0].requirements = requirement_groups("(systemd and /usr/lib/systemd/systemd)");

    let error = require_no_filelists_dependent_requirements(&packages, "https://repo.test/fedora")
        .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("/usr/lib/systemd/systemd"), "{message}");
    assert!(message.contains("crypto-policies"), "{message}");
    assert!(message.contains("filelists"), "{message}");
}

#[test]
fn every_unprovidable_path_of_a_refused_conjunction_is_named() {
    let mut packages = parse_primary();
    packages[0].requirements =
        requirement_groups("(/usr/lib/systemd/systemd and /usr/share/factory/etc)");

    let error = require_no_filelists_dependent_requirements(&packages, "https://repo.test/fedora")
        .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("/usr/lib/systemd/systemd"), "{message}");
    assert!(message.contains("/usr/share/factory/etc"), "{message}");
}

#[test]
fn a_conjunction_whose_path_the_primary_file_set_provides_is_admitted() {
    let mut packages = parse_primary();
    packages[0].requirements = requirement_groups("(systemd and /usr/bin/systemctl)");

    require_no_filelists_dependent_requirements(&packages, "https://repo.test/fedora").unwrap();
}

#[test]
fn a_path_conflict_is_not_filelists_dependent() {
    let mut packages = parse_primary();
    packages[0].requirements = vec![
        crate::repository::package_relation::parse_native_relation(
            RepositoryRequirementKind::Conflict,
            crate::repository::versioning::VersionScheme::Rpm,
            "/usr/lib/systemd/systemd",
        )
        .unwrap(),
    ];

    require_no_filelists_dependent_requirements(&packages, "https://repo.test/fedora").unwrap();
}

fn requirement_groups(
    native_text: &str,
) -> Vec<crate::repository::dependency_model::RepositoryRequirementGroup> {
    vec![
        crate::repository::requirement::parse_native_requirement(
            RepositoryRequirementKind::Depends,
            crate::repository::versioning::VersionScheme::Rpm,
            native_text,
        )
        .unwrap(),
    ]
}
