# M4c Remi Native CCS Publication Design

**Date:** 2026-06-18
**Status:** Locked for implementation planning after DeepSeek, Gemini, and
local agentic review.
**Parent umbrella:** `docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md`
**Prerequisites:** M4a CCS v2 native package contract and M4b native authoring,
build, lint, and local test workflow are implemented and merged.
**Scope:** M4c only: Remi native CCS intake, verification, storage, indexing,
serving, and client proof.

## Purpose

M4c makes Remi a first-class native CCS publication service. A signed CCS v2
package produced by the M4b authoring loop should be publishable to a local
Remi, verified through the M2 static publish gate, indexed as native, fetched
by Conary, and installed without pretending to be a legacy conversion.

The important product boundary is simple: native CCS packages are native
publications, not converted packages with `original_format = "ccs"`.

## Prerequisite Contract

M4c consumes the M4a and M4b contracts; it does not redefine them. The M4c
implementation plan must verify these prerequisite surfaces still exist before
editing Remi publication behavior:

- `AuthorityDocumentV2` is the signed CCS v2 authority document.
- `PackageIdentityV2` carries `name`, `version`, non-empty `release`, optional
  `architecture`, and `kind`.
- `PackageKindTagV2` and `PackageKindV2` distinguish package, group, and
  redirect authority.
- M4a's exact-byte v2 signature verification remains the only way Remi accepts
  signed v2 authority.
- M4b v2 authoring/build output marks local-dev or host-hardened artifacts in
  provenance/trust fields that the M2 publish gate can reject before release
  publication.
- M4b release-eligible artifacts carry accepted build-attestation evidence,
  output identity, origin class, and hardening level in the shape consumed by
  `verify_static_artifact_publish_eligibility`.

If any of these surfaces move during prerequisite work, the M4c implementation
plan must update this design packet or explicitly patch the plan before
implementation starts.

## Core Decision

M4c adds dedicated native publication state and metadata.

Native package publication must not write synthetic `converted_packages` rows,
must not use `release:{distro}:{hash}` source-checksum strings, and must not
depend on legacy conversion scriptlet-publication gates to decide public-ready
state. Conversion rows remain for actual conversion output. Native publication
rows become the source of truth for Remi-served native CCS artifacts.

The existing admin route can remain the first external intake surface:

```text
POST /v1/admin/releases/{distro}
```

That route should become a thin compatibility/intake wrapper around native CCS
publication internals. The internal behavior changes from "release upload
stored as a converted package" to "native package publication with dedicated
state, metadata, and public serving paths."

## Current Repo Facts

The current Remi release upload path already has the trust shape M4c needs:

- `apps/remi/src/server/release_publish.rs` stages upload bodies privately.
- It reads accepted release signers from
  `release_publish.trusted_build_attestation_signers`.
- It calls `verify_static_artifact_publish_eligibility` with the shared M2
  publish-gate policy before public metadata commit.
- It copies accepted artifacts to a release package path and a CAS chunk path.
- It commits repository package rows and TUF targets only after the gate passes.
- It cleans promoted public objects when metadata commit fails.

The storage model is the problem:

- `commit_release_metadata_blocking` writes a `repository_packages` row.
- It also writes a `converted_packages` row using
  `ConvertedPackage::new_server`.
- The synthetic conversion row sets `original_format = "ccs"` and an
  upload-prefixed source checksum.
- Package metadata, download, repository metadata, sparse index, search, chunk
  serving, delta, and garbage-collection paths use conversion rows as public
  readiness and artifact reachability evidence.

M4c should preserve the useful gate and cleanup behavior while replacing the
conversion-shaped native storage contract.

## Non-Goals

- Do not weaken M2 artifact-form publish gates, build-attestation validation,
  trusted signer allowlists, output identity checks, or origin-class checks.
- Do not accept M4b local-dev or host-hardened packages as release-publishable
  native Remi artifacts.
- Do not implement developer key registration, rotation, or revocation as a
  required M4c feature.
- Do not add cloud R2 as a requirement for local native publication proof.
- Do not expand public distro support beyond Fedora 44, Ubuntu 26.04, and Arch.
- Do not make legacy conversion rows the source of truth for native packages.
- Do not add M4d target-profile facts or target-specific lifecycle validation.
- Do not create a second installer path for native Remi packages.
- Do not rewrite Remi federation, OCI, or conversion benchmarking as part of
  the first native publication slice.

## Ownership Boundary

M4c should introduce a native publication ownership boundary under Remi:

- `apps/remi/src/server/native_publish/` owns native CCS intake, artifact
  inspection, trust projection, publication persistence, and public lookup
  helpers.
- `apps/remi/src/server/release_publish.rs` becomes a thin adapter over the
  native publication pipeline or is split so release-body staging remains thin.
- `crates/conary-core/src/db/models/native_publication.rs` or an equivalent
  core DB model owns native publication row access.
- `crates/conary-core/src/db/models/repository.rs` gains explicit package
  release support for repository-visible native identity.
- Public Remi handlers keep their route ownership but consume native lookup
  helpers instead of querying `converted_packages` for native artifacts.

The implementation plan can choose exact file names inside
`native_publish/`, but the design boundary is fixed: native publication logic
does not belong in conversion persistence, conversion workflow, or
scriptlet-publication gate modules.

Recommended child modules:

- `native_publish/intake.rs`: request staging, size limits, route-facing
  request/response mapping.
- `native_publish/verify.rs`: CCS v2 parsing, static publish-gate execution,
  signer/trust projection, and error codes.
- `native_publish/storage.rs`: safe native filenames, release package copy,
  CAS copy, cleanup, and target-path construction.
- `native_publish/persistence.rs`: SQLite transaction, native publication row,
  repository package row, TUF target commit, and replacement semantics.
- `native_publish/public_lookup.rs`: manifest/download/index/search helpers
  that public handlers can call without knowing native table details.
- `native_publish/test_support.rs`: Remi test DB setup and native fixture
  builders shared by handler and persistence tests.

## Native Identity

Native CCS v2 package identity includes release:

```text
distro + name + version + release + architecture
```

M4c should make that identity explicit in repository-visible storage instead
of packing release into a version string. The schema change should add a
repository package release column, preferably named `package_release` to avoid
SQL ambiguity. Native package rows must require a non-empty release and a
normalized architecture key. If the v2 authority omits architecture, Remi must
store a stable non-null value such as `noarch` for native publication identity,
repository projection, TUF target paths, and uniqueness checks. Existing
repository and conversion-derived rows can use an empty release value when no
native release exists and may keep their current nullable architecture behavior.

The active public uniqueness rule should become:

```text
repository_id + name + version + package_release + architecture
```

Public APIs that currently accept `version` and `arch` should add optional
`release` query support. For native rows:

- `version + release + architecture` selects an exact native package.
- If `release` is omitted and exactly one release matches the requested
  version and architecture, Remi may serve it for convenience.
- If multiple releases match and `release` is omitted, Remi returns a conflict
  with the available releases.
- Existing conversion lookups continue to support the old `version` and `arch`
  query shape.

Index, search, sparse-index, and package metadata responses should include
`release` for native packages. Conversion-only rows may omit it or serialize it
as an empty value only when that matches the existing API style.

### Repository Migration

The database migration should be additive and query-audited:

- add `repository_packages.package_release TEXT NOT NULL DEFAULT ''`;
- drop the existing `idx_repo_packages_unique` index;
- recreate the unique index as
  `(repository_id, name, version, package_release, architecture)`;
- ensure native publication projections never write `NULL` architecture; use
  the normalized architecture key for absent-architecture native packages so
  SQLite uniqueness cannot admit duplicate noarch rows;
- update `RepositoryPackage` and batch insert helpers to carry
  `package_release`;
- update every Remi and resolver query that constructs a `RepositoryPackage`
  by hand so it selects or defaults the new column deliberately;
- update replacement/delete operations to use the full key
  `(repository_id, name, version, package_release, architecture)`.

This matters because native v2 permits sibling releases for the same upstream
version. A version-only delete or upsert would erase other valid native
releases.

### Client And Resolver Identity

The client side must carry release identity far enough to fetch the intended
artifact:

- `PackageIdentity` in the resolver should gain an optional
  `package_release` field populated from `repository_packages`.
- Remi repository sync should preserve native `release` in
  `repository_packages` and in the download URL or request metadata it stores.
- Remi HTTP URL helpers should accept optional `release` next to `version` and
  `arch` for package metadata and download requests.
- Version-only client requests that receive an ambiguity response must surface
  the available releases instead of retrying conversion or choosing one
  silently.

The first M4c proof can keep conversion-only rows on the old version/arch path,
but native rows must be resolvable and downloadable by full native identity.

## Native Publication Storage

M4c should add a dedicated native publication table. Recommended shape:

```text
native_package_publications
  id
  repository_id
  repository_package_id
  distro
  name
  version
  package_release
  architecture
  package_kind
  authority_format_version
  status
  content_hash
  chunk_hashes_json
  total_size
  package_path
  target_path
  authority_hash
  package_signature_key_id
  package_signature_public_key_sha256
  build_attestation_hash
  build_attestation_signer_key_id
  origin_class
  hardening_level
  provenance_json
  trust_status
  verification_report_json
  published_at
  superseded_at
  rolled_back_at
  created_at
  updated_at
```

The first implementation does not need an admin rollback command, but the row
shape should not paint us into a corner. The required first states are:

- `public`: the active artifact is verified, indexed, TUF-targeted, and
  servable.
- `superseded`: a newer artifact replaced the same native identity.
- `rolled_back`: a previously public artifact was explicitly removed from the
  active public set by a later rollback-capable slice.

Rejected uploads do not need public rows. The route response must still return
stable rejection codes and verification details. If the implementation chooses
to persist rejected attempts for admin diagnostics, those rows must be private
admin state and must never make package metadata, chunks, TUF targets, search,
or sparse-index entries servable.

`repository_packages` remains the public discovery table for Remi indices and
client resolution. For native publications it should be a projection of the
native row, not the source of truth. Its metadata JSON should carry native
identity and trust projection such as:

```json
{
  "source_kind": "native-ccs",
  "native": true,
  "identity": {
    "name": "hello",
    "version": "0.1.0",
    "release": "1",
    "architecture": "x86_64"
  },
  "trust": {
    "status": "verified",
    "origin_class": "native-built",
    "hardening_level": "hermetic"
  }
}
```

This metadata is public-facing summary data. Private file paths, private review
state, full build logs, unverified signatures, and local operator-only trust
details must not be serialized into public responses.

## Upload Lifecycle

The native publication lifecycle is:

```text
admin upload
  -> private body staging
  -> size and basic body checks
  -> CCS v2 parse and exact-byte signature verification
  -> static publish gate with configured accepted release signers
  -> native identity, normalized architecture, and trust projection from
     verified v2 authority
  -> private artifact promotion to release package path and CAS
  -> SQLite transaction:
       native publication row
       repository package projection
       TUF target metadata
       supersede prior active row for same identity
  -> public response
  -> staged body cleanup
```

No public package row, active native publication row, public chunk reachability,
or TUF target may exist until verification succeeds and the SQLite commit is
ready to publish it. If the commit fails after file promotion, the promoted
package object and CAS object are removed just as the current release upload
path removes public objects on metadata failure.

Replacement semantics should be atomic from the public API point of view. A
new upload for the same native identity either becomes the single active public
row and supersedes the previous row, or the previous row remains active. Failed
replacements must not leave no active generation when a prior public artifact
exists.

### Transaction And Concurrency Semantics

The native publication commit should use one SQLite write transaction, with
`BEGIN IMMEDIATE` or the repo's equivalent immediate transaction helper, for
all public-state changes:

1. validate the target repository row;
2. insert the new native publication row as pending transaction-local state or
   insert it as `public` only after all dependent writes are ready;
3. mark the prior active native row for the same full identity as
   `superseded`;
4. upsert the `repository_packages` projection with `package_release`;
5. delete or deactivate superseded TUF target rows for the same native identity
   before loading targets for regenerated metadata;
6. write or replace the active TUF target metadata;
7. commit.

No public reader should observe a native publication row without a matching
repository projection and TUF target. If commit fails, the prior active row and
projection remain active. The current `RwLock<ServerState>` request path can
remain as outer serialization, but correctness must not depend only on that
process-local lock; the SQLite transaction is the durable consistency
boundary.

## Verification And Trust

M4c must reuse the shared M2 publish gate. Native intake runs
`verify_static_artifact_publish_eligibility` before indexing or serving the
package. The accepted signer set comes from
`release_publish.trusted_build_attestation_signers` unless the implementation
plan renames the config path with a migration.

The first M4c trust policy is:

- CCS v2 package authority must parse and verify.
- Package signature verification must pass using M4a's exact-byte v2 signature
  path.
- Server-side artifact inspection must consume the verified v2 package or
  verified v2 authority produced by the shared gate path. Remi must not reopen
  the accepted artifact with the legacy `CcsPackage::parse` path to extract
  name, version, architecture, scriptlet, or content metadata.
- The static publish gate must pass.
- The build attestation signer must be accepted by Remi config.
- The build attestation output identity must match the uploaded artifact.
- The artifact origin and hardening level must satisfy the M2 release policy.
- M4b local-dev signing and host-hardened local builds are rejected for release
  publication.

`verify_static_artifact_publish_eligibility` is the shared gate. Remi
`native_publish/verify.rs` may layer Remi-specific checks around it, such as
distro validation and response-code mapping, but it must not bypass the shared
gate for build-attestation signer authority, package signature validity, output
identity, origin class, hardening level, command-risk, policy digest, or
recorded-draft refusal.

Release-eligible native uploads are distinguished from local-dev artifacts by
the same M2/M4b fields the static gate already evaluates:

- an accepted build attestation signer;
- matching build output identity for the uploaded CCS artifact;
- non-local release hardening, currently hermetic hardening for release
  publication;
- an origin class accepted by the release policy;
- no local-dev-only signing or provenance mode.

M4b local-dev artifacts should therefore fail through publish-gate failures
such as unaccepted signer, non-hermetic hardening, absent or unknown provenance,
or output identity mismatch. M4c may expose the user-facing
`LOCAL_DEV_ARTIFACT_REFUSED` code when those failures clearly identify a
local-dev artifact, but it must preserve the underlying publish-gate failure
details for diagnostics.

Package signer identity is recorded as native metadata, but package signature
alone is not release authorization in M4c. Developer key registration and key
rotation can be added later as an admin-managed trust surface. Until that
exists, release publication authority remains the accepted build-attestation
signer set plus the shared static publish gate.

Verification failures must return stable machine-readable codes. Required
codes include:

- `INVALID_CCS`
- `UNSUPPORTED_CCS_FORMAT`
- `PACKAGE_SIGNATURE_FAILED`
- `PUBLISH_GATE_FAILED`
- `UNTRUSTED_BUILD_ATTESTATION_SIGNER`
- `OUTPUT_IDENTITY_MISMATCH`
- `LOCAL_DEV_ARTIFACT_REFUSED`
- `UNSUPPORTED_DISTRO`
- `METADATA_COMMIT_FAILED`

Mapping:

| Remi code | Source |
| --- | --- |
| `INVALID_CCS` | Remi/native CCS parse error before publish-gate classification. |
| `UNSUPPORTED_CCS_FORMAT` | Remi/native format check when uploaded CCS is not v2 native authority. |
| `PACKAGE_SIGNATURE_FAILED` | `PublishGateFailureCode::PackageSignatureMismatch` or strict v2 signature verification failure. |
| `PUBLISH_GATE_FAILED` | Wrapper for one or more shared publish-gate failures. |
| `UNTRUSTED_BUILD_ATTESTATION_SIGNER` | `UnacceptedSignerKey` or `RetiredSignerKey`. |
| `OUTPUT_IDENTITY_MISMATCH` | `OutputIdentityMismatch`. |
| `LOCAL_DEV_ARTIFACT_REFUSED` | Remi user-facing grouping for local-dev-only provenance, non-hermetic hardening, or unaccepted local-dev signer evidence. |
| `UNSUPPORTED_DISTRO` | Remi route/config validation before storage. |
| `METADATA_COMMIT_FAILED` | Remi persistence/TUF/internal commit failure after verification. |

`METADATA_COMMIT_FAILED` is not a publish-gate failure; it is a server commit
failure. The implementation plan should decide exact HTTP status codes, but
publish-gate failures should remain structured enough that CLI diagnostics can
show both the stable Remi code and underlying gate failure codes.

Error responses should include concise human text and enough structured detail
for CLI diagnostics without leaking private server paths or secrets.

## Public Serving

M4c must teach public Remi surfaces to understand native rows directly:

- `GET /v1/{distro}/metadata`
- `GET /v1/{distro}/packages/{name}`
- `GET /v1/{distro}/packages/{name}/download`
- `GET /v1/index/{distro}/{name}`
- `GET /v1/index/{distro}`
- `GET /v1/search`
- `GET /v1/suggest`
- chunk serving and local chunk reachability checks
- chunk garbage collection

Native lookup should happen before conversion fallback for exact package
metadata and download requests. If a matching active native publication exists,
Remi returns or streams that artifact. If no native match exists, existing
conversion job behavior remains available for conversion-backed packages.

Public response semantics:

- `converted` remains true only for actual converted artifacts.
- Native packages expose `native = true` or `source_kind = "native-ccs"`.
- Native packages are not counted as converted.
- Native package metadata does not include legacy scriptlet publication summary
  unless future v2 authority explicitly defines a native scriptlet summary
  projection.
- Public chunk reachability treats active native publication chunks as
  servable.
- Chunk garbage collection treats active native publication chunks and package
  paths as protected references.

Search schema can evolve incrementally. If changing Tantivy fields is invasive,
the first plan may keep the old `converted` field only as the conversion flag,
with native packages indexed or post-projected as `converted = false`. Search
responses must add a source-kind/native signal for native rows, such as
`source_kind = "native-ccs"` or `native = true`, and must include release
identity. If the implementation keeps the existing Tantivy schema for the first
slice, it must override response projection from native publication metadata;
it must not store native rows as converted just to fit the old schema.

Search document identity must also stop collapsing native releases. Current
search behavior keys documents by package name and distro and rebuilds one
latest row per name/repository. M4c native publication search must preserve
distinct native `version + release + architecture` rows, either by extending
the search document key or by post-projecting native publication rows after
query. Native search tests must prove that sibling releases are both visible
and neither is mislabeled as converted.

Sparse-index and metadata builders should merge:

- repository package projections;
- active native publication rows;
- public-ready conversion rows.

The merge key must include release for native rows. Native-only upload rows
must not disappear merely because there is no upstream conversion-backed
repository row.

### Client Preflight

The CLI Remi publish path must not reject valid v2 packages before upload. The
current client-side preflight helper uses the legacy `CcsPackage::parse` path,
while v2 package authority requires the verified v2 reader. M4c should update
preflight to recognize v2 packages without weakening server authority. The
client preflight may do structural v2 detection or local verification for a
better error message, but Remi server-side verification remains authoritative.

## TUF And Artifact Paths

Native TUF targets should use a target path that includes native identity and a
content hash. Recommended form:

```text
packages/{distro}/{name}-{version}-{release}-{arch}-{content_hash}.ccs
```

If architecture is absent, use a stable `noarch` or equivalent normalized
segment instead of omitting the segment. The implementation plan should reuse
or narrow the existing safe filename helper so target paths cannot contain
traversal or unsafe characters.

The implementation plan must verify that the chosen safe-filename or encoding
function accepts the full valid CCS v2 release character set, or define a
lossless target-path-safe encoding before using release strings in TUF target
paths.

TUF metadata commit remains part of the same SQLite transaction as native
publication and repository package projection. A target is public only when
the corresponding active native publication row is public. Because native target
paths include content hash, replacing the same native identity with new bytes
must remove or deactivate the superseded target path in the same transaction
that supersedes the old native publication row; otherwise stale TUF metadata
would continue advertising an inactive artifact.

## Client Proof

M4c should prove a local client can consume the native Remi package path:

```text
build or load signed release-eligible v2 fixture
  -> publish to local Remi
  -> fetch repository metadata or sparse index
  -> resolve package identity including release
  -> download the CCS package
  -> verify/install through the existing CCS install path
```

The client proof must reuse the existing v2 install/verification path. M4c
does not add a Remi-only install shortcut.

The first client proof can run against a local Remi instance or focused test
harness. It must demonstrate that native package resolution and download do
not require a `converted_packages` row.

## Failure Behavior

M4c fails closed:

- Invalid or unsigned v2 packages are rejected before promotion.
- Failed static publish gates are non-public and leave no active native row.
- Failed replacement uploads leave the prior active publication intact.
- Metadata or TUF commit failure removes promoted package/CAS objects.
- Missing release identity fails native publication.
- Ambiguous version-only native requests return conflict with choices.
- Unsupported distro slugs fail before storage.
- Public handlers never expose private rejection attempts or local filesystem
  paths.
- Conversion fallback remains conversion-only and must not synthesize native
  publication state.
- Chunk garbage collection treats active native publication chunks as referenced
  even though they no longer live in `converted_packages`.

## Testing Strategy

The M4c implementation plan should add focused tests before broad workspace
verification.

Required Remi tests:

- release/native upload rejects empty or missing trusted signer config;
- local-dev M4b package is rejected by publish gate;
- release-eligible v2 fixture publishes successfully;
- successful native publish writes a native publication row;
- successful native publish writes no `converted_packages` row;
- successful native publish writes repository package projection with release;
- successful native publish writes/updates a TUF target;
- metadata commit failure cleans promoted package and CAS objects;
- replacing the same native identity is atomic and preserves last public state
  on failure;
- package metadata and download succeed from native rows;
- native package appears in metadata, sparse index, and search as native, not
  converted;
- active native chunks are servable and protected from garbage collection;
- conversion-backed package behavior remains unchanged;
- version-only replacement never deletes sibling releases with a different
  `package_release`;
- absent-architecture native packages normalize to one non-null architecture
  key and cannot duplicate the same logical noarch identity;
- replacing a native package removes or deactivates the superseded TUF target
  in the same commit that writes the new target;
- publish/search/download response projection keeps native packages
  `converted = false`;
- search preserves distinct native `version + release + architecture` rows
  instead of deleting by name+distro only;
- client Remi publish preflight accepts structurally valid v2 packages and
  leaves final trust decisions to the server;
- server-side native inspection uses verified v2 authority, not legacy
  `CcsPackage::parse`;
- `chunk_gc::build_referenced_set` includes active
  `native_package_publications` chunks.

Required client tests:

- Conary resolves and downloads a Remi-published native CCS package.
- Conary installs the downloaded package through the existing v2 CCS install
  path.
- Version-only native queries with multiple releases fail with an actionable
  ambiguity response.
- Remi HTTP client helpers send `release` for native package metadata and
  download requests.
- Resolver candidates preserve `package_release` for native Remi rows.

Required regression proof:

```bash
cargo test -p remi release_upload_
cargo test -p remi remi_release_parity
cargo test -p remi publication
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4b
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
```

The final plan should add broader `cargo test -p remi` and targeted client
tests once handler and client surfaces are touched.

## Docs And Audit Updates

The M4c implementation plan should update:

- `docs/modules/remi.md`
- `docs/modules/test-fixtures.md`
- `docs/modules/feature-ownership.md`
- `docs/llms/subsystem-map.md`
- Remi OpenAPI/admin route docs if response payloads or error codes change
- repository/client docs if native release query behavior changes

Because M4c touches public routes, repository claims, and assistant-facing
routing docs, the implementation plan must check
`docs/superpowers/feature-coherency-ledger.tsv` for covered paths before
editing public claims or help text.

Docs verification for the design and plan should include:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
! rg -n "TB[D]|TO[D]O" docs/superpowers/specs/2026-06-18-m4c-remi-native-ccs-publication-design.md
git diff --check
cargo fmt --check
```

## Implementation Plan Shape

The M4c plan should split implementation into small slices:

1. Schema and model foundation: native publication table, repository
   `package_release`, unique-index migration, resolver identity updates,
   non-null normalized native architecture keys, migration tests, and model
   helpers.
2. Native publish module foundation: staging adapter, v2 inspection, publish
   gate reuse, verified-v2 server inspection, trust projection, client
   preflight compatibility, and structured rejection responses.
3. Native persistence and TUF commit: native row, repository projection,
   replacement semantics, superseded TUF target removal, cleanup, and
   no-converted-row tests.
4. Public package metadata/download: native lookup before conversion fallback,
   release query semantics, Remi client release query support, and artifact
   streaming.
5. Index/search/sparse/chunk reachability: native rows in public discovery,
   native source-kind metadata, converted=false search projection, release-aware
   search document identity, chunk serving, and garbage-collection protection
   through `chunk_gc`.
6. Client proof: local Remi publish, fetch, download, and install through the
   existing v2 CCS install path.
7. Docs, ledger, and final regression gates.

Each slice should keep conversion behavior covered while moving native
publication away from conversion-shaped storage.

Implementation slices that add substantial behavior to
`apps/remi/src/server/release_publish.rs`, `apps/conary/src/commands/publish.rs`,
or other Rust files already over the repository's review thresholds must name
the ownership boundary they preserve or improve before editing.
