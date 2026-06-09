// conary-core/src/ccs/convert/scriptlet_bundle.rs
//! Passive legacy scriptlet bundle construction for legacy package conversion.

mod builder;
mod classification;
mod digest;
mod entries;
mod format_metadata;
mod native_contracts;
mod summary;
#[cfg(test)]
mod test_support;
mod types;

pub use builder::build_legacy_scriptlet_bundle;
pub use types::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary,
};
