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
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryRequirementExpression, RepositoryRequirementKind,
};
use crate::repository::parsers::PackageMetadata;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read};

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

/// A reader that admits exactly the signed decompressed length.
///
/// The ceiling comes from the authenticated `repomd.xml` record, so a stream
/// that decodes to more than the repository signed for is refused before the
/// extra bytes are parsed, and a stream that stops early is caught by the
/// final length comparison.
struct AuthenticatedLengthReader<R> {
    inner: R,
    read: u64,
    ceiling: u64,
    label: &'static str,
}

impl<R: Read> AuthenticatedLengthReader<R> {
    fn new(inner: R, ceiling: u64, label: &'static str) -> Self {
        Self {
            inner,
            read: 0,
            ceiling,
            label,
        }
    }

    const fn read_bytes(&self) -> u64 {
        self.read
    }
}

impl<R: Read> Read for AuthenticatedLengthReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.read = self.read.saturating_add(read as u64);
        if self.read > self.ceiling {
            return Err(std::io::Error::other(format!(
                "RPM {} metadata decodes to more than the {} bytes the signed repomd.xml declares",
                self.label, self.ceiling
            )));
        }
        Ok(read)
    }
}

/// Decode and ingest one verified `filelists.xml` payload.
///
/// `compressed_bytes` must already have been verified against its signed
/// `repomd.xml` record; this function owns only decoding and projection.
pub(super) fn ingest_verified_filelists(
    packages: &mut [PackageMetadata],
    compressed_bytes: &[u8],
    open_size: u64,
    source: &str,
) -> Result<FilelistsIngest> {
    let format = crate::compression::CompressionFormat::from_magic_bytes(compressed_bytes);
    let decoder =
        crate::compression::create_decoder(compressed_bytes, format).map_err(|error| {
            Error::ParseError(format!(
                "failed to decode RPM filelists metadata {source}: {error}"
            ))
        })?;
    let mut reader = std::io::BufReader::with_capacity(
        256 * 1024,
        AuthenticatedLengthReader::new(decoder, open_size, DOCUMENT.label()),
    );

    let ingest = ingest_filelists(packages, &mut reader, source)?;

    let decoded = reader.get_ref().read_bytes();
    if decoded != open_size {
        return Err(Error::GpgVerificationFailed(format!(
            "signed repomd.xml authenticates filelists metadata as {open_size} decompressed bytes \
             but {source} decoded to {decoded} bytes"
        )));
    }
    Ok(ingest)
}

/// Fold every `<package>` record of a filelists document into its package.
pub(super) fn ingest_filelists<R: BufRead>(
    packages: &mut [PackageMetadata],
    document: R,
    source: &str,
) -> Result<FilelistsIngest> {
    let mut by_pkgid: HashMap<&str, Vec<usize>> = HashMap::with_capacity(packages.len());
    for (index, package) in packages.iter().enumerate() {
        by_pkgid
            .entry(package.checksum.as_str())
            .or_default()
            .push(index);
    }
    // `by_pkgid` borrows the corpus, so resolve every record's targets into
    // owned indices before any package is mutated.
    let by_pkgid: HashMap<String, Vec<usize>> = by_pkgid
        .into_iter()
        .map(|(pkgid, indices)| (pkgid.to_string(), indices))
        .collect();

    let mut reader = Reader::from_reader(document);
    reader.config_mut().trim_text_end = true;

    let mut buf = Vec::new();
    let mut record = FileRecordReader::new(DOCUMENT);
    let mut cursor: Option<PackageCursor> = None;
    let mut declared_packages: Option<usize> = None;
    let mut matched = vec![false; packages.len()];
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
                        cursor = Some(PackageCursor::open(
                            element,
                            &reader,
                            &by_pkgid,
                            packages,
                            &mut matched,
                        )?);
                        ingest.records += 1;
                    }
                    "version" => {
                        let cursor = cursor.as_mut().ok_or_else(|| {
                            Error::ParseError(
                                "filelists.xml version record appears outside a package"
                                    .to_string(),
                            )
                        })?;
                        cursor.admit_version(element, &reader, packages)?;
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
                        cursor.admit_file(path, packages, &mut ingest);
                    }
                    "package" => {
                        let cursor = cursor.take().ok_or_else(|| {
                            Error::ParseError(
                                "filelists.xml ended a package that was never started".to_string(),
                            )
                        })?;
                        cursor.close()?;
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
    if let Some(index) = matched.iter().position(|matched| !matched) {
        let package = &packages[index];
        return Err(Error::ParseError(format!(
            "signed filelists.xml {source} publishes no file record for package {} {} (pkgid {}); \
             the two signed documents describe different package sets",
            package.name, package.version, package.checksum
        )));
    }

    Ok(ingest)
}

/// One open `<package>` record and the corpus entries it feeds.
struct PackageCursor {
    pkgid: String,
    targets: Vec<usize>,
    /// Paths this package already provides, so a path both documents carry
    /// becomes exactly one capability row.
    known_paths: HashSet<String>,
    version_seen: bool,
}

impl PackageCursor {
    fn open<R>(
        element: &BytesStart<'_>,
        reader: &Reader<R>,
        by_pkgid: &HashMap<String, Vec<usize>>,
        packages: &[PackageMetadata],
        matched: &mut [bool],
    ) -> Result<Self> {
        let pkgid = required_attribute(element, reader, b"pkgid", "package pkgid")?;
        let name = required_attribute(element, reader, b"name", "package name")?;
        let arch = required_attribute(element, reader, b"arch", "package arch")?;

        let targets = by_pkgid.get(&pkgid).cloned().ok_or_else(|| {
            Error::ParseError(format!(
                "signed filelists.xml publishes file records for pkgid {pkgid} ({name}.{arch}), \
                 which the signed primary.xml does not publish; the two signed documents describe \
                 different package sets"
            ))
        })?;

        let mut known_paths = HashSet::new();
        for &index in &targets {
            let package = &packages[index];
            if package.name != name {
                return Err(Self::disagreement(&pkgid, "name", &name, &package.name));
            }
            let package_arch = package.architecture.as_deref().unwrap_or_default();
            if package_arch != arch {
                return Err(Self::disagreement(
                    &pkgid,
                    "architecture",
                    &arch,
                    package_arch,
                ));
            }
            known_paths.extend(
                package
                    .provides
                    .iter()
                    .filter(|provide| provide.kind == RepositoryCapabilityKind::File)
                    .map(|provide| provide.name.clone()),
            );
            matched[index] = true;
        }

        Ok(Self {
            pkgid,
            targets,
            known_paths,
            version_seen: false,
        })
    }

    fn disagreement(pkgid: &str, field: &str, filelists: &str, primary: &str) -> Error {
        Error::ParseError(format!(
            "signed filelists.xml and primary.xml disagree on pkgid {pkgid}: filelists {field} is \
             '{filelists}' but primary {field} is '{primary}'"
        ))
    }

    fn admit_version<R>(
        &mut self,
        element: &BytesStart<'_>,
        reader: &Reader<R>,
        packages: &[PackageMetadata],
    ) -> Result<()> {
        let epoch = attribute(element, reader, b"epoch", "package epoch")?;
        let ver = required_attribute(element, reader, b"ver", "package version")?;
        let rel = required_attribute(element, reader, b"rel", "package release")?;
        let version = rpm_version_text(epoch.as_deref(), &ver, &rel);
        for &index in &self.targets {
            let package = &packages[index];
            if package.version != version {
                return Err(Self::disagreement(
                    &self.pkgid,
                    "version",
                    &version,
                    &package.version,
                ));
            }
        }
        self.version_seen = true;
        Ok(())
    }

    fn admit_file(
        &mut self,
        path: String,
        packages: &mut [PackageMetadata],
        ingest: &mut FilelistsIngest,
    ) {
        if !self.known_paths.insert(path.clone()) {
            ingest.files_already_known += 1;
            return;
        }
        for &index in &self.targets {
            extend_file_provides(&mut packages[index].provides, &path);
        }
        ingest.files_added += 1;
    }

    fn close(self) -> Result<()> {
        if !self.version_seen {
            return Err(Error::ParseError(format!(
                "filelists.xml package record for pkgid {} carries no version",
                self.pkgid
            )));
        }
        Ok(())
    }
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
pub(super) fn require_no_filelists_dependent_requirements(
    packages: &[PackageMetadata],
    repo_url: &str,
) -> Result<()> {
    let mut provided_paths: HashSet<&str> = HashSet::new();
    for package in packages {
        provided_paths.extend(
            package
                .provides
                .iter()
                .filter(|provide| provide.kind == RepositoryCapabilityKind::File)
                .map(|provide| provide.name.as_str()),
        );
    }

    for package in packages {
        for group in &package.requirements {
            // Only a positive dependency needs a provider. A conflict,
            // obsoletion, or optional relation is satisfied by absence.
            if !matches!(
                group.kind,
                RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
            ) {
                continue;
            }
            if may_hold_without_filelists(&group.expression, &provided_paths) {
                continue;
            }

            let paths = unprovidable_paths(&group.expression, &provided_paths).join("', '");
            return Err(Error::ParseError(format!(
                "repository {repo_url} package {} {} requires path '{paths}', but its signed \
                 repomd.xml publishes no filelists record. primary.xml carries only the \
                 generator-filtered file set, so this path has no repository provider. The \
                 repository must publish filelists.xml.",
                package.name, package.version
            )));
        }
    }

    Ok(())
}

/// Whether a requirement expression can still be satisfied when every absolute
/// path outside primary's filtered file set is known to have no provider.
///
/// The assignment is deliberately optimistic, because this rule must never
/// refuse a repository that could resolve:
///
/// - an atom naming a path the filtered primary set does not provide can never
///   hold, since a file dependency's only repository providers are package file
///   records;
/// - every other atom may hold;
/// - no atom is *guaranteed* to hold, because a provider for it may simply be
///   absent, so any condition may fail and a conditional requirement stays
///   satisfiable through its else branch.
///
/// Repeated occurrences of one atom are evaluated independently. That can only
/// make an expression look more satisfiable, so it can cost a refusal that was
/// warranted but can never invent one that was not.
fn may_hold_without_filelists(
    expression: &RepositoryRequirementExpression,
    provided_paths: &HashSet<&str>,
) -> bool {
    match expression {
        RepositoryRequirementExpression::Atom(clause) => {
            !is_unprovidable_path(&clause.name, provided_paths)
        }
        RepositoryRequirementExpression::And(operands) => operands
            .iter()
            .all(|operand| may_hold_without_filelists(operand, provided_paths)),
        RepositoryRequirementExpression::Or(operands) => operands
            .iter()
            .any(|operand| may_hold_without_filelists(operand, provided_paths)),
        // `(A if B)` requires A when B holds, and takes its else branch
        // otherwise. B may always fail, so an absent else branch leaves the
        // requirement satisfiable.
        RepositoryRequirementExpression::If {
            requirement,
            condition,
            otherwise,
        } => {
            (may_hold_without_filelists(condition, provided_paths)
                && may_hold_without_filelists(requirement, provided_paths))
                || otherwise
                    .as_deref()
                    .is_none_or(|otherwise| may_hold_without_filelists(otherwise, provided_paths))
        }
        // `(A unless B)` requires A when B fails, and takes its else branch
        // when B holds.
        RepositoryRequirementExpression::Unless {
            requirement,
            condition,
            otherwise,
        } => {
            may_hold_without_filelists(requirement, provided_paths)
                || (may_hold_without_filelists(condition, provided_paths)
                    && otherwise.as_deref().is_none_or(|otherwise| {
                        may_hold_without_filelists(otherwise, provided_paths)
                    }))
        }
        // `with` needs one provider satisfying both sides, so both must be
        // satisfiable; `without` constrains which provider may be chosen, and
        // that constraint is optimistically satisfiable.
        RepositoryRequirementExpression::With { left, right } => {
            may_hold_without_filelists(left, provided_paths)
                && may_hold_without_filelists(right, provided_paths)
        }
        RepositoryRequirementExpression::Without { left, .. } => {
            may_hold_without_filelists(left, provided_paths)
        }
    }
}

/// The path atoms of one expression that the filtered primary set cannot
/// provide, in the order the source wrote them.
fn unprovidable_paths<'a>(
    expression: &'a RepositoryRequirementExpression,
    provided_paths: &HashSet<&str>,
) -> Vec<&'a str> {
    expression
        .atoms()
        .into_iter()
        .map(|clause| clause.name.as_str())
        .filter(|name| is_unprovidable_path(name, provided_paths))
        .collect()
}

/// An RPM dependency name that starts with `/` is a dependency on a path
/// rather than on a capability name, and the filtered primary file set is the
/// only file authority a repository without `filelists.xml` publishes.
fn is_unprovidable_path(name: &str, provided_paths: &HashSet<&str>) -> bool {
    name.starts_with('/') && !provided_paths.contains(name)
}

#[cfg(test)]
#[path = "filelists/tests.rs"]
mod tests;
