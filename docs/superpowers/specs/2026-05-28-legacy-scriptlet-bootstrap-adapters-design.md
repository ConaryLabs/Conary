# Legacy Scriptlet Bootstrap Adapters Design

## Summary

Goal 3b turns the Goal 3a classification scaffolding into the first useful,
corpus-backed adapter set. It promotes a small number of common helper command
forms from "recognized but not replaced" to typed effect evidence with complete,
partial, review, or blocked outcomes. The work remains passive: it does not
embed bundles, publish Remi conversion results, add database columns, or change
install/update/remove behavior.

The goal is deliberately narrow. A command can receive
`EffectReplacement::Complete` only when an adapter has a precise command-form
parser, a support-matrix row, fixture coverage, and enough payload or argument
evidence to satisfy the parent spec's `replaced` rubric. Static command
extraction may identify an invocation, but static extraction alone is not
replacement authority.

## Source Context

Read these first when implementing:

- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-adapter-registry-blocked-classes-design.md`
- `docs/superpowers/plans/2026-05-28-legacy-scriptlet-adapter-registry-blocked-classes-plan.md`
- `docs/modules/remi.md`
- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/ccs/convert/support_matrix.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `apps/remi/src/server/scriptlet_corpus.rs`

External semantics anchors:

- `systemctl(1)`: <https://www.freedesktop.org/software/systemd/man/latest/systemctl.html>
- `systemd-tmpfiles(8)`: <https://www.freedesktop.org/software/systemd/man/latest/systemd-tmpfiles.html>
- `systemd-sysusers(8)`: <https://www.freedesktop.org/software/systemd/man/latest/systemd-sysusers.html>
- `sysusers.d(5)`: <https://www.freedesktop.org/software/systemd/man/latest/sysusers.d.html>
- `update-alternatives(1)`: <https://manpages.debian.org/bookworm/dpkg/update-alternatives.1.en.html>
- `ldconfig(8)`: <https://man7.org/linux/man-pages/man8/ldconfig.8.html>

Those anchors are requirements sources for command-form parsing. The
implementation should prefer the official or upstream manpage behavior over
distribution folklore when deciding whether an invocation is complete, partial,
or review-only.

## Scope

Goal 3b includes:

- a typed adapter input context that combines a command invocation with package
  payload hints;
- payload-hint extraction from the already available `ExtractedFile` list;
- structured effect metadata for adapter-specific facts;
- complete replacement evidence for a small, safe subset of helper forms;
- review outcomes for runtime, interactive, source-family private, or
  insufficiently modeled variants of those helpers;
- support-matrix rows for every new adapter and review class;
- converter integration tests proving passive classification changes do not
  alter manifest output or existing hook detection.

Goal 3b excludes:

- Remi database migrations;
- Remi publication gating;
- `LegacyScriptletBundle` embedding;
- conversion from `ScriptletClassificationReport` to bundle entries;
- install/update/remove/replay behavior;
- broad shell interpretation;
- support for DEB debconf replay, RPM triggers, DEB triggers, Arch ALPM hook
  replay, or Arch `.INSTALL` wrapper replay;
- completing distro-private helper semantics on foreign targets.

## Corpus Evidence Rule

Bootstrap adapters must be justified by existing Goal 0 corpus evidence or by a
checked-in fixture that models the same evidence shape.

Goal 3b does not require live repository metadata to be present on every
developer machine. The implementation should therefore add tests around a small
corpus-evidence fixture shape in `adapters.rs` instead of requiring networked
repository sync during unit tests. The fixture should contain the command and
form counts that justify the initial candidate list:

- `ldconfig`
- `systemctl daemon-reload`
- `systemctl enable`, `disable`, and `preset`
- `systemd-tmpfiles --create`
- `systemd-sysusers`
- `update-alternatives` or `alternatives`
- common cache refresh commands such as `update-mime-database`,
  `update-desktop-database`, `gtk-update-icon-cache`, `glib-compile-schemas`,
  and `fc-cache`
- common review-only legacy helpers such as `install-info` and
  `gconftool-2`

If a candidate is not present in the evidence fixture, the adapter may remain in
the code only as review or partial coverage. It must not be marked complete.

Use a small typed shape:

```rust
pub struct BootstrapAdapterEvidence {
    pub command: &'static str,
    pub forms: &'static [&'static str],
    pub package_count: u32,
    pub invocation_count: u32,
    pub coverage_ids: &'static [&'static str],
}
```

The fixture is not a replacement for future corpus refreshes. It is a local
guardrail that keeps the bootstrap set tied to the Goal 0 evidence path and
makes later adapter additions explain where their priority came from.

## Adapter Authority Model

An adapter may mark an effect `complete` only when all of these are true:

- the command name and command form match a precise adapter parser;
- the adapter emits a stable effect kind, adapter ID, adapter digest, and reason
  code;
- each argument that affects native behavior is either captured in structured
  effect metadata or rejected to review;
- required payload facts are present when the command depends on files shipped
  by the package;
- the invocation is non-interactive and does not start, stop, restart, reload,
  or signal a live service;
- the helper is not a source-family private configuration runtime such as
  `debconf` or `deb-systemd-helper`;
- fixture tests cover both the complete form and at least one non-complete
  variant.

`complete` remains passive evidence in Goal 3b. It means "the adapter has enough
information for later bundle generation to replace this invocation," not "the
installer runs anything differently today."

The service-manager rule distinguishes unit-cache reloads from service reloads.
`systemctl daemon-reload` communicates with the running systemd manager to
reload unit definitions, but it does not start, stop, restart, reload, or signal
any service unit. Goal 3b may classify that unit-cache effect as complete
because it is idempotent and state-neutral. Runtime unit actions remain review.

## Data Model Changes

Goal 3a introduced `ScriptletEffectEvidence` with basic command fields. Goal 3b
adds structured adapter metadata:

```rust
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

The field mirrors `legacy_scriptlets::ScriptletEffect.extra`, so later bundle
embedding can copy adapter facts without inventing a second metadata shape.

Add adapter input context:

```rust
pub struct AdapterInput<'a> {
    pub invocation: &'a CommandInvocation,
    pub payload: &'a PayloadHints,
}
```

Add payload hints under `crates/conary-core/src/ccs/convert/payload_hints.rs`:

```rust
pub struct PayloadHints {
    pub systemd_units: BTreeSet<String>,
    pub tmpfiles_configs: BTreeSet<String>,
    pub sysusers_configs: BTreeSet<String>,
    pub shared_libraries: BTreeSet<String>,
    pub cache_inputs: BTreeMap<String, BTreeSet<String>>,
}
```

`PayloadHints` is built from `ExtractedFile` paths already passed to the
converter. It should not read from the live host. Goal 3b may inspect file
contents only in later review fixes if a specific adapter needs it, but the
initial adapter set should stay path-based.

## Adapter Set

### Native-Free

The package-level `native-free/v1` path remains complete when both
`metadata.scriptlets` and `metadata.native_scriptlet_abi` are empty. This is the
only complete classification that does not depend on a command invocation.

### Dynamic Linker Cache

Adapter ID: `ldconfig/v2`

Complete forms:

- `ldconfig`
- `/sbin/ldconfig`
- `ldconfig -v`
- `ldconfig --verbose`

Complete evidence:

- effect kind: `dynamic-linker-cache`
- replacement: `complete`
- reason code: `helper-complete-ldconfig`

Review forms:

- `ldconfig -p`
- `ldconfig -l ...`
- `ldconfig -n ...`
- `ldconfig -N`
- `ldconfig -X`
- `ldconfig -C ...`
- `ldconfig -f ...`
- `ldconfig -r ...`
- any form with explicit directory operands

Review reason: `review-class-ldconfig-nonstandard`

The strict split avoids pretending that custom roots, alternate caches, and
link-only/cache-only modes have the same semantics as the common scriptlet
cache refresh.

### systemd Manager Metadata

Adapter IDs:

- `systemd-daemon-reload/v2`
- `systemd-unit-state/v1`

Complete forms:

- `systemctl daemon-reload`
- `systemctl --system daemon-reload`
- `systemctl enable UNIT...`
- `systemctl disable UNIT...`
- `systemctl preset UNIT...`

Unit-state completion requires all unit arguments to look like unit names and,
when payload hints are available, to correspond to units shipped by the package.
If payload hints are empty because the test or conversion path has no extracted
file list, the adapter may emit `partial`, not `complete`.

Review forms:

- `systemctl start|stop|restart|try-restart|reload|reload-or-restart ...`
- `systemctl enable --now ...`
- `systemctl preset-all`
- `systemctl --user ...`
- `systemctl --global ...`
- `service ...`
- `invoke-rc.d ...`
- `deb-systemd-helper ...`
- `deb-systemd-invoke ...`

Review reasons:

- `review-class-systemd-runtime-action`
- `review-class-systemd-user-scope`
- `review-class-deb-systemd-helper`

Runtime service actions are not replacements in Goal 3b because they mutate or
signal a live manager. DEB helper commands remain source-family private until
there is a dedicated compatibility model; Conary should not install dpkg or
deb-systemd-helper on Fedora or Arch targets to make these calls work.
`service` and `invoke-rc.d` should match by command name as review classes even
when static extraction loses their arguments.

### tmpfiles

Adapter ID: `systemd-tmpfiles-create/v1`

Complete forms:

- `systemd-tmpfiles --create`
- `systemd-tmpfiles --create PATH...`
- `systemd-tmpfiles --create --prefix PATH...`

Completion requires at least one packaged tmpfiles config in
`/usr/lib/tmpfiles.d/`, `/lib/tmpfiles.d/`, or `/etc/tmpfiles.d/`. If explicit
config paths are present, each path must match a packaged tmpfiles config.

Review forms:

- `systemd-tmpfiles --remove`
- `systemd-tmpfiles --clean`
- `systemd-tmpfiles --purge`
- `systemd-tmpfiles --boot`
- `systemd-tmpfiles --user`
- `systemd-tmpfiles --replace=...`
- stdin-driven forms

Review reason: `review-class-tmpfiles-noncreate`

### sysusers

Adapter ID: `systemd-sysusers/v1`

Complete forms:

- `systemd-sysusers`
- `systemd-sysusers PATH...`

Completion requires at least one packaged sysusers config in
`/usr/lib/sysusers.d/`, `/lib/sysusers.d/`, or `/etc/sysusers.d/`. If explicit
config paths are present, each path must match a packaged sysusers config.

Review forms:

- `systemd-sysusers --replace=...`
- `systemd-sysusers --root=...`
- stdin-driven forms

Review reason: `review-class-sysusers-nonstandard`

### Alternatives

Adapter ID: `alternatives-registration/v1`

Complete forms:

- `update-alternatives --install LINK NAME PATH PRIORITY [--slave LINK NAME PATH]...`
- `update-alternatives --remove NAME PATH`
- `alternatives --install LINK NAME PATH PRIORITY [--slave LINK NAME PATH]...`
- `alternatives --remove NAME PATH`

The adapter must parse and preserve:

- action: `install` or `remove`;
- master link;
- alternative name;
- alternative path;
- priority;
- slave triplets.

Review forms:

- `--config`
- `--set`
- `--auto`
- `--all`
- `--remove-all`
- `--remove NAME` without the required path argument;
- any malformed `--install` or `--remove` form

Review reason: `review-class-alternatives-interactive-or-broad`

The complete subset is registration/removal evidence only. User or
administrator selection state is not modeled in Goal 3b.

### Derived Cache Refreshes

Adapter ID: `cache-refresh/v1`

Complete command forms:

- `update-mime-database /usr/share/mime`
- `update-desktop-database /usr/share/applications`
- `gtk-update-icon-cache -q /usr/share/icons/THEME`
- `glib-compile-schemas /usr/share/glib-2.0/schemas`
- `fc-cache` with no directory operands or with packaged font directories

Completion requires either a standard path listed above or payload hints showing
the package ships the corresponding cache input:

- MIME XML packages under `/usr/share/mime/packages/`;
- desktop files under `/usr/share/applications/`;
- icon files under `/usr/share/icons/`;
- GSettings schemas under `/usr/share/glib-2.0/schemas/`;
- fonts under `/usr/share/fonts/`, `/usr/local/share/fonts/`, or
  `/usr/share/texmf/fonts/`.

Review reason for nonstandard forms:
`review-class-cache-refresh-nonstandard`

`gtk-update-icon-cache` matching must parse concrete theme directories rather
than matching a literal wildcard. The adapter should strip benign flags such as
`-f`, `--force`, `-q`, `--quiet`, `--ignore-theme-index`, and combined short
forms such as `-qf`, then treat a remaining directory under `/usr/share/icons/`
as the cache root. Completion requires payload icon files under that same theme
prefix.

Benign flags for other cache refresh helpers:

- `update-desktop-database`: `-q`, `--quiet`.
- `glib-compile-schemas`: `--allow-any-name`; a no-argument invocation is also
  complete when payload hints contain GSettings schema files.
- `fc-cache`: `-s`, `--system-only`, `-f`, `--force`, `-r`,
  `--really-force`, `-v`, `--verbose`, and combined short forms such as `-fs`.

All other flags or nonstandard paths remain
`review-class-cache-refresh-nonstandard`.

### Review-Only Legacy Desktop And Documentation Helpers

Goal 3b should classify common obsolete or not-yet-modeled helper commands with
specific review reasons rather than leaving them as generic unknown commands.

Review classes:

- `gconf-schema`: commands `gconftool` and `gconftool-2`; reason
  `review-class-gconf-schema`.
- `install-info`: command `install-info`; reason `review-class-install-info`.

These are not complete adapters in Goal 3b. GConf schema mutation should be
migrated to modern GSettings schema handling when possible. GNU Info directory
registration should become a declarative cache/index effect later, but Goal 3b
only labels it for review.

## Source-Family Private Helper Policy

Some helpers are native package-manager adjuncts rather than portable Linux
facilities. Goal 3b should flag them for review instead of trying to satisfy
them by installing foreign runtime tools:

- `debconf-*`, `db_*`, and DEB `config` scripts remain
  `review-class-debconf`;
- `deb-systemd-helper` and `deb-systemd-invoke` use
  `review-class-deb-systemd-helper`;
- DEB trigger declarations remain `review-class-deb-trigger`;
- RPM triggers and `%verify` remain the Goal 2 parser-owned review classes;
- Arch ALPM hooks and `.INSTALL` wrapper semantics remain review classes.

This policy is the bridge between adapter work and mixed-package safety:
foreign helper runtimes are evidence that the package needs modeling,
curation, or same-family replay policy. They are not dependencies Conary should
silently install on unrelated targets.

## Support Matrix

Every adapter and review class introduced by Goal 3b needs a
`SupportMatrixEntry` with:

- stable ID;
- command or class ID;
- adapter ID where applicable;
- outcome;
- reason code;
- source families;
- lifecycle notes;
- fixture names.

The matrix should distinguish `Known` rows with complete replacement evidence
from `Review` rows that are recognized but not yet portable.

## Data Flow

```text
PackageMetadata + ExtractedFile list
        |
        v
PayloadHints::from_files(files)
        |
        v
command_evidence extraction from flat scriptlets and native ABI entries
        |
        v
blocked/review class precheck
        |
        v
adapter registry with AdapterInput { invocation, payload }
        |
        v
ScriptletClassificationReport on ConversionResult
```

The converter should keep the current hook extraction path unchanged. It should
only pass `files` into classification so adapters can use payload hints.

## Testing Strategy

Goal 3b should add or update tests for:

- payload hint extraction for systemd units, tmpfiles configs, sysusers configs,
  shared libraries, and cache inputs;
- each complete adapter form;
- at least one review/non-complete variant for each adapter family;
- support matrix coverage for every new adapter ID and review class;
- converter integration showing complete classifications appear in
  `ConversionResult.scriptlet_classification`;
- scope guards showing `manifest.legacy_scriptlets` remains `None` and existing
  detected hooks are preserved;
- no live host helper execution.

Run the goal-specific tests plus `cargo clippy --workspace --all-targets -- -D
warnings`, `cargo fmt --check`, and `git diff --check` before calling the
implementation done.

## Open Edges For Later Goals

Goal 3b intentionally leaves these to later goals:

- converting complete effect evidence into `LegacyScriptletBundle` entries;
- generating CCS hooks from complete effects;
- target compatibility and foreign replay policy enforcement;
- publication gates;
- same-family legacy replay;
- native trigger execution;
- DEB debconf transformation;
- administrator override records.
