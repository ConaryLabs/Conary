// apps/remi/src/lib.rs
#![recursion_limit = "256"]

pub mod deployment;
#[cfg(feature = "dormant-federation")]
pub mod federation;
pub mod server;
pub mod trust;
