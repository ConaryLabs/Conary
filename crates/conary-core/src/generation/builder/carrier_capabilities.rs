// conary-core/src/generation/builder/carrier_capabilities.rs

//! Persisted target capability projection into generation artifact authority.

use crate::ccs::HostCapabilityInventory;
use crate::generation::artifact::GenerationCarrierCapabilities;

pub(super) fn generation_carrier_capabilities(
    conn: &rusqlite::Connection,
) -> crate::Result<GenerationCarrierCapabilities> {
    let inventory = HostCapabilityInventory::load_required(conn).map_err(|error| {
        crate::Error::ConfigError(format!(
            "generation build requires the current typed host capability inventory: {error}"
        ))
    })?;
    let capabilities = GenerationCarrierCapabilities {
        immutable_backing_security: inventory.immutable_backing_security,
        ..GenerationCarrierCapabilities::default()
    };
    capabilities.validate()?;
    Ok(capabilities)
}
