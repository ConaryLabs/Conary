// src/commands/generation/takeover_state.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TakeoverStatus {
    Planning,
    Running,
    ReadyToActivate,
    CompletedWithWarnings,
    Incomplete,
    Failed,
}
