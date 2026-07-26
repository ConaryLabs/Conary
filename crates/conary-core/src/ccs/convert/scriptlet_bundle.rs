// conary-core/src/ccs/convert/scriptlet_bundle.rs
//! Exact lifecycle-bundle construction for native package conversion.

mod builder;
mod digest;
mod entries;
mod format_metadata;
mod native_contracts;
mod summary;
mod types;

pub use builder::{build_direct_native_lifecycle_bundle, build_native_lifecycle_bundle};
pub use types::{ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary};
