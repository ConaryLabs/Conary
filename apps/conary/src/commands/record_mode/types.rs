// apps/conary/src/commands/record_mode/types.rs

use std::path::PathBuf;

use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestedRecordBackend {
    Auto,
    Fanotify,
    Inotify,
}

impl RequestedRecordBackend {
    pub(crate) fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "fanotify" => Ok(Self::Fanotify),
            "inotify" => Ok(Self::Inotify),
            other => {
                bail!("unsupported record backend `{other}`; expected auto, fanotify, or inotify")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RecordCliRequest {
    pub(crate) source: PathBuf,
    pub(crate) output_dir: PathBuf,
    pub(crate) backend: RequestedRecordBackend,
    pub(crate) validate: bool,
    pub(crate) keep_raw_trace: bool,
    pub(crate) unsafe_host: bool,
    pub(crate) allow_network: bool,
    pub(crate) json: bool,
    pub(crate) command: Vec<String>,
}
