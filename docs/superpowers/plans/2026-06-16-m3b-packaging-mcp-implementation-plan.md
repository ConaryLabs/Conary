# M3b Packaging MCP Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the local packaging MCP surface over M3a diagnostics and operation records, including read/diagnostic tools plus static artifact publish plan/apply as the first mutation contract.

**Architecture:** Transport-neutral resource/catalog additions live in `conary-agent-contract`, generic MCP response helpers live in `conary-mcp`, and the Conary CLI owns a local stdio `conary mcp packaging` server. Packaging semantics stay in `apps/conary`: a focused `commands/packaging_mcp/` module handles tools, projection, plan registry, and read-only inspection; `publish.rs` remains the owner of static artifact publish routing, gates, and repository writes through a narrow service helper.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, `schemars`, `rmcp` server macros plus stdio transport, `tokio`, `uuid`, `tempfile`, `conary-agent-contract`, `conary-mcp`, `conary-core::ccs::attestation::canonical_json_hash`, existing M3a packaging diagnostics, and existing static repository publish gates.

---

## Scope Locks

M3b includes:

- `conary mcp packaging` as a local stdio MCP server command.
- Tool names:
  - `conary.packaging.inspect_project`
  - `conary.packaging.explain_inference`
  - `conary.packaging.diagnose_latest_failure`
  - `conary.packaging.operation_records.list`
  - `conary.packaging.operation_records.read`
  - `conary.packaging.publish.plan`
  - `conary.packaging.publish.apply`
- Stable `conary-packaging://` resource helpers.
- Catalog entries for the new resources and tools.
- One projection from `PackagingCommandOutput` into `OperationEnvelope` results.
- Operation-record read-by-id, recent-list, event extraction, and newest-failed helpers.
- A read-only static destination trust snapshot helper in `conary-core`.
- Static artifact-form publish planning that binds artifact identity, route, selected options, destination trust state, accepted signer set, policy digest, and expiry into `PublishPlanMaterial`.
- Static artifact-form publish apply that requires exact plan confirmation, rechecks fingerprint and trust state, stages artifact bytes in private storage, reruns gates, and calls a service-safe publish helper in `publish.rs`.

M3b excludes:

- Public TCP or unauthenticated HTTP MCP listeners.
- Arbitrary command execution.
- Shell command strings, environment maps, bearer tokens, or private key contents as tool inputs.
- Remi publish apply.
- Project-form publish apply.
- Key rotation, root reinitialization, destination-state downgrade acceptance, or `force_reinit` through MCP.
- Any publish gate bypass.

## File Structure

Create:

- `apps/conary/src/commands/packaging_mcp/mod.rs`: module exports and `cmd_mcp_packaging`.
- `apps/conary/src/commands/packaging_mcp/server.rs`: `PackagingMcpServer`, rmcp `ServerHandler`, tool registration, and MCP-to-service request routing.
- `apps/conary/src/commands/packaging_mcp/types.rs`: tool input DTOs, result data DTOs, plan material DTOs, and path/target normalized forms used by the service.
- `apps/conary/src/commands/packaging_mcp/service.rs`: `PackagingAgentService` read/diagnostic methods and high-level publish plan/apply orchestration.
- `apps/conary/src/commands/packaging_mcp/projection.rs`: M3a `PackagingCommandOutput` to agent `OperationEnvelope` projection.
- `apps/conary/src/commands/packaging_mcp/records.rs`: operation-record list/read/newest-failed wrappers over `operation_records.rs`.
- `apps/conary/src/commands/packaging_mcp/publish_plan.rs`: static publish plan material construction, in-memory registry, expiry, trust-state comparison, and private artifact staging.
- `apps/conary/tests/packaging_m3b.rs`: end-to-end local service and command-surface regression tests.
- `crates/conary-mcp/src/tools.rs`: small reusable helper for contract-shaped JSON tool results.

Modify:

- `crates/conary-agent-contract/src/resource.rs`: add `conary-packaging://` URI helpers.
- `crates/conary-agent-contract/src/catalog.rs`: add packaging resource and tool catalog entries.
- `crates/conary-mcp/src/lib.rs`: export the new `tools` helper module.
- `crates/conary-core/src/repository/static_repo/publish_gate.rs`: add an accepted-signer-set canonical hash helper.
- `crates/conary-core/src/repository/static_repo/publish_context.rs`: add read-only static destination trust snapshot DTOs and helper.
- `apps/conary/Cargo.toml`: add `conary-agent-contract`, `conary-mcp`, `rmcp` with stdio transport support, and `schemars`.
- `apps/conary/src/cli/mod.rs`: add a hidden `mcp packaging` command branch only.
- `apps/conary/src/dispatch/root.rs`: bypass try-session preflight for `mcp`, dispatch packaging MCP startup.
- `apps/conary/src/command_risk.rs`: classify server startup as read-only.
- `apps/conary/src/commands/mod.rs`: register the new `packaging_mcp` command module.
- `apps/conary/src/commands/operation_records.rs`: add safe packaging record readers.
- `apps/conary/src/commands/diagnostics.rs`: expose redacted-output projection helpers for MCP.
- `apps/conary/src/commands/publish.rs`: add the service-safe static artifact publish helper while preserving CLI behavior.
- `docs/operations/infrastructure.md`: document the local-only packaging MCP entrypoint after implementation lands.
- `docs/llms/subsystem-map.md` and `docs/modules/feature-ownership.md`: add the look-here-first path for packaging MCP after implementation lands.

Maintainability boundaries:

- `apps/conary/src/cli/mod.rs` is 1974 lines. This plan allows only the enum branch and parser tests there.
- `apps/conary/src/commands/publish.rs` is 1029 lines. This plan allows only a service-safe helper extraction and focused tests; new MCP behavior stays outside the file.
- `apps/conary/src/commands/cook.rs` is 1685 lines. M3b must not add behavior there.
- `crates/conary-core/src/repository/static_repo/publish_context.rs` is 1048 lines. The read-only trust snapshot is acceptable because it preserves the static publish context ownership boundary; larger decomposition is a later reviewed slice.
- `crates/conary-mcp/src/stateless_http.rs` is 1300 lines. This plan does not add behavior there.

Focused verification commands:

```bash
cargo test -p conary-agent-contract
cargo test -p conary-mcp
cargo test -p conary-core static_repo::publish_gate
cargo test -p conary-core static_repo::publish_context
cargo test -p conary command_risk::tests::mcp_packaging_startup_is_read_only
cargo test -p conary commands::operation_records::tests
cargo test -p conary commands::diagnostics::tests
cargo test -p conary commands::packaging_mcp
cargo test -p conary --test packaging_m3a
cargo test -p conary --test packaging_m3b
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

---

### Task 1: Agent Contract Resources And Catalog

**Files:**
- Modify: `crates/conary-agent-contract/src/resource.rs`
- Modify: `crates/conary-agent-contract/src/catalog.rs`
- Test: `crates/conary-agent-contract/src/resource.rs`
- Test: `crates/conary-agent-contract/src/catalog.rs`

- [ ] **Step 1: Write failing resource helper tests**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/conary-agent-contract/src/resource.rs`:

```rust
#[test]
fn packaging_resource_helpers_emit_stable_uris() {
    assert_eq!(
        packaging_operations_recent().uri,
        "conary-packaging://operations/recent"
    );
    assert_eq!(
        packaging_operation("publish-1700000000000-42").uri,
        "conary-packaging://operations/publish-1700000000000-42"
    );
    assert_eq!(
        packaging_operation_events("cook-1").uri,
        "conary-packaging://operations/cook-1/events"
    );
    assert_eq!(
        packaging_project("recipe path").uri,
        "conary-packaging://projects/recipe%20path"
    );
    assert_eq!(
        packaging_artifact("/tmp/pkg.ccs").uri,
        "conary-packaging://artifacts/%2Ftmp%2Fpkg.ccs"
    );
}
```

- [ ] **Step 2: Run the failing resource tests**

Run:

```bash
cargo test -p conary-agent-contract packaging_resource_helpers_emit_stable_uris
```

Expected: fail because the packaging helpers do not exist.

- [ ] **Step 3: Add packaging resource helpers**

Add these helpers above `fn encode_segment` in `crates/conary-agent-contract/src/resource.rs`:

```rust
pub fn packaging_operations_recent() -> ResourceRef {
    ResourceRef::new("conary-packaging://operations/recent")
}

pub fn packaging_operation(operation_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!(
            "conary-packaging://operations/{}",
            encode_segment(operation_id)
        ),
        operation_id,
    )
}

pub fn packaging_operation_events(operation_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!(
            "conary-packaging://operations/{}/events",
            encode_segment(operation_id)
        ),
        operation_id,
    )
}

pub fn packaging_project(project_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!("conary-packaging://projects/{}", encode_segment(project_id)),
        project_id,
    )
}

pub fn packaging_artifact(artifact_id: &str) -> ResourceRef {
    ResourceRef::named(
        format!("conary-packaging://artifacts/{}", encode_segment(artifact_id)),
        artifact_id,
    )
}
```

- [ ] **Step 4: Run the resource tests**

Run:

```bash
cargo test -p conary-agent-contract packaging_resource_helpers_emit_stable_uris
```

Expected: pass.

- [ ] **Step 5: Write failing catalog tests**

Add these tests inside `crates/conary-agent-contract/src/catalog.rs`:

```rust
#[test]
fn packaging_catalog_exposes_resources_and_tools() {
    let resources = packaging_resources();
    assert!(resources.iter().any(|item| item.name == "conary-packaging.operations.recent"));
    assert!(resources.iter().all(|item| item.risk == RiskLevel::ReadOnly));

    let tools = packaging_tools();
    let names = tools
        .iter()
        .map(|item| item.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(names.contains("conary.packaging.inspect_project"));
    assert!(names.contains("conary.packaging.explain_inference"));
    assert!(names.contains("conary.packaging.diagnose_latest_failure"));
    assert!(names.contains("conary.packaging.operation_records.list"));
    assert!(names.contains("conary.packaging.operation_records.read"));
    assert!(names.contains("conary.packaging.publish.plan"));
    assert!(names.contains("conary.packaging.publish.apply"));

    let apply = tools
        .iter()
        .find(|item| item.name == "conary.packaging.publish.apply")
        .expect("publish apply catalog entry");
    assert_eq!(apply.risk, RiskLevel::High);
}
```

- [ ] **Step 6: Run the failing catalog tests**

Run:

```bash
cargo test -p conary-agent-contract packaging_catalog_exposes_resources_and_tools
```

Expected: fail because `packaging_resources` and `packaging_tools` do not exist.

- [ ] **Step 7: Add packaging catalog functions**

Add these functions below `default_read_resources()` in `crates/conary-agent-contract/src/catalog.rs`:

```rust
pub fn packaging_resources() -> Vec<CatalogItem> {
    vec![
        CatalogItem {
            name: "conary-packaging.operations.recent".to_string(),
            description: "Read recent local packaging operation records".to_string(),
            when_to_use: "Use before diagnosing recent cook or publish failures".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary-packaging.operation".to_string(),
            description: "Read one redacted local packaging operation record".to_string(),
            when_to_use: "Use when an operation id is known and detailed diagnostics are needed"
                .to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
    ]
}

pub fn packaging_tools() -> Vec<CatalogItem> {
    vec![
        CatalogItem {
            name: "conary.packaging.inspect_project".to_string(),
            description: "Inspect local packaging project or artifact facts without building"
                .to_string(),
            when_to_use: "Use before planning cook or publish work".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.explain_inference".to_string(),
            description: "Explain recipe inference for a local source tree".to_string(),
            when_to_use: "Use when a source tree has no explicit recipe or inference is surprising"
                .to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.diagnose_latest_failure".to_string(),
            description: "Diagnose the newest failed packaging operation record".to_string(),
            when_to_use: "Use after a cook or publish command failed".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.operation_records.list".to_string(),
            description: "List recent redacted packaging operation records".to_string(),
            when_to_use: "Use to find operation ids for follow-up diagnosis".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.operation_records.read".to_string(),
            description: "Read one redacted packaging operation record".to_string(),
            when_to_use: "Use when an exact operation id is already known".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.publish.plan".to_string(),
            description: "Plan static artifact publish and return confirmation material"
                .to_string(),
            when_to_use: "Use before applying an attested CCS artifact to a static repository"
                .to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.publish.apply".to_string(),
            description: "Apply a confirmed static artifact publish plan".to_string(),
            when_to_use: "Use only with a fresh plan id, matching fingerprint, and explicit confirmation"
                .to_string(),
            risk: RiskLevel::High,
            cache: CachePolicy::private_short(),
        },
    ]
}
```

- [ ] **Step 8: Run contract tests and commit**

Run:

```bash
cargo test -p conary-agent-contract
```

Expected: pass.

Commit:

```bash
git add crates/conary-agent-contract/src/resource.rs crates/conary-agent-contract/src/catalog.rs
git commit -m "feat(agent): add packaging resources and tool catalog"
```

### Task 2: Core Read-Only Static Destination Snapshot

**Files:**
- Modify: `crates/conary-core/src/repository/static_repo/publish_gate.rs`
- Modify: `crates/conary-core/src/repository/static_repo/publish_context.rs`
- Test: `crates/conary-core/src/repository/static_repo/publish_gate.rs`
- Test: `crates/conary-core/src/repository/static_repo/publish_context.rs`

- [ ] **Step 1: Write accepted signer hash test**

Add this test inside the existing tests in `crates/conary-core/src/repository/static_repo/publish_gate.rs`:

```rust
#[test]
fn accepted_signer_set_canonical_hash_is_stable() {
    let set = AcceptedStaticSignerSet::from_trusted_artifact_signers(&[
        TrustedArtifactSigner {
            key_id: "b".to_string(),
            public_key: "pub-b".to_string(),
        },
        TrustedArtifactSigner {
            key_id: "a".to_string(),
            public_key: "pub-a".to_string(),
        },
    ])
    .unwrap();

    let first = set.canonical_hash().unwrap();
    let second = set.canonical_hash().unwrap();
    assert_eq!(first, second);
    assert!(first.starts_with("sha256:"));
}
```

- [ ] **Step 2: Run the failing signer hash test**

Run:

```bash
cargo test -p conary-core accepted_signer_set_canonical_hash_is_stable
```

Expected: fail because `canonical_hash` does not exist.

- [ ] **Step 3: Add the accepted signer hash helper**

Add this method to `impl AcceptedStaticSignerSet` in `publish_gate.rs`:

```rust
pub fn canonical_hash(&self) -> Result<String> {
    canonical_json_hash(&self.active_keys)
}
```

- [ ] **Step 4: Run the signer hash test**

Run:

```bash
cargo test -p conary-core accepted_signer_set_canonical_hash_is_stable
```

Expected: pass.

- [ ] **Step 5: Write destination snapshot tests**

Add tests in `crates/conary-core/src/repository/static_repo/publish_context.rs` near existing static publish context tests:

```rust
#[test]
fn artifact_destination_snapshot_is_read_only_for_missing_repo() {
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let destination = RepoLocation::File { root: repo.clone() };

    let snapshot = inspect_artifact_form_static_destination(&destination).unwrap();

    assert!(snapshot.initial);
    assert!(snapshot.root_key_fingerprint.is_none());
    assert!(!repo.exists(), "read-only snapshot must not create repository directories");
}

#[test]
fn artifact_destination_snapshot_reports_existing_trust_state() {
    let fixture = StaticPublishFixture::new();
    fixture.publish_fixture_package();
    let destination = RepoLocation::File {
        root: fixture.repo_dir.clone(),
    };

    let snapshot = inspect_artifact_form_static_destination(&destination).unwrap();

    assert!(!snapshot.initial);
    assert!(snapshot.root_key_fingerprint.as_deref().unwrap().starts_with("sha256:"));
    assert!(snapshot.package_keys_sha256.as_deref().unwrap().starts_with("sha256:"));
    assert!(snapshot.accepted_signer_set_hash.as_deref().unwrap().starts_with("sha256:"));
    assert_eq!(
        snapshot.publish_policy_digest,
        STATIC_PUBLISH_POLICY_DIGEST_V1
    );
    let versions = snapshot.metadata_versions.expect("metadata versions");
    assert!(versions.root_version >= 1);
    assert!(versions.targets_version >= 1);
    assert!(versions.snapshot_version >= 1);
    assert!(versions.timestamp_version >= 1);
}
```

- [ ] **Step 6: Run the failing destination snapshot tests**

Run:

```bash
cargo test -p conary-core artifact_destination_snapshot
```

Expected: fail because the snapshot DTO and helper do not exist.

- [ ] **Step 7: Add read-only snapshot DTOs and helper**

Add these DTOs in `publish_context.rs` above `ProjectFormAttestationInput`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StaticDestinationMetadataVersions {
    pub root_version: u32,
    pub targets_version: u32,
    pub snapshot_version: u32,
    pub timestamp_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StaticArtifactDestinationSnapshot {
    pub initial: bool,
    pub root_key_fingerprint: Option<String>,
    pub package_keys_sha256: Option<String>,
    pub accepted_signer_set_hash: Option<String>,
    pub publish_policy_digest: String,
    pub metadata_versions: Option<StaticDestinationMetadataVersions>,
}
```

Add this helper below `prepare_artifact_form_static_context`:

```rust
pub fn inspect_artifact_form_static_destination(
    destination: &RepoLocation,
) -> Result<StaticArtifactDestinationSnapshot> {
    ensure_static_local_publish_destination(destination)?;
    let RepoLocation::File { root } = destination else {
        bail!("static publish destination inspection only supports file repositories");
    };
    let destination = read_destination_state(root, false)?;
    if destination.initial {
        return Ok(StaticArtifactDestinationSnapshot {
            initial: true,
            root_key_fingerprint: None,
            package_keys_sha256: destination
                .package_keys_bytes
                .as_deref()
                .map(crate::hash::sha256_prefixed),
            accepted_signer_set_hash: None,
            publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
            metadata_versions: None,
        });
    }

    let root = destination
        .root
        .as_ref()
        .context("verified destination snapshot missing root metadata")?;
    let root_key_fingerprint = root_role_keyids_fingerprint(root)?;
    let package_keys_sha256 = destination
        .package_keys_bytes
        .as_deref()
        .map(crate::hash::sha256_prefixed);
    let accepted_signer_set_hash = match destination.package_keys_bytes.as_deref() {
        Some(bytes) => {
            let text = std::str::from_utf8(bytes)?;
            let keys = PackageKeysFile::parse(text)?;
            Some(AcceptedStaticSignerSet::from_verified_package_keys(&keys)?.canonical_hash()?)
        }
        None => None,
    };

    Ok(StaticArtifactDestinationSnapshot {
        initial: false,
        root_key_fingerprint: Some(root_key_fingerprint),
        package_keys_sha256,
        accepted_signer_set_hash,
        publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
        metadata_versions: Some(StaticDestinationMetadataVersions {
            root_version: root.signed.version,
            targets_version: destination
                .targets
                .as_ref()
                .context("verified destination snapshot missing targets metadata")?
                .signed
                .version,
            snapshot_version: destination
                .snapshot
                .as_ref()
                .context("verified destination snapshot missing snapshot metadata")?
                .signed
                .version,
            timestamp_version: destination
                .timestamp
                .as_ref()
                .context("verified destination snapshot missing timestamp metadata")?
                .signed
                .version,
        }),
    })
}

fn root_role_keyids_fingerprint(root: &Signed<RootMetadata>) -> Result<String> {
    let role = root
        .signed
        .roles
        .get("root")
        .context("destination root metadata missing root role")?;
    crate::ccs::attestation::canonical_json_hash(&role.keyids)
}
```

- [ ] **Step 8: Run core static repo tests and commit**

Run:

```bash
cargo test -p conary-core static_repo::publish_gate
cargo test -p conary-core static_repo::publish_context
```

Expected: pass.

Commit:

```bash
git add crates/conary-core/src/repository/static_repo/publish_gate.rs crates/conary-core/src/repository/static_repo/publish_context.rs
git commit -m "security(static-repo): expose read-only publish trust snapshot"
```

### Task 3: MCP Helper Module And CLI Startup Wiring

**Files:**
- Create: `crates/conary-mcp/src/tools.rs`
- Modify: `crates/conary-mcp/src/lib.rs`
- Modify: `apps/conary/Cargo.toml`
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Create: `apps/conary/src/commands/packaging_mcp/mod.rs`
- Test: `crates/conary-mcp/src/tools.rs`
- Test: `apps/conary/src/cli/mod.rs`
- Test: `apps/conary/src/command_risk.rs`

- [ ] **Step 1: Write failing MCP helper test**

Create `crates/conary-mcp/src/tools.rs` with only this test module first:

```rust
// crates/conary-mcp/src/tools.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize)]
    struct Payload {
        status: &'static str,
    }

    #[test]
    fn contract_tool_result_serializes_json_text_content() {
        let result = contract_tool_result(&Payload { status: "ok" }).unwrap();
        assert_eq!(result.content.len(), 1);
        let rendered = serde_json::to_string(&result.content[0]).unwrap();
        assert!(rendered.contains("\"status\": \"ok\""));
    }
}
```

- [ ] **Step 2: Run the failing MCP helper test**

Run:

```bash
cargo test -p conary-mcp contract_tool_result_serializes_json_text_content
```

Expected: fail because `contract_tool_result` is not defined and `tools` is not exported.

- [ ] **Step 3: Add the MCP helper**

Replace `crates/conary-mcp/src/tools.rs` with:

```rust
// crates/conary-mcp/src/tools.rs
//! Reusable MCP tool response helpers for Conary agent-contract results.

use rmcp::{ErrorData as McpError, model::*};

pub fn contract_tool_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let text = crate::contract_json_text(value)?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize)]
    struct Payload {
        status: &'static str,
    }

    #[test]
    fn contract_tool_result_serializes_json_text_content() {
        let result = contract_tool_result(&Payload { status: "ok" }).unwrap();
        assert_eq!(result.content.len(), 1);
        let rendered = serde_json::to_string(&result.content[0]).unwrap();
        assert!(rendered.contains("\"status\": \"ok\""));
    }
}
```

Add this export in `crates/conary-mcp/src/lib.rs`:

```rust
pub mod tools;
```

- [ ] **Step 4: Run MCP helper tests**

Run:

```bash
cargo test -p conary-mcp contract_tool_result_serializes_json_text_content
```

Expected: pass.

- [ ] **Step 5: Add Conary app dependencies**

Add these dependencies to `apps/conary/Cargo.toml`:

```toml
conary-agent-contract = { path = "../../crates/conary-agent-contract" }
conary-mcp = { path = "../../crates/conary-mcp" }
rmcp = { workspace = true, features = ["server", "macros", "transport-io"] }
schemars.workspace = true
```

- [ ] **Step 6: Write CLI and risk tests**

Add tests in `apps/conary/src/cli/mod.rs` near the existing parser tests:

```rust
#[test]
fn parses_hidden_mcp_packaging_command() {
    let cli = Cli::try_parse_from(["conary", "mcp", "packaging"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Mcp(McpCommands::Packaging))
    ));
}
```

Add this test in `apps/conary/src/command_risk.rs`:

```rust
#[test]
fn mcp_packaging_startup_is_read_only() {
    let cli = Cli {
        seccomp_warn: false,
        allow_live_system_mutation: false,
        command: Some(Commands::Mcp(cli::McpCommands::Packaging)),
    };

    let policy = classify_cli(&cli).expect("mcp packaging should have a risk policy");
    assert_eq!(policy.command_label, "conary mcp packaging");
    assert_eq!(policy.risk, CommandRisk::ReadOnly);
    assert!(!policy.dry_run);
    assert!(!policy.apply_intent);
}
```

- [ ] **Step 7: Run the failing CLI and risk tests**

Run:

```bash
cargo test -p conary parses_hidden_mcp_packaging_command
cargo test -p conary mcp_packaging_startup_is_read_only
```

Expected: fail because the CLI command does not exist.

- [ ] **Step 8: Add CLI enum branch**

In `apps/conary/src/cli/mod.rs`, add this variant under the advanced/developer commands:

```rust
    /// Local MCP servers for agent integrations
    #[command(subcommand, hide = true)]
    Mcp(McpCommands),
```

Add this enum near the other subcommand enums:

```rust
#[derive(Subcommand)]
pub enum McpCommands {
    /// Start the local packaging MCP server on stdio
    Packaging,
}
```

- [ ] **Step 9: Add command module stub and dispatch**

Create `apps/conary/src/commands/packaging_mcp/mod.rs`:

```rust
// apps/conary/src/commands/packaging_mcp/mod.rs
//! Local packaging MCP command surface.

use anyhow::Result;

pub async fn cmd_mcp_packaging() -> Result<()> {
    anyhow::bail!("packaging MCP server wiring is added in the next task")
}
```

Add this module and export in `apps/conary/src/commands/mod.rs`:

```rust
pub(crate) mod packaging_mcp;
pub use packaging_mcp::cmd_mcp_packaging;
```

In `apps/conary/src/dispatch/root.rs`, add `Commands::Mcp(_)` to `command_uses_try_session_preflight_db` false matches:

```rust
        | Commands::Mcp(_)
```

Add this dispatch branch near other top-level commands:

```rust
        Some(Commands::Mcp(cli::McpCommands::Packaging)) => {
            commands::cmd_mcp_packaging().await?;
        }
```

- [ ] **Step 10: Add risk classification**

In `apps/conary/src/command_risk.rs`, add this match arm:

```rust
        Commands::Mcp(cli::McpCommands::Packaging) => Some(read_only("conary mcp packaging")),
```

- [ ] **Step 11: Run CLI and risk tests and commit**

Run:

```bash
cargo test -p conary parses_hidden_mcp_packaging_command
cargo test -p conary mcp_packaging_startup_is_read_only
cargo test -p conary-mcp
```

Expected: pass.

Commit:

```bash
git add crates/conary-mcp/src/lib.rs crates/conary-mcp/src/tools.rs apps/conary/Cargo.toml apps/conary/src/cli/mod.rs apps/conary/src/dispatch/root.rs apps/conary/src/command_risk.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/packaging_mcp/mod.rs
git commit -m "feat(conary): add local packaging mcp startup path"
```

### Task 4: Operation Record Readers And M3a Projection

**Files:**
- Modify: `apps/conary/src/commands/operation_records.rs`
- Modify: `apps/conary/src/commands/diagnostics.rs`
- Create: `apps/conary/src/commands/packaging_mcp/projection.rs`
- Create: `apps/conary/src/commands/packaging_mcp/records.rs`
- Modify: `apps/conary/src/commands/packaging_mcp/mod.rs`
- Test: `apps/conary/src/commands/operation_records.rs`
- Test: `apps/conary/src/commands/packaging_mcp/projection.rs`

- [ ] **Step 1: Write failing operation-record reader tests**

Add tests in `apps/conary/src/commands/operation_records.rs`:

```rust
#[test]
fn load_packaging_record_by_id_rejects_unsafe_ids_and_reads_safe_ids() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Fixture {
        operation_id: String,
        status: String,
    }

    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().join("ops");
    write_packaging_record_unchecked(
        &dir,
        "publish-1",
        &Fixture {
            operation_id: "publish-1".to_string(),
            status: "failed".to_string(),
        },
    )
    .unwrap();

    let loaded = load_packaging_record_by_id::<Fixture>(&dir, "publish-1")
        .unwrap()
        .expect("record");
    assert_eq!(loaded.operation_id, "publish-1");

    assert!(load_packaging_record_by_id::<Fixture>(&dir, "../publish-1").is_err());
    assert!(load_packaging_record_by_id::<Fixture>(&dir, "publish/1").is_err());
}

#[test]
fn load_latest_failed_packaging_record_skips_successful_records() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().join("ops");

    let ok = conary_core::diagnostics::PackagingCommandOutput::succeeded(
        "cook-1",
        "conary cook",
    );
    let failed = conary_core::diagnostics::PackagingCommandOutput::failed(
        "publish-2",
        "conary publish",
        vec![conary_core::diagnostics::PackagingDiagnostic::error(
            conary_core::diagnostics::PackagingPhase::Publish,
            conary_core::diagnostics::PackagingDiagnosticCode::PublishGateFailed,
            "gate failed",
        )],
    );

    write_packaging_record_unchecked(&dir, "cook-1", &ok).unwrap();
    write_packaging_record_unchecked(&dir, "publish-2", &failed).unwrap();

    let loaded = load_latest_failed_packaging_record(&dir)
        .unwrap()
        .expect("failed record");
    assert_eq!(loaded.operation_id, "publish-2");
}
```

- [ ] **Step 2: Run failing operation-record reader tests**

Run:

```bash
cargo test -p conary load_packaging_record_by_id_rejects_unsafe_ids_and_reads_safe_ids
cargo test -p conary load_latest_failed_packaging_record_skips_successful_records
```

Expected: fail because the reader helpers do not exist.

- [ ] **Step 3: Add operation-record readers**

Add these helpers to `apps/conary/src/commands/operation_records.rs`:

```rust
pub fn load_packaging_record_by_id<T: DeserializeOwned>(
    dir: &Path,
    operation_id: &str,
) -> Result<Option<T>> {
    validate_operation_id(operation_id)?;
    let path = dir.join(format!("{operation_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(load_json_record(&path)?))
}

pub fn load_latest_failed_packaging_record(
    dir: &Path,
) -> Result<Option<conary_core::diagnostics::PackagingCommandOutput>> {
    let mut records = list_packaging_records(dir)?;
    records.reverse();
    for path in records {
        let record: conary_core::diagnostics::PackagingCommandOutput = load_json_record(&path)?;
        if record.status == conary_core::diagnostics::PackagingCommandStatus::Failed {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

fn validate_operation_id(operation_id: &str) -> Result<()> {
    if operation_id.is_empty()
        || operation_id.contains('/')
        || operation_id.contains('\\')
        || operation_id.contains("..")
        || operation_id.contains('\0')
        || !operation_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        anyhow::bail!("invalid packaging operation id {operation_id:?}");
    }
    Ok(())
}
```

- [ ] **Step 4: Write failing projection tests**

Create `apps/conary/src/commands/packaging_mcp/projection.rs` with these tests first:

```rust
// apps/conary/src/commands/packaging_mcp/projection.rs

#[cfg(test)]
mod tests {
    use super::*;
    use conary_agent_contract::{AgentErrorKind, EvidenceKind, OperationStatus, RiskLevel};
    use conary_core::diagnostics::{
        DiagnosticEvidence, PackagingCommandOutput, PackagingDiagnostic, PackagingDiagnosticCode,
        PackagingPhase,
    };

    #[test]
    fn failed_publish_gate_projects_validation_error_and_evidence() {
        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::Publish,
            PackagingDiagnosticCode::PublishGateFailed,
            "Static artifact publish gate failed",
        )
        .with_evidence(DiagnosticEvidence::log("publish-gate", "RecordedDraftArtifact"));
        let output = PackagingCommandOutput::failed("publish-1", "conary publish", vec![diagnostic]);

        let envelope = project_packaging_output(
            "conary.packaging.publish.apply",
            &output,
            RiskLevel::High,
            AgentProjectionMode::Apply,
            None,
        );

        assert_eq!(envelope.status, OperationStatus::Failed);
        assert_eq!(envelope.error.unwrap().kind, AgentErrorKind::ValidationFailed);
        assert!(envelope.evidence.iter().any(|item| item.kind == EvidenceKind::Log));
    }

    #[test]
    fn succeeded_plan_projects_planned_status() {
        let output = PackagingCommandOutput::succeeded("plan-1", "conary publish");
        let envelope = project_packaging_output(
            "conary.packaging.publish.plan",
            &output,
            RiskLevel::High,
            AgentProjectionMode::Plan,
            None,
        );

        assert_eq!(envelope.status, OperationStatus::Planned);
        assert!(envelope.error.is_none());
    }
}
```

- [ ] **Step 5: Run failing projection tests**

Run:

```bash
cargo test -p conary commands::packaging_mcp::projection
```

Expected: fail because the projection module is not exported and helpers do not exist.

- [ ] **Step 6: Add projection implementation**

Add this module export in `apps/conary/src/commands/packaging_mcp/mod.rs`:

```rust
pub(crate) mod projection;
pub(crate) mod records;
```

Replace `projection.rs` with:

```rust
// apps/conary/src/commands/packaging_mcp/projection.rs
//! Projection from M3a packaging output into agent operation envelopes.

use conary_agent_contract::{
    AgentError, AgentErrorKind, EvidenceItem, EvidenceKind, EvidenceRedaction, OperationEnvelope,
    OperationStatus, ResourceRef, RiskLevel,
};
use conary_core::diagnostics::{
    DiagnosticEvidence, DiagnosticEvidenceKind, PackagingCommandOutput, PackagingCommandStatus,
    PackagingDiagnosticCode, PackagingSeverity,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentProjectionMode {
    Inspect,
    Plan,
    Apply,
    Explain,
}

pub(crate) fn project_packaging_output(
    operation: &str,
    output: &PackagingCommandOutput,
    risk: RiskLevel,
    mode: AgentProjectionMode,
    subject: Option<ResourceRef>,
) -> OperationEnvelope {
    let status = match (output.status, mode) {
        (PackagingCommandStatus::Succeeded, AgentProjectionMode::Plan) => OperationStatus::Planned,
        (PackagingCommandStatus::Succeeded, _) => OperationStatus::Ok,
        (PackagingCommandStatus::Failed, _) => OperationStatus::Failed,
    };
    let mut envelope = OperationEnvelope::new(
        operation,
        status,
        risk,
        output
            .summary
            .clone()
            .unwrap_or_else(|| format!("{} {}", output.command, status_summary(status))),
    );
    envelope.subject = subject;
    envelope.evidence.push(EvidenceItem {
        kind: EvidenceKind::Check,
        summary: "M3a packaging operation output".to_string(),
        uri: None,
        path: None,
        id: Some(output.operation_id.clone()),
        command: None,
        exit_code: None,
        metadata: std::collections::BTreeMap::from([
            ("command".to_string(), serde_json::json!(output.command)),
            ("schema_version".to_string(), serde_json::json!(output.schema_version)),
            ("status".to_string(), serde_json::json!(output.status)),
        ]),
        redactions: Vec::new(),
    });

    for diagnostic in &output.diagnostics {
        if diagnostic.severity == PackagingSeverity::Warning {
            envelope.warnings.push(diagnostic.message.clone());
        }
        for evidence in &diagnostic.evidence {
            envelope.evidence.push(project_evidence(evidence));
        }
    }

    if let Some(error_diagnostic) = output
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.severity == PackagingSeverity::Error)
    {
        envelope.error = Some(AgentError {
            kind: diagnostic_code_to_error_kind(error_diagnostic.code),
            message: error_diagnostic.message.clone(),
            remediation: error_diagnostic
                .suggestions
                .first()
                .map(|suggestion| suggestion.message.clone()),
        });
    }

    envelope
}

fn status_summary(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Ok => "succeeded",
        OperationStatus::Planned => "planned",
        OperationStatus::Running => "running",
        OperationStatus::Unavailable => "unavailable",
        OperationStatus::Failed => "failed",
        OperationStatus::Partial => "partially completed",
    }
}

fn diagnostic_code_to_error_kind(code: PackagingDiagnosticCode) -> AgentErrorKind {
    match code {
        PackagingDiagnosticCode::InferenceTrace | PackagingDiagnosticCode::RecipeValidationWarning => {
            AgentErrorKind::PartialFailure
        }
        PackagingDiagnosticCode::RecipeValidationFailed
        | PackagingDiagnosticCode::BuildNetworkAccess
        | PackagingDiagnosticCode::UnpinnedDependency
        | PackagingDiagnosticCode::CommandRiskEvidence
        | PackagingDiagnosticCode::PublishGateFailed
        | PackagingDiagnosticCode::ProjectPublishPreflightFailed => AgentErrorKind::ValidationFailed,
        PackagingDiagnosticCode::SourceCacheMiss => AgentErrorKind::MissingPrerequisite,
        PackagingDiagnosticCode::PublishJsonUnsupported => AgentErrorKind::NotSupported,
        PackagingDiagnosticCode::CookFailed
        | PackagingDiagnosticCode::OperationRecordWriteFailed
        | PackagingDiagnosticCode::RedactionFailed
        | PackagingDiagnosticCode::Unknown => AgentErrorKind::PartialFailure,
    }
}

fn project_evidence(evidence: &DiagnosticEvidence) -> EvidenceItem {
    EvidenceItem {
        kind: match evidence.kind {
            DiagnosticEvidenceKind::Command => EvidenceKind::Command,
            DiagnosticEvidenceKind::Path | DiagnosticEvidenceKind::Uri => EvidenceKind::Resource,
            DiagnosticEvidenceKind::Log => EvidenceKind::Log,
            DiagnosticEvidenceKind::Check => EvidenceKind::Check,
            DiagnosticEvidenceKind::Artifact => EvidenceKind::Artifact,
        },
        summary: evidence.summary.clone(),
        uri: evidence.uri.clone(),
        path: evidence.path.clone(),
        id: None,
        command: evidence.command.clone(),
        exit_code: None,
        metadata: evidence.metadata.clone(),
        redactions: evidence
            .redactions
            .iter()
            .map(|redaction| EvidenceRedaction {
                field: redaction.field.clone(),
                reason: redaction.reason.clone(),
            })
            .collect(),
    }
}
```

- [ ] **Step 7: Add records wrapper module**

Create `apps/conary/src/commands/packaging_mcp/records.rs`:

```rust
// apps/conary/src/commands/packaging_mcp/records.rs
//! Packaging operation-record readers for MCP service methods.

use std::path::Path;

use anyhow::Result;
use conary_core::diagnostics::PackagingCommandOutput;

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct PackagingRecordSummary {
    pub operation_id: String,
    pub command: String,
    pub status: String,
    pub diagnostics: usize,
}

pub(crate) fn list_recent_records(
    dir: &Path,
    limit: usize,
) -> Result<Vec<PackagingRecordSummary>> {
    let mut paths = super::super::operation_records::list_packaging_records(dir)?;
    paths.reverse();
    paths
        .into_iter()
        .take(limit)
        .map(|path| {
            let record: PackagingCommandOutput =
                super::super::operation_records::load_json_record(&path)?;
            Ok(PackagingRecordSummary {
                operation_id: record.operation_id,
                command: record.command,
                status: format!("{:?}", record.status),
                diagnostics: record.diagnostics.len(),
            })
        })
        .collect()
}

pub(crate) fn read_record(dir: &Path, operation_id: &str) -> Result<Option<PackagingCommandOutput>> {
    super::super::operation_records::load_packaging_record_by_id(dir, operation_id)
}

pub(crate) fn latest_failed_record(dir: &Path) -> Result<Option<PackagingCommandOutput>> {
    super::super::operation_records::load_latest_failed_packaging_record(dir)
}
```

- [ ] **Step 8: Run operation-record and projection tests and commit**

Run:

```bash
cargo test -p conary commands::operation_records::tests
cargo test -p conary commands::packaging_mcp::projection
```

Expected: pass.

Commit:

```bash
git add apps/conary/src/commands/operation_records.rs apps/conary/src/commands/diagnostics.rs apps/conary/src/commands/packaging_mcp/mod.rs apps/conary/src/commands/packaging_mcp/projection.rs apps/conary/src/commands/packaging_mcp/records.rs
git commit -m "feat(packaging): project m3a records into agent results"
```

### Task 5: Packaging MCP Server And Read-Only Service Tools

**Files:**
- Create: `apps/conary/src/commands/packaging_mcp/types.rs`
- Create: `apps/conary/src/commands/packaging_mcp/service.rs`
- Create: `apps/conary/src/commands/packaging_mcp/server.rs`
- Modify: `apps/conary/src/commands/packaging_mcp/mod.rs`
- Test: `apps/conary/src/commands/packaging_mcp/service.rs`
- Test: `apps/conary/src/commands/packaging_mcp/server.rs`

- [ ] **Step 1: Add DTOs**

Create `apps/conary/src/commands/packaging_mcp/types.rs`:

```rust
// apps/conary/src/commands/packaging_mcp/types.rs
//! Tool input and data DTOs for the packaging MCP service.

use conary_agent_contract::ResourceRef;

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct InspectProjectInput {
    pub target: String,
    #[serde(default)]
    pub recipe: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ExplainInferenceInput {
    pub target: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct DiagnoseLatestFailureInput {
    #[serde(default)]
    pub limit_events: Option<usize>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct OperationRecordsListInput {
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct OperationRecordsReadInput {
    pub operation_id: String,
    #[serde(default)]
    pub include_events: bool,
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct InspectProjectData {
    pub target_kind: String,
    pub subject: ResourceRef,
    pub recipe_path: Option<String>,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct OperationRecordData {
    pub operation_id: String,
    pub record: serde_json::Value,
}
```

Add this module export in `packaging_mcp/mod.rs`:

```rust
pub(crate) mod types;
```

- [ ] **Step 2: Write failing service read tests**

Create `apps/conary/src/commands/packaging_mcp/service.rs` with these tests first:

```rust
// apps/conary/src/commands/packaging_mcp/service.rs

#[cfg(test)]
mod tests {
    use super::*;
    use conary_agent_contract::{OperationStatus, RiskLevel};

    #[test]
    fn inspect_project_reads_recipe_without_building() {
        let temp = tempfile::TempDir::new().unwrap();
        let recipe = temp.path().join("recipe.toml");
        std::fs::write(
            &recipe,
            r#"
[package]
name = "demo"
version = "0.1.0"
description = "demo"
license = "MIT"

[source]
path = "."

[build]
install = ["mkdir -p $DESTDIR/usr/bin", "touch $DESTDIR/usr/bin/demo"]
"#,
        )
        .unwrap();

        let service = PackagingAgentService::default();
        let result = service
            .inspect_project(crate::commands::packaging_mcp::types::InspectProjectInput {
                target: recipe.display().to_string(),
                recipe: None,
            })
            .unwrap();

        assert_eq!(result.envelope.status, OperationStatus::Ok);
        assert_eq!(result.envelope.risk, RiskLevel::ReadOnly);
        assert_eq!(result.data["package_name"], "demo");
    }

    #[test]
    fn list_operation_records_reads_private_store() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));
        let output = conary_core::diagnostics::PackagingCommandOutput::failed(
            "publish-1",
            "conary publish",
            vec![conary_core::diagnostics::PackagingDiagnostic::error(
                conary_core::diagnostics::PackagingPhase::Publish,
                conary_core::diagnostics::PackagingDiagnosticCode::PublishGateFailed,
                "gate failed",
            )],
        );
        crate::commands::operation_records::write_packaging_record_unchecked(
            service.operations_dir(),
            "publish-1",
            &output,
        )
        .unwrap();

        let result = service
            .list_operation_records(
                crate::commands::packaging_mcp::types::OperationRecordsListInput {
                    limit: Some(10),
                },
            )
            .unwrap();

        assert_eq!(result.envelope.status, OperationStatus::Ok);
        assert_eq!(result.data["records"][0]["operation_id"], "publish-1");
    }
}
```

- [ ] **Step 3: Run failing service tests**

Run:

```bash
cargo test -p conary commands::packaging_mcp::service
```

Expected: fail because the service implementation does not exist.

- [ ] **Step 4: Implement read-only service methods**

Replace `service.rs` with the `PackagingAgentService` implementation described below. It must expose `with_operations_dir`, `operations_dir`, `inspect_project`, `explain_inference`, `list_operation_records`, and `read_operation_record`; it must call `parse_recipe_file`, `validate_recipe`, `resolve_new_from_target`, and `infer_recipe_from_path`; and every result must use `RiskLevel::ReadOnly`.

Key implementation excerpt:

```rust
#[derive(Debug, Clone)]
pub(crate) struct PackagingAgentService {
    operations_dir: PathBuf,
}

impl PackagingAgentService {
    pub(crate) fn with_operations_dir(operations_dir: PathBuf) -> Self {
        Self { operations_dir }
    }

    pub(crate) fn operations_dir(&self) -> &Path {
        &self.operations_dir
    }

    pub(crate) fn list_operation_records(
        &self,
        input: OperationRecordsListInput,
    ) -> Result<InspectResult> {
        let limit = input.limit.unwrap_or(20).min(50);
        let records = records::list_recent_records(&self.operations_dir, limit)?;
        let mut envelope = OperationEnvelope::new(
            "conary.packaging.operation_records.list",
            OperationStatus::Ok,
            RiskLevel::ReadOnly,
            "Listed packaging operation records",
        );
        envelope.subject = Some(resource::packaging_operations_recent());
        Ok(InspectResult::new(envelope).with_data(serde_json::json!({ "records": records })))
    }
}
```

Add this module export in `packaging_mcp/mod.rs`:

```rust
pub(crate) mod service;
```

- [ ] **Step 5: Add MCP server wrapper**

Create `apps/conary/src/commands/packaging_mcp/server.rs` following the existing Remi and conary-test pattern: `#[derive(Clone)] PackagingMcpServer`, `#[tool_router]`, `ServerHandler`, `list_tools`, `call_tool`, and `get_tool`. Use `conary_mcp::tools::contract_tool_result` for every tool response.

The first four tool methods must have these signatures:

```rust
async fn inspect_project(
    &self,
    Parameters(input): Parameters<InspectProjectInput>,
) -> Result<CallToolResult, McpError>;

async fn explain_inference(
    &self,
    Parameters(input): Parameters<ExplainInferenceInput>,
) -> Result<CallToolResult, McpError>;

async fn operation_records_list(
    &self,
    Parameters(input): Parameters<OperationRecordsListInput>,
) -> Result<CallToolResult, McpError>;

async fn operation_records_read(
    &self,
    Parameters(input): Parameters<OperationRecordsReadInput>,
) -> Result<CallToolResult, McpError>;
```

Add this module export in `packaging_mcp/mod.rs`:

```rust
mod server;
```

- [ ] **Step 6: Wire stdio command**

Replace `cmd_mcp_packaging` in `packaging_mcp/mod.rs` with:

```rust
pub async fn cmd_mcp_packaging() -> Result<()> {
    use rmcp::ServiceExt;

    let service = service::PackagingAgentService::default();
    let server = server::PackagingMcpServer::new(service);
    server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| anyhow::anyhow!("start packaging MCP server: {error}"))?
        .waiting()
        .await
        .map_err(|error| anyhow::anyhow!("packaging MCP server stopped with error: {error}"))?;
    Ok(())
}
```

- [ ] **Step 7: Run service and MCP tests and commit**

Run:

```bash
cargo test -p conary commands::packaging_mcp::service
cargo test -p conary commands::packaging_mcp::server
cargo test -p conary-mcp
```

Expected: pass.

Commit:

```bash
git add apps/conary/src/commands/packaging_mcp crates/conary-mcp/src/tools.rs crates/conary-mcp/src/lib.rs
git commit -m "feat(packaging): expose read-only mcp tools"
```

### Task 6: Service-Safe Static Artifact Publish Helper

**Files:**
- Modify: `apps/conary/src/commands/publish.rs`
- Test: `apps/conary/src/commands/publish.rs`

- [ ] **Step 1: Write failing service-helper tests**

Add a publish test proving the helper returns `PackagingCommandOutput` for gate failure without writing directly to a `Write`:

```rust
#[tokio::test]
async fn static_artifact_service_helper_returns_structured_gate_failure() {
    let fixture = PublishFixture::new();
    let output = publish_static_artifact_form_service(StaticArtifactPublishServiceInput {
        artifact_path: fixture.unsigned_package.clone(),
        target: fixture.repo_target(),
        key_dir: Some(fixture.key_dir.clone()),
        state_file: Some(fixture.state_file.clone()),
        refresh: false,
        operation_id: "publish-test".to_string(),
    })
    .await
    .unwrap();

    assert_eq!(output.operation_id, "publish-test");
    assert_eq!(output.status, PackagingCommandStatus::Failed);
    assert_eq!(output.diagnostics[0].code, PackagingDiagnosticCode::PublishGateFailed);
}
```

- [ ] **Step 2: Run failing service-helper test**

Run:

```bash
cargo test -p conary static_artifact_service_helper_returns_structured_gate_failure
```

Expected: fail because the service helper and input type do not exist.

- [ ] **Step 3: Add service helper types and helper**

Add this struct near `PublishOptions` in `publish.rs`:

```rust
pub(crate) struct StaticArtifactPublishServiceInput {
    pub artifact_path: PathBuf,
    pub target: String,
    pub key_dir: Option<PathBuf>,
    pub state_file: Option<PathBuf>,
    pub refresh: bool,
    pub operation_id: String,
}
```

Add `publish_static_artifact_form_service(input) -> Result<PackagingCommandOutput>` below `publish_static_artifact_form`. The helper must parse the target, ensure a static local destination, resolve key/state paths, call `prepare_artifact_form_static_context`, call `verify_static_artifact_publish_eligibility`, return `publish_gate_failure_output` when the report fails, call `publish_static_repo` only when gates pass, and return `publish_success_output` with the published artifact.

- [ ] **Step 4: Rewire CLI wrapper through the helper**

Reduce `publish_static_artifact_form` to construct `StaticArtifactPublishServiceInput`, call the helper, write M3a JSON/human output, write the operation record, and return a CLI error only after writing the structured failed output.

The CLI wrapper must pass these dangerous options as fixed false values inside the helper:

```rust
force_reinit: false,
accept_destination_state: false,
rotate_publish_key: false,
rotate_root_key: false,
```

- [ ] **Step 5: Run publish helper tests and existing M3a publish test**

Run:

```bash
cargo test -p conary static_artifact_service_helper_returns_structured_gate_failure
cargo test -p conary --test packaging_m3a
```

Expected: pass.

Commit:

```bash
git add apps/conary/src/commands/publish.rs
git commit -m "refactor(publish): expose structured static artifact helper"
```

### Task 7: Publish Plan Material And Registry

**Files:**
- Modify: `apps/conary/src/commands/packaging_mcp/types.rs`
- Create: `apps/conary/src/commands/packaging_mcp/publish_plan.rs`
- Modify: `apps/conary/src/commands/packaging_mcp/mod.rs`
- Test: `apps/conary/src/commands/packaging_mcp/publish_plan.rs`

- [ ] **Step 1: Add publish plan DTOs**

Append these DTOs to `apps/conary/src/commands/packaging_mcp/types.rs`:

```rust
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublishModeInput {
    Auto,
    ArtifactStatic,
    ProjectStatic,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct PublishPlanInput {
    pub artifact_or_project_path: String,
    pub target: String,
    #[serde(default)]
    pub recipe: Option<String>,
    #[serde(default)]
    pub key_dir: Option<String>,
    #[serde(default)]
    pub state_file: Option<String>,
    #[serde(default = "default_publish_mode")]
    pub mode: PublishModeInput,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct PublishApplyInput {
    pub plan_id: String,
    pub fingerprint: String,
    pub confirmation: String,
}

fn default_publish_mode() -> PublishModeInput {
    PublishModeInput::Auto
}
```

- [ ] **Step 2: Write failing plan registry tests**

Create `apps/conary/src/commands/packaging_mcp/publish_plan.rs` with tests for registry capacity, fingerprint confirmation, expiry, and private staging mode. The staging test must assert directory mode `0700` and file mode `0600` on Unix.

- [ ] **Step 3: Run failing plan registry tests**

Run:

```bash
cargo test -p conary commands::packaging_mcp::publish_plan
```

Expected: fail because the module is not exported and plan types do not exist.

- [ ] **Step 4: Implement plan material, registry, and staging helpers**

Add this module export in `packaging_mcp/mod.rs`:

```rust
pub(crate) mod publish_plan;
```

Implement these concrete types and functions in `publish_plan.rs`:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct PublishPlanMaterial {
    pub schema_version: u16,
    pub plan_kind: String,
    pub mode: String,
    pub stored_route_enum: String,
    pub normalized_artifact_or_project_path: String,
    pub artifact_sha256: String,
    pub artifact_size: u64,
    pub artifact_manifest_identity_when_available: Option<serde_json::Value>,
    pub normalized_static_target: String,
    pub key_dir_path_when_supplied: Option<String>,
    pub state_file_path_when_supplied: Option<String>,
    pub selected_options: std::collections::BTreeMap<String, serde_json::Value>,
    pub command_risk_projection: String,
    pub destination_root_key_fingerprint: Option<String>,
    pub destination_package_key_hash: Option<String>,
    pub accepted_signer_set_hash: Option<String>,
    pub publish_policy_digest: String,
    pub metadata_versions_or_watermark: Option<serde_json::Value>,
    pub expires_at: String,
}

pub(crate) struct PublishPlanRegistry {
    capacity: usize,
    order: std::collections::VecDeque<String>,
    plans: std::collections::BTreeMap<String, StoredPublishPlan>,
}

pub(crate) fn stage_artifact_private(source: &std::path::Path) -> anyhow::Result<StagedArtifact>;
```

`PublishPlanRegistry::insert` must generate a UUIDv4 plan id and compute `canonical_json_hash(&material)`. `get_confirmed` must reject missing, expired, mismatched-fingerprint, and mismatched-confirmation plans.

`stage_artifact_private` must create a private temp directory, write the copied artifact with mode `0600` on Unix, fsync the staged file, and return the staged path plus `sha256:` digest.

- [ ] **Step 5: Run plan registry tests and commit**

Run:

```bash
cargo test -p conary commands::packaging_mcp::publish_plan
```

Expected: pass.

Commit:

```bash
git add apps/conary/src/commands/packaging_mcp/types.rs apps/conary/src/commands/packaging_mcp/mod.rs apps/conary/src/commands/packaging_mcp/publish_plan.rs
git commit -m "feat(packaging): add publish plan registry"
```

### Task 8: Publish Plan And Apply Service Methods

**Files:**
- Modify: `apps/conary/src/commands/packaging_mcp/service.rs`
- Modify: `apps/conary/src/commands/packaging_mcp/server.rs`
- Modify: `apps/conary/src/commands/packaging_mcp/publish_plan.rs`
- Modify: `apps/conary/src/commands/packaging_mcp/types.rs`
- Test: `apps/conary/src/commands/packaging_mcp/service.rs`
- Test: `apps/conary/tests/packaging_m3b.rs`

- [ ] **Step 1: Write failing plan/apply service tests**

Add service tests proving:

```rust
#[test]
fn publish_plan_for_missing_static_trust_state_returns_missing_prerequisite() {
    let temp = tempfile::TempDir::new().unwrap();
    let artifact = temp.path().join("pkg.ccs");
    std::fs::write(&artifact, b"not-a-real-package").unwrap();
    let service = PackagingAgentService::with_operations_dir(temp.path().join("ops"));

    let result = service.plan_publish(crate::commands::packaging_mcp::types::PublishPlanInput {
        artifact_or_project_path: artifact.display().to_string(),
        target: temp.path().join("repo").display().to_string(),
        recipe: None,
        key_dir: Some(temp.path().join("keys").display().to_string()),
        state_file: None,
        mode: crate::commands::packaging_mcp::types::PublishModeInput::ArtifactStatic,
    });

    let plan = result.expect("missing trust state is represented as an agent result");
    assert_eq!(plan.envelope.status, conary_agent_contract::OperationStatus::Failed);
    assert_eq!(
        plan.envelope.error.unwrap().kind,
        conary_agent_contract::AgentErrorKind::MissingPrerequisite
    );
    assert!(!temp.path().join("repo").exists());
    assert!(!temp.path().join("keys").exists());
}
```

Add `apps/conary/tests/packaging_m3b.rs` with a parser/help check for `conary mcp packaging --help`.

- [ ] **Step 2: Run failing plan/apply tests**

Run:

```bash
cargo test -p conary publish_plan_for_missing_static_trust_state_returns_missing_prerequisite
cargo test -p conary --test packaging_m3b
```

Expected: fail because publish plan service methods and help behavior are incomplete.

- [ ] **Step 3: Extend service with shared registry**

Change `PackagingAgentService` in `service.rs` to hold:

```rust
publish_plans: std::sync::Arc<std::sync::Mutex<super::publish_plan::PublishPlanRegistry>>,
```

Construct it with `PublishPlanRegistry::new(16)` in every constructor.

- [ ] **Step 4: Implement `plan_publish` for static artifact route**

Add `plan_publish(&self, input: PublishPlanInput) -> Result<PlanResult>`.

The method must:

- canonicalize the artifact path;
- read artifact bytes and compute `sha256:` digest and size;
- parse the target as `RepoLocation`;
- call `inspect_artifact_form_static_destination`;
- return a failed `PlanResult` with `AgentErrorKind::MissingPrerequisite` if the repo is initial or lacks accepted signer state;
- create `PublishPlanMaterial` with all fields listed in the M3b design;
- insert it into the in-memory registry;
- return `PlanResult` with `OperationStatus::Planned`, `RiskLevel::High`, `ConfirmationRequirement`, and next action for `conary.packaging.publish.apply`.

- [ ] **Step 5: Implement `apply_publish` confirmation and staged helper call**

Add `apply_publish(&self, input: PublishApplyInput) -> Result<ApplyResult>`.

The method must:

- call `get_confirmed(plan_id, fingerprint, confirmation)`;
- map any confirmation error to `AgentErrorKind::UnsafeWithoutConfirmation`;
- re-read destination trust snapshot and compare root fingerprint, package key hash, accepted signer hash, and policy digest with plan material;
- stage artifact bytes through `stage_artifact_private`;
- reject staged digest mismatch with `AgentErrorKind::UnsafeWithoutConfirmation`;
- call `publish_static_artifact_form_service` with the staged path and fixed safe options;
- write a redacted operation record with `write_packaging_record_if_possible`;
- project the resulting `PackagingCommandOutput` with `AgentProjectionMode::Apply` and `RiskLevel::High`.

- [ ] **Step 6: Wire publish plan/apply tools into the server**

Add tool methods in `server.rs`:

```rust
#[tool(description = "Plan static artifact publish and return confirmation material.")]
async fn publish_plan(
    &self,
    Parameters(input): Parameters<PublishPlanInput>,
) -> Result<CallToolResult, McpError> {
    let result = self
        .service
        .plan_publish(input)
        .map_err(conary_mcp::map_internal)?;
    contract_tool_result(&result)
}

#[tool(description = "Apply a confirmed static artifact publish plan.")]
async fn publish_apply(
    &self,
    Parameters(input): Parameters<PublishApplyInput>,
) -> Result<CallToolResult, McpError> {
    let result = self
        .service
        .apply_publish(input)
        .await
        .map_err(conary_mcp::map_internal)?;
    contract_tool_result(&result)
}
```

- [ ] **Step 7: Run plan/apply tests**

Run:

```bash
cargo test -p conary commands::packaging_mcp::service
cargo test -p conary --test packaging_m3b
```

Expected: pass after fixture adjustments needed by existing publish helpers.

Commit:

```bash
git add apps/conary/src/commands/packaging_mcp apps/conary/tests/packaging_m3b.rs
git commit -m "feat(packaging): add mcp publish plan apply"
```

### Task 9: Integration Coverage, Redaction Checks, And Docs

**Files:**
- Modify: `apps/conary/tests/packaging_m3b.rs`
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`
- Modify: `docs/superpowers/specs/2026-06-16-m3b-packaging-mcp-design.md`
- Modify if needed: `docs/superpowers/feature-coherency-ledger.tsv`
- Modify if needed: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Add focused M3b integration assertions**

Expand `apps/conary/tests/packaging_m3b.rs` and/or `commands::packaging_mcp` unit tests to prove:

- `conary mcp packaging --help` parses without starting a server.
- tool list includes all seven M3b tools and excludes Remi/admin tools.
- operation-record read results do not expose bearer-token text, private key paths, or credentialed URLs.
- `publish.plan` does not create repo dirs, key dirs, locks, state files, metadata, or operation records.
- `publish.apply` rejects missing plan, expired plan, mismatched fingerprint, mismatched confirmation, changed artifact bytes, changed destination trust state, and changed publish options.
- static artifact-form gate refusal returns `validation_failed` with `PublishLintReport` evidence.

- [ ] **Step 2: Run full focused M3b test set**

Run:

```bash
cargo test -p conary-agent-contract
cargo test -p conary-mcp
cargo test -p conary-core static_repo::publish_gate
cargo test -p conary-core static_repo::publish_context
cargo test -p conary commands::operation_records::tests
cargo test -p conary commands::diagnostics::tests
cargo test -p conary commands::packaging_mcp
cargo test -p conary --test packaging_m3a
cargo test -p conary --test packaging_m3b
```

Expected: pass.

- [ ] **Step 3: Update docs after implementation lands**

In `docs/operations/infrastructure.md`, add:

```markdown
### Local Packaging MCP

`conary mcp packaging` starts the local stdio MCP server for packaging agent
workflows. It does not open a network listener. The first mutation contract is
confirmed static artifact publish through `conary.packaging.publish.plan` and
`conary.packaging.publish.apply`; Remi publish apply and project-form publish
apply are intentionally unsupported in this slice.
```

In `docs/llms/subsystem-map.md`, add:

```markdown
- Packaging MCP: start with `apps/conary/src/commands/packaging_mcp/`, then use
  `crates/conary-agent-contract/src/{resource,catalog,result}.rs`,
  `crates/conary-mcp/src/`, and `apps/conary/src/commands/{diagnostics,operation_records,publish}.rs`.
```

In `docs/modules/feature-ownership.md`, add:

```markdown
| Packaging MCP | `apps/conary/src/commands/packaging_mcp/` | Owns local stdio MCP tools, agent projection, publish plan registry, and read-only operation-record/project inspection. Publish mutations remain owned by `apps/conary/src/commands/publish.rs`. |
```

Update the M3 umbrella status to M3b landed and update the M3b design status to landed once the implementation is verified.

- [ ] **Step 4: Run docs and final verification gates**

Run:

```bash
scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
scripts/docs-audit-inventory.sh > /tmp/conary-doc-inventory.generated
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv /tmp/conary-doc-inventory.generated
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: all commands pass.

- [ ] **Step 5: Commit final M3b implementation**

Commit:

```bash
git add apps/conary crates/conary-agent-contract crates/conary-core crates/conary-mcp docs
git commit -m "feat(packaging): land m3b mcp publish surface"
```

## Plan Self-Review

Spec coverage:

- Local stdio transport: Task 3 and Task 5.
- Contract resources and catalog entries: Task 1.
- M3a-to-agent projection: Task 4.
- Operation record list/read/latest failure: Task 4 and Task 5.
- Read-only inspect and inference explain: Task 5.
- Read-only destination trust snapshot: Task 2.
- Static artifact publish plan fingerprint and in-memory registry: Task 7 and Task 8.
- Static artifact publish apply with confirmation, staging, gate reuse, and structured output: Task 6 through Task 8.
- High risk for static publish apply: Task 1 catalog and Task 8 envelopes.
- No Remi/project apply, no shell/env/token inputs, no network listener: scope locks, DTOs, and service methods.

Exactness scan:

- The plan was checked for unfinished markers, vague implementation phrases,
  and ambiguous doubled-question markers before commit.

Type consistency:

- Tool DTOs are defined in Task 5 and extended in Task 7 before server/service methods consume them.
- `PublishPlanMaterial`, `PublishPlanRegistry`, and `stage_artifact_private` are defined in Task 7 before Task 8 calls them.
- `publish_static_artifact_form_service` is defined in Task 6 before Task 8 calls it.
- Projection mode names are defined in Task 4 before Task 8 uses `AgentProjectionMode::Apply`.
