// conary-core/src/packages/deb/debconf/tests/support.rs

use super::super::*;
use crate::packages::native_abi::{DebconfTemplateField, DebconfTemplateRecord};
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct QueueLoader {
    pub(super) paths: Vec<Vec<u8>>,
    pub(super) results: VecDeque<Result<Vec<DebconfTemplateRecord>, DebconfTemplateLoadError>>,
}

impl DebconfTemplateLoader for QueueLoader {
    fn load(
        &mut self,
        path: &[u8],
    ) -> Result<Vec<DebconfTemplateRecord>, DebconfTemplateLoadError> {
        self.paths.push(path.to_vec());
        self.results
            .pop_front()
            .unwrap_or_else(|| Err(DebconfTemplateLoadError::open(b"not queued".to_vec())))
    }
}

pub(super) fn field(name: &str, value: &str, continuation_lines: &[&str]) -> DebconfTemplateField {
    DebconfTemplateField {
        name: name.to_string(),
        value: value.to_string(),
        continuation_lines: continuation_lines
            .iter()
            .map(|line| (*line).to_string())
            .collect(),
    }
}

pub(super) fn record(
    name: &str,
    template_type: &str,
    extra_fields: Vec<DebconfTemplateField>,
) -> DebconfTemplateRecord {
    let mut fields = vec![
        field("Template", name, &[]),
        field("Type", template_type, &[]),
    ];
    fields.extend(extra_fields);
    DebconfTemplateRecord {
        template: name.to_string(),
        template_type: template_type.to_string(),
        fields,
    }
}

pub(super) fn string_record(name: &str, default: &str) -> DebconfTemplateRecord {
    record(
        name,
        "string",
        vec![
            field("Default", default, &[]),
            field("Description", "Demo question", &[]),
        ],
    )
}

pub(super) fn response(execution: DebconfExecution) -> DebconfResponse {
    match execution {
        DebconfExecution::Reply(response) => response,
        DebconfExecution::Stopped => panic!("expected a protocol reply"),
    }
}

pub(super) fn line(
    engine: &mut DebconfEngine<QueueLoader>,
    state: &mut DebconfState,
    session: &mut DebconfSession,
    command: &[u8],
) -> DebconfResponse {
    response(engine.process_line(state, session, command))
}

pub(super) fn assert_code(response: &DebconfResponse, code: DebconfResultCode) {
    assert_eq!(response.code, code, "RET={:?}", response.ret);
}
