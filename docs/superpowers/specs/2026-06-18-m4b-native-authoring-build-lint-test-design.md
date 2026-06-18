# M4b Native Authoring, Build, Lint, And Local Test Design

**Date:** 2026-06-18
**Status:** Locked for implementation after DeepSeek, Gemini, and local agentic review.
**Parent umbrella:** `docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md`
**Prerequisite:** M4a CCS v2 native package contract is implemented and merged.
**Scope:** M4b only: maintainer-facing native CCS authoring, lint, v2 build,
and local test workflow.

## Purpose

M4b turns the M4a CCS v2 contract into a usable maintainer loop. A maintainer
should be able to create a minimal native package, understand what is missing,
build a signed v2 `.ccs`, verify it, and locally test the install path without
hand-assembling v2 fixtures or learning every internal authority field first.

The slice is deliberately narrow. M4b proves one boring first-package path
before adding richer templates, Remi-native publication, or target-profile
facts.

## Contract Stance

CCS v2 is the native authoring target. M4b should not make v1 the default
"native" output path and should not treat old TOML-to-v1 behavior as the
strategic package workflow. Existing v1 build behavior may remain as an
explicit legacy or compatibility target while implementation migrates, but the
M4b acceptance path must generate and verify v2 package authority.

Authoring tools may infer boring source metadata, but they must not invent
install-time truth. Anything that changes install behavior, host mutation,
lifecycle behavior, target compatibility, publication trust, or package
identity must be represented in signed v2 authority or rejected before build.

## Current Repo Facts

M4a provided the v2 verification side of the contract, not a complete native
authoring loop. The M4b plan should treat current state honestly:

| Available before M4b | Missing until M4b builds it |
| --- | --- |
| `conary ccs init` writes a minimal `ccs.toml` and can infer package metadata from Cargo, Node, or Python project files. | `ccs init --template minimal-file` and a reusable template module do not exist yet. |
| `conary ccs build` parses `ccs.toml`, scans a source directory through the existing builder, builds a `BuildResult`, and writes legacy CCS output. | `ccs build --format v2`, v2 authoring projection, local-dev signing flow, and CLI wiring to `write_v2_ccs_package` do not exist yet. |
| `conary ccs verify` uses `verify_package`, which now verifies v2 authority through M4a's exact-byte signed `MANIFEST` path. | Maintainer-facing lint output for v2 authoring diagnostics does not exist yet. |
| `conary ccs install` distinguishes v2 authority and requires successful signature verification before `parse_verified_v2`. | `conary ccs test` and its isolated root/database dry-run wrapper do not exist yet. |
| `conary ccs shell` and `conary ccs run` are installed-package runtime helpers. | A package-file local proof command is still needed; shell/run should not be stretched into that role. |
| `crates/conary-core/src/ccs/v2/` contains v2 schema, validation, diagnostics, reading, identity, and test fixtures. | `crates/conary-core/src/ccs/v2/authoring.rs` or equivalent v2-owned projection code does not exist yet. |
| `write_v2_ccs_package` can emit signed v2 package fixtures from complete v2 authority and payload maps. | The CLI does not yet construct complete `AuthorityDocumentV2` from maintainer authoring input and build output. |

This existing foundation avoids the need for a second installer or a Remi-first
workflow, but M4b must build the authoring, lint, v2 build, and local test
surface deliberately.

## Non-Goals

- Do not implement Remi native publication, intake, indexing, staging, or
  promotion.
- Do not add Fedora 44, Ubuntu 26.04, or Arch target-profile facts.
- Do not add service, tmpfiles, sysctl, or other profile-dependent templates as
  first-slice acceptance requirements.
- Do not make `config-noreplace` part of the first required smoke path.
- Do not rewrite every CCS command or retire all legacy build behavior in this
  slice.
- Do not allow unsigned v2 packages to install, publish, or pass local test.
- Do not allow local-dev signed packages to pass release publish gates or Remi
  signer allowlists.
- Do not turn recipe shell phases, heuristics, or conversion evidence into
  trusted native package authority.

## Required First Package Path

M4b's required maintainer smoke path is:

```text
conary ccs init --template minimal-file
  -> conary ccs lint
  -> conary ccs build --format v2 --local-dev
  -> conary ccs verify
  -> conary ccs test --dry-run
```

The exact `--local-dev` flag name can be finalized in the implementation plan,
but the mode must be explicit in command help, command output, and test trust
policy. A maintainer should never wonder whether a package was signed for local
iteration or release publication.

The first template is a minimal file or CLI package. It has one default
component, one or more regular files, complete file metadata, complete package
identity, complete provenance authority, and no lifecycle declarations that
require M4d target-profile facts.

The `config-noreplace` template family should be named in code/docs as a
planned follow-up template category, but the first M4b implementation gate does
not need to build or install it. Lint should still recognize config-related
authoring fields well enough to label them supported, incomplete, unsafe, or
deferred instead of silently accepting ambiguous config authority.

## Authoring Format

M4b should keep `ccs.toml` as the maintainer-facing authoring filename, but make
the M4b path v2-native. This is not a promise to preserve old v1 defaults as the
strategic native workflow; it is a filename continuity choice so maintainers do
not need to learn two manifest names while the schema moves forward.

`ccs init --template minimal-file` should write explicit v2 identity fields for
the first package path:

```toml
[package]
name = "hello"
version = "0.1.0"
release = "1"
kind = "package"
```

`version` is the upstream version and `release` is the package release. M4b
should not silently split `1.0.0-1` strings or infer a missing release for v2
builds. If a legacy `ccs.toml` lacks release or kind data, `ccs lint` and
`ccs build --format v2` should produce actionable diagnostics instead of
guessing identity authority.

The shared `CcsManifest` parser may gain small v2 authoring fields such as
`release` and `kind`, but v2 projection rules belong in
`crates/conary-core/src/ccs/v2/authoring.rs` or equivalent. `manifest.rs`
should not become a second v2 validator or a large conversion engine.

Package authoring is the only required M4b kind. Group and redirect authoring
may remain follow-up work unless implementation support falls out naturally
without widening the first smoke path.

## Ownership Boundary

M4b should keep command entrypoints thin and put reusable authoring logic behind
focused modules.

Recommended ownership:

- `apps/conary/src/commands/ccs/init.rs`: command entrypoint for manifest
  creation; delegates template generation.
- `apps/conary/src/commands/ccs/templates.rs`: maintainer template generation
  for `minimal-file` and future template families.
- `apps/conary/src/commands/ccs/lint.rs`: CLI lint orchestration, output mode,
  severity policy, and conversion from v2 diagnostics to maintainer-facing
  findings.
- `apps/conary/src/commands/ccs/build.rs`: existing command entrypoint; gains
  explicit v2 output support and delegates v2 authority construction/package
  writing.
- `apps/conary/src/commands/ccs/test.rs`: package-local proof command that
  reuses verification and dry-run install behavior.
- `crates/conary-core/src/ccs/v2/authoring.rs` or an equivalent v2-owned core
  module: construction of `AuthorityDocumentV2` from authoring/build inputs.
- `crates/conary-core/src/ccs/manifest.rs`: parsing for small v2 authoring
  fields only; no broad v2 conversion or validation ownership.
- `crates/conary-core/src/ccs/v2/validation.rs`: remains the contract
  validator. M4b may wrap its diagnostics but must not fork contract rules in
  the CLI.

The core boundary matters: templates and CLI text are authoring ergonomics;
authority projection and validation are contract-adjacent and belong near
`ccs/v2`.

V2 authoring projection should consume the existing builder scan/classification
output, such as `BuildResult`, rather than duplicating source-tree traversal,
hashing, file typing, or component accounting in a parallel scanner.

## Template Semantics

`minimal-file` should generate a small, editable `ccs.toml` that can round-trip
into complete v2 authority after source scanning. It should be
useful for a package that installs a binary, script, or small file tree.

Safe inference:

- package name and version from explicit CLI flags or supported project files;
- summary, license, homepage, and repository when project metadata provides
  them;
- file path, hash, size, mode, component, and type from the existing builder's
  source scan during build;
- one default component for the simple package path;
- local development provenance fields when the build command is explicitly in
  local-dev mode.

Explicit authority required:

- host mutation and lifecycle behavior;
- service, tmpfiles, sysctl, alternatives, users, groups, and directories;
- target compatibility claims;
- release/publish trust assumptions;
- config merge and noreplace behavior once that template family is implemented;
- dependency/provide entries that cannot be derived from reliable package
  metadata.

## Lint Semantics

`conary ccs lint` is the maintainer-facing diagnostic surface. It should produce
human output by default and support JSON output when the implementation plan
chooses the exact flag shape.

Lint findings are classified into four buckets:

- **Contract errors:** M4a v2 validation failures. Build fails.
- **Publication-readiness errors or warnings:** missing provenance, missing
  signing material, or trust facts that would block later publish.
- **Profile-deferred findings:** lifecycle/service/tmpfiles/sysctl declarations
  that require M4d target-profile facts.
- **Style guidance:** helpful hints that do not block local build.

In M4b, profile-deferred findings do not fail `ccs lint` by themselves, but
they block `ccs build --format v2` and `ccs test` when the unsupported behavior
would enter signed authority. The first `minimal-file` template should produce
no profile-deferred findings. M4d may lower or reclassify these findings only
after real target-profile facts exist.

Diagnostics should include:

- stable code;
- severity;
- field/path when available;
- concise message;
- smallest acceptable fix;
- whether the finding blocks lint, build, local test, or future publication.

Lint must prefer actionable v2 authority diagnostics over raw parser or stack
errors.

## Build Semantics

`conary ccs build --format v2` should produce a signed v2 `.ccs` package for
the first package path. The implementation plan can decide the exact flag name
if the existing `--target` terminology is retained, but the UX must make v2
native output explicit and discoverable.

Signing rules:

- unsigned v2 output is invalid for install, publish, and local test;
- if a key is provided, build signs with that key;
- if no release key is provided, M4b may offer an explicit local-dev signing
  mode; implicit release signing is not allowed;
- local-dev keys may live under the user's Conary state directory, but their
  public keys must not be added to static-repo release trust or Remi signer
  allowlists;
- generated local-dev trust policy for `ccs test` must be local to the test
  workspace and should contain only the selected local-dev public key.

Local-dev provenance is still signed authority, but it is not release evidence.
The v2 authority should set `origin_class = "native-built"` and
`hardening_level = "host"` for the first local build path. Because the current
v2 validator requires a non-empty `hermetic_evidence_hash`, M4b should write a
hash of a local-dev evidence document and label it in diagnostics as local host
evidence, not hermetic proof. A local-dev package should not carry a release
`BuildAttestationEnvelope`; artifact-form publish remains refused because M2
requires hermetic hardening plus an accepted build attestation.

Build should run lint or equivalent validation before writing final output.
Validation failures stop before writing a package that looks usable.

The implementation plan should add negative tests proving local-dev output is
installable/testable only with the explicit local test trust policy and remains
rejected by static publish and Remi release upload gates.

## CLI Surface And Risk

M4b should name the command surface in the Clap layer so command help, command
risk, tests, and docs stay aligned:

| Command | Expected risk | Notes |
| --- | --- | --- |
| `conary ccs init --template minimal-file` | `LocalStateMutation` | Writes or overwrites a local authoring manifest. |
| `conary ccs lint` | `ReadOnly` | Reads authoring input and reports diagnostics. |
| `conary ccs build --format v2 --local-dev` | `LocalStateMutation` | Writes package output and may create/read local-dev signing material. |
| `conary ccs verify` | `ReadOnly` | Verifies package bytes and signatures. |
| `conary ccs test --dry-run` | `LocalStateMutation` or stricter dry-run equivalent | Creates an isolated root/database/trust workspace, but must not mutate live root or default user database. |

If the implementation keeps existing `--target` terminology instead of adding a
new `--format` flag, the help text must still make v2-native output explicit.

## Local Test Semantics

`conary ccs test` should not introduce another install engine. It should:

1. verify the package with the selected trust policy;
2. run the existing dry-run CCS install path against an isolated root/database;
3. surface lint/verify/install diagnostics in maintainer language;
4. leave no installed package state in the developer's real database or root.

The first implementation can require `--dry-run` or make dry-run the only
supported behavior. A later slice may choose to add a stronger isolated install
test, but that must still reuse existing install transaction plumbing.

Dependency checks should stay enabled by default. The required `minimal-file`
template should avoid external dependencies so the smoke path passes against an
empty isolated database. If a maintainer adds dependencies before M4b grows
database snapshot support, `ccs test --dry-run` should report unresolved
dependencies clearly instead of disabling dependency checks silently. Any future
dependency-bypass flag must be explicit and out of the first acceptance path.

`ccs shell` and `ccs run` remain installed-package runtime helpers in M4b. They
can be mentioned as future loop integrations, but they are not the local
package-file proof command.

## Error And Failure Behavior

M4b should fail visibly and early:

- missing v2 authority fails lint/build/test with a field-level diagnostic;
- lifecycle declarations without target-profile support are profile-deferred in
  lint and build/test-blocking before signed authority is written;
- unsigned v2 output cannot pass local test;
- local-dev signed v2 output cannot pass release publish gates;
- v1 packages are legacy inputs, not successful M4b native outputs;
- build must not repair missing authority using `MANIFEST.toml` debug fields;
- package-local test must not mutate the live host or default user database.

When multiple phases fail, command output should prefer the earliest useful
maintainer action. For example, a malformed manifest should not continue into a
signature failure that hides the field the maintainer must fix.

## Testing Strategy

The main proof should be a new focused integration test, likely
`apps/conary/tests/packaging_m4b.rs`, that exercises:

```text
ccs init --template minimal-file
ccs lint
ccs build --format v2 --local-dev
ccs verify
ccs test --dry-run
```

Focused unit tests should cover:

- template generation produces the expected minimal authoring manifest;
- v2 authoring projection creates complete `AuthorityDocumentV2`;
- lint maps contract errors, publication-readiness findings,
  profile-deferred findings, and style guidance to stable diagnostics;
- build refuses v2 output without signing material or explicit local-dev
  signing;
- build reuses existing builder scan output for file hashes, modes, file types,
  and component totals;
- local test reuses verify plus dry-run install behavior;
- command-risk classification keeps lint/verify read-only and classifies init,
  build `--format v2 --local-dev`, and `ccs test --dry-run` as local-state or
  dry-run-only isolated mutations, never active-host mutations;
- local-dev signed output is rejected by static publish and Remi release gates.

Regression proof should include:

```bash
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4b
cargo test -p conary-core ccs::v2
cargo test -p conary --lib commands::ccs
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
```

The implementation plan must carry `cargo fmt --check`, `cargo clippy
--workspace --all-targets -- -D warnings`, docs checks, and command-help tests
as final gates; the focused tests above remain the M4b-specific proof.

## Documentation And Audit Updates

M4b implementation should update:

- `docs/modules/ccs.md` for the native v2 authoring loop;
- `docs/modules/test-fixtures.md` for the M4b template/smoke fixture;
- `docs/llms/subsystem-map.md` when command ownership changes;
- `docs/modules/feature-ownership.md` when "look here first" paths or proof
  commands change;
- docs audit inventory and ledger for this design, the M4b plan, and touched
  docs.

Command help examples should mention only the first supported v2 package path
and must not imply Remi-native publication or profile-backed lifecycle templates
have landed.

Before lock-in and before implementation commit, rerun the docs-audit and
coherency checks required by `AGENTS.md`, including:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
git diff --check
```

## Acceptance Gate

M4b is complete when a maintainer can create, lint, build, verify, and locally
dry-run-test one minimal v2-native package with actionable diagnostics for
missing authority or unsafe/deferred behavior.

The first package path must generate a signed v2 package and verify/install it
through the same v2 trust boundary M4a established.

## Follow-Up Boundaries

- `config-noreplace` becomes the next authoring template family after the first
  v2 maintainer loop is proven.
- service/tmpfiles/sysctl templates wait for M4d supported-target profile facts.
- Remi publication waits for M4c native CCS publication.
- Representative config, daemon, library/devel, and Remi-published packages
  belong in the M4e proof corpus.
