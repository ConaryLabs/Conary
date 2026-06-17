# M3d Record-Mode Spike Design

**Date:** 2026-06-17
**Status:** Approved design; implementation plan next
**Parent design:** `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`
**Prerequisite milestones:** M3a diagnostics/events, M3b packaging MCP, M3c try watch mode

## Purpose

M3d is a record-mode spike. It proves whether Conary can observe a package
author's demonstration command well enough to produce a useful draft recipe,
redacted trace report, and optional capability suggestions without weakening
M2 release gates.

The spike is not the final polished "open a shell and infer everything" UX.
The first surface is hidden and explicit:

```text
conary cook --record [SOURCE_DIR] -- <build-or-install-command...>
```

`SOURCE_DIR` defaults to the current directory. The trailing command is
required. Conary runs it with `DESTDIR` and `CONARY_DESTDIR` pointing at a
private install root, records scoped filesystem activity with fanotify/inotify,
derives a draft recipe, and writes a redacted trace report.

The core invariant is:

> Record mode may help draft packaging metadata, but recorded output is never
> publishable directly. Any artifact produced through the recording path is
> stamped `origin_class = "recorded-draft"` and must still be refused by the M2
> publish gates.

## Current Repo Facts

- M3a landed the shared packaging diagnostic, event, redaction, JSON, and
  operation-record contract in `crates/conary-core/src/diagnostics/`,
  `apps/conary/src/commands/diagnostics.rs`, and
  `apps/conary/src/commands/operation_records.rs`.
- M3b landed the local packaging MCP surface as an adapter over the shared
  packaging contract.
- M3c landed `conary try --watch` using the same event and operation-record
  vocabulary.
- `apps/conary/src/commands/cook.rs`, `apps/conary/src/cli/mod.rs`, and
  `apps/conary/src/dispatch/root.rs` are already large files. M3d must add a
  record-mode owner instead of turning those files into the tracing engine.
- Existing recipe inference lives under
  `crates/conary-core/src/recipe/inference/`. It can provide package
  name/version and build-system hints, but it does not observe behavior.
- Existing capability inference lives under
  `crates/conary-core/src/capability/inference/`. It can produce advisory
  capability declarations from installed-file evidence, but it is not a trace
  collector.
- The workspace already depends on `nix`, `libc`, `walkdir`, and `tempfile`.
  M3d can use low-level Linux APIs directly where a focused wrapper is small,
  but the implementation plan must choose any new dependency deliberately.
- M2 publish gates already know `RecordedDraftArtifact` and must continue to
  refuse recorded-draft artifacts.

## Scope

In scope for M3d:

- Hidden experimental `conary cook --record` routing.
- A fanotify-first, recursive-inotify-assisted filesystem trace collector for
  scoped source, work, and install roots.
- Private record workspace setup and cleanup.
- Redacted trace report generation.
- Conservative draft recipe generation from command, inference, trace, and
  installed-file evidence.
- Optional normal cook validation of the generated draft.
- Optional advisory capability suggestions from installed files.
- Structured diagnostics, events, JSON output, and operation records using the
  existing M3 schema version 1 contract.
- Focused tests for trace truthfulness, redaction, cleanup, draft generation,
  validation, and recorded-draft publish refusal.

Out of scope for M3d:

- A public stable record-mode UX.
- An interactive shell session.
- Full syscall tracing, ptrace, seccomp-notify, or network syscall observation.
- A host-root tracer or broad `/` watch.
- A setuid helper, sudo escalation, or new daemon.
- Any relaxation of M2 hermetic, attestation, static-repository, or
  recorded-draft publish gates.
- Direct publication of a recorded draft.
- DB migrations or a persistent raw-trace store.
- Perfect dependency inference. Dependency and capability output is advisory.

## UX Contract

The spike adds hidden CLI controls:

- `--record`: enter experimental record mode.
- `--record-output <DIR>`: write public draft outputs to a chosen directory;
  defaults to `./recorded/<source-dir-name>/`.
- `--record-backend <auto|fanotify|inotify>`: choose trace backend; defaults
  to `auto`.
- `--record-validate`: run a normal cook against the generated draft recipe.
- `--keep-raw-trace`: hidden developer escape hatch for debugging only.

Normal public help should continue not to advertise `--record` until the spike
graduates. Hidden help and CLI tests may assert the experimental surface exists.

Default human output is concise:

```text
Recording with fanotify-inotify
Running command: ...
Command exited: 0
Draft recipe: recorded/foo/recipe.toml
Trace report: recorded/foo/trace-report.json
Validation: skipped | succeeded | failed
Recorded draft: not directly publishable
```

With `--json`, the command emits the existing final M3 command JSON object.
Streaming NDJSON is not required for the spike.

## Architecture

`apps/conary/src/commands/record_mode/` should own record-mode orchestration:

- CLI request validation.
- Private workspace creation.
- Backend probing and watcher lifecycle.
- Command execution.
- Raw trace collection.
- Redaction and public report writing.
- Draft recipe materialization.
- Optional validation cook.
- Cleanup and final command output.

`apps/conary/src/commands/cook.rs` remains the cook owner. It should only route
record-mode requests to the new module and expose a narrow validation helper if
needed. `apps/conary/src/cli/mod.rs` owns hidden flag definitions. Dispatch
continues to route the `Cook` command normally.

Core should receive only reusable product concepts. A focused
`crates/conary-core/src/recipe/recording/` module is appropriate for DTOs and
pure helpers such as trace report types, event classification, draft derivation,
and command rendering tests. Linux watcher implementation belongs in the CLI
module unless a later non-CLI consumer needs it.

The selected ownership boundary is:

```text
CLI cook route -> record_mode command owner -> scoped trace backend
               -> core recording DTOs/pure derivation helpers
               -> existing recipe materializer/cook validator
               -> existing diagnostics/events/operation records
```

## Trace Backend

M3d uses a backend trait so fanotify and inotify behavior can be tested without
requiring elevated CI privileges:

```text
TraceBackend
  probe(scope, requested_backend) -> TraceBackendStatus
  start(scope) -> TraceSession

TraceSession
  run_command(command_env)
  drain_events()
  finish()
```

The default `auto` backend prefers fanotify plus recursive inotify:

- Fanotify records opened/read/modified paths when the process has permission
  to mark the scoped roots.
- Inotify records creates, writes, deletes, renames, and new directories.
- New directories under watched roots are added dynamically.
- If fanotify is unavailable in `auto`, Conary may fall back to inotify-only,
  but the trace report must say read evidence is incomplete.
- If the user explicitly requests `fanotify`, missing permission or kernel
  support is a fail-closed setup error and the command does not run.
- If the user explicitly requests `inotify`, read evidence is declared
  incomplete from the start.

Trace scope is bounded to canonical roots:

- source root
- private work root
- private install root
- later optional declared extra roots, if the implementation plan decides they
  are necessary

Conary does not watch `/`, `$HOME`, `/etc`, `/var`, or arbitrary host roots by
default. Symlink escapes are not followed into new watch roots unless the target
is already inside the canonical trace scope. Events outside scope are ignored
and counted in the report as ignored or out-of-scope evidence.

Event loss is a correctness failure, not a warning hidden in logs. Fanotify
queue errors, inotify queue overflow, watch-limit exhaustion, or watcher thread
failure must produce a diagnostic. The operation may still write a redacted
partial report, but it must not claim complete trace evidence or successful
validation.

## Data Model

The public trace report is structured, redacted JSON:

```text
RecordingReport
  schema_version
  operation_id
  backend
  source_root
  command_summary
  command_exit
  observed_paths
  installed_files
  inferred_build_steps
  inferred_install_steps
  capability_suggestions
  ignored_events
  redactions
  limitations
```

Observed paths carry scope and operation class:

- `source-read`
- `source-write`
- `work-read`
- `work-write`
- `install-create`
- `install-modify`
- `install-delete`
- `out-of-scope`
- `unknown`

The report must separate observed facts from guesses. Fanotify read evidence is
observed. Inotify-only read evidence is absent and must be named as a
limitation. Capability suggestions and dependency hints are advisory, with
confidence and rationale.

The M3 packaging command output remains `PackagingCommandOutput` with
`schema_version = 1`. M3d may add:

- `PackagingPhase::RecordMode`
- diagnostics such as `RecordBackendUnavailable`, `RecordTraceFailed`,
  `RecordCommandFailed`, `RecordDraftGenerated`, `RecordValidationFailed`,
  `RecordRedactionFailed`, and `RecordCleanupFailed`
- events such as `RecordStarted`, `RecordBackendSelected`,
  `RecordCommandStarted`, `RecordCommandFinished`, `RecordTraceFinished`,
  `RecordDraftGenerated`, `RecordValidationStarted`,
  `RecordValidationFinished`, and `RecordFinished`

Names may be adjusted during implementation for consistency, but the plan must
preserve the semantic distinctions above.

## Draft Recipe Derivation

Draft generation is conservative:

- Package name and version come from existing source inference when possible.
  Otherwise use the source directory name and `0.1.0-recorded`.
- The recipe source is a local relative path under the recipe directory. Public
  recipe output must not contain absolute host paths.
- The recorded command vector is rendered with a structured shell-quoting
  helper, not ad hoc string joining.
- If the install root contains files, the recorded command becomes the draft
  install step.
- If no install files are observed, the recorded command becomes the draft
  build step and the report says no install evidence was captured.
- Installed files are evidence, not a hand-written manifest override.
- Dependencies are suggestions unless existing inference can name them
  confidently.
- Capability suggestions use existing capability inference over installed files
  where possible and are marked with confidence and rationale.
- Network evidence is reported only if a selected backend or explicit
  command/log heuristic can support it. Otherwise the report says network was
  not observed by this spike.

If `--record-validate` is set, Conary runs a normal cook against the generated
draft recipe as a separate validation step. Validation does not reuse raw trace
authority. If validation produces an artifact, the Kitchen config must stamp
`origin_class_override = "recorded-draft"`. The artifact is for inspection and
publish-refusal proof only.

## Redaction And Storage

Raw trace data is secret-bearing until proven otherwise.

Default storage rules:

- Private record workspace directory mode is `0700`.
- Raw trace fragments are mode `0600`.
- Raw trace fragments are deleted on success, command failure, backend failure,
  redaction failure, validation failure, Ctrl-C, and kill-switch cleanup.
- Public report files are written only after redaction.
- Operation records are written only after redaction through the existing
  packaging operation-record path.
- Operation-record retention remains newest 50.
- No DB migration is added.

Redaction covers:

- environment variables and values
- command arguments
- credentialed URLs
- bearer tokens and token-like strings
- private key paths
- absolute private paths
- logs
- trace metadata
- sampled file names when they match secret patterns

The spike should avoid sampling file contents by default. If a later plan adds
content sampling, it must be opt-in, bounded, redacted before persistence, and
tested separately.

If redaction fails, Conary must not write a public trace report or operation
record. It may return a human diagnostic describing the failure without leaking
raw trace details.

`--keep-raw-trace` is hidden, loud, and developer-only. It keeps raw trace under
a private directory and prints a warning that the data may contain secrets. It
does not change operation-record redaction.

## Failure Behavior

Record mode fails closed before running the command when:

- the requested trace backend is unavailable
- required scoped roots cannot be canonicalized
- the private workspace cannot be created with private permissions
- the command is missing
- the command would run without `DESTDIR`/`CONARY_DESTDIR`
- redaction setup fails

If the recorded command exits nonzero, Conary still writes a redacted partial
trace report when redaction succeeds. It does not claim validation success.

If watcher event loss occurs, Conary reports partial trace evidence and returns
failure unless the implementation plan defines a narrower success state that
cannot be mistaken for complete recording.

If cleanup fails, Conary returns failure with a redacted diagnostic naming the
cleanup target. Cleanup failure must not silently leave raw traces behind.

## Testing

M3d spike tests:

- CLI tests prove normal public help does not advertise `--record`, while
  hidden parsing accepts `conary cook --record <source> -- <command>`.
- Request validation tests reject missing commands and unsafe source/output
  shapes.
- Backend probe tests cover supported, unavailable, and permission-denied
  cases.
- Recursive inotify fixture tests record create, modify, delete, rename, and
  dynamic new-directory events inside scoped roots.
- Fanotify behavior is tested through the backend trait and gated integration
  tests so normal CI does not require elevated privileges.
- Trace report tests prove inotify-only runs declare incomplete read evidence.
- Redaction tests cover env secrets, token-like args, credentialed URLs,
  private key paths, logs, trace metadata, and operation records.
- Draft recipe tests prove a simple fixture produces a recipe with relative
  source paths and a safely rendered recorded command.
- Validation tests prove a generated draft can pass normal cook for the simple
  fixture when `--record-validate` is used.
- Recorded-draft tests prove validation artifacts carry
  `origin_class = "recorded-draft"`.
- Publish refusal tests prove recorded-draft artifacts still report
  `RecordedDraftArtifact`.
- Cleanup tests prove raw trace fragments are removed on success, command
  failure, backend failure, validation failure, redaction failure, and
  cancellation.
- Operation-record tests prove only redacted diagnostics/events/report evidence
  are persisted.

Final implementation gates should include focused owner tests, the new M3d
integration test, `cargo fmt --check`, and
`cargo clippy --workspace --all-targets -- -D warnings`.

## Graduation Questions

The spike graduates only if it answers these questions with evidence:

1. Can scoped fanotify/inotify tracing produce useful recipe and installed-file
   evidence without host-root tracing?
2. Can Conary clearly explain which evidence came from fanotify, inotify,
   inference, command logs, or installed-file snapshots?
3. Can redaction protect env, args, paths, URLs, logs, and metadata before
   public report or operation-record persistence?
4. Can the generated draft recipe cook normally for at least one simple
   fixture?
5. Does recorded-draft publish refusal remain visible and unchanged?

If these answers are not strong enough, M3d should end as a documented spike
with next-step recommendations rather than a public feature.

## Review Checklist

- Record mode remains hidden and experimental.
- The first UX is `conary cook --record [SOURCE_DIR] -- <command>`, not an
  interactive shell.
- Fanotify/inotify scope is bounded to source, work, and install roots.
- Explicit fanotify failure is fail-closed.
- Inotify-only traces declare incomplete read evidence.
- Event loss is visible and prevents overclaiming completeness.
- Raw trace is private, ephemeral by default, and never written to operation
  records.
- Public trace reports and operation records are redacted before write.
- Draft recipes avoid absolute host paths.
- Recorded validation artifacts use `origin_class = "recorded-draft"`.
- Recorded drafts remain unpublishable.
- M3d does not weaken M2 publish gates.
- No DB migration is required.
- Large command files remain thin routers; record-mode behavior lives in a
  dedicated owner module.
