# Conary-Test Suites Stateless Resource Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one static read-only stateless MCP resource to `conary-test`: `conary-test://suites`.

**Architecture:** Keep URI/catalog vocabulary in `conary-agent-contract`, suite manifest inspection in a new non-Axum `apps/conary-test/src/suite_inventory.rs` module, and MCP dispatch in the existing `apps/conary-test/src/server/stateless_mcp.rs` provider. The raw `conary-mcp` stateless adapter already supports provider-backed `resources/list` and `resources/read`; this slice wires a second live resource without adding tools, prompts, templates, subscriptions, SSE, Remi behavior, or mutations.

**Tech Stack:** Rust 2024, serde/serde_json, tempfile tests, Axum state extractors, `conary-agent-contract` operation envelopes, `conary-mcp::stateless_http`.

---

## /goal Text

Implement `docs/superpowers/plans/2026-05-24-conary-test-suites-resource.md` task-by-task. Source spec: `docs/superpowers/specs/2026-05-24-conary-test-suites-resource-design.md`.

Add only one static read-only stateless MCP resource to conary-test: `conary-test://suites`. Keep Remi untouched. Do not add live tools, prompts, resource templates, subscriptions, SSE streaming, smoke execution, per-suite resources, or mutations. Keep legacy `/mcp` behavior unchanged. Use tests-first for every code task, verify the expected failure before implementation, update checkboxes as tasks complete, and make one focused commit per task.

## Files And Responsibilities

- `crates/conary-agent-contract/src/resource.rs`
  - Add `test_suites() -> ResourceRef` for the static suite index URI.
- `crates/conary-agent-contract/src/catalog.rs`
  - Add a transport-neutral read-resource catalog item for `conary-test.suites`.
- `apps/conary-test/src/suite_inventory.rs`
  - New module for reading suite manifests, computing container/QEMU flags, and producing an `InspectResult`.
  - No Axum, MCP, `rmcp`, route, or server dependencies.
- `apps/conary-test/src/lib.rs`
  - Export the new `suite_inventory` module.
- `apps/conary-test/src/server/stateless_mcp.rs`
  - Change the stateless handler to receive `AppState`, register two resources, and read suites from `state.manifest_dir`.
- `apps/conary-test/README.md`
  - Document that `/mcp/stateless` exposes bootstrap status plus suites.
- `docs/operations/agent-mcp-adapter-decision.md`
  - Update current-state language and source specs.
- `docs/operations/infrastructure.md`
  - Update agent-operations wording for the second read-only resource.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Refresh after docs are updated.
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Reconcile rows for updated docs and this plan.

## Hard Constraints

- `resources/list` returns exactly two descriptors in this order:
  1. `conary-local://bootstrap/status`
  2. `conary-test://suites`
- `resources/read` for `conary-test://suites` returns one `application/json` text content block.
- The content block text is valid JSON for `conary_agent_contract::InspectResult`.
- Suite inventory uses `AppState.manifest_dir`, never hard-coded repository paths.
- Manifest parse and local filesystem problems are Conary operation state, not MCP transport errors.
- Unknown resource URIs keep returning HTTP `404` with JSON-RPC `-32602`.
- Missing or mismatched `Mcp-Name` keeps returning HTTP `400` with JSON-RPC `-32001`.
- Existing `/mcp` remains session-based and must not return stateless resource output.
- The route stays inside existing conary-test auth middleware.

## Task 1: Add Contract URI And Catalog Entry

**Files:**
- Modify: `crates/conary-agent-contract/src/resource.rs`
- Modify: `crates/conary-agent-contract/src/catalog.rs`

- [x] **Step 1: Write failing resource helper test**

Add this test inside `#[cfg(test)] mod tests` in `crates/conary-agent-contract/src/resource.rs`:

```rust
    #[test]
    fn test_suites_resource_helper_emits_static_index_uri() {
        let resource = test_suites();

        assert_eq!(resource.uri, "conary-test://suites");
        assert!(resource.name.is_none());
    }
```

- [x] **Step 2: Write failing catalog test**

Add this test inside `#[cfg(test)] mod tests` in `crates/conary-agent-contract/src/catalog.rs`:

```rust
    #[test]
    fn default_read_resources_include_conary_test_suites() {
        let resources = default_read_resources();
        let suites = resources
            .iter()
            .find(|item| item.name == "conary-test.suites")
            .expect("conary-test suites catalog entry should exist");

        assert_eq!(
            suites.description,
            "Read local conary-test suite manifest inventory"
        );
        assert_eq!(
            suites.when_to_use,
            "Use before selecting local conary-test smoke or validation suites"
        );
        assert_eq!(suites.risk, RiskLevel::ReadOnly);
        assert_eq!(suites.cache, CachePolicy::private_short());
    }
```

- [x] **Step 3: Run tests to verify failure**

Run:

```bash
cargo test -p conary-agent-contract test_suites
cargo test -p conary-agent-contract default_read_resources_include_conary_test_suites
```

Expected:

- First command fails because `test_suites` does not exist.
- Second command fails because the catalog entry does not exist.

- [x] **Step 4: Add `test_suites()` helper**

Add this function before `test_suite(suite_id: &str)` in `crates/conary-agent-contract/src/resource.rs`:

```rust
pub fn test_suites() -> ResourceRef {
    ResourceRef::new("conary-test://suites")
}
```

- [x] **Step 5: Add catalog item**

Insert this `CatalogItem` in `default_read_resources()` after `conary-test.bootstrap.status`:

```rust
        CatalogItem {
            name: "conary-test.suites".to_string(),
            description: "Read local conary-test suite manifest inventory".to_string(),
            when_to_use: "Use before selecting local conary-test smoke or validation suites"
                .to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
```

- [x] **Step 6: Verify Task 1**

Run:

```bash
cargo fmt --check
cargo test -p conary-agent-contract test_suites_resource_helper_emits_static_index_uri
cargo test -p conary-agent-contract default_read_resources_include_conary_test_suites
cargo test -p conary-agent-contract
```

Expected: all commands pass.

- [x] **Step 7: Commit Task 1**

Run:

```bash
git status --short
git add crates/conary-agent-contract/src/resource.rs crates/conary-agent-contract/src/catalog.rs
git commit -m "feat(agent-contract): add conary-test suites resource"
git status --short
```

## Task 2: Add Suite Inventory Module

**Files:**
- Create: `apps/conary-test/src/suite_inventory.rs`
- Modify: `apps/conary-test/src/lib.rs`

- [ ] **Step 1: Export module and write failing tests**

Add this line to `apps/conary-test/src/lib.rs`:

```rust
pub mod suite_inventory;
```

Create `apps/conary-test/src/suite_inventory.rs` with this test-first skeleton:

```rust
// conary-test/src/suite_inventory.rs
//! Suite manifest inventory for local conary-test agent resources.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use conary_agent_contract::{OperationStatus, RiskLevel};
    use tempfile::tempdir;

    use super::*;

    fn write_manifest(dir: &Path, file_name: &str, body: &str) {
        std::fs::write(dir.join(file_name), body).unwrap();
    }

    fn container_manifest(name: &str, phase: u32) -> String {
        format!(
            r#"
[suite]
name = "{name}"
phase = {phase}

[[test]]
id = "T01"
name = "container_test"
description = "Container test"
timeout = 10

[[test.step]]
run = "true"
"#
        )
    }

    fn qemu_manifest(name: &str, phase: u32) -> String {
        format!(
            r#"
[suite]
name = "{name}"
phase = {phase}

[[test]]
id = "TQEMU"
name = "qemu_test"
description = "QEMU test"
timeout = 10

[[test.step]]
[test.step.qemu_boot]
image = "unused"
local_image_path = "/tmp/missing.qcow2"
commands = ["true"]
"#
        )
    }

    #[test]
    fn inventory_reads_valid_manifests_and_flags() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "phase2-container.toml", &container_manifest("container", 2));
        write_manifest(root.path(), "phase3-qemu.toml", &qemu_manifest("qemu", 3));

        let inspect = inspect_manifest_dir(root.path());
        let suites = inspect.data["suites"].as_array().unwrap();

        assert_eq!(inspect.envelope.operation, "conary-test.suites.inspect");
        assert_eq!(inspect.envelope.status, OperationStatus::Ok);
        assert_eq!(inspect.envelope.risk, RiskLevel::ReadOnly);
        assert_eq!(
            inspect.envelope.subject.as_ref().unwrap().uri,
            "conary-test://suites"
        );
        assert_eq!(inspect.data["dir_exists"], true);
        assert_eq!(inspect.data["toml_files"], 2);
        assert_eq!(inspect.data["parsed"], 2);
        assert_eq!(inspect.data["failed"], 0);
        assert_eq!(suites.len(), 2);
        assert_eq!(suites[0]["id"], "phase2-container");
        assert_eq!(suites[0]["requires_container_runtime"], true);
        assert_eq!(suites[0]["requires_qemu"], false);
        assert_eq!(suites[0]["qemu_only"], false);
        assert_eq!(suites[1]["id"], "phase3-qemu");
        assert_eq!(suites[1]["requires_container_runtime"], false);
        assert_eq!(suites[1]["requires_qemu"], true);
        assert_eq!(suites[1]["qemu_only"], true);
    }

    #[test]
    fn inventory_sorts_suites_by_id() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "z-suite.toml", &container_manifest("z", 1));
        write_manifest(root.path(), "a-suite.toml", &container_manifest("a", 1));

        let inspect = inspect_manifest_dir(root.path());
        let suites = inspect.data["suites"].as_array().unwrap();

        assert_eq!(suites[0]["id"], "a-suite");
        assert_eq!(suites[1]["id"], "z-suite");
    }

    #[test]
    fn invalid_toml_is_partial_when_one_manifest_parses() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "good.toml", &container_manifest("good", 1));
        write_manifest(root.path(), "bad.toml", "not = [valid");

        let inspect = inspect_manifest_dir(root.path());

        assert_eq!(inspect.envelope.status, OperationStatus::Partial);
        assert_eq!(inspect.data["toml_files"], 2);
        assert_eq!(inspect.data["parsed"], 1);
        assert_eq!(inspect.data["failed"], 1);
        assert!(
            inspect
                .data["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("bad.toml"))
        );
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("failed to parse"))
        );
    }

    #[test]
    fn all_invalid_toml_is_unavailable_when_no_manifest_parses() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "bad-a.toml", "not = [valid");
        write_manifest(root.path(), "bad-b.toml", "also = [broken");

        let inspect = inspect_manifest_dir(root.path());

        assert_eq!(inspect.envelope.status, OperationStatus::Unavailable);
        assert_eq!(inspect.data["toml_files"], 2);
        assert_eq!(inspect.data["parsed"], 0);
        assert_eq!(inspect.data["failed"], 2);
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("no parseable test manifests"))
        );
    }

    #[test]
    fn missing_manifest_dir_is_unavailable() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing");

        let inspect = inspect_manifest_dir(&missing);

        assert_eq!(inspect.envelope.status, OperationStatus::Unavailable);
        assert_eq!(inspect.data["dir_exists"], false);
        assert_eq!(inspect.data["parsed"], 0);
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("manifest directory is missing"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_manifest_dir_is_unavailable_with_error_data() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let blocked = root.path().join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        let original_permissions = std::fs::metadata(&blocked).unwrap().permissions();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let inspect = inspect_manifest_dir(&blocked);

        std::fs::set_permissions(&blocked, original_permissions).unwrap();

        assert_eq!(inspect.envelope.status, OperationStatus::Unavailable);
        assert_eq!(inspect.data["dir_exists"], true);
        assert!(
            inspect
                .data["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("unreadable"))
        );
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("unreadable"))
        );
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test -p conary-test suite_inventory
```

Expected: compile failure because `inspect_manifest_dir` and the inventory types do not exist yet.

- [ ] **Step 3: Replace skeleton with full implementation**

Replace `apps/conary-test/src/suite_inventory.rs` with this full file:

```rust
// conary-test/src/suite_inventory.rs
//! Suite manifest inventory for local conary-test agent resources.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use conary_agent_contract::{
    InspectResult, OperationEnvelope, OperationStatus, RiskLevel, test_suites,
};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuiteInventory {
    pub manifest_dir: String,
    pub dir_exists: bool,
    pub toml_files: usize,
    pub parsed: usize,
    pub failed: usize,
    pub suites: Vec<SuiteInventoryEntry>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuiteInventoryEntry {
    pub id: String,
    pub name: String,
    pub phase: u32,
    pub test_count: usize,
    pub requires_container_runtime: bool,
    pub requires_qemu: bool,
    pub qemu_only: bool,
}

pub fn inspect_manifest_dir(manifest_dir: &Path) -> InspectResult {
    let inventory = read_suite_inventory(manifest_dir);
    let mut envelope = OperationEnvelope::new(
        "conary-test.suites.inspect",
        OperationStatus::Ok,
        RiskLevel::ReadOnly,
        "Known conary-test suite manifests inspected",
    );
    envelope.subject = Some(test_suites());

    if !inventory.dir_exists {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "manifest directory is missing: {}",
            manifest_dir.display()
        ));
    } else if inventory
        .errors
        .iter()
        .any(|error| error.contains("unreadable"))
    {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "manifest directory is unreadable: {}",
            manifest_dir.display()
        ));
    } else if inventory.parsed == 0 {
        envelope.status = OperationStatus::Unavailable;
        envelope.warnings.push(format!(
            "no parseable test manifests found in {}",
            manifest_dir.display()
        ));
    } else if inventory.failed > 0 {
        envelope.status = OperationStatus::Partial;
        envelope.warnings.push(format!(
            "{} test manifest(s) failed to parse in {}",
            inventory.failed,
            manifest_dir.display()
        ));
    }

    let data = serde_json::to_value(&inventory)
        .expect("suite inventory should serialize to JSON");
    InspectResult::new(envelope).with_data(data)
}

pub fn read_suite_inventory(manifest_dir: &Path) -> SuiteInventory {
    let dir_exists = manifest_dir.is_dir();
    let mut inventory = SuiteInventory {
        manifest_dir: manifest_dir.display().to_string(),
        dir_exists,
        toml_files: 0,
        parsed: 0,
        failed: 0,
        suites: Vec::new(),
        errors: Vec::new(),
    };

    if !dir_exists {
        return inventory;
    }

    if directory_has_no_read_bits(manifest_dir) {
        inventory.errors.push(format!(
            "{}: manifest directory is unreadable",
            manifest_dir.display()
        ));
        return inventory;
    }

    let entries = match std::fs::read_dir(manifest_dir) {
        Ok(entries) => entries,
        Err(error) => {
            inventory.errors.push(format!(
                "{}: manifest directory is unreadable: {error}",
                manifest_dir.display()
            ));
            return inventory;
        }
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();

    for path in paths {
        inventory.toml_files += 1;
        let id = path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_string();

        match crate::config::load_manifest(&path) {
            Ok(manifest) => {
                inventory.parsed += 1;
                let requires_qemu = manifest_requires_qemu(&manifest);
                let qemu_only = manifest.is_qemu_only();
                inventory.suites.push(SuiteInventoryEntry {
                    id,
                    name: manifest.suite.name,
                    phase: manifest.suite.phase,
                    test_count: manifest.test.len(),
                    requires_container_runtime: !qemu_only,
                    requires_qemu,
                    qemu_only,
                });
            }
            Err(error) => {
                inventory.failed += 1;
                let file = path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("<unknown>");
                inventory.errors.push(format!("{file}: {error}"));
            }
        }
    }

    inventory.suites.sort_by(|left, right| left.id.cmp(&right.id));
    inventory
}

fn manifest_requires_qemu(manifest: &crate::config::TestManifest) -> bool {
    manifest
        .suite
        .setup
        .iter()
        .any(|step| step.qemu_boot.is_some())
        || manifest
            .test
            .iter()
            .any(|test| test.step.iter().any(|step| step.qemu_boot.is_some()))
}

#[cfg(unix)]
fn directory_has_no_read_bits(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    // Early-exit for directories intentionally made unreadable in tests.
    // Fall through to std::fs::read_dir for ordinary permission and platform
    // behavior so OS errors are still reported in the inventory.
    std::fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o444 == 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn directory_has_no_read_bits(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use conary_agent_contract::{OperationStatus, RiskLevel};
    use tempfile::tempdir;

    use super::*;

    fn write_manifest(dir: &Path, file_name: &str, body: &str) {
        std::fs::write(dir.join(file_name), body).unwrap();
    }

    fn container_manifest(name: &str, phase: u32) -> String {
        format!(
            r#"
[suite]
name = "{name}"
phase = {phase}

[[test]]
id = "T01"
name = "container_test"
description = "Container test"
timeout = 10

[[test.step]]
run = "true"
"#
        )
    }

    fn qemu_manifest(name: &str, phase: u32) -> String {
        format!(
            r#"
[suite]
name = "{name}"
phase = {phase}

[[test]]
id = "TQEMU"
name = "qemu_test"
description = "QEMU test"
timeout = 10

[[test.step]]
[test.step.qemu_boot]
image = "unused"
local_image_path = "/tmp/missing.qcow2"
commands = ["true"]
"#
        )
    }

    #[test]
    fn inventory_reads_valid_manifests_and_flags() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "phase2-container.toml", &container_manifest("container", 2));
        write_manifest(root.path(), "phase3-qemu.toml", &qemu_manifest("qemu", 3));

        let inspect = inspect_manifest_dir(root.path());
        let suites = inspect.data["suites"].as_array().unwrap();

        assert_eq!(inspect.envelope.operation, "conary-test.suites.inspect");
        assert_eq!(inspect.envelope.status, OperationStatus::Ok);
        assert_eq!(inspect.envelope.risk, RiskLevel::ReadOnly);
        assert_eq!(
            inspect.envelope.subject.as_ref().unwrap().uri,
            "conary-test://suites"
        );
        assert_eq!(inspect.data["dir_exists"], true);
        assert_eq!(inspect.data["toml_files"], 2);
        assert_eq!(inspect.data["parsed"], 2);
        assert_eq!(inspect.data["failed"], 0);
        assert_eq!(suites.len(), 2);
        assert_eq!(suites[0]["id"], "phase2-container");
        assert_eq!(suites[0]["requires_container_runtime"], true);
        assert_eq!(suites[0]["requires_qemu"], false);
        assert_eq!(suites[0]["qemu_only"], false);
        assert_eq!(suites[1]["id"], "phase3-qemu");
        assert_eq!(suites[1]["requires_container_runtime"], false);
        assert_eq!(suites[1]["requires_qemu"], true);
        assert_eq!(suites[1]["qemu_only"], true);
    }

    #[test]
    fn inventory_sorts_suites_by_id() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "z-suite.toml", &container_manifest("z", 1));
        write_manifest(root.path(), "a-suite.toml", &container_manifest("a", 1));

        let inspect = inspect_manifest_dir(root.path());
        let suites = inspect.data["suites"].as_array().unwrap();

        assert_eq!(suites[0]["id"], "a-suite");
        assert_eq!(suites[1]["id"], "z-suite");
    }

    #[test]
    fn invalid_toml_is_partial_when_one_manifest_parses() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "good.toml", &container_manifest("good", 1));
        write_manifest(root.path(), "bad.toml", "not = [valid");

        let inspect = inspect_manifest_dir(root.path());

        assert_eq!(inspect.envelope.status, OperationStatus::Partial);
        assert_eq!(inspect.data["toml_files"], 2);
        assert_eq!(inspect.data["parsed"], 1);
        assert_eq!(inspect.data["failed"], 1);
        assert!(
            inspect
                .data["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("bad.toml"))
        );
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("failed to parse"))
        );
    }

    #[test]
    fn all_invalid_toml_is_unavailable_when_no_manifest_parses() {
        let root = tempdir().unwrap();
        write_manifest(root.path(), "bad-a.toml", "not = [valid");
        write_manifest(root.path(), "bad-b.toml", "also = [broken");

        let inspect = inspect_manifest_dir(root.path());

        assert_eq!(inspect.envelope.status, OperationStatus::Unavailable);
        assert_eq!(inspect.data["toml_files"], 2);
        assert_eq!(inspect.data["parsed"], 0);
        assert_eq!(inspect.data["failed"], 2);
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("no parseable test manifests"))
        );
    }

    #[test]
    fn missing_manifest_dir_is_unavailable() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing");

        let inspect = inspect_manifest_dir(&missing);

        assert_eq!(inspect.envelope.status, OperationStatus::Unavailable);
        assert_eq!(inspect.data["dir_exists"], false);
        assert_eq!(inspect.data["parsed"], 0);
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("manifest directory is missing"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_manifest_dir_is_unavailable_with_error_data() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let blocked = root.path().join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        let original_permissions = std::fs::metadata(&blocked).unwrap().permissions();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let inspect = inspect_manifest_dir(&blocked);

        std::fs::set_permissions(&blocked, original_permissions).unwrap();

        assert_eq!(inspect.envelope.status, OperationStatus::Unavailable);
        assert_eq!(inspect.data["dir_exists"], true);
        assert!(
            inspect
                .data["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("unreadable"))
        );
        assert!(
            inspect
                .envelope
                .warnings
                .iter()
                .any(|warning| warning.contains("unreadable"))
        );
    }
}
```

- [ ] **Step 4: Verify Task 2**

Run:

```bash
cargo fmt --check
cargo test -p conary-test suite_inventory
```

Expected: all suite inventory tests pass.

- [ ] **Step 5: Commit Task 2**

Run:

```bash
git status --short
git add apps/conary-test/src/lib.rs apps/conary-test/src/suite_inventory.rs
git commit -m "feat(conary-test): inspect suite manifest inventory"
git status --short
```

## Task 3: Wire Suites Resource Into `/mcp/stateless`

**Files:**
- Modify: `apps/conary-test/src/server/stateless_mcp.rs`

- [ ] **Step 1: Add failing route tests first**

In `apps/conary-test/src/server/stateless_mcp.rs`, add these imports to the test module:

```rust
    use std::path::Path;

    use tempfile::tempdir;

    use crate::server::state::AppState;
```

Add these helper functions inside the test module:

```rust
    fn state_with_manifest_dir(manifest_dir: &Path) -> AppState {
        let mut state = test_fixtures::test_app_state();
        state.manifest_dir = manifest_dir.display().to_string();
        state
    }

    fn write_test_manifest(dir: &Path, file_name: &str, suite_name: &str, phase: u32) {
        std::fs::write(
            dir.join(file_name),
            format!(
                r#"
[suite]
name = "{suite_name}"
phase = {phase}

[[test]]
id = "T01"
name = "smoke"
description = "Smoke test"
timeout = 10

[[test.step]]
run = "true"
"#
            ),
        )
        .unwrap();
    }

    fn suites_resource_read_request(id: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/mcp/stateless")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
            .header(HEADER_METHOD, "resources/read")
            .header(HEADER_NAME, "conary-test://suites")
            .body(Body::from(
                serde_json::to_vec(&resource_read_body(id, "conary-test://suites")).unwrap(),
            ))
            .unwrap()
    }
```

Replace `resources_list_route_returns_bootstrap_status_resource` with:

```rust
    #[tokio::test]
    async fn resources_list_route_returns_bootstrap_and_suites_resources() {
        let app = create_router(test_fixtures::test_app_state(), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/stateless")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                    .header(HEADER_METHOD, "resources/list")
                    .body(Body::from(
                        serde_json::to_vec(&resource_list_body("resources-list-1")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["result"]["resultType"], "complete");
        assert_eq!(body["result"]["ttlMs"], 30_000);
        assert_eq!(body["result"]["cacheScope"], "private");
        let resources = body["result"]["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0]["uri"], "conary-local://bootstrap/status");
        assert_eq!(resources[0]["name"], "bootstrap_status");
        assert_eq!(resources[1]["uri"], "conary-test://suites");
        assert_eq!(resources[1]["name"], "conary_test_suites");
        assert_eq!(resources[1]["title"], "Conary-Test Suites");
        assert_eq!(
            resources[1]["description"],
            "Read the local conary-test suite manifest inventory"
        );
        assert_eq!(resources[1]["mimeType"], "application/json");
    }
```

Add these route tests after `resources_read_route_returns_bootstrap_inspect_json`:

```rust
    #[tokio::test]
    async fn resources_read_route_returns_suites_inspect_json_from_state_manifest_dir() {
        let root = tempdir().unwrap();
        write_test_manifest(root.path(), "phase1-core.toml", "phase1-core", 1);
        let app = create_router(state_with_manifest_dir(root.path()), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-read-1"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["result"]["resultType"], "complete");
        assert_eq!(body["result"]["ttlMs"], 30_000);
        assert_eq!(body["result"]["cacheScope"], "private");
        assert_eq!(body["result"]["contents"].as_array().unwrap().len(), 1);
        assert_eq!(body["result"]["contents"][0]["uri"], "conary-test://suites");
        assert_eq!(
            body["result"]["contents"][0]["mimeType"],
            "application/json"
        );

        let text = body["result"]["contents"][0]["text"]
            .as_str()
            .expect("resource content should be text");
        let payload: Value =
            serde_json::from_str(text).expect("suites resource text should be JSON");

        assert_eq!(payload["operation"], "conary-test.suites.inspect");
        assert_eq!(payload["subject"]["uri"], "conary-test://suites");
        assert_eq!(payload["risk"], "read_only");
        assert_eq!(payload["status"], "ok");
        assert_eq!(payload["data"]["manifest_dir"], root.path().display().to_string());
        assert_eq!(payload["data"]["parsed"], 1);
        assert_eq!(payload["data"]["suites"][0]["id"], "phase1-core");
    }

    #[tokio::test]
    async fn suites_resource_reports_partial_manifest_parse_state_inside_content() {
        let root = tempdir().unwrap();
        write_test_manifest(root.path(), "good.toml", "good", 1);
        std::fs::write(root.path().join("bad.toml"), "not = [valid").unwrap();
        let app = create_router(state_with_manifest_dir(root.path()), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-partial-1"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        let text = body["result"]["contents"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();

        assert_eq!(payload["status"], "partial");
        assert_eq!(payload["data"]["parsed"], 1);
        assert_eq!(payload["data"]["failed"], 1);
        assert!(
            payload["data"]["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("bad.toml"))
        );
    }

    #[tokio::test]
    async fn suites_resource_reports_all_failed_manifest_parse_state_inside_content() {
        let root = tempdir().unwrap();
        std::fs::write(root.path().join("bad-a.toml"), "not = [valid").unwrap();
        std::fs::write(root.path().join("bad-b.toml"), "also = [broken").unwrap();
        let app = create_router(state_with_manifest_dir(root.path()), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-all-failed-1"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        let text = body["result"]["contents"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();

        assert_eq!(payload["status"], "unavailable");
        assert_eq!(payload["data"]["parsed"], 0);
        assert_eq!(payload["data"]["failed"], 2);
        assert!(
            payload["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning.as_str().unwrap().contains("no parseable test manifests"))
        );
    }

    #[tokio::test]
    async fn suites_resource_reports_missing_manifest_dir_inside_content() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing");
        let app = create_router(state_with_manifest_dir(&missing), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-missing-1"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        let text = body["result"]["contents"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();

        assert_eq!(payload["status"], "unavailable");
        assert_eq!(payload["data"]["dir_exists"], false);
        assert_eq!(payload["data"]["parsed"], 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn suites_resource_reports_unreadable_manifest_dir_inside_content() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().unwrap();
        let blocked = root.path().join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        let original_permissions = std::fs::metadata(&blocked).unwrap().permissions();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let app = create_router(state_with_manifest_dir(&blocked), None);

        let response = app
            .oneshot(suites_resource_read_request("suites-unreadable-1"))
            .await
            .unwrap();

        std::fs::set_permissions(&blocked, original_permissions).unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        let text = body["result"]["contents"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();

        assert_eq!(payload["status"], "unavailable");
        assert_eq!(payload["data"]["dir_exists"], true);
        assert!(
            payload["data"]["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error.as_str().unwrap().contains("unreadable"))
        );
        assert!(
            payload["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning.as_str().unwrap().contains("unreadable"))
        );
    }
```

Add this test after `legacy_mcp_route_does_not_return_stateless_resource_list`:

```rust
    #[tokio::test]
    async fn legacy_mcp_route_does_not_return_stateless_suites_resource() {
        let app = create_router(
            test_fixtures::test_app_state(),
            Some(TEST_TOKEN.to_string()),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("authorization", format!("Bearer {TEST_TOKEN}"))
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header(HEADER_PROTOCOL_VERSION, MCP_DRAFT_PROTOCOL_VERSION)
                    .header(HEADER_METHOD, "resources/read")
                    .header(HEADER_NAME, "conary-test://suites")
                    .body(Body::from(
                        serde_json::to_vec(&resource_read_body(
                            "cross-wire-suites-1",
                            "conary-test://suites",
                        ))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        // The legacy rmcp endpoint may return non-JSON for malformed or
        // session-era requests. In that case this test passes because the
        // response is definitely not the stateless resource envelope.
        if let Ok(body) = try_read_json(response).await {
            assert!(
                body.get("result")
                    .and_then(|result| result.get("resultType"))
                    .is_none(),
                "legacy /mcp should not return stateless suites resource result"
            );
        }
    }
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test -p conary-test stateless_mcp::tests::resources_list_route_returns_bootstrap_and_suites_resources
cargo test -p conary-test stateless_mcp::tests::resources_read_route_returns_suites_inspect_json_from_state_manifest_dir
```

Expected:

- First command fails because `resources/list` still returns one descriptor.
- Second command fails because `conary-test://suites` is not registered.

- [ ] **Step 3: Wire `AppState` into handler and provider**

In `apps/conary-test/src/server/stateless_mcp.rs`, update imports:

```rust
use std::path::Path;

use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use conary_agent_contract::{local_bootstrap_status, test_suites};
use conary_mcp::stateless::{
    ImplementationInfo, MCP_DRAFT_PROTOCOL_VERSION, ResourceContent, ResourceDescriptor,
};
use conary_mcp::stateless_http::{
    HTTP_BAD_REQUEST, JSON_RPC_PARSE_ERROR, OriginPolicy, RawStatelessHttpConfig,
    RawStatelessHttpResponse, ResourceReadError, StatelessResourceProvider,
    handle_stateless_http_bytes_with_resources,
};
use serde_json::{Value, json};

use crate::server::state::AppState;
```

Remove `const BOOTSTRAP_STATUS_URI`.

Change the handler signature and provider construction:

```rust
pub async fn handle(State(state): State<AppState>, request: axum::http::Request<Body>) -> Response {
```

Replace the provider argument:

```rust
        &ConaryTestResourceProvider { state },
```

Update the instructions string in `stateless_config()`:

```rust
        instructions: Some(
            "Conary test infrastructure stateless MCP endpoint exposes discovery plus read-only bootstrap-status and suites resources."
                .to_string(),
        ),
```

Replace `BootstrapStatusResourceProvider` with this provider:

```rust
#[derive(Clone)]
struct ConaryTestResourceProvider {
    state: AppState,
}

impl StatelessResourceProvider for ConaryTestResourceProvider {
    fn list_resources(&self) -> Vec<ResourceDescriptor> {
        vec![bootstrap_status_descriptor(), suites_descriptor()]
    }

    fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContent>, ResourceReadError> {
        let bootstrap_uri = local_bootstrap_status().uri;
        if uri == bootstrap_uri {
            let inspect = crate::bootstrap::inspect_default();
            return Ok(vec![json_resource_content(uri, &inspect)]);
        }

        let suites_uri = test_suites().uri;
        if uri == suites_uri {
            let inspect = crate::suite_inventory::inspect_manifest_dir(Path::new(&self.state.manifest_dir));
            return Ok(vec![json_resource_content(uri, &inspect)]);
        }

        Err(ResourceReadError::NotFound {
            uri: uri.to_string(),
        })
    }
}

fn bootstrap_status_descriptor() -> ResourceDescriptor {
    ResourceDescriptor {
        uri: local_bootstrap_status().uri,
        name: "bootstrap_status".to_string(),
        title: Some("Local Bootstrap Status".to_string()),
        description: "Read local developer bootstrap prerequisites and smoke-readiness state"
            .to_string(),
        mime_type: "application/json".to_string(),
    }
}

fn suites_descriptor() -> ResourceDescriptor {
    ResourceDescriptor {
        uri: test_suites().uri,
        name: "conary_test_suites".to_string(),
        title: Some("Conary-Test Suites".to_string()),
        description: "Read the local conary-test suite manifest inventory".to_string(),
        mime_type: "application/json".to_string(),
    }
}

fn json_resource_content(
    uri: &str,
    inspect: &impl serde::Serialize,
) -> ResourceContent {
    let text = serde_json::to_string_pretty(inspect)
        .expect("Conary InspectResult should serialize to JSON");

    ResourceContent {
        uri: uri.to_string(),
        mime_type: "application/json".to_string(),
        text,
    }
}
```

Run `cargo fmt` after this step. If rustfmt wraps the long `inspect_manifest_dir` line, keep the rustfmt output.

- [ ] **Step 4: Verify Task 3**

Run:

```bash
cargo fmt --check
cargo test -p conary-test stateless_mcp
cargo test -p conary-test mcp_endpoint_requires_token
```

Expected: all commands pass.

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git status --short
git add apps/conary-test/src/server/stateless_mcp.rs
git commit -m "feat(conary-test): expose suites stateless resource"
git status --short
```

## Task 4: Update Docs And Scope Guards

**Files:**
- Modify: `apps/conary-test/src/server/stateless_mcp.rs`
- Modify: `apps/conary-test/README.md`
- Modify: `docs/operations/agent-mcp-adapter-decision.md`
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Add failing scope guard tests**

Add these tests to the `#[cfg(test)] mod tests` block in `apps/conary-test/src/server/stateless_mcp.rs`:

```rust
    #[test]
    fn remi_files_do_not_expose_conary_test_suites_resource() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for path in [
            "apps/remi/src/server/mcp.rs",
            "apps/remi/src/server/routes/mcp.rs",
        ] {
            let source = std::fs::read_to_string(root.join(path))
                .expect("Remi MCP file should be readable");
            assert!(
                !source.contains("conary-test://suites")
                    && !source.contains("conary_test_suites"),
                "{path} must not expose the conary-test suites stateless resource"
            );
        }
    }

    #[test]
    fn stateless_route_does_not_register_tools_prompts_templates_or_sse() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let source = std::fs::read_to_string(
            root.join("apps/conary-test/src/server/stateless_mcp.rs"),
        )
        .expect("conary-test stateless route file should be readable");

        for forbidden in [
            concat!("tools", "/call"),
            concat!("prompts", "/get"),
            concat!("resources", "/templates", "/list"),
            concat!("subscriptions", "/listen"),
            concat!("notifications", "/resources"),
            concat!("Event", "Stream"),
            concat!("S", "se"),
        ] {
            assert!(
                !source.contains(forbidden),
                "stateless route must not register unsupported MCP surface {forbidden}"
            );
        }
    }
```

- [ ] **Step 2: Run guard tests**

Run:

```bash
cargo test -p conary-test stateless_route_does_not_register_tools_prompts_templates_or_sse
cargo test -p conary-test remi_files_do_not_expose_conary_test_suites_resource
```

Expected: both pass immediately. They are regression guards for the remaining documentation work and future edits.

- [ ] **Step 3: Update `apps/conary-test/README.md`**

Replace the current stateless preview paragraph with:

```markdown
The legacy MCP endpoint is mounted at `/mcp` through `rmcp`'s session-based
Streamable HTTP transport. The draft stateless preview endpoint is mounted at
`/mcp/stateless` and supports `server/discover`, `resources/list`, and
`resources/read` for two read-only resources:
`conary-local://bootstrap/status` and `conary-test://suites`. Bootstrap status
returns the same structured `InspectResult` used by
`conary-test bootstrap check --json`. The suites resource returns a static
manifest inventory from the server's configured manifest directory. The
stateless preview does not expose live tools, prompts, resource templates,
subscriptions, SSE streaming, mutations, or smoke execution.
```

Also add this row to the architecture table after `src/bootstrap.rs`:

```markdown
| `src/suite_inventory.rs` | Suite manifest inventory for agent-facing read-only resources |
```

- [ ] **Step 4: Update adapter decision doc**

In `docs/operations/agent-mcp-adapter-decision.md`, change `revision: 6` to:

```yaml
revision: 7
```

Change the summary line to:

```yaml
summary: Decision record for Conary's stateless MCP adapter path, compliance harness, raw HTTP proof, and conary-test read-only resources
```

Replace the current-state bullet for `apps/conary-test` with:

```markdown
- `apps/conary-test` exposes `POST /mcp/stateless` as the first live
  stateless adapter gate. It handles `server/discover`, `resources/list`, and
  `resources/read` for `conary-local://bootstrap/status` and
  `conary-test://suites`, and keeps the legacy `/mcp` session-based tool
  surface unchanged.
```

Replace the "Current Choice" paragraph that starts with `After the contract` with:

```markdown
After the contract, catalog, local bootstrap, compliance harness, non-live raw
proof, and first live resource slices, the selected live adapter-gate surface
is a `conary-test` route at `POST /mcp/stateless`. It exposes
`server/discover` plus read-only `conary-local://bootstrap/status` and
`conary-test://suites` resources, and advertises no tools or prompts.
```

Replace the conary-test slice resource paragraph with:

```markdown
This route supports `server/discover`, `resources/list`, and `resources/read`
for `conary-local://bootstrap/status` and `conary-test://suites`. Bootstrap
status returns the existing local bootstrap `InspectResult` as
`application/json` text content. The suites resource returns a static
manifest-inventory `InspectResult` from `AppState.manifest_dir`. It must not
add tools, prompts, resource templates, subscriptions, SSE, smoke execution,
per-suite resources, or Remi route behavior.
```

Add this source spec to the source-spec list:

```markdown
- `docs/superpowers/specs/2026-05-24-conary-test-suites-resource-design.md`
```

- [ ] **Step 5: Update infrastructure doc**

In `docs/operations/infrastructure.md`, change `revision: 7` to:

```yaml
revision: 8
```

Replace the `Agent Operations And MCP` opening paragraph with:

```markdown
Today, the live Remi MCP endpoint and the legacy `conary-test` `/mcp` endpoint
are session-based, tool-only surfaces. `conary-test` also exposes
`/mcp/stateless` as a draft stateless preview route with `server/discover`,
`resources/list`, and `resources/read` for
`conary-local://bootstrap/status` and `conary-test://suites`. Those resources
are read-only local bootstrap and suite-manifest state. The stateless preview
does not expose live tools, prompts, resource templates, subscriptions, SSE
streaming, mutations, or smoke execution.
```

- [ ] **Step 6: Refresh docs audit files**

Run:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Then run this Python reconciliation script:

```bash
python - <<'PY'
from pathlib import Path

ledger_path = Path("docs/superpowers/documentation-accuracy-audit-ledger.tsv")
inventory_path = Path("docs/superpowers/documentation-accuracy-audit-inventory.tsv")

inventory_lines = inventory_path.read_text().splitlines()
inventory_header = inventory_lines[0].split("\t")
inventory = {}
for line in inventory_lines[1:]:
    path, family, audience = line.split("\t")
    inventory[path] = {"family": family, "audience": audience}

ledger_lines = ledger_path.read_text().splitlines()
header = ledger_lines[0].split("\t")
rows = {}
for line in ledger_lines[1:]:
    values = line.split("\t")
    row = dict(zip(header, values))
    if row["path"] in inventory:
        rows[row["path"]] = row

def upsert(path, claim_clusters, evidence_sources, disposition, notes):
    if path not in inventory:
        raise SystemExit(f"upserted path not in refreshed inventory: {path}")
    rows[path] = {
        "origin_path": path,
        "path": path,
        "family": inventory[path]["family"],
        "audience": inventory[path]["audience"],
        "claim_clusters": claim_clusters,
        "evidence_sources": evidence_sources,
        "status": "verified",
        "disposition": disposition,
        "notes": notes,
    }

upsert(
    "apps/conary-test/README.md",
    "mcp; stateless-preview; bootstrap-status; suites-resource",
    "apps/conary-test/src/server/stateless_mcp.rs; apps/conary-test/src/suite_inventory.rs; apps/conary-test/src/bootstrap.rs",
    "corrected",
    "Updated stateless MCP route docs for bootstrap/status and conary-test://suites read-only resources.",
)
upsert(
    "docs/operations/agent-mcp-adapter-decision.md",
    "mcp; stateless-adapter; bootstrap-status; suites-resource",
    "crates/conary-mcp/src/stateless_http.rs; apps/conary-test/src/server/stateless_mcp.rs; apps/conary-test/src/suite_inventory.rs; docs/superpowers/specs/2026-05-24-conary-test-suites-resource-design.md",
    "corrected",
    "Updated adapter decision current state for conary-test bootstrap/status and suites resources while Remi remains unwired.",
)
upsert(
    "docs/operations/infrastructure.md",
    "mcp; operations; bootstrap-status; suites-resource",
    "apps/conary-test/src/server/stateless_mcp.rs; apps/conary-test/src/suite_inventory.rs; docs/operations/agent-mcp-adapter-decision.md",
    "corrected",
    "Updated agent operations guidance to record bootstrap/status and suites stateless preview resources.",
)
upsert(
    "docs/superpowers/plans/2026-05-24-conary-test-suites-resource.md",
    "planning; mcp; conary-test-suites",
    "docs/superpowers/specs/2026-05-24-conary-test-suites-resource-design.md; apps/conary-test/src/server/stateless_mcp.rs; apps/conary-test/src/bootstrap.rs; apps/conary-test/src/config/manifest.rs",
    "corrected",
    "Added goal-ready implementation plan for the conary-test://suites stateless resource slice.",
)

ordered_paths = sorted(inventory)
output = ["\t".join(header)]
for path in ordered_paths:
    row = rows.get(path)
    if row is None:
        raise SystemExit(f"missing ledger row for inventory path: {path}")
    output.append("\t".join(row[column] for column in header))

ledger_path.write_text("\n".join(output) + "\n")
PY
```

- [ ] **Step 7: Verify docs and guards**

Run:

```bash
cargo fmt --check
cargo test -p conary-test stateless_mcp
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
rg -n "conary-test://suites|conary_test_suites" docs/operations apps/conary-test/README.md
! rg -n "conary-test://suites|conary_test_suites" apps/remi
! rg -n "tools/call|prompts/get|resources/templates/list|subscriptions/listen|notifications/resources|EventStream|Sse" apps/conary-test/src/server/stateless_mcp.rs
git diff --check
```

Expected:

- `cargo fmt --check` passes.
- `cargo test -p conary-test stateless_mcp` passes.
- Inventory diff prints nothing.
- Ledger check passes.
- Positive `rg` finds updated docs and `apps/conary-test/README.md`.
- Negative `rg` commands return no hits.
- `git diff --check` passes.

- [ ] **Step 8: Commit Task 4**

Run:

```bash
git status --short
git add apps/conary-test/src/server/stateless_mcp.rs apps/conary-test/README.md docs/operations/agent-mcp-adapter-decision.md docs/operations/infrastructure.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(mcp): document conary-test suites resource"
git status --short
```

## Task 5: Final Acceptance

**Files:**
- No new files; this task verifies the whole slice and marks the plan complete.

- [ ] **Step 1: Run final verification**

Run these commands from the repository root:

```bash
cargo fmt --check
cargo test -p conary-agent-contract
cargo test -p conary-test suite_inventory
cargo test -p conary-test stateless_mcp
cargo test -p conary-test bootstrap
cargo test -p conary-test mcp_endpoint_requires_token
cargo clippy --workspace --all-targets -- -D warnings
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
rg -n "conary-test://suites|conary_test_suites" docs/operations apps/conary-test/README.md
! rg -n "conary-test://suites|conary_test_suites" apps/remi
! rg -n "tools/call|prompts/get|resources/templates/list|subscriptions/listen|notifications/resources|EventStream|Sse" apps/conary-test/src/server/stateless_mcp.rs
git diff --check
```

Expected: all commands pass exactly as written.

- [ ] **Step 2: Update this plan checkbox state**

Change every completed task checkbox in this file from `- [ ]` to `- [x]`.

- [ ] **Step 3: Commit final plan update**

Run:

```bash
git status --short
git add docs/superpowers/plans/2026-05-24-conary-test-suites-resource.md
git commit -m "docs(plan): complete conary-test suites resource plan"
git status --short
```

Expected: worktree is clean after the final commit.
