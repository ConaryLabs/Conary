// apps/conary/src/lib.rs
//! Shared Conary CLI command surface for in-process callers.

#![allow(private_interfaces)]

pub mod app;
pub mod cli;
pub mod command_risk;
pub mod commands;
pub mod dispatch;
pub mod live_host_safety;
