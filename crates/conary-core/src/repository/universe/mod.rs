// conary-core/src/repository/universe/mod.rs

//! Signed immutable Remi universe wire and activation contracts.

mod client;
mod contract;
mod enrollment;
mod index;
mod metadata;

pub use client::{RemiUniverseSyncOutcome, sync_remi_universe};
pub(crate) use enrollment::validate_remi_universe_root;
pub use enrollment::{
    RemiUniverseEnrollmentOutcome, enroll_remi_universe_root, normalize_remi_endpoint,
};
#[allow(unused_imports)]
pub(crate) use index::{ClientUniverseIndex, build_client_universe_index};

pub use contract::{
    REMI_UNIVERSE_SCHEMA_V1, RemiUniverseCanonicalMapObjectV1, RemiUniverseCatalogObjectV1,
    RemiUniverseManifestV1, RemiUniverseProfileV1,
};
pub use metadata::{
    VerifiedRemiUniverseTargetSet, verify_remi_universe_manifest_target,
    verify_remi_universe_object_target,
};
