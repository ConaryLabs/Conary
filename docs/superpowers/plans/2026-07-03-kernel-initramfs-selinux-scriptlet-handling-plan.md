# Kernel Initramfs SELinux Scriptlet Handling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the MVP from the 2026-07-03 design: preserve and surface boot/security scriptlet intent evidence while keeping kernel/initramfs/SELinux packages blocked for public Remi.

**Architecture:** Keep classification ownership in `crates/conary-core/src/ccs/convert/adapters.rs`, project sanitized command evidence through the passive legacy bundle, and expose it in Remi publication refusal reports. Do not add new CCS v2 lifecycle authority fields in this MVP; later native handling should extend `TargetProfileQuery` and supported-profile facts in a separate plan.

**Tech Stack:** Rust, serde, TOML/JSON DTOs, cargo test, Remi publication tests.

---

## Baseline Already In This Branch

The current `issue35-remi-diagnostics` branch already improves Remi/client diagnostics for issue #35:

- client-side Remi polling treats `review-required` and `blocked` as terminal actionable errors;
- direct 403/409 publication refusal JSON is pretty-printed instead of shown as raw JSON;
- HTTP client builder failures mention minimal-root inputs such as resolver files and TLS trust roots;
- Remi blocked messages name blocked classes.

Do not reimplement those diagnostics in this MVP. This plan builds on them by carrying better boot/security intent evidence through conversion and publication reports.

## File Structure

- Modify `crates/conary-core/src/ccs/convert/effects.rs`
  - Add a reusable sanitized command evidence DTO.
  - Attach optional command evidence to `Review` and `Blocked` classifications.
- Modify `crates/conary-core/src/ccs/convert/command_evidence.rs`
  - Add a stable source-label helper for command evidence.
- Modify `crates/conary-core/src/ccs/convert/adapters.rs`
  - Populate command evidence when a blocked/review class is matched.
  - Keep the existing adapter registry boundary. This file is over 1500 lines; do not split it in this MVP.
- Modify `crates/conary-core/src/ccs/convert/blocked_classes.rs`
  - Add missing boot/security tool coverage for `kernel-install`, `semodule`, and `fixfiles`.
- Modify `crates/conary-core/src/ccs/convert/converter.rs`
  - Update synthetic review/blocked classification constructors and match patterns.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs`
  - Update review/blocked classification match patterns and intentionally exclude diagnostic command evidence from conversion evidence digests.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/format_metadata.rs`
  - Update synthetic review classification constructors.
- Modify `crates/conary-core/src/ccs/legacy_scriptlets.rs`
  - Add additive, serde-defaulted `BootSecurityIntentEvidence`.
  - Add `boot_security_intents` to `LegacyScriptletEntry`.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`
  - Project boot/security evidence from classifications into entry outcomes.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`
  - Store entry outcome boot/security evidence on `LegacyScriptletEntry`.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs`
  - Add `boot_security_intents` to `ScriptletBundleSummary`.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`
  - Aggregate entry boot/security evidence into summaries.
- Modify `crates/conary-core/src/db/models/converted.rs`
  - Include `boot_security_intents` in summary JSON shape validation.
- Modify `apps/remi/src/server/publication.rs`
  - Include summary boot/security evidence in `PublicationGateReport`.
- Modify `crates/conary-core/src/repository/remi.rs`
  - Mirror the Remi refusal DTO field for client-side pretty-printing.
- Modify `docs/SCRIPTLET_SECURITY.md`
  - Document that boot/security scriptlets are classified and preserved but remain blocked for public serving.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Keep the new spec and this plan registered.

## Task 1: Add Sanitized Command Evidence To Classifications

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/effects.rs`
- Modify: `crates/conary-core/src/ccs/convert/command_evidence.rs`
- Modify: `crates/conary-core/src/ccs/convert/adapters.rs`
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/format_metadata.rs`

- [ ] **Step 1: Write failing classification tests**

Add this test in `crates/conary-core/src/ccs/convert/adapters.rs` near the other adapter registry tests:

```rust
#[test]
fn blocked_boot_security_classes_carry_command_evidence() {
    let registry = AdapterRegistry::default();

    for (command, args, class_id, expected_argv) in [
        (
            "depmod",
            vec!["6.10.0"],
            "kernel-module",
            vec!["<kver>"],
        ),
        (
            "kernel-install",
            vec!["add", "6.10.0", "/lib/modules/6.10.0/vmlinuz"],
            "kernel-module",
            vec!["add", "<kver>", "/lib/modules/<kver>/vmlinuz"],
        ),
        (
            "dracut",
            vec!["--force", "/boot/initramfs.img"],
            "initramfs",
            vec!["--force", "<boot>/initramfs.img"],
        ),
        (
            "restorecon",
            vec!["-R", "/usr/lib/modules"],
            "selinux",
            vec!["-R", "/usr/lib/modules"],
        ),
    ] {
        let classification = registry.classify_invocation(&invocation(command, &args));
        match classification {
            ScriptletClassification::Blocked {
                class_id: actual_class,
                command: Some(evidence),
                ..
            } => {
                assert_eq!(actual_class, class_id);
                assert_eq!(evidence.command, command);
                assert_eq!(evidence.argv, expected_argv);
                assert_eq!(evidence.source, "static-signal");
                assert!(evidence.environment.is_empty());
            }
            other => panic!("expected blocked evidence for {command}, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p conary-core blocked_boot_security_classes_carry_command_evidence
```

Expected: FAIL because `ScriptletClassification::Blocked` does not yet carry command evidence.

- [ ] **Step 3: Add stable command evidence DTO and enum fields**

In `crates/conary-core/src/ccs/convert/command_evidence.rs`, add a stable label helper. Do not use `Debug` output for persisted or client-visible evidence labels.

```rust
impl CommandEvidenceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StaticSignal => "static-signal",
            Self::CaptureLog => "capture-log",
            Self::NativeMetadata => "native-metadata",
            Self::PayloadHeuristic => "payload-heuristic",
            Self::CuratedRule => "curated-rule",
        }
    }
}
```

In `crates/conary-core/src/ccs/convert/effects.rs`, add the import and DTO:

```rust
use crate::ccs::convert::command_evidence::CommandInvocation;
```

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletCommandEvidence {
    pub command: String,
    pub argv: Vec<String>,
    pub phase: Option<String>,
    pub lifecycle_paths: Vec<String>,
    pub raw_line: Option<String>,
    pub source: String,
    pub environment: Vec<String>,
}

impl ScriptletCommandEvidence {
    pub fn from_invocation(invocation: &CommandInvocation) -> Self {
        Self {
            command: invocation.command.clone(),
            argv: sanitize_command_argv(&invocation.argv),
            phase: invocation.phase.clone(),
            lifecycle_paths: invocation.lifecycle_paths.clone(),
            raw_line: invocation.raw_line.clone(),
            source: invocation.source.as_str().to_string(),
            environment: invocation
                .environment
                .iter()
                .map(|fact| fact.name.clone())
                .collect(),
        }
    }
}

fn sanitize_command_argv(argv: &[String]) -> Vec<String> {
    argv.iter().map(|arg| sanitize_command_arg(arg)).collect()
}

fn sanitize_command_arg(arg: &str) -> String {
    if let Some(rest) = arg.strip_prefix("/boot/") {
        return format!("<boot>/{rest}");
    }
    arg.split('/')
        .map(|segment| {
            if looks_like_kernel_version(segment) {
                "<kver>"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn looks_like_kernel_version(segment: &str) -> bool {
    let mut parts = segment.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch))
            if major.chars().all(|ch| ch.is_ascii_digit())
                && minor.chars().all(|ch| ch.is_ascii_digit())
                && patch.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    )
}
```

Change `ScriptletClassification`:

```rust
Review {
    reason_code: String,
    class_id: Option<String>,
    command: Option<ScriptletCommandEvidence>,
},
Blocked {
    reason_code: String,
    class_id: String,
    command: Option<ScriptletCommandEvidence>,
},
```

Update existing tests and constructors in the repo to set `command: None` for synthetic review/blocked classifications, and update destructuring patterns to include `..` where the command field is irrelevant.

Use this inventory before implementation:

```bash
rg -n "ScriptletClassification::(Blocked|Review)" crates/conary-core/src apps/conary/src apps/conary/tests -g '*.rs'
```

At minimum, update these current call sites:

- `crates/conary-core/src/ccs/convert/effects.rs` test constructors and `ScriptletClassificationReport::push()`;
- `crates/conary-core/src/ccs/convert/adapters.rs` blocked/review constructors, `review_classification()`, and adapter tests;
- `crates/conary-core/src/ccs/convert/converter.rs` native-support constructors and test match patterns;
- `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs` reason/class extraction patterns;
- `crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs` reason extraction, digest serialization, and tests;
- `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs` test constructors;
- `crates/conary-core/src/ccs/convert/scriptlet_bundle/format_metadata.rs` test constructors.

Diagnostic command evidence must not participate in the conversion evidence digest in this MVP. Keep `digest.rs` ordering and serialization based on reason codes, adapter evidence, and effect evidence; match the new field with `..` unless a later signed-evidence design deliberately changes the digest contract.

- [ ] **Step 4: Populate evidence from the adapter registry**

In `AdapterRegistry::classify_invocation_with_context()` in `crates/conary-core/src/ccs/convert/adapters.rs`, change the blocked-class branch to attach evidence:

```rust
if let Some(class) = self.blocked_classes.match_invocation(input.invocation) {
    let command = Some(ScriptletCommandEvidence::from_invocation(input.invocation));
    return match class.default_outcome {
        BlockedClassOutcome::Blocked => ScriptletClassification::Blocked {
            reason_code: class.reason_code.to_string(),
            class_id: class.id.to_string(),
            command,
        },
        BlockedClassOutcome::Review => ScriptletClassification::Review {
            reason_code: class.reason_code.to_string(),
            class_id: Some(class.id.to_string()),
            command,
        },
    };
}
```

Change the existing effects import in `adapters.rs` to:

```rust
use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletCommandEvidence, ScriptletEffectEvidence,
};
```

In `ScriptletClassificationReport::push()` in `effects.rs`, update the review and blocked match arms to ignore the new command field:

```rust
ScriptletClassification::Review { class_id, .. } => {
    self.review_count += 1;
    if let Some(class_id) = class_id {
        increment_class_count(&mut self.unsupported_class_counts, class_id);
    }
}
ScriptletClassification::Blocked { class_id, .. } => {
    self.blocked_count += 1;
    increment_class_count(&mut self.unsupported_class_counts, class_id);
}
```

Update helper functions such as `review_classification()` in `adapters.rs` to set `command: None`.

- [ ] **Step 5: Verify the focused test passes**

Run:

```bash
cargo test -p conary-core blocked_boot_security_classes_carry_command_evidence
cargo test -p conary --test bundle_replay
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: all PASS. Running `bundle_replay` and `golden_conversion` here catches enum-shape and classification-digest regressions immediately after the `ScriptletClassification` variant change.

## Task 2: Expand Boot/Security Blocked-Class Coverage

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/blocked_classes.rs`

- [ ] **Step 1: Write failing blocked-class coverage test**

Add to `blocked_classes.rs` tests:

```rust
#[test]
fn blocked_classes_cover_kernel_install_selinux_module_and_label_tools() {
    let registry = BlockedClassRegistry::default();

    for (command, argv, class_id) in [
        ("kernel-install", vec!["add", "6.10.0", "/lib/modules/6.10.0/vmlinuz"], "kernel-module"),
        ("semodule", vec!["-i", "/tmp/demo.pp"], "selinux"),
        ("fixfiles", vec!["restore"], "selinux"),
    ] {
        let class = registry
            .match_invocation(&invocation(command, &argv))
            .unwrap_or_else(|| panic!("missing blocked class for {command}"));
        assert_eq!(class.id, class_id);
        assert_eq!(class.default_outcome, BlockedClassOutcome::Blocked);
    }
}
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p conary-core blocked_classes_cover_kernel_install_selinux_module_and_label_tools
```

Expected: FAIL because `kernel-install`, `semodule`, and `fixfiles` are not currently listed.

- [ ] **Step 3: Extend the kernel-module and SELinux command lists**

In the `kernel-module` blocked class definition, change:

```rust
&["modprobe", "depmod", "dkms"],
```

to:

```rust
&["modprobe", "depmod", "dkms", "kernel-install"],
```

In the `selinux` blocked class definition, change:

```rust
&["restorecon", "semanage", "setsebool"],
```

to:

```rust
&["restorecon", "fixfiles", "semanage", "semodule", "setsebool"],
```

- [ ] **Step 4: Verify blocked-class coverage**

Run:

```bash
cargo test -p conary-core blocked_classes_cover_kernel_install_selinux_module_and_label_tools
cargo test -p conary-core blocked_class
```

Expected: both commands PASS.

## Task 3: Project Boot/Security Evidence Into Legacy Bundles

**Files:**
- Modify: `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`
- Modify direct `LegacyScriptletEntry` constructors found by `rg -n "LegacyScriptletEntry \\{" crates apps -g '*.rs'`.

- [ ] **Step 1: Write failing bundle projection test**

Add this test in `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`:

```rust
#[test]
fn blocked_boot_security_evidence_is_stored_on_bundle_entry() {
    let mut metadata = package_metadata("kernelish", "1.0");
    metadata.scriptlets.push(Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "dracut --force /boot/initramfs.img\n".to_string(),
        flags: None,
    });
    let mut classification = ScriptletClassificationReport::default();
    classification.push(
        "scriptlet:0:post-install",
        ScriptletClassification::Blocked {
            reason_code: "blocked-class-initramfs".to_string(),
            class_id: "initramfs".to_string(),
            command: Some(crate::ccs::convert::effects::ScriptletCommandEvidence {
                command: "dracut".to_string(),
                argv: vec!["--force".to_string(), "<boot>/initramfs.img".to_string()],
                phase: Some("post-install".to_string()),
                lifecycle_paths: vec!["post-install".to_string()],
                raw_line: Some("dracut --force /boot/initramfs.img".to_string()),
                source: "static-signal".to_string(),
                environment: Vec::new(),
            }),
        },
    );

    let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();
    let entry = &build.bundle.entries[0];

    assert_eq!(entry.boot_security_intents.len(), 1);
    assert_eq!(entry.boot_security_intents[0].class_id, "initramfs");
    assert_eq!(entry.boot_security_intents[0].command, "dracut");
    assert_eq!(
        entry.boot_security_intents[0].argv,
        vec!["--force", "<boot>/initramfs.img"]
    );
}
```

Add a second assertion case in the same test module proving synthetic blocked classifications without command evidence remain safe and do not create boot/security intent evidence:

```rust
#[test]
fn blocked_boot_security_class_without_command_evidence_is_safe() {
    let mut metadata = package_metadata("synthetic-block", "1.0");
    metadata.scriptlets.push(Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "echo synthetic\n".to_string(),
        flags: None,
    });
    let mut classification = ScriptletClassificationReport::default();
    classification.push(
        "scriptlet:0:post-install",
        ScriptletClassification::Blocked {
            reason_code: "blocked-class-initramfs".to_string(),
            class_id: "initramfs".to_string(),
            command: None,
        },
    );

    let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();

    assert_eq!(build.bundle.entries[0].decision, ScriptletDecision::Blocked);
    assert!(build.bundle.entries[0].boot_security_intents.is_empty());
}
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p conary-core blocked_boot_security_evidence_is_stored_on_bundle_entry
```

Expected: FAIL because `LegacyScriptletEntry` has no `boot_security_intents`.

- [ ] **Step 3: Add additive legacy bundle evidence type**

In `crates/conary-core/src/ccs/legacy_scriptlets.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BootSecurityIntentEvidence {
    pub class_id: String,
    pub reason_code: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lifecycle_paths: Vec<String>,
}
```

Add this field to `LegacyScriptletEntry`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub boot_security_intents: Vec<BootSecurityIntentEvidence>,
```

When constructing existing entries in tests or code, set `boot_security_intents: Vec::new()` unless using the new projected evidence. Run this inventory before editing:

```bash
rg -n "LegacyScriptletEntry \\{" crates apps -g '*.rs'
```

Current direct constructor sites that must be updated include:

- `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`;
- `crates/conary-core/src/ccs/archive_reader.rs`;
- `crates/conary-core/src/ccs/legacy_replay.rs` tests;
- `crates/conary-core/src/ccs/legacy_scriptlets.rs` tests;
- `crates/conary-core/src/db/models/installed_legacy_scriptlet_bundle.rs` tests;
- `apps/conary/src/commands/install/legacy_replay.rs` tests;
- `apps/conary/src/commands/model/test_support.rs`;
- `apps/conary/src/commands/remove/autoremove.rs` tests;
- `apps/conary/src/commands/query/scripts.rs` tests;
- `apps/conary/src/commands/state.rs` tests;
- `apps/conary/src/commands/system.rs` tests;
- `apps/conary/src/commands/update/package.rs` tests;
- `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`;
- `apps/conary/tests/query_scripts.rs`.

Do not rely on `serde(default)` to satisfy Rust struct literals; every direct constructor must compile with the new field.

- [ ] **Step 4: Carry evidence through entry outcomes**

In `scriptlet_bundle/classification.rs`, import `BootSecurityIntentEvidence` and `ScriptletCommandEvidence`, add a field to `EntryOutcome`, and collect boot/security intents:

```rust
pub(super) struct EntryOutcome {
    pub(super) decision: ScriptletDecision,
    pub(super) reason_code: String,
    pub(super) effects: Vec<ScriptletEffect>,
    pub(super) unknown_commands: Vec<String>,
    pub(super) blocked_classes: Vec<String>,
    pub(super) boot_security_intents: Vec<BootSecurityIntentEvidence>,
}
```

Use this helper in the same file:

```rust
fn boot_security_intent_from_classification(
    classification: &ScriptletClassification,
) -> Option<BootSecurityIntentEvidence> {
    match classification {
        ScriptletClassification::Blocked {
            reason_code,
            class_id,
            command: Some(command),
        }
        | ScriptletClassification::Review {
            reason_code,
            class_id: Some(class_id),
            command: Some(command),
        } if is_boot_security_class(class_id) => Some(intent_from_command(
            class_id,
            reason_code,
            command,
        )),
        _ => None,
    }
}

fn intent_from_command(
    class_id: &str,
    reason_code: &str,
    command: &ScriptletCommandEvidence,
) -> BootSecurityIntentEvidence {
    BootSecurityIntentEvidence {
        class_id: class_id.to_string(),
        reason_code: reason_code.to_string(),
        command: command.command.clone(),
        argv: command.argv.clone(),
        phase: command.phase.clone(),
        lifecycle_paths: command.lifecycle_paths.clone(),
    }
}

fn is_boot_security_class(class_id: &str) -> bool {
    matches!(
        class_id,
        "kernel-module" | "initramfs" | "bootloader" | "selinux"
    )
}
```

Keep the `None` case explicit: synthetic blocked/review classifications can exist in tests and metadata-only support paths, and missing command evidence should omit detailed boot/security intent evidence without panicking.

Do not add `apparmor` to `is_boot_security_class()` in this MVP. AppArmor remains blocked by the existing blocked-class registry, but detailed AppArmor evidence is a separate follow-up unless a new issue or design scopes it in.

At the top of `classify_entry()`, collect:

```rust
let boot_security_intents = classifications
    .iter()
    .filter_map(|entry| boot_security_intent_from_classification(&entry.classification))
    .collect::<Vec<_>>();
```

Set `boot_security_intents` in every `EntryOutcome` return.

- [ ] **Step 5: Store outcome evidence on entries**

In `scriptlet_bundle/entries.rs`, set:

```rust
boot_security_intents: outcome.boot_security_intents,
```

in both `LegacyScriptletEntry` constructors.

- [ ] **Step 6: Verify projection test passes**

Run:

```bash
cargo test -p conary-core blocked_boot_security_evidence_is_stored_on_bundle_entry
```

Expected: PASS.

## Task 4: Add Boot/Security Evidence To Scriptlet Summaries

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`
- Modify: `crates/conary-core/src/db/models/converted.rs`

- [ ] **Step 1: Write failing summary test**

Add this assertion to the Task 3 test immediately after the `bundle_for_metadata()` build call:

```rust
assert_eq!(build.summary.boot_security_intents.len(), 1);
assert_eq!(build.summary.boot_security_intents[0].class_id, "initramfs");
assert_eq!(build.summary.boot_security_intents[0].command, "dracut");
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p conary-core blocked_boot_security_evidence_is_stored_on_bundle_entry
```

Expected: FAIL because `ScriptletBundleSummary` has no `boot_security_intents`.

- [ ] **Step 3: Add summary field**

In `scriptlet_bundle/types.rs`, import `BootSecurityIntentEvidence` and add the field:

```rust
use crate::ccs::legacy_scriptlets::{BootSecurityIntentEvidence, LegacyScriptletBundle};
```

```rust
#[serde(default)]
pub boot_security_intents: Vec<BootSecurityIntentEvidence>,
```

Set it to `Vec::new()` in `Default`.

- [ ] **Step 4: Aggregate summary evidence**

In `summary_from_bundle()` in `scriptlet_bundle/summary.rs`, collect sorted evidence before constructing the summary:

```rust
let boot_security_intents = bundle
    .entries
    .iter()
    .flat_map(|entry| entry.boot_security_intents.iter().cloned())
    .collect::<Vec<_>>();
```

Set:

```rust
boot_security_intents,
```

on `ScriptletBundleSummary`.

- [ ] **Step 5: Update converted-row summary shape validation**

In `crates/conary-core/src/db/models/converted.rs`, update `summary_json_shape_valid_for_publication()` so the required summary keys include:

```rust
"boot_security_intents",
```

Old rows should continue to deserialize through `serde(default)`, but newly persisted summary JSON should carry the field so publication-shape checks prove the evidence surface is present.

- [ ] **Step 6: Verify summary projection**

Run:

```bash
cargo test -p conary-core blocked_boot_security_evidence_is_stored_on_bundle_entry
cargo test -p conary-core scriptlet_bundle_summary
```

Expected: both commands PASS.

## Task 5: Surface Evidence In Remi Publication Refusals

**Files:**
- Modify: `apps/remi/src/server/publication.rs`
- Modify: `crates/conary-core/src/repository/remi.rs`
- Modify direct `PublicationGateReport` test fixtures found by `rg -n "PublicationGateReport \\{" apps/remi/src crates/conary-core/src -g '*.rs'`.

- [ ] **Step 1: Write failing Remi report test**

Add to `apps/remi/src/server/publication.rs` tests:

```rust
#[test]
fn publication_report_includes_boot_security_intents() {
    let summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        blocked_classes: vec!["initramfs".to_string()],
        boot_security_intents: vec![conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence {
            class_id: "initramfs".to_string(),
            reason_code: "blocked-class-initramfs".to_string(),
            command: "dracut".to_string(),
            argv: vec!["--force".to_string()],
            phase: Some("post-install".to_string()),
            lifecycle_paths: vec!["post-install".to_string()],
        }],
        ..ScriptletBundleSummary::default()
    };

    let report = report_from_summary(&summary, true);

    assert_eq!(report.boot_security_intents.len(), 1);
    assert_eq!(report.boot_security_intents[0].class_id, "initramfs");
    assert_eq!(report.boot_security_intents[0].command, "dracut");
}
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p remi publication_report_includes_boot_security_intents
```

Expected: FAIL because `PublicationGateReport` does not include boot/security intents.

- [ ] **Step 3: Add report field**

In `PublicationGateReport` in `apps/remi/src/server/publication.rs`, add:

```rust
#[serde(default)]
pub boot_security_intents: Vec<conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence>,
```

In `report_from_summary()`, set:

```rust
boot_security_intents: summary.boot_security_intents.clone(),
```

- [ ] **Step 4: Mirror the client DTO**

In `crates/conary-core/src/repository/remi.rs`, add the same serde-defaulted field to the client-side `PublicationGateReport` DTO:

```rust
#[serde(default)]
pub boot_security_intents: Vec<crate::ccs::legacy_scriptlets::BootSecurityIntentEvidence>,
```

Update any direct `PublicationGateReport` constructors, including repository client tests, to set:

```rust
boot_security_intents: Vec::new(),
```

If `format_publication_refusal()` already prints blocked classes, append one short line per intent to the existing `message: String` after the existing generic `report_contains_boot_security_class()` warning. Supplement that warning; do not replace it.

```rust
if !report.boot_security_intents.is_empty() {
    message.push_str("\nBoot/security scriptlet evidence:");
    for intent in &report.boot_security_intents {
        let args = if intent.argv.is_empty() {
            String::new()
        } else {
            format!(" {}", intent.argv.join(" "))
        };
        message.push_str(&format!(
            "\n  - {}: {}{}",
            intent.class_id, intent.command, args
        ));
    }
}
```

Keep `reason_code` on the DTO even if the first client formatter does not print it; it is available for later richer diagnostics.

- [ ] **Step 5: Verify Remi and client report tests**

Run:

```bash
cargo test -p remi publication_report_includes_boot_security_intents
cargo test -p conary-core repository::remi
```

Expected: both commands PASS.

## Task 6: Prove Boot/Security Classes Stay Non-Public

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`
- Modify: `apps/remi/src/server/publication.rs`

- [ ] **Step 1: Add explicit support-matrix guard test**

Add to `support_matrix.rs` tests:

```rust
#[test]
fn boot_security_classes_remain_blocked_without_native_adapters() {
    let matrix = SupportMatrix::default();

    for class_id in ["kernel-module", "initramfs", "bootloader", "selinux"] {
        let row = matrix
            .entries()
            .iter()
            .find(|entry| entry.class_id == Some(class_id))
            .unwrap_or_else(|| panic!("missing support row for {class_id}"));
        assert_eq!(row.outcome, SupportOutcome::Blocked);
        assert!(row.adapter_id.is_none());
    }
}
```

- [ ] **Step 2: Add Remi publication guard test**

Add to `publication.rs` tests:

```rust
#[test]
fn boot_security_intent_does_not_make_blocked_summary_public() {
    let summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        blocked_classes: vec!["kernel-module".to_string()],
        boot_security_intents: vec![conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence {
            class_id: "kernel-module".to_string(),
            reason_code: "blocked-class-kernel-module".to_string(),
            command: "depmod".to_string(),
            argv: vec!["6.10.0".to_string()],
            phase: Some("post-install".to_string()),
            lifecycle_paths: vec!["post-install".to_string()],
        }],
        ..ScriptletBundleSummary::default()
    };

    assert!(matches!(
        classify_summary(ScriptletSummaryForPublication {
            summary,
            valid: true,
        }),
        PublicationDecision::Blocked(_)
    ));
}
```

- [ ] **Step 3: Run the guards**

Run:

```bash
cargo test -p conary-core boot_security_classes_remain_blocked_without_native_adapters
cargo test -p remi boot_security_intent_does_not_make_blocked_summary_public
```

Expected: both commands PASS.

## Task 7: Document The MVP Behavior

**Files:**
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update scriptlet security docs**

Add a short subsection under the legacy capture/publication discussion:

```markdown
### Boot And Security Scriptlet Evidence

Kernel-module, initramfs, bootloader, and SELinux scriptlet effects are
boot/security critical. Conary classifies these commands and preserves
sanitized command evidence in legacy scriptlet summaries, but public Remi
serving remains blocked until a future native adapter and target-profile fact
set proves complete replacement. Raw replay, `--no-scripts`, or malformed
summary metadata must not make these packages public-ready.

This evidence is harvested from package-authored metadata and static command
signals during conversion. It is an advisory trace for refusal diagnostics, not
runtime execution and not permission to mutate the host boot or security state.
Environment values are not surfaced in public boot/security evidence.
```

- [ ] **Step 2: Update the documentation ledger row**

Update the existing `docs/SCRIPTLET_SECURITY.md` row in `docs/superpowers/documentation-accuracy-audit-ledger.tsv` so `claim_clusters` includes `boot-security-scriptlets`, `evidence_sources` includes this plan, and the notes mention classified-but-blocked boot/security evidence.

- [ ] **Step 3: Run doc checks**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
```

Expected: all commands PASS.

## Task 8: Final Verification

**Files:**
- All files touched by Tasks 1-7

- [ ] **Step 1: Run focused CCS conversion proof**

Run:

```bash
cargo test -p conary-core blocked_class
cargo test -p conary-core support_matrix
cargo test -p conary-core legacy_replay
```

Expected: all PASS.

- [ ] **Step 2: Run Remi publication/client proof**

Run:

```bash
cargo test -p remi publication
cargo test -p conary-core repository::remi
```

Expected: all PASS.

- [ ] **Step 3: Run issue-adjacent integration proof**

Run:

```bash
cargo test -p conary --test bundle_replay
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: all PASS.

- [ ] **Step 4: Run workspace hygiene**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: all PASS with no formatting, lint, or whitespace errors.

## Follow-Up Plans Not Included Here

The full native handling design needs separate implementation plans for:

- extending `TargetProfileQuery` with boot/security facts and default-unsupported validation;
- adding target-profile catalog facts for kernel ABI, initramfs tooling, boot layout, and SELinux policy-store capabilities;
- adding CCS v2 native authority for adapter-backed boot/security operations;
- promoting exact proof-corpus-backed adapter rows from blocked/private-review to public-ready.

Do not implement those in this MVP unless a new approved plan explicitly scopes them in.
