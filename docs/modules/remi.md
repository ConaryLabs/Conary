---
last_updated: 2026-08-28
revision: 79
summary: Document constant-time coherent typed deployment baselines, zero-copy same-schema deployment rollback and phase-timed failure evidence, exact immutable profile reuse for unchanged ordered source members, bounded authenticated response-body recovery, latest-successful private-candidate retention, exact-profile deployment retry, linear profile composition and catalog relation verification, single-pass private-stage catalog reader reuse, immutable retention and network-free export of exact authenticated native metadata, exact private-candidate native-oracle input materialization, typed and causally inspectable private-candidate and active-repopulation deployment completion, complete pre-write native source- and profile-candidate growth admission, typed exact-chunk admission for unknown-length Arch and eopkg metadata, the stopped-runtime promotion-proof operator, evidence-bound atomic public promotion, durable private refresh candidates, stopped-runtime configured-durability candidate-selected conversion crawling and promotion evidence, complete Conary candidate resolution evidence, independent persisted CCS reopen proof for the strict zero-exclusion public-universe conversion crawl, pinned ALPM, RPM, and Debian native full-catalog package-fact and resolution parity, canonical candidate validation, typed support tiers, complete source universes, immutable catalogs, deterministic duplicate handling, signed endpoint-wide universe publication and activation, exact revision pinning, signing, readiness, and serving authority
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

Native refresh writes immutable candidate package authority outside the
operational database.
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
a typed conflict and the candidate cannot publish. Debian `distribution` and
`component` remain typed, inspectable source-pocket provenance on the selected
record rather than package semantics, so the same authenticated artifact may
collapse across security and updates pockets without discarding where the
higher-precedence copy came from. After request constraints establish
eligibility, every distinct native variant remains available for native version
comparison. Member precedence never hides a different version, release, or
architecture. Member order, repository names, and catalog insertion order never
select a package.

`RepositorySnapshotSink` projection schema 2 is the only native parser output
contract. Each parser gives the sink the exact run-local file that supplied
every authenticated metadata-object fact; the immutable sink transfers those
verified bytes into the source candidate before parser work-file cleanup.
Fedora primary/filelists XML, Debian Packages stanzas, ALPM archive records,
and eopkg Package XML are authenticated into private files and decoded one
record at a time. The sink inserts directly into the source-catalog candidate;
Fedora pkgid joins and ALPM desc/depends pairing use private indexed SQLite
state. Candidate logical hashing, count validation, reopen verification,
source/profile bundle binding, exact cache materialization, and profile
composition iterate in canonical database order. They retain one scalar package record,
one normalized provide row, or one complete requirement group at a time; a
source package's relation cardinality cannot become the publication memory
bound.

Logical verification merges one ordered cursor for packages, provides,
requirement groups, and requirement atoms. Catalog cardinality therefore
changes rows stepped while the verifier retains a fixed four SQLite statements;
it never prepares relation queries per package or group. Missing package or
group owners fail as typed catalog conflicts, and the emitted V1 logical digest
and row counts remain byte-for-byte identical to canonical catalog content.

Profile composition likewise opens one ordered cursor for each source relation
table and merges those rows with the ordered package cursor. It does not issue
provide, requirement-group, or requirement-atom queries per source package,
and it keeps one prepared destination statement per relation table for the
entire member replay. Exact duplicate identities still compare every intrinsic
package and relation field; their indexed destination reads reuse cached
statements and any disagreement fails the private candidate before publication.

Source-manifest finalization returns the immutable reader that proved the
catalog binding, and private staging carries that exact reader into profile
composition instead of reopening and rehashing the same source catalog.
Profile-manifest finalization likewise performs one private-stage binding
proof. Durable source and profile publication still independently reopen and
verify the complete bundle before and after its same-filesystem atomic rename.

After every source is authenticated and staged, refresh derives the complete
ordered `ProfileSourceMemberV2` contract from those verified source manifests
without visiting package rows. It inspects the latest successful private
candidate and active revision manifests in that order. Exact profile,
profile-projection version, ordinal, role, precedence, required state, source,
repository, stream, and source-snapshot identity equality makes an existing
immutable profile eligible for reuse. The selected bundle is then
independently reopened and reader-pinned; the pin remains live until the new
fenced run completes as a durable candidate. This path creates no profile
candidate file and performs no profile-catalog reconstruction. Any changed
member or projection version takes the normal private composition path, and a
malformed registered selection fails instead of becoming reuse authority.

Fedora metadata acquisition is owned by
`crates/conary-core/src/repository/parsers/fedora/metadata.rs`; the parent
parser owns normalized RPM record replay and joins. Source snapshot manifests
remain `SourceSnapshotV1`, but parser projection version 2 and the retained
metadata directory are a hard cut: projection-version-1 manifests and legacy
two-entry source bundles are rejected and must be rebuilt by refresh. There is
no compatibility reader, upstream refetch fallback, or parallel source
authority.

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
membership are reopened and checked before durable registration. Every source
bundle has exactly `catalog.sqlite`, `manifest.json`, and a private
`native-metadata/` directory containing only digest-named objects declared by
the manifest; profile bundles retain their exact two-file layout. The catalog,
manifest, retained metadata objects, and their directories are synchronized
before an atomic rename makes the content-addressed bundle durable. Only then
does one short operational-database
transaction prove the current run owner and fencing epoch, register the exact
source/profile resources, and complete the run as a terminal `candidate`.
Refresh never advances a profile or universe pointer and never updates
`last_published_at`; checked, changed, and validated timestamps describe only
the private candidate work. A failed required member, stale fence, malformed
bundle, or durable-registration fault leaves both the previous active pointer
and the previous public universe readable.
Long parser, profile-composition, and immutable publication-verification calls
renew that fenced lease from an independent coordinator thread at the
core-owned heartbeat cadence, including while the run is `ready_to_publish`.
Candidate completion ends the lease; the exact current candidate survives
restart and lease expiry as typed promotion input. A successor refresh advances
the fencing scope and fences the previous writer, but an in-flight, failed, or
abandoned successor does not erase the latest successful immutable candidate.
Only a newer completed candidate or publication supersedes that proof. The
same rule owns current-candidate lookup, promotion eligibility, and catalog-GC
roots, so retained work remains activatable rather than merely undeleted. Thus
a CPU-bound metadata record stream cannot starve its own ownership proof.

Operational SQLite owns refresh runs and leases, resource metadata, ordered
profile members, the exact current private candidate, the active pointer, and
exact revision pins. It does not own
package, provide, or requirement rows for the activated native Remi catalog.
`crates/conary-core/src/repository/sync/remi/run/candidate.rs` owns the durable
candidate transition, exact run-to-revision member proof, and current-candidate
lookup; the parent run module retains lease, heartbeat, and failure fencing.
`CatalogAuthority` resolves the pointer and verified bundle, opens SQLite in
immutable read-only mode, and records a reader pin for the handle lifetime.
Universe publication performs the complete digest, integrity, binding, count,
and logical verification and seeds the serving cache with that exact reader; a
first serving open performs the same proof if no publisher has done so. Later
opens in that process share the verified read-only connection behind a bounded
per-profile cache instead of rehashing gigabytes for every lookup. A new
revision replaces that cache entry only after its own complete verification.
Readers opened before promotion therefore finish on the old revision; later
readers see the complete new revision. Conversion outcomes own durable exact
revision pins. Catalog garbage collection computes reachability from active,
latest-successful-candidate, reader, work, and conversion pins and removes only resources
absent from that exact graph. A superseded candidate is collectable unless a
different typed pin retains it; age, repository names, process liveness, and
guessed retention windows are not collection authority. An absent exact bundle or
never-published profile namespace is idempotent absence during collection; a
symlink or non-directory at either boundary still fails closed.
Concurrent profile refreshes share one narrow catalog-collection coordinator,
so their plan, filesystem removal, and acknowledgement phases cannot consume
the same deletion intent while source retrieval, parsing, and catalog
construction remain parallel.
Before a profile candidate file exists, core independently validates every
reopened source reader against its exact snapshot manifest and derives one
canonical `CatalogProfileCandidateScratchV1` from the ordered member facts.
The requirement allocates the exact input catalog bytes once for destination
payload, once for arbitrary B-tree repacking, one fixed 4096-byte catalog page
for each input package's expanded profile-origin row, and the full input bytes
for the rollback-journal ceiling. Remi reserves that complete sum on the
candidate filesystem through the shared device ledger before file creation.
The lease remains live through member replay and committed catalog metadata and
evidence. After synchronizing the candidate file and parent, the writer proves
the actual pre-compaction database is within the separately recorded database
ceiling, releases the growth lease, and asks for the page-derived finalization
lease below. A one-byte-short refusal leaves no profile candidate file.
Before each source or profile catalog enters SQLite compaction, the catalog
writer commits its private logical state and derives the exact current database
bytes from SQLite's positive `page_size` and `page_count`. SQLite finalization
may require two additional database-sized allocations while its temporary copy
and rollback journal coexist with the original. Remi reserves that documented
worst case against a process-local ledger keyed by the owning filesystem
device, re-reading available space for every admission. Concurrent finalizers
therefore cannot collectively reserve more than the filesystem reports
available; the lease releases on success, error, cancellation unwind, or
process restart. A one-byte-short refusal is a typed `storage_capacity` refresh
failure before `VACUUM`, and candidate cleanup preserves the active revision.
When a newly finalized source catalog has no exact projection-cache entry, the
cache derives another typed requirement from the verified catalog artifact size
and the exact canonical cache-manifest bytes. It reserves those bytes on the
cache filesystem through the same ledger before creating a private stage, then
retains the lease through copy, file and directory synchronization, atomic
rename, and independent reopen. A current exact cache hit writes and reserves
nothing. Refusal is typed and leaves no cache candidate. Immutable source and
profile publication itself moves verified candidates by same-filesystem atomic
rename, so it creates no second catalog-file copy.
Before Fedora or Debian child metadata is downloaded, the authenticated root
supplies each selected object's exact compressed length: signed `repomd.xml`
records bind RPM primary/filelists bytes, while the verified Debian `Release`
SHA256 entry binds `Packages.gz`. The parser turns those role, path, and size
facts into one canonical typed requirement, reserves the candidate filesystem
through the same device ledger, and applies each signed size as the HTTP stream
cap. The immutable sink retains that lease until its run-local work directory
and every staged child file have been removed, including error and cancellation
unwind. A typed refusal therefore precedes child-file creation.
HTTP acquisition requests identity encoding. A response that succeeds at the
header boundary but truncates or fails while decoding its body is a typed
transport failure and receives only the configured bounded retry envelope;
deterministic size, trust, and wire-contract refusals are not retried. File
recovery resumes only from an exact `bytes START-END/TOTAL` response whose start
equals the staged length and whose interval and body length agree. HTTP 200
resets the staged prefix, while HTTP 416 proves completion only when
`bytes */TOTAL` exactly equals the staged length; disagreement discards the
stale prefix. Final authenticated digest and size remain authority on every
path.
When an RPM repository omits filelists metadata, the parser audits every
positive path requirement against the exact primary projection already held by
the repository sink. The immutable sink walks its private catalog transaction
one typed requirement at a time; the compatibility sink checks its collected
projection. No second SQLite audit database or other parser work file is
created for those duplicated facts.
Arch repository databases may publish each package's desc and depends records
out of order. Before candidate creation, its read-only pass accounts for each
exact raw fragment, each desc projection, and every separately published
depends relation group without assuming archive order. After growth admission,
the replay stages those exact fragments in a strict transient
table inside the private catalog candidate transaction, rejects duplicate and
orphan fragments, replays complete pairs in source-directory order, and drops
the table before finalization. The compatibility sink performs the same typed
pairing in its existing in-memory state. No separate Arch SQLite spool or
sidecar is created; its pages remain in the candidate high-water mark consumed
by finalization admission.
Arch database signatures and eopkg index digest sidecars authenticate exact
completed bytes but publish no signed size before download. Each parser binds a
typed stream subject to the exact metadata role and repository-relative source
path, then uses the download client's explicit admitted-identity path. The
shared filesystem coordinator measures current free space and reserves every
positive response chunk before its run-local write. That permit remains live
across the write and is released only after the allocation is materialized, so
filesystem free-space accounting owns completed chunks while the process
ledger fences concurrent pending writes. A one-byte-short refusal is the same
typed catalog-capacity failure as other construction admission and no
unadmitted byte reaches the staged file. Error, cancellation, and normal sink
drop remove the private work directory.
These construction admissions are independent of the serving readiness floor.
Before a fresh native source candidate file exists, Fedora, Debian, Arch, and
eopkg parsers make a bounded read-only pass over the already authenticated
metadata. The immutable sink canonicalizes the same normalized package facts
used by replay and includes Fedora filelist provides plus Arch separately
ordered requirement groups. `CatalogSourceCandidateScratchV1` allocates those
exact canonical projection bytes once for destination payload and once for
B-tree repacking, the fixed schema roots and one 4096-byte page per package,
and a full candidate-database rollback-journal ceiling. On an exact projection
cache hit, the independently reopened artifact bytes and its bound package
count replace that parser preflight. Remi reserves the complete sum through the
shared candidate-filesystem ledger before the writer creates SQLite. The lease
survives replay and committed metadata/evidence; after file and parent sync the
writer proves the actual database remains below the admitted ceiling, releases
the growth lease, and only then requests page-derived finalization scratch. A
one-byte-short refusal leaves no native candidate file. Profile candidates use
the corresponding ordered-member contract described above.

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
It is a serving-health threshold, not catalog-construction scratch admission.

`apps/remi/src/deployment.rs` owns recoverable config/schema orchestration and
read-only deployment inspection;
`apps/remi/src/deployment/database_transition.rs` owns persisted database
planning, application, and rollback. A current schema revision remains in
place across binary replacement and rollback, making the schema revision the
explicit persisted-data compatibility boundary and performing no complete
database copy. A retired schema epoch plus WAL/SHM is moved into the transition
backup and restored exactly on rollback. Transition-manifest schema 3 is a
hard cut with no reader for schema 2; existing completed backup manifests are
historical evidence, while any unfinished old-schema transition must be
resolved with its owning binary before deploying this cut. Startup reconciles the installed
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
canonical root in transition-manifest schema 3. Rollback reads that typed root
and acquires the same lock before restoring any target. The superseded manifest
shape has no compatibility reader. Deployment inspection remains read-only
evidence and never establishes quiescence or mutation ownership. It reports
active and private-candidate state separately in canonical public-profile
order. `--require-private-candidates` requires one exact current, durable,
nonempty candidate for every public profile, reopens each strict immutable
manifest and two-file bundle, validates its typed member contract, and
re-proves its fenced run members against the current repository bindings. An
active-only profile, superseded run, candidate-tier profile, changed binding,
missing bundle, or empty catalog cannot satisfy that predicate. The protected
private-candidate deployment workflow adds causal evidence that the static
predicate intentionally does not own: it records the pre-transition
inspection, starts the exact merged binary, invokes the loopback-only forced
refresh endpoint, validates the typed completion response, retries only exact
failed public profiles through the typed `profile` scope, and requires the
final Fedora, Ubuntu, and Arch fencing epochs to be strictly newer than their
recorded baselines and each terminal run to have started after the recorded
binary transition. The final evidence binds the exact merged commit, built
binary SHA-256, completion mode, and transition timestamp; the before-and-after
sanitized inspections are retained. The versioned `deployment baseline`
surface runs from the exact SHA-256-checked staged binary and reads only current
schema, installed manifest reconciliation, relational candidate/run-member
identity, and latest typed refresh state. Candidate identity is one optional
object, so a revision, run, or completion field cannot be half-present. It
performs no signing, catalog, package, conversion, or universe inspection and
reports wall/CPU/RSS, SQLite statement and logical page-read work, zero catalog
opens/bytes, and output size. The protected workflow rejects a baseline over
two seconds or any nonzero catalog access before mutation. A profile without a
candidate contributes a null identity and its latest refresh fence but never
satisfies the strict post-transition candidate predicate. Evidence schema 2
also records the typed outcome, causal failure phase, and duration of every
completed or failed remote phase. An early remote-session or transport failure
produces a failure envelope and attempts one read-only recovery inspection
instead of discarding the causal state. The root-owned helper's read-only
`inspect-remi-storage` surface adds
before-and-after available bytes, live SQLite file count and logical/allocated
bytes, and transition-backup directory count and logical/allocated bytes. It
fails closed on symlinked or unexpected backup entries and emits no host paths.
A fresh Solus candidate cannot replace any
public-profile advance, a typed Solus refresh failure cannot block the public
completion set, and a preexisting valid public candidate cannot make a new
binary deployment green.
`--require-repopulated` remains the post-promotion contract: it reads package
counts from each active immutable profile manifest, counts only conversions
pinned to that active revision, and requires the fresh signed universe to name
the exact same ordered revision set. Retired mutable Remi package rows are not
deployment evidence, and neither predicate grants publication authority.

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
fencing epochs authorize private source/profile candidate construction.
There is no warm-up timer or blind retry; a later cycle occurs at the configured
interval, an overdue canonical deadline is serviced immediately after the
current refresh, and each deadline resets only after its owning attempt
completes. Repository refresh entrypoints never publish endpoint universes.
The one-shot promotion owner consumes the canonical promotion evidence and
complete crawl, reopens their exact candidates or already-active revisions,
and alone may change the public profile set and signed universe. No partial
candidate set becomes public merely because an admin or background refresh
completed.

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
Omitting `profile` selects every configured source. Supplying one exact
configured native profile selects only that profile and no legacy repository;
it is the retry boundary after a partial batch and cannot upgrade global
publication readiness from its profile-local result. Global database/setup
failures remain HTTP 500. The release deployment gate
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

Promotion writes and synchronizes every referenced object and the signed
`root`/`targets`/`snapshot`/`timestamp` metadata beneath one immutable bundle,
reopens the complete bundle, then uses one immediate transaction to publish
every selected candidate run, advance every changed public-profile pointer,
insert the evidence-bound universe revision, and advance
`remi_active_universe_revision`. Schema 53 stores the exact canonical
`RemiPromotionEvidenceV1` and `RemiConversionCrawlV4` digests on that universe
revision. A catalog, proof, CAS, signed-metadata, fence, canonical-map,
transaction, or reopen fault leaves the complete previous public state
selected. Exact replay returns the already-active revision. The background
publisher cannot create an initial universe or change profile or canonical
authority without promotion evidence; it may only renew signed freshness for
the exact active authority after revalidating its durable bindings.

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

### Native Oracle Input Materialization

`remi native-oracle-input` is the read-only handoff from exact production
private candidates to the pinned native package-manager producers. It requires
exact `PROFILE=SHA256` bindings in canonical Fedora 44, Ubuntu 26.04, and Arch
order. Before network or output mutation it reproves each current fenced run,
opens a durable reader pin on every exact immutable profile, and independently
reopens every ordered source snapshot catalog. Active-only revisions,
candidate-tier Solus, missing or reordered profiles, and noncanonical digests
fail closed.

Refresh retains every exact parser-authenticated metadata file in the immutable
source bundle that owns its digest, size, role, and source path. The
materializer reopens each strict three-entry source bundle, revalidates every
retained file, and copies only those bytes; it performs no network request, URL
reconstruction, or mutable-mirror lookup. Missing, extra, symlinked,
wrong-sized, or digest-mismatched source metadata fails before export
publication.

`NativeOracleInputSetV1` schema 1 binds the complete profile revisions, ordered
source manifests, and digest-sorted deduplicated object inventory. Its atomic
directory contains canonical `manifest.json` and exact digest-named files under
`objects/`. The independent reopener rejects unknown or missing entries,
symlinks, noncanonical JSON, size drift, and byte tamper. A final fenced
candidate recheck must match the initial candidate records before success.

This bundle supplies input bytes only. It does not produce or compare
`NativeParityOracleV1` or `NativeResolutionOracleV1`, run conversion, or grant
publication authority.

```text
remi native-oracle-input \
  --db /conary/metadata/conary.db \
  --catalog-dir /conary/catalogs \
  --candidate fedora-44=<profile-revision-sha256> \
  --candidate ubuntu-26.04=<profile-revision-sha256> \
  --candidate arch=<profile-revision-sha256> \
  --output-dir /conary/evidence/native-oracle-inputs/<new-export-id>
```

### Native Full-Catalog Parity Artifact

`NativeParityOracleV1` is the sole accepted contract for independent native
package-fact parity against one exact `ProfileRevisionV2`. Its strict manifest
binds the profile revision and logical digests, ordered source members and
precedence, pinned RPM, Debian, or ALPM implementation and version, projection
schema, normalized counts, and the exact SHA-256 and size of a canonical JSONL
package artifact. Each row carries the exact package variant, contributing
member, source snapshot, payload checksum/size/download authority, providers,
and grouped positive and negative relations. Conflict, break, replacement, and
obsolescence declarations remain typed group authority.

The writer and independent reopener reject unknown fields, unsupported schema,
noncanonical bytes, duplicate or reordered keys, count drift, extra bundle
entries, symlinks, and artifact tamper. Comparison merge-walks the verified
oracle and immutable catalog by exact package key, retains one candidate/oracle
package pair at a time, and reports typed candidate-only, oracle-only, identity,
precedence, payload, provider, grouped-requirement, or negative-relation drift.
The native evidence producer must be the pinned implementation named by the
manifest; constructing oracle rows from Conary's own catalog is test support,
not release evidence.

ALPM and RPM have implemented native package-fact producers. The explicit
`native-alpm-oracle` feature links the exact pinned Rust bindings to libalpm;
ordinary Conary and Remi builds remain free of that host-library requirement.
For each ordered profile member, the producer separately verifies the bound
`SourceSnapshotV1` manifest and its exact authenticated `ArchDatabase` object,
then reads package and relation facts through libalpm. It uses a bounded private
spool for profile precedence and exact-identity conflict handling, writes the
canonical bundle, and reopens the complete result before success. It consumes
neither the Conary catalog nor Conary's Arch repository parser.

The explicit `native-rpm-oracle` feature links a narrow private C shim to exact
libsolv 0.7.36. It separately rehashes each ordered member's compressed primary
and filelists objects before libsolv reopens them, then projects every package
variant, payload fact, declared and file provider, required/prerequisite group,
weak RPM relation, conflict, and obsolete. Typed libsolv rich-relation trees
must agree with the canonical RPM grammar. Profile precedence applies only to
fact-identical duplicate identities; contradictory duplicates fail. The
producer uses the shared bounded spool, canonical writer, and independent
complete bundle reopener and reads neither the Conary catalog nor the Fedora
parser's projected packages.

The explicit `native-debian-oracle` feature links a narrow private C++ shim to
exact apt-pkg 3.2.0 in the pinned Ubuntu 26.04 image. For each ordered member it
requires exactly one authenticated `DebianPackages` object, rehashes the exact
compressed bytes, and reopens every deb822 stanza and dependency expression
through apt-pkg. It projects package variants and `Multi-Arch`, payload
authority, declared providers, grouped alternatives and architecture
qualifiers, required and weak relations, conflicts, breaks, and replacements.
Repeated authority fields, malformed or unsupported relations, missing
payload facts, and contradictory exact identities fail the complete input.
apt-pkg's process-global state is serialized for the native handle lifetime.
The producer invokes no apt/dpkg executable or database and reads neither the
Conary catalog nor the Conary Debian parser. It uses the shared bounded spool,
canonical writer, and independent complete bundle reopener.

The hosted `phase4-native-pm-parity` jobs remain deterministic one-package
lifecycle and CLI release tests. They do not satisfy this complete-candidate
contract.

`NativeResolutionOracleV1` is the separate resolver-owned contract for native
solver closure and unresolved-dependency evidence. It binds the exact profile
and package-oracle manifest, pinned solver implementation/version, target
architecture, and one fixed typed policy: empty installed state, every exact
package as a root, required/pre-required groups only, and native
provider/repository precedence. Every package root has exactly one canonical
resolved closure or typed unresolved set. Independent reopen uses a private
disk-backed membership index for referenced package and required-group
authority plus a bounded merge walk for complete root coverage. Comparison
retains one native/candidate root pair and reports typed root, outcome, closure,
or unresolved-set drift.

ALPM, RPM, and Debian have implemented native solver producers. Each independently
reopens and freshly reproduces the bound package oracle from the exact
authenticated native metadata before solving every package as an exact root
against empty installed state. The ALPM producer records prepared libalpm
transaction packages and typed missing-dependency records. The RPM producer
uses exact libsolv transaction and problem-rule IDs, applies profile precedence
as native repository priority, excludes weak relations, and reopens complete
filelists for typed file-provider resolution. The Debian producer uses private
volatile apt-pkg source indexes and empty installed state, projects profile
order into candidate and provider priority, records exact native transactions,
and retains only required or pre-required groups that have no native target as
typed unresolved evidence. All three bind closures and missing groups back to
exact package-oracle authority, reject architecture/conflict/identity or input
drift, write the canonical resolution bundle, and fully reopen it before
success. Conary separately replays the exact verified profile catalog into a
private temporary resolver projection, resolves every exact package key
against empty installed state for the oracle's target architecture, maps
successful closures and typed missing groups back to package-oracle authority,
writes and independently reopens its complete candidate bundle, and requires
exact native comparison. Exact root constraints cannot substitute a different
version, release, architecture, repository, or variant; optional and build
groups remain excluded from the positive solve. Initial conversion crawling,
exact proof reuse, independent CCS reopen, and target preflight retain their
own evidence contracts. The promotion-evidence producer below binds all of
these independently reopened records to the same exact candidates.

### Initial Full-Universe Conversion Crawl

`remi conversion-crawl` is the strict initial-crawl owner. It requires one
exact `PROFILE=SHA256` candidate binding for every typed public profile in
canonical Fedora, Ubuntu, and Arch order. It independently reopens and pins
those durable registered revisions without consulting the active pointers,
then enumerates every exact catalog variant in canonical order. Missing,
repeated, reordered, unknown, uppercase-digest, and candidate-tier Solus
bindings fail closed. The command deliberately has no package-count,
popularity, regex, allowlist, dry-run, skip, exclusion, or per-profile scope
control.

The shipped operator loads the deployed `RemiConfig` and takes the same
exclusive runtime-root lock as the service for the complete crawl. A live
server therefore fails before output or conversion mutation, and its refresh
scheduler cannot supersede the selected candidates mid-ceremony. Database,
catalog, chunk, cache, and repository-key paths come only from that config;
callers cannot compose a second storage authority from raw path flags.

When the deployed config enables R2, the operator initializes and probes that
exact store before attempting a package, attaches it to the conversion
service, and retains the configured bounded local-cache owner. Every newly
produced CCS transport reaches R2 before its conversion row and reusable proof
can commit. A failed durable write is a failed package outcome and prevents a
complete crawl. Local-only durability remains available only when the deployed
config explicitly disables R2.

Each catalog record is selected by its exact package-key SHA-256 rather than
the request-facing name/version/architecture tuple. This preserves distinct
release and origin variants and rejects package-key rebinding. Every record is
passed through the pinned conversion path unless an artifact-level proof has
the exact current reuse key. `ConversionProofKeyV1` binds the source profile,
package key and exact package identity, source-artifact SHA-256, converter
schema and package version, CCS schema, targets signing-key SHA-256, and the
exact ordered supported target identities and contract SHA-256 values. A key
miss validates the artifact afresh. An exact hit hashes the stored CCS,
revalidates the canonical proof, transport and foreign-conversion boundary,
then creates the later revision's own converted row and durable profile pin.
It does not download or convert the unchanged artifact again.

After persistence, the crawl independently reopens each exact `.ccs` path
under the source profile's targets trust anchor. The second verification reads
the persisted bytes, rechecks the complete signed authority and every payload
object, reproduces the transport envelope, and binds format, signer, catalog
identity, foreign-conversion boundary, source digest, and CCS digest back to
the conversion result. Producer-time verification cannot substitute for this
post-persistence proof.

The command writes `RemiConversionCrawlV4`, a strict schema-4 JSON artifact
binding the complete ordered public-profile set, each pinned profile revision,
expected package counts, exact package identities, repository checksums,
terminal states, typed failure evidence, and one exact `ConversionProofV1`
with a `validated` or `reused` disposition for every success. The proof binds
its key, CCS SHA-256, `CcsArtifactReopenProofV1`, validation-origin profile
revision, and complete ordered `CcsTargetCompatibilityProofV1` set. A reused
disposition is valid only when that validation-origin revision differs from
the report's current revision; flipping a current validation to invented reuse
fails report reopen. Missing, repeated, reordered, contract-drifted,
unattempted, corrupt, or failed outcomes prevent success. The proof ledger and
per-revision bindings are one schema-53 database authority and publish
atomically. The writer syncs an atomic staged report, reopens the published
bytes, rejects noncanonical or unknown input, and compares the complete
reopened value before the command may report success. A structurally valid
failure report is still published for diagnosis, then the command exits
unsuccessfully.

The crawl validates registered immutable candidates; it does not activate
them. The promotion-evidence contract consumes its complete report. Atomic
promotion performs the later catalog, proof-bound CCS, CAS, and signed-metadata
durability reopen before any pointer can move.

```text
remi conversion-crawl \
  --config /etc/conary/remi.toml \
  --candidate fedora-44=<profile-revision-sha256> \
  --candidate ubuntu-26.04=<profile-revision-sha256> \
  --candidate arch=<profile-revision-sha256> \
  --output /conary/evidence/initial-conversion-crawl.json
```

### Exact Candidate Promotion Evidence

`RemiPromotionEvidenceV1` is the single promotion-proof authority for one
exact ordered public candidate set. Its producer accepts exactly the declared
Fedora 44, Ubuntu 26.04, and Arch `ProfileRevisionV2` values in public-contract
order; missing, repeated, reordered, foreign, or candidate-tier profiles fail
before evidence generation. Solus cannot enter this record before an explicit
support-tier promotion changes the public contract.

For each profile, the producer independently reopens the immutable catalog,
the pinned native package-fact oracle, and both the native and Conary
resolution bundles. It recomputes package-fact and resolution comparison
records rather than accepting caller-supplied comparison claims. It also
canonically reopens the complete `RemiConversionCrawlV4` artifact and walks its
ordered outcomes against the candidate catalog, requiring exact revision,
count, package key, name, version, release, architecture, and repository
checksum equality. The crawl's own strict reopen proves every package
succeeded with a current exact proof key, independently reopened CCS artifact,
and complete supported-target preflight set.

Canonical-map validation runs against these same reopened candidate catalogs,
so an implementation name cannot be justified by another revision or a
serving cache. The resulting canonical schema-1 artifact binds the complete
crawl digest, canonical-map digest/revision/count, every profile revision and
catalog digest/size, the package-oracle manifest digest, and both resolution
manifest digests through the recomputed comparisons. The writer stages and
synchronizes canonical bytes, atomically publishes them, synchronizes the
parent directory, independently reopens the plain file, and requires exact
value equality before success. This proof does not advance any active pointer;
activation must additionally prove that every referenced catalog, CAS, and
signed metadata object is durable and successfully reopened.

### Promotion Proof Operator

`remi promotion-prove` is the stopped-runtime operator boundary that makes the
library evidence owners usable without ad hoc glue. It takes the normal
exclusive runtime-root lock and requires exact ordered Fedora 44, Ubuntu 26.04,
and Arch bindings for the candidate revision, package-oracle directory,
native-resolution directory, and explicit native target architecture. The
independent native package-manager oracle producers remain separate tools;
this command neither invokes a native package manager nor invents native
facts.

For every selected revision, the command reopens the registered durable
profile catalog without consulting an active pointer, produces the complete
Conary empty-state candidate-resolution bundle, and independently reopens and
compares it with the supplied native resolution oracle. It then loads the
canonical map from the same stopped runtime database and produces the final
`RemiPromotionEvidenceV1` against the exact complete conversion crawl.

All candidate-resolution directories and `promotion.json` are written below
one mode-private staged directory on the destination filesystem. The command
synchronizes and reopens every output, atomically renames that directory to the
requested new path, synchronizes its parent, and reopens the published result
before reporting the evidence digest. It does not advance profile or universe
pointers. A later `promotion-activate` invocation independently reopens the
proof again, so a failed or externally corrupted operator output cannot become
publication authority.

```text
remi promotion-prove \
  --config /etc/conary/remi.toml \
  --candidate fedora-44=<revision> \
  --candidate ubuntu-26.04=<revision> \
  --candidate arch=<revision> \
  --package-oracle fedora-44=<directory> \
  --package-oracle ubuntu-26.04=<directory> \
  --package-oracle arch=<directory> \
  --native-resolution fedora-44=<directory> \
  --native-resolution ubuntu-26.04=<directory> \
  --native-resolution arch=<directory> \
  --architecture fedora-44=x86_64 \
  --architecture ubuntu-26.04=amd64 \
  --architecture arch=x86_64 \
  --conversion-crawl <crawl.json> \
  --output-dir <new-private-evidence-directory>
```

### Atomic Promotion Activation

`remi promotion-activate --config <remi.toml> --promotion-evidence
<promotion.json> --conversion-crawl <crawl.json>` is the sole public-set
activation entry point. It takes the normal exclusive runtime-root lock, opens
the canonical evidence and crawl as plain canonical files, requires their
digests and ordered public profiles to agree, and resolves each profile as
either the exact current fenced private candidate or the exact already-active
revision. A successor candidate fences stale evidence; candidate-tier profiles
cannot enter the evidence contract.

Every selected profile and source catalog is independently reopened from its
registered immutable bundle. Each successful crawl row must resolve one exact
conversion row, revision pin, reusable-proof ledger row, and canonical stored
CCS transport; the CCS bytes and foreign-conversion identity are revalidated.
The union of transport objects is kept in a private disk-backed spool. Every
object is fetched again from configured R2, or from the local CAS when R2 is
absent, and must match its exact size and SHA-256.

Only after those checks does promotion construct the next signed universe,
durably publish all six metadata and content files, and independently reopen
the complete bundle. Its final immediate SQLite transaction rechecks the base
universe, canonical map, current candidate fences, exact ordered run members,
registered catalog identities, and resulting complete active-profile set. It
then publishes all changed runs and advances all profile and universe pointers
together. Database rollback preserves every prior pointer and leaves all
candidate runs private if any final check fails.

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

The same inspection reports each configured public profile's exact latest
refresh run by fencing epoch. Run state, timestamps, typed failure
stage/category, and source-member progress come from the durable refresh
coordinator tables. Free-form failure evidence remains bound by its SHA-256;
only a bounded diagnostic copy processed through the diagnostics redaction
authority is serialized. This state explains an absent private candidate but
cannot substitute for the exact reopened candidate and repository bindings
required by deployment completion.

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

Every public package-read route first loads one immutable snapshot of
`remi_active_universe_revision`, validates its stored manifest and member rows,
and resolves the route slug through the exact profile revision named by that
signed universe. Operational active-profile pointers may already contain the
next private candidate; they are not public read authority. Detail, sparse
resolution, statistics, package lookup, download, and on-demand conversion all
retain the universe-selected revision for the complete request or background
job. A stale conversion row is reconverted; malformed or unpinned current state
fails closed. Conversion has no operator promotion state or alternate serving
lane.

An absent universe, a profile absent from that universe, an unreadable
authority, or an unbound search projection returns HTTP 503 with
`code=PUBLIC_UNIVERSE_UNAVAILABLE`, a stable typed `reason`, `Retry-After`, and
`Cache-Control: no-store`. Successful projections expose
`X-Conary-Universe-Revision` and `X-Conary-Universe-Sequence`; profile-scoped
responses also expose `X-Conary-Profile-Revision`. Deployment and promotion
proofs can therefore assert that detail, sparse resolution, search, and
statistics agree without inferring identity from response contents.

The on-disk Tantivy index has no authority merely because it can be opened.
Startup rebuilds it only from the exact profiles in the active signed universe
and binds the committed projection to that universe identity in memory. Search
returns typed unavailability before the rebuild completes or whenever
activation advances beyond the bound index. Candidate-tier catalogs and native
publications outside the selected universe are never indexed. Federated sparse
entries are merged only when the peer response proves the same exact profile
revision; an absent or different binding contributes no data.

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
  content-addressed registration, and fenced candidate inputs.
- `catalog_capacity.rs`: shared filesystem-scoped metadata, profile-candidate
  growth, catalog finalization, and projection-copy reservations with typed
  capacity refusal.
- `catalog_authority.rs` and `profile_catalog.rs`: exact-revision resolution,
  verified immutable readers, reader-lifetime pins, and catalog projections.
- `public_universe.rs` and `handlers/public_read.rs`: signed-universe snapshot
  selection, typed public unavailability, and exact response identity headers.
- `catalog_gc.rs`: exact active/current-candidate/work/reader/conversion
  reachability and bundle deletion after operational intent is durable.
- `handlers/detail.rs`, `handlers/detail/catalog.rs`, and
  `handlers/detail/tests.rs`: detail and analytics response assembly, with one
  universe-selected exact profile pin per response and catalog-owned
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
- `conversion_crawl.rs`: bounded full-profile orchestration and canonical
  report publication; report types and validation live in
  `conversion_crawl/report.rs`.
- `conversion_crawl/operator.rs`: stopped-runtime config, durable-store probe,
  and bounded-cache wiring for the shipped full-universe crawl command.
- `conversion_crawl/proof_reuse.rs`: exact proof-key construction, durable
  artifact-level proof ledger, per-revision bindings, changed-artifact
  validation, and cross-revision reuse.
- `conversion_crawl/ccs_reopen.rs` and
  `conversion_crawl/target_preflight.rs`: independent persisted CCS reopen and
  exact ordered supported-target compatibility proof.
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
