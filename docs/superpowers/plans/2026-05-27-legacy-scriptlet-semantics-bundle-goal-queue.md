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
- commit with one conventional-style commit;
- merge/push/cleanup when requested;
- archive or update the plan only after the goal is actually complete.

## Goal 0: Remi Conversion Benchmark And Corpus Scan

Recommended objective:

```text
/goal Implement Goal 0: add non-mutating Remi conversion timing and scriptlet corpus-scan evidence so we can measure cold-path latency and adapter bootstrap needs before schema or replay work. Stop when the benchmark command emits JSON evidence, targeted Remi tests pass, and docs record the baseline workflow.
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
- A CLI path can run a benchmark over named packages or a bounded sample from repository metadata.
- Scriptlet command frequency and blocked-class hints are emitted as evidence only, not as conversion authority.
- The first evidence format can support later adapter registry decisions.

Verification:

```bash
cargo test -p remi conversion_timing
cargo test -p remi scriptlet_corpus
cargo test -p remi conversion_benchmark
cargo fmt --check
git diff --check
```

## Goal 1: Bundle Schema V1 And Passive Query Surface

Recommended objective:

```text
/goal Implement Goal 1: add the versioned legacy scriptlet semantics bundle data model, manifest round trips, and passive query rendering without changing install behavior. Stop when schema tests prove reserved trigger/purge fields round-trip and converted packages can carry passive bundle metadata.
```

Files likely touched:

- `crates/conary-core/src/ccs/manifest.rs`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/ccs/mod.rs`
- `crates/conary-core/src/ccs/archive_reader.rs`
- `crates/conary-core/src/ccs/builder/package_writer.rs`
- `apps/conary/src/commands/query.rs`
- `apps/conary/src/cli/query.rs`

Done means:

- `LegacyScriptletBundle` carries source metadata, target compatibility, entries, effects, reserved trigger fields, decisions, timeouts, adapter digests, and evidence digests.
- TOML and binary CCS archive paths preserve the bundle.
- `conary query scripts <pkg>`, `--verbose`, `--entry`, and `--json` can render bundle metadata for a CCS package.
- No install/update/remove behavior changes yet.

Verification:

```bash
cargo test -p conary-core legacy_scriptlets
cargo test -p conary query_scripts
cargo fmt --check
git diff --check
```

## Goal 2: Native ABI Extraction For RPM, DEB, And Arch

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
cargo fmt --check
git diff --check
```

## Goal 3: Adapter Registry And Blocked-Class Registry

Recommended objective:

```text
/goal Implement Goal 3: add the effect adapter registry, blocked-class registry, command evidence model, and bootstrap support matrix for common preview-corpus helpers. Stop when adapters classify fixture invocations and blocked classes produce stable reasons.
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
- Initial adapters cover no-scriptlet, `ldconfig`, systemd daemon reload, simple systemd enable/disable/preset evidence, tmpfiles, sysusers payload hints, alternatives, and common cache refreshes.
- Unsafe classes such as network, package-manager recursion, PAM, kernel/initramfs/bootloader, SELinux/AppArmor policy, and unmodeled triggers fail with stable class IDs.
- Regex analysis may remain as a temporary signal source but not the authority for `replaced`.

Verification:

```bash
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core conversion_integration
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
- `crates/conary-core/src/db/migrations/`
- `apps/remi/src/server/handlers/packages.rs`
- `docs/modules/remi.md`

Done means:

- `ConversionResult` includes the bundle and evidence digest.
- `build_manifest` stores the bundle in the CCS manifest or a referenced sidecar.
- Converted package records store publication status and scriptlet fidelity.
- Existing installs still use current behavior until enforcement goals land.

Verification:

```bash
cargo test -p conary-core legacy_scriptlets
cargo test -p remi conversion
cargo test -p remi packages
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
- Review artifacts remain private and carry evidence.
- Blocked results are visible as structured negative conversion results.
- Job status reports conversion phase and actionable blocked/review reasons.

Verification:

```bash
cargo test -p remi jobs
cargo test -p remi package_publication
cargo test -p remi conversion
cargo fmt --check
git diff --check
```

## Goal 6: Replay Engine Behind Feature Gate

Recommended objective:

```text
/goal Implement Goal 6: make install/update/remove consume legacy scriptlet bundles for no-scriptlet, fully-replaced, and legacy-replay normal paths behind an explicit feature gate. Stop when bundle-aware dry-run and install tests pass without changing default preview behavior unexpectedly.
```

Files likely touched:

- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/install/scriptlets.rs`
- `apps/conary/src/commands/remove.rs`
- `crates/conary-core/src/scriptlet/mod.rs`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `apps/conary/tests/`

Done means:

- `blocked` and `review` entries fail before mutation under bundle-aware mode.
- `replaced` entries run only CCS declarative hooks.
- `legacy` entries replay preserved native scriptlets with native-compatible args when allowed.
- Raw replay and generated hooks from the same entry do not both run.

Verification:

```bash
cargo test -p conary ccs_install
cargo test -p conary-core scriptlet
cargo test -p conary bundle_replay
cargo fmt --check
git diff --check
```

## Goal 7: Target Compatibility Preflight And Foreign Replay Denial

Recommended objective:

```text
/goal Implement Goal 7: enforce target compatibility metadata and deny foreign legacy replay by default before live mutation. Stop when Fedora-origin legacy replay is rejected on Arch/Ubuntu-like targets unless an explicit compatibility matrix entry and operator policy allow it.
```

Files likely touched:

- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/repository/distro.rs`
- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/live_host_safety.rs`
- `apps/conary/tests/`

Done means:

- Target IDs use `<format>/<distro>/<release>/<arch>`.
- `foreign_replay_policy = "deny"` is the default.
- `strict`, `guarded`, and `permissive` policies behave as specified.
- Changeset metadata records compatibility decisions and overrides.

Verification:

```bash
cargo test -p conary-core target_compatibility
cargo test -p conary foreign_replay
cargo test -p conary live_host_safety
cargo fmt --check
git diff --check
```

## Goal 8: Lifecycle Expansion And Regex Authority Retirement

Recommended objective:

```text
/goal Implement Goal 8: expand update/remove/trigger coverage, add golden behavior fixtures, and retire regex analysis as the authoritative conversion mechanism once adapter parity covers the preview corpus. Stop when supported fixture packages match native observable behavior and unsupported packages fail before mutation with specific reasons.
```

Files likely touched:

- `crates/conary-core/src/ccs/convert/`
- `apps/conary/tests/`
- `apps/remi/tests/`
- `docs/modules/remi.md`
- `docs/conaryopedia-v2.md`

Done means:

- Golden fixtures cover no-scriptlet, user/group, systemd, tmpfiles/cache, alternatives, unknown legacy replay, blocked package-manager recursion, foreign replay rejection, RPM trigger quarantine, DEB trigger quarantine, and Arch `.INSTALL` wrapper replay.
- Regex analyzer is no longer the authority for publishing `replaced` decisions.
- Public docs describe scriptlet fidelity truthfully.

Verification:

```bash
cargo test -p conary-core conversion_integration
cargo test -p conary bundle_replay
cargo test -p remi conversion
cargo run -p conary-test -- list
cargo fmt --check
git diff --check
```

## Cross-Goal Rules

- Do not widen public claims until evidence exists in the current repo.
- Do not serve partial conversions as ready CCS packages.
- Do not make cross-distro raw replay the default.
- Do not remove existing compatibility paths until the replacement has tests and a migration story.
- Keep each goal small enough to review, commit, merge, push, and clean up before starting the next.
