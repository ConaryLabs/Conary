# M4a CCS v2 Native Package Contract Design

**Date:** 2026-06-17
**Status:** Draft child design for review
**Parent umbrella:** `docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md`
**Scope:** M4a only: define the CCS v2 native package contract before authoring,
Remi native publication, supported-target profiles, or proof corpus work.

## Purpose

M4a defines the first real native CCS package contract. It decides what a
native package is, what data is authoritative at install time, how validation
fails when authority is missing, and where v2 schema/validation ownership lives
inside `conary-core`.

The output of M4a is a reviewed contract design and a later implementation
plan. It is not a broad compatibility program for CCS v1.

## Contract Stance

CCS v2 is the native contract. CCS v1 is migration debris.

Conary has no public CCS v1 package ecosystem yet, so M4a does not preserve v1
installability as a user-facing compatibility promise. Existing v1 code paths
matter only because they are current repo state that must be migrated, deleted,
or kept briefly as explicit legacy-rejection/test scaffolding.

M4a should not ask "how do we keep v1 working?" It should ask:

- What must v2 carry as install-time authority?
- Which v1 fields/defaulting paths are unsafe and should fail closed?
- Which fixtures must be regenerated to v2?
- Which v1-only tests become legacy rejection tests?
- Which reader/builder functions can be deleted outright?
- Which temporary adapters are needed only to land the change safely?

Temporary v1 scaffolding requires an explicit reason and a removal path. Silent
default reconstruction from thin v1 CBOR into plausible install-time truth is
not a v2 behavior.

The conversion pipeline is a separate transition concern, not a reason to keep
v1 native. On-the-fly and server-side conversion should emit `format_version = 2`
only when the converted artifact satisfies the v2 authority contract. If a
converter cannot satisfy that contract yet, its output remains explicitly
foreign/legacy, requires verified foreign provenance, and cannot be published,
indexed, or installed as native v2. Any temporary v1 reader support for such
artifacts must live outside the v2 native reader and carry a deletion path.

## Current Repo Facts

The current CCS code already has package-building, signing, archive reading,
installation adapters, provenance, and M2 publish gates. It also has a split
authority problem:

- `crates/conary-core/src/ccs/manifest.rs` owns the current TOML-shaped
  `CcsManifest` and is already a hotspot at 1500 lines.
- `crates/conary-core/src/ccs/binary_manifest.rs` defines `FORMAT_VERSION = 1`
  and carries a thinner CBOR authority document.
- `crates/conary-core/src/ccs/package.rs` converts CBOR into `CcsManifest` by
  defaulting fields that CBOR cannot carry.
- `crates/conary-core/src/ccs/archive_reader.rs` overlays TOML-only fields when
  both CBOR and TOML are present.
- `docs/specs/ccs-format-v1.md` documents package `type` values, but the
  current `Package` struct does not implement that package kind.
- `crates/conary-core/src/ccs/attestation.rs` computes content identity by
  clearing specific provenance/signature fields from the old manifest shape.
- `docs/modules/test-fixtures.md` maps CCS conversion/publication fixture
  families that M4a must classify before changing package validity semantics.

These facts justify replacement. They do not justify preserving v1 as a native
contract.

## Non-Goals

- Do not implement M4b authoring UX in M4a.
- Do not implement Remi native intake or indexing in M4a.
- Do not implement M4d supported-target profile facts in M4a.
- Do not expand public support beyond Fedora 44, Ubuntu 26.04, and Arch.
- Do not preserve v1 installability as a product feature.
- Do not turn converted/foreign packages into native v2 packages unless they
  satisfy the v2 authority contract.
- Do not add v2 authority into `manifest.rs` by default.
- Do not write the M4a implementation plan until this design survives external
  and local review.

## Ownership Boundary

`crates/conary-core/src/ccs/v2/` owns the new contract.

The v2 module should own:

- v2 schema types;
- v2 validation;
- v2 archive authority checks;
- v2 content identity projection;
- legacy/v1 classification used during migration;
- public errors/diagnostics for invalid v2 and legacy-v1 inputs.

Existing files become transition surfaces:

- `manifest.rs`: current v1/TOML manifest shape and shared legacy DTOs until
  migrated or split.
- `binary_manifest.rs`: v1 CBOR format, not the model for v2 authority.
- `archive_reader.rs`: routing layer that detects v2 versus legacy v1 and fails
  closed when authority is missing.
- `package.rs`: install-facing adapter that consumes validated v2 packages
  instead of reconstructing missing truth from v1 defaults.
- `attestation.rs`: reusable trust machinery, with v2-owned content identity
  rules replacing fragile field-name clearing over the old manifest shape.

`archive_reader.rs` and `package.rs` should remain thin transition facades. The
implementation plan must decide the exact split between routing in those files
and version-owned parsing in modules such as `ccs/v1/reader.rs` and
`ccs/v2/reader.rs`; it should not bury v2 parsing, merging, or validation in the
legacy transition files.

The implementation plan can choose exact child file names inside `ccs/v2/`, but
the design boundary is fixed: no new v2 install-time authority goes into
`manifest.rs` unless the child plan proves the type is genuinely shared and
does not grow the hotspot.

## Archive And Authority Boundary

M4a uses a signed/binary v2 authority document as the source of install-time
truth. The simplest archive routing is:

- v2 packages carry a CBOR `MANIFEST` with `format_version = 2`.
- That CBOR is serialized from a v2-owned authority type under `ccs/v2/`, such
  as `AuthorityDocumentV2` or `CcsManifestV2`; it does not extend the existing
  v1 `BinaryManifest`.
- v2 archives may also carry `MANIFEST.toml` for source/debug visibility.
- `MANIFEST.toml` is not install-time authority for v2 unless a field is also
  represented in the signed v2 authority document.
- A v1 `MANIFEST` with `format_version = 1` is legacy input, not native v2.

The v2 authority document must be complete enough for install, validation,
publication eligibility, and indexing without reading TOML as truth. If a field
affects install behavior, dependency solving, file ownership, lifecycle effects,
config merge behavior, component selection, trust, or publication eligibility,
it belongs in signed v2 authority.

`toml_integrity_hash` is not enough for v2 authority. It can prove bundled TOML
did not drift, but it does not make TOML-only fields authoritative. M4a either
promotes install-time fields into v2 authority or leaves them explicitly
non-installing/advisory. If a v2 archive includes `MANIFEST.toml`, the signed
v2 authority document must carry `toml_integrity_hash` or an intentionally named
equivalent for that debug TOML, and the verifier must fail closed on mismatch.
A package whose only install-time truth lives in TOML fails closed even when
that hash is valid.

M4a reuses the existing detached signing shape for the first v2 proof:
`MANIFEST.sig` remains a JSON `PackageSignature` generated by `SigningKeyPair`,
and it signs the exact archived CBOR bytes of the v2 `MANIFEST`. M4a does not
embed signatures inside the CBOR authority document and does not verify
signatures by reserializing the decoded manifest. The reader must retain the raw
`MANIFEST` bytes for signature verification; a deterministic v2 CBOR encoder can
still be introduced for writers and identity fixtures, but signature validity is
anchored to the stored bytes. Missing signatures, signature mismatches,
unsupported algorithms, and unsigned v2 authority all fail before install,
publish, or indexing.

Fail-closed rule: if v2 install-time authority is incomplete, the package is
invalid. The reader must not reconstruct a valid v2 package from TOML defaults.

## Package Shape

M4a closes the v1 package kind gap by making kind explicit in v2.

The v2 schema should represent three kinds:

- **package:** ordinary file-owning installable package.
- **group:** composition package with member requirements and no file payload.
- **redirect:** transitional package that redirects/replaces another package and
  has no arbitrary file payload.

The Rust shape should make invalid kind combinations hard to represent. M4a
should model the top-level package as an enum or equivalent ADT, for example
`Package(PackageData)`, `Group(GroupData)`, and `Redirect(RedirectData)`, rather
than a single flat struct plus best-effort validation.

The minimum v2 group authority is a non-empty member requirement list using the
same typed dependency vocabulary as package dependencies. Each member declares a
package or capability name, optional version constraint, optional
target/component scope, and whether the member is required or recommended.
Required members participate in dependency solving; recommended members are
signed advisory resolver input unless a later plan promotes them. A group may
carry package identity, description, provides, conflicts, and policy metadata,
but it must not carry file payloads, config file behavior, lifecycle hooks, or
host-effect declarations. Empty groups are invalid in v2 native packages.

Each kind has allowed and forbidden fields. A `group` must not silently carry
files or lifecycle hooks. A `redirect` must not pretend to be a normal package
with opaque behavior. Unsupported or contradictory kind payloads fail
validation.

The minimum v2 redirect authority is `redirect.to`, an optional
`redirect.version_constraint`, and optional `redirect.reason` diagnostics. The
target and constraint are resolver authority. The reason is signed when present
but must not change resolver behavior.

The legacy `[legacy]` manifest section is not v2 native authority. It is
TOML-only advisory metadata for converted packages; a v2 native package that
carries legacy metadata fails kind-contract validation.

M4a implementation proof should require end-to-end install/publish contract
coverage for `package`. `group` and `redirect` can be schema/validation-only in
the first M4a implementation slice unless the reviewed implementation plan
chooses to broaden that proof.

## Dependency, File, Component, And Lifecycle Authority

v2 authority should answer these questions without touching TOML:

- What does this package provide?
- What must be present or absent before install/activation?
- Which files and components does it own?
- What host lifecycle effects are declared and allowed?

Recommended shape:

- **Dependencies/provides:** typed entries with kind, name, optional version
  constraint, and optional target/component scope. Capability, package, file or
  path, binary, soname, pkg-config, conflict, replace, obsolete, and break are
  explicit concepts, not flattened strings.
- **Files/components:** every payload file has path, hash, type, mode, ownership
  intent, component membership, config/noreplace behavior, and conflict policy.
  Component defaults live in v2 authority.
- **Lifecycle:** users/groups, directories, services, tmpfiles, sysctl,
  alternatives, install/remove behavior, and sandbox/host capability
  declarations are declarative v2 authority. Unknown shell-script-like effects
  are rejected or preserved only as non-native legacy evidence.
- **Target facts:** M4a defines the hook points for profile validation, but M4d
  owns Fedora 44, Ubuntu 26.04, and Arch profile facts. M4a can say that a v2
  package declares a service; it should not encode target-specific service
  policy. The hook point should be a small `ccs/v2` boundary such as
  `ProfileValidator` or `TargetProfileQuery`, so the v2 validator can ask about
  profile constraints without embedding target facts.

## Trust, Provenance, And Content Identity

M4a reuses the M2 trust machinery, but v2 owns contract-level identity rules.
The v2 authority document replaces the v1 `BinaryManifest` for install-time
truth while reusing signing and attestation machinery where the semantics still
fit.

The v2 authority document includes these signed authority categories:

- package identity: name, version, release, architecture, platform, kind, and
  kind-specific data;
- dependency and provide authority;
- file, directory, payload hash, ownership, mode, config, component, and
  conflict-policy authority;
- lifecycle declarations and host-effect policy;
- component defaults and package policy;
- stable build or conversion provenance inputs that affect trust decisions.

M2 attestation types are split by role:

- `BuildOutputIdentity` and the stable input fields of
  `BuildAttestationPayload` can feed v2 build provenance and identity.
- `ForeignConversionBoundary` is evidence for foreign/legacy conversion; it is
  not native truth unless the resulting artifact also satisfies v2 authority.
- `BuildAttestationEnvelope`, package signatures, signer state, upload,
  staging, promotion, and serving metadata are verification or publication
  envelopes, not content identity fields.

v2 defines a canonical package identity projection:

- It includes install-time authority: package identity, files/components,
  dependencies, lifecycle, config, policy, and stable provenance inputs that
  affect trust.
- It excludes signatures, attestation envelopes, upload metadata, staging or
  promotion metadata, and post-build serving metadata.
- Resigning the same package does not change canonical content identity.
- Changing install-time authority changes canonical content identity.

The projection should be a positive v2 type such as
`CcsContentIdentityProjectionV2` that serializes only the categories above. It
must not clone the full manifest and clear signatures or provenance fields by
name.

For provenance, v2 separates:

- **build provenance:** source identity, recipe/build input identity, hermetic
  evidence hash, and build environment identity;
- **publication provenance:** signer/trust state, upload destination, staging,
  and promotion metadata;
- **foreign/conversion provenance:** legacy source package identity and fidelity
  evidence, which is not native truth unless it satisfies v2 authority.

The M4a child plan must update or replace
`compute_content_identity_excluding_signatures` so v2 identity is not a fragile
"clear these fields by name" projection over `CcsManifest`.

## Legacy Deletion And Fixture Migration

M4a must inventory v1-dependent paths and classify each item:

- **Migrate to v2:** fixtures and tests that represent normal native package
  behavior.
- **Delete:** helper paths, fallback conversions, and tests that protect
  behavior M4a intentionally removes.
- **Legacy rejection proof:** narrow fixtures that prove v1/defaulted packages
  fail closed with clear diagnostics.

The inventory must cover at least:

- `BinaryManifest` v1 parsing and format-version routing;
- archive reader CBOR/TOML merge behavior;
- CBOR-only manifest default reconstruction;
- v1 format docs and examples;
- checked-in and in-test CCS fixtures that rely on v1 assumptions;
- conversion pipeline output contract.

The M4a implementation plan starts with the concrete inventory; design lock is
not blocked on enumerating every fixture or call site in this design. The
inventory uses this starter rubric:

| Area | Known examples | M4a classification |
| --- | --- | --- |
| v1 CBOR parsing | `BinaryManifest`, `FORMAT_VERSION = 1` | Legacy reader/format detector, not v2 authority |
| Archive routing | `archive_reader.rs` CBOR/TOML merge and CBOR-only paths | Thin routing facade, with v2 parsing delegated to `ccs/v2` |
| Default reconstruction | `convert_binary_to_ccs_manifest` in `package.rs` and its `archive_reader.rs` call sites | Delete or narrow to v1-only legacy helper; never used by the v2 native reader |
| v1 docs/examples | `docs/specs/ccs-format-v1.md` and examples | Archive or mark superseded once v2 docs exist |
| CCS fixtures | Fixture families mapped in `docs/modules/test-fixtures.md` | Regenerate native fixtures to v2 or move to legacy-rejection proof |
| Conversion output | RPM/DEB/Arch conversion output contracts | Emit native v2 only with complete v2 authority; otherwise explicit foreign/legacy |

`convert_binary_to_ccs_manifest` is the main smell. Its defaulting behavior must
not survive as v2 native behavior. The known current call sites in
`archive_reader.rs` must move behind version routing: v2 reads a v2 authority
document that fails closed on missing fields, while any surviving v1 conversion
helper is private, legacy-only, and removable.

## Failure Behavior And Diagnostics

The validator fails before install, publish, or indexing when authority is
missing, contradictory, stale, or legacy-only.

Core failure classes:

- **missing-authority:** a required v2 install-time field is absent.
- **legacy-v1-package:** archive is v1 and is not accepted as native v2.
- **toml-only-authority:** install behavior exists only in TOML/source metadata.
- **kind-contract-violation:** package/group/redirect fields do not match the
  declared kind.
- **component-authority-mismatch:** file/component/default-component truth is
  incomplete or inconsistent.
- **lifecycle-unsupported:** lifecycle effect cannot be represented
  declaratively or validated against supported profiles.
- **identity-unstable:** content identity would change under resigning or ignore
  install-time changes.
- **conversion-not-native:** converted/foreign package is trying to claim native
  v2 status without satisfying v2 authority.

Diagnostics should include:

- stable code;
- short message;
- field/path involved;
- severity;
- suggested fix;
- whether the package is invalid, legacy-only, or needs regeneration.

v1 failure text should not imply that users have an old supported package to
upgrade. It should say that the archive is an old internal format and must be
rebuilt/regenerated as CCS v2.

## Testing And Verification Strategy

M4a is test-heavy because it changes the meaning of "valid package."

The child implementation plan should prove behavior in layers:

- **Schema tests:** v2 package/group/redirect parse/serialize; invalid kind
  payloads fail.
- **Round-trip tests:** v2 package authority survives build/archive/read without
  TOML overlays.
- **Negative authority tests:** missing config, lifecycle, component defaults,
  package kind payloads, or provenance fields fail with stable diagnostics.
- **Legacy tests:** v1 archive paths are classified as legacy/rejected; CBOR-only
  default reconstruction does not masquerade as v2.
- **Fixture migration tests:** old native fixtures are regenerated to v2 or
  moved into explicit legacy-rejection coverage.
- **Content identity tests:** resigning does not change identity; authority
  changes do.
- **Signature tests:** unsigned v2 authority, modified CBOR, modified
  `MANIFEST.sig`, unsupported signature algorithms, and raw-byte versus
  reserialized-CBOR drift fail closed.
- **Publish-gate regression tests:** M2 attestation/publish refusal gates still
  pass. Active emitted failure fixtures include `MissingAttestation`,
  `BuildAttestationSignatureMismatch`, `PackageSignatureMismatch`,
  `TomlIntegrityMismatch`, `OutputIdentityMismatch`, `UnacceptedSignerKey`,
  `NonHermeticHardeningLevel`, `StaleOrUnknownPolicy`,
  `UncleanCommandRiskReport`, `ForeignConversionMissingBoundary`,
  `ForeignConversionBoundaryHashMismatch`, and `RecordedDraftArtifact`.
  Reserved enum/diagnostic mappings such as `RetiredSignerKey` and
  `AbsentOrUnknownProvenanceClass` must remain explicit unless a later reviewed
  gate change removes or replaces them.

Focused verification should start with `conary-core`, with child-plan commands
including at least:

```bash
cargo test -p conary-core ccs
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p conary --lib commands::publish
cargo test -p conary --test packaging_m2a
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
git diff --check
cargo fmt --check
```

If implementation touches CLI-visible diagnostics or docs claims, add
coherency ledger checks.

## Review And Execution Flow

This design must go through the normal review loop before an implementation
plan is written:

1. Write M4a design.
2. Run Gemini/DeepSeek with `--review-kind design`.
3. Do local agentic review.
4. Patch and lock the design.
5. Write M4a implementation plan.
6. Run Gemini/DeepSeek with `--review-kind plan`.
7. Do local agentic plan review.
8. Lock the plan.
9. Launch `/goal` for implementation.

The implementation plan should not be written until the design survives review,
because M4a contract decisions can change task ordering.

## Acceptance Gate

M4a is ready for implementation planning only when the reviewed design locks:

- v2 as the native contract and v1 as migration debris;
- `ccs/v2/` ownership for v2 schema/validation;
- signed/binary v2 authority sufficient for install, publish, and index without
  TOML truth;
- detached v2 signature verification over the exact archived CBOR authority
  bytes;
- package/group/redirect kind rules;
- dependency, file, component, lifecycle, config, policy, provenance, and
  identity authority;
- v1 fixture migration/deletion/rejection strategy;
- fail-closed diagnostics;
- M2 publish-gate regression proof;
- docs-audit and coherency expectations.
