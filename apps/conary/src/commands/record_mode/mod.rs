// apps/conary/src/commands/record_mode/mod.rs

mod fanotify_backend;
mod inotify_backend;
mod trace;
mod types;
mod workspace;

use anyhow::{Result, bail};

pub(crate) use types::{RecordCliRequest, RequestedRecordBackend};

pub(crate) async fn cmd_cook_record(request: RecordCliRequest) -> Result<()> {
    validate_record_request(&request)?;
    bail!("record mode is not implemented yet")
}

fn validate_record_request(request: &RecordCliRequest) -> Result<()> {
    if request.command.is_empty() {
        bail!("record mode requires a command after `--`");
    }
    if request.allow_network {
        bail!("--record-allow-network is reserved for a later record-mode slice");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: Vec<String>) -> RecordCliRequest {
        RecordCliRequest {
            source: ".".into(),
            output_dir: "recorded/demo".into(),
            backend: RequestedRecordBackend::Auto,
            validate: false,
            keep_raw_trace: false,
            unsafe_host: false,
            allow_network: false,
            json: false,
            command,
        }
    }

    #[test]
    fn record_request_rejects_missing_command() {
        let error = validate_record_request(&request(Vec::new())).unwrap_err();
        assert!(error.to_string().contains("requires a command"));
    }

    #[test]
    fn record_request_rejects_reserved_network_flag() {
        let mut request = request(vec!["make".to_string()]);
        request.allow_network = true;
        let error = validate_record_request(&request).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }
}
