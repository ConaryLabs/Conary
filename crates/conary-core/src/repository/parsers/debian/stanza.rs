// conary-core/src/repository/parsers/debian/stanza.rs

//! Record-at-a-time Debian Packages stanza grammar.

use std::io::BufRead;

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct DebianPackageEntry {
    pub(super) package: String,
    pub(super) version: String,
    pub(super) architecture: String,
    #[serde(rename = "Multi-Arch", default)]
    pub(super) multi_arch: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(rename = "SHA256")]
    pub(super) sha256: String,
    pub(super) size: String,
    pub(super) filename: String,
    #[serde(default)]
    pub(super) depends: Option<String>,
    #[serde(rename = "Pre-Depends", default)]
    pub(super) pre_depends: Option<String>,
    #[serde(default)]
    pub(super) conflicts: Option<String>,
    #[serde(default)]
    pub(super) breaks: Option<String>,
    #[serde(default)]
    pub(super) replaces: Option<String>,
    #[serde(default)]
    pub(super) provides: Option<String>,
    #[serde(default)]
    pub(super) homepage: Option<String>,
    #[serde(default)]
    pub(super) section: Option<String>,
    #[serde(rename = "Installed-Size", default)]
    pub(super) installed_size: Option<String>,
}

pub(super) fn parse_packages(
    mut reader: impl BufRead,
    mut visitor: impl FnMut(DebianPackageEntry) -> Result<()>,
) -> Result<u64> {
    let mut stanza = String::new();
    let mut line = String::new();
    let mut record = 0_u64;
    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(|error| {
            Error::ParseError(format!("failed to read Debian Packages metadata: {error}"))
        })?;
        if read == 0 {
            if !stanza.is_empty() {
                record = admit_stanza(record, &stanza, &mut visitor)?;
            }
            break;
        }
        if line == "\n" || line == "\r\n" {
            if !stanza.is_empty() {
                record = admit_stanza(record, &stanza, &mut visitor)?;
                stanza.clear();
            }
        } else {
            stanza.push_str(&line);
        }
    }
    Ok(record)
}

fn admit_stanza(
    completed: u64,
    stanza: &str,
    visitor: &mut impl FnMut(DebianPackageEntry) -> Result<()>,
) -> Result<u64> {
    let record = completed
        .checked_add(1)
        .ok_or_else(|| Error::ParseError("Debian Packages record count exceeds u64".to_string()))?;
    let mut entries: Vec<DebianPackageEntry> = rfc822_like::from_str(stanza).map_err(|error| {
        Error::ParseError(format!(
            "Debian Packages stanza {record} is malformed: {error}"
        ))
    })?;
    if entries.len() != 1 {
        return Err(Error::ParseError(format!(
            "Debian Packages stanza {record} produced {} records; expected exactly one",
            entries.len()
        )));
    }
    visitor(entries.pop().expect("one stanza entry"))?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Read};

    struct TinyChunks<'a> {
        bytes: &'a [u8],
        offset: usize,
        chunk: usize,
    }

    impl Read for TinyChunks<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.offset == self.bytes.len() {
                return Ok(0);
            }
            let count = output
                .len()
                .min(self.chunk)
                .min(self.bytes.len() - self.offset);
            output[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
            self.offset += count;
            Ok(count)
        }
    }

    const STANZA: &str = "Package: split-token\nVersion: 1.0-1\nArchitecture: amd64\nSHA256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nSize: 12\nFilename: pool/s/split-token.deb\n";

    #[test]
    fn stanza_fields_survive_every_byte_boundary() {
        let input = format!("{STANZA}\n{STANZA}");
        let reader = std::io::BufReader::with_capacity(
            3,
            TinyChunks {
                bytes: input.as_bytes(),
                offset: 0,
                chunk: 1,
            },
        );
        let mut names = Vec::new();
        assert_eq!(
            parse_packages(reader, |entry| {
                names.push(entry.package);
                Ok(())
            })
            .unwrap(),
            2
        );
        assert_eq!(names, ["split-token", "split-token"]);
    }

    #[test]
    fn malformed_final_stanza_is_not_silently_dropped() {
        let error = parse_packages(
            std::io::Cursor::new(b"Package: unfinished\nVersion\n"),
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("stanza 1 is malformed"));
    }
}
