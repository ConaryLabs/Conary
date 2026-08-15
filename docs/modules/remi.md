---
last_updated: 2026-08-15
revision: 23
summary: Document Remi source identity and update policy, sparse sync, signing, canonical-map, repository trust, reproducible conversion profiling, publication, and serving authority
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

Remi opens SQLite once for each HTTP page, selects all visible package/version
rows for the page together, and batch-loads their normalized provides and
requirement groups. Historical zero-sized discovery placeholders are excluded
by one shared wire threshold used by the name count, name page, bulk page, and
per-name lookup; every listed name is therefore fetchable and `total` counts
that exact set.

Each bounded page is written to a disabled staging repository. The previously
synced repository remains the only enabled snapshot while network and parsing
work continues. After the declared name total has been consumed, one SQLite
transaction replaces the old rows, moves the staged rows to the repository,
links canonical IDs, and advances `last_sync`. A fetch, parsing, duplicate
identity, or persistence error returned to the running command removes its
stage and leaves the prior enabled snapshot unchanged.

Sparse pages do not yet carry a server-state revision. Stable totals and global
name ordering detect structural drift, but a same-count or content-only Remi
update can currently span requests. Process termination can also leave a
disabled stage because in-process error cleanup is not crash recovery.
[Issue #163](https://github.com/ConaryLabs/Conary/issues/163) owns the typed
server revision plus the durable per-repository lease required to reject mixed
page sets, serialize publication, and prove an abandoned stage before cleanup.

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

Server conversion has two terminal outcomes: ready or failed. A ready
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

When R2 flags are omitted, benchmark JSON records `r2_write_through` as skipped.
To measure cloud write-through, pass `--r2-endpoint`, `--r2-bucket`,
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
