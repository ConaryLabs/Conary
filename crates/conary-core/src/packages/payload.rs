// conary-core/src/packages/payload.rs

//! Reopenable package payload sources.
//!
//! Install, conversion, and trusted CCS paths carry content identity plus a
//! source that can be opened repeatedly. Whole-file byte vectors remain only
//! behind an explicit bounded in-memory API for tests and small tooling.

use crate::error::{Error, Result};
use crate::packages::traits::ExtractedFile;
use crate::payload::{PayloadContentAuthority, PayloadNode, PayloadNodeKind};
use std::fmt;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod storage;

pub use storage::{PayloadSpool, ensure_available_space};

/// Fixed buffer used for payload copy and verification operations.
pub const PAYLOAD_IO_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Clone)]
pub struct ReopenablePayload {
    backing: PayloadBacking,
}

#[derive(Clone)]
enum PayloadBacking {
    File {
        path: PathBuf,
        _spool: Option<Arc<tempfile::TempDir>>,
    },
    /// Bounded test/tooling backing. Install parsers and trusted archive
    /// readers must use file-backed sources.
    InMemoryBytes(Arc<[u8]>),
}

impl fmt::Debug for ReopenablePayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.backing {
            PayloadBacking::File { path, .. } => formatter
                .debug_tuple("ReopenablePayload::File")
                .field(path)
                .finish(),
            PayloadBacking::InMemoryBytes(bytes) => formatter
                .debug_struct("ReopenablePayload::InMemoryBytes")
                .field("size", &bytes.len())
                .finish(),
        }
    }
}

impl ReopenablePayload {
    /// Use an existing durable file as a reopenable source.
    #[must_use]
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self {
            backing: PayloadBacking::File {
                path: path.into(),
                _spool: None,
            },
        }
    }

    /// Use one file owned by a temporary payload spool.
    #[must_use]
    pub fn from_spooled_path(path: impl Into<PathBuf>, spool: Arc<tempfile::TempDir>) -> Self {
        Self {
            backing: PayloadBacking::File {
                path: path.into(),
                _spool: Some(spool),
            },
        }
    }

    /// Explicit bounded in-memory source for tests and small tooling.
    #[must_use]
    pub fn from_in_memory_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            backing: PayloadBacking::InMemoryBytes(bytes.into()),
        }
    }

    pub fn open(&self) -> Result<Box<dyn Read + Send>> {
        match &self.backing {
            PayloadBacking::File { path, .. } => File::open(path)
                .map(|file| Box::new(file) as Box<dyn Read + Send>)
                .map_err(Error::from),
            PayloadBacking::InMemoryBytes(bytes) => Ok(Box::new(Cursor::new(Arc::clone(bytes)))),
        }
    }

    /// Return the backing path when this is a file source.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match &self.backing {
            PayloadBacking::File { path, .. } => Some(path),
            PayloadBacking::InMemoryBytes(_) => None,
        }
    }
}

/// One package payload node with exact signed/source-native content identity.
#[derive(Debug, Clone)]
pub struct PackagePayloadFile {
    pub path: String,
    pub node: PayloadNode,
    pub content_authority: Option<PayloadContentAuthority>,
    source: Option<ReopenablePayload>,
}

impl PackagePayloadFile {
    pub fn new(
        path: String,
        node: PayloadNode,
        content_authority: Option<PayloadContentAuthority>,
        source: Option<ReopenablePayload>,
    ) -> Result<Self> {
        node.validate_content(content_authority.as_ref())
            .map_err(|error| {
                Error::ParseError(format!("invalid payload authority for {path}: {error}"))
            })?;
        if node.kind.is_regular() != source.is_some() {
            return Err(Error::ParseError(format!(
                "regular payload source presence disagrees with node kind for {path}"
            )));
        }
        Ok(Self {
            path,
            node,
            content_authority,
            source,
        })
    }

    pub fn open_content(&self) -> Result<Box<dyn Read + Send>> {
        self.source
            .as_ref()
            .ok_or_else(|| {
                Error::ParseError(format!(
                    "non-regular payload node {} has no content stream",
                    self.path
                ))
            })?
            .open()
    }

    #[must_use]
    pub fn source(&self) -> Option<&ReopenablePayload> {
        self.source.as_ref()
    }

    fn from_extracted_in_memory(file: ExtractedFile) -> Result<Self> {
        let source = match file.node.kind {
            PayloadNodeKind::Regular { .. } => Some(ReopenablePayload::from_in_memory_bytes(
                Arc::<[u8]>::from(file.content),
            )),
            _ => {
                if !file.content.is_empty() {
                    return Err(Error::ParseError(format!(
                        "non-regular in-memory payload node {} carries ambiguous bytes",
                        file.path
                    )));
                }
                None
            }
        };
        Self::new(file.path, file.node, file.content_authority, source)
    }

    /// Materialize one source for explicitly bounded in-memory callers only.
    pub fn to_extracted_in_memory(&self) -> Result<ExtractedFile> {
        let content = if let Some(authority) = &self.content_authority {
            let capacity = usize::try_from(authority.size).map_err(|_| {
                Error::ParseError(format!(
                    "in-memory extraction cannot address {} bytes for {}",
                    authority.size, self.path
                ))
            })?;
            let mut reader = self.open_content()?;
            let mut content = Vec::with_capacity(capacity);
            reader
                .by_ref()
                .take(authority.size.saturating_add(1))
                .read_to_end(&mut content)?;
            if content.len() as u64 != authority.size {
                return Err(Error::ParseError(format!(
                    "payload source size mismatch for {}: expected {}, got {}",
                    self.path,
                    authority.size,
                    content.len()
                )));
            }
            content
        } else {
            Vec::new()
        };
        Ok(ExtractedFile {
            path: self.path.clone(),
            node: self.node.clone(),
            content,
            content_authority: self.content_authority.clone(),
        })
    }
}

/// Complete package payload projected as independently reopenable files.
#[derive(Debug, Clone, Default)]
pub struct PackagePayload {
    files: Vec<PackagePayloadFile>,
}

impl PackagePayload {
    #[must_use]
    pub fn new(files: Vec<PackagePayloadFile>) -> Self {
        Self { files }
    }

    #[must_use]
    pub fn files(&self) -> &[PackagePayloadFile] {
        &self.files
    }

    #[must_use]
    pub fn into_files(self) -> Vec<PackagePayloadFile> {
        self.files
    }

    pub fn from_extracted_in_memory(files: Vec<ExtractedFile>) -> Result<Self> {
        files
            .into_iter()
            .map(PackagePayloadFile::from_extracted_in_memory)
            .collect::<Result<Vec<_>>>()
            .map(Self::new)
    }

    pub fn to_extracted_in_memory(&self) -> Result<Vec<ExtractedFile>> {
        self.files
            .iter()
            .map(PackagePayloadFile::to_extracted_in_memory)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopenable_source_preserves_exact_bytes() {
        let file = PackagePayloadFile::new(
            "/fixture".to_string(),
            PayloadNode::regular(0o644),
            Some(PayloadContentAuthority {
                sha256: crate::hash::sha256(b"payload"),
                size: 7,
            }),
            Some(ReopenablePayload::from_in_memory_bytes(Arc::<[u8]>::from(
                &b"payload"[..],
            ))),
        )
        .unwrap();

        assert_eq!(file.to_extracted_in_memory().unwrap().content, b"payload");
        assert_eq!(file.to_extracted_in_memory().unwrap().content, b"payload");
    }
}
