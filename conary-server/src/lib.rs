// conary-server/src/lib.rs
//! Temporary compatibility shim for the split remi and conaryd app crates.

pub use conaryd::daemon;
pub use remi::federation;
pub use remi::server;
