---
last_updated: 2026-08-23
revision: 49
summary: Document Remi canonical candidate validation, typed support tiers, complete source universes, immutable catalogs, deterministic duplicate handling, signed endpoint-wide universe publication and activation, exact revision pinning, signing, readiness, and serving authority
---

# Remi

Remi is Conary's on-demand conversion and package-serving service. For the
limited public preview, its public repository profiles are Fedora 44, Ubuntu
26.04, and Arch. Solus is a typed candidate profile: Remi may refresh and test
its eopkg source privately, but it receives no public route, universe entry,
client seed, readiness obligation, or canonical package key. Remi converts
upstream RPM, DEB, Arch, and candidate EOPKG packages into CCS artifacts, stores converted content in the local
content-addressed store, and publishes every chunk to R2 when that durable
authority is configured.

M4d routes every `{distro}` path parameter through repository-feed profile
validation before DB queries, cache/key filesystem paths, or release-upload
trust gates. Public Remi route slugs remain `fedora`, `ubuntu`, and `arch`;
they are backed by profile route metadata rather than a local hard-coded
`SUPPORTED_DISTROS` list.

## Package Source Authority

Hosted package sources are declared in `deploy/remi-repositories.toml` and
loaded by `apps/remi/src/server/repository_manifest.rs`. Every source names one
exact known profile and carries typed parser construction data: RPM declares
architecture; Debian declares distribution, component, and architecture; Arch
declares the repository database name; JSON is explicit when a source actually
serves Conary JSON metadata.

Repository manifest schema 4 also requires each native source's typed member
role, numeric precedence, required state, exact source
identity, distinct repository identity, release/channel/rolling stream, and
closed follow-or-pin policy. An optional policy group shares one normalized
source policy while each repository retains its own member pin. Authenticated
roots are transient refresh inputs and become immutable source-catalog evidence;
they are not mutable repository revision state. Reconciliation replaces a
repository transactionally when those source inputs change; ordinary enabled,
precedence, and expiry changes preserve the enrollment. The supported-profile
catalog independently declares the exact repository identities, roles, and
precedence required for each complete profile. The hosted manifest must match
every public declaration exactly: Fedora includes release and updates; Ubuntu
includes release, updates, security, and backports across main, restricted,
universe, and multiverse; Arch includes core, extra, and multilib. The same
contract is persisted by the core repository source model
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

Native refresh publishes package authority outside the operational database.
Each authenticated source is projected into a strict standalone SQLite catalog
and a canonical `SourceSnapshotV1` manifest under
`<storage.root>/catalogs/sources/<manifest-sha256>/`. The manifest binds exact
source, repository, stream, parser-projection, authenticated root and child
objects, catalog bytes, logical digest, and row counts. A strict
`ProfileRevisionV2` then binds each member's role, precedence, required state,
ordered source identity, and one composed
profile catalog under `catalogs/profiles/<manifest-sha256>/`. Core contract and
bundle verification live in `crates/conary-core/src/repository/catalog/`.
Every serving projection resolves a package origin back to that manifest.
When two members publish the same source-independent native package identity,
profile composition retains the member with the exact higher declared
precedence. Public member precedences are unique, so no repository identity or
incidental order breaks a tie. The origins collapse only when payload digest and
every source-independent semantic and relation row agree; any disagreement is
a typed conflict and the candidate cannot publish. After request constraints
establish eligibility, every distinct native variant remains available for
native version comparison. Member precedence never hides a different version,
release, or architecture. Member order, repository names, and catalog insertion
order never select a package.

`RepositorySnapshotSink` schema 1 is the only native parser output contract.
Fedora primary/filelists XML, Debian Packages stanzas, ALPM archive records,
and eopkg Package XML are authenticated into private files and decoded one
record at a time. The sink inserts directly into the source-catalog candidate;
Fedora pkgid joins and ALPM desc/depends pairing use private indexed SQLite
state. Candidate logical hashing, count validation, reopen verification,
source/profile bundle binding, strict cache replay, and profile composition
iterate in canonical database order. They retain one scalar package record,
one normalized provide row, or one complete requirement group at a time; a
source package's relation cardinality cannot become the publication memory
bound.

Normalized source projections may be reused from
`<storage.root>/cache/native-projections/<key-sha256>/`. Cache schema 1 binds
the exact stream-binding SHA-256, authenticated root digest and size, ordered
child role/path/digest/size set, parser projection version, catalog schema,
and verified catalog binding. A miss reparses. A tampered, mixed, or
noncanonical entry is removed from this exact cache namespace and cannot become
package authority. Cache candidates are private, synchronized, and atomically
renamed; a cache fault fails the private refresh and leaves the active profile
pointer unchanged.

Construction is private beneath `catalog-candidates/<run-id>/`. Candidate
SQLite integrity, schema, ordering, counts, logical digest, and source
membership are reopened and checked before publication. Catalog and manifest
files and their directories are synchronized before an atomic rename makes the
content-addressed bundle durable. Only then may one short operational-database
transaction prove the current run owner and fencing epoch and replace the
profile's active revision pointer. A failed required member, stale fence,
replayed activation, malformed bundle, publication fault, or activation fault
leaves the previous pointer readable.
Long parser, profile-composition, and immutable publication-verification calls
renew that fenced lease from an independent coordinator thread at the
core-owned heartbeat cadence, including while the run is `ready_to_publish`,
so a CPU-bound metadata record stream cannot starve its own ownership proof.

Operational SQLite owns refresh runs and leases, resource metadata, ordered
profile members, the active pointer, and exact revision pins. It does not own
package, provide, or requirement rows for the activated native Remi catalog.
`CatalogAuthority` resolves the pointer and verified bundle, opens SQLite in
immutable read-only mode, and records a reader pin for the handle lifetime.
Universe publication performs the complete digest, integrity, binding, count,
and logical verification and seeds the serving cache with that exact reader; a
first serving open performs the same proof if no publisher has done so. Later
opens in that process share the verified read-only connection behind a bounded
per-profile cache instead of rehashing gigabytes for every lookup. A new
revision replaces that cache entry only after its own complete verification.
Readers opened before activation therefore finish on the old revision; later
readers see the complete new revision. Conversion outcomes own durable exact
revision pins. Catalog garbage collection computes reachability from active,
reader, work, and conversion pins and removes only resources absent from that
exact graph; age, repository names, process liveness, and guessed retention
windows are not collection authority. An absent exact bundle or
never-published profile namespace is idempotent absence during collection; a
symlink or non-directory at either boundary still fails closed.
Concurrent profile refreshes share one narrow catalog-collection coordinator,
so their plan, filesystem removal, and acknowledgement phases cannot consume
the same deletion intent while source retrieval, parsing, and catalog
construction remain parallel.

`apps/remi/src/server/readiness.rs` owns serving readiness. `/health` is an
unconditional liveness reply and proves only that the process is listening;
`/health/ready` is the evidence-bearing one. It opens the database read-only,
requires the expected schema revision, and requires usable typed repository and
canonical publication outcomes from the initial scheduler cycle. The validated
manifest supplies the required exact-profile policy; every required profile
must have a valid durable active pointer, strict canonical manifest, exact
two-file bundle, and a regular catalog file with the signed size and a nonzero
package count. This bounded inspection neither claims the process SQLite writer
nor rehashes the catalog; serving opens retain the complete verification
contract above. A server without an exact configured profile is not ready. It
also checks the serving directories and configured free-space floor. A probe that cannot run reports
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
manifest before opening listeners, then immediately refreshes metadata and runs
the canonical discovery fetch and exact-contract rebuild before eligible
exact-profile prewarm. Before creating storage subdirectories, opening the
runtime database, or reconciling that manifest, the server takes a nonblocking
kernel-backed exclusive lock on `.remi-runtime.lock` inside the canonicalized
`storage.root` and retains its file descriptor until the owned Tokio runtime
finishes shutdown, including outstanding blocking work. A second process
targeting the same root fails startup; lock-file text, PIDs,
timestamps, and stale-file cleanup are never ownership authority.
Deployment prepare derives and canonicalizes that same runtime root from the
validated candidate config, acquires the same kernel lock before signing-key,
backup, config, repository-manifest, or database mutation, and records the
canonical root in transition-manifest schema 2. Rollback reads that typed root
and acquires the same lock before restoring any target. The superseded manifest
shape has no compatibility reader. Deployment inspection remains read-only
evidence and never establishes quiescence or mutation ownership. Its
population report is profile-scoped: it reads package counts from each active
immutable profile manifest, counts only conversions pinned to that active
revision, and requires the fresh signed universe to name the exact same
ordered revision set. Retired mutable Remi package rows are not deployment
evidence.

`apps/remi/src/server/publication_scheduler.rs` owns startup publication order
and both periodic clocks; one process-local publication coordinator also
serializes background cycles, repository-admin mutations, MCP canonical cycles,
and package cache-miss readiness/reservation decisions. Their network, parsing,
and mutation phases therefore cannot invalidate one another's publication
decision. A queued coordinator waiter releases its server-state read guard
before awaiting ownership, so the current cycle can record readiness through
the state write boundary. The narrower database writer serializes only their SQLite mutation
phases, including catalog-pointer, canonical-cache, and exact-map commits. The process-wide root
lock excludes a second Remi runtime; durable refresh-run leases and monotonic
fencing epochs authorize private source/profile candidates and activation
inside that owner.
There is no warm-up timer or blind retry; a later cycle occurs at the configured
interval, an overdue canonical deadline is serviced immediately after the
current refresh, and each deadline resets only after its owning attempt
completes. Concurrent profile refreshes never publish partial endpoint
universes independently. The owning admin batch publishes once after its
profile set settles; the background scheduler publishes once after the
following canonical-map cycle, so one sequence binds one coherent endpoint
state.

Eligible exact-profile prewarm jobs run concurrently under the configured
conversion bound shared with request-driven conversions. Each profile preserves
its own top-N ordering and sequential conversion semantics, while a slow first
profile cannot starve the profiles after it. Multi-source refresh returns one
typed result or failure per source. A source failure remains visible but cannot
discard successful source commits or suppress prewarm for another exact
profile. Prewarm eligibility comes only from the successful result's persisted
`source_profile`; repository names, URLs, formats, and error text are not
selectors.
All conversions for one unchanged profile revision share the process's already
verified immutable catalog reader; the top-N loop does not repeat whole-catalog
digest and integrity verification for each package.

Both internal and external `POST /v1/admin/refresh` routes use the same response
projection: HTTP 200 means every source completed or was current, 207 carries a
mixed success/failure batch, and 502 means every configured source failed.
Global database/setup failures remain HTTP 500. The release deployment gate
requires exact source reconciliation, every configured profile populated in
its active immutable catalog, the fresh signed universe matching those
revisions, and at least one validated converted artifact pinned to every
current profile revision. Result and failure arrays are sorted by exact repository name,
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
it around each short operational mutation. Signed-universe network transfer,
verification, private index construction, CCS emission, CAS work, and read-only
lookups stay outside the gate; one fenced transaction activates the complete
universe.

## Signed Client Universe

The Conary `remi` strategy synchronizes one endpoint-wide signed immutable
universe. `RemiUniverseManifestV2` binds one monotonic sequence, the complete
ordered set of public `ProfileRevisionV2` catalogs, the exact canonical-map
object, every content digest and size, schema versions, row counts, generation
time, expiry, and the dedicated metadata-root digest. The universe
`targets` role authorizes exactly that manifest and its digest-addressed
objects; extra, missing, repeated, reordered, mixed, wrong-sized, or
wrong-schema authority fails before activation.

Publication writes and synchronizes every referenced object and the signed
`root`/`targets`/`snapshot`/`timestamp` metadata beneath one immutable bundle,
reopens the complete bundle, then advances `remi_active_universe_revision` in
one short transaction. A publication fault, stale base pointer, or damaged
active bundle leaves the previous universe selected. An unchanged authority is
reused until either its manifest or timestamp enters the six-hour renewal
window; renewal advances the sequence and refreshes signed freshness rather
than serving authority near expiry.

Canonical-map schema validity is insufficient publication evidence. Before a
new universe is signed, Remi independently reopens every exact public profile
catalog and requires each canonical implementation's literal package name to
exist in that exact profile revision. It repeats the same cross-object proof
after the durable universe bundle is reopened. Presence uses the indexed exact
package name only; provides, aliases, descriptions, case folding, and discovery
caches cannot satisfy it. A missing profile or package is a typed failure, does
not create a replacement universe bundle, and preserves the prior active
pointer. This gate is structurally bounded to one verified reader per public
profile plus scalar implementation facts; it does not materialize a
distribution-sized package-name set.

Clients enroll the universe metadata root independently of CCS package keys.
Self-hosted `repo add` requires `--remi-metadata-root` from an independently
authenticated channel. The canonical `https://remi.conary.io` ceremony root is
embedded in the release authority catalog, selected only by that exact origin,
and enrolled automatically by `conary system init` or canonical `repo add`.
Supplying `--remi-metadata-root` for that origin is an override attempt and
fails before mutation. Sync first verifies the TUF chain, freshness, root
digest, target set, and rollback/fork rules. It streams only missing
digest-addressed catalog or canonical objects to private immutable files. A
manifest-identical sync updates verified freshness state without replacing a
catalog.

Candidate construction verifies every signed catalog's immutable SQLite
artifact, physical schema, integrity, manifest binding, and relational counts.
The Remi publisher already performed the canonical logical/schema replay before
the dedicated universe role signed those exact bytes. Configured profiles are
copied SQLite-to-SQLite into one private immutable resolution index; the
canonical map streams one entry at a time into the same candidate. Secondary
indexes are built after bulk replay, and the append-only candidate must have no
free pages before integrity verification and durable publication; finalization
does not perform a redundant whole-file compaction. One fenced operational
transaction records the immutable object/index identities, selects the complete
universe, advances repository timestamps, and removes retired mutable Remi
package and canonical rows. Operational SQLite retains repository configuration,
independent trust enrollment, verified TUF state, object identities, and the
active pointer; it is not Remi package, provide, requirement, or canonical-map
authority.

Each database connection attaches the index selected when that connection
opens and shadows the `resolved_*` views locally. Existing readers therefore
finish against their open immutable inode while later readers attach the new
index. Reachability collection first removes inactive metadata in a fenced
transaction, then unlinks retired indices and objects; an activation that wins
during collection cancels the stale deletion candidate. Per-name sparse routes
remain read-only browsing surfaces. The whole-client bulk
`GET /v1/index/{distro}` protocol, sparse candidates, mutable replacement
tables, and independent canonical-map client fetch have been deleted.

The generated client proof holds one package name while independently scaling
512 versions, 10,000 provides, 10,000 requirement atoms, and 4 MiB expression
and presentation fields. Direct normalized replay retains SQLite pages rather
than package or relation vectors; the measured debug-test high-water mark is
73,728 KiB under the fixed 262,144 KiB limit. Catalog logical verification also
streams provides, requirement groups, and atoms in canonical row order.

## Durable Repository Signing Authority

`apps/remi/src/server/signing_authority.rs` owns Remi's durable CCS and TUF
signing authority. Deployment derives the unique exact profiles from the typed
repository manifest and provisions one complete Ed25519
`targets`/`snapshot`/`timestamp` role set per profile under
`/conary/repository-keys/<exact-profile>/`. The root and profile directories
are mode `0700`, private files are `0600`, and public files are `0644`. A
profile role set is staged, synced, and atomically published; a repeat deploy
validates and preserves the existing bytes.

The endpoint-wide universe has a separate `universe/` namespace containing
dedicated `root`, `targets`, `snapshot`, and `timestamp` key pairs plus one
canonical self-signed `root.json`. `remi deployment
initialize-universe-authority` is the explicit first ceremony; it atomically
creates and synchronizes that public root and prints only its path. Deployment
prepare and inspection validate the exact persisted root, role thresholds,
public/private agreement, ownership, modes, expiry, and canonical bytes.
Universe publication loads this durable root verbatim, so a timestamp chosen
during later publication cannot silently change the client trust anchor. CCS
package keys cannot sign universe metadata.

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
`fedora-44`, `ubuntu-26.04`, and `arch`.

The public half of each deployed `targets` key is the client-side CCS package
authority. Conary releases track the current canonical
endpoint/exact-profile sets in
`crates/conary-core/src/repository/remi_authority/catalog.toml`; private keys
never enter that catalog or a client response. `conary system init` and an
exact canonical `repo add` persist those pins with the repository before it is
visible. Remi refresh changes only immutable source/profile catalog revisions
and preserves the pins. Installation then verifies a downloaded CCS against the active keys for
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

The browsing response carries `X-Conary-Canonical-Sha256` for the exact bounded
body and `X-Conary-Canonical-Revision` for its content revision. Conary sync
instead verifies the digest, size, revision, and entry count bound by the
signed universe manifest before streaming the object into the private
candidate index. The parser denies unknown fields, unsupported schema versions,
route aliases, unknown profiles, duplicate keys, duplicate identities, and
empty or conflicting mappings.

Universe activation makes Remi-owned mappings visible with the package catalogs
from the same manifest while preserving non-conflicting local `Contract`
authority. An identical Remi mapping cannot demote a contract; an exact local
contract can supply the same mapping. Any package-name disagreement rejects the
complete candidate. The current schema also makes AppStream IDs unique, removes
the unused implementation repository
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
distro/name/version/architecture identity, exact 256-bit input profile
revision, authenticated CCS transport envelope, total size, content hash, and
CCS path. The repository-provide digest is retained only as diagnostic evidence;
the immutable catalog record and exact input revision own package metadata and
cache identity. Row persistence atomically creates a durable conversion pin to
that revision, and deletion removes the row and pin in the same transaction.
Public, OCI, index, search, chunk, and garbage-collection paths validate the
typed artifact and its exact pin instead of filling missing fields with guesses
or consulting mutable operational package rows. Schema revision 49 makes this
identity required, replaces the previous latest-profile conversion key, and
removes the mutable latest-authenticated-snapshot repository field. It is a
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
summary are current, its exact profile revision has a valid durable conversion
pin, and its CCS/CAS objects exist. Refresh never rewrites or reconciles a
conversion against a later profile. Cold conversion owns one pinned profile
reader from lookup through authenticated download, parsing, CCS emission, and
the atomic outcome-and-pin transaction. Activation may advance concurrently,
but it cannot change that conversion's source package or revision.
Repository conversion cache identity is the exact algorithm-prefixed checksum
from the verified immutable package record plus the complete profile-revision
SHA-256. The downloader verifies bytes against that checksum before conversion.
Remi separately computes SHA-256 over the
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
profile, opens its active immutable catalog, and validates conversions against
that handle's exact revision and durable pin. A stale row is reconverted;
malformed or unpinned current state is surfaced as a server data error.
Conversion has no operator promotion state or alternate serving lane.

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

- `catalog_refresh.rs`: private source/profile candidate construction, durable
  content-addressed publication, and fenced activation inputs.
- `catalog_authority.rs` and `profile_catalog.rs`: active-pointer resolution,
  verified immutable readers, reader-lifetime pins, and serving projections.
- `catalog_gc.rs`: exact active/work/reader/conversion reachability and bundle
  deletion after operational intent is durable.
- `handlers/detail.rs` and `handlers/detail/catalog.rs`: detail and analytics
  response assembly, with one exact profile pin per response and catalog-owned
  package/version metadata.
- `delta_manifests.rs` and `delta_manifests/tests.rs`: exact-revision delta
  cache authority and its focused corruption/revision test corpus.
- `chunk_gc.rs` and `chunk_gc/tests.rs`: signed transport/revision-pin object
  reachability and focused local/R2 collection proofs.
- `conversion/workflow.rs`: cold/hot package conversion orchestration and
  timing.
- `conversion/types.rs`: public conversion result DTOs, scriptlet package
  metadata projection, and conversion benchmark evidence records.
- `conversion/benchmark.rs`: benchmark sampling, scan-only scriptlet evidence,
  and benchmark conversion wrappers.
- `conversion/lookup.rs`: exact immutable-catalog package selection, verified
  source-snapshot binding, prepared key-material lookup, and upstream download.
- `conversion/metadata.rs`: safe CCS filenames, profile-backed parser dispatch,
  metadata construction, catalog package identity application, and typed
  provide comparison.
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
arguments, it selects three distinct, not-currently-converted records from one
pinned active profile catalog: the smallest positive-size artifact, the median
artifact by size, and the largest artifact. The emitted source checksum,
version, architecture, and byte size pin the subjects selected from that exact
immutable profile revision.

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
