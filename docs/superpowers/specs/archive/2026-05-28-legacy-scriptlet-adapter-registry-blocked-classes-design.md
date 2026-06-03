# Legacy Scriptlet Adapter Registry And Blocked Classes Design

## Summary

Goal 3a adds the infrastructure that lets Conary classify legacy scriptlet
behavior with stable evidence before any broad conversion claims are made. It
does not decide packages are safe to publish, does not embed bundles into Remi
artifacts, and does not enable install-time replay. It creates the typed command
evidence, effect IR, adapter registry, blocked-class registry, and support
matrix scaffolding that later goals use to make those decisions.

The goal is intentionally conservative: fixture invocations can be classified
as known, unknown, review, or blocked with stable reason IDs, but Goal 3a does
not mark helper commands as fully `replaced` except for native-free/no-entry
evidence. Goal 3b will add the first corpus-backed adapters that can justify
complete replacement claims.

## Source Context

Read these first when implementing:

- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-bundle-schema-v1-passive-query-design.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-native-abi-extraction-design.md`
- `docs/modules/remi.md`

Current code facts:

- `crates/conary-core/src/packages/native_abi.rs` already preserves
  parser-owned native ABI entries.
- `crates/conary-core/src/ccs/legacy_scriptlets.rs` already defines bundle
  decisions, effect sources, replacement status values, and blocked-class
  fields.
- `crates/conary-core/src/ccs/convert/analyzer.rs` is still regex-like advisory
  extraction that can create hooks, but it is not allowed to become the
  authority for dropping preserved native scriptlets.
- `apps/remi/src/server/scriptlet_corpus.rs` emits command-frequency and
  blocked-class hints for planning only.

## Scope

Goal 3a includes:

- a reusable structured command invocation model;
- static command extraction good enough for fixture classification and corpus
  ranking, not a full shell parser;
- effect IR types aligned with the existing legacy bundle schema;
- an adapter registry API and built-in registry digest;
- a blocked-class registry with stable class IDs, stable reason IDs, default
  outcomes, and unblock criteria;
- a support matrix scaffold tying commands/classes to known support status;
- a classification report exposed from conversion results for tests and later
  Remi metadata work;
- compatibility with the existing `ScriptletAnalyzer` and detected hooks path.

Goal 3a excludes:

- Remi database migrations;
- Remi publication gating;
- bundle embedding in converted CCS packages;
- generating `LegacyScriptletBundle` entries from native ABI entries;
- install/update/remove behavior changes;
- replay, target compatibility enforcement, or operator overrides;
- broad helper-specific `replaced` claims.

## Architecture

Add focused modules under `crates/conary-core/src/ccs/convert/`:

- `command_evidence.rs`: structured command invocations extracted from
  scriptlets, capture logs, native metadata, payload hints, or curated rules.
- `effects.rs`: typed conversion effect IR and classification reports.
- `blocked_classes.rs`: stable policy registry for known unsupported or
  review-only behavior classes.
- `adapters.rs`: adapter registry interfaces and initial conservative built-in
  adapters.
- `support_matrix.rs`: testable support matrix entries for helper commands and
  blocked classes.

The converter keeps its current hook extraction path. Goal 3a adds a parallel
classification report:

```text
PackageMetadata.scriptlets/native_scriptlet_abi
        |
        v
command_evidence static extraction
        |
        v
blocked-class registry precheck
        |
        v
adapter registry classification
        |
        v
ScriptletClassificationReport on ConversionResult
```

The report is evidence for later goals. It must not alter the manifest,
database rows, or install behavior in Goal 3a.

## Command Evidence Model

Use a parser-neutral type:

```rust
pub struct CommandInvocation {
    pub id: String,
    pub entry_id: String,
    pub source: CommandEvidenceSource,
    pub phase: Option<String>,
    pub lifecycle_paths: Vec<String>,
    pub interpreter: Option<String>,
    pub command: String,
    pub argv: Vec<String>,
    pub raw_line: Option<String>,
    pub cwd: Option<String>,
    pub environment: Vec<CommandEnvironmentFact>,
}
```

`id` must be stable for a given package and scriptlet body. Use an entry ID plus
the line ordinal and command ordinal inside that line for static evidence, for
example `rpm:%post:line2:cmd0`.

`CommandEvidenceSource` values:

- `StaticSignal`
- `CaptureLog`
- `NativeMetadata`
- `PayloadHeuristic`
- `CuratedRule`

Static extraction is deliberately small. It should:

- ignore comments and blank lines;
- split common shell separators such as `&&`, `||`, `;`, `|`, command
  substitution delimiters, and parentheses in the same spirit as Goal 0 corpus
  scanning;
- skip simple leading environment assignments;
- skip common wrappers such as `env`, `sudo`, and `chroot` while preserving the
  invoked command;
- skip wrapper-specific positional arguments, such as the chroot target
  directory and sudo flag values, rather than treating them as commands;
- normalize absolute paths to the basename for adapter lookup while preserving
  the raw line and original argv;
- produce `Unknown` evidence rather than failing when the syntax is too rich.

Static extraction is not a shell interpreter. It cannot justify
`EffectReplacement::Complete` by itself.

Native ABI executable entries with `NativeScriptletSupport::Parsed` and UTF-8
body text should feed the same command evidence model through a native-entry
helper. That helper must set `source = NativeMetadata`, preserve `entry.id`,
native lifecycle paths, interpreter, and raw line, and remain advisory only.
Native control artifacts that are not executable script bodies should classify
through review classes instead of being projected into shell invocations.

## Effect IR

Goal 3a introduces internal effect evidence that can later be projected to
`legacy_scriptlets::ScriptletEffect`.

```rust
pub struct ScriptletEffectEvidence {
    pub kind: ScriptletEffectKind,
    pub source: EffectSource,
    pub confidence: EffectConfidence,
    pub replacement: EffectReplacement,
    pub adapter_id: Option<String>,
    pub adapter_digest: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub path: Option<String>,
    pub reason_code: Option<String>,
}
```

Use the existing bundle enums for `EffectSource`, `EffectConfidence`, and
`EffectReplacement` where practical. Keep `ScriptletEffectKind` as stable
string-backed IDs, because new effect kinds will appear across goals without a
schema migration.

Initial effect kind IDs:

- `no-scriptlet`
- `dynamic-linker-cache`
- `systemd-daemon-reload`
- `systemd-unit-enable`
- `systemd-unit-disable`
- `tmpfiles`
- `sysusers`
- `alternatives`
- `cache-refresh`
- `unknown-command`
- `blocked-class`

Goal 3a adapters may emit `replacement = "none"`, `partial`, or `blocked`.
Only the native-free/no-scriptlet path can emit `complete` because there is no
raw behavior to replace.

## Classification Outcomes

Use a classification layer separate from bundle decisions:

```rust
pub enum ScriptletClassification {
    Known {
        reason_code: String,
        effects: Vec<ScriptletEffectEvidence>,
    },
    Unknown {
        reason_code: String,
        command: String,
    },
    Review {
        reason_code: String,
        class_id: Option<String>,
    },
    Blocked {
        reason_code: String,
        class_id: String,
    },
}
```

This classification is pre-decision evidence. It must not be treated as the
same thing as `LegacyScriptletEntry.decision`.

Stable reason codes:

- `native-free-no-scriptlets`
- `known-helper-requires-adapter-coverage`
- `known-helper-partial-coverage`
- `unknown-command`
- `review-class-dbus-policy`
- `review-class-rpm-verify`
- `review-class-rpm-trigger`
- `review-class-deb-trigger`
- `review-class-debconf`
- `review-class-udev`
- `review-class-arch-alpm-hook`
- `review-class-arch-install-function`
- `blocked-class-network`
- `blocked-class-package-manager-recursion`
- `blocked-class-pam`
- `blocked-class-selinux`
- `blocked-class-apparmor`
- `blocked-class-kernel-module`
- `blocked-class-initramfs`
- `blocked-class-bootloader`
- `blocked-class-setuid-setcap`
- `blocked-class-sysctl`
- `blocked-class-legacy-init`
- `blocked-class-native-abi-unpreservable`

Parser-owned reason codes from Goal 2 `NativeScriptletSupport` values are also
preserved verbatim in classification reports. Examples include
`rpm-verify-scriptlet-deferred`, `rpm-trigger-semantics-deferred`,
`deb-trigger-semantics-deferred`, `arch-alpm-hook-semantics-deferred`, and
`native-abi-parser-limitation`.

The reason-code list is not closed forever, but new codes must be explicit,
tested, and added to the support matrix.

## Adapter Registry

The registry is an ordered set of adapters. Each adapter declares:

- `adapter_id`, such as `ldconfig/v1`;
- a content digest;
- command names and absolute paths it handles;
- source formats/families it accepts;
- lifecycle paths it accepts;
- argument shapes it recognizes;
- emitted effect kinds;
- replacement status;
- reason code;
- support matrix entry ID.

Rust shape:

```rust
pub trait ScriptletEffectAdapter {
    fn id(&self) -> &'static str;
    fn digest(&self) -> String;
    fn matches(&self, invocation: &CommandInvocation) -> bool;
    fn classify(&self, invocation: &CommandInvocation) -> ScriptletClassification;
}

pub struct AdapterRegistry {
    adapters: Vec<Box<dyn ScriptletEffectAdapter + Send + Sync>>,
    blocked_classes: BlockedClassRegistry,
}
```

If a blocked class matches first, the blocked-class result wins over an adapter
match. This prevents a specific command parser from accidentally weakening a
known unsafe class. Review-only classes also win unless an adapter explicitly
declares the class as unblocked and has complete fixture coverage; Goal 3a must
not add such unblocks.

Initial built-in adapters:

- `native-free/v1`: package-level evidence for no native scriptlet ABI entries
  and no flattened scriptlets. Emits `Known` with
  `native-free-no-scriptlets` and a complete `no-scriptlet` effect.
- `ldconfig/v1`: recognizes `ldconfig` and `/sbin/ldconfig`, emits
  `Known` with `known-helper-requires-adapter-coverage` and
  `dynamic-linker-cache` replacement `none`.
- `systemd-daemon-reload/v1`: recognizes `systemctl daemon-reload`, emits
  `Known` with `known-helper-requires-adapter-coverage` and
  `systemd-daemon-reload` replacement `none`.
- `systemd-enable-disable/v1`: recognizes `systemctl enable|disable UNIT`,
  emits `Known` with `known-helper-requires-adapter-coverage` and the relevant
  systemd unit effect replacement `none`.

These adapters are useful because they prove the registry can route known
commands without claiming Conary has already replaced them.

`NativeFreeAdapter` is a registry and support-matrix entry, but native-free
classification is package-level evidence. It is intentionally reached through
`AdapterRegistry::classify_native_free_package()` rather than per-command
`CommandInvocation` dispatch, because there are no invocations to classify.

## Blocked-Class Registry

The blocked-class registry contains policy facts, not parser heuristics. Each
entry includes:

```rust
pub struct BlockedClass {
    pub id: &'static str,
    pub description: &'static str,
    pub default_outcome: BlockedClassOutcome,
    pub reason_code: &'static str,
    pub command_names: &'static [&'static str],
    pub command_forms: &'static [&'static str],
    pub affected_formats: &'static [&'static str],
    pub preview_distros: &'static [&'static str],
    pub unblock_criteria: &'static str,
}
```

Initial entries:

| Class ID | Outcome | Match examples |
| --- | --- | --- |
| `network` | blocked | `curl`, `wget`, `scp`, `ssh` |
| `package-manager-recursion` | blocked | `dnf`, `yum`, `rpm`, `apt`, `apt-get`, `dpkg`, `pacman` |
| `pam` | blocked | `authselect`, `pam-auth-update` |
| `selinux` | blocked | `restorecon`, `semanage`, `setsebool` |
| `apparmor` | blocked | `apparmor_parser`, `aa-enforce`, `aa-disable` |
| `kernel-module` | blocked | `modprobe`, `depmod`, `dkms` |
| `initramfs` | blocked | `dracut`, `mkinitcpio`, `update-initramfs` |
| `bootloader` | blocked | `grub-mkconfig`, `grub2-mkconfig`, `update-grub`, `bootctl` |
| `setuid-setcap` | blocked | `setcap`, `setpriv`, `chmod u+s`, `chmod 4*` |
| `sysctl` | blocked | `sysctl` |
| `legacy-init` | blocked | `chkconfig`, `update-rc.d`, `rc-update` |
| `native-abi-unpreservable` | blocked | `NativeScriptletSupport::Unpreservable` |
| `dbus-policy` | review | `dbus-update-activation-environment`, `dbus-send`, D-Bus service/policy paths |
| `rpm-verify` | review | RPM `%verify` native ABI metadata |
| `rpm-trigger` | review | RPM trigger native ABI metadata |
| `deb-trigger` | review | DEB trigger control artifacts |
| `debconf` | review | DEB `config` maintainer script |
| `udev` | review | `udevadm trigger*`, `udevadm control*` |
| `arch-alpm-hook` | review | Arch ALPM hook native ABI metadata |
| `arch-install-function` | review | Arch `.INSTALL` function extraction deferred |

`command_names` matches normalized command basenames. `command_forms` matches
command-plus-arguments when the command name alone is too broad, such as
`chmod u+s` or mode forms beginning with `chmod 4`. Build the form as
`command + " " + argv.join(" ")`, compare exact forms, and treat entries ending
in `*` as prefix matches. This lets `chmod u+s /usr/bin/foo` and
`chmod 4755 /usr/bin/foo` classify as `setuid-setcap` without treating every
`chmod` as blocked.

DEB purge and abort maintainer-script modes should receive their own review
reason codes when those lifecycle paths are wired into replay policy. Goal 3a
only uses `review-class-debconf` for debconf/config evidence.

`debconf` review evidence is not a request to install Debian tooling on
non-Debian targets. A converted DEB that requires debconf configuration must
later be handled by a modeled Conary-native configuration equivalent,
source-native/family-compatible policy, or an explicit operator-supplied
configuration transform. On foreign targets such as Fedora, unresolved debconf
requirements remain review/blocked evidence rather than a reason to install
`debconf`, run `dpkg`, or fake a Debian maintainer-script environment.

The registry should expose a summary that later Remi metadata can store as
`unsupported_class_counts`. Goal 3a should count classes in the classification
report but not write those counts to Remi DB rows yet.

## Support Matrix

The support matrix is a testable Rust table in `support_matrix.rs`, not a
human-only Markdown table. Each entry records:

- matrix ID;
- command or class ID;
- classification outcome;
- reason code;
- adapter ID if any;
- source families and formats;
- lifecycle notes;
- fixture names that prove the current status.

Every built-in adapter and every blocked-class entry must have at least one
matrix row. Tests should fail if a class or adapter is missing from the matrix.

## Converter Integration

Add `scriptlet_classification: ScriptletClassificationReport` to
`ConversionResult`.

The converter should build the report after native metadata is cloned and before
or alongside the existing analyzer call. This keeps classification visible to
tests without changing `CcsManifest`, package archives, Remi records, or install
behavior.

Classification must inspect both flattened `metadata.scriptlets` and
`metadata.native_scriptlet_abi`. Flattened scriptlets feed static command
evidence. Native ABI entries feed parser-support evidence: `Parsed` entries may
continue to native command extraction where executable text is available, while
`DeferredReview` maps to `Review` using the parser-owned reason code and
`Unpreservable` maps to `Blocked` with class ID `native-abi-unpreservable`.
This prevents Goal 2 parser warnings from disappearing when Goal 3a adds the
new report.

The only Remi-side change required by this result-field addition is a
mechanical update to test helper literals that construct `ConversionResult`
directly. Goal 3a must not change Remi publication state, API responses, DB
schema, or package-serving behavior.

The existing `ScriptletAnalyzer` path remains:

- `detected_hooks` is still populated as before.
- `FidelityReport` is still produced as before.
- `hooks` in the CCS manifest still come from the existing detected/captured
  hook path until later goals deliberately change authority.

The new registry path is parallel evidence only.

## Error Handling

Goal 3a classification should be infallible for ordinary scriptlet text. Parser
limitations become `Unknown` or `Review` evidence, not conversion errors.

Errors are appropriate only for internal invariants:

- duplicate adapter IDs;
- duplicate class IDs;
- duplicate support matrix IDs;
- an adapter or class missing support matrix coverage;
- invalid static registry definitions.

Those should be caught by tests and by registry constructors in development.

## Testing

Required test classes:

- `command_evidence` tests proving shell control operator splitting, wrapper
  skipping, absolute path normalization, environment assignment skipping, and
  stable IDs.
- `blocked_classes` tests proving network and package-manager recursion are
  blocked, D-Bus/debconf/trigger classes are review, and harmless unknown
  commands are not mislabeled as blocked.
- `adapter_registry` tests proving known helper commands are routed to built-in
  adapters without complete replacement claims.
- `support_matrix` tests proving every built-in adapter and blocked class has a
  matrix row with the same reason code.
- `conversion_integration` tests proving `LegacyConverter::convert` exposes the
  classification report while preserving existing manifest hook behavior.

Verification:

```bash
cargo test -p conary-core command_evidence
cargo test -p conary-core adapter_registry
cargo test -p conary-core blocked_classes
cargo test -p conary-core support_matrix
cargo test -p conary-core conversion_integration
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Non-Goals For Reviewers

Do not ask Goal 3a to:

- prove `ldconfig`, systemd, tmpfiles, sysusers, or alternatives are fully
  replaced;
- embed `LegacyScriptletBundle` into Remi conversions;
- block public Remi publication;
- add database fields;
- run native scriptlets;
- rewrite shell scripts;
- retire the regex analyzer.

Those are later goals. Goal 3a is the steel framing: stable evidence,
classification, and policy surfaces that make the later work auditable.
