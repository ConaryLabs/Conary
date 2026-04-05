---
last_updated: 2026-04-04
revision: 1
summary: Phase 2 kickoff inventory after the 12-chunk simplification rollout
---

# Phase 2 Kickoff Inventory

## Baseline

- `main` baseline for this pass: `5f1892c7`
- Phase 2 branch: `simplify/cross-chunk`
- Phase 1 status: all 12 chunks merged before this branch was cut
- `DEFERRED-PH2` notes found in `main`: none

This means Phase 2 starts from current merged surface area rather than a
carry-forward backlog from Phase 1 merge commits.

## Initial Findings

### `lib.rs` export surface

`crates/conary-core/src/lib.rs` still exposes a broad re-export surface. A
first pass over the re-exported names found a set of items with zero external
Rust callers outside `crates/conary-core/src/`.

These are only **provisional** candidates. Before any removal, they still need:

1. doc-example and doctest checks
2. non-textual liveness checks
3. confirmation that the re-export is not part of an intended external API

Provisional unused re-export groups:

- `automation`: `ActionDecision`, `ActionStatus`, `AiSuggestion`, `PendingAction`
- `model::parser`: `AiAssistConfig`, `AiAssistMode`, `AiFeature`, `AutomationMode`, `FederationTier`, `RepairAutomation`, `RollbackTrigger`, `SecurityAutomation`
- `progress`: `CallbackProgress`, `LogProgress`, `ProgressEvent`, `ProgressTracker`, `SilentProgress`
- `provenance`: `BuildDependency`, `BuildProvenance`, `ComponentHash`, `ContentProvenance`, `DnaHash`, `HostAttestation`, `PackageDna`, `PatchInfo`, `ReproducibilityInfo`, `SignatureProvenance`, `SignatureScope`, `SourceProvenance`, `TransparencyLog`
- `capability`: `CapabilityError`, `EnforcementError`, `EnforcementReport`, `EnforcementSupport`, `EnforcementWarning`, `FilesystemCapabilities`, `NetworkCapabilities`, `SyscallProfile`
- `flavor` / `hash` / `label` / `bootstrap`: `ArchSpec`, `FlavorItem`, `FlavorOp`, `FlavorSpec`, `SystemFlavor`, `HashAlgorithm`, `LabelParseError`, `LabelPath`, `StageManager`, `ToolchainKind`
- `model` / `recipe` / `transaction` / `trust`: `ModelConfig`, `ModelError`, `CookResult`, `TransactionPlan`, `TransactionState`, `TrustError`

The likely Phase 2 win here is trimming unused `pub use` re-exports, not
removing the underlying module types unless whole-codebase checks support it.

### Sparse public items in recently simplified modules

The recently split support/system modules were scanned for `pub` and
`pub(crate)` items with very low textual caller counts. Most of the obvious
helpers are still live and should stay in place:

- `transaction::recovery::EROFS_MAGIC` is referenced from `transaction::mod`
- `transaction::is_valid_erofs_image` is re-exported and covered by tests
- `trigger::handler_exists_in_root` is re-exported from `trigger::mod`
- `self_update::LatestVersionInfo`, `VersionCheckResult`, `validate_download_origin`, and `is_newer` all have current callers
- `container::ScriptRisk`, `ScriptAnalysis`, and `analyze_script` all have current callers

The lower-traffic items worth a closer Phase 2 check are:

- `generation::builder::rebuild_generation_image`
  Current textual callers: definition plus recovery path use
- `generation::metadata::GENERATION_PENDING_MARKER`
  Current textual callers: definition plus local helper use
- `generation::metadata::running_kernel_version`
  Current textual callers: definition plus builder helper use
- `generation::mount::is_generation_mounted`
  Current textual callers: definition plus recovery path use

These do not look removable yet, but they are good candidates for checking
whether `pub` visibility is broader than necessary.

### `pub mod` cleanup

No `DEFERRED-PH2` notes were recorded during Phase 1, so there is no explicit
backlog of dead `pub mod` lines from `crates/conary-core/src/lib.rs`.

That means `pub mod` cleanup should stay conservative unless a later whole-tree
search shows a clearly dead module with no textual or non-textual liveness.

## Recommended Next Steps

1. Validate the provisional `lib.rs` re-export candidates against doc examples,
   doctests, and any intended external API promises.
2. Check whether the sparse generation helpers should become `pub(crate)` or
   private before considering any true deletion.
3. Batch only a few re-export removals at a time, then run:
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test --doc --workspace`
4. Start `cross-crate-duplication-findings.md` only after the export cleanup
   candidate set is stable, so the duplication pass does not get mixed up with
   dead-surface trimming.
