---
last_updated: 2026-08-22
revision: 3
summary: Working CLI UX audit of daily-driver flows with annotated captures, comparator notes, design direction, and proposed implementation slices
---

# CLI UX Audit: Daily-Driver Flows

This is the audit slice for the CLI UX and beautification pass
(issue #132). It captures what the `conary` CLI actually prints today on the
daily-driver paths, judges those captures against dnf5, apt, and pacman
conventions, and produces the ranked slice list. Each later slice lands
separately under the existing `apps/conary/src/ui/` conventions and the
vocabulary guard.

This document owns baseline observations and design proposals while #132 is
active. `docs/operations/daily-driver-ux-matrix.md` remains the durable owner
for operator routes and wording. Later slices keep exact before/after evidence
in their issue and pull request instead of turning this audit into a historical
log; durable behavior moves to the owning module or operations document.

## Scope

Issue #132 supplies the tracking home, not frozen product authority. Its
original checklist predates substantial CLI, generation, and repository work;
this audit does not reassert those old assumptions. Claims here are limited to
the current-head captures and source paths checked for this pull request.

In scope: visual hierarchy, progress rendering, output vocabulary, typed error
presentation, transaction-summary layout, and TTY versus piped degradation.
Changes to mutation semantics, query behavior, publication, generation
authority, download scheduling, or automation contracts require their own
issue and owning-subsystem proof.

## Method

- Observation host: Ubuntu 24.04 container, x86_64, `conary 0.16.1` debug
  build from workspace source (rustc 1.98.0). This names the capture
  environment; it is not a client-support claim.
- Captures are annotated stdout+stderr transcripts. Sandbox paths are replaced
  with `<sandbox>`, terminal control sequences are transcribed, omitted spans
  are marked, and `[exit: N]` is an annotation rather than command output.
  Piped captures use `NO_COLOR=1` against a throwaway root and database
  (`conary system init --db-path <sandbox>/conary.db`).
- The local package used to drive install/remove is a two-file `hello-conary`
  1.0.0 CCS built with `conary ccs build --local-dev`.
- Selected TTY behavior was captured with `NO_COLOR` unset under a real pty
  (`script -qec`), with control sequences transcribed. Piped captures do not
  establish TTY color or redraw behavior for flows that were not separately
  exercised under a pty.
- Comparator: `apt-get` captured on the same host; dnf5 and pacman provide
  visual reference points for transaction tables, size totals, and explicit
  confirmation. They are not Conary behavior authority; any implementation
  slice borrowing a convention must pin the exact comparator version or
  upstream documentation it relies on.

## Captures

### First touch

```
$ conary
Conary Package Manager v0.16.1
Run 'conary --help' for usage information
```

`--help` groups primary commands first and keeps advanced surfaces behind
`--help-advanced`; the after-help shows daily workflow examples. This is in
good shape relative to comparators.

```
$ conary frobnicate
error: unrecognized subcommand 'frobnicate'
```

Note the lowercase `error:` here (clap) versus the capitalized `Error:` from
Conary's own error reporter below — two error voices on adjacent paths.

### Uninitialized database

```
$ conary install nginx --dry-run --db-path <sandbox>/absent.db --root <sandbox>/root
Error: Custom database not initialized at "<sandbox>/absent.db".
Run 'conary system init --db-path <PATH>' with the same custom path.
[exit: 1]
```

Actionable and specific. Two nits: `Error:` capitalization (versus `ui::error`'s
`error:` and clap's `error:`), and the Rust `Debug` quoting of the path
(`"..."` comes from `{:?}` formatting).

### system init

```
$ conary system init --db-path <sandbox>/conary.db
Initialized database at <sandbox>/conary.db
[ok]       Added: remi-fedora-44 (Conary Remi, Fedora 44 source)
[ok]       Added: remi-ubuntu-26.04 (Conary Remi, Ubuntu 26.04 LTS source)
[ok]       Added: remi-arch (Conary Remi, Arch Linux (rolling) source)
[ok]       Added: remi-solus (Conary Remi, Solus (rolling) source)
Configured built-in Remi package source feeds
Discovered typed host lifecycle interfaces (service manager, sysusers, tmpfiles, sysctl, ldconfig)
note: Run 'conary repo sync' to download metadata from every enabled Remi feed.
note: Enroll native sources with exact trust, identity, stream, and update policy.
[exit: 0]
```

Best-behaved surface captured: guarded tags, aligned rows, `note:` routing.
This is the visual language the rest of the CLI should converge on.

### install --dry-run (local CCS)

```
$ conary install <sandbox>/pkg/out/hello-conary-1.0.0-1.ccs --db-path <sandbox>/conary.db --root <sandbox>/root --dry-run
Installing CCS package...
  Lifecycle: none

Would install package: hello-conary version 1.0.0
  Architecture: noarch
  Components to install: runtime (7 files)
  Dependencies: 0

Dry run complete. No changes made.
[exit: 0]
```

### install --yes

```
$ conary install <sandbox>/pkg/out/hello-conary-1.0.0-1.ccs --db-path <sandbox>/conary.db --root <sandbox>/root --yes
Installing CCS package...
 WARN Package mutation committed, but generation publication is pending changeset_id=1 retry="conary system generation publish --yes"
WARNING: package mutation committed, but generation publication is pending for changeset 1.
Run: conary system generation publish --yes
Installed package: hello-conary version 1.0.0
  Architecture: noarch
  Files installed: 7
  Components: :runtime
  Dependencies: 0
[exit: 0]
```

The same fact is printed twice in two different voices: once as a `tracing`
`WARN` line with `key=value` fields (internal log formatting, visible at the
default `warn` level) and once as an ALL-CAPS `WARNING:` println that matches
neither `ui::warn` (`warning:`) nor the tag vocabulary.

### install --yes under a TTY

Under a pty the single-package spinner renders a second, zero-length
progress bar that repeatedly paints a full-width `░`-bar strip ending in
`0/0`, interleaved with redraw erasures, before the final summary:

```
^[[32m⠁^[[0m Installing            ░░░░░░░░░░░░...░░░░ 0/0
░░░░░░░░░░░░░░░░░░░░░░░░░...░░░░ 0/0
^[[2K^[[32m⠁^[[0m Installing
░░░░░░░░░░░░░░░░░░░░░░░░░...░░░░ 0/0
...
Installed package: hello-conary version 1.0.0
```

The `InstallProgress::single` path adds a hidden status bar to the
`MultiProgress`, but the interaction still yields visible zero-length `0/0`
bar frames on the flagship flow. Non-TTY output is clean only because
indicatif suppresses itself there.

### Repeat install

```
$ conary install <same>.ccs ... --yes
Installing CCS package...
Error: Package hello-conary version 1.0.0 (noarch) is already installed
[exit: 1]
```

Comparators treat this as a no-op success with a calm sentence
(apt: `<pkg> is already the newest version (...)`, exit 0). Conary fails the
command after having already printed a progress verb.

### list / list --info

```
$ conary list --db-path <sandbox>/conary.db
Installed packages:
  hello-conary 1.0.0 (Package) [noarch]

Total: 1 package(s)
[exit: 0]

$ conary list hello-conary --info --db-path <sandbox>/conary.db
Name        : hello-conary
Version     : 1.0.0
Type        : Package
Authority   : conary-owned
Source      : file
Versioning  : conary
Architecture: noarch
Description : CCS v3 hello-conary
Installed   : 2026-08-21 22:17:20
Install Type: Explicit
Pinned      : no
Files       : 7
Size        : 32 B

Provides (3):
  file(/usr/bin/hello-conary)
  file(/usr/share/doc/hello/README)
  hello-conary

Components (1):
  :runtime
[exit: 0]
```

Content is competitive with `dnf info`/`pacman -Qi`. The field rendering is a
third hand-rolled idiom (`Name        : value`) distinct from `ui::field`
(`  Label: value`) and from `ccs build`'s underlined headings.

```
$ conary list definitely-not-installed --info --db-path <sandbox>/conary.db
Error: Package 'definitely-not-installed' is not installed
[exit: 1]
```

### query depends / whatprovides

```
$ conary query depends hello-conary --db-path <sandbox>/conary.db
Package 'hello-conary' has no dependencies
[exit: 0]

$ conary query whatprovides /usr/bin/hello-conary --db-path <sandbox>/conary.db
No package provides '/usr/bin/hello-conary'
[exit: 0]

$ conary query whatprovides "file(/usr/bin/hello-conary)" --db-path <sandbox>/conary.db
Capability 'file(/usr/bin/hello-conary)' is provided by:
Installed providers:
  hello-conary 1.0.0 [noarch]

Total: 1 provider(s)
[exit: 0]
```

The help text promises bare file-path lookup ("package name, file path, raw
native provide, or typed form"), and `list --info` records the provide as
`file(/usr/bin/hello-conary)`, but the bare path form silently reports no
provider. That is a behavior defect surfaced by the audit, not a wording
problem.

### update

```
$ conary update --db-path <sandbox>/conary.db --root <sandbox>/root --dry-run
All packages are up to date
[exit: 0]
```

(With an empty database the same command prints `No packages to update` — two
phrasings for adjacent empty states.)

### remove

```
$ conary remove hello-conary --db-path <sandbox>/conary.db --root <sandbox>/root --dry-run
error: unexpected argument '--dry-run' found
[exit: 2]

$ conary remove hello-conary --db-path <sandbox>/conary.db --root <sandbox>/root --yes
Removing package: hello-conary
 WARN Package mutation committed, but generation publication is pending changeset_id=2 retry="conary system generation publish --yes"
WARNING: package mutation committed, but generation publication is pending for changeset 2.
Run: conary system generation publish --yes
Removed package: hello-conary version 1.0.0
  Architecture: noarch
  Files removed: 2
  Directories removed: 5
[exit: 0]
```

`remove` has no `--dry-run` at all, while `install`, `update`, and
`autoremove` do — and the live-mutation refusal text ("Use --dry-run when
available to preview first") plus every comparator (`apt-get remove -s`,
`dnf remove --assumeno`, pacman's pre-transaction summary) assume a removal
preview exists. There is also no pre-apply summary of what will be removed.

### Preflight / refusal experience

```
$ conary install nginx --db-path <sandbox>/conary.db
Error: command 'conary install' may change packages, files, scriptlets, ownership, or the live Conary database. Current --root or similar arguments are not sufficient isolation for this command yet. Use --dry-run when available to preview first. Rerun with --yes when you intend to apply this command.
[exit: 1]
```

The refusal offers the right routes (matching the daily-driver UX matrix) but
as one run-on paragraph, harder to scan than a what/why/next-steps layout.

```
$ conary install no-such-package-xyz --db-path <sandbox>/conary.db --root <sandbox>/root --dry-run
Error: Failed to resolve package 'no-such-package-xyz': Not found: Package 'no-such-package-xyz' not found in any repository
[exit: 1]
```

Three nested layers restate each other ("Failed to resolve X: Not found:
X not found"). apt's equivalent is one line: `E: Unable to locate package
no-such-pkg-xyz`.

```
$ conary install apps/conary/tests/fixtures/adversarial/corrupted/bad-checksum/output/adversarial-bad-checksum-1.0.0-1.ccs ... --yes
Error: CCS package authority verification and permanent CAS ingestion failed for <path>: verify streaming CCS v3 archive <path>: verify CCS v3 MANIFEST signature: CCS v3 package signer is not trusted: key_id=Some("conary-integration-fixtures")
[exit: 1]
```

A typed refusal (untrusted signer) rendered as a five-segment context chain
with the file path repeated twice and a raw Rust `Option` debug
(`Some("...")`) leaking into user output. The typed cause is exactly right;
the rendering launders it into parser prose — the opposite of the issue's
constraint.

### Generations

```
$ conary system generation list
No generations found. Run 'conary system takeover' to create the first.
[exit: 0]

$ conary system generation info 1
Error: Generation 1 does not exist
[exit: 1]
```

Generation commands currently split runtime-root ownership. `build`,
`publish`, `pending`, `activate`, backup/recovery, `gc`, and `recover` accept
`--db-path` and derive a runtime root from it. `list`, `info`, `export`,
`switch`, and `rollback` use fixed-path or artifact-path authority and do not
share that flag surface. This baseline captures only `list` and `info` on the
observation host; it does not claim a successful build/switch/rollback UX
capture. Separate generation-owner scoping must decide whether a supported-host
capture or any flag/help change is worth doing.

### system history

```
$ conary system history
Changeset history:
  [2] 2026-08-21 22:26:28 - Remove hello-conary-1.0.0 (Applied) [deferred] [publication-failed]
      deferred generation_publication pending: generation publication is pending Retry: conary system generation publish --yes.
  [1] 2026-08-21 22:26:28 - Install hello-conary-1.0.0 (Applied) [deferred] [publication-failed]
      deferred generation_publication pending: generation publication is pending Retry: conary system generation publish --yes.

Total: 2 changeset(s)
```

The surface exists and carries the right facts, but the rendering leaks the
same problems as the transaction paths: hand-rolled bracket tags
(`[deferred]`, `[publication-failed]`) outside the guarded vocabulary,
expected deferral framed as failure (the #534 problem in historical form),
and the retry sentence repeated verbatim per row. UI slice 5 owns the
rendering; #534 changes what the rows say about publication.

### Comparator capture (apt, same host)

```
$ apt-get install --dry-run cowsay
Reading package lists...
Building dependency tree...
Reading state information...
The following additional packages will be installed:
  libtext-charwidth-perl
Suggested packages:
  filters cowsay-off
The following NEW packages will be installed:
  cowsay libtext-charwidth-perl
0 upgraded, 2 newly installed, 0 to remove and 1 not upgraded.
Inst libtext-charwidth-perl (0.04-11build3 Ubuntu:24.04/noble [amd64])
Inst cowsay (3.03+dfsg2-8 Ubuntu:24.04/noble [all])
Conf libtext-charwidth-perl (0.04-11build3 Ubuntu:24.04/noble [amd64])
Conf cowsay (3.03+dfsg2-8 Ubuntu:24.04/noble [all])
```

What all three comparators provide that Conary's transaction paths do not
yet: a grouped what-changes summary (new / upgraded / removed), size totals
("Need to get X; after this operation Y will be used" / pacman's
"Total Installed Size"), and one stable closing count line.

## Findings

1. **TTY progress is visibly broken on the flagship flow.** Zero-length
   `0/0` bar frames flood single-package install under a pty
   (`apps/conary/src/commands/progress.rs`). The broken redraw dominates the
   transaction and leaves garbage in terminal scrollback.
2. **Two warning voices, one fact.** tracing `WARN` (internal formatting)
   and `WARNING:` (caps, hand-rolled) both print for generation-publication
   debt, and neither is `ui::warn`'s `warning:`. The happy path ends noisy.
3. **Two error prefixes.** `Error:` from `apps/conary/src/app.rs` versus
   `error:` from clap and `ui::error_line`; nested `anyhow` context chains
   restate the same fact up to three times; one path leaks
   `Some("...")` debug formatting.
4. **No transaction summary discipline.** Install/remove summaries omit
   sizes and disk-delta, dry-run and apply render differently, repeat
   install is an error rather than a calm no-op, and `remove` has neither
   `--dry-run` nor a pre-apply file summary.
5. **Three field-rendering idioms.** `ui::field`, `list --info`'s
   hand-aligned colons, and `ccs build`'s `=====` underlined headings (plus
   a non-ASCII `≤` in its chunking note).
6. **Behavior defect:** `query whatprovides <bare file path>` misses a
   provide that the typed `file(...)` form finds, contradicting its help.
7. **Generation commands expose split runtime-root conventions.** Some derive
   the runtime root from `--db-path`; list/info/switch/rollback use fixed-root
   authority. Empty-state guidance routes only to takeover. This is an
   observation, not an instruction to weaken fixed-root boot operations.
8. **Empty-state phrasing drifts** across adjacent states
   ("No packages to update" / "All packages are up to date";
   "No packages found." / "No packages found matching 'x'").

## Design evaluation

Design target for this pass: the operator should feel in control, never
nervous and never nagged. Mutations should have an honest
preview where that contract exists, capture explicit apply intent, and show a
proven reversal route where one exists. Missing preview behavior such as
`remove --dry-run` is work to add, not a current guarantee. No successful
transaction should end in duplicated or contradictory output; #534 separately
owns whether publication happens inside the apply operation.

Judged as a visual system, from the color pty captures (the audit findings
above are the mechanics; this section is the look). The diagnosis in one
sentence: Conary does not need a design system invented — `system init`
already speaks a complete one (bold green verb lines, the guarded tag column,
lowercase colored prefixes) — it needs that system to become the only dialect
on the daily path.

### Dialect inventory

Four visual dialects reach the user today:

1. The `ui::` language (verb lines, tag rows, `note:`/`warning:`/`error:`
   prefixes) — coherent, and the only one with a guard.
2. Raw `println!` prose (`Installing CCS package...`,
   `Removing package: hello-conary`, `WARNING:`, the hand-aligned
   `Name        :` fields) — unstyled, differently indented, differently
   voiced (gerund lines, noun-colon lines, caps warnings).
3. `tracing` log formatting (yellow ` WARN`, italic `key=value` fields)
   leaking into the user stream at the default level.
4. clap's help/error styling (bold/underline headings, lowercase `error:`).

### Color semantics

Observed on a TTY: green = completed verbs, `[ok]`, and the install spinner;
red = failures *and* the routine remove spinner; yellow = the leaked tracing
`WARN`; cyan = `note:`, `[info]`, and the status spinner; dim = `[skip]`/`[off]`
only; failures from `app.rs` — the highest-stakes output the tool has — render
with no color at all.

Target semantics, stated as a contract: green means success and nothing else;
red means failure and nothing else (the remove spinner loses red); yellow
means caution needing follow-up; cyan means guidance — the next command to
run; dim carries secondary metadata (arch, sizes, paths, timestamps), which
today does not exist as a layer, leaving flat same-weight walls like
`list --info`; bold carries identity (package names, field labels, verbs).
All of it stays `console`-routed so NO_COLOR and pipes degrade exactly as
today.

### Line-shape contract

Persistent user-visible output targets five shapes rooted in the existing
`ui::` vocabulary:

- verb line — `Installed nginx 1.27.2-3 (42 files, 5.4 MB)` with bold green
  verb and dim metadata; completion moments only.
- tag row — guarded tag at column nine, one item, one result.
- prefix line — `error:` / `warning:` / `note:`, lowercase, bold, colored;
  one fact per line.
- field line — two-space indent and bold label; shared colon alignment is a
  target renderer behavior, not a property of today's `ui::field_line`.
- heading — bold, ends with a colon, introduces tag rows or fields.

One additional transient shape never lands in scrollback: a single in-place
progress line per phase —

```
Downloading (2/2)  [==============>-----]  1.6 MB / 2.1 MB  1.1 MB/s  ~1s
```

— a bar only when the total is known (otherwise the spinner alone), bytes,
rate, and eta on downloads, a `(n/m)` count on multi-package phases, and on
completion the line is replaced by the phase's verb line. One live line per
phase; finished transactions read as history, never as leftover machinery.
This is the in-flight contract slices 1 and 3 implement, replacing today's
stacked spinners and zero-length bars.

The progress vocabulary and renderer must support the future parallel-download
experience tracked by #535: one aggregate phase line, a bounded row per active
download, and one queued-count row, all transient under a TTY and reduced to
phase lines for piped output. This audit owns that visual compatibility. #535
owns the concurrent scheduler, configuration, verification, ordering, and the
feature's end-to-end proof; this audit does not claim parallel fetching exists
today.

## Ranked UI slices

These slices change rendering and presentation, not package, publication,
query, download, or boot authority. Each lands separately under #132 unless a
focused issue is created first. Proof for every slice includes before/after
evidence in its issue or pull request, plus
`cargo test -p conary --test output_vocabulary_guard` and
`cargo test -p conary --test cli_daily_ux` passing; slices touching snapshot
surfaces also run `cargo test -p conary --test cli_output_snapshots`. Changes
to public claims also run `bash scripts/check-doc-truth.sh`.

1. **Fix TTY progress rendering** — flows: `install`, `update`, `remove`
   under a TTY. Stop rendering zero-length bars from
   `InstallProgress`/`RemoveProgress`/`UpdateProgress`; single-package
   operations get one spinner line that clears to the final summary; bars
   appear only with known non-zero totals. Keep the rendering primitive capable
   of a bounded aggregate-plus-worker layout so #535 does not need a second
   progress dialect. Proof adds a pty capture (`script -qec`) before/after.
2. **One warning/error voice** — flows: every mutation and refusal path.
   Route deferred or stuck generation-publication warnings once through
   `ui::warn`; stop duplicate tracing output on the default user path (tracing
   keeps it for logs); render application failures through `ui::error_line`
   (`error:`), give clap's parser-owned failures the same visible vocabulary,
   and collapse duplicated context segments so each error states the fact once
   and the remedy once. #534 owns publication behavior; this slice owns
   rendering. Parser and `app.rs` tests update in the same slice.
3. **Transaction summary block** — flows: `install --dry-run`,
   `install --yes`, `update`, `remove`. One shared summary renderer:
   what changes (install/upgrade/remove groups), version, arch, source
   format, file count, size, disk delta; identical between dry-run and
   apply except the closing line. This is the dnf5/apt/pacman visual-parity
   slice; changing repeat-install exit behavior is separate product work.
4. **Typed preflight rendering** — flows: signature/authority/preflight
   refusals. Render typed errors from their fields (no `{:?}`, no
   `Some(...)`, no repeated paths): one `error:` line naming the typed
   cause, indented fact lines, one `note:` remedy line. Typed identity remains
   typed; rendering must not collapse it into vague prose.
5. **Field/heading unification and empty-state phrasing** — flows:
   `list --info`, `ccs build`, `system history`, empty states across
   list/search/update. Route field and heading rendering through
   `ui::field`/`ui::heading`, preserve ASCII-only guarded tags, and use one
   phrasing pattern per empty state. `ccs build` must return typed summary
   data from `conary-core` for rendering at the `apps/conary` boundary rather
   than importing application UI into core; run the CCS feature proof.
   `system history` additionally drops its hand-rolled
   bracket tags for guarded vocabulary and stops repeating the retry
   sentence per row (its publication framing changes under #534).
   Snapshot tests updated in the same slice.
6. **Structured refusal layout** — flow: live-host mutation refusal.
   Keep the exact routes from the daily-driver UX matrix but lay them out
   as a short cause line plus `note:` next-step lines instead of one
   paragraph. `live_host_mutation_safety` expectations update in the same
   slice.

## Separate product and interaction work

The current captures expose behavior outside a visual pass. Those findings do
not inherit priority or implementation authority from this audit:

- `remove --dry-run` needs a focused behavior issue before implementation.
- The bare-path `whatprovides` miss is a query defect; fix it under a focused
  issue or correct the CLI help and UX matrix.
- Generation fixed-root and derived-root commands need generation-owner
  scoping if their help or routing changes. This audit does not require flag
  uniformity.
- #534 owns publication semantics and exact changeset intent.
- #535 owns parallel download scheduling, configuration, verification, and
  end-to-end delivery. Its progress UI extends the visual contract defined
  here rather than inventing a separate dialect.

Future interaction proposals also require their own issue and intent-boundary
review before implementation:

- **TTY confirmation prompt** — flows: `install`, `update`, `remove`,
    `autoremove` on an interactive terminal. dnf5, apt, and pacman all put
    their transaction table behind `Proceed? [y/N]`; Conary instead refuses
    and makes the operator retype with `--yes`. After slice 3 exists, an
    interactive confirm on a TTY (default no; `--yes` unchanged for
    scripts; non-interactive contexts keep the refusal, honoring
    `CONARY_NON_INTERACTIVE`) turns the refusal friction into the moment
    the summary actually gets read. Must not weaken the live-mutation
    intent boundary: the prompt is an explicit intent capture, not a
    bypass.
- **At-a-glance status screen** — flow: the bare `conary` invocation
    (or a new status subcommand named in its own slice). The bare
    invocation prints a version banner today.
    A small status screen — current generation, pending publication debt,
    enabled feeds and last sync age, owned vs adopted package counts — could
    provide a useful daily entry point. It must remain read-only over existing
    typed state and create no new authority.
- **Interactive variant picker** — flows: the ambiguous-variant
    refusals in `list --info`, `pin`, `unpin`, `remove`, `update` (today:
    refuse and instruct `--version`/`--arch`, per the daily-driver UX
    matrix). On a TTY, list the installed variants as numbered tag rows
    and let the operator pick; the selection is an explicit intent
    capture, exactly like the confirmation proposal. Non-interactive
    behavior is unchanged: the typed refusal with selector guidance
    remains, honoring `CONARY_NON_INTERACTIVE`. Selection never widens
    what the command may do — it only fills the selector the operator
    would have typed.

### Illustrative UI frame

This frame demonstrates layout only. It keeps today's deferred-publication
behavior and leaves #534's product decision outside the UX pass:

```
$ sudo conary install nginx --yes
Resolved nginx 1.27.2-3 from remi-fedora-44 (rpm)

Changes (2 install):
  install  nginx        1.27.2-3  x86_64  rpm  1.2 MB
  install  nginx-core   1.27.2-3  x86_64  rpm  864 kB  dependency

  2.1 MB to download · 5.4 MB on disk after

Installed 2 packages (49 files, 4.2 s)
warning: generation publication is pending for changeset 41
note: publish with 'conary system generation publish --yes'
```

### Open question for maintainers

Machine-readable CLI output (a JSON or porcelain mode on query/list/search
for human scripting) came up in review and is deliberately not a slice or
an issue yet: the repository contract makes `conary-agent-contract` the
authority for typed automation surfaces, with MCP as its adapter and never
a second authority. Whether a human-scripting output mode is an adapter of
that contract, a rendering of it, or out of scope is a maintainer decision
that should precede any issue.

### Deferred until after pre-alpha

Localization of user-facing output: a Fluent dependency is present
transitively, but Conary has no localization wiring. Adding that wiring is
deferred until the wording itself stabilizes — translating strings the UX
pass is about to rewrite would double the churn. Revisit at the
external-tester milestone.

### Follow-up once slice 1 lands

Record a scripted demo of the daily-driver flows for the README with a
reproducible terminal recorder (for example charmbracelet's `vhs`, driven by
a tape file checked in under `docs/`), so the recording regenerates from
source instead of rotting as a screen capture. Deliberately deferred: until
the TTY progress rendering is fixed, any recording of an install is an
anti-demo.

## Constraint compliance for later slices

- Guarded vocabulary and ASCII-only tags stay; slices 2, 4, 5, and 6 must
  keep `output_vocabulary_guard` green.
- No information regression: slice 2 keeps the tracing record; slice 3 keeps
  every transaction fact while changing layout; slice 4 keeps every typed
  error field visible.
- Typed errors remain typed: slice 4 changes rendering only.
