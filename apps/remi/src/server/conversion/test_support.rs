// apps/remi/src/server/conversion/test_support.rs
//! Shared test helpers for Remi conversion child modules.

use conary_core::db::models::{ConvertedPackage, Repository, RepositoryPackage, RepositoryProvide};
use conary_core::db::schema;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

pub(crate) fn test_transport(hashes: &[String]) -> conary_core::ccs::CcsTransportEnvelopeV1 {
    conary_core::ccs::CcsTransportEnvelopeV1 {
        schema_version: conary_core::ccs::transport::CCS_TRANSPORT_SCHEMA_V1,
        manifest_base64: String::new(),
        signature_json: "{}".to_string(),
        debug_toml_base64: None,
        build_attestation_json: None,
        foreign_conversion_boundary_json: None,
        objects: hashes
            .iter()
            .map(|sha256| conary_core::ccs::CcsTransportObjectV1 {
                sha256: sha256.clone(),
                size: 1,
            })
            .collect(),
    }
}

pub(super) fn create_test_db() -> (NamedTempFile, rusqlite::Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    (temp_file, conn)
}

pub(super) fn eopkg_fixture() -> tempfile::NamedTempFile {
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_path("usr/bin/demo").unwrap();
    header.set_size(5);
    header.set_mode(0o755);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    tar.append(&header, Cursor::new(b"hello")).unwrap();
    let tar = tar.into_inner().unwrap();
    let stream =
        liblzma::stream::Stream::new_easy_encoder(6, liblzma::stream::Check::Crc64).unwrap();
    let mut encoder = liblzma::read::XzEncoder::new_stream(tar.as_slice(), stream);
    let mut compressed = Vec::new();
    encoder.read_to_end(&mut compressed).unwrap();

    let metadata = br#"<PISI><Package><Name>demo</Name><Summary>demo</Summary><History><Update release="2"><Version>1.0</Version></Update></History><Distribution>Solus</Distribution><DistributionRelease>1</DistributionRelease><Architecture>x86_64</Architecture><PackageFormat>1.2</PackageFormat></Package></PISI>"#;
    let files = br#"<Files><File><Path>usr/bin/demo</Path><Type>executable</Type><Size>5</Size><Uid>0</Uid><Gid>0</Gid><Mode>0755</Mode><Hash>aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d</Hash></File></Files>"#;
    let output = tempfile::NamedTempFile::new().unwrap();
    {
        let mut archive = zip::ZipWriter::new(output.reopen().unwrap());
        for (name, bytes) in [
            ("metadata.xml", metadata.as_slice()),
            ("files.xml", files.as_slice()),
            ("install.tar.xz", compressed.as_slice()),
        ] {
            archive
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
    }
    output
}

/// Seed the exact repository source row that makes a converted fixture current,
/// then bind the fixture to the source row's normalized capability projection.
pub(crate) fn seed_repository_conversion_source(
    conn: &rusqlite::Connection,
    converted: &mut ConvertedPackage,
) -> i64 {
    let artifact = converted.repository_artifact().unwrap();
    let source_profile = artifact.source_profile.to_string();
    let package_name = artifact.package_name.to_string();
    let package_version = artifact.package_version.to_string();
    let package_architecture = artifact.package_architecture.to_string();
    let original_checksum = converted.original_checksum.clone();
    let profile =
        conary_core::repository::supported_profiles::profile_by_public_id(&source_profile)
            .unwrap_or_else(|| panic!("test conversion must use exact profile '{source_profile}'"));

    let repository_name = format!("conversion-fixture-{source_profile}");
    let repository_id = match Repository::find_by_name(conn, &repository_name).unwrap() {
        Some(repository) => repository.id.expect("persisted fixture repository"),
        None => {
            let mut repository = Repository::new(
                repository_name,
                format!("https://example.invalid/{source_profile}"),
            );
            repository.source_profile = Some(source_profile.clone());
            repository.insert(conn).unwrap()
        }
    };

    let mut package = RepositoryPackage::new(
        repository_id,
        package_name,
        package_version,
        profile.version_scheme(),
        original_checksum,
        1,
        "https://example.invalid/package".to_string(),
    );
    package.architecture = Some(package_architecture);
    package.source_profile = Some(source_profile);
    let repository_package_id = package.insert(conn).unwrap();

    converted.repository_provides_digest = Some(
        RepositoryProvide::conversion_capabilities_digest(conn, repository_package_id).unwrap(),
    );
    repository_package_id
}

fn production_source_without_comments(path: &Path) -> String {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    let mut stripped = String::new();
    let mut in_block_comment = false;
    let mut pending_test_cfg = false;

    for line in source.lines() {
        let trimmed = line.trim_start();
        if pending_test_cfg {
            if trimmed.starts_with("#[") {
                continue;
            }
            if trimmed.starts_with("mod tests") {
                break;
            }
            pending_test_cfg = false;
        }
        if trimmed.starts_with("#[cfg(test)]") {
            pending_test_cfg = true;
        }

        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if in_block_comment {
                if ch == '*' && chars.peek() == Some(&'/') {
                    let _ = chars.next();
                    in_block_comment = false;
                }
                continue;
            }

            if ch == '/' && chars.peek() == Some(&'/') {
                break;
            }

            if ch == '/' && chars.peek() == Some(&'*') {
                let _ = chars.next();
                in_block_comment = true;
                continue;
            }

            stripped.push(ch);
        }

        stripped.push('\n');
    }

    stripped
}

pub(super) fn production_rust_sources(relative_root: &str) -> Vec<(PathBuf, String)> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.join(relative_root);
    let mut sources = WalkDir::new(&root)
        .into_iter()
        .map(|entry| entry.unwrap_or_else(|error| panic!("walk {}: {error}", root.display())))
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rs")
        })
        .map(|entry| {
            let path = entry.path().to_path_buf();
            let relative = path
                .strip_prefix(manifest_dir)
                .expect("server source stays below the Remi crate")
                .to_path_buf();
            let source = production_source_without_comments(&path);
            (relative, source)
        })
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}
