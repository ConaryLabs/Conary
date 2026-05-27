# Legacy Scriptlet Semantics Bundle Goal Queue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the legacy scriptlet semantics bundle spec into a sequence of small Codex `/goal` objectives with clear stopping conditions.

**Architecture:** The work is split into independent, evidence-producing goals. Each goal leaves the repo in a useful state, has its own verification loop, and can be merged, pushed, and archived before the next goal starts.

**Tech Stack:** Rust workspace, Remi server, `conary-core` CCS conversion, SQLite-backed repository metadata, local CLI tools, existing Cargo test/clippy/fmt gates.

---

## Source Spec

Read this first:

- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`

The spec is too large for one Codex goal. Treat this document as the goal queue and create or use a focused implementation plan for each goal before touching code.

## `/goal` Usage Pattern

Each goal should start with a concise objective that names the artifact, the boundary, and the verification gate.

Use this shape:

```text
/goal Implement the named goal from docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md. Stop when that goal's verification commands pass and the branch is ready for merge/push/cleanup.
```

At the end of each goal:

- run the goal-specific verification commands;
- run `cargo fmt --check`;
- run `git diff --check`;
- run targeted Cargo tests named by the goal;
- run `cargo clippy --workspace --all-targets -- -D warnings` unless the goal records a narrower justified lint gate;
- commit with one conventional-style commit;
- merge/push/cleanup when requested;
- archive or update the plan only after the goal is actually complete.

## Goal 0: Remi Conversion Benchmark And Corpus Scan

Recommended objective:

```text
/goal Implement Goal 0: add non-mutating Remi conversion timing and scriptlet corpus-scan evidence. Instrument async conversion phases with explicit millisecond duration tracking, handle remote cloud storage metrics, and parse scriptlet commands after shell control operators. Stop when the benchmark command emits correct JSON evidence, target clippy and Remi tests pass, and docs record the workflow.
```

Detailed plan:

- `docs/superpowers/plans/2026-05-27-remi-conversion-benchmark-corpus-plan.md`

Files likely touched:

- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/conversion_timing.rs`
- `apps/remi/src/server/scriptlet_corpus.rs`
- `apps/remi/src/server/mod.rs`
- `apps/remi/src/bin/remi.rs`
- `docs/modules/remi.md`

Done means:

- Remi can time package lookup, download, checksum/cache lookup, parse, conversion, chunk storage, and persistence.
- R2 write-through timing is measured when an R2 store is configured, or the benchmark output explicitly records that R2 timing was skipped.
- A CLI path can run a benchmark over named packages or a bounded sample from repository metadata.
- Scriptlet command frequency and blocked-class hints are emitted as evidence only, not as conversion authority.
- The first evidence format can support later adapter registry decisions.

Verification:

```bash
cargo test -p remi conversion_timing
cargo test -p remi scriptlet_corpus
cargo test -p remi conversion
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 1: Bundle Schema V1 And Passive Query Surface

Recommended objective:

```text
/goal Implement Goal 1: add the versioned legacy scriptlet semantics bundle data model, manifest round trips, and passive query rendering without changing install behavior. Stop when schema tests prove reserved trigger/purge fields round-trip and converted packages can carry passive bundle metadata.
```

Design and detailed plan:

- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-bundle-schema-v1-passive-query-design.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-bundle-schema-v1-passive-query-plan.md`

Files likely touched:

- `crates/conary-core/src/ccs/manifest.rs`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/ccs/mod.rs`
- `crates/conary-core/src/ccs/archive_reader.rs`
- `crates/conary-core/src/ccs/builder/package_writer.rs`
- `apps/conary/src/commands/mod.rs`
- `apps/conary/src/commands/query/mod.rs`
- `apps/conary/src/commands/query/scripts.rs`
- `apps/conary/src/cli/query.rs`
- `apps/conary/src/dispatch.rs`

Done means:

- `LegacyScriptletBundle` carries source metadata, target compatibility, entries, effects, reserved trigger fields, decisions, timeouts, adapter digests, and evidence digests.
- TOML manifests and mixed CBOR+TOML CCS archives preserve the bundle; CBOR-only
  manifests remain default-empty for Goal 1.
- `conary query scripts <pkg>`, `--verbose`, `--entry`, and `--json` can render bundle metadata for a CCS package.
- No install/update/remove behavior changes yet.

Verification:

```bash
cargo test -p conary-core legacy_scriptlets
cargo test -p conary query_scripts
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 2: Native ABI Extraction For RPM, DEB, And Arch

Design:

- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-native-abi-extraction-design.md`

Recommended objective:

```text
/goal Implement Goal 2: convert current flattened scriptlets into native ABI entries for RPM, DEB, and Arch, preserving lifecycle paths and deferred trigger fields. Stop when parser fixture tests prove no native scriptlet slot is silently dropped.
```

Files likely touched:

- `crates/conary-core/src/packages/traits.rs`
- `crates/conary-core/src/packages/rpm.rs`
- `crates/conary-core/src/packages/deb.rs`
- `crates/conary-core/src/packages/arch.rs`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/tests/`

Done means:

- RPM entries preserve `%pre`, `%post`, `%preun`, `%postun`, `%pretrans`, `%posttrans`, and trigger metadata when available.
- DEB entries preserve maintainer-script invocation modes and control `triggers` content when present.
- Arch entries preserve full `.INSTALL` source plus callable function metadata.
- Unsupported native slots become `review` or `blocked` evidence, not dropped data.

Verification:

```bash
cargo test -p conary-core native_abi
cargo test -p conary-core rpm_scriptlet
cargo test -p conary-core deb_scriptlet
cargo test -p conary-core arch_scriptlet
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 3a: Adapter Registry And Blocked-Class Registry Infrastructure

Recommended objective:

```text
/goal Implement Goal 3a: add the effect adapter registry infrastructure, blocked-class registry, command evidence model, and support matrix scaffolding. Stop when fixture invocations are classified as known, unknown, review, or blocked with stable reason IDs, without broad helper-specific replacement claims.
```

Files likely touched:

- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- `crates/conary-core/src/ccs/convert/capture.rs`
- `crates/conary-core/src/ccs/convert/analyzer.rs`
- `crates/conary-core/src/ccs/convert/mod.rs`

Done means:

- The registry consumes structured invocations, metadata, payload hints, and curated rules.
- Unsafe classes such as network, package-manager recursion, PAM, kernel/initramfs/bootloader, SELinux/AppArmor policy, and unmodeled triggers fail with stable class IDs.
- Regex analysis may remain as a temporary signal source but not the authority for `replaced`.
- Adapter infrastructure can report complete, partial, none, blocked, and unknown outcomes without making the first helper coverage set bigger than the evidence supports.

Verification:

```bash
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core conversion_integration
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 3b: Bootstrap Adapters Driven By Corpus Evidence

Recommended objective:

```text
/goal Implement Goal 3b: add bootstrap effect adapters selected from Goal 0 corpus evidence, with fixtures and support-matrix entries. Stop when common preview-corpus helpers are covered by tests and unsupported helper classes still fail with stable reasons.
```

Files likely touched:

- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- `crates/conary-core/src/ccs/convert/capture.rs`
- `crates/conary-core/src/ccs/convert/mod.rs`

Done means:

- Bootstrap adapters are chosen from Goal 0 command-frequency evidence, not guesswork.
- Initial adapter candidates cover no-scriptlet, `ldconfig`, systemd daemon reload, simple systemd enable/disable/preset evidence, tmpfiles, sysusers payload hints, alternatives, and common cache refreshes when the corpus proves they matter.
- Every adapter has fixture coverage and at least one support-matrix entry.
- Adapter decisions explicitly cite the spec's `replaced` and `legacy` rubrics before marking any effect complete.

Verification:

```bash
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core conversion_integration
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 4: Passive Remi Bundle Embedding And Metadata

Recommended objective:

```text
/goal Implement Goal 4: have Remi embed passive legacy scriptlet bundles in converted CCS packages and expose scriptlet fidelity metadata without changing install enforcement. Stop when converted packages carry bundles and Remi DB/API metadata reports fidelity, target compatibility, decisions, unknown commands, and blocked classes.
```

Files likely touched:

- `crates/conary-core/src/ccs/convert/converter.rs`
- `apps/remi/src/server/conversion.rs`
- `crates/conary-core/src/db/models/converted.rs`
- `crates/conary-core/src/db/migrations/v41_current.rs`
- `crates/conary-core/src/db/schema.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `docs/modules/remi.md`

Done means:

- `ConversionResult` includes the bundle and evidence digest.
- `build_manifest` stores the bundle in the CCS manifest or a referenced sidecar.
- Converted package records store publication status and scriptlet fidelity.
- A database migration adds `scriptlet_fidelity`, `target_compatibility`, `publication_status`, `evidence_digest`, `curation_evidence_digest`, `blocked_reason_codes_json`, and `review_artifact_path` fields to `converted_packages` with defaults that preserve existing records.
- Existing installs still use current behavior until enforcement goals land.

Verification:

```bash
cargo test -p conary-core legacy_scriptlets
cargo test -p remi conversion
cargo test -p remi packages
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 5: Review, Blocked, And Publication Gate

Recommended objective:

```text
/goal Implement Goal 5: add Remi publication outcomes for public, review-required, and blocked conversions, including curation evidence and operator-visible reasons. Stop when Remi refuses to serve incomplete converted packages as ready and tests cover public/review/blocked flows.
```

Files likely touched:

- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/jobs.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/handlers/admin/packages.rs`
- `crates/conary-core/src/db/models/converted.rs`
- `docs/modules/remi.md`

Done means:

- Partially converted packages are not served as ready.
- Until Goal 6 client enforcement lands, packages requiring raw legacy replay
  are not served as public-ready artifacts; keep them private-review,
  local-only, or otherwise metadata-only even when conversion evidence is
  complete.
- Review artifacts remain private and carry evidence.
- Blocked results are visible as structured negative conversion results.
- Job status reports conversion phase and actionable blocked/review reasons.

Verification:

```bash
cargo test -p remi jobs
cargo test -p remi package_publication
cargo test -p remi conversion
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 6: Safe Replay Engine With Target Compatibility Gate

Recommended objective:

```text
/goal Implement Goal 6: make install/update/remove consume legacy scriptlet bundles behind an explicit feature gate. Persist installed bundle state for remove/upgrade, enforce target compatibility metadata, and deny foreign legacy replay by default during preflight before live mutation. Stop when bundle-aware same-distro dry-run/install/remove tests pass and cross-distro replay is rejected.
```

Files likely touched:

- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/install/scriptlets.rs`
- `apps/conary/src/commands/remove.rs`
- `crates/conary-core/src/db/migrations/`
- `crates/conary-core/src/db/schema.rs`
- `crates/conary-core/src/scriptlet/mod.rs`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/repository/distro.rs`
- `apps/conary/src/live_host_safety.rs`
- `apps/conary/tests/`

Done means:

- `blocked` and `review` entries fail before mutation under bundle-aware mode.
- No live legacy replay path is enabled until installed bundle state
  persistence and remove/upgrade lookup tests pass.
- `replaced` entries run only CCS declarative hooks.
- `legacy` entries replay preserved native scriptlets with native-compatible args only after target compatibility preflight passes.
- Target IDs use `<format>/<distro>/<release>/<arch>`.
- `foreign_replay_policy = "deny"` is enforced by default before live mutation.
- `strict`, `guarded`, and `permissive` policies behave as specified.
- Install persists the complete `LegacyScriptletBundle` into local
  installed-package state so remove and upgrade can enforce the same decisions,
  target compatibility, sandbox requirements, and timeouts after the original
  `.ccs` archive is no longer present.
- Raw replay and generated hooks from the same entry do not both run.
- The integration test plan for golden behavior fixtures is documented, even if the full native-versus-CCS corpus does not run until Goal 8a.

Verification:

```bash
cargo test -p conary ccs_install
cargo test -p conary-core scriptlet
cargo test -p conary bundle_replay
cargo test -p conary-core target_compatibility
cargo test -p conary foreign_replay
cargo test -p conary live_host_safety
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 7: Compatibility Matrix Expansion And Override Audit

Recommended objective:

```text
/goal Implement Goal 7: expand the target compatibility matrix, helper/path preflight checks, and override audit metadata after Goal 6's default-deny safety gate is in place. Stop when compatible same-family cases require explicit matrix entries and every operator override is recorded.
```

Files likely touched:

- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/repository/distro.rs`
- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/live_host_safety.rs`
- `apps/conary/tests/`

Done means:

- Same-family compatibility is granted only by explicit matrix entries.
- Helper command, path, service-manager, security-policy, and sandbox preflight failures produce stable reason IDs.
- Changeset metadata records compatibility decisions and operator overrides.
- Docs describe why converted CCS format does not imply raw scriptlet portability.

Verification:

```bash
cargo test -p conary-core target_compatibility
cargo test -p conary foreign_replay
cargo test -p conary live_host_safety
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 8a: Lifecycle Expansion And Golden Behavior Fixtures

Recommended objective:

```text
/goal Implement Goal 8a: expand update/remove/trigger coverage and add golden behavior fixtures for supported and unsupported package classes. Stop when supported fixture packages match native observable behavior and unsupported packages fail before mutation with specific reasons.
```

Files likely touched:

- `crates/conary-core/src/ccs/convert/`
- `apps/conary/tests/`
- `apps/remi/tests/`

Done means:

- Golden fixtures cover no-scriptlet, user/group, systemd, tmpfiles/cache, alternatives, unknown legacy replay, blocked package-manager recursion, foreign replay rejection, RPM trigger quarantine, DEB trigger quarantine, and Arch `.INSTALL` wrapper replay.
- Update, remove, trigger, purge, and Arch wrapper behavior is either covered by golden fixtures or fails before mutation with stable reason IDs.

Verification:

```bash
cargo test -p conary-core conversion_integration
cargo test -p conary bundle_replay
cargo test -p remi conversion
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Goal 8b: Regex Authority Retirement And Docs Truth

Recommended objective:

```text
/goal Implement Goal 8b: retire regex analysis as the authoritative conversion mechanism after adapter parity evidence covers the preview corpus, and update public/internal docs to describe scriptlet fidelity truthfully. Stop when corpus parity evidence exists, docs-truth checks pass, and regex analysis is only an advisory signal.
```

Files likely touched:

- `crates/conary-core/src/ccs/convert/analyzer.rs`
- `crates/conary-core/src/ccs/convert/`
- `docs/modules/remi.md`
- `docs/conaryopedia-v2.md`
- `README.md`

Done means:

- Regex analyzer output cannot mark entries `replaced` without adapter or curated-rule evidence.
- Adapter parity against the preview corpus is recorded before any old authority path is removed.
- Public docs describe `native-free`, `fully-replaced`, `legacy-replay`, `review-required`, and `blocked` without overclaiming broad repository portability.

Verification:

```bash
cargo test -p conary-core conversion_integration
cargo test -p remi conversion
bash scripts/check-doc-truth.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Cross-Goal Rules

- Do not widen public claims until evidence exists in the current repo.
- Do not serve partial conversions as ready CCS packages.
- Do not make cross-distro raw replay the default.
- Do not remove existing compatibility paths until the replacement has tests and a migration story.
- Keep each goal small enough to review, commit, merge, push, and clean up before starting the next.
- Run `cargo clippy --workspace --all-targets -- -D warnings` as part of every goal's final verification unless the goal explicitly records why a narrower clippy target is sufficient.
