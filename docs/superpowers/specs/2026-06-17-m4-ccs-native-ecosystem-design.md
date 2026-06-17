# M4 CCS Native Ecosystem Design

**Date:** 2026-06-17
**Status:** Approved umbrella design; child specs and plans required before implementation
**Parent roadmap:** `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
**Prerequisite milestone:** M0-M3 packaging work is closed on `main`.

## Purpose

M4 turns the M0-M3 packaging loop into the start of a real CCS-native package
ecosystem. It answers what a native CCS package is, how a maintainer authors
one, how Remi publishes it as a first-class artifact, and how Conary proves the
result across its supported host targets.

The milestone is intentionally ordered around authority first:

1. M4a defines the CCS v2 native package contract.
2. M4b builds the native authoring, lint, build, and local test workflow on top
   of that contract.
3. M4c makes Remi publish native CCS packages without legacy conversion
   semantics.
4. M4d makes supported distro adapter facts explicit and fixture-tested.
5. M4e closes the milestone with a representative proof corpus.

M4a is the keystone. Later slices can be designed with the full M4 architecture
in mind, but they must not implement behavior that depends on fields, lifecycle
authority, or compatibility semantics that M4a has not defined.

## Current Repo Facts

The active CCS native ecosystem roadmap already names this sequence and keeps
public support limited to Fedora 44, Ubuntu 26.04, and Arch. M4 does not expand
that list.

The current CCS implementation has useful native package pieces, but the v1
contract is not yet a complete install-time authority:

- `crates/conary-core/src/ccs/binary_manifest.rs` uses binary format version
  `1` and carries only a subset of the TOML manifest.
- `crates/conary-core/src/ccs/package.rs` documents that CBOR-only package
  reads default several TOML-only fields.
- `crates/conary-core/src/ccs/archive_reader.rs` overlays TOML-only fields when
  both CBOR and TOML are present.
- `docs/specs/ccs-format-v1.md` documents package kind/type semantics that the
  current `Package` struct in `crates/conary-core/src/ccs/manifest.rs` does not
  fully implement.
- `crates/conary-core/src/ccs/manifest.rs` is large enough that M4a child work
  should name the ownership boundary for v2 schema and validation rather than
  adding all new behavior to the existing file.
- Current Remi release upload paths already run
  `verify_static_artifact_publish_eligibility`; M4c native intake must preserve
  that gate instead of creating a parallel trust shortcut.

Those facts do not make v1 a compatibility obligation. They are the reason M4a
must define v2 authority clearly and then delete, narrow, or fail closed on old
defaulting behavior where possible.

## Scope

M4 owns the CCS-native package ecosystem surface:

- the authoritative native package contract;
- maintainer-facing native authoring, lint, build, and local test workflows;
- Remi native CCS intake, verification, indexing, staging, promotion, and
  serving;
- supported-target adapter facts for Fedora 44, Ubuntu 26.04, and Arch;
- a proof corpus that demonstrates the contract, workflow, publication path,
  and adapter facts together.

## Non-Goals

- Do not add public support for additional distro targets.
- Do not create a Remi-first shortcut that bypasses the local maintainer loop.
- Do not weaken M2 publish hardening, artifact-form publish checks, accepted
  build attestation requirements, static publish trust, Remi release upload
  trust, or recorded-draft refusal behavior.
- Do not turn regex, heuristic, or conversion-only evidence into authoritative
  native package truth.
- Do not make the core package contract depend on live host I/O.
- Do not preserve TOML-only install-time authority unless M4a deliberately
  proves that boundary is correct.
- Do not carry broad v1 package compatibility as a product requirement.
- Do not write all child slice designs or plans from this umbrella design.

## Cross-Slice Invariants

- **Authority before ergonomics:** M4a defines install-time authority before M4b
  makes it convenient.
- **Native is not converted:** Native CCS packages are authored and validated as
  native packages. Legacy conversion can remain a bridge, not the truth source.
- **Support remains narrow:** Fedora 44, Ubuntu 26.04, and Arch are the only
  public targets in this milestone.
- **M2 gates remain gates:** Hermetic evidence, accepted build attestations,
  artifact-form publish checks, static publish trust, Remi upload trust, and
  recorded-draft refusal keep their hardening role.
- **Core validation is host-I/O-free:** Live host facts belong in callers,
  fixtures, or profile layers, not in the core contract validator.
- **Failure is visible and non-silent:** Missing authority, unknown lifecycle
  effects, unsupported targets, and stale compatibility assumptions fail with
  actionable diagnostics instead of defaulting into a plausible package.
- **Child slices own details:** Each implementation slice needs its own design,
  review pass, implementation plan, verification list, and docs-audit update.

## Layered Architecture

### Contract Layer

`conary-core` owns the CCS v2 native package contract. This layer defines the
authoritative install-time representation, validation rules, and migration
boundary. It also defines which data is signed/binary authority, which data is
human-source/advisory, and which data is provenance-only.

### Authoring Layer

The CLI authoring workflow consumes the contract. It may generate templates,
infer useful defaults, explain diagnostics, and provide guided local tests, but
the final package must verify independently of the authoring tool that created
it.

### Publication Layer

Remi consumes native CCS packages through native intake and verification. It
indexes native identity, dependencies, capabilities, components, trust state,
provenance, and public-readiness data without treating a legacy conversion as a
native package. M4c child design must name the owning Remi surface for native
intake, with `apps/remi/src/server/native_intake.rs` or a dedicated
`apps/remi/src/server/native/` subtree as the preferred starting points. Native
intake may add native-specific checks, but it must reuse the shared
`publish_gate` chain for build-attestation verification, signer authority,
output identity, and origin-class refusal.

### Profile Layer

Supported target profiles provide data and fixture validators for Fedora 44,
Ubuntu 26.04, and Arch. They validate lifecycle declarations, service/config
facts, tmpfiles/sysusers facts, package-manager authority boundaries, and public
support identifiers. They do not define the package contract.

### Proof Layer

The proof corpus exercises the contract, authoring workflow, publication path,
and target profiles together. It is the milestone closeout layer, not a place
to patch missing authority from earlier slices.

## Data Flow

The intended M4 flow is:

```text
source/package template/recipe
  -> authoring workflow
  -> CCS v2 package
  -> contract validation
  -> local install/test and/or Remi native intake
  -> profile validation
  -> fetch/install proof
  -> proof corpus docs
```

Later layers may enrich diagnostics, but they must not silently repair missing
contract authority.

## Slice Map

### M4a: CCS v2 Native Package Contract

**Owns:** authoritative install-time package representation.

**Decides:** binary/TOML boundary, package kind model, dependency/provide model,
file/component authority, lifecycle declarations, config behavior, policy
metadata, provenance/trust fields, supported-target validation hooks, and the
v1 migration stance.

**Acceptance gate:** a native package can round-trip through the v2 contract
without silently losing install-time authority. Missing, defaulted, or
unsupported authority fails visibly. The M4a child design must name the module
that owns v2 schema and validation before implementation starts, with
`crates/conary-core/src/ccs/v2/` as the preferred ownership boundary unless the
child design proves a narrower sibling module is safer. It must also state which
types, validation rules, archive-reader bridges, and temporary migration
scaffolding stay in or move out of `manifest.rs`.

**Child spec questions:**

- What data is signed/binary install-time authority?
- What data remains human-source, advisory, or provenance-only?
- How are package kinds, groups, redirects, components, lifecycle effects, and
  dependency/provide types represented?
- How does archive verification prevent drift between human-authored metadata
  and install-time authority?
- Which v1 reader paths are deleted, narrowed to migration scaffolding, or made
  fail-closed?
- How are current v1 test fixtures inventoried, regenerated to v2, retained as
  explicit legacy-only reader coverage, or deleted without breaking the suite
  during the transition?
- Should the legacy conversion pipeline emit v2-native packages after M4a
  lands, or continue producing explicitly legacy v1 packages behind a documented
  v1-to-v2 bridge until a child spec proves conversion fidelity?
- How does the v2 schema affect `ManifestProvenance`,
  `BuildAttestationEnvelope`, and content-identity hashing, including tests that
  prove signature-related fields are excluded from stable package identity?
- If the child design does not use the preferred `ccs/v2/` boundary, what
  narrower module boundary prevents `manifest.rs` from absorbing the new v2
  behavior?

### M4b: Native Authoring, Build, Lint, And Local Test Workflow

**Owns:** maintainer ergonomics for creating correct native packages.

**Builds on:** M4a contract authority.

**Decides:** templates, `conary ccs init` improvements, lint diagnostics, local
build/test flow, recipe/source integration, publication-readiness checks, and
the minimum pleasant path for a first native package.

**Acceptance gate:** a maintainer can create, lint, build, and locally test a
minimal native CCS package with actionable diagnostics for missing authority or
unsafe lifecycle behavior.

**Child spec questions:**

- Which package templates are in scope for the first authoring slice?
- What can be inferred safely, and what must be explicit?
- How does lint distinguish contract errors, publication-readiness warnings,
  profile issues, and style guidance?
- How does local test reuse try-session or existing install proof without
  creating another hidden install path?

### M4c: Remi Native CCS Publication

**Owns:** Remi as a first-class native CCS publication surface.

**Builds on:** M4a contract authority and enough M4b output to publish real
native package artifacts.

**Decides:** native intake, verification, staging, promotion, indexing,
search/list surfaces, trust/signing policy, owner module, storage/migration
semantics, and client fetch/install proof.

**Acceptance gate:** a native CCS package can be published to a local Remi,
verified, indexed, fetched, and installed without legacy conversion semantics.
Native intake reuses `verify_static_artifact_publish_eligibility` and the shared
`publish_gate` trust path for build-attestation verification, signer authority,
output identity, and origin-class checks. It must not reuse conversion-shaped
metadata rows or upload-prefixed source checksum semantics for first-class
native packages unless the child design explicitly defines a migration boundary
and proves no native package is treated as a legacy conversion.

**Child spec questions:**

- What metadata must Remi index from native packages?
- Which verification failures block intake, staging, promotion, or serving?
- How does Remi expose trust state without depending on unverified operator
  state?
- Which Remi module owns native intake, and which legacy conversion/publication
  paths are reused, wrapped, or left untouched?
- How does native intake execute `verify_static_artifact_publish_eligibility`
  with the repository's trusted signers before indexing or serving a package?
- What client proof demonstrates fetch and install of a Remi-served native
  package?

### M4d: Supported Distro Adapter Profiles

**Owns:** supported target facts and validators.

**Builds on:** the M4a contract. It may design after M4a is clear, but it must
not invent package fields to compensate for an incomplete contract.

**Decides:** public support IDs, internal family slugs, lifecycle support,
service/config/tmpfiles/sysusers facts, package-manager authority boundaries,
fixture validators, and docs/help wording for supported targets.

**Acceptance gate:** Fedora 44, Ubuntu 26.04, and Arch facts sit behind one
profile surface, are fixture-tested, and do not require live host I/O in core
validation.

**Child spec questions:**

- What is the stable public support ID for each target?
- What internal family data is needed without leaking unsupported targets into
  public support?
- Which lifecycle declarations can each target validate?
- Which fixtures prove target facts without relying on the developer host?

### M4e: Native CCS Proof Corpus

**Owns:** end-to-end proof that M4 works.

**Builds on:** M4a through M4d.

**Decides:** representative packages, negative fixtures, docs examples,
verification commands, and milestone closeout criteria.

**Acceptance gate:** the corpus proves the package contract, authoring loop,
Remi publication path, and supported-target profiles with honest docs. The
corpus must be reproducible from scratch and must re-run the focused
verification command sets from M4a through M4d to prove the final corpus did not
regress earlier slice guarantees.

**Child spec questions:**

- Which package examples prove the simple CLI, daemon/service, library/devel
  split, config/noreplace, source-built provenance, and Remi-published native
  package paths?
- Which negative fixtures prove missing authority, unsupported target facts,
  unsafe lifecycle declarations, and publication refusal?
- Which docs examples graduate from design-only to supported usage?

## Failure Behavior

M4 should fail closed where authority matters:

- A v2 package missing install-time authority fails validation.
- A package that only works because TOML-only fields were defaulted fails v2
  validation.
- Unknown lifecycle effects fail until represented declaratively or explicitly
  rejected.
- Unsupported target claims fail profile validation.
- Remi intake refuses artifacts whose contract, signature, provenance, or
  publication policy cannot be verified.
- Authoring diagnostics explain the missing field, why it matters, and the
  smallest acceptable fix.

M4 does not promise broad v1 package compatibility. Existing v1 reader paths may
remain only as temporary migration scaffolding while M4a lands and repo fixtures
move forward. New native packages target CCS v2. v1 packages are not
first-class M4-native packages. Compatibility shims should be deleted, narrowed,
or made explicitly legacy-only unless a child plan proves they are required for
current fixtures or current code migration. M4a must inventory existing v1 CCS
test fixtures and classify each one before enforcing v2 failure behavior:
regenerate to v2, keep as explicit legacy-reader coverage, or delete as obsolete
coverage.

## Testing Strategy

M4 testing should scale with the slice:

- M4a starts in `conary-core` with round-trip tests, negative validation tests,
  archive verification tests, and migration/defaulting tests.
- M4b adds CLI tests for init/lint/build/local-test flows and diagnostic
  wording that matters to maintainers.
- M4c adds Remi intake, index, fetch, and install proof tests.
- M4d adds fixture-backed profile validators for Fedora 44, Ubuntu 26.04, and
  Arch.
- M4e adds the proof corpus plus negative fixtures that protect the milestone
  boundaries.

Each child plan must name the focused verification commands it will run. Public
claim, command-help, route, or assistant-facing changes must also update the
docs-audit and feature-coherency ledgers when the touched paths are covered by
those systems.

Any child plan that touches CCS package metadata, publish readiness, authoring,
Remi intake, or conversion output must include focused M2 publish-gate
regression proof in its verification list:

```bash
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
```

## Review And Execution Flow

This umbrella design is not an implementation plan. It records purpose,
sequence, invariants, architecture, and gates.

The expected flow for each child slice is:

1. Write the child slice design.
2. Run external and local agentic reviews.
3. Lock the child design.
4. Write the child implementation plan.
5. Run external and local agentic plan reviews.
6. Lock the plan.
7. Implement in a focused `/goal`.
8. Verify, merge, commit, push, and clean up before starting the next slice.

Do not write all child designs now. The next child packet is M4a: CCS v2 native
package contract.

## Umbrella Verification Gates

For this docs-only umbrella design:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
! rg -n "TB[D]|TO[D]O" docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md
! rg -n "(?i)\\b([c]entos|[r]hel|[d]ebian|[o]pensuse|[a]lpine|[t]umbleweed)\\b" docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md
rg -n "Fedora 4[4]|Ubuntu 26\\.[0]4|\\bArc[h]\\b" docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md
git diff --check
cargo fmt --check
```

If the docs-audit ledger check reports this umbrella design as missing, add a
reviewed ledger row for
`docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md` before
committing. Child slices add their own code, integration verification, and
ledger rows.

## First Follow-Up Packet

The first follow-up is the M4a CCS v2 native package contract design. It should
inspect the current v1 manifest, binary manifest, package archive reader, CCS
format docs, and package builder before proposing fields or migration behavior.
Its main output is a reviewed v2 contract design that makes install-time
authority explicit, narrows legacy defaulting, and gives M4b a contract that can
be generated, linted, built, and locally tested without weakening trust.
