# Source Selection Program Implementation Plan

> **Historical note:** This archived implementation plan is preserved for
> traceability. It reflects the intended work and repository state at the time
> it was written, not the current execution contract. Use active docs under
> `docs/` and non-archived `docs/superpowers/` for current guidance.

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Status:** Executed and merged on 2026-04-07. Keep this document for traceability; do not treat it as pending implementation work.

**Goal:** Build one coherent source-selection subsystem with persistent `selection_mode` and consistent behavior across install, transitive resolution, update, and replatform/model-apply flows.

**Architecture:** Treat `system.profile` as the model-layer preset surface, decompose it into explicit runtime policy state, and teach root resolution, SAT ordering, update, and replatform to consume the same effective policy instead of keeping separate source-selection logic in each path. Use Repology `status == "newest"` as the v1 latest signal, fail closed for explicit version constraints, wire `allowed_distros` into eligibility, and keep all flow-specific behavior behind one persisted effective policy mirror.

**Tech Stack:** Rust, rusqlite, existing repository/resolver modules, resolvo SAT provider, model diff/apply pipeline, clap CLI, `cargo test`

**Milestone Mapping:** Chunk 1 corresponds to spec Milestone 1, Chunk 2 to Milestone 2, Chunk 3 to Milestone 3, Chunk 4 to Milestone 4, and Chunk 5 is final verification. Chunk boundaries are execution groupings, not a different scope model from the spec.

**Commit Convention:** Every commit in this plan should include a body line referencing `docs/superpowers/specs/2026-04-07-source-selection-policy-design.md`.

---

## File Map

**Shared policy substrate**

- Create: `crates/conary-core/src/repository/effective_policy.rs`
  - Runtime load/store helpers for `selection_mode` and `allowed_distros`
  - Shared `EffectiveSourcePolicy` builder from DB state
  - Setting key constants and parsing/validation
- Create: `crates/conary-core/src/repository/latest_signal.rs`
  - Repology-backed latest-signal lookup and scoring helpers
  - Staleness threshold and ambiguity handling
- Modify: `crates/conary-core/src/model/parser.rs`
  - Reconcile `system.profile` with explicit decomposed policy fields
  - Add precedence helpers and update `is_source_policy_configured()`
- Modify: `crates/conary-core/src/repository/resolution_policy.rs`
  - Add `SelectionMode`
  - Extend `ResolutionPolicy` to carry it and the effective allowlist
  - Add builders/serde/tests
- Modify: `crates/conary-core/src/repository/mod.rs`
  - Export the new helpers
- Modify: `crates/conary-core/src/db/models/repology_cache.rs`
  - Add focused batch lookup helpers for ranking instead of full-table scans

**Resolver integration**

- Modify: `crates/conary-core/src/resolver/canonical.rs`
  - Apply `selection_mode=latest` at root canonical ranking
- Modify: `crates/conary-core/src/resolver/provider/mod.rs`
  - Store effective policy on `ConaryProvider` for resolvo trait callbacks
- Modify: `crates/conary-core/src/resolver/provider/traits.rs`
  - Make SAT candidate ordering policy-aware
- Modify: `crates/conary-core/src/resolver/sat/install.rs`
  - Construct `ConaryProvider` with effective policy
- Modify: `crates/conary-core/src/resolver/sat/removal.rs`
  - Preserve backward-compatible provider construction where policy is not relevant
- Modify: `crates/conary-core/src/repository/selector.rs`
  - Decide and implement latest-aware exact-name repository selection where needed
- Modify: `crates/conary-core/src/repository/resolution.rs`
  - Thread the richer effective policy through package resolution
- Modify: `apps/conary/src/commands/install/mod.rs`
  - Stop building root policy ad hoc inside install

**CLI and runtime state**

- Modify: `apps/conary/src/cli/distro.rs`
  - Add a `selection-mode` management surface
- Modify: `apps/conary/src/commands/distro.rs`
  - Persist and display `selection_mode`
- Modify: `apps/conary/src/dispatch.rs`
  - Dispatch the new CLI surface

**Update integration**

- Modify: `apps/conary/src/cli/mod.rs`
  - Add update dry-run support if needed for source-switch previews
- Modify: `apps/conary/src/commands/update.rs`
  - Load effective policy instead of passing `policy: None`
  - Re-evaluate candidates according to the chosen update semantics
  - Preview and confirm source switches

**Model / replatform integration**

- Modify: `crates/conary-core/src/model/parser.rs`
  - Add explicit model-level decomposed policy representation
- Modify: `crates/conary-core/src/model/state.rs`
  - Capture persisted `selection_mode` and `allowed_distros` in system state
- Modify: `crates/conary-core/src/model/diff.rs`
  - Diff desired vs current decomposed policy state
- Modify: `apps/conary/src/commands/model.rs`
  - Render selection-mode drift and convergence messaging
- Modify: `apps/conary/src/commands/model/apply.rs`
  - Persist decomposed policy changes and replace package-action stubs as needed
- Modify: `crates/conary-core/src/model/replatform.rs`
  - Replace bespoke target selection with shared source-policy-aware ranking
- Modify: `crates/conary-core/src/db/models/trove.rs`
  - Support final provenance/selection metadata updates after replatform execution

**Tests and docs**

- Modify: `docs/superpowers/specs/2026-04-07-source-selection-policy-design.md`
  - Keep spec aligned as implementation decisions are locked
- Add or extend tests in the files above, plus any narrow integration coverage needed in `apps/conary/tests/`

## Chunk 1: Shared Policy Substrate

### Task 1: Add `SelectionMode` and explicit allowlist support to the core policy model

**Files:**
- Modify: `crates/conary-core/src/repository/resolution_policy.rs`
- Test: `crates/conary-core/src/repository/resolution_policy.rs`

- [ ] **Step 1: Write the failing unit tests for the new policy axes**

```rust
#[test]
fn resolution_policy_defaults_to_policy_selection_mode() {
    let policy = ResolutionPolicy::new();
    assert_eq!(policy.selection_mode, SelectionMode::Policy);
}

#[test]
fn resolution_policy_builder_sets_latest_mode() {
    let policy = ResolutionPolicy::new().with_selection_mode(SelectionMode::Latest);
    assert_eq!(policy.selection_mode, SelectionMode::Latest);
}

#[test]
fn resolution_policy_rejects_candidates_outside_allowed_distros() {
    let policy = ResolutionPolicy::new().with_allowed_distros(vec!["arch".to_string()]);
    assert!(!policy.accepts_candidate("fedora-43", VersionScheme::Rpm, "bash", true, None));
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary-core resolution_policy_`
Expected: FAIL because `SelectionMode`, `selection_mode`, and explicit allowed-distro filtering do not exist yet

- [ ] **Step 3: Add the enum plus an explicit allowlist field**

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionMode {
    #[default]
    Policy,
    Latest,
}
```

- [ ] **Step 4: Add `with_selection_mode()`, `with_allowed_distros()`, and enforce the allowlist in `accepts_candidate()`**

Run: `cargo test -p conary-core resolution_policy_`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/repository/resolution_policy.rs
git commit -m "feat(policy): add selection mode and allowlist support" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 2: Reconcile `system.profile` with decomposed policy fields

**Files:**
- Modify: `crates/conary-core/src/model/parser.rs`
- Test: `crates/conary-core/src/model/parser.rs`

- [ ] **Step 1: Write failing tests for preset mapping, precedence, explicitness, and validation**

```rust
#[test]
fn source_policy_default_profile_maps_to_latest_selection_mode() {
    let config = SystemConfig::default();
    assert_eq!(config.effective_selection_mode(), Some(SelectionMode::Latest));
}

#[test]
fn source_policy_explicit_selection_mode_overrides_profile_mapping() {
    let config = SystemConfig {
        profile: Some("balanced/latest-anywhere".to_string()),
        selection_mode: Some("policy".to_string()),
        ..Default::default()
    };
    assert_eq!(config.effective_selection_mode(), Some(SelectionMode::Policy));
}

#[test]
fn source_policy_non_default_profile_counts_as_configuration() {
    let config = SystemConfig {
        profile: Some("conservative/policy-first".to_string()),
        ..Default::default()
    };
    assert!(config.is_source_policy_configured());
}

#[test]
fn source_policy_implicit_default_profile_is_not_counted_as_explicit_configuration() {
    let model = parse_model(minimal_model_toml()).unwrap();
    assert!(!model.system.is_source_policy_configured());
}

#[test]
fn source_policy_explicit_default_profile_counts_as_configuration() {
    let model = parse_model(model_toml_with_system("profile = \"balanced/latest-anywhere\"")).unwrap();
    assert!(model.system.is_source_policy_configured());
}

#[test]
fn source_policy_unknown_profile_is_rejected() {
    let model = parse_model(model_toml_with_system("profile = \"mystery/not-real\""));
    assert!(model.is_err());
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary-core source_policy_`
Expected: FAIL because `SystemConfig` does not define the mapping or precedence yet

- [ ] **Step 3: Add explicit decomposed fields and helper methods**

Required behavior:
- keep `system.profile` as the model-layer preset surface
- add optional `system.selection_mode` as a decomposed override
- treat explicit decomposed fields as stronger than profile-derived defaults
- keep the current default profile of `balanced/latest-anywhere`
- reject unknown profile names with a clear validation/model-apply error
- preserve whether `profile` was explicit in the parsed model instead of
  inferring explicitness from the string value alone
- update `is_source_policy_configured()` so an implicit parser default does not
  count as explicit configuration, while an explicitly written default profile,
  non-default profiles, and explicit decomposed overrides do

- [ ] **Step 4: Re-run parser tests**

Run: `cargo test -p conary-core parser::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/model/parser.rs
git commit -m "feat(model): reconcile profile presets with decomposed policy" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 3: Create a shared effective-policy loader backed by settings

**Files:**
- Create: `crates/conary-core/src/repository/effective_policy.rs`
- Modify: `crates/conary-core/src/repository/mod.rs`
- Modify: `crates/conary-core/src/db/models/settings.rs`
- Test: `crates/conary-core/src/repository/effective_policy.rs`

- [ ] **Step 1: Write failing tests for runtime policy mirror loading**

```rust
#[test]
fn effective_policy_loads_default_selection_mode_when_setting_missing() {
    let (_tmp, conn) = create_test_db();
    let policy = load_effective_policy(&conn, RequestScope::Any).unwrap();
    assert_eq!(policy.resolution.selection_mode, SelectionMode::Policy);
}

#[test]
fn effective_policy_loads_latest_selection_mode_from_settings() {
    let (_tmp, conn) = create_test_db();
    settings::set(&conn, SETTINGS_KEY_SELECTION_MODE, "latest").unwrap();
    let policy = load_effective_policy(&conn, RequestScope::Any).unwrap();
    assert_eq!(policy.resolution.selection_mode, SelectionMode::Latest);
}

#[test]
fn effective_policy_loads_allowed_distros_from_settings() {
    let (_tmp, conn) = create_test_db();
    settings::set(&conn, SETTINGS_KEY_ALLOWED_DISTROS, "[\"arch\",\"fedora-43\"]").unwrap();
    let policy = load_effective_policy(&conn, RequestScope::Any).unwrap();
    assert_eq!(policy.resolution.allowed_distros.as_deref(), Some(&["arch".to_string(), "fedora-43".to_string()][..]));
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary-core effective_policy_loads_`
Expected: FAIL because `effective_policy.rs` plus the namespaced settings keys do not exist yet

- [ ] **Step 3: Implement the shared runtime helper and key naming**

```rust
pub const SETTINGS_KEY_SELECTION_MODE: &str = "source.selection-mode";
pub const SETTINGS_KEY_ALLOWED_DISTROS: &str = "source.allowed-distros";

pub struct EffectiveSourcePolicy {
    pub resolution: ResolutionPolicy,
    pub primary_flavor: Option<RepositoryDependencyFlavor>,
}

pub fn load_effective_policy(conn: &Connection, scope: RequestScope) -> Result<EffectiveSourcePolicy> {
    // Read DistroPin + settings, map to ResolutionPolicy, derive primary flavor,
    // and decode the JSON allowlist when present.
}
```

Deliberate storage choice:
- store `allowed_distros` as a JSON array string in `settings` for this
  milestone to avoid adding schema surface area
- note in code comments/tests that a dedicated table can replace this later if
  the setting becomes more complex or more frequently queried

- [ ] **Step 4: Export the helper from `repository/mod.rs` and re-run the tests**

Run: `cargo test -p conary-core effective_policy_loads_`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/repository/effective_policy.rs crates/conary-core/src/repository/mod.rs crates/conary-core/src/db/models/settings.rs
git commit -m "feat(policy): add effective runtime policy loader" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 4: Add Repology latest-signal helpers with concrete acceptance rules

**Files:**
- Create: `crates/conary-core/src/repository/latest_signal.rs`
- Modify: `crates/conary-core/src/db/models/repology_cache.rs`
- Modify: `crates/conary-core/src/repository/mod.rs`
- Test: `crates/conary-core/src/repository/latest_signal.rs`

- [ ] **Step 1: Write failing tests for positive and fallback signal cases**

```rust
#[test]
fn newest_recent_row_is_a_positive_latest_signal() {
    let signal = LatestSignal::from_repology("newest", Some("1.2.3"), "2026-04-07T00:00:00Z", now()).unwrap();
    assert!(signal.is_positive());
}

#[test]
fn outdated_or_stale_rows_do_not_count_as_positive_signal() {
    assert!(!LatestSignal::from_repology("outdated", Some("1.2.3"), "2026-04-07T00:00:00Z", now()).unwrap().is_positive());
    assert!(!LatestSignal::from_repology("newest", Some("1.2.3"), "2026-03-01T00:00:00Z", now()).unwrap().is_positive());
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary-core latest_signal`
Expected: FAIL because `latest_signal.rs` does not exist yet

- [ ] **Step 3: Implement the signal model and explicit lookup path**

```rust
pub enum LatestSignal {
    Positive { version: String },
    Fallback,
}
```

- [ ] **Step 4: Add a batch helper that follows the real data path**

Required lookup path:
- `canonical_id`
- `canonical_packages.name`
- `repology_cache.project_name`
- filtered to the eligible distro set for the current ranking call

Recommended helper shape:

```rust
pub fn find_for_canonical_and_distros(
    conn: &Connection,
    canonical_id: i64,
    distros: &[String],
) -> Result<Vec<RepologyCacheEntry>>
```

Run: `cargo test -p conary-core latest_signal`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/repository/latest_signal.rs crates/conary-core/src/db/models/repology_cache.rs crates/conary-core/src/repository/mod.rs
git commit -m "feat(policy): add repology latest signal helpers" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 5: Add a user-visible `selection_mode` runtime surface

**Files:**
- Modify: `apps/conary/src/cli/distro.rs`
- Modify: `apps/conary/src/commands/distro.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Test: `apps/conary/src/commands/distro.rs`

- [ ] **Step 1: Write failing CLI-level tests for reading and writing `selection_mode`**

```rust
#[tokio::test]
async fn test_cmd_distro_selection_mode_persists_latest() {
    let (_tmp, db_path, conn) = create_test_db();
    cmd_distro_selection_mode(&db_path, "latest").await.unwrap();
    assert_eq!(settings::get(&conn, "source.selection-mode").unwrap().as_deref(), Some("latest"));
}

#[tokio::test]
async fn test_cmd_distro_info_includes_effective_selection_mode() {
    let (_tmp, db_path, conn) = create_test_db();
    settings::set(&conn, "source.selection-mode", "latest").unwrap();
    let rendered = render_distro_info(&conn).unwrap();
    assert!(rendered.contains("Selection mode: latest"));
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary distro_selection_mode`
Expected: FAIL because the command and CLI shape do not exist yet

- [ ] **Step 3: Add a new `distro selection-mode` command and show it in `distro info`**

```rust
SelectionMode {
    mode: String,
    #[command(flatten)]
    db: DbArgs,
}
```

Required behavior:
- `distro info` should show the effective selection mode and, when possible,
  where it came from, such as `latest (from profile balanced/latest-anywhere)`
  vs `policy (runtime default)`

- [ ] **Step 4: Re-run the focused tests and a small CLI smoke test**

Run: `cargo test -p conary distro_selection_mode`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/cli/distro.rs apps/conary/src/commands/distro.rs apps/conary/src/dispatch.rs
git commit -m "feat(cli): add selection mode command" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

## Chunk 2: Resolver Coherence

### Task 6: Make root canonical ranking honor `selection_mode=latest`

**Files:**
- Modify: `crates/conary-core/src/resolver/canonical.rs`
- Test: `crates/conary-core/src/resolver/canonical.rs`

- [ ] **Step 1: Write failing tests for `latest` root ranking**

```rust
#[test]
fn latest_mode_prefers_newest_repology_candidate_before_pin_affinity_tiebreakers() {
    let (_tmp, conn) = create_test_db();
    // Seed canonical impls + repology rows for fedora and arch.
    let policy = ResolutionPolicy::new().with_selection_mode(SelectionMode::Latest);
    let ranked = CanonicalResolver::new(&conn).rank_candidates_with_policy(&candidates, &policy).unwrap();
    assert_eq!(ranked[0].distro, "arch");
}

#[test]
fn latest_mode_does_not_choose_ineligible_newest_candidate() {
    let (_tmp, conn) = create_test_db();
    let policy = ResolutionPolicy::new()
        .with_selection_mode(SelectionMode::Latest)
        .with_allowed_distros(vec!["fedora".to_string()]);
    let ranked = CanonicalResolver::new(&conn).rank_candidates_with_policy(&candidates, &policy).unwrap();
    assert_eq!(ranked[0].distro, "fedora");
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary-core latest_mode_prefers_newest_repology_candidate_before_pin_affinity_tiebreakers`
Expected: FAIL because canonical ranking still ignores `selection_mode`

- [ ] **Step 3: Use the shared latest-signal helper in `rank_candidates_with_policy()`**

```rust
if policy.selection_mode == SelectionMode::Latest {
    // Rank candidates with positive latest signal ahead of fallback candidates.
}
```

Required behavior:
- record enough selection context for install-path diagnostics to explain when
  `latest` chose a candidate because Repology marked it `newest`
- emit or surface fallback diagnostics when `latest` did not influence the
  result because Repology data was stale, missing, or ambiguous

- [ ] **Step 4: Re-run the focused tests and the existing canonical resolver tests**

Run: `cargo test -p conary-core canonical::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/resolver/canonical.rs
git commit -m "feat(resolver): honor latest mode in canonical ranking" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 7: Make SAT candidate ordering consume the same policy

**Files:**
- Modify: `crates/conary-core/src/resolver/provider/traits.rs`
- Modify: `crates/conary-core/src/resolver/provider/mod.rs`
- Modify: `crates/conary-core/src/resolver/sat/install.rs`
- Modify: `crates/conary-core/src/resolver/sat/removal.rs`
- Test: `crates/conary-core/src/resolver/provider/mod.rs`

- [ ] **Step 1: Write a failing solver-ordering test**

```rust
#[test]
fn sort_candidates_prefers_latest_signal_when_policy_requests_it() {
    let (_tmp, conn) = setup_test_db();
    let mut provider = ConaryProvider::new_with_policy(&conn, ResolutionPolicy::new().with_selection_mode(SelectionMode::Latest));
    // Seed same logical package from two distros and assert newest-positive candidate sorts first.
}
```

- [ ] **Step 2: Run the focused test to confirm it fails**

Run: `cargo test -p conary-core sort_candidates_prefers_latest_signal_when_policy_requests_it`
Expected: FAIL because provider sorting is not policy-aware

- [ ] **Step 3: Thread the policy into `ConaryProvider` and update the constructor call sites**

```rust
pub fn new_with_policy(conn: &'a Connection, policy: ResolutionPolicy) -> Self

if self.policy.selection_mode == SelectionMode::Latest {
    // Apply latest signal before repo priority / version fallback.
}
```

Required constraint:
- `resolvo::DependencyProvider::sort_candidates()` has a fixed trait signature,
  so the policy must live on `ConaryProvider`
- keep `ConaryProvider::new(conn)` as a backward-compatible default that uses
  `SelectionMode::Policy`
- update the SAT install call sites to use `new_with_policy()` where effective
  policy should matter

Required caching strategy:
- do not query Repology from each `sort_candidates()` call
- batch-load the needed latest-signal rows during provider construction and
  store them in a lookup map on `ConaryProvider`
- key the cache by the candidate identity actually available during SAT
  ordering, such as canonical-id plus distro or an equivalent stable key
- keep root canonical ranking free to do smaller on-demand lookups because its
  candidate sets are much smaller

- [ ] **Step 4: Re-run focused solver tests plus dependency-resolution coverage**

Run: `cargo test -p conary-core resolver::provider`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/resolver/provider/traits.rs crates/conary-core/src/resolver/provider/mod.rs crates/conary-core/src/resolver/sat/install.rs crates/conary-core/src/resolver/sat/removal.rs
git commit -m "feat(resolver): make sat sorting selection-mode aware" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 8: Replace install's ad hoc policy builder and close the exact-name repository-selection bypass

**Files:**
- Modify: `crates/conary-core/src/repository/selector.rs`
- Modify: `crates/conary-core/src/repository/resolution.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Test: `crates/conary-core/src/repository/selector.rs`

- [ ] **Step 1: Write a failing test for exact-name selection under `latest`**

```rust
#[test]
fn select_best_respects_latest_mode_for_cross_distro_exact_name_candidates() {
    let (_tmp, conn) = create_test_db();
    // Seed same package name from two allowed repos with different distros.
    let selected = PackageSelector::find_best_package(&conn, "foo", &options).unwrap();
    assert_eq!(selected.repository.name, "arch");
}
```

- [ ] **Step 2: Run the focused test to confirm it fails**

Run: `cargo test -p conary-core select_best_respects_latest_mode_for_cross_distro_exact_name_candidates`
Expected: FAIL because selector still uses repo-priority-first ordering

- [ ] **Step 3: Teach selector/resolution to consult `selection_mode` before repo-priority fallback**

```rust
match options.policy.as_ref().map(|p| p.selection_mode) {
    Some(SelectionMode::Latest) => { /* consult latest signal */ }
    _ => { /* existing ordering */ }
}
```

Required install wiring:
- stop building install policy solely through `build_resolution_policy()`
- load the shared effective policy first so `selection_mode` and
  `allowed_distros` actually reach install resolution
- treat install-only CLI scope overrides such as `--from` / `--repo` as a root
  request overlay on top of the loaded effective policy rather than as a
  separate policy source

- [ ] **Step 4: Re-run selector tests and a focused install-path smoke test**

Run: `cargo test -p conary-core repository::selector`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/repository/selector.rs crates/conary-core/src/repository/resolution.rs apps/conary/src/commands/install/mod.rs
git commit -m "feat(resolver): close exact-name selection bypass" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

## Chunk 3: Update Coherence

### Task 9: Lock the update semantic contract and safety behavior in tests first

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Test: `apps/conary/src/commands/update.rs`

- [ ] **Step 1: Add failing tests for the chosen update semantics and source-switch safeguards**

Recommended contract for this plan:
- `selection_mode=policy`: keep current-source-biased update behavior unless request scope/pin/profile says otherwise
- `selection_mode=latest`: re-evaluate eligible sources for each installed package and allow source switches when an allowed candidate has the positive latest signal
- source-switching updates must preview the change, explain why it happened, and require confirmation unless `--yes` is supplied
- update dry-run must show proposed source switches without applying them

```rust
#[test]
fn latest_mode_update_can_switch_sources_when_newest_allowed_candidate_differs() {
    // Seed installed Fedora package, newer allowed Arch candidate, latest mode.
    // Assert update logic selects Arch candidate.
}

#[test]
fn latest_mode_update_previews_source_switches_in_dry_run() {
    // Assert dry-run output includes "Fedora -> Arch" style source preview.
}

#[test]
fn latest_mode_update_requires_confirmation_for_source_switch_without_yes() {
    // Assert the command does not silently switch sources when --yes is false.
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary latest_mode_update_`
Expected: FAIL because update still passes `policy: None`

- [ ] **Step 3: Document the chosen semantics inline near the update selection logic**

```rust
// In latest mode, updates re-evaluate allowed sources rather than staying
// pinned to the currently installed repository when a newer allowed source exists.
// Source switches must be previewed and confirmed unless --yes is supplied.
```

- [ ] **Step 4: Re-run the focused test to confirm it still fails for the right reason**

Run: `cargo test -p conary latest_mode_update_`
Expected: FAIL in selection logic, not in test setup

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/commands/update.rs
git commit -m "test(update): lock latest-mode update semantics" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 10: Make update load and use the shared effective policy

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `crates/conary-core/src/repository/resolution.rs`
- Test: `apps/conary/src/commands/update.rs`

- [ ] **Step 1: Replace `policy: None` with the shared effective policy**

```rust
let effective = load_effective_policy(&conn, RequestScope::Any)?;
let options = ResolutionOptions {
    policy: Some(effective.resolution.clone()),
    is_root: false,
    primary_flavor: effective.primary_flavor,
    ..
};
```

- [ ] **Step 2: Implement source re-evaluation plus preview/confirmation behavior**

Required behavior:
- `--dry-run` shows proposed updates and source switches without applying them
- when a package would switch source under `latest`, print the before/after
  source and the reason
- require confirmation for source-switching updates unless `--yes` is set
- preserve same-source behavior in `policy` mode unless policy inputs say
  otherwise
- keep `is_root = false` for update resolution in this milestone because
  `update` has no per-invocation `--from` / `--repo` source-scope flags today;
  if update later gains request-scope CLI overrides, that should be a separate
  CLI design

Run: `cargo test -p conary latest_mode_update_`
Expected: PASS

- [ ] **Step 3: Add/update tests for same-source `policy` mode and guarded/strict pin behavior**

Run: `cargo test -p conary update::`
Expected: PASS

- [ ] **Step 4: Run broader update/install regression coverage**

Run: `cargo test -p conary source_policy_update_context`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/commands/update.rs crates/conary-core/src/repository/resolution.rs
git commit -m "feat(update): make update source-policy aware" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

## Chunk 4: Model And Replatform Coherence

### Task 11: Capture and diff decomposed source policy in model state

**Files:**
- Modify: `crates/conary-core/src/model/parser.rs`
- Modify: `crates/conary-core/src/model/state.rs`
- Modify: `crates/conary-core/src/model/diff.rs`
- Modify: `apps/conary/src/commands/model.rs`
- Test: `crates/conary-core/src/model/state.rs`
- Test: `crates/conary-core/src/model/diff.rs`

- [ ] **Step 1: Write failing state/diff tests for selection-mode drift**

```rust
#[test]
fn source_policy_state_reads_selection_mode_from_settings() {
    let (_tmp, conn) = create_test_db();
    settings::set(&conn, "source.selection-mode", "latest").unwrap();
    let state = capture_current_state(&conn).unwrap();
    assert_eq!(state.selection_mode, Some(SelectionMode::Latest));
}

#[test]
fn source_policy_state_reads_allowed_distros_from_settings() {
    let (_tmp, conn) = create_test_db();
    settings::set(&conn, "source.allowed-distros", "[\"arch\"]").unwrap();
    let state = capture_current_state(&conn).unwrap();
    assert_eq!(state.allowed_distros, vec!["arch".to_string()]);
}

#[test]
fn source_policy_diff_emits_selection_mode_change() {
    let mut state = SystemState::new();
    state.selection_mode = Some(SelectionMode::Policy);
    let mut model = SystemModel::new();
    model.system.selection_mode = Some("latest".to_string());
    let diff = compute_diff(&model, &state);
    assert!(diff.actions.iter().any(|a| matches!(a, DiffAction::SetSelectionMode { .. })));
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary-core source_policy_`
Expected: FAIL because state and diff do not know about decomposed runtime policy yet

- [ ] **Step 3: Add explicit model representation for decomposed policy state**

Recommended approach:
- add `system.selection_mode` as an explicit field in `SystemConfig`
- keep `system.profile` as the higher-level preset string
- capture persisted `allowed_distros` in `SystemState`
- diff decomposed runtime mirrors against model-derived effective policy

- [ ] **Step 4: Re-run the focused tests and snapshot/model command tests**

Run: `cargo test -p conary-core source_policy_`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/model/state.rs crates/conary-core/src/model/diff.rs apps/conary/src/commands/model.rs crates/conary-core/src/model/parser.rs
git commit -m "feat(model): capture and diff source policy state" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 12: Persist decomposed source-policy changes in `model apply`

**Files:**
- Modify: `apps/conary/src/commands/model/apply.rs`
- Modify: `apps/conary/src/commands/model.rs`
- Test: `apps/conary/src/commands/model.rs`

- [ ] **Step 1: Write failing tests for model-apply selection-mode changes**

```rust
#[tokio::test]
async fn test_model_apply_updates_selection_mode_without_package_changes() {
    // Write model with system.selection_mode = "latest".
    // Run cmd_model_apply and assert settings now store latest.
}

#[tokio::test]
async fn test_model_apply_updates_allowed_distros_without_package_changes() {
    // Write model with system.allowed_distros = ["arch"].
    // Run cmd_model_apply and assert settings mirror the allowlist.
}
```

- [ ] **Step 2: Run the focused test to confirm it fails**

Run: `cargo test -p conary model_apply_updates_`
Expected: FAIL because model apply only handles source pin changes

- [ ] **Step 3: Add decomposed source-policy diff actions and persistence handling**

```rust
match action {
    DiffAction::SetSelectionMode { mode } => settings::set(conn, SETTINGS_KEY_SELECTION_MODE, mode.as_str())?,
    DiffAction::ClearSelectionMode => settings::delete(conn, SETTINGS_KEY_SELECTION_MODE)?,
    DiffAction::SetAllowedDistros { distros } => settings::set(conn, SETTINGS_KEY_ALLOWED_DISTROS, &serde_json::to_string(distros)?)?,
    DiffAction::ClearAllowedDistros => settings::delete(conn, SETTINGS_KEY_ALLOWED_DISTROS)?,
    _ => {}
}
```

- [ ] **Step 4: Re-run focused model tests**

Run: `cargo test -p conary model_apply_updates_`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/model/apply.rs apps/conary/src/commands/model.rs crates/conary-core/src/model/diff.rs
git commit -m "feat(model): persist decomposed source policy" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 13: Make replatform planning use the shared selector instead of bespoke ranking

**Files:**
- Modify: `crates/conary-core/src/model/replatform.rs`
- Test: `crates/conary-core/src/model/replatform.rs`

- [ ] **Step 1: Write failing tests for selection-aware replatform targeting**

```rust
#[test]
fn source_policy_replatform_snapshot_uses_latest_mode_when_selecting_targets() {
    let (_tmp, conn) = create_test_db();
    settings::set(&conn, "source.selection-mode", "latest").unwrap();
    let snapshot = source_policy_replatform_snapshot(&conn, "arch").unwrap();
    assert!(snapshot.visible_realignment_proposals.iter().any(|p| p.target_distro == "arch"));
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary-core source_policy_replatform_snapshot_uses_latest_mode_when_selecting_targets`
Expected: FAIL because replatform planning still uses bespoke per-distro version sorting

- [ ] **Step 3: Replace ad hoc target selection with the shared policy/ranking helpers**

```rust
// Stop hand-rolling candidate_target_package(); instead build the effective
// target policy and ask the shared selector for the winning candidate.
```

- [ ] **Step 4: Re-run focused replatform tests plus existing snapshot coverage**

Run: `cargo test -p conary-core replatform::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/model/replatform.rs
git commit -m "feat(replatform): use shared source-selection ranking" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 14: Define the replatform execution transaction model

**Files:**
- Modify: `crates/conary-core/src/model/replatform.rs`
- Modify: `apps/conary/src/commands/model.rs`
- Test: `crates/conary-core/src/model/replatform.rs`

- [ ] **Step 1: Write failing tests for executable vs blocked replatform transactions**

```rust
#[test]
fn replatform_execution_plan_marks_transaction_executable_only_when_all_legs_are_ready() {
    let plan = build_replatform_execution_plan(...);
    assert!(plan.transactions[0].executable);
    assert!(plan.transactions[0].blocked_reasons.is_empty());
}

#[test]
fn replatform_execution_plan_marks_transaction_blocked_when_route_is_missing() {
    let plan = build_replatform_execution_plan(...);
    assert!(!plan.transactions[0].executable);
    assert!(plan.transactions[0].blocked_reasons.iter().any(|r| r.contains("route")));
}
```

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary-core replatform_execution_plan_`
Expected: FAIL if the current plan model is too weak or underspecified

- [ ] **Step 3: Make the execution plan explicit about atomic units**

Required plan shape:
- remove old package source leg
- install replacement source leg
- metadata/provenance update leg
- failure classification before any mutation begins

- [ ] **Step 4: Re-run focused plan-model tests**

Run: `cargo test -p conary-core replatform_execution_plan_`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/model/replatform.rs apps/conary/src/commands/model.rs
git commit -m "feat(replatform): define execution transaction model" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 15: Implement the remove and install legs for executable replatform transactions

**Files:**
- Modify: `apps/conary/src/commands/model/apply.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Test: `apps/conary/src/commands/model.rs`

- [ ] **Step 1: Write failing tests for executing a `ReplatformReplace` action**

```rust
#[tokio::test]
async fn test_model_apply_executes_replatform_replacement_when_route_is_executable() {
    // Seed installed Fedora package + executable Arch replacement transaction.
    // Run cmd_model_apply and assert old trove is removed or superseded,
    // replacement trove is installed, and the recorded source changes.
}
```

Concrete assertions:
- installed trove set no longer reports the old source as active
- replacement trove exists with the expected target source metadata
- model/apply output shows the replacement as executed rather than planning-only

- [ ] **Step 2: Run the focused test to confirm it fails**

Run: `cargo test -p conary test_model_apply_executes_replatform_replacement_when_route_is_executable`
Expected: FAIL because model apply currently prints a planning-only notice

- [ ] **Step 3: Replace the package-action stub path with shared install/remove execution**

Required behavior:
- executable replatform transactions call the same install/remove primitives as normal package operations
- blocked transactions stay visible and fail clearly
- installation happens through the same shared source-selection-aware primitives used elsewhere

- [ ] **Step 4: Re-run focused model-apply tests and any affected install/update regressions**

Run: `cargo test -p conary test_model_apply_executes_replatform_replacement_when_route_is_executable`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/model/apply.rs apps/conary/src/commands/install/mod.rs apps/conary/src/commands/update.rs
git commit -m "feat(model): execute replatform install and remove legs" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

### Task 16: Handle partial failure, metadata updates, and end-to-end execution tests

**Files:**
- Modify: `apps/conary/src/commands/model/apply.rs`
- Modify: `apps/conary/src/commands/model.rs`
- Modify: `crates/conary-core/src/db/models/trove.rs`
- Test: `apps/conary/src/commands/model.rs`

- [ ] **Step 1: Write failing tests for partial-failure handling**

```rust
#[tokio::test]
async fn test_model_apply_rolls_back_or_reports_partial_failure_during_replatform() {
    // Inject a failure after install succeeds but before provenance update.
    // Assert either rollback restores the old active source, or the system
    // records an explicit auditable partial-failure state.
}
```

Concrete assertions:
- failure output distinguishes blocked vs failed execution
- trove/provenance state after failure is explicit and queryable
- no silent success path leaves old and new sources in an ambiguous state

- [ ] **Step 2: Run the focused tests to confirm they fail**

Run: `cargo test -p conary test_model_apply_rolls_back_or_reports_partial_failure_during_replatform`
Expected: FAIL because replatform execution does not yet handle mid-transaction failure cleanly

- [ ] **Step 3: Make the post-install metadata update explicit and safe**

Required behavior:
- update selection reason and source provenance on success
- if true rollback is not feasible for a leg, fail loudly and leave an auditable state
- keep blocked vs failed execution clearly distinguished in user-facing output

- [ ] **Step 4: Re-run focused tests plus end-to-end replatform execution tests**

Run: `cargo test -p conary replatform`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/model/apply.rs apps/conary/src/commands/model.rs crates/conary-core/src/db/models/trove.rs
git commit -m "feat(model): harden replatform execution failure handling" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```

## Chunk 5: Full Verification And Cleanup

### Task 17: Run the full verification matrix and align docs/spec text

**Files:**
- Modify: `docs/superpowers/specs/2026-04-07-source-selection-policy-design.md`
- Modify: any touched docs/tests discovered during verification

- [ ] **Step 1: Run targeted package tests for every touched subsystem**

Run: `cargo test -p conary-core repository::`
Expected: PASS

Run: `cargo test -p conary-core resolver::`
Expected: PASS

Run: `cargo test -p conary-core model::`
Expected: PASS

- [ ] **Step 2: Run CLI/service package tests that cover command integration**

Run: `cargo test -p conary`
Expected: PASS

- [ ] **Step 3: Run formatting and linting**

Run: `cargo fmt --check`
Expected: PASS

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS

- [ ] **Step 4: Update the spec if implementation decisions changed any milestone wording**

Run: `git diff -- docs/superpowers/specs/2026-04-07-source-selection-policy-design.md`
Expected: only intentional wording alignment

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-04-07-source-selection-policy-design.md
git commit -m "docs: align source-selection spec with implementation" -m "Part of docs/superpowers/specs/2026-04-07-source-selection-policy-design.md"
```
