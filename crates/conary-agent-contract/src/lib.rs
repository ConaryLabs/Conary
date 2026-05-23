// conary-agent-contract/src/lib.rs
//! Transport-neutral operation contract for Conary agent-facing workflows.

pub mod catalog;
pub mod resource;
pub mod result;

pub use catalog::*;
pub use resource::*;
pub use result::*;
