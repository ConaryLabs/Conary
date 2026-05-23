# LLM-Native Operations Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first LLM-native operations slice: a transport-neutral agent contract, stale AI/MCP surface cleanup, local developer bootstrap inspection, and stateless-MCP-ready resource/tool/prompt boundaries.

**Architecture:** `crates/conary-agent-contract` becomes the source of truth for agent operation semantics. `crates/conary-mcp` remains an MCP adapter, Remi and `conary-test` keep service-layer business logic, and `apps/conary-test` owns the first local bootstrap proof loop. Live MCP expansion stays blocked until the stateless MCP adapter decision is recorded.

**Tech Stack:** Rust 1.94, serde, schemars, rmcp, axum, clap, conary-test, Remi admin service, Codex `/goal`.

---

## Codex `/goal` Operating Model

This plan is tailored for Codex Goal mode as verified from official OpenAI docs on May 22, 2026:

- `/goal <objective>` sets a persistent objective attached to the active thread.
- `/goal` views the current goal.
- `/goal pause`, `/goal resume`, and `/goal clear` control the run.
- If `/goal` is not visible, enable it with:

```toml
[features]
goals = true
```

or run:

```bash
codex features enable goals
```

Goal text must be non-empty and at most 4,000 characters. Keep the detailed instructions in this file and point the goal at it.

Recommended goal text:

```text
Implement docs/superpowers/plans/2026-05-22-llm-native-operations-surface.md task-by-task. Use docs/superpowers/specs/2026-05-22-llm-native-operations-surface-design.md as source of truth. Keep milestone one contract-only plus inventory/prune/local bootstrap. Do not add live MCP registrations until the stateless adapter decision is settled. For each task: write tests first, make one focused commit, run verification, update checkboxes, and stop only when final acceptance criteria pass.
```

Goal-mode checkpoint rules:

- Keep each task as one commit unless a task explicitly says otherwise.
- After every commit, run `git status --short` and keep the worktree intentional.
- Use `/goal pause` before leaving a long-running operation unattended.
- Use `/goal resume` with the latest completed task, commit SHA, and next unchecked step.
- Use `/goal clear` only after the final acceptance criteria pass and the branch is either merged or intentionally left ready for review.

Official docs used for this section:

- <https://developers.openai.com/codex/prompting#goal-mode>
- <https://developers.openai.com/codex/cli/slash-commands#set-or-view-a-task-goal-with-goal>
- <https://developers.openai.com/codex/app/commands#set-or-manage-a-goal-with-goal>

## Source Specs And Constraints

- Primary spec: `docs/superpowers/specs/2026-05-22-llm-native-operations-surface-design.md`
- Repository contract: `AGENTS.md`
- Assistant map: `docs/llms/README.md`
- MCP/host notes: `docs/operations/infrastructure.md`
- Current MCP dependency fact: `Cargo.toml` has workspace requirement `rmcp = "1.1"` and `Cargo.lock` currently resolves `rmcp` to `1.6.0`; local source inspection shows that resolved SDK does not implement the target stateless MCP draft.

Non-negotiable constraints:

- Do not add new live session-based MCP resources, tools, prompts, or discovery behavior.
- Treat the MCP draft's current `DRAFT-2026-v1` protocol-version token as non-final until live adapter work verifies the released spec.
- Do not implement OpenAPI generation in this slice.
- Do not publish packages or fixtures from local bootstrap.
- Do not expose host-local secrets, bearer tokens, SSH identities, or ignored local access notes.
- Medium, high, and destructive mutations require plan-then-apply confirmation.

## File Structure

Create:

- `crates/conary-agent-contract/Cargo.toml`: transport-neutral contract crate manifest.
- `crates/conary-agent-contract/src/lib.rs`: public module exports and shared type re-exports.
- `crates/conary-agent-contract/src/result.rs`: result envelope, operation status, risk, confirmation, evidence, errors.
- `crates/conary-agent-contract/src/resource.rs`: canonical Conary agent resource URI helpers.
- `crates/conary-agent-contract/src/catalog.rs`: resource/tool/prompt catalog item types and cache policy types.
- `apps/conary-test/src/bootstrap.rs`: local developer bootstrap inspection service.
- `docs/operations/agent-mcp-adapter-decision.md`: current stateless MCP adapter decision record.

Modify:

- `Cargo.toml`: add `crates/conary-agent-contract` to workspace members.
- `crates/conary-mcp/Cargo.toml`: depend on `conary-agent-contract`.
- `crates/conary-mcp/src/lib.rs`: add contract JSON/schema helper functions without expanding live MCP registration.
- `apps/conary-test/Cargo.toml`: depend on `conary-agent-contract`.
- `apps/conary-test/src/lib.rs`: export `bootstrap`.
- `apps/conary-test/src/cli.rs`: add `conary-test bootstrap check --json`.
- `apps/conary/src/cli/automation.rs`: remove the experimental `automation ai` command family.
- `apps/conary/src/commands/automation.rs`: remove not-implemented AI handlers.
- `crates/conary-core/src/automation/mod.rs`: reword AI-assist claims as deferred config, or remove references if no current behavior uses them.
- `crates/conary-core/src/model/parser.rs`: classify or reword `AiAssistConfig` fields as deferred config.
- `apps/remi/src/server/mcp.rs`: classify high-risk mutation tools in descriptions and tests before changing behavior.
- `apps/conary-test/src/server/mcp.rs`: add catalog/context-budget tests and start moving read-only semantics toward resources/catalogs.
- Active docs listed in the spec inventory.

## Task 1: Create The Transport-Neutral Contract Crate

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/conary-agent-contract/Cargo.toml`
- Create: `crates/conary-agent-contract/src/lib.rs`
- Create: `crates/conary-agent-contract/src/result.rs`
- Create: `crates/conary-agent-contract/src/resource.rs`
- Create: `crates/conary-agent-contract/src/catalog.rs`

- [x] **Step 1: Add a failing workspace crate check**

Run:

```bash
cargo test -p conary-agent-contract
```

Expected: FAIL because the package does not exist.

- [x] **Step 2: Register the crate in the workspace**

In `Cargo.toml`, add the new crate beside the other crates:

```toml
members = [
    "apps/conary",
    "apps/remi",
    "apps/conaryd",
    "apps/conary-test",
    "crates/conary-bootstrap",
    "crates/conary-agent-contract",
    "crates/conary-mcp",
    "crates/conary-core",
]
```

- [x] **Step 3: Create the crate manifest**

Create `crates/conary-agent-contract/Cargo.toml`:

```toml
[package]
name = "conary-agent-contract"
version = "0.8.0"
edition = "2024"
rust-version = "1.94"
authors = ["Conary Contributors"]
description = "Transport-neutral agent operation contract for Conary"
license = "MIT OR Apache-2.0"

[dependencies]
schemars.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
```

- [x] **Step 4: Create module exports**

Create `crates/conary-agent-contract/src/lib.rs`:

```rust
// conary-agent-contract/src/lib.rs
//! Transport-neutral operation contract for Conary agent-facing workflows.

pub mod catalog;
pub mod resource;
pub mod result;

pub use catalog::*;
pub use resource::*;
pub use result::*;
```

- [x] **Step 5: Add resource URI helpers**

Create `crates/conary-agent-contract/src/resource.rs`:

```rust
// conary-agent-contract/src/resource.rs
//! Canonical Conary agent resource URI helpers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceRef {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ResourceRef {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            name: None,
        }
    }

    pub fn named(uri: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            name: Some(name.into()),
        }
    }
}

pub fn remi_health() -> ResourceRef {
    ResourceRef::new("conary://remi/health")
}

pub fn remi_repository(name: &str) -> ResourceRef {
    ResourceRef::named(format!("conary://remi/repositories/{name}"), name)
}

pub fn remi_federation_peer(peer_id: &str) -> ResourceRef {
    ResourceRef::named(format!("conary://remi/federation/peers/{peer_id}"), peer_id)
}

pub fn remi_audit_summary() -> ResourceRef {
    ResourceRef::new("conary://remi/audit/summary")
}

pub fn remi_chunk_stats() -> ResourceRef {
    ResourceRef::new("conary://remi/chunks/stats")
}

pub fn test_suite(suite_id: &str) -> ResourceRef {
    ResourceRef::named(format!("conary-test://suites/{suite_id}"), suite_id)
}

pub fn test_run(run_id: u64) -> ResourceRef {
    ResourceRef::named(format!("conary-test://runs/{run_id}"), run_id.to_string())
}

pub fn test_run_artifact(run_id: u64, artifact_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!("conary-test://runs/{run_id}/artifacts/{artifact_id}"),
        artifact_id,
    )
}

pub fn local_bootstrap_status() -> ResourceRef {
    ResourceRef::new("conary-local://bootstrap/status")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_helpers_emit_stable_uris() {
        assert_eq!(remi_health().uri, "conary://remi/health");
        assert_eq!(
            remi_repository("fedora44").uri,
            "conary://remi/repositories/fedora44"
        );
        assert_eq!(test_run(42).uri, "conary-test://runs/42");
        assert_eq!(
            test_run_artifact(42, "logs").uri,
            "conary-test://runs/42/artifacts/logs"
        );
        assert_eq!(
            local_bootstrap_status().uri,
            "conary-local://bootstrap/status"
        );
    }
}
```

- [x] **Step 6: Add operation result types**

Create `crates/conary-agent-contract/src/result.rs`:

```rust
// conary-agent-contract/src/result.rs
//! Shared result envelope for Conary agent-facing operations.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resource::ResourceRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Ok,
    Planned,
    Running,
    Unavailable,
    Failed,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    ReadOnly,
    Low,
    Medium,
    High,
    Destructive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorKind {
    MissingPrerequisite,
    NotSupported,
    Deferred,
    UnsafeWithoutConfirmation,
    RemoteUnavailable,
    ValidationFailed,
    PartialFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentError {
    pub kind: AgentErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Command,
    Resource,
    Artifact,
    Log,
    Check,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceItem {
    pub kind: EvidenceKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NextAction {
    pub label: String,
    pub description: String,
    pub risk: RiskLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub requires_confirmation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfirmationRequirement {
    pub plan_id: String,
    pub level: RiskLevel,
    pub reason: String,
    pub input_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OperationEnvelope {
    pub operation: String,
    pub status: OperationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<ResourceRef>,
    pub risk: RiskLevel,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<ResourceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_actions: Vec<NextAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<ConfirmationRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_logs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentError>,
}

impl OperationEnvelope {
    pub fn new(
        operation: impl Into<String>,
        status: OperationStatus,
        risk: RiskLevel,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            operation: operation.into(),
            status,
            subject: None,
            risk,
            summary: summary.into(),
            changed: Vec::new(),
            evidence: Vec::new(),
            warnings: Vec::new(),
            next_actions: Vec::new(),
            confirmation: None,
            raw_logs: None,
            error: None,
        }
    }
}

macro_rules! result_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        pub struct $name {
            #[serde(flatten)]
            pub envelope: OperationEnvelope,
            #[serde(default)]
            pub data: serde_json::Value,
        }

        impl $name {
            pub fn new(envelope: OperationEnvelope) -> Self {
                Self {
                    envelope,
                    data: serde_json::Value::Null,
                }
            }

            pub fn with_data(mut self, data: serde_json::Value) -> Self {
                self.data = data;
                self
            }
        }
    };
}

result_type!(InspectResult);
result_type!(PlanResult);
result_type!(VerifyResult);
result_type!(ApplyResult);
result_type!(ExplainResult);
result_type!(RecoverResult);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource;

    #[test]
    fn serializes_snake_case_status_and_risk() {
        let envelope = OperationEnvelope {
            subject: Some(resource::remi_health()),
            ..OperationEnvelope::new(
                "remi.health.inspect",
                OperationStatus::Ok,
                RiskLevel::ReadOnly,
                "Remi health is available",
            )
        };
        let value = serde_json::to_value(InspectResult::new(envelope)).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["risk"], "read_only");
        assert_eq!(value["subject"]["uri"], "conary://remi/health");
    }

    #[test]
    fn failed_result_uses_partial_failure_error_kind() {
        let mut envelope = OperationEnvelope::new(
            "conary-test.bootstrap.verify",
            OperationStatus::Partial,
            RiskLevel::ReadOnly,
            "Bootstrap check partially completed",
        );
        envelope.error = Some(AgentError {
            kind: AgentErrorKind::PartialFailure,
            message: "Container runtime is unavailable".to_string(),
            remediation: Some("Start Podman or Docker and rerun the check".to_string()),
        });
        let value = serde_json::to_value(VerifyResult::new(envelope)).unwrap();
        assert_eq!(value["status"], "partial");
        assert_eq!(value["error"]["kind"], "partial_failure");
    }

    #[test]
    fn confirmation_requirement_carries_plan_identity() {
        let confirmation = ConfirmationRequirement {
            plan_id: "plan-remi-audit-purge-1".to_string(),
            level: RiskLevel::Destructive,
            reason: "Purging audit rows removes evidence".to_string(),
            input_label: "Type the plan ID to confirm".to_string(),
            fingerprint: Some("audit-before-1700000000".to_string()),
            expires_at: None,
        };
        let value = serde_json::to_value(confirmation).unwrap();
        assert_eq!(value["level"], "destructive");
        assert_eq!(value["plan_id"], "plan-remi-audit-purge-1");
    }
}
```

- [x] **Step 7: Add catalog and cache-policy types**

Create `crates/conary-agent-contract/src/catalog.rs`:

```rust
// conary-agent-contract/src/catalog.rs
//! Catalog metadata for Conary agent-facing resources, tools, and prompts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::result::RiskLevel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope {
    Public,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CachePolicy {
    #[serde(rename = "ttlMs")]
    pub ttl_ms: u64,
    #[serde(rename = "cacheScope")]
    pub cache_scope: CacheScope,
}

impl CachePolicy {
    pub const fn private_short() -> Self {
        Self {
            ttl_ms: 30_000,
            cache_scope: CacheScope::Private,
        }
    }

    pub const fn private_static() -> Self {
        Self {
            ttl_ms: 300_000,
            cache_scope: CacheScope::Private,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogItem {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub risk: RiskLevel,
    pub cache: CachePolicy,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_policy_serializes_rc_field_names() {
        let value = serde_json::to_value(CachePolicy::private_short()).unwrap();
        assert_eq!(value["ttlMs"], 30_000);
        assert_eq!(value["cacheScope"], "private");
    }
}
```

- [x] **Step 8: Run contract tests**

Run:

```bash
cargo test -p conary-agent-contract
```

Expected: PASS.

- [x] **Step 9: Commit**

```bash
git add Cargo.toml crates/conary-agent-contract
git commit -m "feat(agent): add transport-neutral operation contract"
```

## Task 2: Teach The MCP Adapter About Contract Schemas

**Files:**
- Modify: `crates/conary-mcp/Cargo.toml`
- Modify: `crates/conary-mcp/src/lib.rs`

- [x] **Step 1: Write failing schema helper tests**

Add these tests to `crates/conary-mcp/src/lib.rs`:

```rust
    #[test]
    fn contract_json_text_serializes_contract_result() {
        let result = conary_agent_contract::InspectResult::new(
            conary_agent_contract::OperationEnvelope::new(
                "remi.health.inspect",
                conary_agent_contract::OperationStatus::Ok,
                conary_agent_contract::RiskLevel::ReadOnly,
                "Remi health inspected",
            ),
        );
        let text = contract_json_text(&result).expect("serialize contract result");
        assert!(text.contains("\"operation\": \"remi.health.inspect\""));
        assert!(text.contains("\"risk\": \"read_only\""));
    }

    #[test]
    fn output_schema_for_contract_result_mentions_operation() {
        let schema = output_schema_for::<conary_agent_contract::InspectResult>()
            .expect("schema should serialize");
        let text = serde_json::to_string(&schema).unwrap();
        assert!(text.contains("operation"));
        assert!(text.contains("status"));
    }
```

Run:

```bash
cargo test -p conary-mcp contract_result
```

Expected: FAIL because the helper functions and dependency do not exist yet.

- [x] **Step 2: Add dependencies**

In `crates/conary-mcp/Cargo.toml`, add:

```toml
conary-agent-contract = { path = "../conary-agent-contract" }
schemars.workspace = true
```

- [x] **Step 3: Add schema helpers**

In `crates/conary-mcp/src/lib.rs`, add these imports:

```rust
use schemars::{JsonSchema, schema_for};
```

Then add:

```rust
/// Serialize a transport-neutral Conary agent contract value to pretty JSON.
pub fn contract_json_text<T: serde::Serialize>(value: &T) -> Result<String, McpError> {
    to_json_text(value)
}

/// Return a JSON Schema value for an MCP `outputSchema`.
pub fn output_schema_for<T: JsonSchema>() -> Result<serde_json::Value, McpError> {
    serde_json::to_value(schema_for!(T))
        .map_err(|e| McpError::internal_error(format!("Schema serialization error: {e}"), None))
}
```

- [x] **Step 4: Run adapter tests**

```bash
cargo test -p conary-mcp
```

Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add crates/conary-mcp/Cargo.toml crates/conary-mcp/src/lib.rs
git commit -m "feat(mcp): expose contract schema helpers"
```

## Task 3: Record The Stateless MCP Adapter Decision

**Files:**
- Create: `docs/operations/agent-mcp-adapter-decision.md`
- Modify: `docs/superpowers/specs/2026-05-22-llm-native-operations-surface-design.md` only if the decision changes a spec assumption.

- [ ] **Step 1: Inspect current SDK state**

Run:

```bash
cargo tree -p conary-mcp -i rmcp
rg -n "LocalSessionManager|RoleServer|ServerHandler|server/discover|Mcp-Method|MCP-Protocol-Version|ttlMs|cacheScope" apps crates Cargo.toml Cargo.lock
```

Expected: output confirms current code still uses `RoleServer` / `ServerHandler` and local session managers. It should not show `server/discover`, `Mcp-Method`, `ttlMs`, or `cacheScope` in Conary code yet; those are target draft strings for the future stateless adapter.

- [ ] **Step 2: Create the decision record**

Create `docs/operations/agent-mcp-adapter-decision.md`:

```markdown
---
last_updated: 2026-05-22
revision: 1
summary: Decision record for Conary's first stateless MCP adapter path
---

# Agent MCP Adapter Decision

## Decision

The first LLM-native operations milestone remains contract-only plus inventory/prune. Conary will not add new live MCP resources, tools, prompts, or discovery behavior on the existing session-based `rmcp` path.

## Current State

- Workspace requirement: `rmcp = "1.1"` in `Cargo.toml`
- Resolved dependency: `rmcp 1.6.0` in `Cargo.lock`
- Current Remi and conary-test wiring uses `RoleServer`, `ServerHandler`, `StreamableHttpService`, and `LocalSessionManager`
- Current live MCP surfaces are tool-only from Conary's product perspective
- Local source inspection on 2026-05-22 shows `rmcp 1.6.0` does not implement the target stateless MCP draft; it still uses `initialize`, `Mcp-Session-Id`, and session-manager based Streamable HTTP code

## Target

Target the current MCP draft stateless direction associated with the 2026-07-28 release candidate. The draft docs currently use `DRAFT-2026-v1` as the protocol-version token; re-verify the final token before live adapter work.

## Adapter Gate

Before new live MCP registration work begins, implementation must prove one of these paths:

1. `rmcp` supports the target draft features needed by Conary.
2. A thin raw HTTP adapter can implement the target draft with tests for:
   - per-request `POST`
   - `Accept`
   - `MCP-Protocol-Version`
   - `Mcp-Method`
   - `Mcp-Name`
   - per-request `_meta`
   - `Origin` validation
   - `server/discover`
   - cache metadata on list/read responses

## Current Choice

Do not build new live MCP registrations in the first milestone. Build the contract crate, catalog metadata, local bootstrap inspection, and cleanup first.
```

- [ ] **Step 3: Verify docs**

```bash
rg -n "LocalSessionManager|DRAFT-2026-v1|server/discover|Mcp-Method|cache metadata" docs/operations/agent-mcp-adapter-decision.md
git diff --check
```

Expected: command prints the decision-record lines and no whitespace errors.

- [ ] **Step 4: Commit**

```bash
git add docs/operations/agent-mcp-adapter-decision.md
git commit -m "docs(agent): record stateless MCP adapter gate"
```

## Task 4: Remove Fake `conary automation ai` Commands

**Files:**
- Modify: `apps/conary/src/cli/automation.rs`
- Modify: `apps/conary/src/commands/automation.rs`
- Modify: tests in the same files or nearby CLI tests
- Modify: `crates/conary-core/src/automation/mod.rs`
- Modify: `crates/conary-core/src/model/parser.rs`

- [ ] **Step 1: Write CLI regression tests**

In `apps/conary/src/cli/automation.rs`, add tests under the existing `#[cfg(test)] mod tests`:

```rust
    #[cfg(feature = "experimental")]
    #[test]
    fn cli_rejects_removed_ai_subcommand_family() {
        let parsed = AutomationCli::try_parse_from(["automation", "ai", "find", "web server"]);
        assert!(
            parsed.is_err(),
            "automation ai should not remain as a visible not-implemented command"
        );
    }
```

Run:

```bash
cargo test -p conary --features experimental cli_rejects_removed_ai_subcommand_family
```

Expected before removal: FAIL because the command still parses.

- [ ] **Step 2: Remove the CLI command family**

In `apps/conary/src/cli/automation.rs`, remove:

```rust
    #[cfg(feature = "experimental")]
    #[command(subcommand)]
    Ai(AiCommands),
```

Remove the `AiCommands` enum.

- [ ] **Step 3: Remove not-implemented handlers**

In `apps/conary/src/commands/automation.rs`, remove these functions:

```rust
cmd_ai_find
cmd_ai_translate
cmd_ai_query
cmd_ai_explain
```

Then remove any match arms that call them. If removing the match arms exposes unused imports, delete those imports in the same commit.

- [ ] **Step 4: Reword dormant AI-assist configuration**

In `crates/conary-core/src/automation/mod.rs`, change the module docs from:

```rust
//! Automation system for self-healing, auto-updates, and AI-assisted operations.
```

to:

```rust
//! Automation system for self-healing and suggest-confirm maintenance flows.
//!
//! Some model/config fields reserve space for future assistant-driven behavior,
//! but this module does not execute LLM-backed operations today.
```

In `crates/conary-core/src/model/parser.rs`, reword the `AiAssistConfig` comments so they say the fields are deferred configuration, not active behavior.

- [ ] **Step 5: Run focused tests**

```bash
cargo test -p conary --features experimental automation
cargo test -p conary-core model::parser
rg -n "\\[NOT IMPLEMENTED\\]|AI-assisted operations|Use AI assistance" apps/conary/src crates/conary-core/src
```

Expected: tests pass, and the `rg` command does not find the removed fake CLI claims. If it finds intentionally deferred config wording, confirm the wording says deferred or future.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/cli/automation.rs apps/conary/src/commands/automation.rs crates/conary-core/src/automation/mod.rs crates/conary-core/src/model/parser.rs
git commit -m "fix(agent): remove fake automation ai commands"
```

## Task 5: Add Local Developer Bootstrap Inspection

**Files:**
- Modify: `apps/conary-test/Cargo.toml`
- Modify: `apps/conary-test/src/lib.rs`
- Create: `apps/conary-test/src/bootstrap.rs`
- Modify: `apps/conary-test/src/cli.rs`

- [ ] **Step 1: Add failing bootstrap service tests**

Create `apps/conary-test/src/bootstrap.rs` with tests first:

```rust
// conary-test/src/bootstrap.rs
//! Local developer bootstrap inspection for conary-test.

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn inspect_reports_missing_manifest_dir_without_success() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing-manifests");
        let report = inspect_with_paths(root.path(), &missing);

        assert_ne!(report.envelope.status, conary_agent_contract::OperationStatus::Ok);
        assert!(
            report
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("manifest directory"))
        );
    }

    #[test]
    fn inspect_uses_local_bootstrap_subject_uri() {
        let root = tempdir().unwrap();
        let manifests = root.path().join("manifests");
        std::fs::create_dir_all(&manifests).unwrap();

        let report = inspect_with_paths(root.path(), &manifests);
        assert_eq!(
            report.envelope.subject.unwrap().uri,
            "conary-local://bootstrap/status"
        );
    }
}
```

Run:

```bash
cargo test -p conary-test bootstrap
```

Expected: FAIL because `inspect_with_paths` and the dependency are not defined.

- [ ] **Step 2: Add the contract dependency**

In `apps/conary-test/Cargo.toml`, add:

```toml
conary-agent-contract = { path = "../../crates/conary-agent-contract" }
```

- [ ] **Step 3: Implement bootstrap inspection**

Replace `apps/conary-test/src/bootstrap.rs` with:

```rust
// conary-test/src/bootstrap.rs
//! Local developer bootstrap inspection for conary-test.

use std::path::Path;

use conary_agent_contract::{
    EvidenceItem, EvidenceKind, InspectResult, OperationEnvelope, OperationStatus, RiskLevel,
    local_bootstrap_status,
};

pub fn inspect_default() -> InspectResult {
    // conary-test already owns repository-root discovery through paths::project_dir.
    let root = crate::paths::project_dir().unwrap_or_else(|_| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });
    let manifests = std::env::var_os("CONARY_TEST_MANIFESTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("apps/conary/tests/integration/remi/manifests"));
    inspect_with_paths(&root, &manifests)
}

pub fn inspect_with_paths(root: &Path, manifest_dir: &Path) -> InspectResult {
    let mut envelope = OperationEnvelope::new(
        "conary-test.bootstrap.inspect",
        OperationStatus::Ok,
        RiskLevel::ReadOnly,
        "Local Conary developer bootstrap prerequisites inspected",
    );
    envelope.subject = Some(local_bootstrap_status());

    let cargo_ok = command_available("cargo");
    let podman_ok = command_available("podman");
    let docker_ok = command_available("docker");
    let qemu_ok = command_available("qemu-system-x86_64");
    let kvm_ok = Path::new("/dev/kvm").exists();

    envelope.evidence.push(EvidenceItem {
        kind: EvidenceKind::Check,
        summary: format!("cargo available: {cargo_ok}"),
        uri: None,
        path: None,
        id: Some("cargo".to_string()),
        command: Some(vec!["cargo".to_string(), "--version".to_string()]),
        exit_code: None,
        metadata: Default::default(),
    });

    if !cargo_ok {
        envelope.status = OperationStatus::Unavailable;
        envelope
            .warnings
            .push("cargo is required for local Conary development".to_string());
    }

    if !manifest_dir.is_dir() {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "manifest directory is missing: {}",
            manifest_dir.display()
        ));
    }

    if !podman_ok && !docker_ok {
        envelope.status = OperationStatus::Partial;
        envelope.warnings.push(
            "Podman or Docker is required before container smoke validation can run".to_string(),
        );
    }

    if !qemu_ok || !kvm_ok {
        envelope.warnings.push(
            "QEMU/KVM is unavailable; non-QEMU bootstrap checks can still run".to_string(),
        );
    }

    let data = serde_json::json!({
        "project_root": root.display().to_string(),
        "manifest_dir": manifest_dir.display().to_string(),
        "required": {
            "cargo": cargo_ok,
            "container_runtime": podman_ok || docker_ok,
            "manifest_dir": manifest_dir.is_dir()
        },
        "optional": {
            "podman": podman_ok,
            "docker": docker_ok,
            "qemu_system_x86_64": qemu_ok,
            "dev_kvm": kvm_ok
        },
        "default_smoke_candidate": {
            "suite": "phase1-core",
            "distro": "fedora44",
            "requires_container_runtime": true,
            "requires_qemu": false
        }
    });

    InspectResult::new(envelope).with_data(data)
}

fn command_available(command: &str) -> bool {
    std::process::Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn inspect_reports_missing_manifest_dir_without_success() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing-manifests");
        let report = inspect_with_paths(root.path(), &missing);

        assert_ne!(report.envelope.status, OperationStatus::Ok);
        assert!(
            report
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("manifest directory"))
        );
    }

    #[test]
    fn inspect_uses_local_bootstrap_subject_uri() {
        let root = tempdir().unwrap();
        let manifests = root.path().join("manifests");
        std::fs::create_dir_all(&manifests).unwrap();

        let report = inspect_with_paths(root.path(), &manifests);
        assert_eq!(
            report.envelope.subject.unwrap().uri,
            "conary-local://bootstrap/status"
        );
    }
}
```

- [ ] **Step 4: Export the module**

In `apps/conary-test/src/lib.rs`, add:

```rust
pub mod bootstrap;
```

- [ ] **Step 5: Add CLI command**

In `apps/conary-test/src/cli.rs`, add a command variant:

```rust
    /// Inspect local developer bootstrap prerequisites
    Bootstrap {
        #[command(subcommand)]
        command: BootstrapCommands,
    },
```

Add the subcommand enum:

```rust
#[derive(Subcommand)]
enum BootstrapCommands {
    /// Check local prerequisites and emit structured bootstrap status
    Check,
}
```

In the command dispatch, add:

```rust
        Commands::Bootstrap {
            command: BootstrapCommands::Check,
        } => {
            let report = conary_test::bootstrap::inspect_default();
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("{}", report.envelope.summary);
                for warning in &report.envelope.warnings {
                    println!("warning: {warning}");
                }
            }
            Ok(())
        }
```

- [ ] **Step 6: Run focused tests and smoke command**

```bash
cargo test -p conary-test bootstrap
cargo run -p conary-test -- bootstrap check --json
cargo run -p conary-test -- list
```

Expected: tests pass, bootstrap emits JSON with `conary-local://bootstrap/status`, and suite inventory lists manifests.

- [ ] **Step 7: Commit**

```bash
git add apps/conary-test/Cargo.toml apps/conary-test/src/bootstrap.rs apps/conary-test/src/lib.rs apps/conary-test/src/cli.rs
git commit -m "feat(test): add local bootstrap inspection"
```

## Task 6: Add Contract Catalogs For Read-Heavy Resources

**Files:**
- Modify: `crates/conary-agent-contract/src/catalog.rs`
- Modify: `apps/remi/src/server/mcp.rs`
- Modify: `apps/conary-test/src/server/mcp.rs`

- [ ] **Step 1: Add catalog tests**

In `crates/conary-agent-contract/src/catalog.rs`, add:

```rust
pub fn default_read_resources() -> Vec<CatalogItem> {
    vec![
        CatalogItem {
            name: "remi.health".to_string(),
            description: "Read Remi service health".to_string(),
            when_to_use: "Use before Remi admin or package-service operations".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary-test.bootstrap.status".to_string(),
            description: "Read local developer bootstrap status".to_string(),
            when_to_use: "Use before running local smoke validation".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
    ]
}
```

Add this test:

```rust
    #[test]
    fn default_resources_are_read_only_and_explain_when_to_use() {
        let resources = default_read_resources();
        assert!(resources.iter().all(|item| item.risk == RiskLevel::ReadOnly));
        assert!(resources.iter().all(|item| !item.when_to_use.is_empty()));
        assert!(resources.iter().all(|item| item.cache.ttl_ms > 0));
    }
```

Run:

```bash
cargo test -p conary-agent-contract default_resources_are_read_only_and_explain_when_to_use
```

Expected: PASS after imports are adjusted.

- [ ] **Step 2: Add MCP catalog guard tests**

In `apps/remi/src/server/mcp.rs`, add or update a test that reports the current tool count and enforces a catalog decision:

```rust
    #[test]
    fn mcp_tool_catalog_requires_context_budget_decision() {
        let tools = RemiMcpServer::tool_router().list_all();
        assert!(
            tools.len() <= 15,
            "Remi has {} MCP tools; the agent plan must split the surface or document progressive discovery before adding more",
            tools.len()
        );
    }
```

If this fails because the current surface already exceeds the guardrail, do not mark the test ignored and do not pin an exact magic count. Replace it with a test that records context-budget debt while still preventing unreviewed growth:

```rust
    #[test]
    fn mcp_tool_catalog_records_context_budget_debt() {
        let tools = RemiMcpServer::tool_router().list_all();
        assert!(
            tools.len() <= 20,
            "Remi has {} MCP tools; split read-only/admin/mutation surfaces or document progressive discovery before adding more",
            tools.len()
        );
    }
```

Use the same pattern in `apps/conary-test/src/server/mcp.rs` with a budget that allows the current surface but blocks unreviewed growth.

- [ ] **Step 3: Run focused MCP tests**

```bash
cargo test -p remi mcp_tool_catalog
cargo test -p conary-test mcp_tool_catalog
```

Expected: tests pass and document current catalog debt without adding new live MCP registrations.

- [ ] **Step 4: Commit**

```bash
git add crates/conary-agent-contract/src/catalog.rs apps/remi/src/server/mcp.rs apps/conary-test/src/server/mcp.rs
git commit -m "test(agent): record MCP catalog budget debt"
```

## Task 7: Classify High-Risk Mutation Tools

**Files:**
- Modify: `apps/remi/src/server/mcp.rs`
- Modify: `apps/conary-test/src/server/mcp.rs`
- Modify: `docs/operations/infrastructure.md` if operational guidance changes

- [ ] **Step 1: Add Remi risk classification tests**

In `apps/remi/src/server/mcp.rs`, add a test that searches tool names/descriptions for high-risk operations:

```rust
    #[test]
    fn high_risk_tools_are_named_for_confirmation_review() {
        let tools = RemiMcpServer::tool_router().list_all();
        let names: Vec<_> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert!(names.iter().any(|name| name.contains("token")));
        assert!(names.iter().any(|name| name.contains("audit")));
    }
```

Run:

```bash
cargo test -p remi high_risk_tools_are_named_for_confirmation_review
```

Expected: PASS if current tool names are discoverable; otherwise adjust the test to the actual tool-router return type and keep the same assertion.

- [ ] **Step 2: Add risk wording to high-risk tool descriptions**

For Remi token creation, token deletion, and audit purge tools, add description text in the tool annotation or handler doc comment:

```text
Risk: high/destructive. Requires plan-then-apply confirmation in the LLM-native operations contract before this tool remains exposed in the stateless MCP mutation surface.
```

Do not change runtime behavior in this task unless the existing tool can be hidden without breaking tests.

- [ ] **Step 3: Classify conary-test deploy/restart/fixture tools**

In `apps/conary-test/src/server/mcp.rs`, add equivalent risk wording to deploy, rebuild, restart, fixture publish, image prune, and cleanup tools.

- [ ] **Step 4: Run focused tests**

```bash
cargo test -p remi mcp
cargo test -p conary-test mcp
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/remi/src/server/mcp.rs apps/conary-test/src/server/mcp.rs docs/operations/infrastructure.md
git commit -m "docs(agent): classify high-risk MCP mutations"
```

## Task 8: Add Hybrid Prompt Catalogs Without Live MCP Expansion

**Files:**
- Modify: `crates/conary-agent-contract/src/catalog.rs`
- Modify: `docs/operations/infrastructure.md`

- [ ] **Step 1: Add prompt catalog types**

In `crates/conary-agent-contract/src/catalog.rs`, add:

```rust
// These are catalog definitions only. Do not register them as live MCP prompts
// until the stateless MCP adapter decision is satisfied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptCatalogItem {
    pub name: String,
    pub description: String,
    pub deterministic_inputs: Vec<String>,
    pub expected_result: String,
    pub cache: CachePolicy,
}

pub fn first_slice_prompts() -> Vec<PromptCatalogItem> {
    vec![
        PromptCatalogItem {
            name: "inspect_remi_health".to_string(),
            description: "Inspect Remi health before admin or package-service work".to_string(),
            deterministic_inputs: vec!["conary://remi/health".to_string()],
            expected_result: "InspectResult".to_string(),
            cache: CachePolicy::private_short(),
        },
        PromptCatalogItem {
            name: "debug_failing_test".to_string(),
            description: "Collect run, artifact, and log evidence for a failing conary-test run".to_string(),
            deterministic_inputs: vec![
                "conary-test://runs/{run_id}".to_string(),
                "conary-test://runs/{run_id}/artifacts/{artifact_id}".to_string(),
            ],
            expected_result: "ExplainResult".to_string(),
            cache: CachePolicy::private_short(),
        },
        PromptCatalogItem {
            name: "bootstrap_local_dev_environment".to_string(),
            description: "Inspect local prerequisites and propose the next bootstrap proof step".to_string(),
            deterministic_inputs: vec!["conary-local://bootstrap/status".to_string()],
            expected_result: "PlanResult".to_string(),
            cache: CachePolicy::private_short(),
        },
    ]
}
```

Add test:

```rust
    #[test]
    fn first_slice_prompt_catalog_is_limited_to_three_prompts() {
        let prompts = first_slice_prompts();
        assert_eq!(prompts.len(), 3);
        assert!(prompts.iter().all(|prompt| !prompt.deterministic_inputs.is_empty()));
    }
```

- [ ] **Step 2: Run prompt catalog tests**

```bash
cargo test -p conary-agent-contract first_slice_prompt_catalog_is_limited_to_three_prompts
```

Expected: PASS.

- [ ] **Step 3: Document non-registration boundary**

In `docs/operations/infrastructure.md`, add one sentence under `Agent Operations And MCP`:

```markdown
The first LLM-native operations milestone may define prompt catalogs in `conary-agent-contract`, but it must not register new live MCP prompts until the stateless MCP adapter decision is satisfied.
```

- [ ] **Step 4: Commit**

```bash
git add crates/conary-agent-contract/src/catalog.rs docs/operations/infrastructure.md
git commit -m "feat(agent): define first prompt catalog"
```

## Task 9: Update Product And Assistant Docs

**Files:**
- Modify: `README.md`
- Modify: `docs/llms/README.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `apps/conary-test/README.md`

- [ ] **Step 1: Add the new contract crate to docs**

Update workspace/module listings so they mention:

```text
crates/conary-agent-contract/ transport-neutral agent operation contract
crates/conary-mcp/            shared MCP adapter helpers
```

- [ ] **Step 2: Document bootstrap command**

In `apps/conary-test/README.md` and `docs/INTEGRATION-TESTING.md`, add:

```bash
cargo run -p conary-test -- bootstrap check --json
```

Describe it as a local developer prerequisite and smoke-readiness inspection command.

- [ ] **Step 3: Sweep stale wording**

Run:

```bash
rg -n "MCP-first|transport-agnostic MCP|23 tools|Every MCP tool|\\[NOT IMPLEMENTED\\]|AI-assisted operations|conary automation ai" README.md docs apps crates
```

Expected: no active-doc hits that describe stale behavior as current. Code hits are acceptable only when they are tests asserting removal or deferred config comments.

- [ ] **Step 4: Verify docs**

```bash
git diff --check
```

Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add README.md docs/llms/README.md docs/llms/subsystem-map.md docs/ARCHITECTURE.md docs/operations/infrastructure.md docs/conaryopedia-v2.md docs/INTEGRATION-TESTING.md apps/conary-test/README.md
git commit -m "docs(agent): document operation contract slice"
```

## Task 10: Final Verification And Review

**Files:**
- All files touched by prior tasks.

- [ ] **Step 1: Run full formatting check**

```bash
cargo fmt --check
```

Expected: PASS.

- [ ] **Step 2: Run focused package tests**

```bash
cargo test -p conary-agent-contract
cargo test -p conary-mcp
cargo test -p conary --features experimental automation
cargo test -p conary-core model::parser
cargo test -p conary-test bootstrap
cargo test -p conary-test mcp
cargo test -p remi mcp
cargo run -p conary-test -- list
```

Expected: all commands pass.

- [ ] **Step 3: Run workspace lint**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 4: Run final stale-surface scan**

```bash
rg -n "MCP-first|transport-agnostic MCP|23 tools|Every MCP tool|\\[NOT IMPLEMENTED\\]|conary automation ai" README.md docs apps crates
```

Expected: no active stale claims. Any remaining hits must be intentional tests or historical archive files.

- [ ] **Step 5: Inspect final diff and status**

```bash
git status --short
git log --oneline -10
```

Expected: worktree is clean after the final commit, and the last commits match the task sequence.

- [ ] **Step 6: Request review**

Use `/review` or dispatch a code-review subagent with this scope:

```text
Review the LLM-native operations first slice. Check the new conary-agent-contract crate, MCP adapter helpers, removed automation ai commands, bootstrap inspection path, catalog/risk tests, and docs. Focus on false current-state claims, MCP session-model regressions, missing confirmation boundaries, and test gaps.
```

Address Critical and Important findings before marking the goal complete.

## Final Acceptance Criteria

- `crates/conary-agent-contract` exists, compiles, has no MCP dependency, and defines result envelopes, risk labels, confirmation, evidence, resource refs, cache policy, and prompt/resource catalog types.
- `crates/conary-mcp` remains MCP-specific and can produce contract JSON/schema values.
- No new live session-based MCP resources, tools, prompts, or discovery behavior were added.
- The raw/stateless MCP adapter decision is recorded in `docs/operations/agent-mcp-adapter-decision.md`.
- Fake `conary automation ai ...` commands are removed, and active docs no longer present them as implemented.
- `conary-test bootstrap check --json` emits structured local developer bootstrap status without publishing packages or requiring cloud credentials.
- High-risk Remi and conary-test mutation tools are classified for confirmation or removal in the stateless MCP mutation surface.
- First-slice prompt catalog is limited to `inspect_remi_health`, `debug_failing_test`, and `bootstrap_local_dev_environment`.
- Active docs distinguish the transport-neutral contract from MCP adapter code.
- Final verification commands in Task 10 pass.
