// conary-core/src/packages/deb/debconf/session.rs

//! Per-confmodule debconf session state.

use super::{DebconfPriority, DebconfState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfSessionPolicy {
    /// Lowest priority that would be visible on an interactive frontend.
    pub priority: DebconfPriority,
    /// Allow questions whose persistent `seen` flag is true to be queued.
    pub reshow: bool,
    /// Mark eligible hidden questions seen after a successful noninteractive
    /// `GO`. This matches `DEBCONF_NONINTERACTIVE_SEEN`.
    pub noninteractive_seen: bool,
}

impl Default for DebconfSessionPolicy {
    fn default() -> Self {
        Self {
            priority: DebconfPriority::High,
            reshow: false,
            noninteractive_seen: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfPendingQuestion {
    pub question: String,
    pub priority: DebconfPriority,
    /// Noninteractive elements are never visible, but retaining this flag
    /// makes the `GO` backup transition explicit and auditable.
    pub visible: bool,
    pub mark_seen: bool,
    pub block_path: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfBlock {
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfProgress {
    pub minimum: i64,
    pub maximum: i64,
    pub current: i64,
    pub question: String,
    pub info: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebconfGoOutcome {
    Proceed,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebconfProgressOutcome {
    Continue,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebconfSessionRequestError {
    BackupCapabilityNotNegotiated,
    Stopped,
}

impl fmt::Display for DebconfSessionRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackupCapabilityNotNegotiated => {
                formatter.write_str("the confmodule did not negotiate the backup capability")
            }
            Self::Stopped => formatter.write_str("the confmodule session has stopped"),
        }
    }
}

impl std::error::Error for DebconfSessionRequestError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebconfSession {
    pub owner: String,
    pub policy: DebconfSessionPolicy,
    /// Capabilities in caller order. Unknown capabilities are retained but
    /// never treated as authority for behavior.
    pub capabilities: Vec<Vec<u8>>,
    pub pending: Vec<DebconfPendingQuestion>,
    pub busy: BTreeSet<String>,
    /// Questions shown (or explicitly configured to count as shown) in this
    /// run. These become persistently seen only when the session finishes.
    pub seen: BTreeSet<String>,
    pub blocks: Vec<DebconfBlock>,
    pub progress: Option<DebconfProgress>,
    pub title: Vec<u8>,
    pub info: Option<String>,
    pub backed_up: bool,
    pub stopped: bool,
    next_block_id: u64,
    next_go_outcome: DebconfGoOutcome,
    next_progress_outcome: DebconfProgressOutcome,
}

impl DebconfSession {
    pub fn new(owner: impl Into<String>, policy: DebconfSessionPolicy) -> Self {
        Self {
            owner: owner.into(),
            policy,
            capabilities: Vec::new(),
            pending: Vec::new(),
            busy: BTreeSet::new(),
            seen: BTreeSet::new(),
            blocks: Vec::new(),
            progress: None,
            title: Vec::new(),
            info: None,
            backed_up: false,
            stopped: false,
            next_block_id: 1,
            next_go_outcome: DebconfGoOutcome::Proceed,
            next_progress_outcome: DebconfProgressOutcome::Continue,
        }
    }

    pub fn has_capability(&self, capability: &[u8]) -> bool {
        self.capabilities
            .iter()
            .any(|candidate| candidate == capability)
    }

    pub fn request_go_outcome(
        &mut self,
        outcome: DebconfGoOutcome,
    ) -> Result<(), DebconfSessionRequestError> {
        if self.stopped {
            return Err(DebconfSessionRequestError::Stopped);
        }
        if outcome == DebconfGoOutcome::Back && !self.has_capability(b"backup") {
            return Err(DebconfSessionRequestError::BackupCapabilityNotNegotiated);
        }
        self.next_go_outcome = outcome;
        Ok(())
    }

    pub fn request_progress_outcome(
        &mut self,
        outcome: DebconfProgressOutcome,
    ) -> Result<(), DebconfSessionRequestError> {
        if self.stopped {
            return Err(DebconfSessionRequestError::Stopped);
        }
        self.next_progress_outcome = outcome;
        Ok(())
    }

    pub(crate) fn take_go_outcome(&mut self) -> DebconfGoOutcome {
        std::mem::replace(&mut self.next_go_outcome, DebconfGoOutcome::Proceed)
    }

    pub(crate) fn take_progress_outcome(&mut self) -> DebconfProgressOutcome {
        std::mem::replace(
            &mut self.next_progress_outcome,
            DebconfProgressOutcome::Continue,
        )
    }

    pub(crate) fn begin_block(&mut self) {
        let id = self.next_block_id;
        self.next_block_id += 1;
        self.blocks.push(DebconfBlock { id });
    }

    pub(crate) fn end_block(&mut self) -> bool {
        self.blocks.pop().is_some()
    }

    pub(crate) fn block_path(&self) -> Vec<u64> {
        self.blocks.iter().map(|block| block.id).collect()
    }

    pub(crate) fn clear_pending(&mut self) {
        self.pending.clear();
        self.busy.clear();
    }

    pub fn finish(&mut self, state: &mut DebconfState) {
        for name in &self.seen {
            if let Some(question) = state.questions.get_mut(name) {
                question.seen = true;
            }
        }
        self.seen.clear();
        self.clear_pending();
        self.blocks.clear();
        self.progress = None;
        self.stopped = true;
    }
}
