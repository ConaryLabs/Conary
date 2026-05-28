# Legacy Scriptlet Adapter Registry And Blocked Classes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Goal 3a by adding structured command evidence, effect classification, adapter registry scaffolding, blocked-class policy, and a support matrix without changing manifest output, Remi publication, or install behavior.

**Architecture:** Add focused modules under `crates/conary-core/src/ccs/convert/` and expose a passive `ScriptletClassificationReport` from `ConversionResult`. The existing regex analyzer and hook extraction path remain in place; the new registry is parallel evidence for later bundle embedding and publication gates. Built-in adapters classify known helpers conservatively and do not claim full replacement except for native-free packages with no scriptlet entries.

**Tech Stack:** Rust, existing `conary-core` CCS conversion modules, `LegacyScriptletBundle` enum types, native ABI parser metadata, Cargo unit and integration tests.

---

## `/goal` Objective

Use this objective when implementation starts:

```text
/goal Implement Goal 3a: add structured legacy scriptlet command evidence, effect classification, adapter registry scaffolding, blocked-class policy, and support matrix coverage. Stop when fixture invocations classify as known, unknown, review, or blocked with stable reason IDs, existing conversion hooks still behave unchanged, no Remi DB/publication/install behavior changes, and the targeted tests plus clippy/fmt/diff checks pass.
```

## Read First

- `AGENTS.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-adapter-registry-blocked-classes-design.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `crates/conary-core/src/ccs/convert/analyzer.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/packages/native_abi.rs`

## Safety Rules

- Do not change install, update, remove, or replay behavior.
- Do not add DB migrations.
- Do not embed a `LegacyScriptletBundle` in converted Remi packages.
- Do not change public Remi package serving or publication status.
- The only allowed Remi touch is the mechanical test-helper update needed for the new `ConversionResult` field.
- Do not remove the existing analyzer or hook extraction path.
- Do not mark helper command adapters as `EffectReplacement::Complete` except the native-free/no-scriptlet case.
- Do not use static command extraction as the authority for dropping raw native scriptlets.

## File Structure

Create:

- `crates/conary-core/src/ccs/convert/command_evidence.rs`
- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/support_matrix.rs`

Modify:

- `crates/conary-core/src/ccs/convert/mod.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `apps/remi/src/server/conversion.rs` (test helper literal only)

Do not modify:

- `crates/conary-core/src/db/migrations/`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/jobs.rs`
- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/install/scriptlets.rs`
- `apps/conary/src/commands/remove.rs`

## Task 1: Command Evidence Model

**Files:**

- Create: `crates/conary-core/src/ccs/convert/command_evidence.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`

- [ ] **Step 1: Add the module and failing command evidence tests**

Add the module export:

```rust
// conary-core/src/ccs/convert/mod.rs
pub mod command_evidence;
```

Create `command_evidence.rs` with the path comment and these tests first:

```rust
// conary-core/src/ccs/convert/command_evidence.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};

    fn scriptlet(content: &str) -> Scriptlet {
        Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: content.to_string(),
            flags: None,
        }
    }

    #[test]
    fn command_evidence_splits_control_operators_with_stable_ids() {
        let invocations = extract_scriptlet_invocations(
            "rpm:%post",
            &scriptlet("VAR=1 /usr/bin/systemctl daemon-reload && /sbin/ldconfig\n"),
        );

        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].id, "rpm:%post:line0:cmd0");
        assert_eq!(invocations[0].entry_id, "rpm:%post");
        assert_eq!(invocations[0].source, CommandEvidenceSource::StaticSignal);
        assert_eq!(invocations[0].phase.as_deref(), Some("post-install"));
        assert_eq!(invocations[0].lifecycle_paths, vec!["post-install"]);
        assert_eq!(invocations[0].interpreter.as_deref(), Some("/bin/sh"));
        assert_eq!(
            invocations[0].environment,
            vec![CommandEnvironmentFact {
                name: "VAR".to_string(),
                value: Some("1".to_string()),
            }]
        );
        assert_eq!(invocations[0].command, "systemctl");
        assert_eq!(invocations[0].argv, vec!["daemon-reload"]);
        assert_eq!(invocations[1].id, "rpm:%post:line0:cmd1");
        assert_eq!(invocations[1].command, "ldconfig");
    }

    #[test]
    fn command_evidence_skips_wrappers_and_preserves_raw_line() {
        let invocations = extract_scriptlet_invocations(
            "deb:postinst",
            &scriptlet("env -i chroot /target /usr/bin/update-mime-database /usr/share/mime\n"),
        );

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].command, "update-mime-database");
        assert_eq!(invocations[0].argv, vec!["/usr/share/mime"]);
        assert_eq!(
            invocations[0].raw_line.as_deref(),
            Some("env -i chroot /target /usr/bin/update-mime-database /usr/share/mime")
        );
    }

    #[test]
    fn command_evidence_skips_wrapper_positional_arguments() {
        let invocations = extract_scriptlet_invocations(
            "rpm:%post",
            &scriptlet("sudo -u nobody chroot /target /sbin/ldconfig\n"),
        );

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].command, "ldconfig");
        assert!(invocations[0].argv.is_empty());
    }

    #[test]
    fn command_evidence_ignores_non_shell_interpreters() {
        let mut perl = scriptlet("#!/usr/bin/perl\nprint 'ok';\n");
        perl.interpreter = "/usr/bin/perl".to_string();

        assert!(extract_scriptlet_invocations("deb:config", &perl).is_empty());
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core command_evidence
```

Expected: compile failure for missing command evidence types and functions.

- [ ] **Step 3: Implement the command evidence types**

Add these public types:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    pub id: String,
    pub entry_id: String,
    pub source: CommandEvidenceSource,
    pub phase: Option<String>,
    pub lifecycle_paths: Vec<String>,
    pub interpreter: Option<String>,
    pub command: String,
    pub argv: Vec<String>,
    pub raw_line: Option<String>,
    pub cwd: Option<String>,
    pub environment: Vec<CommandEnvironmentFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEvidenceSource {
    StaticSignal,
    CaptureLog,
    NativeMetadata,
    PayloadHeuristic,
    CuratedRule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEnvironmentFact {
    pub name: String,
    pub value: Option<String>,
}
```

- [ ] **Step 4: Implement static extraction**

Implement:

```rust
pub fn extract_scriptlet_invocations(
    entry_id: &str,
    scriptlet: &crate::packages::traits::Scriptlet,
) -> Vec<CommandInvocation>

pub fn extract_native_entry_invocations(
    entry: &crate::packages::native_abi::NativeScriptletEntry,
) -> Vec<CommandInvocation>
```

Behavior:

- return empty for non-shell interpreters;
- ignore blank/comment lines;
- split `&&`, `||`, `;`, `|`, `$(`, parentheses, and backticks into command segments;
- skip leading `NAME=value` assignments and record them in `environment`;
- skip wrappers `sudo`, `env`, and `chroot`;
- iterate wrapper skipping while the current token is a known wrapper, so chains
  such as `env -i chroot /target /usr/bin/helper` resolve to `helper`;
- handle wrapper-specific positional arguments:
  - `chroot`: skip flags, then skip the first positional target directory;
  - `sudo`: skip flags and flag arguments for common flags such as `-u`, `-g`,
    `-h`, and `-p`;
  - `env`: skip flags and `NAME=value` assignments until the command token;
- normalize absolute command paths to basenames;
- preserve `raw_line`;
- build stable IDs as `{entry_id}:line{line_index}:cmd{command_index}`.

Use a dedicated wrapper-skipping loop instead of copying the legacy corpus
helper directly. The current corpus helper is evidence-only and does not model
all wrapper positional arguments correctly.

`extract_native_entry_invocations` should return empty unless the native entry
is executable, `support == NativeScriptletSupport::Parsed`, and
`entry.body.text` is present. For extracted invocations, set
`source = CommandEvidenceSource::NativeMetadata`, `entry_id = entry.id`,
`phase` from `entry.compatibility_phase.map(|phase| phase.to_string())`,
`lifecycle_paths` from the native lifecycle path `Debug` or stable display
labels, and `interpreter` from `entry.interpreter`.

- [ ] **Step 5: Verify Task 1**

Run:

```bash
cargo test -p conary-core command_evidence
```

Expected: the command evidence tests pass.

- [ ] **Step 6: Commit Task 1**

```bash
git add crates/conary-core/src/ccs/convert/mod.rs crates/conary-core/src/ccs/convert/command_evidence.rs
git commit -m "feat(scriptlets): add command evidence model"
```

## Task 2: Effect IR And Classification Report

**Files:**

- Create: `crates/conary-core/src/ccs/convert/effects.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`

- [ ] **Step 1: Add the module and failing tests**

Add the module export:

```rust
// conary-core/src/ccs/convert/mod.rs
pub mod effects;
```

Create tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::legacy_scriptlets::{
        EffectConfidence, EffectReplacement, EffectSource,
    };

    #[test]
    fn classification_report_counts_known_unknown_review_and_blocked() {
        let mut report = ScriptletClassificationReport::default();

        report.push(
            "rpm:%post",
            ScriptletClassification::Known {
                reason_code: "known-helper-requires-adapter-coverage".to_string(),
                effects: vec![ScriptletEffectEvidence {
                    kind: "dynamic-linker-cache".to_string(),
                    source: EffectSource::StaticSignal,
                    confidence: EffectConfidence::Inferred,
                    replacement: EffectReplacement::None,
                    adapter_id: Some("ldconfig/v1".to_string()),
                    adapter_digest: Some("sha256:test".to_string()),
                    command: Some("ldconfig".to_string()),
                    args: vec![],
                    path: None,
                    reason_code: Some("known-helper-requires-adapter-coverage".to_string()),
                }],
            },
        );
        report.push(
            "rpm:%post",
            ScriptletClassification::Unknown {
                reason_code: "unknown-command".to_string(),
                command: "custom-helper".to_string(),
            },
        );
        report.push(
            "deb:config",
            ScriptletClassification::Review {
                reason_code: "review-class-debconf".to_string(),
                class_id: Some("debconf".to_string()),
            },
        );
        report.push(
            "rpm:%post",
            ScriptletClassification::Blocked {
                reason_code: "blocked-class-network".to_string(),
                class_id: "network".to_string(),
            },
        );

        assert_eq!(report.known_count, 1);
        assert_eq!(report.unknown_count, 1);
        assert_eq!(report.review_count, 1);
        assert_eq!(report.blocked_count, 1);
        assert_eq!(report.unsupported_class_counts.get("debconf"), Some(&1));
        assert_eq!(report.unsupported_class_counts.get("network"), Some(&1));
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core classification_report_counts
```

Expected: compile failure for missing effect/classification types.

- [ ] **Step 3: Implement effect evidence and classification types**

Create:

```rust
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEffectEvidence {
    pub kind: String,
    pub source: EffectSource,
    pub confidence: EffectConfidence,
    pub replacement: EffectReplacement,
    pub adapter_id: Option<String>,
    pub adapter_digest: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub path: Option<String>,
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptletClassification {
    Known {
        reason_code: String,
        effects: Vec<ScriptletEffectEvidence>,
    },
    Unknown {
        reason_code: String,
        command: String,
    },
    Review {
        reason_code: String,
        class_id: Option<String>,
    },
    Blocked {
        reason_code: String,
        class_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptletClassificationReport {
    pub entries: Vec<EntryClassification>,
    pub known_count: u32,
    pub unknown_count: u32,
    pub review_count: u32,
    pub blocked_count: u32,
    pub unsupported_class_counts: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryClassification {
    pub entry_id: String,
    pub classification: ScriptletClassification,
}
```

Implement `ScriptletClassificationReport::push(entry_id, classification)`.
`unsupported_class_counts` must increment for every
`Review { class_id: Some(..) }` and every `Blocked { class_id, .. }`.

- [ ] **Step 4: Verify Task 2**

Run:

```bash
cargo test -p conary-core classification_report_counts
```

Expected: pass.

- [ ] **Step 5: Commit Task 2**

```bash
git add crates/conary-core/src/ccs/convert/mod.rs crates/conary-core/src/ccs/convert/effects.rs
git commit -m "feat(scriptlets): add effect classification report"
```

## Task 3: Blocked-Class Registry

**Files:**

- Create: `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`

- [ ] **Step 1: Add the module and failing tests**

Export:

```rust
pub mod blocked_classes;
```

Tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::convert::command_evidence::{
        CommandEvidenceSource, CommandInvocation,
    };

    fn invocation(command: &str, argv: &[&str]) -> CommandInvocation {
        CommandInvocation {
            id: format!("entry:line0:cmd0:{command}"),
            entry_id: "entry".to_string(),
            source: CommandEvidenceSource::StaticSignal,
            phase: Some("post-install".to_string()),
            lifecycle_paths: vec!["post-install".to_string()],
            interpreter: Some("/bin/sh".to_string()),
            command: command.to_string(),
            argv: argv.iter().map(|arg| arg.to_string()).collect(),
            raw_line: Some(format!("{} {}", command, argv.join(" ")).trim().to_string()),
            cwd: None,
            environment: vec![],
        }
    }

    #[test]
    fn blocked_classes_block_network_and_package_manager_recursion() {
        let registry = BlockedClassRegistry::default();

        let network = registry.match_invocation(&invocation("curl", &["https://example.invalid"]));
        assert_eq!(network.unwrap().reason_code, "blocked-class-network");

        let pm = registry.match_invocation(&invocation("dnf", &["install", "foo"]));
        assert_eq!(
            pm.unwrap().reason_code,
            "blocked-class-package-manager-recursion"
        );
    }

    #[test]
    fn blocked_classes_mark_dbus_and_debconf_for_review() {
        let registry = BlockedClassRegistry::default();

        let dbus = registry.match_invocation(&invocation("dbus-update-activation-environment", &[]));
        assert_eq!(dbus.unwrap().default_outcome, BlockedClassOutcome::Review);

        let debconf = registry.class_by_id("debconf").expect("debconf class");
        assert_eq!(debconf.reason_code, "review-class-debconf");
    }

    #[test]
    fn blocked_classes_mark_rpm_verify_legacy_init_and_udev() {
        let registry = BlockedClassRegistry::default();

        let verify = registry.class_by_id("rpm-verify").expect("rpm verify class");
        assert_eq!(verify.reason_code, "review-class-rpm-verify");

        let init = registry.match_invocation(&invocation("update-rc.d", &["demo", "defaults"]));
        assert_eq!(init.unwrap().reason_code, "blocked-class-legacy-init");

        let udev = registry.match_invocation(&invocation("udevadm", &["trigger"]));
        assert_eq!(udev.unwrap().default_outcome, BlockedClassOutcome::Review);
        assert_eq!(udev.unwrap().reason_code, "review-class-udev");

        assert!(registry.match_invocation(&invocation("udevadm", &["info"])).is_none());
    }

    #[test]
    fn blocked_classes_match_command_forms() {
        let registry = BlockedClassRegistry::default();

        let chmod_form = registry.match_invocation(&invocation("chmod", &["u+s", "/usr/bin/foo"]));
        assert_eq!(
            chmod_form.unwrap().reason_code,
            "blocked-class-setuid-setcap"
        );

        let chmod_mode = registry.match_invocation(&invocation("chmod", &["4755", "/usr/bin/foo"]));
        assert_eq!(
            chmod_mode.unwrap().reason_code,
            "blocked-class-setuid-setcap"
        );
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core blocked_classes
```

Expected: compile failure for missing registry types.

- [ ] **Step 3: Implement registry types and built-ins**

Create:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedClassOutcome {
    Review,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedClass {
    pub id: &'static str,
    pub description: &'static str,
    pub default_outcome: BlockedClassOutcome,
    pub reason_code: &'static str,
    pub command_names: &'static [&'static str],
    pub command_forms: &'static [&'static str],
    pub affected_formats: &'static [&'static str],
    pub preview_distros: &'static [&'static str],
    pub unblock_criteria: &'static str,
}

#[derive(Debug, Clone)]
pub struct BlockedClassRegistry {
    classes: Vec<BlockedClass>,
}
```

Implement:

- `Default` with the classes from the design spec.
- `classes(&self) -> &[BlockedClass]`
- `class_by_id(&self, id: &str) -> Option<&BlockedClass>`
- `match_invocation(&self, invocation: &CommandInvocation) -> Option<&BlockedClass>`
- duplicate ID assertion in `Default`.

Built-ins must include `setuid-setcap` and `sysctl` in addition to the other
blocked classes from the design spec. Use command-name matches for `setcap`,
`setpriv`, and `sysctl`; use `command_forms` for `chmod u+s` and `chmod 4*`.
The `dbus-policy` review class must include concrete command names
`dbus-update-activation-environment` and `dbus-send`, plus path-oriented
metadata in its description and unblock criteria.

Also include:

- `rpm-verify` review class with reason `review-class-rpm-verify`;
- `legacy-init` blocked class with reason `blocked-class-legacy-init` for
  `chkconfig`, `update-rc.d`, and `rc-update`;
- `udev` review class with reason `review-class-udev` for forms
  `udevadm trigger*` and `udevadm control*`, not every `udevadm` command;
- `arch-alpm-hook` review class with reason `review-class-arch-alpm-hook`;
- `arch-install-function` review class with reason
  `review-class-arch-install-function`;
- `native-abi-unpreservable` blocked class with reason
  `blocked-class-native-abi-unpreservable` for parser-level unpreservable
  native entries.

The `debconf` class is review evidence only. It must not cause Conary to add
`debconf`, `dpkg`, or other Debian runtime tooling as a target dependency on
non-Debian systems. A DEB package that requires debconf configuration remains
review/blocked for foreign targets until a later goal provides a modeled
Conary-native configuration equivalent, source-native/family-compatible policy,
or an explicit operator-supplied configuration transform.

`match_invocation` must check both matching modes:

- compare `invocation.command` against `command_names`;
- build `form = invocation.command + " " + invocation.argv.join(" ")`;
- compare `form` against exact `command_forms`;
- for `command_forms` entries ending in `*`, compare `form` against the entry
  prefix before the `*`.

This preserves the existing corpus behavior where dangerous `chmod` forms are
blocked without making every `chmod` invocation a blocked class.

- [ ] **Step 4: Verify Task 3**

Run:

```bash
cargo test -p conary-core blocked_classes
```

Expected: pass.

- [ ] **Step 5: Commit Task 3**

```bash
git add crates/conary-core/src/ccs/convert/mod.rs crates/conary-core/src/ccs/convert/blocked_classes.rs
git commit -m "feat(scriptlets): add blocked class registry"
```

## Task 4: Adapter Registry

**Files:**

- Create: `crates/conary-core/src/ccs/convert/adapters.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`

- [ ] **Step 1: Add module and failing adapter tests**

Export:

```rust
pub mod adapters;
```

Tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::convert::command_evidence::{
        CommandEvidenceSource, CommandInvocation,
    };
    use crate::ccs::convert::effects::ScriptletClassification;

    fn invocation(command: &str, argv: &[&str]) -> CommandInvocation {
        CommandInvocation {
            id: format!("entry:line0:cmd0:{command}"),
            entry_id: "entry".to_string(),
            source: CommandEvidenceSource::StaticSignal,
            phase: Some("post-install".to_string()),
            lifecycle_paths: vec!["post-install".to_string()],
            interpreter: Some("/bin/sh".to_string()),
            command: command.to_string(),
            argv: argv.iter().map(|arg| arg.to_string()).collect(),
            raw_line: Some(format!("{} {}", command, argv.join(" ")).trim().to_string()),
            cwd: None,
            environment: vec![],
        }
    }

    #[test]
    fn adapter_registry_classifies_known_helpers_without_complete_replacement() {
        let registry = AdapterRegistry::default();

        let classification = registry.classify_invocation(&invocation("ldconfig", &[]));

        let ScriptletClassification::Known { reason_code, effects } = classification else {
            panic!("ldconfig should be known");
        };
        assert_eq!(reason_code, "known-helper-requires-adapter-coverage");
        assert_eq!(effects[0].adapter_id.as_deref(), Some("ldconfig/v1"));
        assert_ne!(
            effects[0].replacement,
            crate::ccs::legacy_scriptlets::EffectReplacement::Complete
        );
    }

    #[test]
    fn adapter_registry_lets_blocked_class_win_before_adapter_matching() {
        let registry = AdapterRegistry::default();

        let classification = registry.classify_invocation(&invocation("curl", &["https://example.invalid"]));

        assert!(matches!(
            classification,
            ScriptletClassification::Blocked { reason_code, class_id }
                if reason_code == "blocked-class-network" && class_id == "network"
        ));
    }

    #[test]
    fn adapter_registry_reports_unknown_commands() {
        let registry = AdapterRegistry::default();

        let classification = registry.classify_invocation(&invocation("custom-helper", &["--do-it"]));

        assert!(matches!(
            classification,
            ScriptletClassification::Unknown { reason_code, command }
                if reason_code == "unknown-command" && command == "custom-helper"
        ));
    }

    #[test]
    fn adapter_registry_has_stable_builtin_order_and_unique_ids() {
        let registry = AdapterRegistry::default();
        let ids = registry.adapter_ids();

        assert_eq!(
            ids,
            vec![
                "native-free/v1",
                "ldconfig/v1",
                "systemd-daemon-reload/v1",
                "systemd-enable-disable/v1",
            ]
        );

        let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len());

        let native_free = registry
            .adapters_for_testing()
            .into_iter()
            .find(|adapter| adapter.id() == "native-free/v1")
            .expect("native-free adapter present");
        assert!(!native_free.matches(&invocation("true", &[])));
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core adapter_registry
```

Expected: compile failure for missing adapter registry.

- [ ] **Step 3: Implement adapter types**

Define:

```rust
use crate::ccs::convert::blocked_classes::{BlockedClassOutcome, BlockedClassRegistry};
use crate::ccs::convert::command_evidence::CommandInvocation;
use crate::ccs::convert::effects::{ScriptletClassification, ScriptletEffectEvidence};

pub trait ScriptletEffectAdapter {
    fn id(&self) -> &'static str;
    fn digest(&self) -> String;
    fn command_names(&self) -> &'static [&'static str];
    fn matches(&self, invocation: &CommandInvocation) -> bool;
    fn classify(&self, invocation: &CommandInvocation) -> ScriptletClassification;
}

pub struct AdapterRegistry {
    adapters: Vec<Box<dyn ScriptletEffectAdapter + Send + Sync>>,
    blocked_classes: BlockedClassRegistry,
}
```

Expose these accessors for tests and support-matrix validation:

```rust
impl AdapterRegistry {
    pub fn adapter_ids(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|adapter| adapter.id()).collect()
    }

    #[cfg(test)]
    fn adapters_for_testing(&self) -> Vec<&(dyn ScriptletEffectAdapter + Send + Sync)> {
        self.adapters.iter().map(|adapter| adapter.as_ref()).collect()
    }
}
```

Implement `Default` with:

- `NativeFreeAdapter`
- `LdconfigAdapter`
- `SystemdDaemonReloadAdapter`
- `SystemdEnableDisableAdapter`

Use `crate::hash::sha256_prefixed(adapter_descriptor.as_bytes())` for stable
adapter digests.

`NativeFreeAdapter` is included for registry digest and support-matrix coverage
but never participates in per-command dispatch. Its trait methods should return
`command_names() -> &[]`, `matches() -> false`, and `classify()` should be
`unreachable!("native-free is package-level evidence")`.

Sketch concrete adapters with private unit structs. For example:

```rust
struct LdconfigAdapter;

impl ScriptletEffectAdapter for LdconfigAdapter {
    fn id(&self) -> &'static str {
        "ldconfig/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"ldconfig/v1:dynamic-linker-cache:none")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["ldconfig"]
    }

    fn matches(&self, invocation: &CommandInvocation) -> bool {
        invocation.command == "ldconfig"
    }

    fn classify(&self, invocation: &CommandInvocation) -> ScriptletClassification {
        ScriptletClassification::Known {
            reason_code: "known-helper-requires-adapter-coverage".to_string(),
            effects: vec![ScriptletEffectEvidence {
                kind: "dynamic-linker-cache".to_string(),
                source: crate::ccs::legacy_scriptlets::EffectSource::StaticSignal,
                confidence: crate::ccs::legacy_scriptlets::EffectConfidence::Inferred,
                replacement: crate::ccs::legacy_scriptlets::EffectReplacement::None,
                adapter_id: Some(self.id().to_string()),
                adapter_digest: Some(self.digest()),
                command: Some(invocation.command.clone()),
                args: invocation.argv.clone(),
                path: None,
                reason_code: Some("known-helper-requires-adapter-coverage".to_string()),
            }],
        }
    }
}
```

Also add:

```rust
impl AdapterRegistry {
    /// Native-free classification is package-level evidence, not per-command
    /// dispatch. `NativeFreeAdapter` remains in the registry so support-matrix
    /// coverage and adapter digests include the no-scriptlet case.
    pub fn classify_native_free_package(&self) -> ScriptletClassification {
        let adapter = self
            .adapters
            .iter()
            .find(|adapter| adapter.id() == "native-free/v1")
            .expect("default registry must include native-free/v1");

        ScriptletClassification::Known {
            reason_code: "native-free-no-scriptlets".to_string(),
            effects: vec![ScriptletEffectEvidence {
                kind: "no-scriptlet".to_string(),
                source: crate::ccs::legacy_scriptlets::EffectSource::NativeMetadata,
                confidence: crate::ccs::legacy_scriptlets::EffectConfidence::Declared,
                replacement: crate::ccs::legacy_scriptlets::EffectReplacement::Complete,
                adapter_id: Some(adapter.id().to_string()),
                adapter_digest: Some(adapter.digest()),
                command: None,
                args: vec![],
                path: None,
                reason_code: Some("native-free-no-scriptlets".to_string()),
            }],
        }
    }
}
```

- [ ] **Step 4: Implement classification order**

`AdapterRegistry::classify_invocation` must:

1. consult `BlockedClassRegistry::match_invocation`;
2. return `Blocked` for blocked classes;
3. return `Review` for review classes;
4. run matching adapters;
5. return `Unknown` when nothing matches.

- [ ] **Step 5: Verify Task 4**

Run:

```bash
cargo test -p conary-core adapter_registry
```

Expected: pass.

- [ ] **Step 6: Commit Task 4**

```bash
git add crates/conary-core/src/ccs/convert/mod.rs crates/conary-core/src/ccs/convert/adapters.rs
git commit -m "feat(scriptlets): add conservative adapter registry"
```

## Task 5: Support Matrix Scaffold

**Files:**

- Create: `crates/conary-core/src/ccs/convert/support_matrix.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`

- [ ] **Step 1: Add module and failing support matrix tests**

Export:

```rust
pub mod support_matrix;
```

Tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::convert::adapters::AdapterRegistry;
    use crate::ccs::convert::blocked_classes::BlockedClassRegistry;

    #[test]
    fn support_matrix_covers_every_builtin_adapter() {
        let matrix = SupportMatrix::default();
        let registry = AdapterRegistry::default();

        for adapter_id in registry.adapter_ids() {
            let row = matrix
                .entries()
                .iter()
                .find(|entry| entry.adapter_id.as_deref() == Some(adapter_id));
            assert!(
                row.is_some(),
                "missing support matrix row for adapter {adapter_id}"
            );
            let row = row.unwrap();
            assert_eq!(row.outcome, SupportOutcome::Known);
            assert!(!row.reason_code.is_empty());
            assert!(!row.source_families.is_empty());
            assert!(!row.fixture_names.is_empty());
        }
    }

    #[test]
    fn support_matrix_covers_every_blocked_class_reason() {
        let matrix = SupportMatrix::default();
        let classes = BlockedClassRegistry::default();

        for class in classes.classes() {
            assert!(
                matrix.entries().iter().any(|entry| {
                    entry.class_id.as_deref() == Some(class.id)
                        && entry.reason_code == class.reason_code
                }),
                "missing support matrix row for class {}",
                class.id
            );
        }
    }

    #[test]
    fn support_matrix_has_no_orphan_adapter_or_class_rows() {
        let matrix = SupportMatrix::default();
        let adapter_ids: std::collections::BTreeSet<_> =
            AdapterRegistry::default().adapter_ids().into_iter().collect();
        let class_ids: std::collections::BTreeSet<_> = BlockedClassRegistry::default()
            .classes()
            .iter()
            .map(|class| class.id)
            .collect();

        for entry in matrix.entries() {
            if let Some(adapter_id) = entry.adapter_id {
                assert!(adapter_ids.contains(adapter_id), "orphan adapter row {adapter_id}");
            }
            if let Some(class_id) = entry.class_id {
                assert!(class_ids.contains(class_id), "orphan class row {class_id}");
            }
        }
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core support_matrix
```

Expected: compile failure for missing support matrix.

- [ ] **Step 3: Implement support matrix types**

Create:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportMatrixEntry {
    pub id: &'static str,
    pub command: Option<&'static str>,
    pub class_id: Option<&'static str>,
    pub adapter_id: Option<&'static str>,
    pub outcome: SupportOutcome,
    pub reason_code: &'static str,
    pub source_families: &'static [&'static str],
    pub lifecycle_notes: &'static str,
    pub fixture_names: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportOutcome {
    Known,
    Review,
    Blocked,
}

#[derive(Debug, Clone)]
pub struct SupportMatrix {
    entries: Vec<SupportMatrixEntry>,
}
```

Implement `Default` with rows for all built-in adapters and blocked classes.
Assert duplicate matrix IDs are absent.
Expose `pub fn entries(&self) -> &[SupportMatrixEntry] { &self.entries }`.
The tests above intentionally call both `AdapterRegistry::default()` and
`SupportMatrix::default()` so duplicate static IDs fail during CI, not only on
a production cold path.

- [ ] **Step 4: Use adapter ID accessors**

Use the `AdapterRegistry::adapter_ids()` accessor added in Task 4 for support
matrix validation. Do not add a duplicate local registry table in
`support_matrix.rs`.


- [ ] **Step 5: Verify Task 5**

Run:

```bash
cargo test -p conary-core support_matrix
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
```

Expected: pass.

- [ ] **Step 6: Commit Task 5**

```bash
git add crates/conary-core/src/ccs/convert/mod.rs crates/conary-core/src/ccs/convert/support_matrix.rs crates/conary-core/src/ccs/convert/adapters.rs
git commit -m "feat(scriptlets): add adapter support matrix"
```

## Task 6: Converter Integration Without Behavior Changes

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Modify: `apps/remi/src/server/conversion.rs` (test helper literal only)

- [ ] **Step 1: Add failing conversion integration tests**

In the existing `#[cfg(test)]` module in `converter.rs`, add:

```rust
#[test]
fn conversion_result_carries_scriptlet_classification_report() {
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "/sbin/ldconfig\n".to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = LegacyConverter::new(ConversionOptions {
        capture_scriptlets: false,
        min_fidelity: FidelityLevel::Low,
        ..ConversionOptions::default()
    });

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.known_count >= 1);
    assert!(
        result
            .scriptlet_classification
            .entries
            .iter()
            .any(|entry| entry.entry_id.contains("scriptlet"))
    );
    assert!(result.build_result.manifest.legacy_scriptlets.is_none());
}

#[test]
fn adapter_registry_does_not_remove_existing_detected_hooks() {
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "systemctl enable demo.service\n".to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = LegacyConverter::new(ConversionOptions {
        capture_scriptlets: false,
        min_fidelity: FidelityLevel::Low,
        ..ConversionOptions::default()
    });

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert!(
        result
            .detected_hooks
            .systemd
            .iter()
            .any(|hook| hook.unit == "demo.service")
    );
    assert!(
        result
            .scriptlet_classification
            .entries
            .iter()
            .any(|entry| matches!(
                &entry.classification,
                crate::ccs::convert::effects::ScriptletClassification::Known { .. }
            ))
    );
}

#[test]
fn native_parser_support_status_is_preserved_in_classification_report() {
    use crate::packages::native_abi::*;

    let mut metadata = make_test_metadata();
    metadata.scriptlets.clear();
    metadata.native_scriptlet_abi = vec![
        rpm_native_entry(
            "rpm:%verify",
            "%verify",
            "echo verify\n",
            RpmScriptletSlot::Verify,
            NativeLifecyclePath::Verify,
            NativeTransactionPosition::Verification,
            NativeScriptletSupport::DeferredReview {
                reason_code: "rpm-verify-scriptlet-deferred".to_string(),
            },
        ),
        rpm_native_entry(
            "rpm:broken",
            "%broken",
            "echo broken\n",
            RpmScriptletSlot::Verify,
            NativeLifecyclePath::Verify,
            NativeTransactionPosition::Verification,
            NativeScriptletSupport::Unpreservable {
                reason_code: "native-abi-parser-limitation".to_string(),
            },
        ),
    ];
    let files = make_test_files();
    let converter = LegacyConverter::new(ConversionOptions {
        capture_scriptlets: false,
        min_fidelity: FidelityLevel::Low,
        ..ConversionOptions::default()
    });

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.entries.iter().any(|entry| matches!(
        &entry.classification,
        crate::ccs::convert::effects::ScriptletClassification::Review { reason_code, class_id }
            if reason_code == "rpm-verify-scriptlet-deferred"
                && class_id.as_deref() == Some("rpm-verify")
    )));
    assert!(result.scriptlet_classification.entries.iter().any(|entry| matches!(
        &entry.classification,
        crate::ccs::convert::effects::ScriptletClassification::Blocked { reason_code, class_id }
            if reason_code == "native-abi-parser-limitation"
                && class_id == "native-abi-unpreservable"
    )));
}

#[test]
fn parsed_native_abi_body_is_classified_when_flattened_scriptlets_are_empty() {
    use crate::packages::native_abi::*;

    let mut metadata = make_test_metadata();
    metadata.scriptlets.clear();
    metadata.native_scriptlet_abi = vec![rpm_native_entry(
        "rpm:%post",
        "%post",
        "/sbin/ldconfig\n",
        RpmScriptletSlot::Post,
        NativeLifecyclePath::PostInstall,
        NativeTransactionPosition::AfterPayload,
        NativeScriptletSupport::Parsed,
    )];
    let files = make_test_files();
    let converter = LegacyConverter::new(ConversionOptions {
        capture_scriptlets: false,
        min_fidelity: FidelityLevel::Low,
        ..ConversionOptions::default()
    });

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.entries.iter().any(|entry| {
        entry.entry_id == "rpm:%post"
            && matches!(
                &entry.classification,
                crate::ccs::convert::effects::ScriptletClassification::Known { reason_code, .. }
                    if reason_code == "known-helper-requires-adapter-coverage"
            )
    }));
}

#[test]
fn arch_deferred_native_reason_is_preserved_with_arch_class_id() {
    use crate::packages::native_abi::*;

    let mut metadata = make_test_metadata();
    metadata.scriptlets.clear();
    metadata.native_scriptlet_abi = vec![arch_alpm_entry(
        "arch:hook:demo",
        NativeScriptletSupport::DeferredReview {
            reason_code: "arch-alpm-hook-semantics-deferred".to_string(),
        },
    )];
    let files = make_test_files();
    let converter = LegacyConverter::new(ConversionOptions {
        capture_scriptlets: false,
        min_fidelity: FidelityLevel::Low,
        ..ConversionOptions::default()
    });

    let result = converter
        .convert(&metadata, &files, "arch", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.entries.iter().any(|entry| matches!(
        &entry.classification,
        crate::ccs::convert::effects::ScriptletClassification::Review { reason_code, class_id }
            if reason_code == "arch-alpm-hook-semantics-deferred"
                && class_id.as_deref() == Some("arch-alpm-hook")
    )));
}
```

Keep existing converter tests passing; these new tests set their own scriptlet
content so `make_test_metadata()` does not need to change for classification.
Add small `rpm_native_entry(...) -> NativeScriptletEntry` and
`arch_alpm_entry(...) -> NativeScriptletEntry` test helpers beside the existing
converter test helpers. Use concrete current struct fields rather than elided
literals:

```rust
fn rpm_native_entry(
    id: &str,
    slot_name: &str,
    body: &str,
    slot: RpmScriptletSlot,
    lifecycle: NativeLifecyclePath,
    position: NativeTransactionPosition,
    support: NativeScriptletSupport,
) -> NativeScriptletEntry {
    NativeScriptletEntry {
        id: id.to_string(),
        format: NativeScriptletFormat::Rpm,
        kind: NativeScriptletKind::Executable,
        native_slot: slot_name.to_string(),
        primary_lifecycle: lifecycle,
        compatibility_phase: None,
        lifecycle_paths: vec![lifecycle],
        interpreter: Some("/bin/sh".to_string()),
        interpreter_args: vec![],
        body: NativeScriptletBody::from_bytes(body.as_bytes().to_vec()),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(position),
        support,
        metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
            slot,
            scriptlet_flags: None,
            trigger: None,
        }),
    }
}
```

The Arch helper can use `NativeScriptletFormat::Arch`,
`NativeScriptletKind::ControlArtifact`, lifecycle `NativeLifecyclePath::Trigger`,
position `NativeTransactionPosition::ControlArtifact`, and
`NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(...))`
with empty trigger/action metadata.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core conversion_result_carries_scriptlet_classification_report
cargo test -p conary-core adapter_registry_does_not_remove_existing_detected_hooks
cargo test -p conary-core native_parser_support_status_is_preserved_in_classification_report
cargo test -p conary-core parsed_native_abi_body_is_classified_when_flattened_scriptlets_are_empty
cargo test -p conary-core arch_deferred_native_reason_is_preserved_with_arch_class_id
```

Expected: compile failure for missing `ConversionResult.scriptlet_classification`.

- [ ] **Step 3: Add the result field**

In `ConversionResult`, add:

```rust
pub scriptlet_classification: crate::ccs::convert::effects::ScriptletClassificationReport,
```

Update all `ConversionResult` literals in tests/helpers to use
`ScriptletClassificationReport::default()` where they do not run the converter.
The known direct-construction sites are:

- production result construction in `crates/conary-core/src/ccs/convert/converter.rs`;
- Remi test helper `make_conversion_result` in `apps/remi/src/server/conversion.rs`.

Keep the field non-optional so every real conversion carries a classification
report. A broader `ConversionResult` builder can be added later if more
downstream direct-construction sites appear; Goal 3a only has the two sites
above.

- [ ] **Step 4: Build classification report in `convert`**

Add a private helper in `converter.rs`:

```rust
fn classify_scriptlets(
    metadata: &PackageMetadata,
) -> crate::ccs::convert::effects::ScriptletClassificationReport {
    let registry = crate::ccs::convert::adapters::AdapterRegistry::default();
    let mut report = crate::ccs::convert::effects::ScriptletClassificationReport::default();

    if metadata.scriptlets.is_empty() && metadata.native_scriptlet_abi.is_empty() {
        report.push(
            "package",
            registry.classify_native_free_package(),
        );
        return report;
    }

    for entry in &metadata.native_scriptlet_abi {
        if let Some(classification) = classify_native_support(entry) {
            report.push(&entry.id, classification);
        }
        for invocation in crate::ccs::convert::command_evidence::extract_native_entry_invocations(
            entry,
        ) {
            report.push(&entry.id, registry.classify_invocation(&invocation));
        }
    }

    for (index, scriptlet) in metadata.scriptlets.iter().enumerate() {
        let entry_id = format!("scriptlet:{index}:{}", scriptlet.phase);
        for invocation in crate::ccs::convert::command_evidence::extract_scriptlet_invocations(
            &entry_id,
            scriptlet,
        ) {
            report.push(&entry_id, registry.classify_invocation(&invocation));
        }
    }

    report
}
```

Use `AdapterRegistry::classify_native_free_package()` from Task 4 for the
native-free branch.

Add `classify_native_support(entry)` as a private helper:

```rust
fn classify_native_support(
    entry: &crate::packages::native_abi::NativeScriptletEntry,
) -> Option<crate::ccs::convert::effects::ScriptletClassification> {
    use crate::packages::native_abi::NativeScriptletSupport;

    match &entry.support {
        NativeScriptletSupport::Parsed => None,
        NativeScriptletSupport::DeferredReview { reason_code } => {
            Some(crate::ccs::convert::effects::ScriptletClassification::Review {
                reason_code: reason_code.clone(),
                class_id: native_review_class_id(entry),
            })
        }
        NativeScriptletSupport::Unpreservable { reason_code } => {
            Some(crate::ccs::convert::effects::ScriptletClassification::Blocked {
                reason_code: reason_code.clone(),
                class_id: "native-abi-unpreservable".to_string(),
            })
        }
    }
}
```

- `NativeScriptletSupport::Parsed` returns `None` from this support-status
  helper, but parsed executable text still flows through
  `extract_native_entry_invocations(entry)` above.
- `NativeScriptletSupport::DeferredReview { reason_code }` returns
  `ScriptletClassification::Review` and preserves the parser-owned
  `reason_code`.
- `NativeScriptletSupport::Unpreservable { reason_code }` returns
  `ScriptletClassification::Blocked` with class ID
  `native-abi-unpreservable` and preserves the parser-owned `reason_code`.

For deferred review class IDs, map known native metadata to the closest registry
class without rewriting the reason code:

- implement `native_review_class_id(entry: &NativeScriptletEntry) -> Option<String>`
  by matching borrowed fields (`&entry.metadata`, `&entry.support`) so no
  `String` or metadata is moved out of the shared native entry;
- RPM `NativeLifecyclePath::Verify` or `RpmScriptletSlot::Verify` maps to
  `rpm-verify`;
- RPM trigger, file-trigger, and transaction-file-trigger lifecycle paths map
  to `rpm-trigger`;
- DEB trigger control artifacts map to `deb-trigger`;
- DEB `config` maps to `debconf`;
- Arch ALPM hook metadata maps to `arch-alpm-hook`;
- Arch `.INSTALL` function extraction deferred reason maps to
  `arch-install-function`;
- unknown parser-deferred entries may use `None` for `class_id`.

- [ ] **Step 5: Use the report in the conversion result**

Call `let scriptlet_classification = classify_scriptlets(metadata);` near the
start of `convert`, before capture mutates `final_metadata.scriptlets`.

Store it in the final `ConversionResult`.

- [ ] **Step 6: Verify Task 6**

Run:

```bash
cargo test -p conary-core conversion_result_carries_scriptlet_classification_report
cargo test -p conary-core adapter_registry_does_not_remove_existing_detected_hooks
cargo test -p conary-core native_parser_support_status_is_preserved_in_classification_report
cargo test -p conary-core parsed_native_abi_body_is_classified_when_flattened_scriptlets_are_empty
cargo test -p conary-core arch_deferred_native_reason_is_preserved_with_arch_class_id
cargo test -p conary-core converter
cargo test -p remi
```

Expected: pass.

- [ ] **Step 7: Commit Task 6**

```bash
git add crates/conary-core/src/ccs/convert/converter.rs crates/conary-core/src/ccs/convert/adapters.rs apps/remi/src/server/conversion.rs
git commit -m "feat(scriptlets): expose conversion classification evidence"
```

## Task 7: Cross-Module Verification And Scope Guard

**Files:**

- Modify only if tests reveal a compile gap from previous tasks.

- [ ] **Step 1: Run targeted registry tests**

```bash
cargo test -p conary-core command_evidence
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core support_matrix
cargo test -p conary-core native_parser_support_status_is_preserved_in_classification_report
cargo test -p conary-core parsed_native_abi_body_is_classified_when_flattened_scriptlets_are_empty
cargo test -p conary-core arch_deferred_native_reason_is_preserved_with_arch_class_id
cargo test -p conary-core conversion_integration
```

Expected: pass. If `conversion_integration` matches no tests, keep the command
in the verification log and rely on the converter tests from Task 6 for
behavioral coverage.

- [ ] **Step 2: Run package-level core tests**

```bash
cargo test -p conary-core
```

Expected: pass.

- [ ] **Step 3: Prove no out-of-scope files changed**

Run:

```bash
git diff --stat origin/main..HEAD
git diff --name-only origin/main..HEAD
```

Expected changed implementation files are limited to:

- `crates/conary-core/src/ccs/convert/mod.rs`
- `crates/conary-core/src/ccs/convert/command_evidence.rs`
- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/support_matrix.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `apps/remi/src/server/conversion.rs` (test helper literal only)

If this implementation happens in the same branch as the design packet,
`git diff --name-only origin/main..HEAD` may also include these planning files:

- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-adapter-registry-blocked-classes-design.md`
- `docs/superpowers/plans/2026-05-28-legacy-scriptlet-adapter-registry-blocked-classes-plan.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`

If tests required additional changed files, document why in the commit message
body before merging.

- [ ] **Step 4: Run final workspace gates**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

Expected: pass.

- [ ] **Step 5: Commit any final cleanup**

If Task 7 required cleanup changes:

```bash
git add crates/conary-core/src/ccs/convert
git commit -m "test(scriptlets): verify adapter registry scope"
```

If no changes were needed, do not create an empty commit.

## Done Checklist

Goal 3a is complete when:

- structured command evidence extracts stable fixture invocations;
- blocked classes produce stable review/blocked reason codes;
- known helpers classify through the adapter registry without complete
  replacement claims;
- unknown helpers remain first-class `Unknown` evidence;
- every built-in adapter and blocked class has support matrix coverage;
- parser-level native ABI review/blocked statuses remain visible in the report;
- `ConversionResult` carries the passive classification report;
- existing hook extraction and manifest output remain compatible;
- no Remi DB, Remi publication, install/update/remove, or replay behavior
  changes were made;
- all verification commands in Task 7 pass.

## Expected Final Verification

Run before asking to merge:

```bash
cargo test -p conary-core command_evidence
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core support_matrix
cargo test -p conary-core native_parser_support_status_is_preserved_in_classification_report
cargo test -p conary-core parsed_native_abi_body_is_classified_when_flattened_scriptlets_are_empty
cargo test -p conary-core arch_deferred_native_reason_is_preserved_with_arch_class_id
cargo test -p conary-core conversion_integration
cargo test -p conary-core
cargo test -p remi
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```
