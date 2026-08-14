// conary-core/src/repository/parsers/fedora/files.rs

//! The one `<file>` record grammar shared by RPM repository metadata.
//!
//! Pinned `createrepo_c` `5cf41fe5d703901d78078ed18c67ab667e446c1a` emits
//! package-owned paths through a single writer,
//! [`cr_xml_dump_files()`](https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump.c#L175-L225),
//! for both `primary.xml` and `filelists.xml`. The only difference between the
//! two documents is the generator's primary-selection filter, which decides
//! *which* records are written, never *how* one record is written: the element
//! text is the absolute path and an optional `type` attribute names `dir` or
//! `ghost`.
//!
//! Both Conary parsers therefore read a record through this owner, so a path
//! one document admits can never be a path the other rejects.
//!
//! The `type` attribute does not gate provider projection. RPM's own file
//! provides come from the package's complete file list, so a directory or a
//! `%ghost` entry owned by a package is a path that package provides; upstream
//! `libsolv` likewise extends solvables with every `filelists` entry. Conary
//! preserves every signed record and does not reimplement any selection rule.

use super::repomd::RepoMdDocument;
use crate::error::{Error, Result};
use quick_xml::Reader;

/// Accumulates the text of one `<file>` record for one document.
pub(super) struct FileRecordReader {
    document: RepoMdDocument,
    text: Option<String>,
}

impl FileRecordReader {
    pub(super) const fn new(document: RepoMdDocument) -> Self {
        Self {
            document,
            text: None,
        }
    }

    /// Whether a record is currently open and consuming text.
    pub(super) const fn is_open(&self) -> bool {
        self.text.is_some()
    }

    /// A path is element text, never markup: any nested element inside a record
    /// would make the path ambiguous, so it is refused rather than flattened.
    pub(super) fn reject_nested_element(&self) -> Result<()> {
        if self.is_open() {
            return Err(Error::ParseError(format!(
                "RPM {} file record cannot contain nested elements",
                self.document.label()
            )));
        }
        Ok(())
    }

    /// Open a record.
    ///
    /// Trailing XML whitespace inside the element is package identity, not
    /// inter-element formatting, so text trimming is disabled while the record
    /// is open.
    pub(super) fn open<R>(&mut self, reader: &mut Reader<R>) {
        self.text = Some(String::new());
        reader.config_mut().trim_text_end = false;
    }

    /// Append decoded text to the open record. Returns `false` when no record
    /// is open, so the caller can route the text to its own fields.
    pub(super) fn push_text(&mut self, text: &str) -> bool {
        match self.text.as_mut() {
            Some(path) => {
                path.push_str(text);
                true
            }
            None => false,
        }
    }

    /// Close the open record and return its validated absolute path.
    pub(super) fn close<R>(&mut self, reader: &mut Reader<R>) -> Result<String> {
        let path = self.text.take().ok_or_else(|| {
            Error::ParseError(format!(
                "RPM {} file record ended without starting",
                self.document.label()
            ))
        })?;
        reader.config_mut().trim_text_end = true;
        self.validate_path(path)
    }

    /// The refusal for a record that carries no path at all, such as
    /// `<file/>` or `<file></file>`.
    pub(super) fn empty_record_error(&self) -> Error {
        Error::ParseError(format!(
            "RPM {} file record must contain an absolute path",
            self.document.label()
        ))
    }

    fn validate_path(&self, path: String) -> Result<String> {
        if path.is_empty() || !path.starts_with('/') {
            return Err(self.empty_record_error());
        }
        Ok(path)
    }
}
