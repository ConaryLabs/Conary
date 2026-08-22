// conary-core/src/repository/universe/mod.rs

//! Signed immutable Remi universe wire and activation contracts.

mod contract;
mod metadata;

pub use contract::{
    REMI_UNIVERSE_SCHEMA_V1, RemiUniverseCanonicalMapObjectV1, RemiUniverseCatalogObjectV1,
    RemiUniverseManifestV1, RemiUniverseProfileV1,
};
pub use metadata::{
    VerifiedRemiUniverseTargetSet, verify_remi_universe_manifest_target,
    verify_remi_universe_object_target,
};
