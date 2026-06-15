# M3a Structured Diagnostics And Events Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the M3a foundation: stable packaging diagnostics/events, redaction, command JSON envelopes, and a file-backed packaging operation-record store for `cook` and `publish`.

**Architecture:** Shared diagnostic/event DTOs live in `conary-core` and are the source of truth for human rendering, JSON output, operation records, and later MCP adapters. The CLI owns rendering and operation-record persistence at the command edge, while command implementations emit structured packaging reports instead of creating a second JSON-only path. `try --json`, MCP tools, watch mode, and record mode remain out of scope for this plan.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, `schemars` only in `conary-agent-contract`, `anyhow`, existing `conary-core::filesystem::durable`, existing CLI tests, `cargo test`.

---

## Scope Locks

M3a includes:

- `schema_version = 1` on command JSON and operation records.
- Core DTOs for `PackagingDiagnostic`, `PackagingEvent`, evidence, suggestions, artifacts, progress, command output, and redaction markers.
- A redactor that handles command evidence, env-like values, bearer tokens, credentialed URLs, private-key paths, and bounded log snippets.
- Redaction metadata on `conary-agent-contract::EvidenceItem` through an additive field.
- CLI rendering helpers in `apps/conary/src/commands/diagnostics.rs`.
- File-backed packaging operation records under XDG state or an explicit test override, with `0700` directory mode, `0600` record mode, atomic writes, newest-50 retention, and redaction-before-write.
- `--json` on `conary cook` and `conary publish`.
- Structured JSON for representative cook validation/inference outcomes, artifact-form publish gate failures, and project-form publish preflight failures.
- A structured `PublishJsonUnsupported` error when `conary publish --json` is pointed at the Remi/HTTP route; Remi JSON success is deferred to the later Remi surface slice.
- Top-level packaging events for command and phase boundaries. Kitchen step-level command events are intentionally deferred until the build-event instrumentation slice so this task does not expose placeholder event streams.

M3a excludes:

- MCP tools or MCP server transport.
- `try --json`, `try --watch`, and try-session decomposition.
- Record-mode tracing.
- A DB migration.

## File Structure

Create:

- `crates/conary-core/src/diagnostics/mod.rs`: stable packaging DTOs and helper constructors.
- `crates/conary-core/src/diagnostics/redaction.rs`: redaction policy, redacted string helpers, and redaction tests.
- `apps/conary/src/commands/diagnostics.rs`: CLI human/JSON rendering, command-envelope helpers, and operation-record integration helpers.
- `apps/conary/tests/packaging_m3a.rs`: end-to-end CLI JSON and operation-record tests.

Modify:

- `crates/conary-core/src/lib.rs`: export the new `diagnostics` module.
- `crates/conary-agent-contract/src/result.rs`: add redaction metadata to `EvidenceItem`.
- `apps/conary/src/commands/mod.rs`: add the command diagnostics module.
- `apps/conary/src/commands/operation_records.rs`: add packaging operation store helpers.
- `crates/conary-core/src/filesystem/durable.rs`: add private-mode atomic JSON/file write helpers reused by operation records.
- `apps/conary/src/commands/cook.rs`: emit structured reports and support `--json`.
- `apps/conary/src/commands/publish.rs`: emit structured publish diagnostics and support `--json`.
- `apps/conary/src/cli/mod.rs`: add `--json` to `cook` and `publish`, while keeping `try --json` hidden/unavailable.
- `apps/conary/src/dispatch/root.rs`: pass the new flags into command options.

Focused verification commands:

```bash
cargo test -p conary-core diagnostics
cargo test -p conary-agent-contract
cargo test -p conary operation_records
cargo test -p conary cook_validate_only_json_has_schema_version_and_summary
cargo test -p conary artifact_form_publish_json_reports_static_gate_failure
cargo test -p conary --test packaging_m3a
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Ownership Boundaries

- `conary-core` owns the stable data contract and redaction mechanics because later MCP and watch surfaces must not depend on CLI internals.
- `apps/conary/src/commands/diagnostics.rs` owns rendering and persistence glue. This keeps `cook.rs` and `publish.rs` as orchestration files instead of burying formatting logic in already-large command handlers.
- `apps/conary/src/commands/operation_records.rs` owns filesystem record paths, permissions, atomic write behavior, recent-record listing, and retention.
- `cook.rs` and `publish.rs` own command-specific report construction and phase-specific diagnostics.

---

### Task 1: Core Packaging Diagnostic And Event DTOs

**Files:**
- Create: `crates/conary-core/src/diagnostics/mod.rs`
- Create: `crates/conary-core/src/diagnostics/redaction.rs`
- Modify: `crates/conary-core/src/lib.rs`

- [ ] **Step 1: Write the failing core serialization tests**

Add this test module at the bottom of `crates/conary-core/src/diagnostics/mod.rs` while creating the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_output_serializes_schema_version_and_diagnostic_code() {
        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::RecipeValidation,
            PackagingDiagnosticCode::RecipeValidationFailed,
            "Recipe validation failed",
        )
        .with_evidence(DiagnosticEvidence::log("validation", "missing [package] section"));

        let output = PackagingCommandOutput::failed(
            "op-1",
            "conary cook",
            vec![diagnostic],
        );

        let value = serde_json::to_value(&output).expect("serialize command output");
        assert_eq!(value["schema_version"], PACKAGING_JSON_SCHEMA_VERSION);
        assert_eq!(value["status"], "failed");
        assert_eq!(value["diagnostics"][0]["code"], "recipe-validation-failed");
        assert_eq!(value["diagnostics"][0]["phase"], "recipe-validation");
    }

    #[test]
    fn event_serializes_sequence_and_optional_diagnostic() {
        let event = PackagingEvent::diagnostic(
            "cook-1",
            7,
            PackagingDiagnostic::warning(
                PackagingPhase::Inference,
                PackagingDiagnosticCode::InferenceTrace,
                "Inference used Cargo metadata",
            ),
        );

        let value = serde_json::to_value(&event).expect("serialize event");
        assert_eq!(value["schema_version"], PACKAGING_JSON_SCHEMA_VERSION);
        assert_eq!(value["operation_id"], "cook-1");
        assert_eq!(value["sequence"], 7);
        assert_eq!(value["kind"], "diagnostic-emitted");
        assert_eq!(value["diagnostic"]["code"], "inference-trace");
    }
}
```

- [ ] **Step 2: Run the failing core tests**

Run:

```bash
cargo test -p conary-core diagnostics::tests
```

Expected: fail because `conary_core::diagnostics` does not exist.

- [ ] **Step 3: Implement the DTO module**

Create `crates/conary-core/src/diagnostics/mod.rs` with these concrete types and constructors:

```rust
// conary-core/src/diagnostics/mod.rs

pub mod redaction;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PACKAGING_JSON_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingCommandStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingPhase {
    Inference,
    RecipeValidation,
    SourceFetch,
    Build,
    TrySession,
    Publish,
    OperationRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingDiagnosticCode {
    InferenceTrace,
    RecipeValidationWarning,
    RecipeValidationFailed,
    SourceCacheMiss,
    BuildNetworkAccess,
    UnpinnedDependency,
    CommandRiskEvidence,
    CookFailed,
    PublishGateFailed,
    ProjectPublishPreflightFailed,
    PublishJsonUnsupported,
    OperationRecordWriteFailed,
    RedactionFailed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticEvidenceKind {
    Command,
    Path,
    Uri,
    Log,
    Check,
    Artifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionMarker {
    pub field: String,
    pub reason: String,
}

impl RedactionMarker {
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticEvidence {
    pub kind: DiagnosticEvidenceKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redactions: Vec<RedactionMarker>,
}

impl DiagnosticEvidence {
    pub fn log(summary: impl Into<String>, log: impl Into<String>) -> Self {
        Self {
            kind: DiagnosticEvidenceKind::Log,
            summary: summary.into(),
            path: None,
            uri: None,
            command: None,
            log: Some(log.into()),
            metadata: BTreeMap::new(),
            redactions: Vec::new(),
        }
    }

    pub fn artifact(summary: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            kind: DiagnosticEvidenceKind::Artifact,
            summary: summary.into(),
            path: Some(path.into()),
            uri: None,
            command: None,
            log: None,
            metadata: BTreeMap::new(),
            redactions: Vec::new(),
        }
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    pub fn with_redaction(mut self, redaction: RedactionMarker) -> Self {
        self.redactions.push(redaction);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagingSuggestion {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

impl PackagingSuggestion {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            command: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackagingDiagnostic {
    pub phase: PackagingPhase,
    pub code: PackagingDiagnosticCode,
    pub severity: PackagingSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<DiagnosticEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<PackagingSuggestion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redactions: Vec<RedactionMarker>,
}

impl PackagingDiagnostic {
    pub fn new(
        phase: PackagingPhase,
        code: PackagingDiagnosticCode,
        severity: PackagingSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            code,
            severity,
            message: message.into(),
            evidence: Vec::new(),
            suggestions: Vec::new(),
            redactions: Vec::new(),
        }
    }

    pub fn info(
        phase: PackagingPhase,
        code: PackagingDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self::new(phase, code, PackagingSeverity::Info, message)
    }

    pub fn warning(
        phase: PackagingPhase,
        code: PackagingDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self::new(phase, code, PackagingSeverity::Warning, message)
    }

    pub fn error(
        phase: PackagingPhase,
        code: PackagingDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self::new(phase, code, PackagingSeverity::Error, message)
    }

    pub fn with_evidence(mut self, evidence: DiagnosticEvidence) -> Self {
        self.evidence.push(evidence);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagingArtifact {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagingProgress {
    pub current: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingEventKind {
    OperationStarted,
    PhaseStarted,
    PhaseFinished,
    CommandStarted,
    CommandFailed,
    DiagnosticEmitted,
    ArtifactCreated,
    OperationFinished,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackagingEvent {
    pub schema_version: u16,
    pub operation_id: String,
    pub sequence: u64,
    pub phase: PackagingPhase,
    pub kind: PackagingEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<PackagingDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<PackagingArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<PackagingProgress>,
}

impl PackagingEvent {
    pub fn diagnostic(
        operation_id: impl Into<String>,
        sequence: u64,
        diagnostic: PackagingDiagnostic,
    ) -> Self {
        Self {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: operation_id.into(),
            sequence,
            phase: diagnostic.phase,
            kind: PackagingEventKind::DiagnosticEmitted,
            message: Some(diagnostic.message.clone()),
            diagnostic: Some(diagnostic),
            artifact: None,
            progress: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackagingCommandOutput {
    pub schema_version: u16,
    pub operation_id: String,
    pub command: String,
    pub status: PackagingCommandStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<PackagingDiagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<PackagingEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<PackagingArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl PackagingCommandOutput {
    pub fn succeeded(operation_id: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: operation_id.into(),
            command: command.into(),
            status: PackagingCommandStatus::Succeeded,
            diagnostics: Vec::new(),
            events: Vec::new(),
            artifacts: Vec::new(),
            summary: None,
        }
    }

    pub fn failed(
        operation_id: impl Into<String>,
        command: impl Into<String>,
        diagnostics: Vec<PackagingDiagnostic>,
    ) -> Self {
        Self {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: operation_id.into(),
            command: command.into(),
            status: PackagingCommandStatus::Failed,
            diagnostics,
            events: Vec::new(),
            artifacts: Vec::new(),
            summary: None,
        }
    }
}
```

Modify `crates/conary-core/src/lib.rs`:

```rust
pub mod diagnostics;
```

- [ ] **Step 4: Run the core DTO tests**

Run:

```bash
cargo test -p conary-core diagnostics::tests
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/lib.rs crates/conary-core/src/diagnostics/mod.rs crates/conary-core/src/diagnostics/redaction.rs
git commit -m "feat(packaging): add diagnostic event dto"
```

---

### Task 2: Core Redaction Policy

**Files:**
- Modify: `crates/conary-core/src/diagnostics/redaction.rs`
- Modify: `crates/conary-core/src/diagnostics/mod.rs`

- [ ] **Step 1: Write redaction tests**

Create `crates/conary-core/src/diagnostics/redaction.rs` with tests first:

```rust
// conary-core/src/diagnostics/redaction.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_token_env_assignments_and_bearer_values() {
        let value = redact_text("API_TOKEN=sk-secret curl -H 'Authorization: Bearer abc.def'");
        assert!(!value.value.contains("sk-secret"));
        assert!(!value.value.contains("abc.def"));
        assert!(value.value.contains("API_TOKEN=[REDACTED]"));
        assert!(value.value.contains("Bearer [REDACTED]"));
        assert!(value.redactions.iter().any(|item| item.field == "text"));
    }

    #[test]
    fn redacts_credentialed_urls() {
        let value = redact_text("https://user:pass@example.invalid/source.tar.gz");
        assert_eq!(value.value, "https://[REDACTED]@example.invalid/source.tar.gz");
        assert_eq!(value.redactions[0].reason, "credentialed-url");
    }

    #[test]
    fn redacts_private_key_paths() {
        let value = redact_text("/home/dev/.ssh/id_ed25519");
        assert_eq!(value.value, "[REDACTED-PATH]");
        assert_eq!(value.redactions[0].reason, "private-key-path");
    }

    #[test]
    fn does_not_redact_generic_pem_or_key_words_without_path_shape() {
        let value = redact_text("documented files include bundle.pem and api.key examples");
        assert_eq!(
            value.value,
            "documented files include bundle.pem and api.key examples"
        );
        assert!(value.redactions.is_empty());
    }

    #[test]
    fn redact_command_preserves_argument_boundaries() {
        let command = redact_command(&[
            "curl".to_string(),
            "-H".to_string(),
            "Authorization: Bearer abc.def".to_string(),
        ]);
        assert_eq!(command.value[0], "curl");
        assert_eq!(command.value[2], "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redact_log_redacts_and_bounds_long_output() {
        let input = format!(
            "Authorization: Bearer abc.def\n{}",
            "x".repeat(MAX_DIAGNOSTIC_LOG_BYTES + 64)
        );
        let value = redact_log(&input);
        assert!(!value.value.contains("abc.def"));
        assert!(value.value.contains("Bearer [REDACTED]"));
        assert!(value.value.contains("[TRUNCATED]"));
        assert!(value
            .redactions
            .iter()
            .any(|item| item.reason == "log-truncated"));
    }

    #[test]
    fn redact_json_value_walks_nested_metadata() {
        let mut value = serde_json::json!({
            "publish_lint_report": {
                "url": "https://user:pass@example.invalid/pkg.ccs",
                "nested": ["API_TOKEN=sk-secret"]
            }
        });
        let redactions = redact_json_value(&mut value, "metadata");
        let text = value.to_string();
        assert!(!text.contains("user:pass"));
        assert!(!text.contains("sk-secret"));
        assert!(text.contains("[REDACTED]"));
        assert!(redactions.iter().any(|item| item.field.contains("metadata")));
    }
}
```

- [ ] **Step 2: Run the failing redaction tests**

Run:

```bash
cargo test -p conary-core diagnostics::redaction::tests
```

Expected: fail because the redaction functions do not exist.

- [ ] **Step 3: Implement redaction functions**

Fill `redaction.rs` above the tests:

```rust
use serde_json::Value;

use super::RedactionMarker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedText {
    pub value: String,
    pub redactions: Vec<RedactionMarker>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedCommand {
    pub value: Vec<String>,
    pub redactions: Vec<RedactionMarker>,
}

const SECRET_KEYS: &[&str] = &[
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "API_KEY",
    "ACCESS_KEY",
    "PRIVATE_KEY",
];

pub const MAX_DIAGNOSTIC_LOG_BYTES: usize = 16 * 1024;

pub fn redact_text(input: &str) -> RedactedText {
    let mut value = input.to_string();
    let mut redactions = Vec::new();

    if is_private_key_path(&value) {
        return RedactedText {
            value: "[REDACTED-PATH]".to_string(),
            redactions: vec![RedactionMarker::new("text", "private-key-path")],
        };
    }

    if let Some(redacted) = redact_credentialed_url(&value) {
        value = redacted;
        redactions.push(RedactionMarker::new("text", "credentialed-url"));
    }

    for key in SECRET_KEYS {
        let marker = format!("{key}=");
        if let Some(start) = value.to_ascii_uppercase().find(&marker) {
            let key_start = start;
            let value_start = start + marker.len();
            let value_end = value[value_start..]
                .find(|ch: char| ch.is_whitespace() || ch == '\'' || ch == '"')
                .map(|offset| value_start + offset)
                .unwrap_or_else(|| value.len());
            value.replace_range(key_start..value_end, &format!("{key}=[REDACTED]"));
            redactions.push(RedactionMarker::new("text", "secret-env-assignment"));
        }
    }

    for prefix in ["Bearer ", "bearer "] {
        let mut search_start = 0;
        while let Some(relative_start) = value[search_start..].find(prefix) {
            let start = search_start + relative_start;
            let token_start = start + prefix.len();
            if value[token_start..].starts_with("[REDACTED]") {
                search_start = token_start + "[REDACTED]".len();
                continue;
            }
            let token_end = value[token_start..]
                .find(|ch: char| ch.is_whitespace() || ch == '\'' || ch == '"')
                .map(|offset| token_start + offset)
                .unwrap_or_else(|| value.len());
            value.replace_range(token_start..token_end, "[REDACTED]");
            redactions.push(RedactionMarker::new("text", "bearer-token"));
            search_start = token_start + "[REDACTED]".len();
        }
    }

    RedactedText { value, redactions }
}

pub fn redact_log(input: &str) -> RedactedText {
    let mut redactions = Vec::new();
    let bounded = if input.len() > MAX_DIAGNOSTIC_LOG_BYTES {
        let mut end = MAX_DIAGNOSTIC_LOG_BYTES;
        while !input.is_char_boundary(end) {
            end -= 1;
        }
        redactions.push(RedactionMarker::new("log", "log-truncated"));
        format!("{}\n[TRUNCATED]", &input[..end])
    } else {
        input.to_string()
    };
    let redacted = redact_text(&bounded);
    redactions.extend(redacted.redactions);
    RedactedText {
        value: redacted.value,
        redactions,
    }
}

pub fn redact_command(command: &[String]) -> RedactedCommand {
    let mut redacted = Vec::with_capacity(command.len());
    let mut markers = Vec::new();
    for arg in command {
        let item = redact_text(arg);
        redacted.push(item.value);
        markers.extend(item.redactions);
    }
    RedactedCommand {
        value: redacted,
        redactions: markers,
    }
}

pub fn redact_json_value(value: &mut Value, field: &str) -> Vec<RedactionMarker> {
    match value {
        Value::String(text) => {
            let redacted = redact_text(text);
            *text = redacted.value;
            redacted
                .redactions
                .into_iter()
                .map(|item| RedactionMarker::new(field, item.reason))
                .collect()
        }
        Value::Array(items) => {
            let mut redactions = Vec::new();
            for (index, item) in items.iter_mut().enumerate() {
                redactions.extend(redact_json_value(item, &format!("{field}[{index}]")));
            }
            redactions
        }
        Value::Object(map) => {
            let mut redactions = Vec::new();
            for (key, item) in map {
                redactions.extend(redact_json_value(item, &format!("{field}.{key}")));
            }
            redactions
        }
        _ => Vec::new(),
    }
}

fn is_private_key_path(value: &str) -> bool {
    let token = value.trim_matches(|ch: char| {
        ch == '\'' || ch == '"' || ch == ',' || ch == ';' || ch == ':'
    });
    let path_like = token.starts_with('/') || token.starts_with('~') || token.starts_with("./");
    path_like
        && (token.contains("/.ssh/id_")
            || token.ends_with("/id_rsa")
            || token.ends_with("/id_ed25519")
            || token.ends_with(".pem")
            || token.ends_with(".key")
            || token.contains("/private_key"))
}

fn redact_credentialed_url(value: &str) -> Option<String> {
    let scheme_end = value.find("://")?;
    let rest_start = scheme_end + 3;
    let at = value[rest_start..].find('@')? + rest_start;
    let slash = value[rest_start..]
        .find('/')
        .map(|offset| rest_start + offset)
        .unwrap_or(value.len());
    if at > slash || !value[rest_start..at].contains(':') {
        return None;
    }
    Some(format!(
        "{}[REDACTED]@{}",
        &value[..rest_start],
        &value[at + 1..]
    ))
}
```

- [ ] **Step 4: Add diagnostic redaction helper tests**

Add to `crates/conary-core/src/diagnostics/mod.rs` tests:

```rust
#[test]
fn diagnostic_evidence_can_store_redaction_markers() {
    let evidence = DiagnosticEvidence::log("command", "Bearer [REDACTED]")
        .with_metadata("source", serde_json::json!("scriptlet"))
        .with_redaction(RedactionMarker::new("log", "bearer-token"));
    assert_eq!(evidence.redactions[0].field, "log");
    assert_eq!(evidence.metadata["source"], "scriptlet");
}
```
This test covers the `with_metadata` and `with_redaction` helper methods added to `DiagnosticEvidence` in Task 1.

- [ ] **Step 5: Run redaction tests**

Run:

```bash
cargo test -p conary-core diagnostics
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/diagnostics/mod.rs crates/conary-core/src/diagnostics/redaction.rs
git commit -m "security(packaging): add diagnostic redaction policy"
```

---

### Task 3: Agent Evidence Redaction Boundary

**Files:**
- Modify: `crates/conary-agent-contract/src/result.rs`

- [ ] **Step 1: Write the additive schema test**

Add to `crates/conary-agent-contract/src/result.rs` tests:

```rust
#[test]
fn evidence_item_serializes_redaction_metadata() {
    let mut item = EvidenceItem {
        kind: EvidenceKind::Command,
        summary: "redacted command".to_string(),
        uri: None,
        path: None,
        id: None,
        command: Some(vec!["curl".to_string(), "Bearer [REDACTED]".to_string()]),
        exit_code: None,
        metadata: BTreeMap::new(),
        redactions: vec![EvidenceRedaction {
            field: "command[1]".to_string(),
            reason: "bearer-token".to_string(),
        }],
    };
    item.metadata.insert("source".to_string(), serde_json::json!("packaging"));

    let value = serde_json::to_value(&item).unwrap();
    assert_eq!(value["redactions"][0]["field"], "command[1]");
    assert_eq!(value["redactions"][0]["reason"], "bearer-token");
}
```

- [ ] **Step 2: Run the failing contract test**

Run:

```bash
cargo test -p conary-agent-contract evidence_item_serializes_redaction_metadata
```

Expected: fail because `EvidenceRedaction` and `EvidenceItem.redactions` do not exist.

- [ ] **Step 3: Implement additive evidence redactions**

Add this struct near `EvidenceItem`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceRedaction {
    pub field: String,
    pub reason: String,
}
```

Add this field to `EvidenceItem`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub redactions: Vec<EvidenceRedaction>,
```

Update every in-repo `EvidenceItem` literal by adding:

```rust
redactions: Vec::new(),
```

- [ ] **Step 4: Run contract and dependent tests**

Run:

```bash
cargo test -p conary-agent-contract
cargo test -p conary-test bootstrap
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-agent-contract/src/result.rs apps/conary-test/src
git commit -m "security(agent): carry evidence redaction metadata"
```

---

### Task 4: Packaging Operation Record Store

**Files:**
- Modify: `crates/conary-core/src/filesystem/durable.rs`
- Modify: `apps/conary/src/commands/operation_records.rs`

- [ ] **Step 1: Write operation-record tests**

Update the durable test-module import in `crates/conary-core/src/filesystem/durable.rs`:

```rust
use super::{sync_parent_directory, write_json_atomic, write_json_atomic_with_mode};
```

Add this test to the same test module:

```rust
#[test]
fn write_json_atomic_with_mode_uses_requested_mode() {
    use std::os::unix::fs::PermissionsExt;

    #[derive(serde::Serialize)]
    struct Fixture {
        name: &'static str,
    }

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("private.json");
    write_json_atomic_with_mode(&path, &Fixture { name: "private" }, 0o600).unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert!(!temp.path().join("private.json.tmp").exists());
}
```

Add these tests to the existing test module in `operation_records.rs`:

```rust
#[test]
fn packaging_operations_dir_uses_xdg_state_home() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = packaging_operations_dir_from_state_home(temp.path());
    assert_eq!(dir, temp.path().join("conary/packaging/operations"));
}

#[test]
fn write_packaging_record_uses_private_modes_and_prunes_old_records() {
    use std::os::unix::fs::PermissionsExt;

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Fixture {
        schema_version: u16,
        operation_id: String,
    }

    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().join("ops");

    for index in 0..55 {
        write_packaging_record_unchecked(
            &dir,
            &format!("cook-{index:02}"),
            &Fixture {
                schema_version: 1,
                operation_id: format!("cook-{index:02}"),
            },
        )
        .unwrap();
    }

    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    let latest = load_latest_packaging_record::<Fixture>(&dir).unwrap().unwrap();
    assert_eq!(latest.operation_id, "cook-54");
    let records = list_packaging_records(&dir).unwrap();
    assert_eq!(records.len(), 50);
    assert!(!dir.join("cook-00.json").exists());
    let mode = std::fs::metadata(dir.join("cook-54.json")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
```

- [ ] **Step 2: Run the failing operation-record tests**

Run:

```bash
cargo test -p conary-core filesystem::durable::tests::write_json_atomic_with_mode_uses_requested_mode
cargo test -p conary operation_records
```

Expected: fail because the packaging record helpers do not exist.

- [ ] **Step 3: Implement durable private-mode and record directory helpers**

In `crates/conary-core/src/filesystem/durable.rs`, extend the existing atomic helpers instead of duplicating the write/fsync/rename sequence in the CLI:

```rust
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
```

Add these helpers below `write_file_atomic`:

```rust
pub fn write_file_atomic_with_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path_for(path);
    {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(mode)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, path)?;
    sync_parent_directory(path)
}

pub fn write_json_atomic_with_mode<T: Serialize>(
    path: &Path,
    value: &T,
    mode: u32,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| Error::InternalError(format!("failed to serialize JSON: {error}")))?;
    write_file_atomic_with_mode(path, &bytes, mode)
}
```

In `apps/conary/src/commands/operation_records.rs`, add imports:

```rust
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
```

Add helpers below `bootstrap_operations_dir`. The writer is named `unchecked` because redaction happens in `commands::diagnostics::write_packaging_record_if_possible`; direct callers must not pass unredacted command output.

```rust
const PACKAGING_RECORD_RETENTION: usize = 50;

pub fn packaging_operations_dir_from_state_home(state_home: &Path) -> PathBuf {
    state_home.join("conary").join("packaging").join("operations")
}

pub fn default_packaging_operations_dir() -> Result<PathBuf> {
    if let Some(override_dir) = std::env::var_os("CONARY_PACKAGING_OPERATIONS_DIR") {
        return Ok(PathBuf::from(override_dir));
    }
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(packaging_operations_dir_from_state_home(Path::new(&state_home)));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(Path::new(&home)
            .join(".local")
            .join("state")
            .join("conary")
            .join("packaging")
            .join("operations"));
    }
    anyhow::bail!("cannot determine packaging operation record directory; set XDG_STATE_HOME or HOME")
}

pub(crate) fn write_packaging_record_unchecked<T: Serialize>(
    dir: &Path,
    operation_id: &str,
    value: &T,
) -> Result<PathBuf> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{operation_id}.json"));
    conary_core::filesystem::durable::write_json_atomic_with_mode(&path, value, 0o600)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    prune_packaging_records(dir, PACKAGING_RECORD_RETENTION)?;
    Ok(path)
}

pub fn list_packaging_records(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            records.push(path);
        }
    }
    records.sort();
    Ok(records)
}

pub fn load_latest_packaging_record<T: DeserializeOwned>(dir: &Path) -> Result<Option<T>> {
    let Some(path) = list_packaging_records(dir)?.pop() else {
        return Ok(None);
    };
    Ok(Some(load_json_record(&path)?))
}

fn prune_packaging_records(dir: &Path, keep: usize) -> Result<()> {
    let records = list_packaging_records(dir)?;
    if records.len() <= keep {
        return Ok(());
    }
    let remove_count = records.len() - keep;
    for path in records.into_iter().take(remove_count) {
        fs::remove_file(path)?;
    }
    Ok(())
}
```

- [ ] **Step 4: Run operation-record tests**

Run:

```bash
cargo test -p conary-core filesystem::durable::tests::write_json_atomic_with_mode_uses_requested_mode
cargo test -p conary operation_records
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/filesystem/durable.rs apps/conary/src/commands/operation_records.rs
git commit -m "feat(packaging): add operation record store"
```

---

### Task 5: CLI Packaging Diagnostics Renderer

**Files:**
- Create: `apps/conary/src/commands/diagnostics.rs`
- Modify: `apps/conary/src/commands/mod.rs`

- [ ] **Step 1: Write renderer tests**

Create `apps/conary/src/commands/diagnostics.rs` with tests first:

```rust
// apps/conary/src/commands/diagnostics.rs

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::diagnostics::{
        DiagnosticEvidence, PackagingArtifact, PackagingCommandOutput, PackagingDiagnostic,
        PackagingDiagnosticCode, PackagingEvent, PackagingPhase,
    };

    #[test]
    fn json_renderer_includes_schema_version() {
        let output = PackagingCommandOutput::failed(
            "cook-1",
            "conary cook",
            vec![PackagingDiagnostic::error(
                PackagingPhase::RecipeValidation,
                PackagingDiagnosticCode::RecipeValidationFailed,
                "Recipe validation failed",
            )],
        );

        let json = render_packaging_json(&output).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["diagnostics"][0]["code"], "recipe-validation-failed");
    }

    #[test]
    fn human_renderer_uses_same_diagnostic_values() {
        let diagnostic = PackagingDiagnostic::warning(
            PackagingPhase::RecipeValidation,
            PackagingDiagnosticCode::RecipeValidationWarning,
            "unused field",
        )
        .with_evidence(DiagnosticEvidence::log("validator", "ignored key"));
        let mut output = Vec::new();
        render_diagnostics_human(&[diagnostic], &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Warning: unused field"));
        assert!(text.contains("validator"));
    }

    #[test]
    fn json_renderer_redacts_nested_evidence_events_and_artifacts_before_serializing() {
        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::Build,
            PackagingDiagnosticCode::CookFailed,
            "build command failed",
        )
        .with_evidence(
            DiagnosticEvidence::log(
                "command",
                format!(
                    "API_TOKEN=sk-demo-secret curl -H 'Authorization: Bearer abc.def'\n{}",
                    "x".repeat(20_000)
                ),
            )
            .with_metadata(
                "fetch",
                serde_json::json!({
                    "uri": "https://user:pass@example.invalid/source.tar.gz",
                    "env": ["PRIVATE_KEY=/home/dev/.ssh/id_ed25519"]
                }),
            ),
        );
        let mut output = PackagingCommandOutput::failed("cook-1", "conary cook", vec![diagnostic.clone()]);
        output.summary = Some("Bearer summary.secret".to_string());
        output.artifacts.push(PackagingArtifact {
            path: "/home/dev/.ssh/id_rsa".to_string(),
            kind: Some("debug-key".to_string()),
        });
        let mut event = PackagingEvent::diagnostic("cook-1", 1, diagnostic);
        event.message = Some("Bearer event.secret".to_string());
        event.artifact = Some(PackagingArtifact {
            path: "/home/dev/private.key".to_string(),
            kind: Some("artifact".to_string()),
        });
        output.events.push(event);

        let json = render_packaging_json(&output).unwrap();
        assert!(!json.contains("sk-demo-secret"));
        assert!(!json.contains("abc.def"));
        assert!(!json.contains("event.secret"));
        assert!(!json.contains("summary.secret"));
        assert!(!json.contains("user:pass"));
        assert!(!json.contains("/home/dev/.ssh/id_rsa"));
        assert!(!json.contains("/home/dev/private.key"));
        assert!(json.contains("API_TOKEN=[REDACTED]"));
        assert!(json.contains("Bearer [REDACTED]"));
        assert!(json.contains("[TRUNCATED]"));
        assert!(json.contains("\"redactions\""));
    }

    #[test]
    fn write_packaging_record_if_possible_redacts_before_persisting() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();

        let temp = tempfile::TempDir::new().unwrap();
        let previous = std::env::var_os("CONARY_PACKAGING_OPERATIONS_DIR");
        unsafe { std::env::set_var("CONARY_PACKAGING_OPERATIONS_DIR", temp.path()) };

        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::Build,
            PackagingDiagnosticCode::CookFailed,
            "API_TOKEN=sk-message-secret failed",
        )
        .with_evidence(
            DiagnosticEvidence::log("command", "Bearer record.secret")
                .with_metadata("uri", serde_json::json!("https://user:pass@example.invalid/pkg.ccs")),
        );
        let output = PackagingCommandOutput::failed("cook-redact", "conary cook", vec![diagnostic]);

        write_packaging_record_if_possible(&output);

        match previous {
            Some(value) => unsafe { std::env::set_var("CONARY_PACKAGING_OPERATIONS_DIR", value) },
            None => unsafe { std::env::remove_var("CONARY_PACKAGING_OPERATIONS_DIR") },
        }

        let record_text = std::fs::read_to_string(temp.path().join("cook-redact.json")).unwrap();
        assert!(!record_text.contains("sk-message-secret"));
        assert!(!record_text.contains("record.secret"));
        assert!(!record_text.contains("user:pass"));
        assert!(record_text.contains("[REDACTED]"));
    }
}
```

- [ ] **Step 2: Run the failing renderer tests**

Run:

```bash
cargo test -p conary commands::diagnostics::tests
```

Expected: fail because the module is not wired in and functions do not exist.

- [ ] **Step 3: Implement the renderer**

Add this implementation above the tests:

```rust
use std::io::Write;

use anyhow::Result;
use conary_core::diagnostics::{
    DiagnosticEvidence, PackagingArtifact, PackagingCommandOutput, PackagingDiagnostic,
    PackagingSeverity,
};
use conary_core::diagnostics::redaction::{
    redact_command, redact_json_value, redact_log, redact_text,
};

pub(crate) fn render_packaging_json(output: &PackagingCommandOutput) -> Result<String> {
    Ok(serde_json::to_string_pretty(&redacted_packaging_output(output))?)
}

pub(crate) fn render_diagnostics_human(
    diagnostics: &[PackagingDiagnostic],
    output: &mut impl Write,
) -> Result<()> {
    for diagnostic in diagnostics {
        let label = match diagnostic.severity {
            PackagingSeverity::Info => "Info",
            PackagingSeverity::Warning => "Warning",
            PackagingSeverity::Error => "Error",
        };
        writeln!(output, "{label}: {}", diagnostic.message)?;
        for evidence in &diagnostic.evidence {
            writeln!(output, "  {}: {}", evidence.summary, evidence.log.as_deref().unwrap_or(""))?;
        }
    }
    Ok(())
}

pub(crate) fn write_packaging_output(
    output: &PackagingCommandOutput,
    json: bool,
    writer: &mut impl Write,
) -> Result<()> {
    let output = redacted_packaging_output(output);
    if json {
        writeln!(writer, "{}", serde_json::to_string_pretty(&output)?)?;
    } else {
        render_diagnostics_human(&output.diagnostics, writer)?;
    }
    Ok(())
}

pub(crate) fn write_packaging_record_if_possible(output: &PackagingCommandOutput) {
    let dir = match super::operation_records::default_packaging_operations_dir() {
        Ok(dir) => dir,
        Err(error) => {
            tracing::warn!(%error, "failed to resolve packaging operation record directory");
            return;
        }
    };
    let output = redacted_packaging_output(output);
    if let Err(error) = super::operation_records::write_packaging_record_unchecked(
        &dir,
        &output.operation_id,
        &output,
    ) {
        tracing::warn!(%error, "failed to write packaging operation record");
    }
}

pub(crate) fn redacted_packaging_output(output: &PackagingCommandOutput) -> PackagingCommandOutput {
    let mut output = output.clone();
    if let Some(summary) = &mut output.summary {
        let redacted = redact_text(summary);
        *summary = redacted.value;
    }
    for diagnostic in &mut output.diagnostics {
        redact_diagnostic(diagnostic);
    }
    for artifact in &mut output.artifacts {
        redact_artifact(artifact);
    }
    for event in &mut output.events {
        if let Some(message) = &mut event.message {
            let redacted = redact_text(message);
            *message = redacted.value;
        }
        if let Some(artifact) = &mut event.artifact {
            redact_artifact(artifact);
        }
        if let Some(diagnostic) = &mut event.diagnostic {
            redact_diagnostic(diagnostic);
        }
    }
    output
}

fn redact_diagnostic(diagnostic: &mut PackagingDiagnostic) {
    let message = redact_text(&diagnostic.message);
    diagnostic.message = message.value;
    diagnostic.redactions.extend(message.redactions);
    for evidence in &mut diagnostic.evidence {
        redact_evidence(evidence);
    }
}

fn redact_evidence(evidence: &mut DiagnosticEvidence) {
    let summary = redact_text(&evidence.summary);
    evidence.summary = summary.value;
    evidence.redactions.extend(summary.redactions);
    if let Some(path) = &mut evidence.path {
        let redacted = redact_text(path);
        *path = redacted.value;
        evidence.redactions.extend(redacted.redactions);
    }
    if let Some(uri) = &mut evidence.uri {
        let redacted = redact_text(uri);
        *uri = redacted.value;
        evidence.redactions.extend(redacted.redactions);
    }
    if let Some(log) = &mut evidence.log {
        let redacted = redact_log(log);
        *log = redacted.value;
        evidence.redactions.extend(redacted.redactions);
    }
    if let Some(command) = &mut evidence.command {
        let redacted = redact_command(command);
        *command = redacted.value;
        evidence.redactions.extend(redacted.redactions);
    }
    for (key, value) in &mut evidence.metadata {
        evidence
            .redactions
            .extend(redact_json_value(value, &format!("metadata.{key}")));
    }
}

fn redact_artifact(artifact: &mut PackagingArtifact) {
    let redacted = redact_text(&artifact.path);
    artifact.path = redacted.value;
}
```

Modify `apps/conary/src/commands/mod.rs`:

```rust
mod diagnostics;
```

- [ ] **Step 4: Run renderer tests**

Run:

```bash
cargo test -p conary commands::diagnostics::tests
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/diagnostics.rs apps/conary/src/commands/mod.rs
git commit -m "feat(packaging): add cli diagnostic renderer"
```

---

### Task 6: CLI Flags And Dispatch Wiring

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/commands/cook.rs`
- Modify: `apps/conary/src/commands/publish.rs`

- [ ] **Step 1: Write CLI parse tests**

Add to `apps/conary/src/cli/mod.rs` tests:

```rust
#[test]
fn cook_and_publish_accept_json_flag_but_try_does_not() {
    let cook = Cli::try_parse_from(["conary", "cook", ".", "--json"]).unwrap();
    match cook.command {
        Some(Commands::Cook { json, .. }) => assert!(json),
        other => panic!("unexpected command: {other:?}"),
    }

    let publish = Cli::try_parse_from(["conary", "publish", "dist/pkg.ccs", "./repo", "--json"]).unwrap();
    match publish.command {
        Some(Commands::Publish { json, .. }) => assert!(json),
        other => panic!("unexpected command: {other:?}"),
    }

    let try_json = Cli::try_parse_from(["conary", "try", "pkg.ccs", "--json"]);
    assert!(try_json.is_err(), "try --json is not part of M3a");
}
```

- [ ] **Step 2: Run the failing CLI test**

Run:

```bash
cargo test -p conary cli::tests::cook_and_publish_accept_json_flag_but_try_does_not
```

Expected: fail because `cook` and `publish` do not accept `--json`.

- [ ] **Step 3: Add `json` fields to CLI variants**

Modify the `Cook` variant in `apps/conary/src/cli/mod.rs`:

```rust
/// Emit structured M3a JSON output
#[arg(long)]
json: bool,
```

Modify the `Publish` variant:

```rust
/// Emit structured M3a JSON output
#[arg(long)]
json: bool,
```

Keep `Try` unchanged.

- [ ] **Step 4: Thread the flags through dispatch**

In `apps/conary/src/dispatch/root.rs`, include `json` in the `Commands::Cook` match and pass it to `cmd_cook`. Include `json` in the `Commands::Publish` match and set it on `PublishOptions`.

Update `apps/conary/src/commands/cook.rs` function signatures by adding a final `json: bool` argument to `cmd_cook`, and adding `json: bool` immediately before the `output` writer argument in `cmd_cook_with_output`. Existing tests should pass `false` until the JSON tests are added.

Update `apps/conary/src/commands/publish.rs`:

```rust
pub struct PublishOptions {
    pub what: String,
    pub target: Option<String>,
    pub recipe: Option<String>,
    pub key_dir: Option<String>,
    pub state_file: Option<String>,
    pub refresh: bool,
    pub force_reinit: bool,
    pub accept_destination_state: bool,
    pub rotate_publish_key: bool,
    pub rotate_root_key: bool,
    pub yes: bool,
    pub json: bool,
}
```

Update every `PublishOptions` literal with `json: false` in existing tests, then use `json: true` in new JSON tests.

- [ ] **Step 5: Run parse and existing command tests**

Run:

```bash
cargo test -p conary cli::tests::cook_and_publish_accept_json_flag_but_try_does_not
cargo test -p conary commands::cook::tests
cargo test -p conary commands::publish::tests
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/dispatch/root.rs apps/conary/src/commands/cook.rs apps/conary/src/commands/publish.rs
git commit -m "feat(packaging): add json flags for cook and publish"
```

---

### Task 7: Cook Structured Reports And JSON Output

**Files:**
- Modify: `apps/conary/src/commands/cook.rs`
- Modify: `apps/conary/src/commands/diagnostics.rs`

`apps/conary/src/commands/cook.rs` is already over 1000 lines. Preserve the ownership boundary by keeping it responsible for cook orchestration and command-specific report construction only; shared rendering, redaction, and operation-record persistence stay in `commands::diagnostics` and `commands::operation_records`. Do not mechanically split `cook.rs` in this slice unless a focused helper extraction is required to keep the JSON edge thin.

- [ ] **Step 1: Write cook JSON tests**

Add to `apps/conary/src/commands/cook.rs` tests:

```rust
#[tokio::test]
async fn cook_validate_only_json_has_schema_version_and_summary() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_local_recipe(&recipe_path);

    let mut output = Vec::new();
    cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        true,
        &mut output,
    )
    .await
    .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output).expect("valid cook json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["command"], "conary cook");
    assert_eq!(value["status"], "succeeded");
    assert_eq!(value["summary"], "Recipe validation passed");
    assert!(value["operation_id"].as_str().unwrap().starts_with("cook-"));
}

#[tokio::test]
async fn cook_json_conflict_error_is_single_structured_json() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_local_recipe(&recipe_path);

    let mut output = Vec::new();
    let error = cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        false,
        false,
        false,
        true,
        true,
        false,
        true,
        &mut output,
    )
    .await
    .unwrap_err();

    let rendered = String::from_utf8(output).unwrap();
    assert!(format!("{error:#}").contains("--no-isolation conflicts"));
    assert!(rendered.trim_start().starts_with('{'), "{rendered}");
    assert!(!rendered.contains("Reading recipe:"));
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid error json");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "cook-failed");
}
```

- [ ] **Step 2: Run the failing cook JSON tests**

Run:

```bash
cargo test -p conary cook_validate_only_json_has_schema_version_and_summary
cargo test -p conary cook_json_conflict_error_is_single_structured_json
```

Expected: fail because `cmd_cook_with_output` does not render JSON.

- [ ] **Step 3: Add cook report helpers**

In `cook.rs`, import:

```rust
use conary_core::diagnostics::{
    DiagnosticEvidence, PackagingArtifact, PackagingCommandOutput, PackagingDiagnostic,
    PackagingDiagnosticCode, PackagingEvent, PackagingEventKind, PackagingPhase,
    PACKAGING_JSON_SCHEMA_VERSION,
};
```

Add helper functions near `write_inference_trace`:

```rust
fn cook_operation_id() -> String {
    super::operation_records::new_operation_id("cook")
}

fn cook_failure_output(operation_id: &str, error: &anyhow::Error) -> PackagingCommandOutput {
    let code = cook_error_code(error);
    PackagingCommandOutput::failed(
        operation_id.to_string(),
        "conary cook",
        vec![PackagingDiagnostic::error(
            PackagingPhase::Build,
            code,
            error.to_string(),
        )],
    )
}

fn cook_success_output(operation_id: &str, summary: impl Into<String>) -> PackagingCommandOutput {
    let mut output = PackagingCommandOutput::succeeded(operation_id.to_string(), "conary cook");
    output.summary = Some(summary.into());
    output
}

fn push_cook_event(
    report: &mut PackagingCommandOutput,
    sequence: &mut u64,
    phase: PackagingPhase,
    kind: PackagingEventKind,
    message: impl Into<String>,
) {
    *sequence += 1;
    report.events.push(PackagingEvent {
        schema_version: PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: report.operation_id.clone(),
        sequence: *sequence,
        phase,
        kind,
        message: Some(message.into()),
        diagnostic: None,
        artifact: None,
        progress: None,
    });
}

fn cook_error_code(error: &anyhow::Error) -> PackagingDiagnosticCode {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("source cache") || message.contains("cache miss") {
        PackagingDiagnosticCode::SourceCacheMiss
    } else if message.contains("network") || message.contains("offline") {
        PackagingDiagnosticCode::BuildNetworkAccess
    } else if message.contains("unpinned") || message.contains("content lock") {
        PackagingDiagnosticCode::UnpinnedDependency
    } else if message.contains("command risk") || message.contains("risk report") {
        PackagingDiagnosticCode::CommandRiskEvidence
    } else {
        PackagingDiagnosticCode::CookFailed
    }
}
```

- [ ] **Step 4: Render JSON at the command edge**

Change `cmd_cook_with_output` so it creates `let operation_id = cook_operation_id();` before validation, runs the existing behavior in an inner async helper, and on `json == true` writes a `PackagingCommandOutput` instead of human progress.

Use this structure:

```rust
let operation_id = cook_operation_id();
let options = CookRunOptions {
    target,
    recipe,
    output_dir,
    source_cache,
    jobs,
    keep_builddir,
    validate_only,
    fetch_only,
    explain,
    isolated,
    no_isolation,
    hermetic,
    json,
    operation_id: operation_id.clone(),
};
let result = run_cook_operation(options, output).await;
match result {
    Ok(mut report) => {
        report.operation_id = operation_id.clone();
        if json {
            super::diagnostics::write_packaging_output(&report, true, output)?;
            write_packaging_record_if_possible(&report);
        }
        Ok(())
    }
    Err(error) => {
        if json {
            let report = cook_failure_output(&operation_id, &error);
            super::diagnostics::write_packaging_output(&report, true, output)?;
            write_packaging_record_if_possible(&report);
        }
        Err(error)
    }
}
```

Add the helper input type near `ResolvedCookInput`:

```rust
struct CookRunOptions<'a> {
    target: Option<&'a str>,
    recipe: Option<&'a str>,
    output_dir: &'a str,
    source_cache: &'a str,
    jobs: Option<u32>,
    keep_builddir: bool,
    validate_only: bool,
    fetch_only: bool,
    explain: bool,
    isolated: bool,
    no_isolation: bool,
    hermetic: bool,
    json: bool,
    operation_id: String,
}
```

The inner helper should return `Result<PackagingCommandOutput>` and should not write human lines when `json == true`. For `validate_only`, return:

```rust
let mut report = cook_success_output(&operation_id, "Recipe validation passed");
let mut sequence = 0;
push_cook_event(
    &mut report,
    &mut sequence,
    PackagingPhase::RecipeValidation,
    PackagingEventKind::PhaseStarted,
    "Recipe validation started",
);
for warning in warnings {
    report.diagnostics.push(PackagingDiagnostic::warning(
        PackagingPhase::RecipeValidation,
        PackagingDiagnosticCode::RecipeValidationWarning,
        warning.to_string(),
    ));
}
push_cook_event(
    &mut report,
    &mut sequence,
    PackagingPhase::RecipeValidation,
    PackagingEventKind::PhaseFinished,
    "Recipe validation finished",
);
return Ok(report);
```

For successful package builds, push the cooked artifact:

```rust
report.artifacts.push(PackagingArtifact {
    path: result.package_path.display().to_string(),
    kind: Some("ccs".to_string()),
});
```

For M3a, emit only top-level command and phase events (`OperationStarted`, `PhaseStarted`, `PhaseFinished`, `ArtifactCreated`, and `OperationFinished`) from the cook command edge. Do not emit `CommandStarted` or `CommandFailed` for Kitchen shell steps until the later Kitchen instrumentation slice can source those events from the build runner instead of inventing them after the fact.

- [ ] **Step 5: Write redacted operation records for cook**

Call `super::diagnostics::write_packaging_record_if_possible(&report)` for both JSON and human paths once the command has a structured output. The helper was added in Task 5, redacts before persistence, writes through `write_packaging_record_unchecked`, and logs record-write failures with `tracing::warn!` without turning them into cook failures. Add an `OperationRecordWriteFailed` diagnostic only in a later slice if record persistence becomes a user-visible status.

- [ ] **Step 6: Run cook tests**

Run:

```bash
cargo test -p conary cook_validate_only_json_has_schema_version_and_summary
cargo test -p conary cook_json_conflict_error_is_single_structured_json
cargo test -p conary commands::cook::tests
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add apps/conary/src/commands/cook.rs apps/conary/src/commands/diagnostics.rs
git commit -m "feat(packaging): render cook json diagnostics"
```

---

### Task 8: Publish Structured Reports And JSON Output

**Files:**
- Modify: `apps/conary/src/commands/publish.rs`
- Modify: `apps/conary/src/commands/diagnostics.rs`

- [ ] **Step 1: Write publish JSON tests**

Add to `apps/conary/src/commands/publish.rs` tests:

```rust
#[tokio::test]
async fn artifact_form_publish_json_reports_static_gate_failure() {
    let fixture = ArtifactPublishFixture::without_attestation();
    let mut options = fixture.options();
    options.json = true;

    let mut output = Vec::new();
    let error = cmd_publish_with_output(options, &mut output).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("artifact is missing a build attestation"), "{message}");
    let value: serde_json::Value = serde_json::from_slice(&output).expect("valid publish json");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "publish-gate-failed");
    assert_eq!(
        value["diagnostics"][0]["evidence"][0]["metadata"]["publish_lint_report"]["failures"][0]["code"],
        "missing-attestation"
    );
}

#[tokio::test]
async fn project_form_publish_json_preflight_is_structured() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let repo_dir = temp.path().join("repo");
    let key_dir = temp.path().join("keys");
    let state_file = temp.path().join("publish-state.toml");
    std::fs::write(
        &recipe_path,
        r#"
[package]
name = "publish-json"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/publish-json"
"#,
    )
    .unwrap();

    let mut output = Vec::new();
    let error = cmd_publish_with_output(
        PublishOptions {
            what: repo_dir.display().to_string(),
            target: None,
            recipe: Some(recipe_path.display().to_string()),
            key_dir: Some(key_dir.display().to_string()),
            state_file: Some(state_file.display().to_string()),
            refresh: false,
            force_reinit: false,
            accept_destination_state: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: true,
            json: true,
        },
        &mut output,
    )
    .await
    .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("hermetic config"), "{message}");
    let rendered = String::from_utf8(output).unwrap();
    assert!(!rendered.contains("Reading recipe:"));
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid preflight json");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "project-publish-preflight-failed");
}

#[tokio::test]
async fn remi_publish_json_is_structured_unsupported() {
    let temp = tempfile::tempdir().unwrap();
    let artifact = temp.path().join("artifact.ccs");
    std::fs::write(&artifact, b"not read for unsupported route").unwrap();

    let mut output = Vec::new();
    let error = cmd_publish_with_output(
        PublishOptions {
            what: artifact.display().to_string(),
            target: Some("https://remi.example.invalid/v1/admin/releases/test".to_string()),
            recipe: None,
            key_dir: None,
            state_file: None,
            refresh: false,
            force_reinit: false,
            accept_destination_state: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: true,
            json: true,
        },
        &mut output,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("Remi publish JSON output is not supported in M3a"));
    let value: serde_json::Value = serde_json::from_slice(&output).expect("valid remi unsupported json");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "publish-json-unsupported");
}
```

These unit tests prove the paths still fail and that JSON-mode publish writes structured output without human progress text. The integration test in Task 9 verifies stdout JSON shape through the real CLI binary.

- [ ] **Step 2: Run the failing publish tests**

Run:

```bash
cargo test -p conary artifact_form_publish_json_reports_static_gate_failure
cargo test -p conary project_form_publish_json_preflight_is_structured
cargo test -p conary remi_publish_json_is_structured_unsupported
```

Expected: fail until `PublishOptions` has `json` and publish writes structured reports.

- [ ] **Step 3: Add publish report helpers**

In `publish.rs`, import:

```rust
use conary_core::diagnostics::{
    DiagnosticEvidence, PackagingCommandOutput, PackagingDiagnostic,
    PackagingDiagnosticCode, PackagingPhase,
};
use conary_core::repository::static_repo::publish_gate::PublishLintReport;
```

Add helpers:

```rust
fn publish_operation_id() -> String {
    super::operation_records::new_operation_id("publish")
}

fn publish_failure_output(
    operation_id: &str,
    code: PackagingDiagnosticCode,
    message: impl Into<String>,
) -> PackagingCommandOutput {
    PackagingCommandOutput::failed(
        operation_id.to_string(),
        "conary publish",
        vec![PackagingDiagnostic::error(
            PackagingPhase::Publish,
            code,
            message,
        )],
    )
}

fn publish_gate_failure_output(
    operation_id: &str,
    report: &PublishLintReport,
) -> PackagingCommandOutput {
    let mut diagnostic = PackagingDiagnostic::error(
        PackagingPhase::Publish,
        PackagingDiagnosticCode::PublishGateFailed,
        "Static artifact publish gate failed",
    );
    let report_value = serde_json::to_value(report)
        .unwrap_or_else(|error| serde_json::json!({ "serialization_error": error.to_string() }));
    diagnostic.evidence.push(
        DiagnosticEvidence::log("publish-gate", "Static artifact publish gate failed")
            .with_metadata("publish_lint_report", report_value),
    );
    PackagingCommandOutput::failed(operation_id, "conary publish", vec![diagnostic])
}
```

- [ ] **Step 4: Add writer-aware publish entrypoint and Remi JSON rejection**

Add a writer-aware entrypoint and make `cmd_publish` delegate to it:

```rust
pub async fn cmd_publish(options: PublishOptions) -> Result<()> {
    let mut stdout = std::io::stdout();
    cmd_publish_with_output(options, &mut stdout).await
}

pub(crate) async fn cmd_publish_with_output(
    options: PublishOptions,
    writer: &mut impl std::io::Write,
) -> Result<()> {
    if let Some(target) = options.target.clone() {
        publish_artifact_form(options, &target, writer).await
    } else {
        publish_project_form(options, writer).await
    }
}
```

Change artifact-form dispatch to classify the target once and reject Remi JSON before resolving tokens or publishing:

```rust
async fn publish_artifact_form(
    options: PublishOptions,
    target: &str,
    writer: &mut impl std::io::Write,
) -> Result<()> {
    let operation_id = publish_operation_id();
    match classify_publish_target(target)? {
        PublishTargetRoute::StaticLocal => {
            publish_static_artifact_form(options, target, writer, operation_id).await
        }
        PublishTargetRoute::RemiRelease if options.json => {
            let message = "Remi publish JSON output is not supported in M3a";
            let output = publish_failure_output(
                &operation_id,
                PackagingDiagnosticCode::PublishJsonUnsupported,
                message,
            );
            super::diagnostics::write_packaging_output(&output, true, writer)?;
            super::diagnostics::write_packaging_record_if_possible(&output);
            bail!("{message}")
        }
        PublishTargetRoute::RemiRelease => publish_remi_artifact_form(options, target).await,
    }
}
```

- [ ] **Step 5: Render artifact-form gate JSON**

Change `publish_static_artifact_form` to accept the `operation_id` created by `publish_artifact_form`. When `report.is_passed()` is false and `options.json` is true:

```rust
let output = publish_gate_failure_output(&operation_id, &report);
super::diagnostics::write_packaging_output(&output, true, writer)?;
super::diagnostics::write_packaging_record_if_possible(&output);
bail!("{}", format_publish_gate_failures(&report));
```

For success, create a succeeded `PackagingCommandOutput` with summary `Published static artifact to repo` and one artifact path, render JSON when requested, and write the operation record. Do not label the artifact "attested" in the summary unless the rendered evidence explicitly includes the accepted attestation identity.

- [ ] **Step 6: Render project-form preflight JSON**

Change `publish_project_form` to take `writer: &mut impl std::io::Write`, create an outer `operation_id`, and move the current body into `run_project_form_publish`. The inner function should write human progress with `writeln!(writer, ...)` only when `options.json == false`. On any error before static repo publication when `options.json` is true, render:

```rust
let output = publish_failure_output(
    &operation_id,
    PackagingDiagnosticCode::ProjectPublishPreflightFailed,
    error.to_string(),
);
```

This preserves fail-closed behavior, adds a structured diagnostic for the current direct-bail path, and prevents preflight failures from writing `Reading recipe:` or warning lines before the JSON envelope.

- [ ] **Step 7: Run publish tests**

Run:

```bash
cargo test -p conary commands::publish::tests
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add apps/conary/src/commands/publish.rs apps/conary/src/commands/diagnostics.rs
git commit -m "feat(packaging): render publish json diagnostics"
```

---

### Task 9: End-To-End M3a CLI JSON And Operation Records

**Files:**
- Create: `apps/conary/tests/packaging_m3a.rs`

- [ ] **Step 1: Write the integration tests**

Create `apps/conary/tests/packaging_m3a.rs`:

```rust
// apps/conary/tests/packaging_m3a.rs

use std::path::Path;
use std::process::{Command, Output};

use conary_core::ccs::builder::write_ccs_package;
use conary_core::ccs::{CcsBuilder, CcsManifest, SigningKeyPair};

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn assert_failure(output: &Output) {
    assert!(!output.status.success(), "{}", output_text(output));
}

fn build_unsigned_ccs(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let source = temp.path().join("source");
    let package_path = temp.path().join("dist/missing-attestation.ccs");
    std::fs::create_dir_all(source.join("usr/share/m3a")).unwrap();
    std::fs::create_dir_all(package_path.parent().unwrap()).unwrap();
    std::fs::write(source.join("usr/share/m3a/payload"), "hello\n").unwrap();
    let manifest = CcsManifest::parse(
        r#"
[package]
name = "m3a-missing-attestation"
version = "1.0"
description = "missing attestation fixture"
license = "MIT"

[provenance]
origin_class = "native-built"
hardening_level = "hermetic"
"#,
    )
    .unwrap();
    let result = CcsBuilder::new(manifest, &source).build().unwrap();
    write_ccs_package(&result, &package_path).unwrap();
    package_path
}

fn write_publish_key_pair(key_dir: &Path) {
    std::fs::create_dir_all(key_dir).unwrap();
    let key = SigningKeyPair::generate().with_key_id("publish");
    key.save_to_files(
        &key_dir.join("publish.private"),
        &key_dir.join("publish.public"),
    )
    .unwrap();
}

#[test]
fn cook_validate_only_json_writes_redacted_operation_record() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let recipe = project.join("recipe.toml");
    let dist = temp.path().join("dist");
    let source_cache = temp.path().join("sources");
    let records = temp.path().join("records");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("payload.txt"), "hello\n").unwrap();
    std::fs::write(
        &recipe,
        r#"
[package]
name = "m3a-json"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/m3a-json && cp payload.txt %(destdir)s/usr/share/m3a-json/payload.txt"
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg(&recipe)
        .arg("--validate-only")
        .arg("--json")
        .arg("--output")
        .arg(&dist)
        .arg("--source-cache")
        .arg(&source_cache)
        .env("CONARY_PACKAGING_OPERATIONS_DIR", &records)
        .output()
        .expect("run conary cook --json");

    assert_success(&output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["command"], "conary cook");
    assert_eq!(value["status"], "succeeded");
    let operation_id = value["operation_id"].as_str().expect("operation id");
    let record_path = records.join(format!("{operation_id}.json"));
    assert!(record_path.is_file(), "missing record {}", record_path.display());
    let record_text = std::fs::read_to_string(record_path).unwrap();
    let record_value: serde_json::Value = serde_json::from_str(&record_text).unwrap();
    assert_eq!(record_value, value);
    assert!(!record_text.contains("sk-"));
}

#[test]
fn publish_artifact_form_json_reports_gate_failure_without_bypass() {
    let temp = tempfile::tempdir().unwrap();
    let artifact = build_unsigned_ccs(&temp);
    let repo = temp.path().join("repo");
    let keys = temp.path().join("keys");
    let state_file = temp.path().join("publish-state.toml");
    let records = temp.path().join("records");
    write_publish_key_pair(&keys);

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("publish")
        .arg(&artifact)
        .arg(&repo)
        .arg("--key-dir")
        .arg(&keys)
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .env("CONARY_PACKAGING_OPERATIONS_DIR", &records)
        .output()
        .expect("run conary publish --json");

    assert_failure(&output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "publish-gate-failed");
    assert_eq!(
        value["diagnostics"][0]["evidence"][0]["metadata"]["publish_lint_report"]["failures"][0]["code"],
        "missing-attestation"
    );
    let operation_id = value["operation_id"].as_str().expect("operation id");
    let record_path = records.join(format!("{operation_id}.json"));
    assert!(record_path.is_file(), "missing record {}", record_path.display());
    let record_text = std::fs::read_to_string(record_path).unwrap();
    let record_value: serde_json::Value = serde_json::from_str(&record_text).unwrap();
    assert_eq!(record_value["operation_id"], value["operation_id"]);
    assert_eq!(record_value["status"], value["status"]);
    assert_eq!(
        record_value["diagnostics"][0]["evidence"][0]["metadata"]["publish_lint_report"]["failures"][0]["code"],
        "missing-attestation"
    );
    assert!(!repo.exists(), "publish gate failure must not create repo");
}
```

- [ ] **Step 2: Run the failing integration tests**

Run:

```bash
cargo test -p conary --test packaging_m3a
```

Expected: fail until the command JSON paths and operation records are fully wired.

- [ ] **Step 3: Fix command output until the integration tests pass**

Use the failures to tighten Task 7 and Task 8 implementation. The expected behavior is:

- `cook --validate-only --json` emits only JSON on stdout.
- `publish <artifact> <repo> --json` emits JSON on stdout even on failure, and the process still exits non-zero.
- The artifact-form publish fixture has a valid `publish.private`/`publish.public` key pair so the test reaches the publish gate and fails on `missing-attestation`, not key setup.
- Operation records are written under `CONARY_PACKAGING_OPERATIONS_DIR` in tests.
- Record contents match stdout JSON for cook and carry the same operation id, status, and publish-gate metadata for publish failures. Records do not contain known secret patterns.

- [ ] **Step 4: Run the integration tests**

Run:

```bash
cargo test -p conary --test packaging_m3a
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/tests/packaging_m3a.rs apps/conary/src/commands/cook.rs apps/conary/src/commands/publish.rs apps/conary/src/commands/diagnostics.rs apps/conary/src/commands/operation_records.rs
git commit -m "test(packaging): cover m3a json operation records"
```

---

### Task 10: Publish Gate Diagnostic Mapping Exhaustiveness

**Files:**
- Modify: `apps/conary/src/commands/diagnostics.rs`
- Modify: `apps/conary/src/commands/publish.rs`

- [ ] **Step 1: Write the mapping test**

Add to `apps/conary/src/commands/diagnostics.rs` tests:

```rust
#[test]
fn every_publish_gate_failure_code_maps_to_packaging_diagnostic_code() {
    use conary_core::repository::static_repo::publish_gate::PublishGateFailureCode;

    // Keep this list in exact sync with the live PublishGateFailureCode enum.
    // If a new gate appears, this test should fail until its diagnostic mapping is explicit.
    let codes = [
        PublishGateFailureCode::MissingAttestation,
        PublishGateFailureCode::BuildAttestationSignatureMismatch,
        PublishGateFailureCode::PackageSignatureMismatch,
        PublishGateFailureCode::TomlIntegrityMismatch,
        PublishGateFailureCode::OutputIdentityMismatch,
        PublishGateFailureCode::UnacceptedSignerKey,
        PublishGateFailureCode::RetiredSignerKey,
        PublishGateFailureCode::AbsentOrUnknownProvenanceClass,
        PublishGateFailureCode::NonHermeticHardeningLevel,
        PublishGateFailureCode::StaleOrUnknownPolicy,
        PublishGateFailureCode::UncleanCommandRiskReport,
        PublishGateFailureCode::ForeignConversionMissingBoundary,
        PublishGateFailureCode::ForeignConversionBoundaryHashMismatch,
        PublishGateFailureCode::RecordedDraftArtifact,
    ];

    for code in codes {
        assert_eq!(
            publish_gate_code_to_diagnostic_code(code),
            PackagingDiagnosticCode::PublishGateFailed
        );
    }
}
```

- [ ] **Step 2: Run the failing mapping test**

Run:

```bash
cargo test -p conary every_publish_gate_failure_code_maps_to_packaging_diagnostic_code
```

Expected: fail because the mapping helper does not exist.

- [ ] **Step 3: Implement the mapping helper**

Add to `apps/conary/src/commands/diagnostics.rs`:

```rust
pub(crate) fn publish_gate_code_to_diagnostic_code(
    code: conary_core::repository::static_repo::publish_gate::PublishGateFailureCode,
) -> conary_core::diagnostics::PackagingDiagnosticCode {
    match code {
        conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::MissingAttestation
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::BuildAttestationSignatureMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::PackageSignatureMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::TomlIntegrityMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::OutputIdentityMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::UnacceptedSignerKey
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::RetiredSignerKey
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::AbsentOrUnknownProvenanceClass
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::NonHermeticHardeningLevel
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::StaleOrUnknownPolicy
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::UncleanCommandRiskReport
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::ForeignConversionMissingBoundary
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::ForeignConversionBoundaryHashMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::RecordedDraftArtifact => {
            conary_core::diagnostics::PackagingDiagnosticCode::PublishGateFailed
        }
    }
}
```

Using an exhaustive `match` makes future `PublishGateFailureCode` additions fail compile until the diagnostic mapping is updated.

- [ ] **Step 4: Use the mapping in publish diagnostics**

When converting each static gate failure to a diagnostic, call:

```rust
super::diagnostics::publish_gate_code_to_diagnostic_code(failure.code)
```

Keep the full `PublishLintReport` in evidence metadata or log evidence so JSON clients can distinguish individual gate codes.

- [ ] **Step 5: Run mapping and publish tests**

Run:

```bash
cargo test -p conary every_publish_gate_failure_code_maps_to_packaging_diagnostic_code
cargo test -p conary commands::publish::tests
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/diagnostics.rs apps/conary/src/commands/publish.rs
git commit -m "test(packaging): enforce publish gate diagnostic mapping"
```

---

### Task 11: Documentation And Final Verification

**Files:**
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`

- [ ] **Step 1: Update docs for the new look-here-first paths**

In `docs/modules/feature-ownership.md`, add M3a ownership notes:

```markdown
### M3a Packaging Diagnostics

Start with `crates/conary-core/src/diagnostics/` for the shared diagnostic,
event, redaction, and JSON schema contract. CLI rendering and operation-record
glue live in `apps/conary/src/commands/diagnostics.rs`; command-specific report
construction stays in `cook.rs` and `publish.rs`.
```

In `docs/llms/subsystem-map.md`, add the same look-here-first path for packaging diagnostics.

In `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`, update the M3 status line to say M3a implementation has started once code lands.

- [ ] **Step 2: Run focused tests**

Run:

```bash
cargo test -p conary-core diagnostics
cargo test -p conary-agent-contract
cargo test -p conary commands::diagnostics::tests
cargo test -p conary commands::operation_records::tests
cargo test -p conary commands::cook::tests
cargo test -p conary commands::publish::tests
cargo test -p conary --test packaging_m3a
```

Expected: all pass.

- [ ] **Step 3: Run repo gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
```

Expected: all pass.

- [ ] **Step 4: Commit docs and final polish**

```bash
git add docs/modules/feature-ownership.md docs/llms/subsystem-map.md docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md
git commit -m "docs(packaging): document m3a diagnostics ownership"
```

---

## Self-Review Checklist

- M3a first slice only: yes. MCP, watch, try-session decomposition, and record-mode tracing are excluded.
- Stable schema: `PACKAGING_JSON_SCHEMA_VERSION = 1` appears on command output, events, and operation records.
- Renderer parity: human and JSON rendering consume `PackagingDiagnostic` values.
- Redaction: command evidence, metadata, event messages, artifact paths, env-like secrets, bearer tokens, credentialed URLs, private-key paths, bounded logs, inference traces, and operation records use the same redactor.
- Operation records: file-backed only, no DB migration, private modes, newest-50 retention, and redaction-before-write.
- M2 gate preservation: publish failures remain fail-closed; JSON adds structure but does not turn failures into success.
- Publish JSON: static artifact/project-form paths emit one structured JSON object on success or failure; Remi `publish --json` returns a structured `publish-json-unsupported` diagnostic in M3a.
- Events: M3a emits top-level command/phase events only; Kitchen shell-step events are deferred until they can be sourced from the build runner.
- Try help remains honest: `try --json`, `try --watch`, and `try --record` are still unavailable.
