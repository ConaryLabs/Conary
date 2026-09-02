// crates/conary-core/src/repository/catalog/parity/resolution_survey/evidence.rs

//! Exact byte accounting for bounded native survey explanations.

use std::io::{self, Write};

use serde::Serialize;

use super::NativeResolutionSurveyNativeExplanationV1;
use crate::error::{Error, Result};

/// Incremental compact-JSON accounting for one native explanation.
///
/// Builders charge the empty typed container first, then each retained list
/// item and its comma. This is the exact size the collector later validates,
/// without retaining more native records than the remaining survey budget.
#[allow(dead_code)] // Shared by feature-gated native explanation builders.
pub(crate) struct NativeExplanationBudget {
    limit: u64,
    bytes: u64,
}

#[allow(dead_code)]
impl NativeExplanationBudget {
    pub(crate) fn for_explanation(
        explanation: &NativeResolutionSurveyNativeExplanationV1,
        limit: u64,
    ) -> Option<Self> {
        canonical_explanation_size_with_limit(explanation, limit)
            .ok()
            .flatten()
            .map(|bytes| Self { limit, bytes })
    }

    pub(crate) fn retain<T: Serialize>(&mut self, value: &T, preceded_by_item: bool) -> bool {
        let separator_bytes = u64::from(preceded_by_item);
        let Some(remaining) = self
            .limit
            .checked_sub(self.bytes)
            .and_then(|remaining| remaining.checked_sub(separator_bytes))
        else {
            return false;
        };
        let Ok(Some(value_bytes)) = serialized_size_with_limit(value, remaining) else {
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

/// Key ordering changes canonical JSON order but not its compact encoded length,
/// so a bounded serde writer measures the exact canonical size without
/// allocating a second copy of a potentially large explanation.
pub(crate) fn canonical_explanation_size_with_limit(
    explanation: &NativeResolutionSurveyNativeExplanationV1,
    limit: u64,
) -> Result<Option<u64>> {
    serialized_size_with_limit(explanation, limit)
}

fn serialized_size_with_limit<T: Serialize>(value: &T, limit: u64) -> Result<Option<u64>> {
    let mut counter = ExplanationSizeCounter {
        limit,
        bytes: 0,
        exceeded: false,
    };
    match serde_json::to_writer(&mut counter, value) {
        Ok(()) => Ok(Some(counter.bytes)),
        Err(_) if counter.exceeded => Ok(None),
        Err(error) => Err(Error::ParseError(format!(
            "serialize native resolution survey explanation: {error}"
        ))),
    }
}

struct ExplanationSizeCounter {
    limit: u64,
    bytes: u64,
    exceeded: bool,
}

impl Write for ExplanationSizeCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let buffer_bytes = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if buffer_bytes > self.limit.saturating_sub(self.bytes) {
            self.exceeded = true;
            return Err(io::Error::other(
                "native resolution survey evidence byte limit exceeded",
            ));
        }
        self.bytes += buffer_bytes;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
