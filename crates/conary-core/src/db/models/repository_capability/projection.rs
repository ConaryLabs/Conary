// conary-core/src/db/models/repository_capability/projection.rs

//! Canonical capability projection used to invalidate native conversions.

use super::RepositoryProvide;
use crate::error::{Error, Result};
use crate::repository::dependency_model::ProvidedCapability;
use std::collections::BTreeMap;

pub(super) fn project(
    provides: impl IntoIterator<Item = RepositoryProvide>,
) -> Result<(Vec<ProvidedCapability>, String)> {
    let mut capabilities = BTreeMap::<Vec<u8>, ProvidedCapability>::new();
    for provide in provides {
        let capability = provide.as_provided_capability()?;
        capability.validate()?;
        let canonical = crate::json::canonical_json(&capability).map_err(|error| {
            Error::InternalError(format!(
                "failed to canonicalize repository provide projection: {error}"
            ))
        })?;
        capabilities.entry(canonical).or_insert(capability);
    }
    let capabilities = capabilities.into_values().collect::<Vec<_>>();
    let canonical = crate::json::canonical_json(&capabilities).map_err(|error| {
        Error::InternalError(format!(
            "failed to canonicalize repository conversion metadata: {error}"
        ))
    })?;
    let digest = crate::hash::sha256_prefixed(&canonical);
    Ok((capabilities, digest))
}
