# Sysctl Target-Profile Public Policy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep `sysctl/v1` native-hook replacement evidence intact while making public-ready sysctl conversion require the route-derived target profile to allow the exact key.

**Architecture:** Add target-profile context to passive scriptlet bundle construction, persist the profile id in bundle metadata, and extend `ccs::convert::public_policy` so complete `sysctl/v1` effects are public only when `TargetProfileQuery::sysctl_status(key)` returns `Accepted`. Remi conversion resolves the route slug to the current public target profile before converting, and summary recomputation treats missing or unsupported profile metadata as private-review so stale rows fail closed. Bump converted-row policy version again because this slice makes previously public-looking sysctl adapter rows private-review.

**Tech Stack:** Rust, existing CCS supported-profile catalog, TOML/JSON passive scriptlet bundle metadata, rusqlite converted-row model tests, Remi conversion tests, cargo test, docs-audit checks.

## Global Constraints

- `sysctl/v1` continues to classify exactly one validated `sysctl -w <key>=<value>` or `sysctl --write <key>=<value>` as complete adapter replacement evidence.
- Existing sysctl key and value syntax validation plus denied-key rejection remain the defensive floor.
- Public-ready sysctl conversion requires complete adapter replacement evidence and target-profile approval for the exact key through `TargetProfileQuery::sysctl_status()`.
- Defaults answer unsupported. Missing target-profile context, unknown target-profile ids, unsupported keys, malformed persisted metadata, and stale converted rows must not be public-ready.
- Complete but target-unsupported sysctl evidence still projects into native `hooks.sysctl` and remains available to local/private-review/admin-test workflows.
- The current supported-profile catalog allowlists only `kernel.example` for public sysctl lifecycle policy; `net.ipv4.ip_forward` is valid adapter evidence but private-review until catalog policy changes.
- Remi conversion must derive the exact target profile from the public route slug, not from package metadata or an unversioned distro family string.
- Bump `CONVERSION_VERSION` from `5` to `6` and ensure stale converted rows are never public-ready.
- Do not add broad sysctl namespaces, value-range policy, runtime sysctl application changes, or new Remi public-serving routes in this slice.
- Keep docs and fixture metadata aligned with `docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md`.

---

## File Structure

- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs`
  - Add optional target-profile context to `ScriptletBundleInput`.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/builder.rs`
  - Resolve the optional target profile before aggregate status construction.
  - Persist `public_policy_target_profile_id` in `LegacyScriptletBundle.extra`.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs`
  - Include the persisted target profile id in the scriptlet evidence digest.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`
  - Recompute public-policy review reasons with persisted target-profile context.
  - Treat missing or invalid target-profile context as unsupported for `sysctl/v1`.
- Modify `crates/conary-core/src/ccs/convert/public_policy.rs`
  - Add sysctl public-policy helpers while keeping file-capability policy unchanged.
- Modify `crates/conary-core/src/ccs/convert/converter.rs`
  - Add `LegacyConverter::with_target_profile_id`.
  - Pass the optional target profile id into `ScriptletBundleInput`.
  - Add conversion integration tests for profile-accepted and profile-unsupported sysctl keys.
- Modify `apps/remi/src/server/conversion/workflow.rs`
  - Resolve the public profile for the Remi route slug and call `with_target_profile_id(profile.id())`.
- Modify `crates/conary-core/src/ccs/convert/golden_fixtures.rs`
  - Make the public sysctl fixture use `kernel.example`.
  - Add a private-review sysctl fixture for `net.ipv4.ip_forward`.
- Modify `crates/conary-core/src/ccs/convert/adapters.rs`
  - Align the built-in `adapter-sysctl` golden adapter command with `kernel.example`.
  - Add the private-review sysctl adapter fixture id for `net.ipv4.ip_forward`.
- Modify `crates/conary-core/src/ccs/convert/support_matrix.rs`
  - Register both public-ready and private-review sysctl fixture evidence.
- Modify `crates/conary-core/src/db/models/converted.rs`
  - Bump `CONVERSION_VERSION` to `6`.
  - Keep stale-row public readiness tests explicit.
- Modify `docs/SCRIPTLET_SECURITY.md`
  - Document target-profile sysctl public policy.
- Modify `docs/modules/ccs.md`
  - Clarify sysctl adapter replacement versus target-profile public policy.
- Modify `docs/modules/remi.md`
  - Document route-derived sysctl policy for Remi conversion/publication.
- Modify `docs/modules/test-fixtures.md`
  - Register the sysctl public/private fixture split.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Register this implementation plan and claim updates.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Regenerate after staging this new plan.

## Task 1: Thread Target Profile Context into Scriptlet Bundles

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/builder.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/test_support.rs`

**Interfaces:**
- Produces: `ScriptletBundleInput::target_profile_id: Option<&str>`
- Produces persisted bundle extra key: `public_policy_target_profile_id`
- Produces digest field: `public_policy_target_profile_id`

- [ ] **Step 1: Write failing builder and digest tests**

Add tests in `crates/conary-core/src/ccs/convert/scriptlet_bundle/builder.rs` and `digest.rs` that build the same sysctl-classified metadata with `target_profile_id = Some("fedora-44")` versus `Some("ubuntu-26.04")` and assert:

```rust
assert_eq!(
    build.bundle.extra.get("public_policy_target_profile_id").and_then(toml::Value::as_str),
    Some("fedora-44")
);
assert_ne!(fedora_digest, ubuntu_digest);
```

Also add a zero-target test that asserts `bundle.extra` does not contain `public_policy_target_profile_id` when `target_profile_id` is `None`.

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core scriptlet_bundle --lib
```

Expected: FAIL because `ScriptletBundleInput` has no target-profile field and the bundle extra/digest field do not exist.

- [ ] **Step 3: Add target profile input and persisted metadata**

Add this field to `ScriptletBundleInput<'a>`:

```rust
pub target_profile_id: Option<&'a str>,
```

In `build_legacy_scriptlet_bundle`, trim the profile id, keep only nonempty values, insert it into `bundle.extra` before calling `evidence_digest`, and pass that same value into later aggregate-status work in Task 3:

```rust
let target_profile_id = input
    .target_profile_id
    .map(str::trim)
    .filter(|value| !value.is_empty());

if let Some(profile_id) = target_profile_id {
    bundle.extra.insert(
        "public_policy_target_profile_id".to_string(),
        toml::Value::String(profile_id.to_string()),
    );
}
```

Update every `ScriptletBundleInput { ... }` literal and the `bundle_for_metadata` helper to set `target_profile_id: None` unless a test explicitly needs `Some("fedora-44")`.

- [ ] **Step 4: Include target profile id in the evidence digest**

Add this field to the `digest_doc` JSON in `crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs`:

```rust
"public_policy_target_profile_id": bundle
    .extra
    .get("public_policy_target_profile_id")
    .and_then(toml::Value::as_str),
```

- [ ] **Step 5: Verify and commit**

Run:

```bash
cargo test -p conary-core scriptlet_bundle --lib
cargo fmt --check
git diff --check
```

Expected: PASS.

Commit:

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle
git commit -m "security: record scriptlet public target profile"
```

## Task 2: Add Sysctl Public Policy Helper

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/public_policy.rs`

**Interfaces:**
- Produces: `SYSCTL_PUBLIC_REVIEW_REASON: &str = "public-policy-sysctl-target-profile-unsupported"`
- Produces: `sysctl_public_review_reason(key: &str, profile: Option<&dyn TargetProfileQuery>) -> Option<&'static str>`
- Changes: `entry_public_policy_review_reasons(entry: &LegacyScriptletEntry, profile: Option<&dyn TargetProfileQuery>) -> Vec<String>`

- [ ] **Step 1: Write failing public-policy tests**

Add tests in `public_policy.rs`:

```rust
use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

struct SysctlProfile {
    accepted: &'static str,
}

impl TargetProfileQuery for SysctlProfile {
    fn sysctl_status(&self, key: &str) -> ProfileConstraintStatus {
        if key == self.accepted {
            ProfileConstraintStatus::Accepted
        } else {
            ProfileConstraintStatus::Unsupported
        }
    }

    fn service_status(&self, _service: &str) -> ProfileConstraintStatus { ProfileConstraintStatus::Unsupported }
    fn tmpfiles_status(&self, _entry: &str) -> ProfileConstraintStatus { ProfileConstraintStatus::Unsupported }
    fn user_status(&self, _user: &str) -> ProfileConstraintStatus { ProfileConstraintStatus::Unsupported }
    fn group_status(&self, _group: &str) -> ProfileConstraintStatus { ProfileConstraintStatus::Unsupported }
    fn directory_status(&self, _directory: &str) -> ProfileConstraintStatus { ProfileConstraintStatus::Unsupported }
    fn alternative_status(&self, _alternative: &str) -> ProfileConstraintStatus { ProfileConstraintStatus::Unsupported }
}
```

Assert:

```rust
let profile = SysctlProfile { accepted: "kernel.example" };
assert_eq!(sysctl_public_review_reason("kernel.example", Some(&profile)), None);
assert_eq!(
    sysctl_public_review_reason("net.ipv4.ip_forward", Some(&profile)),
    Some(SYSCTL_PUBLIC_REVIEW_REASON)
);
assert_eq!(
    sysctl_public_review_reason("kernel.example", None),
    Some(SYSCTL_PUBLIC_REVIEW_REASON)
);
```

- [ ] **Step 2: Run the focused failing test**

Run:

```bash
cargo test -p conary-core public_policy --lib
```

Expected: FAIL because the sysctl helper and target-aware entry helper do not exist.

- [ ] **Step 3: Implement target-aware sysctl policy**

Add imports and helpers:

```rust
use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

pub(crate) const SYSCTL_PUBLIC_REVIEW_REASON: &str =
    "public-policy-sysctl-target-profile-unsupported";

pub(crate) fn sysctl_public_review_reason(
    key: &str,
    profile: Option<&dyn TargetProfileQuery>,
) -> Option<&'static str> {
    match profile.map(|profile| profile.sysctl_status(key.trim())) {
        Some(ProfileConstraintStatus::Accepted) => None,
        Some(ProfileConstraintStatus::Unsupported) | None => Some(SYSCTL_PUBLIC_REVIEW_REASON),
    }
}
```

Change `entry_public_policy_review_reasons` to accept the optional profile. Preserve existing file-capability behavior, then scan complete sysctl effects:

```rust
// kind "sysctl-setting" is defined by SysctlAdapter::classify in
// crates/conary-core/src/ccs/convert/adapters.rs.
if effect.adapter_id.as_deref() == Some("sysctl/v1") && effect.kind == "sysctl-setting" {
    let key = effect
        .extra
        .get("key")
        .and_then(toml::Value::as_str)
        .or(effect.path.as_deref());
    let review_reason = key.map_or(Some(SYSCTL_PUBLIC_REVIEW_REASON), |key| {
        sysctl_public_review_reason(key, profile)
    });
    match review_reason {
        Some(reason) => {
            reasons.insert(reason.to_string());
        }
        None => {}
    }
}
```

- [ ] **Step 4: Update callers temporarily with `None`**

Update current callers in `summary.rs` to pass `None`. Task 3 will replace that with resolved target-profile context. The current direct function-pointer call must become a closure because the helper now takes two arguments:

```rust
.flat_map(|entry| public_policy::entry_public_policy_review_reasons(entry, None))
```

- [ ] **Step 5: Verify and commit**

Run:

```bash
cargo test -p conary-core public_policy --lib
cargo fmt --check
git diff --check
```

Expected: PASS.

Commit:

```bash
git add crates/conary-core/src/ccs/convert/public_policy.rs crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs
git commit -m "security: require target policy for sysctl public status"
```

## Task 3: Apply Target-Aware Policy in Bundle Aggregate and Summary Recompute

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/builder.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/test_support.rs`

**Interfaces:**
- Produces: `aggregate_status(entries: &[LegacyScriptletEntry], counts: &DecisionCounts, profile: Option<&dyn TargetProfileQuery>)`
- Produces: target-profile lookup from `public_policy_target_profile_id`

- [ ] **Step 1: Write failing summary tests**

In `summary.rs`, add tests that create `LegacyScriptletEntry` values with a `sysctl/v1` `sysctl-setting` effect for `kernel.example` and `net.ipv4.ip_forward`.

Assert these cases:

```rust
let fedora = crate::repository::supported_profiles::profile_by_public_id("fedora-44")
    .expect("fedora profile");

let public = aggregate_status(
    &[kernel_example_entry],
    &counts,
    Some(fedora as &dyn crate::ccs::v2::validation::TargetProfileQuery),
);
assert_eq!(public.3.as_str(), "public");

let private = aggregate_status(
    &[ip_forward_entry],
    &counts,
    Some(fedora as &dyn crate::ccs::v2::validation::TargetProfileQuery),
);
assert_eq!(private.3.as_str(), "private-review");

let missing = aggregate_status(&[kernel_example_entry], &counts, None);
assert_eq!(missing.3.as_str(), "private-review");
```

Also add a recompute test that starts with a bundle whose persisted columns are public but whose `extra` is missing `public_policy_target_profile_id`; `ScriptletBundleSummary::from_bundle` must return `publication_status == "private-review"` and include `public-policy-sysctl-target-profile-unsupported`.

Add the same recompute assertion for a bundle whose `extra` contains an unknown profile id:

```rust
bundle.extra.insert(
    "public_policy_target_profile_id".to_string(),
    toml::Value::String("unknown-distro".to_string()),
);
let summary = ScriptletBundleSummary::from_bundle(&bundle, bundle.evidence_digest.clone());
assert_eq!(summary.publication_status, "private-review");
assert!(summary
    .review_reason_codes
    .contains(&"public-policy-sysctl-target-profile-unsupported".to_string()));
```

Add a builder-path test with `target_profile_id: None` and a `sysctl/v1` effect for `kernel.example`; it must build a bundle with `publication_status == "private-review"`. This proves the no-profile default through the real builder path, not only through direct `aggregate_status`.

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core scriptlet_bundle::summary --lib
```

Expected: FAIL because aggregate and summary recompute do not resolve target profiles yet.

- [ ] **Step 3: Resolve persisted target profile for summary recompute**

In `summary.rs`, add:

```rust
fn public_policy_target_profile(
    bundle: &LegacyScriptletBundle,
) -> Option<&'static crate::repository::supported_profiles::SupportedProfile> {
    bundle
        .extra
        .get("public_policy_target_profile_id")
        .and_then(toml::Value::as_str)
        .and_then(crate::repository::supported_profiles::profile_by_public_id)
}
```

Use that profile in `summary_from_bundle`, `public_policy_review_reason_codes`, and `aggregate_status`. Coerce the resolved `Option<&SupportedProfile>` into the trait-object type explicitly:

```rust
let target_profile = public_policy_target_profile(bundle);
let target_profile_query = target_profile.map(|profile| {
    profile as &dyn crate::ccs::v2::validation::TargetProfileQuery
});
```

Pass `target_profile_query` to `aggregate_status` and to the closure used by `public_policy_review_reason_codes`.

- [ ] **Step 4: Resolve input target profile for build-time aggregate status**

In `builder.rs`, before aggregate status construction, resolve:

```rust
let target_profile = target_profile_id
    .and_then(crate::repository::supported_profiles::profile_by_public_id);
```

Call the new aggregate signature:

```rust
aggregate_status(
    &entries,
    &decision_counts,
    target_profile.map(|profile| {
        profile as &dyn crate::ccs::v2::validation::TargetProfileQuery
    }),
)
```

Do not silently create a public profile for unknown ids; unknown ids must behave like `None`.

- [ ] **Step 5: Verify and commit**

Run:

```bash
cargo test -p conary-core scriptlet_bundle --lib
cargo fmt --check
git diff --check
```

Expected: PASS.

Commit:

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle crates/conary-core/src/ccs/convert/public_policy.rs
git commit -m "security: apply sysctl target policy to scriptlet bundles"
```

## Task 4: Wire Converter and Remi Route Profiles

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Modify: `apps/remi/src/server/conversion/workflow.rs`

**Interfaces:**
- Produces: `LegacyConverter::with_target_profile_id(self, profile_id: impl Into<String>) -> Self`
- Produces: Remi conversion call chain `.with_target_profile_id(profile.id())`

- [ ] **Step 1: Write failing converter tests**

Add or split converter tests near `conversion_integration_projects_safe_sysctl_write_into_manifest_hook`:

```rust
fn convert_scriptlet_body(converter: &LegacyConverter, content: &str) -> ConversionResult {
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: content.to_string(),
        flags: None,
    }];
    converter
        .convert(&metadata, &make_test_files(), "rpm", "sha256:test")
        .expect("conversion succeeds")
}

#[test]
fn conversion_public_ready_for_profile_allowed_sysctl_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path())
        .with_target_profile_id("fedora-44");
    let result = convert_scriptlet_body(&converter, "sysctl -w kernel.example=1\n");
    let sysctl_hooks = &result.build_result.manifest.hooks.sysctl;
    assert_eq!(sysctl_hooks.len(), 1);
    assert_eq!(sysctl_hooks[0].key, "kernel.example");
    let bundle = result.legacy_scriptlets.as_ref().expect("scriptlet bundle");
    assert_eq!(bundle.publication_status.as_str(), "public");
    assert_eq!(result.scriptlet_metadata.publication_status, "public");
}

#[test]
fn conversion_keeps_unsupported_sysctl_projected_but_private_review() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path())
        .with_target_profile_id("fedora-44");
    let result = convert_scriptlet_body(&converter, "sysctl -w net.ipv4.ip_forward=1\n");
    let sysctl_hooks = &result.build_result.manifest.hooks.sysctl;
    assert_eq!(sysctl_hooks.len(), 1);
    assert_eq!(sysctl_hooks[0].key, "net.ipv4.ip_forward");
    let bundle = result.legacy_scriptlets.as_ref().expect("scriptlet bundle");
    assert_eq!(bundle.decision_counts.replaced, 1);
    assert_eq!(bundle.publication_status.as_str(), "private-review");
    assert!(result
        .scriptlet_metadata
        .review_reason_codes
        .contains(&"public-policy-sysctl-target-profile-unsupported".to_string()));
}
```

This helper follows the existing setup pattern from `conversion_integration_projects_safe_sysctl_write_into_manifest_hook`, which already uses `passive_test_converter(temp_dir.path())`, `make_test_metadata()`, and `make_test_files()`.

- [ ] **Step 2: Write failing Remi route-profile test**

Add a unit test in `workflow.rs` that resolves `fedora` through the conversion workflow helper and asserts the profile id is `fedora-44`. If the helper is private, test it in the same module:

```rust
#[test]
fn conversion_route_resolves_public_target_profile() {
    let profile = ConversionService::public_target_profile_for_route("fedora")
        .expect("fedora route profile");
    assert_eq!(profile.id(), "fedora-44");
}
```

- [ ] **Step 3: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core conversion_integration_projects_safe_sysctl_write_into_manifest_hook --lib
cargo test -p conary-core conversion_keeps_unsupported_sysctl_projected_but_private_review --lib
cargo test -p remi conversion_route_resolves_public_target_profile --lib
```

Expected: FAIL because the converter and Remi workflow do not pass target-profile ids.

- [ ] **Step 4: Implement converter target profile plumbing**

Add a field to `LegacyConverter`:

```rust
target_profile_id: Option<String>,
```

Initialize it to `None`, add:

```rust
pub fn with_target_profile_id(mut self, profile_id: impl Into<String>) -> Self {
    self.target_profile_id = Some(profile_id.into());
    self
}
```

Pass it to bundle construction:

```rust
target_profile_id: self.target_profile_id.as_deref(),
```

Update the existing `conversion_integration_projects_safe_sysctl_write_into_manifest_hook` test at the same time. Because no-profile sysctl now fails closed, either:

- change that existing test to use `kernel.example` plus `.with_target_profile_id("fedora-44")` and keep the `public` assertion; or
- keep `net.ipv4.ip_forward` without a target profile and change the assertion to `private-review`.

Prefer the first option because the new unsupported-key test already covers private-review projection.

- [ ] **Step 5: Implement Remi route-derived profile resolution**

Add this associated helper inside `impl ConversionService` in `workflow.rs`:

```rust
fn public_target_profile_for_route(
    distro: &str,
) -> anyhow::Result<&'static conary_core::repository::supported_profiles::SupportedProfile> {
    let route = conary_core::repository::supported_profiles::route_by_slug(distro)
        .ok_or_else(|| anyhow!("unsupported release distro {distro}"))?;
    let profile_id = route
        .public_profile_ids()
        .first()
        .ok_or_else(|| anyhow!("release distro {distro} has no public target profile"))?;
    conary_core::repository::supported_profiles::profile_by_public_id(profile_id)
        .ok_or_else(|| anyhow!("release distro {distro} maps to missing public target profile {profile_id}"))
}
```

Call this before constructing the converter and chain:

```rust
let target_profile = Self::public_target_profile_for_route(distro)?;
let converter = LegacyConverter::new(options)
    .with_source_distro(distro)
    .with_target_profile_id(target_profile.id())
    .with_conversion_tool("remi");
```

- [ ] **Step 6: Verify and commit**

Run:

```bash
cargo test -p conary-core sysctl --lib
cargo test -p remi conversion_route_resolves_public_target_profile --lib
cargo fmt --check
git diff --check
```

Expected: PASS.

Commit:

```bash
git add crates/conary-core/src/ccs/convert/converter.rs apps/remi/src/server/conversion/workflow.rs
git commit -m "security: route sysctl conversion through target profiles"
```

## Task 5: Update Fixtures, Support Matrix, and Conversion Version

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/adapters.rs`
- Modify: `crates/conary-core/src/ccs/convert/golden_fixtures.rs`
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`
- Modify: `crates/conary-core/src/db/models/converted.rs`

**Interfaces:**
- Changes: `CONVERSION_VERSION: i32 = 6`
- Adds public fixture: `adapter-sysctl` with `kernel.example`
- Adds private-review fixture: `adapter-sysctl-target-profile-private-review`

- [ ] **Step 1: Write failing fixture/version tests**

Adjust existing fixture tests so `adapter-sysctl` uses `kernel.example` and remains `FullyReplaced`.

Add a new golden fixture for:

```sh
sysctl -w net.ipv4.ip_forward=1
```

with expected outcome `ReviewRequired`, and assert its review reason includes `public-policy-sysctl-target-profile-unsupported`.

Add a support-matrix regression test following the existing file-capability fixture-split test:

```rust
#[test]
fn sysctl_support_matrix_distinguishes_public_and_private_review_fixtures() {
    let matrix = SupportMatrix::default();
    let row = matrix
        .entries()
        .iter()
        .find(|entry| entry.adapter_id == Some("sysctl/v1"))
        .expect("sysctl adapter row exists");

    assert_eq!(
        row.fixture_names,
        &["adapter-sysctl", "adapter-sysctl-target-profile-private-review"]
    );

    let fixtures: std::collections::BTreeMap<_, _> = golden_fixtures::all_cases()
        .iter()
        .map(|case| (case.id, case.expected_outcome))
        .collect();
    assert_eq!(
        fixtures.get("adapter-sysctl"),
        Some(&golden_fixtures::GoldenFixtureOutcome::FullyReplaced)
    );
    assert_eq!(
        fixtures.get("adapter-sysctl-target-profile-private-review"),
        Some(&golden_fixtures::GoldenFixtureOutcome::ReviewRequired)
    );
}
```

In `converted.rs`, update the stale-row test to assert:

```rust
assert_eq!(CONVERSION_VERSION, 6);
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core golden_fixtures --lib
cargo test -p conary-core support_matrix --lib
cargo test -p conary-core adapter_registry_golden_helpers_are_fully_replaced_with_adapter_evidence --lib
cargo test -p conary-core stale_converted_rows_are_not_scriptlet_public_ready --lib
```

Expected: FAIL because fixtures, support matrix rows, and conversion version have not been updated.

- [ ] **Step 3: Update golden fixtures and support matrix**

In `adapters.rs`, change the built-in sysctl adapter fixture argv from:

```rust
&["-w", "net.ipv4.ip_forward=1"]
```

to:

```rust
&["-w", "kernel.example=1"]
```

Add `adapter-sysctl-target-profile-private-review` with `GoldenFixtureOutcome::ReviewRequired`.

In `adapters.rs`, add a new `GoldenAdapterCase` for `adapter-sysctl-target-profile-private-review` with:

```rust
command: "sysctl",
argv: &["-w", "net.ipv4.ip_forward=1"],
adapter_id: "sysctl/v1",
reason_code: "helper-complete-sysctl",
```

This adapter-level case proves the unsupported key remains complete replacement evidence. The public/private publication behavior is proven by the converter integration tests from Task 4.

Update the `sysctl/v1` support-matrix row description exactly to:

```rust
"Narrow sysctl writes are complete when the key and value validate and conversion projects the effect into native hooks.sysctl; public-ready status also requires target-profile policy to accept the exact key."
```

Set the row fixture ids to:

```rust
&["adapter-sysctl", "adapter-sysctl-target-profile-private-review"]
```

- [ ] **Step 4: Bump conversion version**

In `crates/conary-core/src/db/models/converted.rs`, change:

```rust
pub const CONVERSION_VERSION: i32 = 6;
```

Keep `is_scriptlet_public_ready()` failing closed for `self.conversion_version < CONVERSION_VERSION`.

- [ ] **Step 5: Verify and commit**

Run:

```bash
cargo test -p conary-core golden_fixtures --lib
cargo test -p conary-core support_matrix --lib
cargo test -p conary-core adapter_registry_golden_helpers_are_fully_replaced_with_adapter_evidence --lib
cargo test -p conary-core stale_converted_rows_are_not_scriptlet_public_ready --lib
cargo fmt --check
git diff --check
```

Expected: PASS.

Commit:

```bash
git add crates/conary-core/src/ccs/convert/adapters.rs crates/conary-core/src/ccs/convert/golden_fixtures.rs crates/conary-core/src/ccs/convert/support_matrix.rs crates/conary-core/src/db/models/converted.rs
git commit -m "security: version sysctl target-profile policy"
```

## Task 6: Update Docs and Audit Metadata

**Files:**
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

**Interfaces:**
- Produces doc truth for the `sysctl/v1` target-profile public policy split.

- [ ] **Step 1: Update security and module docs**

Document these exact claims:

- `sysctl/v1` still recognizes one validated `sysctl -w key=value` as complete native replacement evidence.
- Public Remi status additionally requires the target profile to allow the exact sysctl key.
- Missing profile context or unsupported keys are private-review, not public.
- The current catalog public proof key is `kernel.example`; `net.ipv4.ip_forward` remains valid private-review evidence unless the target profile later allows it.

- [ ] **Step 2: Register this plan and touched claim surfaces in the docs audit ledger**

Add a row for `docs/superpowers/plans/2026-07-08-sysctl-target-profile-public-policy-plan.md` with claim clusters:

```text
scriptlet-security; sysctl; remi-publication-gate; target-profiles; implementation-plan
```

Use evidence sources including:

```text
docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md; docs/SCRIPTLET_SECURITY.md; docs/modules/ccs.md; docs/modules/remi.md; docs/modules/test-fixtures.md; crates/conary-core/src/ccs/convert/public_policy.rs; crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs; crates/conary-core/src/ccs/convert/converter.rs; crates/conary-core/src/repository/supported_profiles/catalog.toml; apps/remi/src/server/conversion/workflow.rs
```

- [ ] **Step 3: Stage and regenerate inventory**

Run:

```bash
git add docs/superpowers/plans/2026-07-08-sysctl-target-profile-public-policy-plan.md docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 4: Verify docs and commit**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

Commit:

```bash
git add docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: document sysctl target-profile policy"
```

## Task 7: Slice Closeout Verification and Review

**Files:**
- Verify-only across the files changed by Tasks 1-6.

**Interfaces:**
- Produces whole-slice proof that sysctl adapter replacement and Remi public gating agree.

- [ ] **Step 1: Run focused product tests**

Run:

```bash
cargo test -p conary-core sysctl --lib
cargo test -p conary-core scriptlet_bundle --lib
cargo test -p conary-core golden_fixtures --lib
cargo test -p conary-core support_matrix --lib
cargo test -p remi conversion --lib
cargo test -p remi publication --lib
cargo test -p conary --test conversion_integration golden_conversion
cargo run -p conary-test -- list
```

Expected: PASS.

- [ ] **Step 2: Run workspace hygiene and docs gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

- [ ] **Step 3: Request final code review**

Generate a review package from the slice base commit through `HEAD`, dispatch a final reviewer with the umbrella spec and this plan as required context, and fix Critical/Important findings before considering the slice complete.

- [ ] **Step 4: Commit final verification note if docs changed during review**

If review fixes changed docs or ledgers, rerun Step 2 and commit the final adjustments with:

```bash
git commit -m "docs: close sysctl target-profile policy plan"
```
