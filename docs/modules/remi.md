---
last_updated: 2026-08-21
revision: 30
summary: Document Remi source identity and update policy, revision-pinned durable sparse sync, signing, canonical-map, repository trust, publication coordination and readiness, database-writer ownership, reproducible conversion profiling, R2 durability inventory, and serving authority
---

# Remi

Remi is Conary's on-demand conversion and package-serving service. For the
limited public preview, its configured public repository feeds are Fedora 44,
Ubuntu 26.04, and Arch. It converts upstream RPM, DEB, and Arch packages into
CCS artifacts, stores converted content in the local content-addressed store,
and publishes every chunk to R2 when that durable authority is configured.

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

Repository manifest schema 3 also requires each native source's exact source
identity, distinct repository identity, release/channel/rolling stream, and
closed follow-or-pin policy. An optional policy group shares one normalized
source policy while each repository retains its own authenticated snapshot and
pin. Reconciliation replaces a repository transactionally when those source
inputs change; ordinary enabled, priority, and expiry changes preserve the
enrollment. The same contract is persisted by the core repository source model
and is not inferred from the profile, URL, or display name.

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
requires the expected schema revision, and requires usable typed repository and
canonical publication outcomes from the initial scheduler cycle. The validated
manifest supplies the required exact-profile policy; persisted enabled
repositories and packages must populate every required profile, and a server
without an exact configured profile is not ready. It also checks the serving
directories and configured free-space floor. A probe that cannot run reports
`unavailable` rather than success, so an unmeasurable resource never reads as
ready. A public package cache miss before that profile is populated returns the
typed retryable `REPOSITORY_NOT_READY` 503 response and creates no conversion
job. Deploy verification and `scripts/remi-health.sh` assert `ready == true`
from that endpoint; liveness alone is not deployment evidence. The free-space
floor is `storage.readiness_min_free`, defaulting to 10 GiB.

`apps/remi/src/deployment.rs` owns recoverable config/schema transitions and
read-only deployment inspection. It snapshots a current database or retires an
old schema epoch before replacement, so health failure can restore config,
source authority, and SQLite state together. Startup reconciles the installed
manifest before opening listeners, then immediately refreshes metadata and
runs the canonical discovery fetch and exact-contract rebuild before eligible
exact-profile prewarm. `apps/remi/src/server/publication_scheduler.rs` owns that
startup order and both periodic clocks; one process-local publication
coordinator also serializes background cycles, repository-admin mutations, MCP
canonical cycles, and package cache-miss readiness/reservation decisions. Their
network, parsing, and mutation phases therefore cannot invalidate one another's
publication decision. The narrower database writer serializes only their
SQLite mutation phases, including canonical cache and exact-map commits. There
is no warm-up timer or blind retry; a later cycle occurs at the configured
interval, an overdue canonical deadline is serviced immediately after the
current refresh, and each deadline resets only after its owning attempt
completes.

Eligible exact-profile prewarm jobs run concurrently under the configured
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

`apps/remi/src/server/database_writer.rs` owns the process-local gate for short
SQLite mutation phases. `ServerState` gives that same owner to repository CRUD,
repository-sync persistence, analytics flush and aggregate refresh,
authentication-token touches, and conversion publication. Repository create,
update, and delete acquire it before opening an immediate transaction, and the
multi-statement source replacement commits before releasing it. The core
path-based repository-sync API requires an explicit writer authority and holds
it around each TUF, package, sparse-stage, and canonical-map commit. Network,
parsing, CCS emission, CAS work, and read-only lookups stay outside the gate.

## Sparse Client Sync

The Conary `remi` repository strategy consumes only Remi's sparse index. It
pages through
`GET /v1/index/{distro}?page=N&per_page=128&include=versions`. The typed
expansion returns one resolution-only document per name, including every
version plus the exact normalized provides and grouped native requirements
needed for offline resolution, together with persisted package metadata such
as explicitly trusted security advisories. Conversion-cache state, diagnostic
scriptlet summaries, and content hashes stay on other public projections
because sync does not consume them. Both sparse list projections identify the
exact `source_profile` behind the stable family route, and the client rejects a
page whose profile disagrees with its configured target. The former unbounded
`GET /v1/{distro}/metadata` route has been removed; health and sync use bounded
sparse pages rather than constructing repository-wide JSON.

For Fedora, normalized provides include every package-owned `<file>` record
from authenticated `primary.xml` and `filelists.xml` as `kind = "file"` with
exact `source-derived-file` provenance. The sparse page and per-name lookup
project that persisted typed row unchanged; Remi does not derive file
providers from package names or filter them through its own path rules.
Complete file ownership is what makes a path dependency outside createrepo's
primary filter solvable, and it is the dominant row population: Fedora 44
`Everything/x86_64` carries 9.5M file providers against 76,354 packages, so
sparse pages, their wire payload, and the replace transaction all scale with
it.

Remi builds each `include=versions` page inside one SQLite read transaction,
selects all visible package/version rows for the page together, and batch-loads
their normalized provides and requirement groups. The page carries a typed
revision scoped to the exact public profile: a projection-schema version, a
monotonic sequence, and a strict 128-bit state identity that cannot collide
with the same sequence after a database rebuild or replacement. Persisted
triggers advance that revision whenever visible repository membership, exact
package identity, size, package metadata, provides, or grouped requirements
change; disabled candidate writes do not advance public authority. Historical
zero-sized discovery placeholders are excluded by one shared wire threshold
used by the name count, name page, bulk page, and per-name lookup; every listed
name is therefore fetchable and `total` counts that exact set.

`crates/conary-core/src/repository/sync/remi/path.rs` owns the path-based sparse
sync writer-authority handoff;
`crates/conary-core/src/repository/sync/remi/run.rs` owns its durable lifecycle.
Every run records the repository scope, process instance UUID, monotonically
increasing fencing epoch, disabled candidate repository ID, input and candidate
revisions, typed state and failure, and start/heartbeat/lease/finish facts.
Each persisted page renews the lease. The client pins the first server revision
and rejects any later page whose revision differs, including same-total
package, version, relation, or metadata mutations.

The previously synced repository remains the only enabled snapshot while
network, parsing, and validation work continues. Publication first commits a
`ready_to_publish` state, then one SQLite transaction proves the run still owns
the scope's current process identity and fencing epoch, replaces the old rows,
moves the candidate rows, links canonical IDs, advances `last_sync`, records
the active revision, and marks the run published. A stale worker can continue
computation but cannot publish. Returned failures abandon and remove their
exact candidate. After process termination, the next run recovers only the
candidate ID named by an expired durable lease; repository-name prefixes and
candidate age never authorize cleanup.

The fixed name page is the structural owner of distribution scaling:
distribution growth increases page count, not the retained metadata set or
HTTP request count within one page. Relation collections remain exact native
semantics and are never truncated to satisfy a guessed byte ceiling. The name
page does not yet structurally bound independently growing version, relation,
expression, or metadata cardinality; [issue
#164](https://github.com/ConaryLabs/Conary/issues/164) owns typed sub-page
continuation plus streaming server/client processing. The client validates
stable totals, global name ordering, page identity, non-empty version sets, and
unique version/release/architecture identities before a page enters the
staging repository.

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
`fedora-44`, `ubuntu-26.04`, and `solus`.

The public half of each deployed `targets` key is the client-side CCS package
authority. Conary releases track the current canonical
endpoint/exact-profile sets in
`crates/conary-core/src/repository/remi_authority/catalog.toml`; private keys
never enter that catalog or a client response. `conary system init` and an
exact canonical `repo add` persist those pins with the repository before it is
visible. Remi sparse sync changes only the package snapshot and preserves the
pins. Installation then verifies a downloaded CCS against the active keys for
its exact repository provenance, so a key for one profile cannot authorize
another. The self-hosted key option cannot replace canonical catalog authority.

Self-hosted Remi has no implicit ConaryLabs authority. Its operator must move
the appropriate `targets.public` file over an independently authenticated
administrative channel and pass it to `conary repo add --ccs-package-key`.
Serving that key beside the package would not establish trust. Key rotation
therefore requires a coordinated client pin update; public TUF routes do not
stand in for a configured and verified root.

## Canonical Map Exchange

Remi builds canonical package equivalence only from the versioned literal
contracts loaded by `apps/remi/src/server/canonical_job.rs`. Repology remains a
discovery cache. AppStream may enrich an already-authorized canonical identity
with one unique application ID, but it cannot create an implementation row.
`apps/remi/src/server/canonical_fetch.rs` returns a typed persisted-or-failed
outcome for each discovery source and a typed rebuild outcome. Two persistence
failures therefore report a failed cycle, never a successful zero-entry fetch;
the exact-contract rebuild still runs because discovery does not own mapping
authority.
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
but accepted CCS v3 uploads are stored in `native_package_publications` and
projected into `repository_packages`; they are not synthetic
`converted_packages` rows. Native uploads stage privately, run the shared static
publish gate against `release_publish.trusted_build_attestation_signers`.
There is no parallel `/v1/admin/packages/{distro}` publication route.
After structural parsing, signature/trust verification, and the shared static
publish gate pass, Remi validates signed CCS v3 lifecycle authority
structurally. The route selects a repository feed, never a destination
compatibility policy. Unsupported route slugs fail before storage or artifact
verification. Package rows, native rows, chunks, and TUF targets are published
only after the gate, structural lifecycle validation, and metadata commit pass.

Client-side publication routing is parsed once by
`apps/conary/src/commands/publish/target.rs` and shared by the CLI and
packaging agent service. Only an exact hierarchical HTTP(S)
`/v1/admin/releases/{route}` path, without user information, a query, or a
fragment, selects this authenticated mutation path. A matching substring in
another URL is not route authority.

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

RPM, Debian, Arch, and eopkg conversions embed the current `native_lifecycle` bundle
in the generated CCS manifest and persist its aggregate summary on
`converted_packages`. The row records lifecycle fidelity, evidence digest,
formal unknown-command evidence, and diagnostic classes. Entry presence in the
validated bundle is lifecycle authority; the summary carries no duplicated
entry-decision count. There is no scriptlet publication-status projection.
The current schema separates installed conversions from repository-serving
artifacts with a required discriminator. Installed rows require an exact trove
identity and cannot carry serving fields. Repository rows require their exact
distro/name/version/architecture identity, authenticated CCS transport
envelope, total size, content hash, CCS path, and SHA-256 digest of the
normalized repository-provide cache
projection. That digest invalidates cached conversions when repository metadata
changes, but repository rows never mutate identity or capabilities parsed from
the authenticated source artifact. Public, OCI, index, search, chunk, and
garbage-collection paths validate that typed artifact instead of filling
missing fields with guesses or empty values. Architecture and the repository
provide digest are required constructor and API-view fields, and the current
schema rejects missing, empty, or malformed values. Schema revision 40 replaces
the retired chunk-list columns with the signed transport envelope. It is a
pre-alpha hard cut: prior databases are rebuilt and
re-ingested from configured repository authority rather than migrated. Local
conversion tracking is written only after the CCS install transaction commits.

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

Package detail, generated indexes, search, OCI, delta, and download routes
expose the sanitized `scriptlets` projection. Sparse index
documents carry resolution authority—versions, provides, requirements,
architecture, size, persisted package metadata, conversion state, and public
content hash—rather than the diagnostic scriptlet summary. Program bodies and
local filesystem paths are not part of any public response.

### Current Converted Artifact Serving

Server conversion has three terminal outcomes: ready, failed, or cancelled. A ready
conversion is advertised and served when its conversion version and lifecycle
summary are current, its stored repository-provide cache digest exactly matches
the current normalized repository metadata, and its CCS/CAS objects exist. A native
metadata refresh removes mismatched conversion rows in the same SQLite
transaction. Cold conversion carries one immutable repository-metadata digest
through cache lookup and persistence, then revalidates it under the database
write transaction so a concurrent refresh cannot publish stale source input.
Repository conversion cache identity is the exact algorithm-prefixed checksum
from authenticated repository metadata. The downloader verifies bytes against
that checksum before conversion. Remi separately computes SHA-256 over the
downloaded artifact for CCS provenance and emission; that CCS digest never
replaces the repository checksum used for cache and refresh authority.
CCS identity and capabilities come solely from the downloaded artifact. CCS
cache files use their emitted content hash as the local filename
instead of a mutable package-name/version slot. Lifecycle program content,
diagnostic classes, package names, and command-name matches do not create a
second serving decision.

Only pending and converting jobs own in-memory package-key deduplication. A
failed or cancelled job remains available through its original poll URL with
its exact outcome, but releases the key immediately so the next package or
download request creates a fresh attempt. Ready jobs also remain pollable by ID; reuse
is decided by the persisted converted-package lookup and its current-artifact
validation, never by stale in-memory job state. Accepted responses publish the
actual pending or converting state rather than flattening both to converting.

Every route resolves the public route slug to one exact persisted source
profile and uses the same current-row validation. A stale row is reconverted;
malformed current state is surfaced as a server data error. Conversion has no
operator promotion state or alternate serving lane.

Local object visibility in `server/publication.rs` is a reachability check:
native and converted publication references come from their signed transport
envelopes, and objects are served only when a current validated publication
references them. Stale-only
and unreferenced local cache objects remain private. It does not classify
lifecycle program text.

Sparse-index and search responses use `converted=true` only for current,
validated conversions. The package response and completed conversion job carry
the same authenticated transport envelope. Conary authenticates it with the
repository targets keys before object fetch, reuses permanent-CAS hits, fetches
only missing exact objects, verifies reconstruction, and hands a temporary
deterministic carrier to install or update. Lifecycle execution remains the client's typed
transaction responsibility; serving a lifecycle-bearing CCS does not bypass
client preflight.

Remi has no independent chunking toggle or boundary-size configuration. The
signed CCS layout owns whether a file is whole-object or FastCDC and owns the
canonical FastCDC profile; server configuration cannot override or fork it.

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
- `conversion/storage.rs`: signed CCS verification, exact local CAS object
  persistence, and missing-only optional R2 write-through.
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

Remi includes a local benchmark command for measuring exact cold and warm
conversion work before making public latency claims. With no `--package`
arguments, it selects three distinct, not-currently-converted repository rows:
the smallest positive-size artifact, the median artifact by size, and the
largest artifact. The emitted source checksum, version, architecture, and byte
size pin the subjects selected from the current authenticated repository
snapshot.

```bash
cargo run -p remi -- conversion-benchmark \
  --db /var/lib/conary/conary.db \
  --chunk-dir /var/lib/conary/data/chunks \
  --cache-dir /var/lib/conary/data/cache \
  --distro fedora \
  --hardware-label remi-production-i7-8700-raid1 \
  --iterations 2 \
  --jsonl
```

Explicit repeated `--package` arguments replace automatic size-class
selection. An explicit package may already be converted, so its records are a
cold baseline only when `cache_state` says `cold`; the tool never relabels a
hot result. Each sample is resolved to one exact version and architecture
before its iterations start, and the run fails if that source identity changes
between selection and conversion.

Schema-v1 evidence records the operator-defined hardware label, Remi version,
source commit and dirty state, OS, kernel, CPU model, logical CPU count, and
memory size. A commit-worthy baseline requires an exact `source_commit`,
`source_dirty: false`, and the full JSONL output. Per-sample evidence separates
phase latency from deterministic work: downloaded and hashed source bytes,
CCS and signed-object sizes, verified-CAS hits, misses, bytes, and durability
calls, plus R2 HEAD hits/misses, PUT count, and bytes written. These counters
are regression inputs; they do not weaken verification or storage authority.
The first committed small/median/multi-GiB baseline and its measured
optimization are recorded in [performance evidence](../performance/README.md).

When benchmark R2 arguments are omitted, benchmark JSON records the historical
schema-v1 phase name `r2_write_through` as skipped. To measure cloud durability,
pass `--r2-endpoint`, `--r2-bucket`,
`--r2-prefix`, and `--r2-region` with `CONARY_R2_ACCESS_KEY` and
`CONARY_R2_SECRET_KEY` set in the environment.

The benchmark runs the real conversion contract and writes CCS/CAS cache,
conversion-database, and optional R2 state through the normal service path.
Use a copied database plus scratch cache/chunk paths and a benchmark R2 prefix
for experiments. Point it at live state only when the resulting conversions
and cache warming are intentional operator actions.
The former scan-only corpus tokenizer was removed because line splitting and
manual command lists duplicated the formal shell/parser pipeline and produced
non-authoritative evidence that required ongoing heuristic maintenance.

## Chunk Garbage Collection

`apps/remi/src/server/chunk_gc.rs` owns the referenced-set computation and the
local/R2 deletion pass. `admin_service::run_chunk_gc_op` is the single caller:
it resolves the database path, chunk objects directory, and optional R2 store
from server state, applies the one-hour grace period that protects chunks of
in-flight conversions, and returns the typed report.

The live referenced set is derived from current converted and public native
transport envelopes plus explicitly protected cache rows. Malformed envelope
state fails the scan; GC never falls back to a parallel hash list. Object sizes
inside delta manifests likewise come from the signed envelope, while
`chunk_access` remains cache/grace/accounting metadata only.

Two surfaces call that one function, so neither can drift from the other. The
MCP tool `chunk_gc` is the agent surface, and `POST /v1/admin/chunk-gc` on the
external admin router is the operator surface; both require the `admin` scope.
Deletion requires an explicit `dry_run: false`, so an omitted body, an omitted
field, or an absent MCP parameter previews the run instead of deleting.

## R2 Durability Inventory And Backfill

`apps/remi/src/server/r2_durability.rs` owns the typed comparison between the
local CAS, R2, and the object identities and sizes named by persisted converted
and public-native transport envelopes. It does not infer durability from the
`chunk_access` cache index. A malformed transport, a repeated object with a
contradictory size, or a non-canonical chunk key fails the inventory instead of
silently reducing the required set.

The operator surface is `POST /v1/admin/r2-durability`; the agent surface is
the MCP `r2_durability` tool. The external HTTP and MCP routes require admin
authority. The internal admin listener exposes the same typed operation only
through its loopback transport boundary, allowing host-local automation to
operate without copying a bearer token. All surfaces emit schema-v1 reports
with exact total and required object counts and bytes. An omitted body or mode
is read-only `plan`. `apply` uploads only required local objects absent from R2,
or required objects whose R2 size disagrees with local storage, with 1 to 64
concurrent PUT requests. Each local object is SHA-256 verified against its path
before upload. Apply then lists R2 again, and `r2_complete` is true only when
every required identity has the exact authority-declared size in that fresh
listing. Missing-from-both identities, unrepairable size contradictions, and
upload failures are counted in full with at most ten bounded diagnostic samples
per class.

`.github/workflows/remi-r2-durability.yml` is the production operator adapter.
It accepts only an exact commit already merged into `main`, runs through the
protected production environment and SSH boundary, invokes the loopback route,
and retains a public-sanitized aggregate report with all diagnostic samples
removed. Apply fails the workflow unless the fresh post-upload report is
`applied_complete` with `r2_complete: true`.

When `[r2].enabled = true`, R2 is the only durable chunk authority. Startup
requires an explicit endpoint and usable credentials; it does not continue in
a local-durable mode when R2 initialization fails. Conversion publishes chunks
to R2 before it publishes converted-package state. Public chunk `GET` requests
return an R2 presigned redirect, including requests carrying `Range`; `HEAD`
and missing-object checks query R2. Local presence never masks an absent or
unreachable R2 object. Such disagreement returns HTTP 503 with the stable
`x-conary-error: durable-chunk-unavailable` marker, which the core client maps
to `DurableChunkUnavailable` without trying another repository.

The local chunk directory is a bounded LRU cache owned by
`apps/remi/src/server/bounded_cache.rs`. Conversion completion and the hourly
maintenance loop enforce `storage.max_cache_size`; each candidate is checked
against R2 immediately before unlink. Missing or unreachable durable state
fails closed, protected objects remain local, and failure to reach the exact
byte bound is an error. The retired `r2_redirect`, `write_through`,
`eviction_threshold`, eviction-age, and batch-chunk modes are rejected instead
of preserving multiple storage policies. With R2 disabled, Remi is an explicit
local-only deployment and bounded eviction is unavailable because no second
durable copy exists.

## MCP

The external admin `/mcp` endpoint is a modern-only, stateless Streamable HTTP
surface using rmcp 3.1.2 and protocol revision `2026-07-28`. Every POST carries
the protocol version and client metadata in `_meta`; Remi does not negotiate an
`initialize` session, issue `Mcp-Session-Id`, retain session state, or serve the
MCP GET and DELETE operations. The per-request handler shares Remi's state
through the existing `Arc<RwLock<ServerState>>`, and every request remains
behind the admin Bearer-token middleware.

Origin validation permits `http://localhost` and `http://127.0.0.1`, including
the default internal `8081` and external `8082` admin-port variants. The
allowlist is fixed at these defaults; an operator who moves the admin ports in
`remi.toml` must extend it in `create_mcp_router` (browser origins on other
ports are rejected; CLI and curl clients send no `Origin` and are
unaffected). A
missing `Origin` is accepted for CLI and curl clients. Host validation keeps
rmcp's default loopback allowlist; public hostnames are intentionally not added
without a separately reviewed exposure decision. Changes to this MCP contract
must update this module's frontmatter `revision` and its router-level protocol
tests together.
