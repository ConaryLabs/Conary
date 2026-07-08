# Generic LSM Security Policy Intent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the first additive generic LSM security-policy intent slice so converted packages can carry typed SELinux and AppArmor policy metadata without replaying distro helper commands during conversion.

**Architecture:** Keep the initial authority inside passive legacy conversion metadata. Add a reusable `SecurityPolicyIntent` DTO in `conary-core`, bridge existing `selinux-policy/v1` effects into that DTO, classify AppArmor helper calls as typed review intent, and project the normalized intent into the existing Remi scriptlet evidence queue. Do not execute host LSM tooling or claim cross-provider SELinux-to-AppArmor translation in this plan.

**Tech Stack:** Rust, serde, TOML/JSON DTOs, SQLite migrations, existing CCS legacy scriptlet bundle metadata, existing Remi scriptlet evidence queue, cargo test, docs audit scripts.

## Global Constraints

- Public package and repository support claims stay limited to Fedora 44, Ubuntu 26.04, and Arch.
- LSM helper commands are evidence sources, not install authority.
- No generic SELinux-to-AppArmor translation is allowed by default.
- Absent providers are not failures unless the package declares policy as required for safe runtime.
- Public Remi serving requires fully modeled intent and deterministic provider behavior, never raw scriptlet replay.
- The first implementation is additive: older bundles without generic LSM intent remain valid.
- Provider planning and host policy reconciliation are separate implementation slices after this metadata shape is stable.
- Raw helper execution during conversion remains forbidden.

---

## Scope Boundary

This plan implements the metadata and review-data foundation from `docs/superpowers/specs/archive/2026-07-07-generic-lsm-security-policy-intent-design.md`.

Included:

- A generic `SecurityPolicyIntent` schema in `conary-core`.
- Additive fields on `LegacyScriptletBundle`, `LegacyScriptletEntry`, and `ScriptletBundleSummary`.
- SELinux bridge from current `selinux-policy/v1` adapter effects to generic LSM intent.
- AppArmor helper command inventory as typed review intent, not public-ready replacement.
- Remi queue projection through a v76 `security_policy_intents_json` sample column.
- Golden/support-matrix proof that SELinux remains public-ready when supported and AppArmor remains review-only until modeled.

Not included:

- No host facts detection from `/sys/kernel/security/lsm`, `getenforce`, `aa-status`, or package-manager state.
- No install-time policy application.
- No SELinux-to-AppArmor translation.
- No AppArmor public-ready adapter claim.
- No CCS v2 native authority field for first-class authored generic LSM policy.

## File Structure

- Create `crates/conary-core/src/ccs/security_policy.rs`
  - Owns generic LSM intent DTOs and string enums.
- Modify `crates/conary-core/src/ccs/mod.rs`
  - Exports the new module.
- Modify `crates/conary-core/src/ccs/legacy_scriptlets.rs`
  - Adds additive `security_policy_intents` vectors to bundle and entry metadata.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs`
  - Adds summary-level `security_policy_intents`.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`
  - Aggregates entry policy intents into summaries.
- Create `crates/conary-core/src/ccs/convert/security_policy.rs`
  - Bridges adapter effects into generic policy intents.
- Modify `crates/conary-core/src/ccs/convert/mod.rs`
  - Registers the conversion helper module.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`
  - Stores policy intents on each built legacy scriptlet entry.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`
  - Adds review-only policy intent from AppArmor blocked/review classifications.
- Modify `crates/conary-core/src/ccs/convert/blocked_classes.rs`
  - Expands AppArmor helper command coverage to documented common tools.
- Modify `crates/conary-core/src/ccs/convert/support_matrix.rs`
  - Keeps `selinux-policy/v1` known and makes the AppArmor row explicit review-only typed intent.
- Modify `crates/conary-core/src/ccs/convert/golden_fixtures.rs`
  - Adds/adjusts fixture expectations for generic LSM metadata.
- Modify `crates/conary-core/src/db/schema.rs`
  - Bumps schema version from 75 to 76 and routes migration v76.
- Modify `crates/conary-core/src/db/migrations/v41_current.rs`
  - Adds `security_policy_intents_json` to `scriptlet_evidence_cluster_samples`.
- Modify `crates/conary-core/src/db/models/scriptlet_evidence.rs`
  - Persists and reads the new sample JSON field.
- Modify `apps/remi/src/server/scriptlet_evidence_queue/aggregation.rs`
  - Writes generic policy intent JSON into queue samples.
- Modify `apps/remi/src/server/scriptlet_evidence_queue/normalization.rs`
  - Sanitizes generic policy intent values using existing path/token normalization.
- Modify `apps/remi/src/server/scriptlet_evidence_queue/packet.rs`
  - Exposes policy intent in private and public-sanitized packets without raw paths.
- Modify `docs/modules/ccs.md`
  - Documents the generic LSM metadata lane.
- Modify `docs/modules/remi.md`
  - Documents queue packet projection for LSM intent.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Registers this plan.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Regenerated inventory includes this plan.

## Task 1: Add Generic Security Policy Intent DTOs

**Files:**
- Create: `crates/conary-core/src/ccs/security_policy.rs`
- Modify: `crates/conary-core/src/ccs/mod.rs`
- Modify: `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`
- Modify: `crates/conary-core/src/ccs/manifest.rs`

**Interfaces:**
- Produces: `SecurityPolicyIntent`, `SecurityPolicyProvider`, `SecurityPolicyFallback`, `SecurityPolicyReconciliationState`, `SecurityPolicySource`, `SecurityPolicyScope`, `SecurityPolicyRequirements`, `SecurityPolicyPayloadEvidence`, and `SecurityPolicyReconciliation`.
- Produces: `LegacyScriptletBundle.security_policy_intents: Vec<SecurityPolicyIntent>`.
- Produces: `LegacyScriptletEntry.security_policy_intents: Vec<SecurityPolicyIntent>`.
- Produces: `ScriptletBundleSummary.security_policy_intents: Vec<SecurityPolicyIntent>`.
- Consumed By: Tasks 2, 3, and 4.

- [ ] **Step 1: Write failing manifest and summary tests**

Add this test to `crates/conary-core/src/ccs/manifest.rs` near `manifest_toml_round_trips_legacy_scriptlet_bundle`:

```rust
#[test]
fn manifest_toml_round_trips_generic_security_policy_intent() {
    let body_sha256 = crate::hash::sha256_prefixed(b"restorecon /usr/share/demo\n");
    let toml = format!(
        r#"
[package]
name = "demo-policy"
version = "1.0.0"
description = "policy fixture"

[legacy_scriptlets]
schema = "conary.legacy-scriptlets.v1"
schema_revision = 1
source_format = "rpm"
source_family = "rpm"
source_distro = "fedora"
source_release = "44"
source_arch = "x86_64"
source_package = "demo-policy"
source_version = "1.0.0-1.fc44"
version_scheme = "rpm"
conversion_tool = "remi"
conversion_tool_version = "0.10.1"
conversion_policy = "passive-scriptlet-bundle-goal4"
target_compatibility = "conary-portable"
foreign_replay_policy = "deny"
publication_policy = "public-if-no-blocked"
publication_status = "public"
scriptlet_fidelity = "fully-replaced"

[legacy_scriptlets.decision_counts]
replaced = 1

[[legacy_scriptlets.security_policy_intents]]
schema = "conary.security-policy-intent.v1"
id = "rpm:%post:selinux-label-refresh"
provider = "selinux"
operation = "label-refresh"
fallback = "dormant"

[legacy_scriptlets.security_policy_intents.source]
source_format = "rpm"
source_distro = "fedora"
entry_id = "rpm:%post"
command = "restorecon"
argv = ["-R", "/usr/share/demo"]
adapter_id = "selinux-policy/v1"

[legacy_scriptlets.security_policy_intents.scope]
kind = "path"
paths = ["/usr/share/demo"]

[legacy_scriptlets.security_policy_intents.desired_state]
recursive = true

[legacy_scriptlets.security_policy_intents.requirements]
required_on_active_provider = false
tools = ["restorecon"]

[legacy_scriptlets.security_policy_intents.payload_evidence]
payload_backed = true
paths = ["/usr/share/demo"]

[legacy_scriptlets.security_policy_intents.reconciliation]
state = "pending"

[[legacy_scriptlets.entries]]
id = "rpm:%post"
native_slot = "%post"
phase = "post-install"
lifecycle_paths = ["post-install"]
interpreter = "/bin/sh"
body_sha256 = "{body_sha256}"
body = "restorecon /usr/share/demo\n"
native_invocation = {{ args = [], environment = [] }}
transaction_order = {{ position = "after-payload" }}
timeout_ms = 30000
decision = "replaced"
reason_code = "helper-complete-selinux-policy"

[[legacy_scriptlets.entries.security_policy_intents]]
schema = "conary.security-policy-intent.v1"
id = "rpm:%post:selinux-label-refresh"
provider = "selinux"
operation = "label-refresh"
fallback = "dormant"

[legacy_scriptlets.entries.security_policy_intents.source]
source_format = "rpm"
source_distro = "fedora"
entry_id = "rpm:%post"
command = "restorecon"
argv = ["-R", "/usr/share/demo"]
adapter_id = "selinux-policy/v1"

[legacy_scriptlets.entries.security_policy_intents.scope]
kind = "path"
paths = ["/usr/share/demo"]

[legacy_scriptlets.entries.security_policy_intents.desired_state]
recursive = true

[legacy_scriptlets.entries.security_policy_intents.requirements]
required_on_active_provider = false
tools = ["restorecon"]

[legacy_scriptlets.entries.security_policy_intents.payload_evidence]
payload_backed = true
paths = ["/usr/share/demo"]

[legacy_scriptlets.entries.security_policy_intents.reconciliation]
state = "pending"
"#
    );

    let manifest = CcsManifest::parse(&toml).expect("parse manifest");
    let bundle = manifest.legacy_scriptlets.as_ref().unwrap();

    assert_eq!(bundle.security_policy_intents.len(), 1);
    assert_eq!(bundle.security_policy_intents[0].provider.as_str(), "selinux");
    assert_eq!(bundle.security_policy_intents[0].fallback.as_str(), "dormant");
    assert_eq!(bundle.entries[0].security_policy_intents.len(), 1);

    let encoded = manifest.to_toml().expect("serialize manifest");
    assert!(encoded.contains("[[legacy_scriptlets.security_policy_intents]]"));
    assert!(encoded.contains("[[legacy_scriptlets.entries.security_policy_intents]]"));

    let decoded = CcsManifest::parse(&encoded).expect("parse serialized manifest");
    let decoded_bundle = decoded.legacy_scriptlets.as_ref().unwrap();
    assert_eq!(
        decoded_bundle.security_policy_intents[0].reconciliation.state.as_str(),
        "pending"
    );
}
```

Add this test to `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`:

```rust
#[test]
fn scriptlet_bundle_summary_defaults_include_empty_security_policy_intents() {
    let summary = ScriptletBundleSummary::default();

    assert!(summary.security_policy_intents.is_empty());
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core manifest_toml_round_trips_generic_security_policy_intent
cargo test -p conary-core scriptlet_bundle_summary_defaults_include_empty_security_policy_intents
```

Expected: FAIL with missing `security_policy` module or missing fields.

- [ ] **Step 3: Add the DTO module**

Create `crates/conary-core/src/ccs/security_policy.rs`:

```rust
// conary-core/src/ccs/security_policy.rs
//! Generic Linux Security Module policy intent metadata.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

pub const SECURITY_POLICY_INTENT_SCHEMA_V1: &str = "conary.security-policy-intent.v1";

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
            Unknown(String),
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $value,)+
                    Self::Unknown(value) => value.as_str(),
                }
            }

            pub fn is_known(&self) -> bool {
                !matches!(self, Self::Unknown(_))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct Visitor;

                impl<'de> serde::de::Visitor<'de> for Visitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a string enum value")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        Ok(match value {
                            $($value => $name::$variant,)+
                            other => $name::Unknown(other.to_string()),
                        })
                    }
                }

                deserializer.deserialize_str(Visitor)
            }
        }
    };
}

string_enum! {
    pub enum SecurityPolicyProvider {
        Selinux => "selinux",
        Apparmor => "apparmor",
        Tomoyo => "tomoyo",
        Smack => "smack",
        Landlock => "landlock",
        Any => "any",
    }
}

impl Default for SecurityPolicyProvider {
    fn default() -> Self {
        Self::Any
    }
}

string_enum! {
    pub enum SecurityPolicyFallback {
        Dormant => "dormant",
        Warning => "warning",
        Degraded => "degraded",
        BlockOnEnforcingTarget => "block-on-enforcing-target",
    }
}

impl Default for SecurityPolicyFallback {
    fn default() -> Self {
        Self::Dormant
    }
}

string_enum! {
    pub enum SecurityPolicyReconciliationState {
        Applied => "applied",
        Dormant => "dormant",
        Pending => "pending",
        Degraded => "degraded",
        Blocked => "blocked",
        Review => "review",
    }
}

impl Default for SecurityPolicyReconciliationState {
    fn default() -> Self {
        Self::Pending
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyIntent {
    pub schema: String,
    pub id: String,
    #[serde(default)]
    pub source: SecurityPolicySource,
    #[serde(default)]
    pub provider: SecurityPolicyProvider,
    pub operation: String,
    #[serde(default)]
    pub scope: SecurityPolicyScope,
    #[serde(default)]
    pub desired_state: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub requirements: SecurityPolicyRequirements,
    #[serde(default)]
    pub fallback: SecurityPolicyFallback,
    #[serde(default)]
    pub payload_evidence: SecurityPolicyPayloadEvidence,
    #[serde(default)]
    pub reconciliation: SecurityPolicyReconciliation,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicySource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_distro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyScope {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyRequirements {
    #[serde(default)]
    pub required_on_active_provider: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyPayloadEvidence {
    #[serde(default)]
    pub payload_backed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyReconciliation {
    #[serde(default)]
    pub state: SecurityPolicyReconciliationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_provider: Option<String>,
}
```

- [ ] **Step 4: Wire additive bundle and summary fields**

In `crates/conary-core/src/ccs/mod.rs`, add:

```rust
pub mod security_policy;
```

In `crates/conary-core/src/ccs/legacy_scriptlets.rs`, import and add fields:

```rust
use crate::ccs::security_policy::SecurityPolicyIntent;
```

Add this field to `LegacyScriptletBundle` after `unsupported_class_counts`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub security_policy_intents: Vec<SecurityPolicyIntent>,
```

Add this field to `LegacyScriptletEntry` after `boot_security_intents`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub security_policy_intents: Vec<SecurityPolicyIntent>,
```

Update every `LegacyScriptletBundle` and `LegacyScriptletEntry` initializer by setting `security_policy_intents: Vec::new()` until Task 2 fills real values.

In `crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs`, add:

```rust
use crate::ccs::security_policy::SecurityPolicyIntent;
```

and add this field to `ScriptletBundleSummary`:

```rust
#[serde(default)]
pub security_policy_intents: Vec<SecurityPolicyIntent>,
```

Set the default to `Vec::new()`.

In `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`, aggregate bundle and entry values:

```rust
let security_policy_intents = bundle
    .security_policy_intents
    .iter()
    .chain(
        bundle
            .entries
            .iter()
            .flat_map(|entry| entry.security_policy_intents.iter()),
    )
    .cloned()
    .collect::<Vec<_>>();
```

and include `security_policy_intents` in the `ScriptletBundleSummary` initializer.

- [ ] **Step 5: Run Task 1 tests**

Run:

```bash
cargo test -p conary-core manifest_toml_round_trips_generic_security_policy_intent
cargo test -p conary-core scriptlet_bundle_summary_defaults_include_empty_security_policy_intents
cargo test -p conary-core ccs::legacy_scriptlets
```

Expected: PASS.

- [ ] **Step 6: Commit Task 1**

```bash
git add crates/conary-core/src/ccs/security_policy.rs crates/conary-core/src/ccs/mod.rs crates/conary-core/src/ccs/legacy_scriptlets.rs crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs crates/conary-core/src/ccs/manifest.rs
git commit -m "feat(ccs): add generic LSM policy intent metadata"
```

## Task 2: Bridge SELinux Adapter Effects Into Generic Intent

**Files:**
- Create: `crates/conary-core/src/ccs/convert/security_policy.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/test_support.rs`
- Modify: `crates/conary-core/src/ccs/convert/selinux_adapters.rs`

**Interfaces:**
- Consumes: `ScriptletEffect` and `SecurityPolicyIntent` from Task 1.
- Produces: `policy_intents_from_effects(entry_id: &str, effects: &[ScriptletEffect]) -> Vec<SecurityPolicyIntent>`.
- Produces: entry-level and bundle-summary SELinux intent for `selinux-policy/v1` effects.

- [ ] **Step 1: Write failing bridge tests**

Add this test to `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`:

```rust
use super::super::ScriptletBundleSummary;

#[test]
fn selinux_policy_effects_project_generic_policy_intent() {
    let mut metadata = package_metadata("selinux-generic-intent", "1.0");
    metadata.scriptlets.push(Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "restorecon -R /usr/share/demo\n".to_string(),
        flags: None,
    });

    let mut effect = complete_effect("selinux-label-refresh", "restorecon");
    effect.adapter_id = Some("selinux-policy/v1".to_string());
    effect.args = vec!["-R".to_string(), "/usr/share/demo".to_string()];
    effect.path = Some("/usr/share/demo".to_string());
    effect.extra.insert(
        "selinux_operation".to_string(),
        toml::Value::String("label-refresh".to_string()),
    );
    effect.extra.insert("recursive".to_string(), toml::Value::Boolean(true));
    effect.extra.insert("payload_backed".to_string(), toml::Value::Boolean(true));
    effect.extra.insert(
        "paths".to_string(),
        toml::Value::Array(vec![toml::Value::String("/usr/share/demo".to_string())]),
    );

    let classification = known_report_with_effect(effect);
    let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();
    let entry = &build.bundle.entries[0];

    assert_eq!(entry.security_policy_intents.len(), 1);
    let intent = &entry.security_policy_intents[0];
    assert_eq!(intent.provider.as_str(), "selinux");
    assert_eq!(intent.operation, "label-refresh");
    assert_eq!(intent.scope.kind, "path");
    assert_eq!(intent.scope.paths, vec!["/usr/share/demo"]);
    assert_eq!(intent.payload_evidence.payload_backed, true);
    assert_eq!(intent.fallback.as_str(), "dormant");
    assert_eq!(intent.reconciliation.state.as_str(), "pending");

    let summary = ScriptletBundleSummary::from_bundle(&build.bundle, build.bundle.evidence_digest.clone());
    assert_eq!(summary.security_policy_intents.len(), 1);
}
```

Add this test to `crates/conary-core/src/ccs/convert/security_policy.rs`:

```rust
#[test]
fn non_lsm_effects_do_not_project_policy_intent() {
    use crate::ccs::legacy_scriptlets::{
        EffectConfidence, EffectReplacement, EffectSource, ScriptletEffect,
    };
    use std::collections::BTreeMap;

    let effect = ScriptletEffect {
        kind: "dynamic-linker-cache".to_string(),
        source: EffectSource::StaticSignal,
        confidence: EffectConfidence::Inferred,
        replacement: EffectReplacement::Complete,
        adapter_id: Some("ldconfig/v2".to_string()),
        adapter_digest: Some(crate::hash::sha256_prefixed(b"ldconfig/v2")),
        command: Some("ldconfig".to_string()),
        args: Vec::new(),
        path: None,
        reason_code: Some("helper-complete-ldconfig".to_string()),
        extra: BTreeMap::new(),
    };
    let intents = policy_intents_from_effects("scriptlet:0:post-install", &[effect]);

    assert!(intents.is_empty());
}
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core selinux_policy_effects_project_generic_policy_intent
cargo test -p conary-core non_lsm_effects_do_not_project_policy_intent
```

Expected: FAIL because the bridge module and entry projection do not exist.

- [ ] **Step 3: Implement the conversion bridge**

Add this module registration to `crates/conary-core/src/ccs/convert/mod.rs`:

```rust
mod security_policy;
```

Create `crates/conary-core/src/ccs/convert/security_policy.rs`:

```rust
// conary-core/src/ccs/convert/security_policy.rs

use crate::ccs::legacy_scriptlets::ScriptletEffect;
use crate::ccs::security_policy::{
    SECURITY_POLICY_INTENT_SCHEMA_V1, SecurityPolicyFallback, SecurityPolicyIntent,
    SecurityPolicyPayloadEvidence, SecurityPolicyProvider, SecurityPolicyReconciliation,
    SecurityPolicyReconciliationState, SecurityPolicyRequirements, SecurityPolicyScope,
    SecurityPolicySource,
};
use std::collections::BTreeMap;

pub(super) fn policy_intents_from_effects(
    entry_id: &str,
    effects: &[ScriptletEffect],
) -> Vec<SecurityPolicyIntent> {
    effects
        .iter()
        .filter_map(|effect| policy_intent_from_effect(entry_id, effect))
        .collect()
}

fn policy_intent_from_effect(entry_id: &str, effect: &ScriptletEffect) -> Option<SecurityPolicyIntent> {
    if effect.adapter_id.as_deref() != Some("selinux-policy/v1") {
        return None;
    }

    let operation = effect
        .extra
        .get("selinux_operation")
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| operation_from_kind(&effect.kind))
        .to_string();

    let scope = scope_from_selinux_effect(effect, &operation);
    let payload_paths = payload_paths(effect);
    let mut desired_state = BTreeMap::new();
    for key in [
        "action",
        "selinux_type",
        "expression",
        "payload_root",
        "boolean",
        "value",
        "persistent",
        "module_path",
        "module_format",
        "recursive",
    ] {
        if let Some(value) = effect.extra.get(key) {
            desired_state.insert(key.to_string(), value.clone());
        }
    }

    Some(SecurityPolicyIntent {
        schema: SECURITY_POLICY_INTENT_SCHEMA_V1.to_string(),
        id: format!("{entry_id}:{}", effect.kind),
        source: SecurityPolicySource {
            source_format: None,
            source_distro: None,
            entry_id: Some(entry_id.to_string()),
            command: effect.command.clone(),
            argv: effect.args.clone(),
            adapter_id: effect.adapter_id.clone(),
        },
        provider: SecurityPolicyProvider::Selinux,
        operation,
        scope,
        desired_state,
        requirements: SecurityPolicyRequirements {
            required_on_active_provider: false,
            provider_mode: None,
            tools: effect.command.iter().cloned().collect(),
            modules: Vec::new(),
        },
        fallback: SecurityPolicyFallback::Dormant,
        payload_evidence: SecurityPolicyPayloadEvidence {
            payload_backed: effect
                .extra
                .get("payload_backed")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false),
            paths: payload_paths,
            digest: effect.adapter_digest.clone(),
        },
        reconciliation: SecurityPolicyReconciliation {
            state: SecurityPolicyReconciliationState::Pending,
            reason: Some("metadata-only-provider-planning-deferred".to_string()),
            target_provider: None,
        },
        extra: BTreeMap::new(),
    })
}

fn operation_from_kind(kind: &str) -> &str {
    match kind {
        "selinux-label-refresh" => "label-refresh",
        "selinux-file-context" => "file-context",
        "selinux-boolean" => "boolean-set",
        "selinux-policy-module" => "module-install",
        _ => kind,
    }
}

fn scope_from_selinux_effect(effect: &ScriptletEffect, operation: &str) -> SecurityPolicyScope {
    match effect.kind.as_str() {
        "selinux-boolean" => SecurityPolicyScope {
            kind: "boolean".to_string(),
            name: effect
                .extra
                .get("boolean")
                .and_then(toml::Value::as_str)
                .map(ToOwned::to_owned),
            paths: Vec::new(),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
        "selinux-policy-module" => SecurityPolicyScope {
            kind: "policy-module".to_string(),
            name: effect.path.clone(),
            paths: effect.path.iter().cloned().collect(),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
        _ if operation.contains("file-context") => SecurityPolicyScope {
            kind: "file-context".to_string(),
            name: effect.path.clone(),
            paths: payload_paths(effect),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
        _ => SecurityPolicyScope {
            kind: "path".to_string(),
            name: effect.path.clone(),
            paths: payload_paths(effect),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
    }
}

fn payload_paths(effect: &ScriptletEffect) -> Vec<String> {
    if let Some(values) = effect.extra.get("paths").and_then(toml::Value::as_array) {
        return values
            .iter()
            .filter_map(toml::Value::as_str)
            .map(ToOwned::to_owned)
            .collect();
    }
    effect.path.iter().cloned().collect()
}
```

- [ ] **Step 4: Store bridged intents on entries and bundle**

In `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`, import:

```rust
use super::super::security_policy::policy_intents_from_effects;
```

In both `build_flat_entry` and `build_native_entry`, compute after `outcome`:

```rust
let security_policy_intents = policy_intents_from_effects(&id, &outcome.effects);
```

For native entries use `&native.id`:

```rust
let security_policy_intents = policy_intents_from_effects(&native.id, &outcome.effects);
```

Set:

```rust
security_policy_intents,
```

in each `LegacyScriptletEntry` initializer.

In `crates/conary-core/src/ccs/convert/scriptlet_bundle/builder.rs`, compute the bundle-level mirror after entries are built:

```rust
let security_policy_intents = entries
    .iter()
    .flat_map(|entry| entry.security_policy_intents.iter().cloned())
    .collect::<Vec<_>>();
```

and set `security_policy_intents` in the `LegacyScriptletBundle` initializer.

- [ ] **Step 5: Keep SELinux adapter evidence stable**

In `crates/conary-core/src/ccs/convert/selinux_adapters.rs`, preserve these existing extra keys for the bridge:

```rust
"selinux_operation"
"target_security_policy"
"host_policy_behavior"
"payload_backed"
"paths"
"payload_root"
"boolean"
"module_path"
```

Do not rename them in this task. Add a test assertion in `selinux_adapter_models_payload_backed_policy_and_label_intent_as_portable_effects` that each supported effect has `target_security_policy = "selinux-optional"` and `host_policy_behavior = "apply-when-selinux-present-dormant-when-absent"`.

- [ ] **Step 6: Run Task 2 tests**

Run:

```bash
cargo test -p conary-core selinux_policy_effects_project_generic_policy_intent
cargo test -p conary-core non_lsm_effects_do_not_project_policy_intent
cargo test -p conary-core ccs::convert::selinux_adapters
cargo test -p conary-core ccs::convert::scriptlet_bundle
```

Expected: PASS.

- [ ] **Step 7: Commit Task 2**

```bash
git add crates/conary-core/src/ccs/convert/security_policy.rs crates/conary-core/src/ccs/convert/mod.rs crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs crates/conary-core/src/ccs/convert/scriptlet_bundle/builder.rs crates/conary-core/src/ccs/convert/scriptlet_bundle/test_support.rs crates/conary-core/src/ccs/convert/selinux_adapters.rs
git commit -m "feat(ccs): bridge SELinux effects to generic policy intent"
```

## Task 3: Add AppArmor Typed Review Intent

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`
- Modify: `crates/conary-core/src/ccs/convert/golden_fixtures.rs`

**Interfaces:**
- Consumes: `SecurityPolicyIntent` from Task 1.
- Produces: review-only AppArmor intent for known AppArmor helper commands.
- Produces: no public-ready AppArmor adapter claim.

- [ ] **Step 1: Write failing AppArmor review-intent test**

Add this test to `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`:

```rust
#[test]
fn blocked_apparmor_command_projects_review_policy_intent() {
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletCommandEvidence};

    let classification = ScriptletClassification::Blocked {
        reason_code: "blocked-class-apparmor".to_string(),
        class_id: "apparmor".to_string(),
        command: Some(ScriptletCommandEvidence {
            command: "apparmor_parser".to_string(),
            argv: vec!["-r".to_string(), "/etc/apparmor.d/usr.bin.demo".to_string()],
            phase: Some("post-install".to_string()),
            lifecycle_paths: vec!["post-install".to_string()],
            raw_line: None,
            source: "static-signal".to_string(),
            environment: Vec::new(),
        }),
    };

    let intent = security_policy_intent_from_classification("scriptlet:0:post-install", &classification)
        .expect("apparmor intent");

    assert_eq!(intent.provider.as_str(), "apparmor");
    assert_eq!(intent.operation, "profile-reload");
    assert_eq!(intent.scope.kind, "profile");
    assert_eq!(intent.fallback.as_str(), "dormant");
    assert_eq!(intent.reconciliation.state.as_str(), "review");
    assert_eq!(intent.source.command.as_deref(), Some("apparmor_parser"));
}
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p conary-core blocked_apparmor_command_projects_review_policy_intent
```

Expected: FAIL because review-only AppArmor policy intent is not projected.

- [ ] **Step 3: Expand AppArmor helper inventory**

In `crates/conary-core/src/ccs/convert/blocked_classes.rs`, extend the AppArmor class command list:

```rust
&[
    "apparmor_parser",
    "aa-enforce",
    "aa-complain",
    "aa-disable",
    "aa-status",
]
```

Keep `default_outcome = Blocked`. This is typed review evidence, not replacement.

- [ ] **Step 4: Project review-only AppArmor intent**

In `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`, add an internal function that returns generic policy intent for LSM review classifications:

```rust
pub(super) fn security_policy_intent_from_classification(
    entry_id: &str,
    classification: &ScriptletClassification,
) -> Option<SecurityPolicyIntent> {
    let (reason_code, class_id, command) = match classification {
        ScriptletClassification::Blocked {
            reason_code,
            class_id,
            command: Some(command),
        }
        | ScriptletClassification::Review {
            reason_code,
            class_id: Some(class_id),
            command: Some(command),
        } => (reason_code, class_id, command),
        _ => return None,
    };

    if class_id != "apparmor" {
        return None;
    }

    Some(SecurityPolicyIntent {
        schema: SECURITY_POLICY_INTENT_SCHEMA_V1.to_string(),
        id: format!("{entry_id}:apparmor:{}", command.command),
        source: SecurityPolicySource {
            source_format: None,
            source_distro: None,
            entry_id: Some(entry_id.to_string()),
            command: Some(command.command.clone()),
            argv: command.argv.clone(),
            adapter_id: None,
        },
        provider: SecurityPolicyProvider::Apparmor,
        operation: apparmor_operation(&command.command).to_string(),
        scope: SecurityPolicyScope {
            kind: "profile".to_string(),
            name: command.argv.iter().find(|arg| !arg.starts_with('-')).cloned(),
            paths: command
                .argv
                .iter()
                .filter(|arg| arg.starts_with('/'))
                .cloned()
                .collect(),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
        desired_state: BTreeMap::new(),
        requirements: SecurityPolicyRequirements {
            required_on_active_provider: false,
            provider_mode: None,
            tools: vec![command.command.clone()],
            modules: Vec::new(),
        },
        fallback: SecurityPolicyFallback::Dormant,
        payload_evidence: SecurityPolicyPayloadEvidence::default(),
        reconciliation: SecurityPolicyReconciliation {
            state: SecurityPolicyReconciliationState::Review,
            reason: Some(reason_code.clone()),
            target_provider: None,
        },
        extra: BTreeMap::new(),
    })
}

fn apparmor_operation(command: &str) -> &'static str {
    match command {
        "apparmor_parser" => "profile-reload",
        "aa-enforce" => "mode-enforce",
        "aa-complain" => "mode-complain",
        "aa-disable" => "profile-disable",
        "aa-status" => "status-query",
        _ => "profile-operation",
    }
}
```

Add imports for `SecurityPolicyIntent` and related types from `crate::ccs::security_policy`, and expand the existing collection import to:

```rust
use std::collections::{BTreeMap, BTreeSet};
```

In `EntryOutcome`, add:

```rust
pub(super) security_policy_intents: Vec<SecurityPolicyIntent>,
```

In `classify_entry`, collect these from classifications and carry them into every returned `EntryOutcome`:

```rust
let security_policy_intents = classifications
    .iter()
    .filter_map(|entry| security_policy_intent_from_classification(&entry.entry_id, &entry.classification))
    .collect::<Vec<_>>();
```

In `entries.rs`, append classification-derived intents to effect-derived intents before setting the entry field:

```rust
let mut security_policy_intents = outcome.security_policy_intents.clone();
security_policy_intents.extend(policy_intents_from_effects(&id, &outcome.effects));
```

- [ ] **Step 5: Make support matrix outcome explicit**

In `crates/conary-core/src/ccs/convert/support_matrix.rs`, update the AppArmor lifecycle notes for `blocked-class-apparmor` through `fixture_names_for_class` and the blocked-class description already read from `blocked_classes.rs`. The AppArmor row remains `SupportOutcome::Blocked`; its note must say:

```text
AppArmor helper calls are captured as generic security-policy review intent, but no AppArmor command is public-ready until a payload-backed adapter proves profile install/reload/mode semantics.
```

- [ ] **Step 6: Run Task 3 tests**

Run:

```bash
cargo test -p conary-core blocked_apparmor_command_projects_review_policy_intent
cargo test -p conary-core blocked_classes_cover_kernel_install_selinux_module_and_label_tools
cargo test -p conary-core support_matrix
```

Expected: PASS.

- [ ] **Step 7: Commit Task 3**

```bash
git add crates/conary-core/src/ccs/convert/blocked_classes.rs crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs crates/conary-core/src/ccs/convert/support_matrix.rs crates/conary-core/src/ccs/convert/golden_fixtures.rs
git commit -m "feat(ccs): capture AppArmor helpers as policy review intent"
```

## Task 4: Project Generic LSM Intent Into Remi Evidence Queue

**Files:**
- Modify: `crates/conary-core/src/db/schema.rs`
- Modify: `crates/conary-core/src/db/migrations/v41_current.rs`
- Modify: `crates/conary-core/src/db/models/scriptlet_evidence.rs`
- Modify: `apps/remi/src/server/scriptlet_evidence_queue/aggregation.rs`
- Modify: `apps/remi/src/server/scriptlet_evidence_queue/normalization.rs`
- Modify: `apps/remi/src/server/scriptlet_evidence_queue/packet.rs`
- Modify: `apps/remi/src/server/scriptlet_evidence_queue/types.rs`

**Interfaces:**
- Consumes: `ScriptletBundleSummary.security_policy_intents` from Task 1.
- Produces: `scriptlet_evidence_cluster_samples.security_policy_intents_json`.
- Produces: sanitized packet field `evidence.samples[].security_policy_intents`.

- [ ] **Step 1: Write failing migration/model test**

Add this test to `crates/conary-core/src/db/migrations/v41_current.rs` near the v75 test:

```rust
#[test]
fn test_migrate_v76_adds_security_policy_intents_to_queue_samples() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::migrate(&conn).unwrap();

    let columns = table_columns(&conn, "scriptlet_evidence_cluster_samples");
    assert!(columns.contains(&"security_policy_intents_json".to_string()));

    let default_value: String = conn
        .query_row(
            "SELECT dflt_value FROM pragma_table_info('scriptlet_evidence_cluster_samples')
             WHERE name = 'security_policy_intents_json'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(default_value, "'[]'");
}
```

Add this assertion to `crates/conary-core/src/db/models/scriptlet_evidence.rs` in `scriptlet_evidence_sample_upsert_updates_existing_observation`:

```rust
let loaded = ScriptletEvidenceSample::list_for_cluster(&conn, "s1-test").unwrap();
assert_eq!(loaded[0].security_policy_intents_json, r#"[{"provider":"selinux"}]"#);
```

Update `new_sample` in the same test module to initialize:

```rust
security_policy_intents_json: r#"[{"provider":"selinux"}]"#.to_string(),
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core test_migrate_v76_adds_security_policy_intents_to_queue_samples
cargo test -p conary-core scriptlet_evidence_sample_upsert_updates_existing_observation
```

Expected: FAIL because schema v76 and model fields do not exist.

- [ ] **Step 3: Add migration v76**

In `crates/conary-core/src/db/schema.rs`, set:

```rust
pub const SCHEMA_VERSION: i32 = 76;
```

Add this match arm:

```rust
76 => migrations::migrate_v76(conn),
```

Update the schema-version assertion from `75` to `76`.

In `crates/conary-core/src/db/migrations/v41_current.rs`, add:

```rust
/// Version 76: Generic LSM policy intent evidence queue projection
pub fn migrate_v76(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        ALTER TABLE scriptlet_evidence_cluster_samples
            ADD COLUMN security_policy_intents_json TEXT NOT NULL DEFAULT '[]';
        ",
    )?;
    info!("Schema version 76 applied successfully (generic LSM policy intent queue projection)");
    Ok(())
}
```

- [ ] **Step 4: Persist the new sample field**

In `crates/conary-core/src/db/models/scriptlet_evidence.rs`, add this field to `NewScriptletEvidenceSample` and `ScriptletEvidenceSample`:

```rust
pub security_policy_intents_json: String,
```

Update `ScriptletEvidenceSample::COLUMNS` by inserting `security_policy_intents_json` immediately after `boot_security_intents_json`.

Update the insert SQL, update SQL, params, and `from_row` indexes. The target order is:

```text
reason_codes_json
blocked_classes_json
boot_security_intents_json
security_policy_intents_json
review_artifact_path
review_artifact_stale
evidence_digest
curation_evidence_digest
observed_at
```

- [ ] **Step 5: Add Remi aggregation and sanitization**

In `apps/remi/src/server/scriptlet_evidence_queue/aggregation.rs`, set:

```rust
security_policy_intents_json: serde_json::to_string(&sanitize_security_policy_intents(
    &summary.security_policy_intents,
))?,
```

In `apps/remi/src/server/scriptlet_evidence_queue/normalization.rs`, add:

```rust
use conary_core::ccs::security_policy::SecurityPolicyIntent;
```

and add:

```rust
pub fn sanitize_security_policy_intents(
    intents: &[SecurityPolicyIntent],
) -> Vec<SecurityPolicyIntent> {
    let mut value = serde_json::to_value(intents).unwrap_or_else(|_| Value::Array(Vec::new()));
    sanitize_security_policy_intents_value_inner(&mut value);
    serde_json::from_value(value).unwrap_or_default()
}

pub fn sanitize_security_policy_intents_value(value: &str) -> Value {
    let Ok(mut value) = serde_json::from_str::<Value>(value) else {
        return Value::Array(Vec::new());
    };
    sanitize_security_policy_intents_value_inner(&mut value);
    value
}

fn sanitize_security_policy_intents_value_inner(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for field in fields.values_mut() {
                sanitize_security_policy_intents_value_inner(field);
            }
        }
        Value::Array(values) => {
            for value in values {
                sanitize_security_policy_intents_value_inner(value);
            }
        }
        Value::String(value) => {
            if let Some(normalized) = normalize_token(value) {
                *value = normalized;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}
```

In `apps/remi/src/server/scriptlet_evidence_queue/packet.rs`, add `security_policy_intents` beside `boot_security_intents`:

```rust
"security_policy_intents": sanitize_security_policy_intents_value(&sample.security_policy_intents_json),
```

- [ ] **Step 6: Run Task 4 tests**

Run:

```bash
cargo test -p conary-core test_migrate_v76_adds_security_policy_intents_to_queue_samples
cargo test -p conary-core scriptlet_evidence_sample_upsert_updates_existing_observation
cargo test -p remi scriptlet_evidence_queue
cargo test -p remi publication
```

Expected: PASS.

- [ ] **Step 7: Commit Task 4**

```bash
git add crates/conary-core/src/db/schema.rs crates/conary-core/src/db/migrations/v41_current.rs crates/conary-core/src/db/models/scriptlet_evidence.rs apps/remi/src/server/scriptlet_evidence_queue/aggregation.rs apps/remi/src/server/scriptlet_evidence_queue/normalization.rs apps/remi/src/server/scriptlet_evidence_queue/packet.rs apps/remi/src/server/scriptlet_evidence_queue/types.rs
git commit -m "feat(remi): queue generic LSM policy intent evidence"
```

## Task 5: Prove Conversion And Public Serving Outcomes

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/golden_fixtures.rs`
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Modify: `apps/conary/tests/conversion_integration.rs`
- Modify: `apps/conary/tests/bundle_replay.rs`
- Modify: `apps/remi/src/server/publication.rs`

**Interfaces:**
- Consumes: generic intent metadata from Tasks 1 through 4.
- Produces: proof that SELinux supported forms are public-ready with generic intent metadata.
- Produces: proof that AppArmor typed review intent does not become public-ready.
- Produces: proof that local bundle replay refusal gates remain unchanged.

- [ ] **Step 1: Write failing conversion assertions**

In `crates/conary-core/src/ccs/convert/converter.rs`, extend the existing SELinux adapter test with:

```rust
let bundle = result
    .build_result
    .manifest
    .legacy_scriptlets
    .as_ref()
    .expect("legacy bundle");
assert_eq!(bundle.security_policy_intents.len(), 4);
assert!(
    bundle
        .security_policy_intents
        .iter()
        .any(|intent| intent.provider.as_str() == "selinux"
            && intent.operation == "label-refresh"
            && intent.fallback.as_str() == "dormant")
);
assert_eq!(result.scriptlet_metadata.security_policy_intents.len(), 4);
```

Add a new converter test:

```rust
#[test]
fn apparmor_helper_is_typed_review_intent_not_public_ready() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "apparmor_parser -r /etc/apparmor.d/usr.bin.demo\n".to_string(),
        flags: None,
    }];
    let files = vec![ExtractedFile {
        path: "/etc/apparmor.d/usr.bin.demo".to_string(),
        content: b"profile demo /usr/bin/demo { }\n".to_vec(),
        size: 31,
        mode: 0o644,
        sha256: Some(crate::hash::sha256_prefixed(b"profile demo /usr/bin/demo { }\n")),
        symlink_target: None,
    }];
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "deb", "sha256:test")
        .expect("conversion succeeds");
    let bundle = result.build_result.manifest.legacy_scriptlets.as_ref().unwrap();

    assert_eq!(bundle.publication_status.as_str(), "blocked");
    assert_eq!(bundle.entries[0].security_policy_intents.len(), 1);
    assert_eq!(bundle.entries[0].security_policy_intents[0].provider.as_str(), "apparmor");
    assert_eq!(
        bundle.entries[0].security_policy_intents[0].reconciliation.state.as_str(),
        "review"
    );
}
```

- [ ] **Step 2: Run the focused failing conversion tests**

Run:

```bash
cargo test -p conary-core selinux_adapter_models_payload_backed_policy_and_label_intent_as_portable_effects
cargo test -p conary-core apparmor_helper_is_typed_review_intent_not_public_ready
```

Expected: FAIL until the projection and AppArmor classification from earlier tasks are wired through converter output.

- [ ] **Step 3: Update fixture expectations**

In `crates/conary-core/src/ccs/convert/golden_fixtures.rs`, keep:

```rust
public_fixture(
    "adapter-selinux-policy",
    GoldenFixtureOutcome::FullyReplaced,
    "fedora-44",
    "arch",
),
```

and keep:

```rust
fixture("blocked-class-apparmor", GoldenFixtureOutcome::Blocked),
```

Do not add an AppArmor public fixture in this plan.

- [ ] **Step 4: Prove public gate behavior**

In `apps/remi/src/server/publication.rs`, extend `PublicationGateScriptlets` with:

```rust
pub security_policy_intents: Vec<conary_core::ccs::security_policy::SecurityPolicyIntent>,
```

Populate it from `ScriptletBundleSummary.security_policy_intents`, then extend an existing blocked/publication report test so a blocked AppArmor sample includes `security_policy_intents` in private report data but still returns a blocked/private status. The assertion must check both facts:

```rust
assert_eq!(report.status.as_str(), "blocked");
assert!(
    report
        .scriptlets
        .security_policy_intents
        .iter()
        .any(|intent| intent.provider.as_str() == "apparmor")
);
```

- [ ] **Step 5: Run interaction proof**

Run:

```bash
cargo test -p conary-core golden_fixtures
cargo test -p conary-core support_matrix
cargo test -p conary --test conversion_integration golden_conversion
cargo test -p conary --test bundle_replay
cargo test -p remi publication
```

Expected: PASS.

- [ ] **Step 6: Commit Task 5**

```bash
git add crates/conary-core/src/ccs/convert/golden_fixtures.rs crates/conary-core/src/ccs/convert/support_matrix.rs crates/conary-core/src/ccs/convert/converter.rs apps/conary/tests/conversion_integration.rs apps/conary/tests/bundle_replay.rs apps/remi/src/server/publication.rs
git commit -m "test: prove generic LSM conversion metadata outcomes"
```

## Task 6: Update Docs And Final Verification

**Files:**
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

**Interfaces:**
- Consumes: completed code behavior from Tasks 1 through 5.
- Produces: current contributor-facing docs and audit registration.

- [ ] **Step 1: Update CCS docs**

In `docs/modules/ccs.md`, update the `convert/` section so it says:

```markdown
Supported SELinux scriptlet forms are modeled as `selinux-policy/v1` effects and bridged into generic `SecurityPolicyIntent` metadata. The generic intent records provider, operation, scope, fallback, payload evidence, and reconciliation state while preserving the original provider-specific effect evidence for older tooling. AppArmor helper calls are captured as review-only generic policy intent until a payload-backed AppArmor adapter proves profile install, reload, and mode semantics.
```

- [ ] **Step 2: Update Remi docs**

In `docs/modules/remi.md`, update `Scriptlet Evidence Queue` so it says:

```markdown
Queue samples include sanitized generic LSM `security_policy_intents` when conversion can type SELinux or AppArmor helper behavior. The queue uses those records for adapter planning and public/private packet export; the queue still is not publication authority, and moving a cluster state never makes a package public.
```

- [ ] **Step 3: Register docs audit state**

Append or refresh the ledger row for this plan:

```text
docs/superpowers/plans/archive/2026-07-07-generic-lsm-security-policy-intent-plan.md	docs/superpowers/plans/archive/2026-07-07-generic-lsm-security-policy-intent-plan.md	planning	maintainer	lsm-security-policy; selinux; apparmor; implementation-plan; scriptlet-evidence-queue	docs/superpowers/specs/archive/2026-07-07-generic-lsm-security-policy-intent-design.md; docs/superpowers/specs/archive/2026-07-03-remi-scriptlet-evidence-queue-design.md; docs/modules/ccs.md; docs/modules/remi.md; crates/conary-core/src/ccs/legacy_scriptlets.rs; crates/conary-core/src/ccs/convert/selinux_adapters.rs; apps/remi/src/server/scriptlet_evidence_queue/	verified	verified-no-change	Implementation plan for the first additive generic LSM policy intent slice, covering legacy conversion metadata, SELinux bridging, AppArmor review intent, Remi queue projection, public/private outcomes, and docs verification.
```

Regenerate inventory:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 4: Run final verification**

Run:

```bash
cargo fmt --check
cargo test -p conary-core ccs::convert
cargo test -p conary-core golden_fixtures
cargo test -p conary-core support_matrix
cargo test -p conary-core scriptlet_evidence
cargo test -p conary --test conversion_integration golden_conversion
cargo test -p conary --test bundle_replay
cargo test -p remi publication
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh --check
git diff --check
```

Expected: all commands PASS.

- [ ] **Step 5: Commit Task 6**

```bash
git add docs/modules/ccs.md docs/modules/remi.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: document generic LSM policy intent metadata"
```

## Plan Self-Review Checklist

- Spec coverage:
  - Goal and principles: Tasks 1 through 5 model LSM intent as data and avoid helper replay.
  - Initial storage and compatibility: Task 1 adds serde-defaulted fields to existing legacy metadata.
  - SELinux mapping: Task 2 bridges `selinux-policy/v1` effects.
  - AppArmor mapping: Task 3 captures helper calls as typed review intent without public-ready claims.
  - Public serving: Task 5 preserves SELinux public-ready proof and AppArmor private/blocked behavior.
  - Review queue/data collection: Task 4 projects sanitized intent into queue samples and packets.
  - Testing strategy: Tasks 1 through 5 add unit, conversion, queue, and Remi publication proof.
- Placeholder scan:
  - No banned placeholder tokens or generic test-only instructions are present.
- Type consistency:
  - `SecurityPolicyIntent`, `security_policy_intents`, and `security_policy_intents_json` names are consistent across tasks.
  - Reconciliation state values use `pending` for metadata-only SELinux intent and `review` for AppArmor typed review intent.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/archive/2026-07-07-generic-lsm-security-policy-intent-plan.md`. Two execution options:

1. Subagent-Driven (recommended) - dispatch a fresh subagent per task, review between tasks, fast iteration.
2. Inline Execution - execute tasks in this session using `superpowers:executing-plans`, batch execution with checkpoints.
