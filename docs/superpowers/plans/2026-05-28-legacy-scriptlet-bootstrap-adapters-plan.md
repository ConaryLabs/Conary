# Legacy Scriptlet Bootstrap Adapters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Goal 3b by promoting a small corpus-backed set of scriptlet helper adapters from recognition-only evidence to precise complete, partial, review, or blocked classifications without changing package output or install behavior.

**Architecture:** Extend the Goal 3a adapter registry with payload-aware adapter input, structured effect metadata, and conservative command-form parsers. The converter passes extracted package files into passive classification so adapters can prove package-owned systemd, tmpfiles, sysusers, alternatives, and cache-refresh facts. Classification remains evidence only; manifest output, Remi publication, and install/replay behavior do not change.

**Tech Stack:** Rust, `conary-core` CCS conversion modules, `ExtractedFile` payload metadata, existing legacy scriptlet effect enums, Cargo unit tests and converter integration tests.

---

## `/goal` Objective

Use this objective when implementation starts:

```text
/goal Implement Goal 3b: add corpus-backed bootstrap scriptlet effect adapters for safe helper command forms, including payload hints, support-matrix rows, precise complete/review outcomes, and converter integration evidence. Stop when common helper fixtures classify as complete only where justified, unsupported variants still produce stable review or blocked reasons, no Remi DB/publication/install behavior changes, and targeted tests plus clippy/fmt/diff checks pass.
```

## Read First

- `AGENTS.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-bootstrap-adapters-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-adapter-registry-blocked-classes-design.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/ccs/convert/support_matrix.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `apps/remi/src/server/scriptlet_corpus.rs`

## Safety Rules

- Do not change install, update, remove, or replay behavior.
- Do not add DB migrations.
- Do not embed `LegacyScriptletBundle` into converted CCS packages.
- Do not change Remi publication status or package-serving behavior.
- Do not remove or rewrite the existing analyzer or detected hook path.
- Do not run live host helper commands.
- Do not mark source-family private helpers such as `debconf`,
  `deb-systemd-helper`, RPM triggers, DEB triggers, or Arch ALPM hooks as
  complete.
- Do not mark a command form complete unless its adapter has fixture coverage
  and a support-matrix row.

## File Structure

Create:

- `crates/conary-core/src/ccs/convert/payload_hints.rs`

Modify:

- `crates/conary-core/src/ccs/convert/mod.rs`
- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- `crates/conary-core/src/ccs/convert/support_matrix.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`

Do not modify:

- `crates/conary-core/src/db/migrations/`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/jobs.rs`
- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/install/scriptlets.rs`
- `apps/conary/src/commands/remove.rs`

## Task 1: Payload Hints And Effect Metadata

**Files:**

- Create: `crates/conary-core/src/ccs/convert/payload_hints.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`
- Modify: `crates/conary-core/src/ccs/convert/effects.rs`
- Test: `crates/conary-core/src/ccs/convert/payload_hints.rs`
- Test: `crates/conary-core/src/ccs/convert/effects.rs`

- [ ] **Step 1: Add failing payload hint tests**

Add the module export:

```rust
// conary-core/src/ccs/convert/mod.rs
pub mod payload_hints;
```

Create `payload_hints.rs` with these tests first:

```rust
// conary-core/src/ccs/convert/payload_hints.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::traits::ExtractedFile;

    fn file(path: &str) -> ExtractedFile {
        ExtractedFile {
            path: path.to_string(),
            content: Vec::new(),
            size: 0,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        }
    }

    #[test]
    fn payload_hints_find_systemd_tmpfiles_sysusers_and_libraries() {
        let hints = PayloadHints::from_files(&[
            file("/usr/lib/systemd/system/demo.service"),
            file("/usr/lib/tmpfiles.d/demo.conf"),
            file("/usr/lib/sysusers.d/demo.conf"),
            file("/usr/lib64/libdemo.so.1"),
        ]);

        assert!(hints.systemd_units.contains("demo.service"));
        assert!(hints.tmpfiles_configs.contains("/usr/lib/tmpfiles.d/demo.conf"));
        assert!(hints.sysusers_configs.contains("/usr/lib/sysusers.d/demo.conf"));
        assert!(hints.shared_libraries.contains("/usr/lib64/libdemo.so.1"));
    }

    #[test]
    fn payload_hints_find_cache_inputs_by_kind() {
        let hints = PayloadHints::from_files(&[
            file("/usr/share/mime/packages/demo.xml"),
            file("/usr/share/applications/demo.desktop"),
            file("/usr/share/icons/hicolor/16x16/apps/demo.png"),
            file("/usr/share/glib-2.0/schemas/org.example.demo.gschema.xml"),
            file("/usr/share/fonts/demo/demo.ttf"),
        ]);

        assert!(hints.cache_inputs["mime-db"].contains("/usr/share/mime/packages/demo.xml"));
        assert!(hints.cache_inputs["desktop-db"].contains("/usr/share/applications/demo.desktop"));
        assert!(hints.cache_inputs["icon-cache"].contains("/usr/share/icons/hicolor/16x16/apps/demo.png"));
        assert!(hints.cache_inputs["gsettings"].contains("/usr/share/glib-2.0/schemas/org.example.demo.gschema.xml"));
        assert!(hints.cache_inputs["font-cache"].contains("/usr/share/fonts/demo/demo.ttf"));
    }
}
```

- [ ] **Step 2: Run failing payload tests**

Run:

```bash
cargo test -p conary-core payload_hints
```

Expected: compile failure for missing `PayloadHints`.

- [ ] **Step 3: Implement `PayloadHints`**

Add this implementation above the tests:

```rust
use crate::packages::traits::ExtractedFile;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PayloadHints {
    pub systemd_units: BTreeSet<String>,
    pub tmpfiles_configs: BTreeSet<String>,
    pub sysusers_configs: BTreeSet<String>,
    pub shared_libraries: BTreeSet<String>,
    pub cache_inputs: BTreeMap<String, BTreeSet<String>>,
}

impl PayloadHints {
    pub fn from_files(files: &[ExtractedFile]) -> Self {
        let mut hints = Self::default();

        for file in files {
            let path = file.path.as_str();
            if let Some(unit) = systemd_unit_name(path) {
                hints.systemd_units.insert(unit.to_string());
            }
            if is_tmpfiles_config(path) {
                hints.tmpfiles_configs.insert(path.to_string());
            }
            if is_sysusers_config(path) {
                hints.sysusers_configs.insert(path.to_string());
            }
            if is_shared_library(path) {
                hints.shared_libraries.insert(path.to_string());
            }
            for kind in cache_input_kinds(path) {
                hints
                    .cache_inputs
                    .entry(kind.to_string())
                    .or_default()
                    .insert(path.to_string());
            }
        }

        hints
    }

    pub fn has_cache_input(&self, kind: &str) -> bool {
        self.cache_inputs
            .get(kind)
            .is_some_and(|paths| !paths.is_empty())
    }
}

fn systemd_unit_name(path: &str) -> Option<&str> {
    let prefix_matches = path.starts_with("/usr/lib/systemd/system/")
        || path.starts_with("/lib/systemd/system/")
        || path.starts_with("/etc/systemd/system/");
    if !prefix_matches {
        return None;
    }
    path.rsplit('/').next().filter(|name| {
        matches!(
            name.rsplit_once('.').map(|(_, suffix)| suffix),
            Some("service" | "socket" | "timer" | "path" | "target")
        )
    })
}

fn is_tmpfiles_config(path: &str) -> bool {
    (path.starts_with("/usr/lib/tmpfiles.d/")
        || path.starts_with("/lib/tmpfiles.d/")
        || path.starts_with("/etc/tmpfiles.d/"))
        && path.ends_with(".conf")
}

fn is_sysusers_config(path: &str) -> bool {
    (path.starts_with("/usr/lib/sysusers.d/")
        || path.starts_with("/lib/sysusers.d/")
        || path.starts_with("/etc/sysusers.d/"))
        && path.ends_with(".conf")
}

fn is_shared_library(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|name| {
        name.starts_with("lib") && (name.contains(".so.") || name.ends_with(".so"))
    })
}

fn cache_input_kinds(path: &str) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    if path.starts_with("/usr/share/mime/packages/") && path.ends_with(".xml") {
        kinds.push("mime-db");
    }
    if path.starts_with("/usr/share/applications/") && path.ends_with(".desktop") {
        kinds.push("desktop-db");
    }
    if path.starts_with("/usr/share/icons/") {
        kinds.push("icon-cache");
    }
    if path.starts_with("/usr/share/glib-2.0/schemas/") && path.ends_with(".gschema.xml") {
        kinds.push("gsettings");
    }
    if (path.starts_with("/usr/share/fonts/")
        || path.starts_with("/usr/local/share/fonts/")
        || path.starts_with("/usr/share/texmf/fonts/"))
        && matches!(
            path.rsplit('.').next(),
            Some("ttf" | "otf" | "pcf" | "pfb" | "pfm")
        )
    {
        kinds.push("font-cache");
    }
    kinds
}
```

- [ ] **Step 4: Add effect metadata field test**

In `effects.rs`, update the existing `classification_report_counts_known_unknown_review_and_blocked` test's `ScriptletEffectEvidence` literal with:

```rust
extra: BTreeMap::from([(
    "cache".to_string(),
    toml::Value::String("ld.so.cache".to_string()),
)]),
```

Expected compile failure until the field is added.

- [ ] **Step 5: Add `extra` to `ScriptletEffectEvidence`**

Update the struct:

```rust
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
    pub extra: BTreeMap<String, toml::Value>,
}
```

Update every existing literal in `adapters.rs`, `effects.rs`, and
`converter.rs` with `extra: BTreeMap::new()` unless the test intentionally
asserts metadata content.

Current literal inventory to update:

- `effects.rs` test literal in
  `classification_report_counts_known_unknown_review_and_blocked`;
- `adapters.rs` package-level literal in `classify_native_free_package`;
- `adapters.rs` helper literal in `known_effect_classification`;
- any new adapter-specific `ScriptletEffectEvidence` literal added in Goal 3b.

- [ ] **Step 6: Verify Task 1**

Run:

```bash
cargo test -p conary-core payload_hints
cargo test -p conary-core effects
```

Expected: both pass.

- [ ] **Step 7: Commit Task 1**

```bash
git add crates/conary-core/src/ccs/convert/mod.rs \
        crates/conary-core/src/ccs/convert/payload_hints.rs \
        crates/conary-core/src/ccs/convert/effects.rs \
        crates/conary-core/src/ccs/convert/adapters.rs \
        crates/conary-core/src/ccs/convert/converter.rs
git commit -m "feat(scriptlets): add payload hints for adapters"
```

## Task 2: Corpus Evidence Guard And Payload-Aware Adapter Registry

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/adapters.rs`
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Test: `crates/conary-core/src/ccs/convert/adapters.rs`
- Test: `crates/conary-core/src/ccs/convert/converter.rs`

- [ ] **Step 1: Add failing corpus evidence guard tests**

Add this test to `adapters.rs`:

```rust
#[test]
fn bootstrap_adapter_candidates_are_backed_by_corpus_evidence() {
    let evidence = bootstrap_adapter_evidence();

    for command in [
        "ldconfig",
        "systemctl",
        "systemd-tmpfiles",
        "systemd-sysusers",
        "update-alternatives",
        "update-mime-database",
        "install-info",
        "gconftool-2",
    ] {
        assert!(
            evidence.iter().any(|entry| entry.command == command),
            "missing bootstrap corpus evidence for {command}"
        );
    }

    for entry in evidence {
        assert!(entry.package_count > 0);
        assert!(entry.invocation_count >= entry.package_count);
        assert!(!entry.forms.is_empty());
        assert!(!entry.coverage_ids.is_empty());
    }
}
```

- [ ] **Step 2: Run failing corpus evidence guard test**

Run:

```bash
cargo test -p conary-core bootstrap_adapter_candidates_are_backed_by_corpus_evidence
```

Expected: compile failure for missing `bootstrap_adapter_evidence`.

- [ ] **Step 3: Add bootstrap corpus evidence fixture**

Add this public fixture to `adapters.rs` near the registry types:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapAdapterEvidence {
    pub command: &'static str,
    pub forms: &'static [&'static str],
    pub package_count: u32,
    pub invocation_count: u32,
    pub coverage_ids: &'static [&'static str],
}

pub fn bootstrap_adapter_evidence() -> &'static [BootstrapAdapterEvidence] {
    &[
        BootstrapAdapterEvidence {
            command: "ldconfig",
            forms: &["ldconfig", "/sbin/ldconfig"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["ldconfig/v2"],
        },
        BootstrapAdapterEvidence {
            command: "systemctl",
            forms: &[
                "systemctl daemon-reload",
                "systemctl enable",
                "systemctl disable",
                "systemctl preset",
            ],
            package_count: 1,
            invocation_count: 3,
            coverage_ids: &["systemd-daemon-reload/v2", "systemd-unit-state/v1"],
        },
        BootstrapAdapterEvidence {
            command: "systemd-tmpfiles",
            forms: &["systemd-tmpfiles --create"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["systemd-tmpfiles-create/v1"],
        },
        BootstrapAdapterEvidence {
            command: "systemd-sysusers",
            forms: &["systemd-sysusers"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["systemd-sysusers/v1"],
        },
        BootstrapAdapterEvidence {
            command: "update-alternatives",
            forms: &["update-alternatives --install", "update-alternatives --remove"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["alternatives-registration/v1"],
        },
        BootstrapAdapterEvidence {
            command: "update-mime-database",
            forms: &["update-mime-database /usr/share/mime"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["cache-refresh/v1"],
        },
        BootstrapAdapterEvidence {
            command: "install-info",
            forms: &["install-info"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["review-class-install-info"],
        },
        BootstrapAdapterEvidence {
            command: "gconftool-2",
            forms: &["gconftool-2 --makefile-install-rule"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["review-class-gconf-schema"],
        },
    ]
}
```

The counts are deliberately minimal fixture counts. They prove the adapter set
is tied to the Goal 0 evidence shape without requiring live repository metadata
during unit tests. Real corpus refreshes can raise these counts later.

- [ ] **Step 4: Add failing context tests**

Add this test to `adapters.rs`:

```rust
#[test]
fn adapter_registry_uses_payload_context_for_systemd_units() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload.systemd_units.insert("demo.service".to_string());

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["enable", "demo.service"]),
        payload: &payload,
    });

    let ScriptletClassification::Known { effects, .. } = classification else {
        panic!("systemctl enable should be known through context dispatch");
    };
    assert_eq!(effects[0].command.as_deref(), Some("systemctl"));
    assert_eq!(effects[0].args, vec!["enable", "demo.service"]);
}
```

Add these imports to the test module:

```rust
use crate::ccs::convert::payload_hints::PayloadHints;
```

- [ ] **Step 5: Run failing adapter context test**

Run:

```bash
cargo test -p conary-core adapter_registry_uses_payload_context_for_systemd_units
```

Expected: compile failure for missing `AdapterInput` and
`classify_invocation_with_context`.

- [ ] **Step 6: Add `AdapterInput` and context dispatch**

In `adapters.rs`, add:

```rust
use crate::ccs::convert::payload_hints::PayloadHints;
use std::collections::{BTreeMap, BTreeSet};

pub struct AdapterInput<'a> {
    pub invocation: &'a CommandInvocation,
    pub payload: &'a PayloadHints,
}
```

Change the trait:

```rust
pub trait ScriptletEffectAdapter {
    fn id(&self) -> &'static str;
    fn digest(&self) -> String;
    fn command_names(&self) -> &'static [&'static str];
    fn matches(&self, input: AdapterInput<'_>) -> bool;
    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification;
}
```

Add registry methods:

```rust
pub fn classify_invocation_with_context(
    &self,
    input: AdapterInput<'_>,
) -> ScriptletClassification {
    if let Some(class) = self.blocked_classes.match_invocation(input.invocation) {
        return match class.default_outcome {
            BlockedClassOutcome::Blocked => ScriptletClassification::Blocked {
                reason_code: class.reason_code.to_string(),
                class_id: class.id.to_string(),
            },
            BlockedClassOutcome::Review => ScriptletClassification::Review {
                reason_code: class.reason_code.to_string(),
                class_id: Some(class.id.to_string()),
            },
        };
    }

    self.adapters
        .iter()
        .find(|adapter| {
            adapter.matches(AdapterInput {
                invocation: input.invocation,
                payload: input.payload,
            })
        })
        .map_or_else(
            || ScriptletClassification::Unknown {
                reason_code: "unknown-command".to_string(),
                command: input.invocation.command.clone(),
            },
            |adapter| {
                adapter.classify(AdapterInput {
                    invocation: input.invocation,
                    payload: input.payload,
                })
            },
        )
}

pub fn classify_invocation(&self, invocation: &CommandInvocation) -> ScriptletClassification {
    let payload = PayloadHints::default();
    self.classify_invocation_with_context(AdapterInput {
        invocation,
        payload: &payload,
    })
}
```

Update every adapter `matches` and `classify` implementation to read
`input.invocation` instead of a direct `invocation` parameter.

- [ ] **Step 7: Update converter classification to pass files**

In `converter.rs`, change the call:

```rust
let scriptlet_classification = classify_scriptlets(metadata, files);
```

Change the helper signature:

```rust
fn classify_scriptlets(
    metadata: &PackageMetadata,
    files: &[ExtractedFile],
) -> ScriptletClassificationReport {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::from_files(files);
    let mut report = ScriptletClassificationReport::default();
    // existing body
}
```

Add import:

```rust
use crate::ccs::convert::adapters::{AdapterInput, AdapterRegistry};
use crate::ccs::convert::payload_hints::PayloadHints;
```

For each invocation classification, call:

```rust
registry.classify_invocation_with_context(AdapterInput {
    invocation: &invocation,
    payload: &payload,
})
```

- [ ] **Step 8: Verify Task 2**

Run:

```bash
cargo test -p conary-core bootstrap_adapter_candidates_are_backed_by_corpus_evidence
cargo test -p conary-core adapter_registry_uses_payload_context_for_systemd_units
cargo test -p conary-core conversion_integration
```

Expected: all pass, and existing converter tests still show
`manifest.legacy_scriptlets.is_none()`.

- [ ] **Step 9: Commit Task 2**

```bash
git add crates/conary-core/src/ccs/convert/adapters.rs \
        crates/conary-core/src/ccs/convert/converter.rs
git commit -m "feat(scriptlets): pass payload context to adapters"
```

## Task 3: Complete Dynamic Linker And systemd Metadata Adapters

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/adapters.rs`
- Modify: `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`
- Test: `crates/conary-core/src/ccs/convert/adapters.rs`
- Test: `crates/conary-core/src/ccs/convert/blocked_classes.rs`

- [ ] **Step 1: Add failing adapter tests**

Add these tests to `adapters.rs`:

```rust
#[test]
fn ldconfig_complete_only_for_simple_cache_refresh_forms() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    let complete = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("ldconfig", &[]),
        payload: &payload,
    });
    let ScriptletClassification::Known { reason_code, effects } = complete else {
        panic!("simple ldconfig should be known");
    };
    assert_eq!(reason_code, "helper-complete-ldconfig");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "dynamic-linker-cache");

    let review = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("ldconfig", &["-p"]),
        payload: &payload,
    });
    assert!(matches!(
        review,
        ScriptletClassification::Review { reason_code, class_id }
            if reason_code == "review-class-ldconfig-nonstandard"
                && class_id.as_deref() == Some("ldconfig-nonstandard")
    ));
}

#[test]
fn systemd_daemon_reload_is_complete_but_runtime_actions_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    let reload = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["daemon-reload"]),
        payload: &payload,
    });
    let ScriptletClassification::Known { reason_code, effects } = reload else {
        panic!("daemon-reload should be known");
    };
    assert_eq!(reason_code, "helper-complete-systemd-daemon-reload");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);

    let system_scope = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["--system", "daemon-reload"]),
        payload: &payload,
    });
    assert!(matches!(
        system_scope,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-systemd-daemon-reload"
    ));

    let restart = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["restart", "demo.service"]),
        payload: &payload,
    });
    assert!(matches!(
        restart,
        ScriptletClassification::Review { reason_code, class_id }
            if reason_code == "review-class-systemd-runtime-action"
                && class_id.as_deref() == Some("systemd-runtime-action")
    ));
}

#[test]
fn systemd_unit_state_requires_payload_evidence_for_complete() {
    let registry = AdapterRegistry::default();
    let empty_payload = PayloadHints::default();

    let partial = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["enable", "demo.service"]),
        payload: &empty_payload,
    });
    let ScriptletClassification::Known { reason_code, effects } = partial else {
        panic!("systemctl enable should be known");
    };
    assert_eq!(reason_code, "known-helper-partial-coverage");
    assert_eq!(effects[0].replacement, EffectReplacement::Partial);

    let mut payload = PayloadHints::default();
    payload.systemd_units.insert("demo.service".to_string());
    let complete = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemctl", &["preset", "demo.service"]),
        payload: &payload,
    });
    let ScriptletClassification::Known { reason_code, effects } = complete else {
        panic!("systemctl preset should be known");
    };
    assert_eq!(reason_code, "helper-complete-systemd-unit-state");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].path.as_deref(), Some("demo.service"));
}
```

Ensure the adapter test module imports:

```rust
use crate::ccs::legacy_scriptlets::EffectReplacement;
```

- [ ] **Step 2: Add failing blocked-class tests for systemd review forms**

Add to `blocked_classes.rs`:

```rust
#[test]
fn blocked_classes_review_systemd_runtime_user_and_deb_helpers() {
    let registry = BlockedClassRegistry::default();

    let runtime = registry.match_invocation(&invocation("systemctl", &["restart", "demo.service"]));
    assert_eq!(runtime.unwrap().reason_code, "review-class-systemd-runtime-action");

    let service_without_args = registry.match_invocation(&invocation("service", &[]));
    assert_eq!(
        service_without_args.unwrap().reason_code,
        "review-class-systemd-runtime-action"
    );

    let invoke_rc_without_args = registry.match_invocation(&invocation("invoke-rc.d", &[]));
    assert_eq!(
        invoke_rc_without_args.unwrap().reason_code,
        "review-class-systemd-runtime-action"
    );

    let user = registry.match_invocation(&invocation("systemctl", &["--user", "enable", "demo.service"]));
    assert_eq!(user.unwrap().reason_code, "review-class-systemd-user-scope");

    let deb = registry.match_invocation(&invocation("deb-systemd-helper", &["enable", "demo.service"]));
    assert_eq!(deb.unwrap().reason_code, "review-class-deb-systemd-helper");

    let preset_all = registry.match_invocation(&invocation("systemctl", &["preset-all"]));
    assert_eq!(preset_all.unwrap().reason_code, "review-class-systemd-runtime-action");
}
```

- [ ] **Step 3: Run failing tests**

Run:

```bash
cargo test -p conary-core ldconfig_complete_only_for_simple_cache_refresh_forms
cargo test -p conary-core systemd_daemon_reload_is_complete_but_runtime_actions_are_review
cargo test -p conary-core systemd_unit_state_requires_payload_evidence_for_complete
cargo test -p conary-core blocked_classes_review_systemd_runtime_user_and_deb_helpers
```

Expected: failing assertions because existing adapters still report
recognition-only results and missing review classes.

- [ ] **Step 4: Add review classes**

In `BlockedClassRegistry::default()`, add review classes before generic adapter
matching can see these forms:

```rust
review_class(
    "ldconfig-nonstandard",
    "ldconfig forms with custom roots, caches, link-only modes, print modes, or explicit directories need review.",
    "review-class-ldconfig-nonstandard",
    &[],
    &[
        "ldconfig -p*",
        "ldconfig -l*",
        "ldconfig -n*",
        "ldconfig -N*",
        "ldconfig -X*",
        "ldconfig -C*",
        "ldconfig -f*",
        "ldconfig -r*",
    ],
    "Add a dynamic-linker adapter that models the specific root/cache/link semantics.",
),
review_class(
    "systemd-runtime-action",
    "systemd runtime service actions signal a live manager and are not passive metadata changes.",
    "review-class-systemd-runtime-action",
    &["service", "invoke-rc.d"],
    &[
        "systemctl start*",
        "systemctl stop*",
        "systemctl restart*",
        "systemctl try-restart*",
        "systemctl reload*",
        "systemctl reload-or-restart*",
        "systemctl enable --now*",
        "systemctl disable --now*",
        "systemctl preset --now*",
        "systemctl preset-all*",
        "service *",
        "invoke-rc.d *",
    ],
    "Add modeled service runtime semantics or keep the package review-only.",
),
review_class(
    "systemd-user-scope",
    "systemd user/global scope enablement is target-user policy, not package-global metadata.",
    "review-class-systemd-user-scope",
    &[],
    &["systemctl --user*", "systemctl --global*"],
    "Add user-scope service policy and target compatibility checks.",
),
review_class(
    "deb-systemd-helper",
    "DEB systemd helper state is dpkg-family private and must not require installing dpkg helpers on foreign targets.",
    "review-class-deb-systemd-helper",
    &["deb-systemd-helper", "deb-systemd-invoke"],
    &[],
    "Model DEB helper state explicitly or require same-family review policy.",
),
```

Add fixture names for the new classes in `fixture_names_for_class()`.

- [ ] **Step 5: Promote ldconfig and systemd adapters**

Replace the existing v1 adapter IDs and digests:

```rust
"ldconfig/v2"
"systemd-daemon-reload/v2"
"systemd-unit-state/v1"
```

Implement `LdconfigAdapter::classify()` so simple forms return:

```rust
known_effect_classification(
    self,
    input.invocation,
    "dynamic-linker-cache",
    EffectReplacement::Complete,
    None,
    "helper-complete-ldconfig",
    BTreeMap::from([(
        "cache".to_string(),
        toml::Value::String("ld.so.cache".to_string()),
    )]),
)
```

Implement `SystemdDaemonReloadAdapter::classify()` with reason
`helper-complete-systemd-daemon-reload`, kind `systemd-daemon-reload`, and
`EffectReplacement::Complete`. It should accept `daemon-reload` and
`--system daemon-reload` only. `daemon-reload` talks to PID 1 to reload unit
definitions, but it is treated as complete because it does not start, stop,
restart, reload, or signal any service unit.

Replace `SystemdEnableDisableAdapter` with `SystemdUnitStateAdapter` matching
`enable`, `disable`, and `preset`. It should:

- reject `--now`, `--user`, `--global`, `--runtime`, and `preset-all` by not
  matching so blocked-class review rows win;
- collect unit names after the action;
- return `EffectReplacement::Complete` with reason
  `helper-complete-systemd-unit-state` only when every unit is present in
  `input.payload.systemd_units`;
- otherwise return `EffectReplacement::Partial` with reason
  `known-helper-partial-coverage`.

Update `known_effect_classification` to accept `reason_code` and `extra`:

```rust
fn known_effect_classification(
    adapter: &dyn ScriptletEffectAdapter,
    invocation: &CommandInvocation,
    kind: &str,
    replacement: EffectReplacement,
    path: Option<String>,
    reason_code: &str,
    extra: BTreeMap<String, toml::Value>,
) -> ScriptletClassification
```

- [ ] **Step 6: Update support matrix adapter rows**

Update `adapter_entry()` rows for:

- `ldconfig/v2`
- `systemd-daemon-reload/v2`
- `systemd-unit-state/v1`

Use reason codes:

- `helper-complete-ldconfig`
- `helper-complete-systemd-daemon-reload`
- `helper-complete-systemd-unit-state`

Keep the old v1 IDs out of the default registry.

Update the existing
`adapter_registry_has_stable_builtin_order_and_unique_ids` test in
`adapters.rs` so it no longer expects the Goal 3a IDs
`ldconfig/v1`, `systemd-daemon-reload/v1`, or
`systemd-enable-disable/v1`. Use the new ordered prefix:

```rust
assert_eq!(
    &ids[..4],
    &[
        "native-free/v1",
        "ldconfig/v2",
        "systemd-daemon-reload/v2",
        "systemd-unit-state/v1",
    ]
);
```

Later tasks will extend this expected list as new adapters are registered.

- [ ] **Step 7: Verify Task 3**

Run:

```bash
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core support_matrix
```

Expected: all pass.

- [ ] **Step 8: Commit Task 3**

```bash
git add crates/conary-core/src/ccs/convert/adapters.rs \
        crates/conary-core/src/ccs/convert/blocked_classes.rs \
        crates/conary-core/src/ccs/convert/support_matrix.rs
git commit -m "feat(scriptlets): complete safe ldconfig and systemd adapters"
```

## Task 4: tmpfiles And sysusers Adapters

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/adapters.rs`
- Modify: `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`
- Test: `crates/conary-core/src/ccs/convert/adapters.rs`
- Test: `crates/conary-core/src/ccs/convert/blocked_classes.rs`

- [ ] **Step 1: Add failing tmpfiles/sysusers tests**

Add to `adapters.rs`:

```rust
#[test]
fn tmpfiles_create_is_complete_with_packaged_config() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload
        .tmpfiles_configs
        .insert("/usr/lib/tmpfiles.d/demo.conf".to_string());

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemd-tmpfiles", &["--create", "/usr/lib/tmpfiles.d/demo.conf"]),
        payload: &payload,
    });

    let ScriptletClassification::Known { reason_code, effects } = classification else {
        panic!("tmpfiles create should be known");
    };
    assert_eq!(reason_code, "helper-complete-tmpfiles-create");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "tmpfiles");
}

#[test]
fn tmpfiles_remove_and_boot_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    for argv in [vec!["--remove"], vec!["--boot", "--create"]] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemd-tmpfiles", &argv),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Review { reason_code, class_id }
                if reason_code == "review-class-tmpfiles-noncreate"
                    && class_id.as_deref() == Some("tmpfiles-noncreate")
        ));
    }
}

#[test]
fn sysusers_is_complete_with_packaged_config() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload
        .sysusers_configs
        .insert("/usr/lib/sysusers.d/demo.conf".to_string());

    let classification = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("systemd-sysusers", &["/usr/lib/sysusers.d/demo.conf"]),
        payload: &payload,
    });

    let ScriptletClassification::Known { reason_code, effects } = classification else {
        panic!("sysusers should be known");
    };
    assert_eq!(reason_code, "helper-complete-sysusers");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "sysusers");
}

#[test]
fn sysusers_replace_and_root_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    for argv in [vec!["--replace=/usr/lib/sysusers.d/demo.conf"], vec!["--root=/tmp/root"]] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemd-sysusers", &argv),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Review { reason_code, class_id }
                if reason_code == "review-class-sysusers-nonstandard"
                    && class_id.as_deref() == Some("sysusers-nonstandard")
        ));
    }
}
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary-core tmpfiles_create_is_complete_with_packaged_config
cargo test -p conary-core sysusers_is_complete_with_packaged_config
```

Expected: unknown classifications.

- [ ] **Step 3: Add tmpfiles/sysusers review classes**

Add to `BlockedClassRegistry::default()`:

```rust
review_class(
    "tmpfiles-noncreate",
    "tmpfiles cleanup, removal, boot-only, user, purge, replace, or stdin forms need lifecycle-specific review.",
    "review-class-tmpfiles-noncreate",
    &[],
    &[
        "systemd-tmpfiles --remove*",
        "systemd-tmpfiles --clean*",
        "systemd-tmpfiles --purge*",
        "systemd-tmpfiles --boot*",
        "systemd-tmpfiles --user*",
        "systemd-tmpfiles --replace*",
    ],
    "Add tmpfiles lifecycle semantics and remove/purge ordering tests.",
),
review_class(
    "sysusers-nonstandard",
    "sysusers root, replace, or stdin forms need explicit target-root and input modeling.",
    "review-class-sysusers-nonstandard",
    &[],
    &["systemd-sysusers --replace*", "systemd-sysusers --root*"],
    "Add sysusers root/input modeling before claiming replacement.",
),
```

Add support-matrix fixture names for both classes.

- [ ] **Step 4: Add tmpfiles/sysusers adapters**

Add `SystemdTmpfilesCreateAdapter` and `SystemdSysusersAdapter` to the default
adapter vector after systemd unit-state.

Adapter IDs and reason codes:

```rust
systemd-tmpfiles-create/v1 -> helper-complete-tmpfiles-create
systemd-sysusers/v1 -> helper-complete-sysusers
```

`SystemdTmpfilesCreateAdapter` should match only `systemd-tmpfiles` invocations
whose first semantic command is `--create`, that do not contain review flags,
and whose explicit `.conf` paths are all in `payload.tmpfiles_configs`. If no
explicit paths are present, require `payload.tmpfiles_configs` to be non-empty.

`SystemdSysusersAdapter` should match only `systemd-sysusers` invocations
without `--replace`, `--root`, stdin markers, or unknown options. If explicit
`.conf` paths are present, require all paths to be in `payload.sysusers_configs`.
If no explicit paths are present, require `payload.sysusers_configs` to be
non-empty.

Emit `extra` values:

```rust
BTreeMap::from([(
    "configs".to_string(),
    toml::Value::Array(configs.into_iter().map(toml::Value::String).collect()),
)])
```

- [ ] **Step 5: Add support matrix rows**

Update `adapter_entry()` for:

- `systemd-tmpfiles-create/v1`
- `systemd-sysusers/v1`

Use fixture names:

- `adapter-tmpfiles-create`
- `adapter-sysusers`

- [ ] **Step 6: Verify Task 4**

Run:

```bash
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core support_matrix
```

Expected: all pass.

- [ ] **Step 7: Commit Task 4**

```bash
git add crates/conary-core/src/ccs/convert/adapters.rs \
        crates/conary-core/src/ccs/convert/blocked_classes.rs \
        crates/conary-core/src/ccs/convert/support_matrix.rs
git commit -m "feat(scriptlets): add tmpfiles and sysusers adapters"
```

## Task 5: Alternatives And Cache Refresh Adapters

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/adapters.rs`
- Modify: `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`
- Test: `crates/conary-core/src/ccs/convert/adapters.rs`

- [ ] **Step 1: Add failing alternatives tests**

Add to `adapters.rs`:

```rust
#[test]
fn alternatives_install_and_remove_are_complete_when_parseable() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    let install = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation(
            "update-alternatives",
            &[
                "--install",
                "/usr/bin/editor",
                "editor",
                "/usr/bin/demo-editor",
                "50",
                "--slave",
                "/usr/share/man/man1/editor.1.gz",
                "editor.1.gz",
                "/usr/share/man/man1/demo-editor.1.gz",
                "--slave",
                "/usr/share/man/man1/view.1.gz",
                "view.1.gz",
                "/usr/share/man/man1/demo-view.1.gz",
            ],
        ),
        payload: &payload,
    });
    let ScriptletClassification::Known { reason_code, effects } = install else {
        panic!("alternatives install should be known");
    };
    assert_eq!(reason_code, "helper-complete-alternatives-registration");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "alternatives");
    assert_eq!(effects[0].path.as_deref(), Some("/usr/bin/editor"));

    let remove = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("alternatives", &["--remove", "editor", "/usr/bin/demo-editor"]),
        payload: &payload,
    });
    assert!(matches!(
        remove,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-alternatives-registration"
    ));
}

#[test]
fn alternatives_interactive_and_broad_actions_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    for argv in [
        vec!["--config", "editor"],
        vec!["--remove-all", "editor"],
        vec!["--remove", "editor"],
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("update-alternatives", &argv),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Review { reason_code, class_id }
                if reason_code == "review-class-alternatives-interactive-or-broad"
                    && class_id.as_deref() == Some("alternatives-interactive-or-broad")
        ));
    }
}
```

- [ ] **Step 2: Add failing cache refresh tests**

Add to `adapters.rs`:

```rust
#[test]
fn cache_refresh_known_forms_are_complete_with_payload_inputs() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload
        .cache_inputs
        .entry("mime-db".to_string())
        .or_default()
        .insert("/usr/share/mime/packages/demo.xml".to_string());
    payload
        .cache_inputs
        .entry("desktop-db".to_string())
        .or_default()
        .insert("/usr/share/applications/demo.desktop".to_string());
    payload
        .cache_inputs
        .entry("icon-cache".to_string())
        .or_default()
        .insert("/usr/share/icons/hicolor/16x16/apps/demo.png".to_string());
    payload
        .cache_inputs
        .entry("gsettings".to_string())
        .or_default()
        .insert("/usr/share/glib-2.0/schemas/org.example.demo.gschema.xml".to_string());
    payload
        .cache_inputs
        .entry("font-cache".to_string())
        .or_default()
        .insert("/usr/share/fonts/demo/demo.ttf".to_string());

    let mime = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("update-mime-database", &["/usr/share/mime"]),
        payload: &payload,
    });
    let ScriptletClassification::Known { reason_code, effects } = mime else {
        panic!("mime cache refresh should be known");
    };
    assert_eq!(reason_code, "helper-complete-cache-refresh");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "cache-refresh");
    assert_eq!(effects[0].extra["cache_kind"], toml::Value::String("mime-db".to_string()));

    let desktop = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("update-desktop-database", &["-q", "/usr/share/applications"]),
        payload: &payload,
    });
    assert!(matches!(
        desktop,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let icons = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation(
            "gtk-update-icon-cache",
            &["--force", "--quiet", "/usr/share/icons/hicolor"],
        ),
        payload: &payload,
    });
    assert!(matches!(
        icons,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let icons_combined_flags = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("gtk-update-icon-cache", &["-qf", "/usr/share/icons/hicolor"]),
        payload: &payload,
    });
    assert!(matches!(
        icons_combined_flags,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let schemas = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("glib-compile-schemas", &["--allow-any-name", "/usr/share/glib-2.0/schemas"]),
        payload: &payload,
    });
    assert!(matches!(
        schemas,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let schemas_default_path = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("glib-compile-schemas", &[]),
        payload: &payload,
    });
    assert!(matches!(
        schemas_default_path,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let fonts = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("fc-cache", &["-fs"]),
        payload: &payload,
    });
    assert!(matches!(
        fonts,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let fonts_with_dir = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("fc-cache", &["-f", "/usr/share/fonts/demo"]),
        payload: &payload,
    });
    assert!(matches!(
        fonts_with_dir,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));
}

#[test]
fn cache_refresh_nonstandard_paths_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    for path in ["/opt/vendor/mime", "/usr/local/share/mime"] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("update-mime-database", &[path]),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Review { reason_code, class_id }
                if reason_code == "review-class-cache-refresh-nonstandard"
                    && class_id.as_deref() == Some("cache-refresh-nonstandard")
        ));
    }
}
```

- [ ] **Step 3: Add failing review-class tests for legacy desktop/doc helpers**

Add to `blocked_classes.rs`:

```rust
#[test]
fn blocked_classes_review_gconf_and_install_info_helpers() {
    let registry = BlockedClassRegistry::default();

    let gconf = registry.match_invocation(&invocation(
        "gconftool-2",
        &["--makefile-install-rule", "/etc/gconf/schemas/demo.schemas"],
    ));
    assert_eq!(gconf.unwrap().reason_code, "review-class-gconf-schema");

    let info = registry.match_invocation(&invocation(
        "install-info",
        &["/usr/share/info/demo.info.gz", "/usr/share/info/dir"],
    ));
    assert_eq!(info.unwrap().reason_code, "review-class-install-info");
}
```

- [ ] **Step 4: Run failing tests**

Run:

```bash
cargo test -p conary-core alternatives_install_and_remove_are_complete_when_parseable
cargo test -p conary-core cache_refresh_known_forms_are_complete_with_payload_inputs
cargo test -p conary-core cache_refresh_nonstandard_paths_are_review
cargo test -p conary-core blocked_classes_review_gconf_and_install_info_helpers
```

Expected: adapter tests return unknown classifications, and the blocked-class
test fails because the review classes are not registered yet.

- [ ] **Step 5: Add review classes**

Add to `BlockedClassRegistry::default()`:

```rust
review_class(
    "gconf-schema",
    "GConf schema installation mutates an obsolete desktop configuration registry.",
    "review-class-gconf-schema",
    &["gconftool", "gconftool-2"],
    &[],
    "Migrate obsolete GConf schemas to GSettings XML schemas and glib-compile-schemas.",
),
review_class(
    "install-info",
    "GNU Info directory registration is a common documentation index mutation that is not yet modeled.",
    "review-class-install-info",
    &["install-info"],
    &[],
    "Model Info manual registration as a declarative documentation index/cache effect.",
),
review_class(
    "alternatives-interactive-or-broad",
    "Interactive or broad alternatives commands can alter administrator choice state.",
    "review-class-alternatives-interactive-or-broad",
    &[],
    &[
        "update-alternatives --config*",
        "update-alternatives --set*",
        "update-alternatives --auto*",
        "update-alternatives --all*",
        "update-alternatives --remove-all*",
        "alternatives --config*",
        "alternatives --set*",
        "alternatives --auto*",
        "alternatives --all*",
        "alternatives --remove-all*",
    ],
    "Model administrator alternatives state before claiming replacement.",
),
review_class(
    "cache-refresh-nonstandard",
    "Cache refresh command uses nonstandard paths or options outside the bootstrap adapter contract.",
    "review-class-cache-refresh-nonstandard",
    &[],
    &[
        "update-mime-database /opt*",
        "update-mime-database /usr/local*",
        "update-desktop-database /opt*",
        "update-desktop-database /usr/local*",
        "gtk-update-icon-cache /opt*",
        "gtk-update-icon-cache /usr/local*",
        "glib-compile-schemas /opt*",
        "glib-compile-schemas /usr/local*",
        "fc-cache /opt*",
        "fc-cache /usr/local*",
    ],
    "Add a cache-specific adapter rule for the nonstandard path or keep package review-only.",
),
```

Add fixture names for all four classes:

- `review-class-gconf-schema`
- `review-class-install-info`
- `review-class-alternatives-interactive-or-broad`
- `review-class-cache-refresh-nonstandard`

- [ ] **Step 6: Add alternatives adapter**

Add `AlternativesRegistrationAdapter` with ID
`alternatives-registration/v1`. Match commands `update-alternatives` and
`alternatives`.

Parsing rules:

- `--install LINK NAME PATH PRIORITY` is complete only when all four arguments
  exist and `PRIORITY` parses as `i32`;
- zero or more `--slave LINK NAME PATH` groups may follow;
- `--remove NAME PATH` is complete only when exactly two positional arguments
  follow `--remove`;
- `--remove NAME` rejects to `review-class-alternatives-interactive-or-broad`
  because it can behave like broad link-group removal rather than a specific
  alternative-path removal;
- malformed forms return `Review` with
  `review-class-alternatives-interactive-or-broad`.

Effect fields:

```rust
kind = "alternatives"
replacement = EffectReplacement::Complete
reason_code = "helper-complete-alternatives-registration"
path = Some(master_link_or_removed_path)
extra = {
    "action": "install" | "remove",
    "name": alternative_name,
    "target": target_path,
    "priority": priority,
    "slaves": ["link name path", ...]
}
```

The `alternatives_install_and_remove_are_complete_when_parseable` test includes
multiple `--slave` triplets. Preserve every triplet in stable order.

- [ ] **Step 7: Add cache refresh adapter**

Add `CacheRefreshAdapter` with ID `cache-refresh/v1`. Complete mappings:

```rust
update-mime-database /usr/share/mime -> mime-db
update-desktop-database [-q|--quiet] /usr/share/applications -> desktop-db
gtk-update-icon-cache [benign flags] /usr/share/icons/THEME -> icon-cache
glib-compile-schemas [--allow-any-name] [/usr/share/glib-2.0/schemas] -> gsettings
fc-cache [benign flags] [FONT_DIR...] -> font-cache
```

For command forms with path arguments, require the path to be a standard path
for the cache kind. For every cache kind, require
`input.payload.has_cache_input(kind)` before returning complete. Without payload
input, return `Known` with `EffectReplacement::Partial` and reason
`known-helper-partial-coverage`.

For `gtk-update-icon-cache`, do not match a literal wildcard. Strip optional
flags `-f`, `--force`, `-q`, `--quiet`, `--ignore-theme-index`, and combined
short forms such as `-qf`, then require the remaining directory argument to
start with `/usr/share/icons/`. To return complete, verify that
`payload.cache_inputs["icon-cache"]` contains at least one icon path under the
same theme directory prefix, for example `/usr/share/icons/hicolor/`.

For `update-desktop-database`, accept `-q` and `--quiet` before the standard
directory argument.

For `glib-compile-schemas`, accept `--allow-any-name`. Also accept a no-argument
form as `/usr/share/glib-2.0/schemas` when `payload.has_cache_input("gsettings")`
is true.

For `fc-cache`, accept benign flags `-s`, `--system-only`, `-f`, `--force`,
`-r`, `--really-force`, `-v`, `--verbose`, and combined short forms such as
`-fs` or `-srv`. Directory operands, when present, must be packaged font
directories or standard font roots.

Effect fields:

```rust
kind = "cache-refresh"
replacement = Complete or Partial
reason_code = "helper-complete-cache-refresh" or "known-helper-partial-coverage"
path = Some(cache_root)
extra["cache_kind"] = cache_kind
```

- [ ] **Step 8: Add support matrix rows**

Update `adapter_entry()` for:

- `alternatives-registration/v1`
- `cache-refresh/v1`

Add fixture names:

- `adapter-alternatives-registration`
- `adapter-cache-refresh`

Update `adapter_registry_has_stable_builtin_order_and_unique_ids` one more time
so it expects the full Goal 3b adapter order:

```rust
assert_eq!(
    ids,
    vec![
        "native-free/v1",
        "ldconfig/v2",
        "systemd-daemon-reload/v2",
        "systemd-unit-state/v1",
        "systemd-tmpfiles-create/v1",
        "systemd-sysusers/v1",
        "alternatives-registration/v1",
        "cache-refresh/v1",
    ]
);
```

- [ ] **Step 9: Verify Task 5**

Run:

```bash
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core support_matrix
```

Expected: all pass.

- [ ] **Step 10: Commit Task 5**

```bash
git add crates/conary-core/src/ccs/convert/adapters.rs \
        crates/conary-core/src/ccs/convert/blocked_classes.rs \
        crates/conary-core/src/ccs/convert/support_matrix.rs
git commit -m "feat(scriptlets): add alternatives and cache refresh adapters"
```

## Task 6: Converter Integration And Scope Guards

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Test: `crates/conary-core/src/ccs/convert/converter.rs`

- [ ] **Step 1: Add converter integration test for complete payload-backed helpers**

Add to the converter test module:

```rust
#[test]
fn conversion_classification_reports_complete_payload_backed_helpers() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "\
/sbin/ldconfig
systemctl daemon-reload
systemctl enable demo.service
systemd-tmpfiles --create /usr/lib/tmpfiles.d/demo.conf
systemd-sysusers /usr/lib/sysusers.d/demo.conf
update-mime-database /usr/share/mime
"
        .to_string(),
        flags: None,
    }];
    let mut files = make_test_files();
    files.extend([
        ExtractedFile {
            path: "/usr/lib/systemd/system/demo.service".to_string(),
            content: b"[Service]\nExecStart=/usr/bin/demo\n".to_vec(),
            size: 32,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/lib/tmpfiles.d/demo.conf".to_string(),
            content: b"d /run/demo 0755 root root -\n".to_vec(),
            size: 28,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/lib/sysusers.d/demo.conf".to_string(),
            content: b"u demo - \"Demo User\" /run/demo -\n".to_vec(),
            size: 32,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
        ExtractedFile {
            path: "/usr/share/mime/packages/demo.xml".to_string(),
            content: b"<mime-info/>".to_vec(),
            size: 12,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        },
    ]);
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
        .expect("conversion succeeds");

    let complete_effects = result
        .scriptlet_classification
        .entries
        .iter()
        .filter_map(|entry| match &entry.classification {
            ScriptletClassification::Known { effects, .. } => Some(effects),
            _ => None,
        })
        .flatten()
        .filter(|effect| effect.replacement == EffectReplacement::Complete)
        .count();

    assert_eq!(
        complete_effects, 6,
        "all 6 known helper invocations should be complete"
    );
    assert!(result.build_result.manifest.legacy_scriptlets.is_none());
    assert!(
        result
            .detected_hooks
            .systemd
            .iter()
            .any(|hook| hook.unit == "demo.service")
    );
}
```

Add imports if needed:

```rust
use crate::ccs::convert::effects::ScriptletClassification;
use crate::ccs::legacy_scriptlets::EffectReplacement;
```

- [ ] **Step 2: Add converter integration test for foreign/private helpers**

Add:

```rust
#[test]
fn conversion_classification_reviews_deb_private_helpers_without_manifest_changes() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "deb-systemd-helper enable demo.service\ndebconf-communicate demo\n".to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "deb", "sha256:test")
        .expect("conversion succeeds");

    assert!(result.scriptlet_classification.entries.iter().any(|entry| {
        matches!(
            &entry.classification,
            ScriptletClassification::Review { reason_code, class_id }
                if reason_code == "review-class-deb-systemd-helper"
                    && class_id.as_deref() == Some("deb-systemd-helper")
        )
    }));
    assert!(result.scriptlet_classification.entries.iter().any(|entry| {
        matches!(
            &entry.classification,
            ScriptletClassification::Review { reason_code, class_id }
                if reason_code == "review-class-debconf"
                    && class_id.as_deref() == Some("debconf")
        )
    }));
    assert!(result.build_result.manifest.legacy_scriptlets.is_none());
}
```

- [ ] **Step 3: Run failing converter tests**

Run:

```bash
cargo test -p conary-core conversion_classification_reports_complete_payload_backed_helpers
cargo test -p conary-core conversion_classification_reviews_deb_private_helpers_without_manifest_changes
```

Expected: pass if Tasks 1-5 are complete; otherwise expose integration gaps.

- [ ] **Step 4: Fix integration gaps without changing output behavior**

Allowed fixes:

- update imports;
- update test helper literals for `ScriptletEffectEvidence.extra`;
- ensure `classify_scriptlets(metadata, files)` is called after `files` are
  available and before output writing;
- ensure review classes run before adapter matching.
- add assertions or comments proving classification remains pure data flow and
  does not call any host command execution API.

Not allowed:

- adding `legacy_scriptlets` to `CcsManifest`;
- adding Remi database fields;
- changing install or replay code.

- [ ] **Step 5: Verify Task 6**

Run:

```bash
cargo test -p conary-core conversion_integration
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core support_matrix
```

Expected: all pass.

- [ ] **Step 6: Commit Task 6**

```bash
git add crates/conary-core/src/ccs/convert/converter.rs
git commit -m "feat(scriptlets): classify complete bootstrap helpers in conversion"
```

## Task 7: Goal Verification

**Files:**

- Modify only as required by formatting.

- [ ] **Step 1: Run targeted tests**

Run:

```bash
cargo test -p conary-core payload_hints
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core support_matrix
cargo test -p conary-core conversion_integration
```

Expected: all pass.

- [ ] **Step 2: Run package-level regression tests**

Run:

```bash
cargo test -p conary-core
cargo test -p conary
```

Expected: all pass.

- [ ] **Step 3: Run workspace lint and format gates**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

Expected: all pass with no warnings and no whitespace errors.

- [ ] **Step 4: Verify scope**

Run:

```bash
git diff --stat origin/main..HEAD
git diff --name-only origin/main..HEAD
```

Expected changed implementation files are limited to:

- `crates/conary-core/src/ccs/convert/mod.rs`
- `crates/conary-core/src/ccs/convert/payload_hints.rs`
- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- `crates/conary-core/src/ccs/convert/support_matrix.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`

If formatting touches additional files, inspect them before committing. There
should be no database migrations, Remi handler/job changes, or install/remove
changes.

- [ ] **Step 5: Final commit if needed**

If verification required fixes:

```bash
git add crates/conary-core/src/ccs/convert
git commit -m "fix(scriptlets): tighten bootstrap adapter verification"
```

If no fixes were needed, do not create an empty commit.

## Completion Criteria

Goal 3b is complete when:

- safe `ldconfig`, systemd metadata, tmpfiles, sysusers, alternatives, and cache
  refresh forms produce complete effect evidence only under the documented
  constraints;
- nonstandard, runtime, interactive, source-family private, and unmodeled forms
  produce stable review or blocked reason codes;
- every adapter and new review class has support-matrix coverage;
- converter integration exposes passive classification evidence without
  writing `legacy_scriptlets` into the manifest;
- no install/update/remove/replay, Remi publication, or DB schema behavior
  changes;
- all verification commands in Task 7 pass.
