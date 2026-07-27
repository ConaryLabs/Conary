---
last_updated: 2026-07-27
revision: 10
summary: Document Remi source, signing, canonical-map, repository trust, conversion, publication, and serving authority
---

# Remi

Remi is Conary's on-demand conversion and package-serving service. For the
limited public preview, its configured public repository feeds are Fedora 44,
Ubuntu 26.04, and Arch. It converts upstream RPM, DEB, and Arch packages into
CCS artifacts, stores converted content in the local content-addressed store,
and can write chunks through to R2 when configured.

M4d routes every `{distro}` path parameter through repository-feed profile
validation before DB queries, cache/key filesystem paths, or release-upload
trust gates. Public Remi route slugs remain `fedora`, `ubuntu`, and `arch`;
they are backed by profile route metadata rather than a local hard-coded
`SUPPORTED_DISTROS` list.

## Package Source Authority

Hosted package sources are declared in `deploy/remi-repositories.toml` and
loaded by `apps/remi/src/server/repository_manifest.rs`. Every source names one
exact public profile and carries typed parser construction data: RPM declares
architecture; Debian declares distribution, component, and architecture; Arch
declares the repository database name; JSON is explicit when a source actually
serves Conary JSON metadata.

Every native source also carries the matching `RepositoryTrustPolicy`. Fedora
uses its official metalink to authenticate the exact `repomd.xml` and a
separate pinned Fedora package key for embedded RPM signatures. Ubuntu pins the
archive Release certificate that authenticates `InRelease` through Packages to
the `.deb`. Each Arch source consumes the exact `archlinux-keyring` ALPM
package, pins all five current master fingerprints, requires three distinct
master certifications for packager keys, requires package signatures, and
checks database signatures when present. Startup refuses a missing or
format-mismatched trust contract; repository reconciliation cannot install an
authority bypass.

`repositories.parser_config_json` persists that typed configuration;
`package_format` is its validated query projection, while
`trust_policy_json` persists the role-separated trust contract. Repository
names, URL substrings, file extensions, route-family aliases, and SQL `LIKE`
expressions are not source, parser, or trust authority. A configured native
parser or trust failure is an error for that source; it does not silently retry
the URL as another metadata format or continue unsigned.

`apps/remi/src/server/readiness.rs` owns serving readiness. `/health` is an
unconditional liveness reply and proves only that the process is listening;
`/health/ready` is the evidence-bearing one. It opens the database read-only,
requires the expected schema revision, and checks the serving directories and
configured free-space floor. A probe that cannot run reports `unavailable`
rather than success, so an unmeasurable resource never reads as ready. Deploy
verification and `scripts/remi-health.sh` assert `ready == true` from that
endpoint; liveness alone is not deployment evidence. The free-space floor is
`storage.readiness_min_free`, defaulting to 10 GiB.

`apps/remi/src/deployment.rs` owns recoverable config/schema transitions and
read-only deployment inspection. It snapshots a current database or retires an
old schema epoch before replacement, so health failure can restore config,
source authority, and SQLite state together. Startup reconciles the installed
manifest before opening listeners, then immediately refreshes metadata and
runs eligible exact-profile prewarm jobs concurrently under the configured
conversion bound shared with request-driven conversions. Each profile preserves
its own top-N ordering and sequential conversion semantics, while a slow first
profile cannot starve the profiles after it. Multi-source refresh returns one
typed result or failure per source. A source failure remains visible but cannot
discard successful source commits or suppress prewarm for another exact
profile. Prewarm eligibility comes only from the successful result's persisted
`source_profile`; repository names, URLs, formats, and error text are not
selectors.

Both internal and external `POST /v1/admin/refresh` routes use the same response
projection: HTTP 200 means every source completed or was current, 207 carries a
mixed success/failure batch, and 502 means every configured source failed.
Global database/setup failures remain HTTP 500. The release deployment gate
still requires exact source reconciliation, all sources populated, and at
least one conversion and validated converted artifact for every configured
public profile. Result and failure arrays are sorted by exact repository name,
so concurrent completion order is not API order.

`apps/remi/src/server/admin_service/refresh.rs` owns the batch state, typed
per-source results, and bounded concurrent collection. `server/mod.rs` owns the
exact-profile handoff from successful refresh results into configured prewarm
jobs; `apps/remi/src/server/prewarm/scheduler.rs` owns their bounded fair
execution and complete outcome collection. HTTP handlers only publish events
and project the shared batch.

## Durable Repository Signing Authority

`apps/remi/src/server/signing_authority.rs` owns Remi's durable CCS and TUF
signing authority. Deployment derives the unique exact profiles from the typed
repository manifest and provisions one complete Ed25519
`targets`/`snapshot`/`timestamp` role set per profile under
`/conary/repository-keys/<exact-profile>/`. The root and profile directories
are mode `0700`, private files are `0600`, and public files are `0644`. A
profile role set is staged, synced, and atomically published; a repeat deploy
validates and preserves the existing bytes.

Existing authority is never repaired by replacement. Missing halves,
malformed keys, mismatched private/public material, incorrect role IDs,
symlinked or incorrectly owned entries, unsafe modes, unexpected files, and
route-slug directories fail deployment. `deployment inspect` independently
validates that the manifest profiles and complete signing profiles agree.
Signing authority lives outside binary/config rollback paths.

Conversion and recipe builds load `targets` by the persisted exact source
profile. Public native-release and timestamp routes first resolve their route
slug through the supported-profile registry, then load the exact profile's
role keys. `fedora` and `ubuntu` are therefore never key-directory aliases for
`fedora-44` and `ubuntu-26.04`.

## Canonical Map Exchange

Remi builds canonical package equivalence only from the versioned literal
contracts loaded by `apps/remi/src/server/canonical_job.rs`. Repology remains a
discovery cache. AppStream may enrich an already-authorized canonical identity
with one unique application ID, but it cannot create an implementation row.
The exact contract rows, rebuild timestamp, and content revision commit in one
SQLite transaction, so a reader cannot observe new mappings under an old
revision.

The shared wire and replacement owner is
`crates/conary-core/src/canonical/exchange.rs`; local persistence rules live in
`db/models/canonical.rs`. `GET /v1/canonical/map` returns canonical-map schema
version 1, a persisted content revision and rebuild timestamp, and each
identity's exact kind, optional category, and public-profile package map.
`generated_at` is `null` only for the empty revision-zero map, so the response
body and ETag stay stable between rebuilds.

The response carries `X-Conary-Canonical-Sha256` for the exact bounded body and
`X-Conary-Canonical-Revision` for its content revision. Both Conary fetch paths
verify the checksum before parsing or opening a persistence transaction. The
parser denies unknown fields, unsupported schema versions, route aliases,
unknown profiles, duplicate keys, duplicate identities, and empty or
conflicting mappings.

Snapshot application atomically replaces Remi-owned rows while preserving
non-conflicting local `Contract` authority. An identical Remi mapping cannot
demote a contract; an exact local contract can promote an identical Remi row.
Any package-name disagreement rolls the replacement back. The current schema
also makes AppStream IDs unique, removes the unused implementation repository
column, and permits exactly one package implementation per canonical identity
and public profile.

## Release Uploads

Remi release push is the first native CCS publication intake surface. The
route remains `POST /v1/admin/releases/{distro}` with bearer-token admin auth,
but accepted CCS v2 uploads are stored in `native_package_publications` and
projected into `repository_packages`; they are not synthetic
`converted_packages` rows. Native uploads stage privately, run the shared static
publish gate against `release_publish.trusted_build_attestation_signers`.
There is no parallel `/v1/admin/packages/{distro}` publication route.
After structural parsing, signature/trust verification, and the shared static
publish gate pass, Remi validates signed CCS v2 lifecycle authority
structurally. The route selects a repository feed, never a destination
compatibility policy. Unsupported route slugs fail before storage or artifact
verification. Package rows, native rows, chunks, and TUF targets are published
only after the gate, structural lifecycle validation, and metadata commit pass.

The route/staging wrapper lives in `apps/remi/src/server/release_publish.rs`.
Native CCS verification, artifact promotion, metadata persistence, supersede
behavior, and public native lookup live under
`apps/remi/src/server/native_publish/`. Failed authorization, metadata, or TUF
commits must leave the previous public native generation intact and must not
write a new public package row, chunk object, `converted_packages` row, or TUF
target for the rejected upload.

Public metadata and download lookups are release-aware for native rows:
clients should request `version`, `release`, and `arch` when selecting a native
package. If a version-only request matches multiple native releases, Remi
returns a conflict with the available releases instead of guessing.

## Native Lifecycle Metadata

RPM, Debian, and Arch conversions embed the current `native_lifecycle` bundle
in the generated CCS manifest and persist its aggregate summary on
`converted_packages`. The row records lifecycle fidelity, evidence digest,
formal unknown-command evidence, and diagnostic classes. Entry presence in the
validated bundle is lifecycle authority; the summary carries no duplicated
entry-decision count. There is no scriptlet publication-status projection.
The current schema separates installed conversions from repository-serving
artifacts with a required discriminator. Installed rows require an exact trove
identity and cannot carry serving fields. Repository rows require their exact
distro/name/version/architecture identity, chunk list, total size, content
hash, and CCS path; public and OCI handlers validate that typed artifact
instead of filling missing fields with guesses or empty values. Architecture is
a required constructor and API-view field, and the current schema rejects
missing or empty values. Local conversion tracking is written only after the
CCS install transaction commits.

Ready conversion means the artifact carries a source-independent Conary
lifecycle contract. A client may install it on any target whose typed
capability inventory satisfies that contract, without consulting or mutating
RPM, dpkg, or ALPM state. Missing source-format semantics are required
parser/planner/executor defects; Remi does not turn them into a review workflow
or make a source package manager part of the runtime.

The summary is validated when conversion is persisted and when a current row is
read. Malformed JSON, scalar/summary disagreement, an unknown schema revision,
or a missing CCS object is data corruption or stale conversion state and
returns an error or triggers reconversion. It is not converted into a human
review outcome.

Package detail, metadata, generated indexes, sparse indexes, search, OCI, delta,
and download routes expose the same sanitized `scriptlets` projection. Program
bodies and local filesystem paths are not part of that response.

### Current Converted Artifact Serving

Server conversion has two terminal outcomes: ready or failed. A ready
conversion is advertised and served when its conversion version and lifecycle
summary are current and its CCS/CAS objects exist. Lifecycle program content,
diagnostic classes, package names, provides, and command-name matches do not
create a second serving decision.

Every route resolves the public route slug to one exact persisted source
profile and uses the same current-row validation. A stale row is reconverted;
malformed current state is surfaced as a server data error. Conversion has no
operator promotion state or alternate serving lane.

Local chunk visibility in `server/publication.rs` is a reachability check:
native publication references are authoritative directly, and converted chunks
are served only when a current validated conversion references them. Stale-only
and unreferenced local cache objects remain private. It does not classify
lifecycle program text.

Sparse-index and search responses use `converted=true` only for current,
validated conversions. Lifecycle execution remains the client's typed
transaction responsibility; serving a lifecycle-bearing CCS does not bypass
client preflight.

Public package lookup and download orchestration remain in
`apps/remi/src/server/handlers/packages.rs`; delta lookup is owned by
`apps/remi/src/server/handlers/packages/delta.rs`. Chunk serving and batch
transport remain in `handlers/chunks.rs`, while cache statistics, eviction,
Bloom rebuild, and chunk-directory scanning are owned by
`handlers/chunks/admin.rs`. Focused handler tests live beside those modules
under `handlers/{packages,chunks,index,oci}/tests.rs`.

### Fixture Ownership

The first Remi fixture ownership map lives in `docs/modules/test-fixtures.md`.
Start there before changing conversion validation, public index metadata,
static test fixture uploads, or `conary-test` manifest behavior.

Fast proof for native release-publication edits:

```bash
cargo test -p remi release_upload_
cargo test -p conary --test packaging_m4c
```

Fast proof for conversion edits:

```bash
cargo test -p remi conversion
```

Medium proof when public serving, conversion state, or generated metadata
changes:

```bash
cargo test -p remi
```

## Conversion Service Ownership

The conversion service now keeps `apps/remi/src/server/conversion.rs` as the
stable public hub for `ConversionService` and conversion result DTO re-exports.
Implementation ownership lives in child modules:

- `conversion/workflow.rs`: cold/hot package conversion orchestration and
  timing.
- `conversion/types.rs`: public conversion result DTOs, scriptlet package
  metadata projection, and conversion benchmark evidence records.
- `conversion/benchmark.rs`: benchmark sampling, scan-only scriptlet evidence,
  and benchmark conversion wrappers.
- `conversion/lookup.rs`: repository package selection, profile-backed
  repository hints and version scheme, upstream download, and one-shot metadata
  refresh after upstream 404s.
- `conversion/metadata.rs`: safe CCS filenames, profile-backed parser dispatch,
  metadata construction, repository identity application, and
  repository-provide merging.
- `conversion/storage.rs`: local CAS writes, optional R2 write-through, and
  checksum helpers.
- `conversion/persistence.rs`: converted-package rows, cache-hit
  reconstruction, current-summary validation, and ready-result construction.
- `conversion/recipe.rs`: recipe URL fetch, DNS/IP validation, SSRF refusal,
  and server-side recipe builds.
- `conversion/test_support.rs`: conversion-owned test DB, repository package,
  conversion result, and scriptlet summary builders shared by child-module
  tests.

For conversion behavior changes, start with the owner module and run the
focused module tests plus `cargo test -p remi --lib conversion`. For public
listing or lifecycle-summary behavior changes, also run `cargo test -p remi`.

## Conversion Benchmark Evidence

Remi includes a local benchmark command for measuring cold-path conversion cost
before making public latency claims:

```bash
cargo run -p remi -- conversion-benchmark \
  --db /var/lib/conary/conary.db \
  --chunk-dir /var/lib/conary/data/chunks \
  --cache-dir /var/lib/conary/data/cache \
  --distro fedora \
  --package nginx \
  --jsonl
```

When R2 flags are omitted, benchmark JSON records `r2_write_through` as skipped.
To measure cloud write-through, pass `--r2-endpoint`, `--r2-bucket`,
`--r2-prefix`, and `--r2-region` with `CONARY_R2_ACCESS_KEY` and
`CONARY_R2_SECRET_KEY` set in the environment.

The benchmark runs the real conversion contract and writes CCS/CAS cache
artifacts under the supplied cache and chunk directories. Use scratch paths for
local experiments unless you intentionally want to warm a real Remi cache.
The former scan-only corpus tokenizer was removed because line splitting and
manual command lists duplicated the formal shell/parser pipeline and produced
non-authoritative evidence that required ongoing heuristic maintenance.
