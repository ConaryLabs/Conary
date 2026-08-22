// conary-core/src/repository/parsers/fedora/repomd.rs

//! Exact `repomd.xml` record admission for the RPM metadata documents Conary
//! reads.
//!
//! `repomd.xml` is the signed root of RPM repository metadata: the size and
//! SHA-256 it records authenticate every other document, so a document Conary
//! reads must have a record here and must match that record exactly.
//! Pinned `createrepo_c` `5cf41fe5d703901d78078ed18c67ab667e446c1a` writes one
//! `<data type="...">` record per generated document
//! (<https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump_repomd.c>),
//! including the decompressed `<open-size>`/`<open-checksum>` pair whenever the
//! served document is compressed.
//!
//! Conary reads exactly two of those records:
//!
//! - `primary`: the package corpus, its relations, and the generator-filtered
//!   file set.
//! - `filelists`: complete package file ownership, which is the only authority
//!   for a file dependency on a path the primary filter drops.
//!
//! Record types Conary does not read are ignored. A repeated record for a type
//! Conary does read is fatal: the repository would be naming two different
//! authorities for one document.

use super::{local_tag_name, validate_sha256};
use crate::error::{Error, Result};
use crate::repository::parsers::common;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// One metadata document Conary reads out of `repomd.xml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepoMdDocument {
    Primary,
    Filelists,
}

impl RepoMdDocument {
    /// The exact `type` attribute createrepo_c writes for this document.
    const fn repomd_type(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Filelists => "filelists",
        }
    }

    /// Diagnostic label; identical to the record type on purpose, so an error
    /// names the record an operator can look up in the signed document.
    pub(super) const fn label(self) -> &'static str {
        self.repomd_type()
    }

    fn from_type(value: &str) -> Option<Self> {
        [Self::Primary, Self::Filelists]
            .into_iter()
            .find(|document| document.repomd_type() == value)
    }
}

/// One authenticated `repomd.xml` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepoMdRecord {
    pub(super) document: RepoMdDocument,
    /// Repository-relative location of the served document.
    pub(super) href: String,
    /// SHA-256 of the served (still compressed) bytes.
    pub(super) sha256: String,
    /// Size in bytes of the served (still compressed) document.
    pub(super) size: u64,
    /// Size in bytes of the decompressed document. createrepo_c writes this
    /// only for a compressed document, so an uncompressed document keeps it
    /// absent and `size` is already the decompressed length.
    pub(super) open_size: Option<u64>,
}

impl RepoMdRecord {
    /// Verify served bytes against this signed record before anything reads
    /// them.
    ///
    /// Size is checked first so a truncated or padded document is named as
    /// such instead of surfacing as a digest mismatch.
    #[cfg(test)]
    pub(super) fn verify_served_bytes(&self, bytes: &[u8]) -> Result<()> {
        let label = self.document.label();
        if bytes.len() as u64 != self.size {
            return Err(Error::GpgVerificationFailed(format!(
                "signed repomd.xml authenticates {label} metadata as {} bytes but the repository \
                 served {} bytes",
                self.size,
                bytes.len()
            )));
        }
        let actual = crate::hash::sha256(bytes);
        if actual != self.sha256 {
            return Err(Error::GpgVerificationFailed(format!(
                "RPM {label} metadata identity mismatch: signed repomd.xml SHA256 is {}, \
                 downloaded SHA256 is {}",
                self.sha256, actual
            )));
        }
        Ok(())
    }

    pub(super) fn verify_served_download(
        &self,
        identity: &crate::repository::client::DownloadedFileIdentity,
    ) -> Result<()> {
        let label = self.document.label();
        if identity.size != self.size {
            return Err(Error::GpgVerificationFailed(format!(
                "signed repomd.xml authenticates {label} metadata as {} bytes but the repository served {} bytes",
                self.size, identity.size
            )));
        }
        if identity.sha256 != self.sha256 {
            return Err(Error::GpgVerificationFailed(format!(
                "RPM {label} metadata identity mismatch: signed repomd.xml SHA256 is {}, downloaded SHA256 is {}",
                self.sha256, identity.sha256
            )));
        }
        Ok(())
    }

    /// The exact decompressed length the signed record admits.
    ///
    /// This is the authenticated ceiling a streaming reader applies, so the
    /// bound on decoder memory comes from the signed document rather than from
    /// a locally chosen constant.
    pub(super) fn authenticated_open_size(&self, compressed: bool) -> Result<u64> {
        match (compressed, self.open_size) {
            (false, _) => Ok(self.size),
            (true, Some(open_size)) => Ok(open_size),
            (true, None) => Err(Error::ParseError(format!(
                "repomd.xml {} record serves a compressed document without an <open-size>; the \
                 signed decompressed length is required to read it",
                self.document.label()
            ))),
        }
    }
}

/// Every record Conary reads out of one signed `repomd.xml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepoMdIndex {
    pub(super) primary: RepoMdRecord,
    /// Absent when the repository publishes no `filelists` record at all.
    pub(super) filelists: Option<RepoMdRecord>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecordField {
    Checksum,
    Size,
    OpenSize,
}

#[derive(Default)]
struct RecordBuilder {
    checksum: Option<String>,
    checksum_type: Option<String>,
    size: Option<String>,
    open_size: Option<String>,
    href: Option<String>,
}

impl RecordBuilder {
    fn build(self, document: RepoMdDocument) -> Result<RepoMdRecord> {
        let label = document.label();
        let href = self.href.ok_or_else(|| {
            Error::ParseError(format!(
                "Could not find {label} data location in repomd.xml"
            ))
        })?;
        common::validate_filename(&href).map_err(Error::ParseError)?;

        if self.checksum_type.as_deref() != Some("sha256") {
            return Err(Error::ParseError(format!(
                "RPM {label} metadata requires exact sha256 identity; found {:?}",
                self.checksum_type.as_deref()
            )));
        }
        let sha256 = self.checksum.ok_or_else(|| {
            Error::ParseError(format!("repomd.xml {label} record has no checksum"))
        })?;
        validate_sha256(&sha256, &format!("repomd.xml {label}"))?;

        let size = self
            .size
            .ok_or_else(|| {
                Error::ParseError(format!("repomd.xml {label} record has no compressed size"))
            })?
            .parse::<u64>()
            .map_err(|error| {
                Error::ParseError(format!(
                    "repomd.xml {label} compressed size is invalid: {error}"
                ))
            })?;

        let open_size = self
            .open_size
            .map(|value| {
                value.parse::<u64>().map_err(|error| {
                    Error::ParseError(format!(
                        "repomd.xml {label} decompressed size is invalid: {error}"
                    ))
                })
            })
            .transpose()?;

        Ok(RepoMdRecord {
            document,
            href,
            sha256,
            size,
            open_size,
        })
    }
}

fn attribute_value<R>(
    element: &BytesStart<'_>,
    reader: &Reader<R>,
    key: &[u8],
    field: &str,
) -> Result<Option<String>> {
    for attr in element.attributes() {
        let attr = attr.map_err(|error| {
            Error::ParseError(format!("Failed to parse repomd.xml {field}: {error}"))
        })?;
        if attr.key.as_ref() == key {
            return Ok(Some(
                attr.decoded_and_normalized_value(
                    quick_xml::XmlVersion::Implicit1_0,
                    reader.decoder(),
                )
                .map_err(|error| {
                    Error::ParseError(format!("Failed to decode repomd.xml {field}: {error}"))
                })?
                .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn element_local_name(name: &[u8]) -> String {
    local_tag_name(&String::from_utf8_lossy(name)).to_string()
}

/// Parse one authenticated `repomd.xml` into the records Conary reads.
pub(super) fn parse_repomd(xml_content: &str) -> Result<RepoMdIndex> {
    let mut reader = Reader::from_str(xml_content);
    reader.config_mut().trim_text_end = true;

    let mut buf = Vec::new();
    let mut current: Option<(RepoMdDocument, RecordBuilder)> = None;
    let mut field: Option<RecordField> = None;
    let mut primary: Option<RepoMdRecord> = None;
    let mut filelists: Option<RepoMdRecord> = None;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|error| Error::ParseError(format!("Failed to parse repomd.xml: {error}")))?;
        let (element, closed) = match &event {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            _ => (None, false),
        };

        if let Some(element) = element {
            let local = element_local_name(element.name().as_ref());
            if local == "data" {
                let declared = attribute_value(element, &reader, b"type", "data type")?;
                current = declared
                    .as_deref()
                    .and_then(RepoMdDocument::from_type)
                    .map(|document| (document, RecordBuilder::default()));
                field = None;
                if let Some((document, _)) = &current {
                    let occupied = match document {
                        RepoMdDocument::Primary => primary.is_some(),
                        RepoMdDocument::Filelists => filelists.is_some(),
                    };
                    if occupied {
                        return Err(Error::ParseError(format!(
                            "repomd.xml repeats {} metadata records",
                            document.label()
                        )));
                    }
                }
                if closed && let Some((document, builder)) = current.take() {
                    // A self-closed record declares a document with no served
                    // identity at all; build it so the missing field is named.
                    store_record(
                        &mut primary,
                        &mut filelists,
                        document,
                        builder.build(document)?,
                    )?;
                }
            } else if let Some((_, builder)) = current.as_mut() {
                field = match local.as_str() {
                    "checksum" => {
                        builder.checksum_type =
                            attribute_value(element, &reader, b"type", "checksum type")?;
                        Some(RecordField::Checksum)
                    }
                    "size" => Some(RecordField::Size),
                    "open-size" => Some(RecordField::OpenSize),
                    "location" => {
                        builder.href = attribute_value(element, &reader, b"href", "location")?;
                        None
                    }
                    // `open-checksum` and every other child are not this
                    // record's served identity and must never be read as one.
                    _ => None,
                };
                if closed {
                    field = None;
                }
            }
            buf.clear();
            continue;
        }

        match event {
            Event::Text(text) => {
                if let Some((_, builder)) = current.as_mut() {
                    let value = text
                        .xml_content(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|error| {
                            Error::ParseError(format!("Failed to decode repomd.xml text: {error}"))
                        })?
                        .trim()
                        .to_string();
                    match field {
                        Some(RecordField::Checksum) => builder.checksum = Some(value),
                        Some(RecordField::Size) => builder.size = Some(value),
                        Some(RecordField::OpenSize) => builder.open_size = Some(value),
                        None => {}
                    }
                }
            }
            Event::End(end) => {
                if element_local_name(end.name().as_ref()) == "data"
                    && let Some((document, builder)) = current.take()
                {
                    store_record(
                        &mut primary,
                        &mut filelists,
                        document,
                        builder.build(document)?,
                    )?;
                }
                field = None;
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    let primary = primary.ok_or_else(|| {
        Error::ParseError("Could not find primary data location in repomd.xml".to_string())
    })?;

    Ok(RepoMdIndex { primary, filelists })
}

fn store_record(
    primary: &mut Option<RepoMdRecord>,
    filelists: &mut Option<RepoMdRecord>,
    document: RepoMdDocument,
    record: RepoMdRecord,
) -> Result<()> {
    let slot = match document {
        RepoMdDocument::Primary => primary,
        RepoMdDocument::Filelists => filelists,
    };
    if slot.is_some() {
        return Err(Error::ParseError(format!(
            "repomd.xml repeats {} metadata records",
            document.label()
        )));
    }
    *slot = Some(record);
    Ok(())
}

#[cfg(test)]
#[path = "repomd/tests.rs"]
mod tests;
