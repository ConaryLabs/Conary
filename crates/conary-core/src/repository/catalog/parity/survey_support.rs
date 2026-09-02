// crates/conary-core/src/repository/catalog/parity/survey_support.rs

//! Shared bounded canonical-JSON support for diagnostics-only surveys.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};

/// Incremental compact-JSON accounting for one retained evidence value.
#[allow(dead_code)] // Item-wise builders are used by feature-gated native producers.
pub(super) struct SurveyEvidenceBudget {
    limit: u64,
    bytes: u64,
}

#[allow(dead_code)]
impl SurveyEvidenceBudget {
    pub(super) fn for_value<T: Serialize>(value: &T, limit: u64) -> Option<Self> {
        canonical_value_size_with_limit(value, limit)
            .ok()
            .flatten()
            .map(|bytes| Self { limit, bytes })
    }

    pub(super) fn for_explanation<T: Serialize>(value: &T, limit: u64) -> Option<Self> {
        Self::for_value(value, limit)
    }

    pub(super) fn retain<T: Serialize>(&mut self, value: &T, preceded_by_item: bool) -> bool {
        let separator_bytes = u64::from(preceded_by_item);
        let Some(remaining) = self
            .limit
            .checked_sub(self.bytes)
            .and_then(|remaining| remaining.checked_sub(separator_bytes))
        else {
            return false;
        };
        let Ok(Some(value_bytes)) = canonical_value_size_with_limit(value, remaining) else {
            return false;
        };
        let Some(bytes) = self
            .bytes
            .checked_add(separator_bytes)
            .and_then(|bytes| bytes.checked_add(value_bytes))
        else {
            return false;
        };
        self.bytes = bytes;
        true
    }
}

pub(super) fn canonical_value_size_with_limit<T: Serialize>(
    value: &T,
    limit: u64,
) -> Result<Option<u64>> {
    let mut counter = CanonicalSizeCounter {
        limit,
        bytes: 0,
        exceeded: false,
    };
    match serde_json::to_writer(&mut counter, value) {
        Ok(()) => Ok(Some(counter.bytes)),
        Err(_) if counter.exceeded => Ok(None),
        Err(error) => Err(Error::ParseError(format!(
            "serialize resolution survey evidence: {error}"
        ))),
    }
}

pub(super) fn write_private_canonical_json<T: Serialize>(
    path: &Path,
    value: &T,
    label: &str,
) -> Result<()> {
    let bytes = crate::json::canonical_json(value)
        .map_err(|error| Error::ParseError(format!("serialize {label}: {error}")))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

struct CanonicalSizeCounter {
    limit: u64,
    bytes: u64,
    exceeded: bool,
}

impl Write for CanonicalSizeCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let buffer_bytes = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if buffer_bytes > self.limit.saturating_sub(self.bytes) {
            self.exceeded = true;
            return Err(io::Error::other(
                "resolution survey evidence byte limit exceeded",
            ));
        }
        self.bytes += buffer_bytes;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
