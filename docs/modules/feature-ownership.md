---
last_updated: 2026-08-28
revision: 81
summary: Route feature ownership through shared compiler caching with isolated build targets, hosted CI bootstrap, typed profile tiers and complete universes, focused corpus coverage, package transactions, database rebuilds, generation recovery, source identity, Remi, lifecycle, release, and canonical docs
---

# Feature Ownership And Interaction Gates

This map helps contributors and agents choose one Conary capability, start in
the right files, and know when a narrow edit needs broader verification. It does
not replace `docs/ARCHITECTURE.md`, subsystem docs, or tests.

## How To Use This Map

- Pick the capability that matches the change.
- Read the `Start here` files before editing.
- Use the focused proof for small local edits.
- Use the interaction gate when behavior crosses a listed neighbor system.
- Update the named docs when a "look here first" path, public behavior, or proof
  command changes.

Public package and repository support claims stay limited to Fedora 44,
Ubuntu 26.04, and Arch.

## Card Schema

Each ownership card uses these fields:

- **Slug:** short unique kebab-case identifier; the first field of each card,
  used by `scripts/agent-context.sh` to select cards.
- **Capability:** what user-facing or contributor-facing job this area owns.
- **Start here:** owner files and canonical docs to read first.
- **Neighbor systems:** nearby systems that often need verification when
  behavior changes.
- **Paths:** semicolon-separated, backtick-quoted glob patterns that route
  repository paths to this card. Globs match shell-style over repo-relative
  paths (`*` may span `/`); the most specific match wins, where specificity is
  the length of the literal prefix before the first `*`, `?`, or `[`. Two
  cards matching a path at equal specificity is a validation error
  (`scripts/agent-context.sh --validate`).
- **Focused proof:** narrow command for small edits.
- **Interaction gate:** broader command when the change crosses a boundary.
- **Docs to update:** docs that should move with the feature.
- **Safety notes:** persisted-state, trust, host mutation, fixture,
  private-path, or distro-scope boundaries.

## Persisted Database Model Authority

**Slug:** database-state

**Capability:** keep current-schema identity, rebuilds, model enums, and tagged
values exact, fallible on read, constrained on write, and bound to the row that
supplied them.

**Start here:** `crates/conary-core/src/db/schema.rs`;
`crates/conary-core/src/db/rebuild.rs`;
`apps/conary/src/commands/system/rebuild_database.rs`;
`crates/conary-core/src/db/current_schema/`;
`crates/conary-core/src/db/models/persisted_value.rs`;
`crates/conary-core/src/db/models/trove/identity.rs`;
`crates/conary-core/src/db/models/provide_entry.rs`;
`crates/conary-core/src/db/models/trigger.rs`;
`crates/conary-core/src/db/models/trigger_engine.rs`;
`crates/conary-core/src/db/models/derived.rs`;
`docs/ARCHITECTURE.md`.

**Neighbor systems:** CLI dispatch and destructive-mutation confirmation,
install trigger execution, derived-package CLI and model application, database
backup/restore, Remi deployment/readiness, and every current-schema SQL owner
that persists a typed state.

**Paths:** `crates/conary-core/src/db/schema.rs`;
`crates/conary-core/src/db/rebuild.rs`;
`apps/conary/src/commands/system/rebuild_database.rs`;
`crates/conary-core/src/db/models/mod.rs`;
`crates/conary-core/src/db/models/persisted_value.rs`;
`crates/conary-core/src/db/models/trove/identity.rs`;
`crates/conary-core/src/db/models/trigger.rs`;
`crates/conary-core/src/db/models/trigger_engine.rs`;
`crates/conary-core/src/db/models/derived.rs`.

**Focused proof:** `cargo test -p conary-core --lib db::schema`;
`cargo test -p conary-core --lib db::rebuild`;
`cargo test -p conary-core --lib db::models::trove::tests`;
`cargo test -p conary-core --lib db::models::provide_entry::tests`;
`cargo test -p conary-core --lib db::models::trigger`;
`cargo test -p conary-core --lib db::models::derived`;
`cargo test -p conary --lib commands::system`;
`cargo test -p conary --test live_host_mutation_safety`.

**Interaction gate:** `cargo test -p conary-core`;
`cargo test -p conary --test features` when derived-package behavior changes.

**Docs to update:** `docs/ARCHITECTURE.md` when the database contract changes;
the owning specification when a persisted product contract changes;
`docs/modules/feature-ownership.md` when model ownership or proof moves.

**Safety notes:** persisted values never select a semantic default after a
parse failure. Current-schema shape changes increment `SCHEMA_VERSION`, reject
older databases, and state the authoritative rebuild path; do not add a
migration, fallback reader, or compatibility alias. A destructive rebuild
requires explicit discard intent and confirmation, preserves a durable retired
snapshot first, and never infers installed state from generation artifacts.

## CLI Dispatch And Command Routing

**Slug:** dispatch

**Capability:** route parsed CLI command variants to command implementations
while preserving live-mutation labels, dry-run bypasses, command risk checks,
and top-level command UX.

**Start here:** `apps/conary/src/dispatch.rs`;
`apps/conary/src/dispatch/root.rs`;
`apps/conary/src/dispatch/root/try_preflight.rs`;
`apps/conary/src/dispatch/context.rs`;
`apps/conary/src/dispatch/`; `apps/conary/src/cli/`;
`apps/conary/src/command_risk.rs`; `apps/conary/src/live_host_safety.rs`.

**Neighbor systems:** command implementation modules under
`apps/conary/src/commands/`, Clap command definitions under
`apps/conary/src/cli/`, conaryd package-job compatibility, and integration
tests that exercise CLI surfaces.

**Paths:** `apps/conary/src/dispatch.rs`;
`apps/conary/src/dispatch/*`; `apps/conary/src/cli/*`;
`apps/conary/src/command_risk.rs`; `apps/conary/src/command_risk/*`;
`apps/conary/src/live_host_safety.rs`.

**Focused proof:** `cargo check -p conary`;
`cargo test -p conary --lib cli::tests`;
`cargo test -p conary --test live_host_mutation_safety`;
`cargo run -p conary -- system completions bash >/dev/null`.

**Interaction gate:** `cargo test -p conary --test query`;
`cargo test -p conary --test query_scripts`;
`cargo test -p conary --test cli_daily_ux`;
`cargo test -p conary --lib commands::model` when routing crosses query,
completion, UX, model, or live-mutation behavior.

**Docs to update:** `docs/ARCHITECTURE.md`;
`docs/llms/subsystem-map.md`; `docs/modules/feature-ownership.md`;
`docs/modules/query.md` when query or SBOM routing paths move.

**Safety notes:** keep `command_risk::enforce_cli_policy` ahead of command
routing, preserve `require_live_mutation` labels/classes/dry-run arguments
exactly, and do not add new command surfaces without matching CLI and dispatch
proof.

## Native Package Install, Update, Remove, And Live-Root Mutation

**Slug:** install

**Capability:** install, update, remove, restore, batch, scriptlet, and live-root
mutation flows for local package operations.

**Start here:** `apps/conary/src/commands/install/mod.rs`;
`apps/conary/src/commands/install/` for child modules;
`apps/conary/src/commands/install/command.rs`;
`apps/conary/src/commands/install/acquire.rs`;
`apps/conary/src/commands/install/ccs_transaction.rs`;
`apps/conary/src/commands/install/conversion.rs`;
`apps/conary/src/commands/install/conversion/tests/`;
`apps/conary/src/commands/install/ownership_mode.rs`;
`apps/conary/src/commands/install/dep_resolution.rs`;
`apps/conary/src/commands/install/validation.rs`;
`apps/conary/src/commands/install/dependencies.rs`;
`apps/conary/src/commands/install/execute.rs`;
`apps/conary/src/commands/install/lifecycle.rs`;
`apps/conary/src/commands/install/transaction.rs`;
`apps/conary/src/commands/install/options.rs`;
`apps/conary/src/commands/install/semantics.rs`;
`apps/conary/src/commands/install/source_policy.rs`;
`apps/conary/src/commands/install/native_lifecycle.rs`;
`apps/conary/src/commands/install/native_events.rs`;
`apps/conary/src/commands/install/native_events/preflight.rs`;
`apps/conary/src/commands/install/native_events/transaction_state.rs`;
`apps/conary/src/commands/install/native_events/debian_runtime.rs`;
`apps/conary/src/commands/install/native_events/debian_runtime/admin_projection.rs`;
`apps/conary/src/commands/install/native_events/debian_runtime/alternatives_state.rs`;
`apps/conary/src/commands/install/native_events/debian_runtime/trigger_mutations.rs`;
`apps/conary/src/commands/install/ccs_removal_hooks.rs`;
`apps/conary/src/commands/install/inner.rs`;
`apps/conary/src/commands/install/rollback_snapshot.rs`;
`apps/conary/src/commands/install/shared_directory.rs`;
`apps/conary/src/commands/install/config_files.rs`;
`apps/conary/src/commands/install/config_files/tests.rs`;
`apps/conary/src/commands/install/transaction/selected_root.rs`;
`apps/conary/src/commands/install/batch.rs`;
`apps/conary/src/commands/install/batch/config.rs`;
`apps/conary/src/commands/install/batch/execution.rs`;
`apps/conary/src/commands/install/batch/promises.rs`;
`apps/conary/src/commands/install/batch/witness_universe.rs`;
`apps/conary/src/commands/generation/selected_root.rs`;
`apps/conary/src/commands/generation/config_transaction.rs`;
`apps/conary/src/commands/generation/publication.rs`;
`crates/conary-core/src/config_transaction.rs`;
`crates/conary-core/src/db/current_schema/sql/package_manager.sql`;
`crates/conary-core/src/db/models/trove.rs`;
`crates/conary-core/src/db/models/provide_entry.rs`;
`crates/conary-core/src/packages/installed_identity.rs`;
`crates/conary-core/src/repository/versioning.rs`;
`apps/conary/src/commands/install/prepare.rs`;
`apps/conary/src/commands/install/resolve.rs`;
`apps/conary/src/commands/install/restore.rs`;
`apps/conary/src/commands/install/restore/`;
`apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/package.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/update/adopted_authority.rs`;
`apps/conary/src/commands/update/collection.rs`;
`apps/conary/src/commands/update/pinning.rs`;
`apps/conary/src/commands/update/delta_stats.rs`;
`apps/conary/src/commands/remove.rs`;
`apps/conary/src/commands/remove/command.rs`;
`apps/conary/src/commands/remove/autoremove.rs`;
`apps/conary/src/commands/remove/transaction.rs`;
`apps/conary/src/commands/remove/ccs_hook.rs`;
`apps/conary/src/commands/remove/native_graph.rs`;
`apps/conary/src/commands/remove/payload_ownership.rs`;
`apps/conary/src/commands/remove/types.rs`;
`apps/conary/src/commands/installed_authority_snapshot.rs`;
`apps/conary/src/commands/installed_authority_snapshot/`;
`apps/conary/src/commands/rollback_system_authority.rs`;
`apps/conary/src/commands/rollback_system_authority/`;
`apps/conary/src/commands/system.rs`;
`apps/conary/src/commands/system/rollback_command.rs`;
`apps/conary/src/commands/system/rollback_restore.rs`;
`apps/conary/src/commands/system/rollback_restore/`;
`crates/conary-core/src/transaction/mod.rs`;
`crates/conary-core/src/db/models/changeset.rs`;
`crates/conary-core/src/db/models/payload_claim.rs`;
`crates/conary-core/src/db/models/payload_claim/`;
`crates/conary-core/src/db/models/package_payload_ownership.rs`;
`crates/conary-core/src/db/models/package_payload_ownership/`;
`crates/conary-core/src/filesystem/selected_root.rs`;
`apps/conary/src/commands/live_root.rs`;
`apps/conary/src/commands/live_root/recovery.rs`;
`docs/specs/foreign-package-lifecycle-contracts.md`;
`docs/modules/test-fixtures.md`; `docs/operations/daily-driver-ux-matrix.md`.

**Neighbor systems:** `crates/conary-core/src/transaction/`;
`crates/conary-core/src/db/`; `crates/conary-core/src/scriptlet/mod.rs`;
`crates/conary-core/src/scriptlet/executor.rs`;
`crates/conary-core/src/scriptlet/sandbox.rs`;
`crates/conary-core/src/scriptlet/process.rs`;
`crates/conary-core/src/scriptlet/boundary.rs`;
`crates/conary-core/src/scriptlet/sysusers.rs`;
`crates/conary-core/src/scriptlet/native_lifecycle.rs`;
`crates/conary-core/src/scriptlet/native_lifecycle/contracts.rs`;
`crates/conary-core/src/db/models/installed_ccs_remove_hook.rs`;
`crates/conary-core/src/ccs/native_lifecycle.rs`;
`crates/conary-core/src/ccs/native_transaction.rs`;
`apps/conary/src/commands/state.rs`;
`apps/conary/src/commands/provenance.rs`; conaryd package jobs.

**Paths:** `apps/conary/src/commands/install/*`;
`apps/conary/src/commands/update/*`;
`apps/conary/src/commands/remove.rs`;
`apps/conary/src/commands/remove/*`;
`apps/conary/src/commands/installed_authority_snapshot.rs`;
`apps/conary/src/commands/installed_authority_snapshot/*`;
`apps/conary/src/commands/rollback_system_authority.rs`;
`apps/conary/src/commands/rollback_system_authority/*`;
`apps/conary/src/commands/system/rollback_command.rs`;
`apps/conary/src/commands/system/rollback_restore.rs`;
`apps/conary/src/commands/system/rollback_restore/*`;
`apps/conary/src/commands/system/tests/rollback.rs`;
`apps/conary/src/commands/system/tests/rollback/*`;
`crates/conary-core/src/db/current_schema/sql/package_manager.sql`;
`crates/conary-core/src/db/current_schema/sql/payload_claims.sql`;
`crates/conary-core/src/db/models/payload_claim.rs`;
`crates/conary-core/src/db/models/payload_claim/*`;
`crates/conary-core/src/db/models/package_payload_ownership.rs`;
`crates/conary-core/src/db/models/package_payload_ownership/*`;
`crates/conary-core/src/db/models/package_transaction_staging.rs`;
`crates/conary-core/src/db/models/package_transaction_staging/*`;
`crates/conary-core/benches/package_transaction_staging.rs`;
`crates/conary-core/src/db/models/file_entry.rs`;
`crates/conary-core/src/db/models/trove.rs`;
`crates/conary-core/src/db/models/provide_entry.rs`;
`crates/conary-core/src/packages/installed_identity.rs`;
`crates/conary-core/src/repository/versioning.rs`;
`crates/conary-core/src/db/models/installed_ccs_remove_hook.rs`;
`crates/conary-core/src/db/models/installed_native_lifecycle_bundle.rs`;
`crates/conary-core/src/db/models/native_lifecycle_residual_state.rs`;
`crates/conary-core/src/db/models/changeset.rs`;
`crates/conary-core/src/transaction/mod.rs`;
`crates/conary-core/src/filesystem/selected_root.rs`;
`apps/conary/src/commands/live_root.rs`;
`apps/conary/src/commands/live_root/*`;
`apps/conary/tests/features/*`.

**Focused proof:** `cargo test -p conary --lib commands::install`;
`cargo test -p conary --lib commands::remove`;
`cargo test -p conary --lib exact_installed_authority_round_trips_and_rejects_broken_relations`;
`cargo test -p conary-core --lib db::models::payload_claim`;
`cargo test -p conary-core --lib db::models::package_payload_ownership`;
`cargo test -p conary-core --lib db::models::package_transaction_staging::tests`;
`cargo test -p conary-core --lib filesystem::selected_root`;
`cargo test -p conary-core --lib config_transaction`;
`cargo test -p conary --lib commands::generation::config_transaction`;
`cargo test -p conary --lib commands::install::rollback_snapshot::tests`;
`cargo test -p conary-core native_transaction`;
`cargo test -p conary --test live_host_mutation_safety`;
`cargo test -p conary-core native_lifecycle`.

**Interaction gate:** `cargo test -p conary --test conversion_integration`;
`cargo test -p conary --test query_scripts`; `cargo test -p conaryd daemon::routes`
when daemon package jobs are affected.

**Docs to update:** `docs/llms/subsystem-map.md`;
`docs/modules/feature-ownership.md`; `docs/modules/test-fixtures.md`;
`docs/operations/daily-driver-ux-matrix.md`.

**Safety notes:** preserve preflight-before-mutation, exact source-ABI
lifecycle argv/stdin and stage order, persisted lifecycle bundles, private-path
redaction, the old-only payload visibility boundary, and execution without a
source package manager or its database. Diagnostic shell or command
classifications must not select, suppress, or reorder lifecycle events.
The selected-root session owns the single runtime mutation lock before it
prepares database or generation authority. A debt-free current generation uses
its verified composefs artifact plus typed mutable state directly; recoverable
candidates, first-generation database state, and retained try sessions keep
explicit materialization boundaries. The lock remains held through candidate
persistence, SQLite commit, and publication. Rollback is one
serialized immediate SQLite transaction: forward mutations and compensating
rollback rows have distinct typed lineage, and applied rollback rows never
block the next effective LIFO rollback.
Installed package version schemes are mandatory typed state supplied at
construction. Parsed package identity, repository provenance, install
semantics, persisted installed provides, and exact native identity must agree;
no distro/name/version inference or post-construction placeholder replacement
may establish that authority. Every shared payload path retains one exact claim
per package; query, lifecycle, removal, derived-package, and rollback
projections must use claim-aware payload ownership rather than treating the
materialized file anchor as the only owner. A converted CCS archive retains the
validated native source format's typed payload-sharing and
directory-materialization contracts. Package payload, component, config, and
history rows load into transaction-local TEMP tables through cached statements;
typed validation establishes anchor decisions before a fixed set of canonical
reconciliation statements. Those TEMP tables are never durable authority, and
the selected-root session plus enclosing SQLite transaction remain the rollback
boundary for any validation or reconciliation failure.

## Adoption, Unadoption, And Native-Authority Handoff

**Slug:** adopt

**Capability:** preserve migration continuity for existing native
package-manager state, support explicit takeover, recover selected-generation
handoff state, provide non-destructive escape hatches, and convert an adopted
package only after exact native-artifact re-resolution and payload equivalence.
Adoption itself is not the foreign-package acquisition or cross-distro
execution path; the verified resulting CCS enters that path separately.

**Start here:** `apps/conary/src/cli/system.rs` ->
`apps/conary/src/dispatch/system.rs` ->
`apps/conary/src/commands/adopt/`;
`apps/conary/src/commands/adopt/mod.rs`;
`apps/conary/src/commands/adopt/system.rs`;
`apps/conary/src/commands/adopt/packages.rs`;
`apps/conary/src/commands/adopt/refresh.rs`;
`apps/conary/src/commands/adopt/hooks.rs`;
`apps/conary/src/commands/adopt/status.rs`;
`apps/conary/src/commands/adopt/unadopt.rs`;
`apps/conary/src/commands/adopt/native_handoff.rs`;
`apps/conary/src/commands/adopt/convert.rs` and
`apps/conary/src/commands/adopt/convert/tests/`;
`docs/modules/source-selection.md`; `docs/ARCHITECTURE.md`. Installed state
alone is never lifecycle authority.

**Neighbor systems:** `apps/conary/src/commands/update/mod.rs`;
`apps/conary/src/commands/update/package.rs`;
`apps/conary/src/commands/update/selection.rs`;
`apps/conary/src/commands/update/adopted_authority.rs`;
`apps/conary/src/commands/update/collection.rs`;
`apps/conary/src/commands/install/`; `crates/conary-core/src/repository/`;
`crates/conary-core/src/generation/`; integration manifests under
`apps/conary/tests/integration/remi/manifests/`.

**Paths:** `apps/conary/src/commands/adopt/*`.

**Focused proof:** `cargo test -p conary --lib adopt::native_handoff`;
`cargo test -p conary --lib adopt::unadopt`;
`cargo test -p conary --lib commands::adopt::convert`.

**Interaction gate:** `cargo run -p conary-test -- list`;
`cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro fedora44 --phase 3`
when selected-generation handoff behavior changes.

**Docs to update:** `docs/modules/source-selection.md`;
`docs/llms/subsystem-map.md`; `docs/INTEGRATION-TESTING.md`.

**Safety notes:** do not silently take over adopted packages or erase native
package-manager authority without an explicit takeover path. Adopted
conversion must hold the runtime mutation lock before installed authority
reads; accept only exact enrolled source/trust/stream authority; verify identity
and the complete payload before conversion; and publish the signed CCS path and
database record atomically.

## Declarative System Models And Replatform Planning

**Slug:** model

**Capability:** diff, apply, check, snapshot, publish, lock, update, and
remote-diff declarative system model files while preserving package ownership
convergence and reproducible planning behavior.

**Start here:** `apps/conary/src/commands/model.rs`;
`apps/conary/src/commands/model/context.rs`;
`apps/conary/src/commands/model/presentation.rs`;
`apps/conary/src/commands/model/diff.rs`;
`apps/conary/src/commands/model/apply.rs`;
`apps/conary/src/commands/model/apply/derived.rs`;
`apps/conary/src/commands/model/check.rs`;
`apps/conary/src/commands/model/snapshot.rs`;
`apps/conary/src/commands/model/remote_diff.rs`;
`apps/conary/src/commands/model/lock.rs`;
`apps/conary/src/commands/model/publish.rs`;
`crates/conary-core/src/model/parser.rs`;
`crates/conary-core/src/model/parser/source_policy.rs`;
`crates/conary-core/src/model/replatform.rs`;
`docs/modules/source-selection.md`.

**Neighbor systems:** install/remove execution, exact-source update selection,
repository remote include cache, derived package builds, live-host mutation
acknowledgement, and conaryd package-job mutation intent.

**Paths:** `apps/conary/src/commands/model.rs`;
`apps/conary/src/commands/model/*`;
`crates/conary-core/src/model/*`.

**Focused proof:** `cargo test -p conary --lib commands::model`.

**Interaction gate:** `cargo test -p conary model`;
`cargo test -p conary --test model_apply`;
`cargo test -p conary --test live_host_mutation_safety model` when apply
behavior or live-mutation safety changes.

**Docs to update:** `docs/modules/source-selection.md`;
`docs/llms/subsystem-map.md`; `docs/ARCHITECTURE.md`.

**Safety notes:** preserve `model check` drift exit code 2, package ownership
convergence, executable replatform planning boundaries, lockfile
reproducibility, remote include cache behavior, and refusal-before-live-mutation
gates. Declarative models do not own global repository source state.

## Repository Metadata, Requirements, And SAT Resolution

**Slug:** resolution

**Capability:** authenticate and parse native repository metadata into
source-scheme-aware relations, persist exact requirement groups, resolve
providers through the SAT solver, and carry the selected relation graph into
package transactions. Discover, preview, and transactionally take ownership of
native repository declarations as drift-detected selected-root projections.

**Start here:** `crates/conary-core/src/repository/trust.rs`;
`crates/conary-core/src/repository/declarations/`;
`docs/specs/native-repository-declarations.md`;
`docs/specs/native-repository-trust-import.md`;
`docs/specs/native-source-identity-policy.md`;
`docs/specs/native-repository-takeover.md`;
`crates/conary-core/src/repository/declarations/takeover.rs`;
`apps/conary/src/commands/repository_takeover.rs`;
`crates/conary-core/src/repository/trust/openpgp.rs`;
`crates/conary-core/src/repository/trust/openpgp/arch/`;
`crates/conary-core/src/repository/parsers/`;
`crates/conary-core/src/repository/parsers/sink.rs`;
`crates/conary-core/src/repository/parsers/fedora/metalink.rs`;
`crates/conary-core/src/repository/parsers/fedora/repomd.rs`;
`crates/conary-core/src/repository/parsers/fedora/files.rs`;
`crates/conary-core/src/repository/parsers/fedora/filelists.rs`;
`crates/conary-core/src/repository/parsers/fedora/provides.rs`;
`crates/conary-core/src/repository/sync.rs`;
`crates/conary-core/src/repository/sync/immutable_catalog.rs`;
`crates/conary-core/src/repository/sync/projection_cache.rs`;
`crates/conary-core/src/repository/download.rs`;
`crates/conary-core/src/repository/rpm_dependency.rs`;
`crates/conary-core/src/repository/requirement.rs`;
`crates/conary-core/src/repository/package_relation.rs`;
`crates/conary-core/src/repository/resolution_policy.rs`;
`crates/conary-core/src/repository/selector.rs`;
`crates/conary-core/src/repository/resolution.rs`;
`crates/conary-core/src/repository/resolution/`;
`crates/conary-core/src/resolver/requirements.rs`;
`crates/conary-core/src/resolver/provider/`;
`crates/conary-core/src/resolver/sat.rs`;
`crates/conary-core/src/resolver/sat/`;
`crates/conary-core/src/transaction/package_relations.rs`;
`crates/conary-core/src/db/models/repository/source.rs`;
`crates/conary-core/src/db/models/repository/source/`;
`crates/conary-core/src/db/models/repository/`;
`crates/conary-core/src/db/models/installed_requirement_atom.rs`;
`crates/conary-core/src/db/models/installed_requirement_group.rs`;
`docs/modules/source-selection.md`.

**Neighbor systems:** static-repository TUF trust; install/update candidate
selection; installed package state; model replatform planning; Remi repository
manifests, admin routes, and hosted feed configuration.

**Paths:** `crates/conary-core/src/repository/*`;
`crates/conary-core/tests/fixtures/repository_declarations/*`;
`crates/conary-core/tests/fixtures/rpm/*`;
`crates/conary-core/src/resolver/*`;
`crates/conary-core/src/transaction/package_relations.rs`;
`crates/conary-core/src/transaction/package_relations/*`;
`crates/conary-core/src/db/models/repository/source.rs`;
`crates/conary-core/src/db/models/repository/source/*`;
`crates/conary-core/src/db/models/repository/*`;
`crates/conary-core/src/db/models/installed_requirement_atom.rs`;
`crates/conary-core/src/db/models/installed_requirement_group.rs`;
`apps/conary/src/commands/repository_takeover.rs`.

**Focused proof:** `cargo test -p conary-core repository::trust`;
`cargo test -p conary-core repository::declarations`;
`cargo test -p conary-core repository::declarations::takeover`;
`cargo test -p conary-core repository::parsers`;
`cargo test -p conary-core repository::download`;
`cargo test -p conary-core repository::sync`;
`cargo test -p conary-core repository`;
`cargo test -p conary-core resolver`;
`cargo test -p conary-core transaction::package_relations`.

**Interaction gate:** `cargo test -p conary --lib commands::repo`;
`cargo test -p conary --lib cli::tests`;
`cargo test -p remi repository_manifest`;
`cargo test -p conary --lib commands::install`;
`cargo test -p conary --test conversion_integration`;
`cargo test -p remi conversion` when repository metadata or public conversion
output changes.

**Docs to update:** `docs/modules/source-selection.md`;
`docs/llms/subsystem-map.md`; `docs/ARCHITECTURE.md`;
`docs/specs/native-repository-declarations.md` when declaration grammar,
selected-root discovery, or its no-enrollment boundary changes;
`docs/specs/native-repository-trust-import.md` when trust-import disposition,
evidence, or selected-root key planning changes;
`docs/specs/native-repository-takeover.md` when enrollment preview, projection
ownership, drift, or rollback changes;
`docs/specs/foreign-package-lifecycle-contracts.md` when native relation
semantics change.

**Safety notes:** repository format and trust are one validated tagged
contract. Debian Release authority, RPM metadata authority, RPM package
authority, and Arch keyring/SigLevel authority stay role-separated. A missing
root, signature, strong hash, authenticated index, or required master
certification is fatal; no boolean or runtime command may bypass it. Relation
operators and version comparison come from the source package ABI; provider
selection is typed SAT input. Repository names, URLs, filenames, distro labels,
and diagnostic text must not select trust, relation semantics, or bypass the
solver.

## Generation Build, Switch, Recovery, And Export

**Slug:** generation

**Capability:** build generation artifacts, select complete generations for the
next boot, recover publication debt, collect generations and local CAS objects,
and export raw/qcow2/ISO carriers.

**Start here:** `crates/conary-core/src/generation/builder.rs`;
`crates/conary-core/src/db/backup.rs`;
`crates/conary-core/src/db/generation_backup_chain.rs`;
`crates/conary-core/src/db/generation_delta.rs`;
`crates/conary-core/src/db/generation_snapshot.rs`;
`crates/conary-core/src/generation/root_manifest.rs`;
`crates/conary-core/src/generation/root_manifest/delta.rs`;
`crates/conary-core/src/generation/root_manifest/scan.rs`;
`crates/conary-core/src/generation/root_manifest/materialize.rs`;
`crates/conary-core/src/generation/root_manifest/overlay/config_state.rs`;
`crates/conary-core/src/generation/root_manifest/composefs.rs`;
`crates/conary-core/src/generation/builder/create.rs`;
`crates/conary-core/src/generation/builder/rebuild.rs`;
`crates/conary-core/src/generation/builder/carrier_capabilities.rs`;
`crates/conary-core/src/generation/builder/boot_assets.rs`;
`crates/conary-core/src/generation/builder/initramfs.rs`;
`crates/conary-core/src/generation/builder/kernel.rs`;
`crates/conary-core/src/generation/builder/root_validation.rs`;
`crates/conary-core/src/generation/builder/runtime_inputs.rs`;
`crates/conary-core/src/generation/export.rs`;
`crates/conary-core/src/generation/export/tests.rs`;
`crates/conary-core/src/generation/artifact.rs`;
`crates/conary-core/src/generation/artifact/tests.rs`;
`crates/conary-core/src/ccs/hooks/capabilities.rs`;
`crates/conary-core/src/ccs/hooks/capabilities/filesystem_security.rs`;
`crates/conary-core/src/generation/gc.rs`;
`apps/conary/src/commands/generation/gc.rs`;
`crates/conary-core/src/boot_runtime.rs`;
`crates/conary-core/src/boot_runtime/`;
`crates/conary-core/src/activation/systemd.rs`;
`crates/conary-core/src/activation/systemd/grammar.rs`;
`crates/conary-core/src/activation/security_policy.rs`;
`crates/conary-core/src/activation/security_policy/`;
`crates/conary-core/src/scriptlet/activation_capture.rs`;
`crates/conary-core/src/scriptlet/boot_runtime_capture.rs`;
`crates/conary-core/src/config_transaction.rs`;
`crates/conary-core/src/config_transaction/`;
`crates/conary-core/src/db/models/generation_activation.rs`;
`crates/conary-core/src/db/models/generation_publication.rs`;
`crates/conary-core/src/transaction/recovery.rs`;
`apps/conary/src/commands/generation/activation_intents.rs`;
`packaging/systemd/conary-generation-activation.service`;
`apps/conary/src/commands/generation/selected_root.rs`;
`apps/conary/src/commands/generation/selected_root/config_state.rs`;
`apps/conary/src/commands/generation/config_transaction.rs`;
`apps/conary/src/commands/generation/publication.rs`;
`apps/conary/src/commands/system.rs`;
`apps/conary/src/commands/state.rs`;
`apps/conary/src/commands/provenance.rs`;
`apps/conary/src/commands/provenance/`;
`packaging/dracut/90conary/module-setup.sh`;
`packaging/dracut/90conary/conary-init.sh`;
`packaging/dracut/90conary/conary-generator.sh`;
`docs/roadmaps/development-roadmap.md`.

**Neighbor systems:** selected-root lifecycle execution, systemd and
SELinux/AppArmor provider interfaces, transaction commit, SQLite generation
state, image building, bootstrap validation, conaryd route history.

**Paths:** `crates/conary-core/src/generation/*`;
`crates/conary-core/src/db/backup.rs`;
`crates/conary-core/src/db/generation_backup_chain.rs`;
`crates/conary-core/src/db/generation_delta.rs`;
`crates/conary-core/src/db/generation_snapshot.rs`;
`crates/conary-core/benches/generation_db_snapshot.rs`;
`crates/conary-core/src/ccs/hooks/capabilities.rs`;
`crates/conary-core/src/ccs/hooks/capabilities/*`;
`crates/conary-core/src/boot_runtime.rs`;
`crates/conary-core/src/boot_runtime/*`;
`crates/conary-core/src/activation/*`;
`crates/conary-core/src/scriptlet/activation_capture.rs`;
`crates/conary-core/src/scriptlet/boot_runtime_capture.rs`;
`crates/conary-core/src/config_transaction.rs`;
`crates/conary-core/src/config_transaction/*`;
`crates/conary-core/src/db/models/generation_activation.rs`;
`crates/conary-core/src/db/models/generation_publication.rs`;
`crates/conary-core/src/transaction/recovery.rs`;
`apps/conary/src/commands/generation/*`;
`packaging/systemd/conary-generation-activation.service`;
`packaging/dracut/90conary/*`;
`apps/conary/src/commands/provenance.rs`;
`apps/conary/src/commands/provenance/*`.

**Focused proof:** `cargo test -p conary-core generation::export`;
`cargo test -p conary-core generation::builder`;
`cargo test -p conary-core --lib ccs::hooks::capabilities`;
`cargo test -p conary-core generation::gc`;
`cargo test -p conary-core --lib boot_runtime`;
`cargo test -p conary-core --lib activation`;
`cargo test -p conary-core --lib scriptlet::activation_capture`;
`cargo test -p conary-core --lib scriptlet::boot_runtime_capture`;
`cargo test -p conary-core --lib generation_activation`;
`cargo test -p conary-core --lib db::models::generation_publication`;
`cargo test -p conary-core --lib config_transaction`;
`cargo test -p conary --lib commands::generation::config_transaction`;
`cargo test -p conary --lib commands::generation::publication`;
`cargo test -p conary --lib commands::generation::activation_intents`;
`cargo test -p conary --lib commands::generation::gc`;
`cargo test -p conary-core --test db_backup`;
`cargo test -p conary-core --lib db::generation_delta`;
`cargo bench -p conary-core --bench generation_db_snapshot`;
`cargo test -p conary-core --test generation_composefs_runtime_contract`;
`cargo test -p conary --test packaged_onboarding`.

**Interaction gate:** `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`;
`cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3`
when export or boot-carrier behavior changes. Both suites now stand on the
supported-host fixture in
`apps/conary/tests/fixtures/supported-host-generation-export/`: they assemble a
Fedora 44 root from ordinary repository installs on a scratch disk, publish it
with `conary system generation publish`, export it, and boot the artifact under
UEFI. Bootable-image export is therefore a generation capability with no
bootstrap dependency. The gate needs a boot lane: any host with `/dev/kvm` and
OVMF firmware, currently remi-dev.

**Docs to update:** `docs/ARCHITECTURE.md`;
`docs/roadmaps/development-roadmap.md`;
`docs/INTEGRATION-TESTING.md`; `docs/SCRIPTLET_SECURITY.md`;
`docs/specs/foreign-package-lifecycle-contracts.md`;
`docs/llms/subsystem-map.md`.

**Safety notes:** generation state and artifact formats are persisted behavior;
schema or format changes require explicit compatibility decisions. Runtime
generation GC resolves and validates surviving generation manifests,
recoverable publication snapshots, the complete unreversed rollback stack,
current installed/config/derived roots, current converted/public native chunk
authority, and seed images before any deletion; every live digest must exist
in the local CAS. `conary system generation gc` is the sole GC surface and
runs under the canonical mutation lock. Runtime
Selected-root mutation reads are serialized by the lock owned in
`apps/conary/src/commands/generation/selected_root.rs`; lower authority may not
be prepared or mounted before that lock is held, and it remains held until the
matching database state and publication outcome are durable. Runtime
lifecycle work is consumed only for the single generation proven by the kernel
command line, matching artifact, and database state; skipped generations must
carry forward unapplied requests. Booted-generation host-interface drift is
re-probed through the typed capability contract and persisted automatically;
it is not an operator reconciliation queue. Captured security-policy requests
are bound to the selected root's exact invoked/canonical provider paths and
SHA-256; boot applies their exact argv only after identity verification, and
missing, changed, or failing providers remain durable automatic retries.

## CCS Authoring, Conversion, Install, And Native Lifecycle Execution

**Slug:** ccs

**Capability:** build native CCS packages, convert RPM, Debian, and Arch
packages, install CCS packages, and preserve their exact lifecycle ABIs.

**Start here:** `crates/conary-core/src/ccs/`;
`crates/conary-core/src/payload.rs`;
`crates/conary-core/src/ccs/budget.rs`;
`crates/conary-core/src/ccs/v3/`;
`crates/conary-core/src/ccs/v3/authoring.rs`;
`crates/conary-core/src/ccs/v3/component_view.rs`;
`crates/conary-core/src/ccs/v3/debug_projection.rs`;
`crates/conary-core/src/repository/supported_profiles/`;
`crates/conary-core/src/ccs/archive_reader.rs`;
`crates/conary-core/src/ccs/package.rs`;
`crates/conary-core/src/ccs/package/v3_projection.rs`;
`crates/conary-core/src/ccs/convert/`;
`crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`;
`crates/conary-core/src/ccs/convert/scriptlet_bundle/`;
`crates/conary-core/src/ccs/native_export/`;
`crates/conary-core/src/ccs/native_lifecycle.rs`;
`crates/conary-core/src/ccs/native_lifecycle/`;
`crates/conary-core/src/ccs/native_transaction.rs`;
`crates/conary-core/src/ccs/native_transaction/`;
`crates/conary-core/src/packages/native_abi.rs`;
`crates/conary-core/src/packages/native_scriptlet_support.rs`;
`crates/conary-core/src/packages/payload.rs`;
`crates/conary-core/src/packages/payload/`;
`crates/conary-core/src/filesystem/cas.rs`;
`crates/conary-core/src/filesystem/cas/stream.rs`;
`crates/conary-core/src/packages/rpm/payload.rs`;
`crates/conary-core/src/packages/rpm/payload/header.rs`;
`crates/conary-core/src/packages/rpm/payload/stream.rs`;
`crates/conary-core/src/packages/rpm/scriptlets.rs`;
`crates/conary-core/src/packages/rpm/scriptlets/runtime_context.rs`;
`crates/conary-core/src/packages/deb/lifecycle_helpers.rs`;
`crates/conary-core/src/packages/deb/lifecycle_helpers/`;
`crates/conary-core/src/packages/deb/native.rs`;
`crates/conary-core/src/packages/deb/triggers.rs`;
`crates/conary-core/src/packages/arch/install_script.rs`;
`crates/conary-core/src/packages/arch/alpm_hook.rs`;
`crates/conary-core/src/scriptlet/`;
`crates/conary-core/tests/native_abi.rs`;
`apps/conary/src/commands/ccs/`;
`apps/conary/src/commands/ccs/templates.rs`;
`apps/conary/src/commands/ccs/lint.rs`;
`apps/conary/src/commands/ccs/build.rs`;
`apps/conary/src/commands/ccs/test.rs`;
`apps/conary/src/commands/ccs/local_dev.rs`;
`apps/conary/src/commands/ccs/install.rs`;
`apps/conary/src/commands/ccs/install/command.rs`;
`apps/conary/src/commands/ccs/install/dependency.rs`;
`apps/conary/src/commands/ccs/install/component_selection.rs`;
`apps/conary/src/commands/ccs/install/capability_declaration.rs`;
`apps/conary/src/commands/ccs/payload_paths.rs`;
`apps/conary/src/commands/install/payload_identity.rs`;
`apps/conary/tests/rpm_named_ownership.rs`;
`packaging/ccs/`;
`docs/modules/ccs.md`;
`docs/modules/test-fixtures.md`;
`docs/specs/source-package-authority.md`;
`docs/specs/foreign-package-lifecycle-contracts.md`;
`docs/specs/static-repo-format-v1.md`.

**Neighbor systems:** install orchestration, Remi publication, repository
metadata, scriptlet sandboxing (`crates/conary-core/src/scriptlet/mod.rs`,
`crates/conary-core/src/scriptlet/executor.rs`,
`crates/conary-core/src/scriptlet/sandbox.rs`,
`crates/conary-core/src/scriptlet/process.rs`,
`crates/conary-core/src/scriptlet/boundary.rs`,
`crates/conary-core/src/scriptlet/native_lifecycle.rs`,
`crates/conary-core/src/scriptlet/native_lifecycle/contracts.rs`,
`crates/conary-core/src/scriptlet/rpm_runtime/query_format.rs`,
`crates/conary-core/src/scriptlet/rpm_runtime/query_format/`,
`crates/conary-core/src/scriptlet/rpm_runtime/macro_expand.rs`,
`crates/conary-core/src/scriptlet/rpm_runtime/macro_expand/`,
`crates/conary-core/src/scriptlet/rpm_runtime/lua.rs`,
`crates/conary-core/src/scriptlet/rpm_runtime/lua/`,
`crates/conary-core/src/scriptlet/rpm_runtime/target.rs`,
`crates/conary-core/src/scriptlet/native_command.rs`), fixture maps.

**Paths:** `crates/conary-core/src/ccs/*`;
`crates/conary-core/src/payload.rs`;
`crates/conary-core/src/packages/native_abi.rs`;
`crates/conary-core/src/packages/native_scriptlet_support.rs`;
`crates/conary-core/src/packages/payload.rs`;
`crates/conary-core/src/packages/payload/*`;
`crates/conary-core/src/packages/*`;
`crates/conary-core/src/filesystem/cas/stream.rs`;
`crates/conary-core/src/packages/rpm/*`;
`crates/conary-core/src/packages/deb/*`;
`crates/conary-core/src/packages/arch/*`;
`crates/conary-core/src/packages/eopkg/*`;
`crates/conary-core/src/scriptlet/*`;
`crates/conary-core/tests/native_abi.rs`;
`apps/conary/src/commands/ccs/*`;
`apps/conary/src/commands/install/payload_identity.rs`;
`apps/conary/tests/rpm_named_ownership.rs`;
`packaging/ccs/*`;
`docs/specs/ccs-format-v3.md`;
`docs/specs/source-package-authority.md`;
`docs/specs/foreign-package-lifecycle-contracts.md`;
`docs/specs/eopkg-source-abi.md`.

**Focused proof:** `cargo test -p conary-core ccs::budget`;
`cargo test -p conary-core ccs::v3`;
`cargo test -p conary-core ccs::archive_reader`;
`cargo test -p conary-core ccs::verify`;
`cargo test -p conary-core filesystem::cas`;
`cargo test -p conary-core packages::rpm::payload`;
`cargo test -p conary-core --lib lifecycle_helpers`;
`cargo test -p conary --test packaging_m4b`;
`cargo test -p conary --test packaging_m4e`;
`cargo test -p conary-core supported_profiles`;
`cargo test -p conary-core native_abi`;
`cargo test -p conary-core native_lifecycle`;
`cargo test -p conary-core native_transaction`;
`cargo test -p conary --test rpm_named_ownership`.

**Interaction gate:** `cargo test -p conary --test conversion_integration golden_conversion`;
`cargo test -p conary --test packaging_m4a`;
`cargo test -p conary --test packaging_m4d`;
`cargo test -p conary-core repository::static_repo::publish_gate`;
`cargo test -p remi release_upload_` when lifecycle-bearing native authority
crosses Remi publication;
`cargo test -p remi conversion` when conversion output affects public serving.

**Docs to update:** `docs/modules/ccs.md`; `docs/modules/test-fixtures.md`;
`docs/specs/source-package-authority.md`;
`docs/specs/foreign-package-lifecycle-contracts.md`;
`docs/specs/static-repo-format-v1.md`; `docs/llms/subsystem-map.md`; the primary
issue and draft pull request while the change is still in flight.

**Safety notes:** `crates/conary-core/src/ccs/budget.rs` is the sole owner of
every CCS structural and operator-resource limit. Do not add a limit constant
to a reader, writer, or archive path: add a dimension to the budget so
authoring preflight and verification stay one owner and the writer cannot emit
a package the reader refuses. Start in `crates/conary-core/src/ccs/v3/` for v3
authority, validation, diagnostics, archive reading, debug projection, and
content identity. Use `archive_reader.rs` and `package.rs` only as
version-routing/adaptation surfaces. CCS v3 package and lifecycle authority is
source-independent and must not contain a destination distro gate; transaction
planning resolves it against typed host capabilities. Debug TOML is never
install authority. `crates/conary-core/src/payload.rs` is the sole exact payload
node contract shared by native parsers, CCS, installed state, and generation
artifacts; consumers must not recreate partial file-kind or metadata
projections. Text-pattern detections are advisory. Converted-artifact serving
requires a current structurally valid lifecycle bundle and intact artifact, not
command classification or a scriptlet publication state.
The native package parser and typed transaction planner own lifecycle
selection, arguments, triggers, order, and payload visibility. Heuristics may
prioritize engineering work but cannot grant or deny compatibility,
publication, mutation, or security authority.

## Packaging, Try Sessions, And Static Repository Publishing

**Slug:** packaging

**Capability:** scaffold and materialize explicitly named package recipes,
build explicit-recipe or typed foreign-package CCS packages, try a built artifact with an
explicit keep/rollback decision, publish recipe-built CCS packages to local
static repositories, establish root trust, sync TUF-verified indexes, and
install packages only when their CCS signatures chain to active package keys
pinned by the repository.

**Start here:** `docs/specs/static-repo-format-v1.md`;
`docs/guides/first-package.md`;
`docs/modules/recipe.md`;
`docs/modules/remi.md`;
`crates/conary-core/src/recipe/scaffold.rs`;
`crates/conary-core/src/recipe/hermetic/`;
`crates/conary-core/src/recipe/kitchen/`;
`crates/conary-core/src/recipe/kitchen/package_output.rs`;
`crates/conary-core/src/derivation/pipeline.rs`;
`crates/conary-core/src/derivation/pipeline/tests.rs`;
`crates/conary-core/src/container/mod.rs`;
`crates/conary-core/src/container/execution.rs`;
`crates/conary-core/src/container/execution/root_setup.rs`;
`crates/conary-core/src/diagnostics/`;
`apps/conary/src/commands/packaging_mcp/`;
`crates/conary-core/src/db/models/try_session.rs`;
`apps/conary/src/commands/new.rs`;
`apps/conary/src/commands/publish.rs`;
`apps/conary/src/commands/publish/artifact.rs`;
`apps/conary/src/commands/cook.rs`;
`apps/conary/src/commands/cook/foreign_package.rs`;
`apps/conary/src/commands/record_mode/`;
`apps/conary/src/commands/diagnostics.rs`;
`apps/conary/src/commands/operation_records.rs`;
`apps/conary/src/commands/hermetic_config.rs`;
`apps/conary/src/commands/hermetic_state.rs`;
`apps/conary/src/commands/try_session/`;
`apps/conary/src/commands/try_session/session.rs`;
`apps/conary/src/commands/try_session/session/watch_marker.rs`;
`apps/conary/src/commands/try_session/watch.rs`;
`apps/conary/src/commands/try_session/watch_source.rs`;
`apps/conary/src/commands/repo_static.rs`;
`crates/conary-core/src/db/current_schema/sql/repository.sql`;
`crates/conary-core/src/recipe/recording/`;
`apps/conary/tests/packaging_m1b.rs`;
`apps/conary/tests/packaging_m2a.rs`;
`apps/conary/tests/packaging_m3a.rs`;
`apps/conary/tests/packaging_m3c.rs`;
`apps/conary/tests/packaging_m3b.rs`;
`apps/conary/tests/packaging_m3d.rs`;
`crates/conary-agent-contract/src/{resource,catalog,result}.rs`;
`crates/conary-mcp/src/`;
`crates/conary-core/src/ccs/attestation.rs`;
`crates/conary-core/src/repository/static_repo/`;
`crates/conary-core/src/trust/`;
`crates/conary-core/src/ccs/signing.rs`.

**Neighbor systems:** CLI command routing and command-risk labels, exact recipe
selection/materialization, recipe Kitchen source fetching and provenance, try
session SQLite state, generation building/current-generation selection, install
acquisition and static package signature policy, repository sync
orchestration, CCS signing/verification, TUF metadata verification, and
documentation-truth and owning-card proof.

**Paths:** `docs/specs/static-repo-format-v1.md`;
`docs/guides/first-package.md`; `crates/conary-core/src/recipe/*`;
`crates/conary-core/src/diagnostics/*`;
`apps/conary/src/commands/packaging_mcp/*`;
`crates/conary-core/src/db/models/try_session.rs`;
`apps/conary/src/commands/new.rs`; `apps/conary/src/commands/publish.rs`;
`apps/conary/src/commands/publish/artifact.rs`;
`apps/conary/src/commands/cook.rs`; `apps/conary/src/commands/cook/*`;
`crates/conary-core/src/derivation/pipeline.rs`;
`crates/conary-core/src/derivation/pipeline/*`;
`apps/conary/src/commands/record_mode/*`;
`apps/conary/src/commands/diagnostics.rs`;
`apps/conary/src/commands/operation_records.rs`;
`apps/conary/src/commands/hermetic_config.rs`;
`apps/conary/src/commands/hermetic_state.rs`;
`apps/conary/src/commands/try_session/*`;
`apps/conary/src/commands/repo_static.rs`;
`apps/conary/src/commands/repo_static/*`;
`apps/conary/tests/packaging_m*.rs`;
`crates/conary-core/src/db/current_schema/sql/repository.sql`;
`crates/conary-core/src/ccs/attestation.rs`;
`crates/conary-core/src/ccs/signing.rs`;
`crates/conary-core/src/repository/static_repo/*`;
`crates/conary-core/src/filesystem/cas/*`;
`crates/conary-core/src/trust/*`; `crates/conary-core/src/container/*`.

**Focused proof:** `cargo test -p conary-core repository::static_repo`;
`cargo test -p conary-core recipe::hermetic`;
`cargo test -p conary-core recipe::kitchen`;
`cargo test -p conary-core trust::client`;
`cargo test -p conary-core trust::verify`;
`cargo test -p conary-core db::models::try_session`;
`cargo test -p conary --test static_repo_m1a`;
`cargo test -p conary --test packaging_m2a`;
`cargo test -p conary --test packaging_m3a`;
`cargo test -p conary --test packaging_m3b`;
`cargo test -p conary --lib commands::record_mode`;
`cargo test -p conary-core recipe::recording`;
`cargo test -p conary --test packaging_m3d`;
`cargo test -p conary commands::diagnostics::tests`;
`cargo test -p conary commands::packaging_mcp`;
`cargo test -p conary --lib commands::try_session`;
`cargo test -p conary --lib dispatch::root`;
`cargo test -p conary --test packaging_m1b`;
`cargo test -p conary --test packaging_m3c`.

**Interaction gate:** `cargo test -p conary-core`;
`cargo test -p conary`;
`cargo run -p conary-test -- list`;
`cargo clippy --workspace --all-targets -- -D warnings` when changes cross
publish, trust establishment, sync, install, or package-signing boundaries.

**Docs to update:** `docs/specs/static-repo-format-v1.md`;
`docs/guides/first-package.md`;
`docs/modules/recipe.md`; `docs/modules/remi.md`;
`docs/ARCHITECTURE.md`; `docs/llms/subsystem-map.md`;
`docs/modules/feature-ownership.md`.

**Safety notes:** `conary new` requires an explicit package name, `conary cook`
accepts only a recipe file or directory containing `recipe.toml`, and
`conary try --watch` requires the same explicit recipe/project boundary. No
inference compatibility aliases or explanation surface exist. A direct package
try requires `--policy` and verifies current signed CCS authority before opening
a session; watch mode derives trust from its required cook key. The accepted
signer is persisted with the session so refresh and keep can reverify copied
bytes without self-trust or a policy-path dependency. Try rollback/keep decisions
operate on the selected database/runtime and must preserve the one-active-session
invariant. After M2a, `conary cook --isolated` and project-form
`conary publish <target>` must use hermetic Kitchen execution before emitting
`hardening_level = "hermetic"` and a signed build-attestation envelope.
Artifact-form `conary publish <pkg.ccs> <target>` must pass
`publish_gate.rs` checks for package signatures, TOML integrity, attestation
authority, output identity, command-risk evidence, and foreign-boundary hashes
before static publication or Remi release upload. The artifact-form CLI and
service boundary lives in `apps/conary/src/commands/publish/artifact.rs`; exact
CLI/MCP destination parsing lives in `apps/conary/src/commands/publish/target.rs`.
Destination inspection and attestation preparation live in
`publish_context.rs`; key staging, recovery, promotion, and package-key
projection live in `publish_context/key_management.rs`. Never parse static `index.json` or
`keys/package-keys.json` before TUF target length/hash verification succeeds;
do not allow `--allow-unsigned` to bypass static repository package signature
checks; keep static repo GPG and TUF trust surfaces separate. Static package
entries must carry typed provides and authoritative requirement expression
groups; string dependency lists do not drive resolution. Project capability
kinds from the verified package contract, never from name or path shape, and
keep positive requirements distinct from typed negative/replacement relations.
Reject source selectors the static schema cannot represent. Retired package
keys are audit/history only.
Recorded-draft recipes must keep refusing publication until validated —
`publish_context.rs` and `publish_gate.rs` enforce that refusal — and Remi
release uploads stay behind the trusted build-attestation signer policy.

- Record-mode spike: start in `apps/conary/src/commands/record_mode/`, keep
  `apps/conary/src/commands/cook.rs` as a thin router/validator helper, and put
  reusable DTO/draft helpers under `crates/conary-core/src/recipe/recording/`.
- Focused proof: `cargo test -p conary --lib commands::record_mode`,
  `cargo test -p conary-core recipe::recording`, and
  `cargo test -p conary --test packaging_m3d`.

### M3a Packaging Diagnostics

Start with `crates/conary-core/src/diagnostics/` for the shared diagnostic,
event, redaction, and JSON schema contract. CLI rendering and operation-record
glue live in `apps/conary/src/commands/diagnostics.rs`; command-specific report
construction stays in `cook.rs` and `publish.rs`.

### M3b Packaging MCP

Start with `apps/conary/src/commands/packaging_mcp/` for local stdio MCP tools,
agent projection, publish plan registry, and read-only operation-record/project
inspection. Transport-neutral resource and catalog vocabulary lives in
`crates/conary-agent-contract/src/{resource,catalog,result}.rs`; generic MCP
helpers live in `crates/conary-mcp/src/`. Publish mutations remain owned by
`apps/conary/src/commands/publish.rs`, with artifact-form execution in
`apps/conary/src/commands/publish/artifact.rs`.

### M3c Try Watch Mode

Start with `apps/conary/src/commands/try_session/watch.rs` for watch lifecycle,
event streaming, refresh retry behavior, and cancellation. Session
start/refresh/keep/rollback orchestration lives in
`apps/conary/src/commands/try_session/session.rs`; signed package verification
and capability construction live in
`apps/conary/src/commands/try_session/package_verification.rs`; durable watch-created
identity lives in `session/watch_marker.rs`, and session tests live in
`session/tests.rs`. Source-set discovery, identity hashing, and debounce live in
`apps/conary/src/commands/try_session/watch_source.rs`; staged generation
refresh remains behind the try-session API in `session.rs` and namespace
switching helpers in `namespace.rs`.

## Canonical Package Map Authority

**Slug:** canonical-map

**Capability:** own exact cross-profile package equivalence, the local
versioned mapping contract, canonical persistence authority, and Remi's
versioned canonical-map exchange.

**Start here:** `crates/conary-core/src/canonical/exchange.rs`;
`crates/conary-core/src/canonical/rules.rs`;
`crates/conary-core/src/db/models/canonical.rs`;
`apps/remi/src/server/canonical_job.rs`;
`apps/remi/src/server/handlers/canonical.rs`;
`crates/conary-core/src/repository/sync/remi.rs`;
`docs/modules/source-selection.md`; `docs/modules/remi.md`.

**Neighbor systems:** repository feed profiles, exact-source request scope,
Remi repository sync, AppStream and Repology discovery caches, resolver
canonical expansion, and current-schema rebuilds.

**Paths:** `crates/conary-core/src/canonical/*`;
`crates/conary-core/src/db/models/canonical.rs`;
`crates/conary-core/src/repository/sync/remi.rs`;
`apps/remi/src/server/canonical_job.rs`;
`apps/remi/src/server/handlers/canonical.rs`;
`data/canonical-rules/*`.

**Focused proof:** `cargo test -p conary-core canonical`;
`cargo test -p remi canonical_job`;
`cargo test -p remi handlers::canonical::tests`.

**Interaction gate:** `cargo test -p conary-core repository::sync`;
`cargo test -p remi` when the HTTP contract, Remi rebuild, or client sync
changes.

**Docs to update:** `docs/modules/source-selection.md`; `docs/modules/remi.md`;
`docs/llms/subsystem-map.md`.

**Safety notes:** only literal versioned `Contract` mappings and
checksum-verified `Remi` snapshots create equivalence. Persist exact public
profile IDs. AppStream may enrich one already-authorized identity; Repology,
AppStream, package names, aliases, and ranking signals never create or select a
mapping. Unknown, duplicate, or conflicting authority fails before mutation,
and snapshot replacement is atomic.

## Repository Feed Profiles

**Slug:** profiles

**Capability:** own configured upstream repository feeds: public IDs,
dependency flavor, version scheme, Remi route slugs, repository hints, and
source parser selection. Feed profiles describe where packages come from; they
do not describe destination compatibility or CCS v3 lifecycle policy.

**Start here:** `crates/conary-core/src/repository/supported_profiles/`.
CLI repository commands, Remi route validation, conversion lookup/parser
dispatch, and Remi sync should delegate to that profile API instead of adding
new hard-coded feed matches. Host compatibility is owned by typed capability
inventory and transaction planning.

**Neighbor systems:** source selection, resolver version schemes, Remi serving
routes, conversion, and native release upload.

**Paths:** `crates/conary-core/src/repository/supported_profiles/*`.

**Focused proof:** `cargo test -p conary-core supported_profiles`;
`cargo test -p conary --test packaging_m4d`;
`cargo test -p conary --test packaging_m4e`; `cargo test -p remi route`;
`cargo test -p remi release_upload_`; `cargo test -p conary-core remi_sync`.

**Interaction gate:** `cargo test -p remi`;
`cargo test -p conary --test packaging_m4c`;
`cargo test -p conary --test conversion_integration golden_conversion` when
feed changes cross Remi serving routes, conversion lookup or parser dispatch,
or native release upload.

**Docs to update:** `docs/modules/source-selection.md`; `docs/modules/remi.md`;
`docs/modules/ccs.md`; `docs/modules/test-fixtures.md`;
`docs/llms/subsystem-map.md`.

**Safety notes:** configured public feed IDs are exact and narrow:
`fedora-44`, `ubuntu-26.04`, and `arch`. Their public Remi route slugs are
`fedora`, `ubuntu`, and `arch`; `solus` is an explicit candidate with no public
authority. Generic route slugs such as `fedora` and `ubuntu` are not feed IDs.
This catalog is not a list of destination distros Conary supports.

## Remi Publication, Serving, Admin, And Fixture Artifacts

**Slug:** remi

**Capability:** ingest, convert, publish, index, search, and serve CCS artifacts,
release uploads, and static test fixtures through Remi.

**Start here:** `apps/remi/src/server/release_publish.rs`;
`apps/remi/src/deployment.rs`;
`apps/remi/src/server/signing_authority.rs`;
`apps/remi/src/server/mod.rs`;
`apps/remi/src/server/admin_service.rs`;
`apps/remi/src/server/admin_service/refresh.rs`;
`apps/remi/src/server/repository_manifest.rs`;
`deploy/remi-repositories.toml`;
`crates/conary-core/src/db/current_schema/sql/remi.sql`;
`apps/remi/src/server/native_publish/`;
`apps/remi/src/server/native_publish/verify.rs`;
`apps/remi/src/server/publication.rs`;
`apps/remi/src/server/promotion_proof.rs`;
`apps/remi/src/server/promotion_evidence.rs`;
`apps/remi/src/server/conversion.rs`;
`apps/remi/src/server/conversion/types.rs`;
`apps/remi/src/server/conversion/workflow.rs`;
`apps/remi/src/server/conversion/persistence.rs`;
`apps/remi/src/server/conversion/lookup.rs`;
`apps/remi/src/server/conversion/metadata.rs`;
`apps/remi/src/server/conversion/storage.rs`;
`apps/remi/src/server/conversion/recipe.rs`;
`apps/remi/src/server/conversion/benchmark.rs`;
`apps/remi/src/server/index_gen.rs`;
`apps/remi/src/server/prewarm.rs`; `apps/remi/src/server/handlers/`;
`deploy/remi-deploy-helper.sh`;
`docs/modules/remi.md`; `docs/modules/test-fixtures.md`.

**Neighbor systems:** CCS conversion metadata, repository client behavior,
typed repository parser configuration, schema deployment transitions,
federation peer state, admin audit logs, artifact path handling, and repository
feed profiles.

**Paths:** `apps/remi/*`;
`crates/conary-core/src/db/current_schema/sql/remi.sql`;
`deploy/remi-repositories.toml`.

**Focused proof:** `cargo test -p remi release_upload_`;
`cargo test -p remi signing_authority`;
`cargo test -p remi deployment`;
`cargo test -p remi native_publish`;
`cargo test -p remi refresh`;
`cargo test -p conary --test packaging_m4c`;
`cargo test -p remi remi_release_parity`;
`cargo test -p remi conversion`;
`cargo test -p remi test_upload_fixture`;
`cargo test -p remi test_public_fixture_get_and_head`;
`bash scripts/test-remi-deploy-helper.sh`;
`bash scripts/test-remi-health.sh`.

**Interaction gate:** `cargo test -p remi`;
`cargo test -p conary --test conversion_integration golden_conversion` when
serving behavior depends on conversion output, and
`cargo test -p conary --test packaging_m4c` when native release intake,
metadata, download, structural lifecycle validation, or client install proof
changes.

**Docs to update:** `docs/modules/remi.md`; `docs/modules/test-fixtures.md`;
`docs/llms/subsystem-map.md`; operator docs when deployment behavior changes.

**Safety notes:** validate current converted-package lifecycle summaries and
never use heuristic evidence, diagnostic classifications, package names, or
provides as serving authority. Do not expose program bodies, private diagnostic
values, or unverified native package signatures through public listings. Remi
release uploads must stage privately, enforce trusted build-attestation signer
policy, validate source-independent lifecycle authority structurally, and
publish package rows, native publication rows, chunks, and TUF targets only
after the shared gate and lifecycle validation pass. Native CCS release uploads
must not create synthetic `converted_packages` rows; failed replacement must
preserve the last public native generation. Repository signing keys are owned
only by exact source-profile directories; deployment must preserve complete
existing role sets unchanged and reject partial, aliased, insecure, or
unexpected authority before activation.

## conaryd Package Jobs And Daemon Routes

**Slug:** conaryd

**Capability:** accept local daemon requests, authenticate socket access, queue
package jobs, expose job state, and stream route lifecycle events.

**Start here:** `apps/conaryd/src/daemon/mod.rs`;
`apps/conaryd/src/daemon/config.rs`;
`apps/conaryd/src/daemon/routes.rs`;
`apps/conaryd/src/daemon/routes/router.rs`;
`apps/conaryd/src/daemon/routes/auth.rs`;
`apps/conaryd/src/daemon/routes/types.rs`;
`apps/conaryd/src/daemon/routes/errors.rs`;
`apps/conaryd/src/daemon/routes/db.rs`;
`apps/conaryd/src/daemon/routes/sse.rs`;
`apps/conaryd/src/daemon/routes/transactions.rs`;
`apps/conaryd/src/daemon/routes/query.rs`;
`apps/conaryd/src/daemon/routes/system.rs`;
`apps/conaryd/src/daemon/routes/events.rs`;
`apps/conaryd/src/daemon/jobs.rs`;
`docs/modules/conaryd.md`.

**Neighbor systems:** Conary CLI package commands, SQLite `daemon_jobs` state,
operation vocabulary in `crates/conary-core/src/operations.rs`, live-host
mutation acknowledgement.

**Paths:** `apps/conaryd/*`.

**Focused proof:** `cargo test -p conaryd daemon::routes` for route behavior;
`cargo test -p conaryd daemon` for broader daemon behavior including auth, jobs,
and route lifecycle.

**Interaction gate:** `cargo test -p conary --test cli_daily_ux` when CLI
diagnostics change; `cargo test -p conaryd` for route/job behavior.

**Docs to update:** `docs/modules/conaryd.md`;
`docs/llms/subsystem-map.md`; `docs/operations/infrastructure.md` for host
workflow changes.

**Safety notes:** preserve job idempotency, queued/running restart behavior,
SSE lifecycle, socket auth, and live-host mutation boundaries.

## Bootstrap And Self-Hosting

**Slug:** bootstrap

**Capability:** validate bootstrap prerequisites, build self-hosting images,
run dry-run smoke checks, and support local QEMU validation.

**Start here:** `apps/conary/src/commands/bootstrap/mod.rs`;
`apps/conary/src/commands/bootstrap/setup.rs`;
`apps/conary/src/commands/bootstrap/phases.rs`;
`apps/conary/src/commands/bootstrap/image.rs`;
`apps/conary/src/commands/bootstrap/run.rs`;
`apps/conary/src/commands/bootstrap/run_record.rs`;
`apps/conary/src/commands/bootstrap/run_artifact.rs`;
`apps/conary/src/commands/bootstrap/seed.rs`;
`apps/conary/src/commands/bootstrap/convergence.rs`;
`apps/conary/src/commands/bootstrap/cleanup.rs`;
`apps/conary/src/commands/bootstrap/types.rs`;
`apps/conary/src/commands/bootstrap/state.rs`;
`apps/conary-test/src/bootstrap.rs`;
`crates/conary-core/src/bootstrap/image.rs`;
`crates/conary-core/src/bootstrap/image/erofs_generation.rs`;
`crates/conary-core/src/bootstrap/image/tests.rs`;
`docs/modules/bootstrap.md`;
`docs/operations/bootstrap-selfhosting-vm.md`;
`docs/roadmaps/development-roadmap.md`.

**Neighbor systems:** recipe versions, image generation, QEMU validation,
container runtime availability, ignored local artifact paths.

**Paths:** `apps/conary/src/commands/bootstrap/*`;
`apps/conary-test/src/bootstrap.rs`;
`crates/conary-core/src/bootstrap/image.rs`;
`crates/conary-core/src/bootstrap/image/*`;
`crates/conary-bootstrap/*`;
`docs/modules/bootstrap.md`;
`docs/operations/bootstrap-selfhosting-vm.md`;
`docs/roadmaps/development-roadmap.md`.

**Focused proof:** `cargo test -p conary --lib commands::bootstrap`;
`cargo test -p conary-core --lib bootstrap::image`;
`cargo test -p conary --test bootstrap_workflow`;
`cargo run -p conary-test -- bootstrap check --json`;
`cargo run -p conary-test -- bootstrap smoke --dry-run --json`.

**Interaction gate:** `cargo run -p conary-test -- bootstrap smoke --json` when
the local environment is intended to build or run the image.

**Docs to update:** `docs/modules/bootstrap.md`;
`docs/operations/bootstrap-selfhosting-vm.md`;
`docs/INTEGRATION-TESTING.md`; `docs/llms/subsystem-map.md`.

**Safety notes:** do not treat ignored local image paths, credentials, or
machine-specific artifacts as tracked repo truth. Non-dry-run bootstrap smoke
can start QEMU-backed validation and depends on local container/runtime
availability; keep dry-run smoke as the routine contributor gate unless the
task explicitly needs live image proof.

## Release Construction, Publication, Deployment, And Proof

**Slug:** release

**Capability:** synchronize one workspace version, construct four artifact
products from one exact suite tag, bind CCS and detached signatures to that
release authority, publish one immutable GitHub release, route serialized
deployment, and prove installed or live behavior independently.

**Start here:** `.github/workflows/release-build.yml`;
`.github/workflows/deploy-and-verify.yml`;
`.github/workflows/release-artifact-proof.yml`;
`.github/actions/setup-rust-workspace/action.yml`;
`scripts/ci-install-ubuntu-packages.sh`;
`scripts/check-github-action-runtimes.sh`;
`scripts/test-github-action-runtimes.sh`;
`Cargo.toml`; workspace member `Cargo.toml` manifests; `Cargo.lock`;
`scripts/release.sh`; `scripts/release-matrix.sh`;
`scripts/check-release-matrix.sh`; `scripts/test-release-matrix.sh`;
`scripts/sign-release.sh`; `crates/conary-core/examples/sign_hash.rs`;
`apps/conary/tests/release_ccs_manifest.rs`;
`docs/operations/release-artifact-matrix.md`;
`docs/operations/infrastructure.md`.

**Neighbor systems:** CCS authoring and verification, native package
construction, GitHub tags and releases, self-update serving, Remi deployment,
static-site deployment, and production health proof.

**Paths:** `.github/workflows/release-build.yml`;
`.github/workflows/deploy-and-verify.yml`;
`.github/workflows/release-artifact-proof.yml`;
`.github/actions/setup-rust-workspace/action.yml`;
`.github/actions/test-generation-db-reflink/action.yml`;
`scripts/ci-install-ubuntu-packages.sh`;
`scripts/check-github-action-runtimes.sh`;
`scripts/test-github-action-runtimes.sh`;
`Cargo.toml`; `apps/*/Cargo.toml`; `crates/*/Cargo.toml`; `Cargo.lock`;
`scripts/release.sh`; `scripts/release-matrix.sh`;
`scripts/check-release-matrix.sh`; `scripts/test-release-matrix.sh`;
`scripts/sign-release.sh`; `crates/conary-core/examples/sign_hash.rs`;
`apps/conary/tests/release_ccs_manifest.rs`;
`docs/operations/release-artifact-matrix.md`.

**Focused proof:** `bash scripts/check-release-matrix.sh`;
`bash scripts/test-release-matrix.sh`;
`bash scripts/check-github-action-runtimes.sh`;
`bash scripts/test-github-action-runtimes.sh`;
`cargo test -p conary-core --example sign_hash`;
`cargo test -p conary --test release_ccs_manifest`;
`cargo test -p conary-test container::image`.

**Interaction gate:** `bash scripts/test-remi-deploy-helper.sh`;
`bash scripts/test-deploy-sites.sh` when release routing, remote deployment, or
static-site publication changes. After a Conary release is published, the
terminal release-artifact-proof workflow must install all three native
packages and pass the Cartesian lifecycle before released-binary evidence is
claimed.

**Docs to update:** `docs/operations/release-artifact-matrix.md`;
`docs/operations/infrastructure.md`;
`docs/roadmaps/external-tester-milestone.md`;
`docs/llms/subsystem-map.md`.

**Safety notes:** published tags and releases are immutable evidence. The
workspace has one suite version and one current `vMAJOR.MINOR.PATCH` tag route;
artifact products do not own independent version baselines or Cargo-registry
publication. A live release must come from the exact canonical tag at a
reviewed commit already reachable from `main`. The active suite-tag rule must
reject updates and deletions of `v*` tags from creation onward; GitHub's
immutable-release enforcement must then lock the published tag and assets.
Dry-run artifact proof is not publication or production proof. Release signing
secrets must never be logged or persisted in artifacts. A successful workflow
dispatch is not deployment proof: wait for terminal CI, then verify installed
binaries, served artifacts, signatures, and live health independently.

## conary-test Integration Execution

**Slug:** conary-test

**Capability:** list, validate, and execute declarative integration suites,
including slow QEMU/KVM proof when release evidence needs it. `conary-test` is
a local CLI and engine; Remi owns networked test-data and MCP surfaces.

**Start here:** `apps/conary-test/src/`;
`apps/conary-test/src/suite_inventory.rs`;
`apps/conary-test/src/config/`;
`docs/INTEGRATION-TESTING.md`; `docs/modules/test-fixtures.md`.

**Neighbor systems:** package-manager CLI behavior, Remi fixture publication,
QEMU images, integration manifests, result JSON, and the native lifecycle
matrix job in `.github/workflows/pr-gate.yml`.

**Paths:** `apps/conary-test/*`;
`apps/conary/tests/fixtures/*`;
`apps/conary/tests/integration/remi/containers/*`;
`apps/conary/tests/integration/remi/manifests/*`;
`scripts/build-static-conary.sh`;
`scripts/kernel-header-roots.sh`;
`scripts/native-matrix-artifact.sh`;
`.github/actions/build-static-conary/action.yml`;
`.github/actions/restore-native-matrix-artifact/action.yml`;
`.github/workflows/pr-gate.yml`.

**Focused proof:** `cargo run -p conary-test -- list`;
`cargo test -p conary-test suite_inventory`;
`cargo test -p conary-test distro_config_requires_a_typed_build_context`;
`cargo test -p conary-test focused_native_cross_source_manifest_runs_the_shared_lifecycle_contract`;
`cargo test -p conary-test native_cross_source_`.

**Interaction gate:** `bash scripts/build-static-conary.sh`;
`cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4`;
`cargo run -p conary-test -- run --suite phase4-native-daily-driver-corpus --distro fedora44 --phase 4`;
`cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro fedora44 --phase 3`;
run `cargo run -p conary-test -- run --suite native-cross-source-lifecycle --distro <distro> --phase 4`
for each configured distro when native conversion/lifecycle behavior or image
build-context staging changes.

**Docs to update:** `docs/INTEGRATION-TESTING.md`;
`docs/modules/test-fixtures.md`; affected feature cards.

**Safety notes:** manifest TOML is persisted test configuration; schema changes
need parser proof and migration or defaulting decisions. Suite names in
`--suite` arguments use the manifest filename stem, such as
`phase4-native-pm-parity`, not the human-readable title shown by
`cargo run -p conary-test -- list`. Which Conary binary a distro image stages is
selected by the typed `build_context` field, never by matching the distro key.
The static choice fails closed rather than falling back to the host build, so an
image build requires `scripts/build-static-conary.sh` to have produced its
artifact first. The protected PR matrix builds Conary, the integration CLI,
and the one ignored container-contract test executable once as static musl
binaries. Every distro cell downloads the same immutable artifact and verifies
its exact commit, tree, lockfile, toolchain, flags, cache namespace, archive
member list, and binary digests before reopening it; an absent, corrupt, or
misattributed artifact fails the cell and never weakens a predicate. Compiler
cache entries reduce work but are not artifact or test authority. Corpus cases
must carry versioned runtime evidence with exact role-tagged artifact digests,
typed digest authority, target capabilities, and canonically ordered stage
checkpoints; report aggregation uses typed stage/failure discriminants and
never diagnostic text. Corpus suites declare one exact semantic requirement
set, and each case binds its claims to source-artifact roles that must resolve
to unique runtime SHA-256 identities. Only completed cases contribute coverage;
the declared and emitted case counts and required/covered semantic sets must
agree.

## Developer Build Environment

**Slug:** dev-build

**Capability:** share eligible compiler outputs across linked worktrees while
keeping Cargo targets isolated per worktree and cleanup explicitly bounded to
the compiler cache.

**Start here:** `scripts/dev-build.sh`; `scripts/test-dev-build.sh`;
`apps/remi/build.rs`; `apps/conary-test/build.rs`; `CONTRIBUTING.md`;
`docs/llms/README.md`.

**Neighbor systems:** Cargo and rustc invocation, `sccache`, Git linked
worktrees, agent-context proof execution, caller-provided build environments,
and local disk usage.

**Paths:** `scripts/dev-build.sh`; `scripts/test-dev-build.sh`;
`apps/remi/build.rs`; `apps/conary-test/build.rs`.

**Focused proof:** `bash scripts/test-dev-build.sh`.

**Interaction gate:** `bash scripts/test-agent-context.sh` when proof execution
changes; `bash scripts/agent-context.sh --validate` when ownership routing
changes.

**Docs to update:** `CONTRIBUTING.md`; `docs/llms/README.md`;
`docs/modules/feature-ownership.md`.

**Safety notes:** the Git common directory owns only the bounded compiler
cache; Cargo targets remain worktree-local unless the caller explicitly
selects one. Existing `RUSTC_WRAPPER`, `CARGO_TARGET_DIR`, `SCCACHE_DIR`, and
cache-size settings retain precedence. Cache cleanup requires an exact marker
and explicit confirmation, rejects broad or symlink targets, and never deletes
a Cargo target. A cache miss or compiler failure is reported once; it must not
silently rerun the compiler outside the selected cache. Build metadata may
watch only existing Git control paths; it must not permanently invalidate a
linked worktree or recursively watch the common Git directory that owns the
shared cache.

## Agent/MCP Operation Surfaces

**Slug:** agent-mcp

**Capability:** expose transport-neutral operation vocabulary and MCP adapters
for Conary and Remi automation. `conary-test` no longer owns a network or MCP
server.

**Start here:** `crates/conary-agent-contract/src/`;
`crates/conary-mcp/src/`; `apps/remi/src/server/mcp.rs`;
`docs/operations/infrastructure.md`.

**Neighbor systems:** Remi HTTP handlers, operation risk labels, resource
references, and authentication.

**Paths:** `crates/conary-agent-contract/*`;
`crates/conary-mcp/*`; `apps/remi/src/server/mcp.rs`.

**Focused proof:** `cargo test -p conary-agent-contract`;
`cargo test -p conary-mcp`.

**Interaction gate:** `cargo test -p remi` when adapter changes call service
behavior.

**Docs to update:** `docs/operations/infrastructure.md`;
`docs/llms/README.md`; `docs/llms/subsystem-map.md`.

**Safety notes:** keep `crates/conary-agent-contract` transport-neutral; MCP
code should adapt the contract rather than becoming product truth.
