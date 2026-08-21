---
last_updated: 2026-08-21
revision: 1
summary: CLI UX audit of the daily-driver flows with exact captures, comparator notes against dnf5/apt/pacman, and the ranked beautification slice list for the UX pass
---

# CLI UX Audit: Daily-Driver Flows

This is the audit slice for the CLI UX and beautification pass
(issue #132). It captures what the `conary` CLI actually prints today on the
daily-driver paths, judges those captures against dnf5, apt, and pacman
conventions, and produces the ranked slice list. Each later slice lands
separately under the existing `apps/conary/src/ui/` conventions and the
vocabulary guard.

## Method

- Host: Ubuntu 24.04 container, x86_64, `conary 0.16.1` debug build from
  workspace source (rustc 1.98.0).
- Every capture is the exact stdout+stderr of one command against a throwaway
  root and database (`conary system init --db-path <sandbox>/conary.db`),
  with `NO_COLOR=1` for the piped captures. Sandbox temp paths are shown as
  `<sandbox>`.
- The local package used to drive install/remove is a two-file `hello-conary`
  1.0.0 CCS built with `conary ccs build --local-dev`.
- TTY behavior was captured under a real pty (`script -qec`), with control
  sequences transcribed.
- Comparator: `apt-get` captured on the same host; dnf5 and pacman judged
  from their documented, stable output conventions (transaction tables,
  size totals, `:: Proceed with installation? [Y/n]`-style confirmation).

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

Unlike every daily package command, the generation family accepts no
`--db-path`/`--root` (`system generation build --summary <S> --db-path <P>`
accepts the db but `switch`/`rollback`/`list` accept neither), so the family
cannot be exercised against a sandbox at all and its flag surface is
inconsistent with the rest of the CLI. Build/switch/rollback captures on this
host therefore stop at clap usage errors; a follow-up capture on a
Conary-managed host belongs to the generation slice below.

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
   (`apps/conary/src/commands/progress.rs`). This alone fails the "cannot
   look worse than what it replaces" bar.
2. **Three warning voices, one fact.** tracing `WARN` (internal formatting)
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
7. **Generation family diverges from CLI conventions** (no common
   `--db-path`/`--root` surface; empty-state guidance routes only to
   takeover).
8. **Empty-state phrasing drifts** across adjacent states
   ("No packages to update" / "All packages are up to date";
   "No packages found." / "No packages found matching 'x'").

## Ranked slice list

Each slice lands separately. Proof for every slice: before/after capture of
the named flow appended to this document, plus
`cargo test -p conary --test output_vocabulary_guard` and
`cargo test -p conary --test cli_daily_ux` passing; slices touching snapshot
surfaces also run `cargo test -p conary --test cli_output_snapshots`.

1. **Fix TTY progress rendering** — flows: `install`, `update`, `remove`
   under a TTY. Stop rendering zero-length bars from
   `InstallProgress`/`RemoveProgress`/`UpdateProgress`; single-package
   operations get one spinner line that clears to the final summary; bars
   appear only with known non-zero totals. Proof adds a pty capture
   (`script -qec`) before/after.
2. **One warning/error voice** — flows: every mutation and refusal path.
   Route the generation-publication warning once through `ui::warn`; stop
   duplicate tracing output on the default user path (tracing keeps it for
   logs); render top-level failures through `ui::error_line` (`error:`),
   making clap, `app.rs`, and `ui` agree; collapse duplicated context
   segments so each error states the fact once and the remedy once.
   `app.rs` unit tests update in the same slice.
3. **Transaction summary block** — flows: `install --dry-run`,
   `install --yes`, `update`, `remove`. One shared summary renderer:
   what changes (install/upgrade/remove groups), version, arch, source
   format, file count, size, disk delta; identical between dry-run and
   apply except the closing line; repeat-install becomes a calm no-op
   sentence with exit 0. This is the dnf5/apt/pacman parity slice.
4. **`remove --dry-run` and removal preview** — flow: `remove`. Add the
   flag (aligning with install/update/autoremove and the refusal text) and
   show the files/config summary before apply. Extends
   `cli_daily_ux` with a focused test.
5. **Typed preflight rendering** — flows: signature/authority/preflight
   refusals. Render typed errors from their fields (no `{:?}`, no
   `Some(...)`, no repeated paths): one `error:` line naming the typed
   cause, indented fact lines, one `note:` remedy line. Typed identity
   stays typed per the issue constraints.
6. **Fix `whatprovides` bare-path lookup** — flow: `query whatprovides`.
   Behavior defect: bare `/usr/bin/...` must resolve like
   `file(/usr/bin/...)` or the help and matrix claims must change; the fix
   direction is resolution, not documentation. Focused query test.
7. **Field/heading unification and empty-state phrasing** — flows:
   `list --info`, `ccs build`, empty states across list/search/update.
   Route field and heading rendering through `ui::field`/`ui::heading`,
   ASCII-only text, and one phrasing pattern per empty state. Snapshot
   tests updated in the same slice.
8. **Generation family conventions** — flows: `system generation
   list/build/switch/rollback`. Give the family the same `--db-path`
   (and where meaningful `--root`) surface as the rest of the CLI or
   document the fixed-path contract in help text; make empty-state guidance
   route to generation build where takeover is not the next action; capture
   the build/switch/rollback experience on a Conary-managed host as part of
   the slice's proof.
9. **Structured refusal layout** — flow: live-host mutation refusal.
   Keep the exact routes from the daily-driver UX matrix but lay them out
   as a short cause line plus `note:` next-step lines instead of one
   paragraph. `live_host_mutation_safety` expectations update in the same
   slice.

## Constraint compliance for later slices

- Guarded vocabulary and ASCII-only tags stay; slices 2, 5, 7, and 9 must
  keep `output_vocabulary_guard` green.
- No information regression: slice 2 keeps the tracing record; slice 3 only
  adds fields; slice 5 keeps every typed field visible.
- Typed errors remain typed: slice 5 changes rendering only.
