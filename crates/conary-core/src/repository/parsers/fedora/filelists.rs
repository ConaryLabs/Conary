// conary-core/src/repository/parsers/fedora/filelists.rs

//! Complete RPM package file ownership from the signed `filelists.xml`.
//!
//! `primary.xml` carries only the file records createrepo_c's yum-compatible
//! selection rule keeps
//! ([`cr_xml_dump_files()`](https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump.c#L175-L225)
//! writes a record into primary only when `cr_is_primary()` admits the path).
//! Every other owned path -- most of `/usr/lib`, `/usr/lib64`, and
//! `/usr/share` -- exists only in `filelists.xml`, which the same generator
//! writes from the same package list through the same `<file>` writer with the
//! filter turned off
//! ([`cr_xml_dump_filelists()`](https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump_filelists.c)).
//!
//! A file dependency on a filtered-out path has no repository provider without
//! this document, which is exactly the unsolvable class this module closes.
//! The projection is the same one primary files take: an unversioned typed
//! file capability with `source-derived-file` provenance on the exact package
//! (`parsers/fedora/provides.rs`). No selection rule is reimplemented here.
//!
//! ## Streaming
//!
//! Fedora 44's `filelists.xml` decompresses to roughly 830 MB, so the document
//! is never materialized. It is decoded from the verified compressed bytes
//! through a streaming decompressor bounded by the signed `<open-size>`, and
//! each `<package>` record is folded into its package as it is read. Nothing
//! is persisted until the whole sync snapshot is persisted, so a refusal at
//! any point -- including the final length check -- leaves no partial state.
//!
//! ## Join
//!
//! `pkgid` is the package's SHA-256, which is the same identity `primary.xml`
//! records as its `<checksum type="sha256">`. Both documents are authenticated
//! by the same signed `repomd.xml` revision and are generated from the same
//! package list, so the join is total: every filelists record must name a
//! package primary published, every primary package must be named, and the
//! records must agree on name, architecture, and EVR. A disagreement means the
//! two signed documents describe different repositories, which is refused
//! rather than resolved.

use super::files::FileRecordReader;
use super::provides::extend_file_provides;
use super::repomd::RepoMdDocument;
use super::{local_tag_name, rpm_version_text};
use crate::error::{Error, Result};
#[cfg(test)]
use crate::repository::parsers::PackageMetadata;
use crate::repository::parsers::{
    ChecksumType, RepositorySnapshotSink, SnapshotPackageIdentity, SnapshotPackageJoin,
};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashSet;
use std::io::BufRead;
use std::path::Path;

const DOCUMENT: RepoMdDocument = RepoMdDocument::Filelists;

/// What one filelists ingest added to the package corpus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct FilelistsIngest {
    /// `<package>` records read.
    pub(super) records: usize,
    /// File capabilities added.
    pub(super) files_added: usize,
    /// Records already known from `primary.xml`, so no duplicate row is made.
    pub(super) files_already_known: usize,
}

/// Decode and ingest one verified, file-backed `filelists.xml` payload.
///
/// `path` must already have been verified against its signed
/// `repomd.xml` record; this function owns only decoding and projection.
pub(super) fn ingest_verified_filelists_into<S: RepositorySnapshotSink>(
    sink: &mut S,
    path: &Path,
    open_size: u64,
    source: &str,
) -> Result<FilelistsIngest> {
    let decoder = crate::repository::parsers::common::open_metadata_decoder(
        path,
        &format!("RPM filelists metadata {source}"),
    )?;
    let mut reader = std::io::BufReader::with_capacity(
        256 * 1024,
        crate::repository::parsers::common::AuthenticatedLengthReader::new(
            decoder,
            open_size,
            "RPM filelists metadata",
        ),
    );

    let ingest = ingest_filelists_into(sink, &mut reader, source)?;

    let decoded = reader.get_ref().read_bytes();
    if decoded != open_size {
        return Err(Error::GpgVerificationFailed(format!(
            "signed repomd.xml authenticates filelists metadata as {open_size} decompressed bytes \
             but {source} decoded to {decoded} bytes"
        )));
    }
    Ok(ingest)
}

/// Fold every `<package>` record into its exact catalog package through the
/// authenticated sink join.
fn ingest_filelists_into<R: BufRead, S: RepositorySnapshotSink>(
    sink: &mut S,
    document: R,
    source: &str,
) -> Result<FilelistsIngest> {
    let mut reader = Reader::from_reader(document);
    reader.config_mut().trim_text_end = true;

    let mut buf = Vec::new();
    let mut record = FileRecordReader::new(DOCUMENT);
    let mut cursor: Option<PackageCursor> = None;
    let mut declared_packages: Option<usize> = None;
    let mut ingest = FilelistsIngest::default();

    loop {
        let event = reader.read_event_into(&mut buf).map_err(|error| {
            Error::ParseError(format!("Failed to parse filelists.xml {source}: {error}"))
        })?;

        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let closed = matches!(event, Event::Empty(_));
                record.reject_nested_element()?;
                let local =
                    local_tag_name(&String::from_utf8_lossy(element.name().as_ref())).to_string();
                match local.as_str() {
                    "filelists" => {
                        declared_packages =
                            match attribute(element, &reader, b"packages", "packages count")? {
                                Some(value) => Some(value.parse::<usize>().map_err(|error| {
                                    Error::ParseError(format!(
                                        "filelists.xml declares an invalid package count: {error}"
                                    ))
                                })?),
                                None => None,
                            };
                    }
                    "package" => {
                        if cursor.is_some() {
                            return Err(Error::ParseError(
                                "filelists.xml package record cannot nest another package record"
                                    .to_string(),
                            ));
                        }
                        cursor = Some(PackageCursor::open(element, &reader)?);
                        ingest.records += 1;
                    }
                    "version" => {
                        let cursor = cursor.as_mut().ok_or_else(|| {
                            Error::ParseError(
                                "filelists.xml version record appears outside a package"
                                    .to_string(),
                            )
                        })?;
                        cursor.admit_version(element, &reader)?;
                    }
                    "file" => {
                        if cursor.is_none() {
                            return Err(Error::ParseError(
                                "RPM filelists file record appears outside a package".to_string(),
                            ));
                        }
                        if closed {
                            return Err(record.empty_record_error());
                        }
                        record.open(&mut reader);
                    }
                    _ => {}
                }
            }
            Event::Text(ref text) => {
                if record.is_open() {
                    let decoded = text
                        .xml_content(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|error| {
                            Error::ParseError(format!(
                                "Failed to decode filelists.xml text: {error}"
                            ))
                        })?;
                    let unescaped = quick_xml::escape::unescape(&decoded).map_err(|error| {
                        Error::ParseError(format!("Failed to unescape filelists.xml text: {error}"))
                    })?;
                    record.push_text(&unescaped);
                }
            }
            Event::CData(ref data) => {
                if record.is_open() {
                    let text = data
                        .xml_content(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|error| {
                            Error::ParseError(format!(
                                "Failed to decode filelists.xml CDATA: {error}"
                            ))
                        })?;
                    record.push_text(&text);
                }
            }
            Event::GeneralRef(ref reference) => {
                if record.is_open() {
                    let text = match reference.resolve_char_ref().map_err(|error| {
                        Error::ParseError(format!(
                            "Failed to resolve filelists.xml character reference: {error}"
                        ))
                    })? {
                        Some(character) => character.to_string(),
                        None => {
                            let entity = reference.decode().map_err(|error| {
                                Error::ParseError(format!(
                                    "Failed to decode filelists.xml entity reference: {error}"
                                ))
                            })?;
                            quick_xml::escape::resolve_xml_entity(&entity)
                                .ok_or_else(|| {
                                    Error::ParseError(format!(
                                        "filelists.xml uses undeclared entity '&{entity};'"
                                    ))
                                })?
                                .to_string()
                        }
                    };
                    record.push_text(&text);
                }
            }
            Event::End(ref element) => {
                let local =
                    local_tag_name(&String::from_utf8_lossy(element.name().as_ref())).to_string();
                match local.as_str() {
                    "file" => {
                        let path = record.close(&mut reader)?;
                        let cursor = cursor.as_mut().ok_or_else(|| {
                            Error::ParseError(
                                "RPM filelists file record appears outside a package".to_string(),
                            )
                        })?;
                        cursor.admit_file(path);
                    }
                    "package" => {
                        let cursor = cursor.take().ok_or_else(|| {
                            Error::ParseError(
                                "filelists.xml ended a package that was never started".to_string(),
                            )
                        })?;
                        let update = cursor.close(sink)?;
                        ingest.files_added += update.added;
                        ingest.files_already_known += update.already_known;
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if cursor.is_some() {
        return Err(Error::ParseError(
            "filelists.xml ended inside an unterminated package record".to_string(),
        ));
    }
    if let Some(declared) = declared_packages
        && declared != ingest.records
    {
        return Err(Error::ParseError(format!(
            "filelists.xml declares {declared} packages but carries {} package records",
            ingest.records
        )));
    }
    sink.finish_package_join(SnapshotPackageJoin::RpmFilelists)?;

    Ok(ingest)
}

/// One open `<package>` record. Memory is bounded by this record alone.
struct PackageCursor {
    identity: SnapshotPackageIdentity,
    known_paths: HashSet<String>,
    provides: Vec<crate::repository::dependency_model::RepositoryProvide>,
    version_seen: bool,
}

impl PackageCursor {
    fn open<R>(element: &BytesStart<'_>, reader: &Reader<R>) -> Result<Self> {
        let pkgid = required_attribute(element, reader, b"pkgid", "package pkgid")?;
        let name = required_attribute(element, reader, b"name", "package name")?;
        let arch = required_attribute(element, reader, b"arch", "package arch")?;

        Ok(Self {
            identity: SnapshotPackageIdentity {
                name,
                version: String::new(),
                architecture: Some(arch),
                checksum: pkgid,
                checksum_type: ChecksumType::Sha256,
            },
            known_paths: HashSet::new(),
            provides: Vec::new(),
            version_seen: false,
        })
    }

    fn admit_version<R>(&mut self, element: &BytesStart<'_>, reader: &Reader<R>) -> Result<()> {
        if self.version_seen {
            return Err(Error::ParseError(format!(
                "filelists.xml package record for pkgid {} repeats its version",
                self.identity.checksum
            )));
        }
        let epoch = attribute(element, reader, b"epoch", "package epoch")?;
        let ver = required_attribute(element, reader, b"ver", "package version")?;
        let rel = required_attribute(element, reader, b"rel", "package release")?;
        self.identity.version = rpm_version_text(epoch.as_deref(), &ver, &rel);
        self.version_seen = true;
        Ok(())
    }

    fn admit_file(&mut self, path: String) {
        if self.known_paths.insert(path.clone()) {
            extend_file_provides(&mut self.provides, &path);
        }
    }

    fn close<S: RepositorySnapshotSink>(
        self,
        sink: &mut S,
    ) -> Result<crate::repository::parsers::SnapshotProvideUpdate> {
        if !self.version_seen {
            return Err(Error::ParseError(format!(
                "filelists.xml package record for pkgid {} carries no version",
                self.identity.checksum
            )));
        }
        sink.extend_package_provides(
            SnapshotPackageJoin::RpmFilelists,
            &self.identity,
            self.provides,
        )
    }
}

#[cfg(test)]
pub(super) fn ingest_filelists<R: BufRead>(
    packages: &mut [PackageMetadata],
    document: R,
    source: &str,
) -> Result<FilelistsIngest> {
    use crate::repository::parsers::CollectingRepositorySnapshotSink;

    let mut sink = CollectingRepositorySnapshotSink::create()?;
    for package in packages.iter().cloned() {
        sink.package(package)?;
    }
    let ingest = ingest_filelists_into(&mut sink, document, source)?;
    let (projected, _) = sink.finish();
    packages.clone_from_slice(&projected);
    Ok(ingest)
}

#[cfg(test)]
pub(super) fn ingest_verified_filelists(
    packages: &mut [PackageMetadata],
    compressed_bytes: &[u8],
    open_size: u64,
    source: &str,
) -> Result<FilelistsIngest> {
    use crate::repository::parsers::CollectingRepositorySnapshotSink;

    let format = crate::compression::CompressionFormat::from_magic_bytes(compressed_bytes);
    let decoder =
        crate::compression::create_decoder(compressed_bytes, format).map_err(|error| {
            Error::ParseError(format!(
                "failed to decode RPM filelists metadata {source}: {error}"
            ))
        })?;
    let mut reader = std::io::BufReader::new(
        crate::repository::parsers::common::AuthenticatedLengthReader::new(
            decoder,
            open_size,
            "RPM filelists metadata",
        ),
    );
    let mut sink = CollectingRepositorySnapshotSink::create()?;
    for package in packages.iter().cloned() {
        sink.package(package)?;
    }
    let ingest = ingest_filelists_into(&mut sink, &mut reader, source)?;
    let decoded = reader.get_ref().read_bytes();
    if decoded != open_size {
        return Err(Error::GpgVerificationFailed(format!(
            "signed repomd.xml authenticates filelists metadata as {open_size} decompressed bytes but {source} decoded to {decoded} bytes"
        )));
    }
    let (projected, _) = sink.finish();
    packages.clone_from_slice(&projected);
    Ok(ingest)
}

fn attribute<R>(
    element: &BytesStart<'_>,
    reader: &Reader<R>,
    key: &[u8],
    field: &str,
) -> Result<Option<String>> {
    for attr in element.attributes() {
        let attr = attr.map_err(|error| {
            Error::ParseError(format!("Failed to parse filelists.xml {field}: {error}"))
        })?;
        if attr.key.as_ref() == key {
            return Ok(Some(
                attr.decoded_and_normalized_value(
                    quick_xml::XmlVersion::Implicit1_0,
                    reader.decoder(),
                )
                .map_err(|error| {
                    Error::ParseError(format!("Failed to decode filelists.xml {field}: {error}"))
                })?
                .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn required_attribute<R>(
    element: &BytesStart<'_>,
    reader: &Reader<R>,
    key: &[u8],
    field: &str,
) -> Result<String> {
    attribute(element, reader, key, field)?.ok_or_else(|| {
        Error::ParseError(format!(
            "filelists.xml record is missing its required {field} attribute"
        ))
    })
}

/// Refuse a repository whose packages carry file dependencies its signed
/// `repomd.xml` cannot answer.
///
/// A dependency name that starts with `/` is a file dependency, not a
/// capability name (<https://rpm.org/docs/latest/manual/spec.html#dependencies>;
/// upstream `libsolv` selects file dependencies for its provider index the
/// same way in `pool_addfileprovides_queue()`). Its only repository providers
/// are package file records. When the repository publishes no `filelists`
/// record, the sole file authority available is primary's generator-filtered
/// set, so a path outside that set has no provider and can never be solved.
///
/// That is refused here, naming the repository and the missing record, rather
/// than surfacing later as an anonymous "no candidates were found" conflict.
/// When the repository does publish `filelists`, Conary holds complete file
/// ownership and a missing path is an ordinary unsatisfied dependency.
///
/// The decision is made on the group's typed boolean expression, never on its
/// flattened alternatives: a flattened view cannot tell `(cap or /path)` from
/// `(cap and /path)`, and the conjunction is exactly the unsolvable case.
#[cfg(test)]
pub(super) fn require_no_filelists_dependent_requirements(
    packages: &[PackageMetadata],
    repo_url: &str,
) -> Result<()> {
    let mut sink = crate::repository::parsers::CollectingRepositorySnapshotSink::create()?;
    for package in packages.iter().cloned() {
        sink.package(package)?;
    }
    sink.validate_rpm_primary_file_requirements(repo_url)
}

#[cfg(test)]
#[path = "filelists/tests.rs"]
mod tests;
