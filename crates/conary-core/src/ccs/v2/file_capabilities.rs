// conary-core/src/ccs/v2/file_capabilities.rs

//! Canonical signed projection for Linux file capability declarations.

use crate::ccs::manifest::FileCapability;
use anyhow::{Result, bail};
use std::collections::BTreeMap;

pub(crate) fn canonicalize_file_capabilities(
    capabilities: &[FileCapability],
) -> Result<Vec<FileCapability>> {
    let mut by_path = BTreeMap::new();
    for capability in capabilities {
        capability.validate()?;
        let mut canonical = capability.clone();
        canonical.capabilities.sort();
        if by_path.insert(canonical.path.clone(), canonical).is_some() {
            bail!(
                "file capability path {} is declared more than once",
                capability.path
            );
        }
    }
    Ok(by_path.into_values().collect())
}
