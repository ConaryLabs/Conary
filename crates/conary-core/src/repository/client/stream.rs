// crates/conary-core/src/repository/client/stream.rs

//! Bounded HTTP response streaming and typed byte-range validation.

use std::fs::File;
use std::io::Write;
use std::time::Duration;

use indicatif::ProgressBar;
use reqwest::header::{self, HeaderMap};

use crate::error::{Error, Result};
use crate::repository::catalog::CatalogMetadataStreamAdmission;
use crate::repository::error_helpers::ResultExt;

/// Options for one response-body segment written into a staged file.
pub(super) struct ResponseStreamOptions<'a> {
    pub(super) total_size: u64,
    pub(super) offset: u64,
    pub(super) progress_bar: Option<&'a ProgressBar>,
    pub(super) display_name: &'a str,
    pub(super) max_size: Option<u64>,
    pub(super) scratch_admission: Option<&'a dyn CatalogMetadataStreamAdmission>,
    pub(super) inactivity_timeout: Duration,
}

/// Stream one HTTP response segment while hashing and admitting its exact
/// served bytes before every filesystem write.
pub(super) async fn stream_response_to_file(
    mut response: reqwest::Response,
    file: &mut File,
    hasher: &mut crate::hash::Hasher,
    options: ResponseStreamOptions<'_>,
) -> Result<u64> {
    if let Some(progress) = options.progress_bar {
        if options.total_size > 0 {
            progress.set_length(options.total_size);
            progress.set_position(options.offset);
            progress.set_message(options.display_name.to_string());
        } else {
            progress.set_message(format!("{} (unknown size)", options.display_name));
        }
    }

    let mut downloaded = options.offset;
    loop {
        let chunk = tokio::time::timeout(options.inactivity_timeout, response.chunk())
            .await
            .map_err(|_| {
                Error::DownloadError(format!(
                    "response stream made no progress for {:?}",
                    options.inactivity_timeout
                ))
            })?
            .map_err(|error| Error::DownloadError(format!("read response stream: {error}")))?;
        let Some(chunk) = chunk else {
            break;
        };
        let next_size = downloaded
            .checked_add(u64::try_from(chunk.len()).map_err(|_| {
                Error::DownloadError("response chunk length exceeds u64".to_string())
            })?)
            .ok_or_else(|| {
                Error::DownloadError("downloaded response size exceeds u64".to_string())
            })?;
        if options.max_size.is_some_and(|maximum| next_size > maximum) {
            return Err(Error::DownloadError(format!(
                "response exceeded declared size limit of {} bytes",
                options.max_size.expect("checked maximum")
            )));
        }
        let _scratch_permit = options
            .scratch_admission
            .map(|admission| {
                admission.reserve_next(u64::try_from(chunk.len()).map_err(|_| {
                    Error::DownloadError("response chunk length exceeds u64".to_string())
                })?)
            })
            .transpose()?;
        file.write_all(&chunk).io_context("write download data")?;
        hasher.update(&chunk);
        downloaded = next_size;

        if let Some(progress) = options.progress_bar {
            progress.set_position(downloaded);
        }
    }
    Ok(downloaded)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContentRange {
    Partial { start: u64, end: u64, total: u64 },
    Unsatisfied { total: u64 },
}

/// Parse the exact RFC 9110 byte-range forms used for a 206 response or a 416
/// completion/reset decision. No wildcard total or alternate range unit can
/// authorize staged-byte reuse.
pub(super) fn content_range(headers: &HeaderMap) -> Result<ContentRange> {
    let value = headers
        .get(header::CONTENT_RANGE)
        .ok_or_else(|| Error::ParseError("byte-range response has no Content-Range".to_string()))?
        .to_str()
        .map_err(|error| {
            Error::ParseError(format!(
                "byte-range response has non-ASCII Content-Range: {error}"
            ))
        })?;
    let value = value.strip_prefix("bytes ").ok_or_else(|| {
        Error::ParseError(format!(
            "byte-range response uses unsupported Content-Range unit: {value}"
        ))
    })?;
    if let Some(total) = value.strip_prefix("*/") {
        return Ok(ContentRange::Unsatisfied {
            total: parse_range_integer(total, "unsatisfied range total")?,
        });
    }
    let (range, total) = value
        .split_once('/')
        .ok_or_else(|| Error::ParseError(format!("malformed byte Content-Range: {value}")))?;
    if total.contains('/') {
        return Err(Error::ParseError(format!(
            "malformed byte Content-Range total: {value}"
        )));
    }
    let (start, end) = range.split_once('-').ok_or_else(|| {
        Error::ParseError(format!("malformed byte Content-Range interval: {value}"))
    })?;
    if end.contains('-') {
        return Err(Error::ParseError(format!(
            "malformed byte Content-Range interval: {value}"
        )));
    }
    let start = parse_range_integer(start, "range start")?;
    let end = parse_range_integer(end, "range end")?;
    let total = parse_range_integer(total, "range total")?;
    if start > end || end >= total {
        return Err(Error::ParseError(format!(
            "impossible byte Content-Range interval {start}-{end}/{total}"
        )));
    }
    Ok(ContentRange::Partial { start, end, total })
}

fn parse_range_integer(value: &str, label: &str) -> Result<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::ParseError(format!(
            "byte Content-Range {label} is not an unsigned decimal integer: {value:?}"
        )));
    }
    value.parse::<u64>().map_err(|error| {
        Error::ParseError(format!("byte Content-Range {label} exceeds u64: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_RANGE, value.parse().unwrap());
        headers
    }

    #[test]
    fn content_range_accepts_only_typed_byte_intervals() {
        assert_eq!(
            content_range(&headers("bytes 3-5/6")).unwrap(),
            ContentRange::Partial {
                start: 3,
                end: 5,
                total: 6
            }
        );
        assert_eq!(
            content_range(&headers("bytes */6")).unwrap(),
            ContentRange::Unsatisfied { total: 6 }
        );
        for invalid in [
            "items 3-5/6",
            "bytes 3-6/6",
            "bytes 5-3/6",
            "bytes 3-5/*",
            "bytes 3--5/6",
            "bytes */*",
        ] {
            assert!(content_range(&headers(invalid)).is_err(), "{invalid}");
        }
    }
}
