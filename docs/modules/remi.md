---
last_updated: 2026-09-04
revision: 167
summary: Describe Remi repository ingestion, immutable catalog publication, conversion and benchmark boundaries, typed conflicting-closure parity handoff, deployment, readiness, storage, and operator-facing serving contracts.
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

Remi routes every `{distro}` path parameter through repository-feed profile
validation before DB queries, cache/key filesystem paths, or release-upload
trust gates. Public Remi route slugs remain `fedora`, `ubuntu`, and `arch`;
they are backed by profile route metadata rather than a local hard-coded
`SUPPORTED_DISTROS` list.

## Package Source Authority

Source-format identity, relations, payload, configuration, lifecycle, and
consumer projections are owned by
[the source-package authority specification](../specs/source-package-authority.md) and are
not repeated here. This section starts at Remi's authenticated repository
ingestion boundary.

### Source Manifests And Trust

Hosted package sources are declared in `deploy/remi-repositories.toml` and
loaded by `apps/remi/src/server/repository_manifest.rs`. Every source names one
known profile and carries parser construction data: RPM declares
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
catalog independently declares the target machine architecture,
repository identities, roles, and precedence required for each complete
profile. Its closed `ProfileTargetArchitecture` authority declares Fedora 44
`x86_64`, Ubuntu 26.04 `amd64`, and Arch `x86_64`. The hosted manifest must match
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

### Source And Profile Catalogs

Native refresh writes immutable candidate package authority outside the
operational database.
Each authenticated source is projected into a strict standalone SQLite catalog
and a canonical `SourceSnapshotV1` manifest under
`<storage.root>/catalogs/sources/<manifest-sha256>/`. The manifest binds exact
source, repository, stream, parser-projection, authenticated root and child
objects, catalog bytes, logical digest, and row counts. A strict
profile-revision schema 3 `ProfileRevisionV2` then binds that typed target
architecture, each member's role, precedence, required state, ordered source
identity, and one composed
profile catalog under `catalogs/profiles/<manifest-sha256>/`. Core contract and
bundle verification live in `crates/conary-core/src/repository/catalog/`.
Profile catalog projection version 3 is the current hard cut. Upgrade-tolerant
inspection classifies a stored schema-2 profile revision as
`ObsoleteSchema { found: 2, required: 3 }`; refresh records that non-reuse
decision and composes a schema-3 replacement. Deployment and readiness report
the obsolete active or candidate revision as unpopulated until that refresh
finishes. Serving, catalog comparison, and promotion evidence remain strict:
schema-2 profile revisions and projection-version-2 profile catalogs never
become readable authority, and no compatibility deserializer or migration
adapter remains. Promotion activation may classify a canonical, digest-bound
active universe containing an embedded schema-2 revision as
`ObsoleteProfileSchema`. That typed state contributes only its immutable
manifest identity and sequence to the activation fence; it is never serving,
comparison, or replay authority. A proved schema-3 candidate may supersede it
at the next sequence, while the obsolete universe row remains as fencing
history.
`apps/remi/src/server/catalog_authority/revision_inspection.rs` owns that
upgrade/rebuild classification and strict manifest deserialization;
`apps/remi/src/server/admin_service/profile_refresh/source_reuse.rs` owns the
corresponding reuse decision.
Every serving projection resolves a package origin back to that manifest.
When two members publish the same source-independent native package identity,
profile composition retains the member with the higher declared
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

### Sink Projection And Capacity Admission

`RepositorySnapshotSink` projection schema 2 is the only native parser output
contract. Each parser gives the sink the run-local file that supplied
every authenticated metadata-object fact; the immutable sink transfers those
verified bytes into the source candidate before parser work-file cleanup.
Fedora primary/filelists XML, Debian Packages stanzas, ALPM archive records,
and eopkg Package XML are authenticated into private files and decoded one
record at a time. On the capacity-preflight path, the immutable sink writes
those normalized records into one length-framed, SHA-256-bound private spool;
every positive chunk is admitted before its write. After the complete catalog
growth bound is reserved, Fedora, Debian, and eopkg replay those records without
decoding the native metadata again. ALPM replays its exact desc/depends
fragments without reopening or walking the archive, then performs its one
normalization pass from the paired fragments. Fedora pkgid joins and ALPM
desc/depends pairing use private indexed SQLite state. Candidate logical
hashing, count validation, reopen verification,
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

### Catalog Proof, Reuse, And Retention

Candidate finalization calculates the deterministic logical digest and then
performs one complete independent candidate reopen. That full reopen mints an opaque,
non-serializable process-local proof bound to the scope, byte SHA-256 and
size, logical digest, and relation counts. Private staging carries that proof
through source manifesting, projection-cache publication, profile composition,
and profile manifesting instead of reconstructing and re-digesting the same
rows a third or fourth time. Manifesting requires that same reader to own the
canonical private candidate path and binding, then hands the same linear reader
to publication. Publication rechecks bounded directory and manifest structure,
drops the candidate reader, atomically renames the same directory, and performs
one complete independent destination reopen. An already-existing matching
destination is independently reopened instead. The destination proof checks
file type and sidecars, byte hash and size, SQLite application/schema identity
and integrity, the complete stored binding, manifest evidence, and
directory membership. The exact-byte proof
carries the already-completed canonical logical replay, computed table
cardinalities, and explicit missing-package/group rejection, so neither the
Rust row reconstruction, table-count scans, nor SQLite's foreign-key relation
scan is repeated for unchanged bytes.

Native projection cache manifest schema 3 and key schema 2 also make that
logical proof durable without making cache bytes an independent package
authority. Publication requires the process-local full-replay proof and writes
a canonical attestation bound to the exact catalog binding. A later hit first
re-derives the complete projection identity from the stream, parser-projection,
and catalog schema identities plus every authenticated child
role/path/digest/size and typed root-derived parser bound. The top-level
authenticated root is verified before lookup and remains authority in the
new source manifest, but its signature, timestamp, and other wrapper-only bytes
do not invalidate an unchanged normalized projection. It then checks the
canonical manifest and attestation,
regular-file and sidecar policy, byte SHA-256 and size, SQLite
application/schema identity and integrity, the complete stored binding, and
source evidence. Only after those checks does it mint a new process-local
proof. The catalog SHA-256 binds the publisher's canonical logical proof,
computed table cardinalities, and orphan rejection, so cache lookup does not
replay any complete relation pass.
The schema hard cut makes older cache entries misses; separately loaded bundles
and signed artifacts retain their own verification authorities.

Before projection-cache materialization, refresh also resolves the latest
successful private-candidate and active profile manifests through the durable
resource registry. A planned member may offer its registered
`SourceSnapshotV1` and canonical bundle path as a reuse candidate. That offer
does not establish freshness: the native parser independently authenticates
the current top-level root and every projection-affecting child, rebuilds the
complete source state, and accepts reuse only when it is byte-for-byte the
registered manifest. An exact match performs one registered durable
bundle reopen, creates and reserves no source-candidate SQLite file, and
carries that reader directly through profile composition and the already-
published source boundary. The registered reopen checks file type,
directory membership, compact portable-manifest identity and size, SQLite
application identity, embedded binding, and retained metadata. Its read-only
VFS authenticates every demanded fixed-size chunk before SQLite receives it,
so unchanged bytes inherit the publication-time structural and logical proof
without another complete catalog or normalized-relation scan. Changed authenticated identity takes the
ordinary projection-cache or parser path. Missing, malformed, noncanonical, or
tampered registered authority fails closed instead of becoming a cache miss.

Operational resource identity is the canonical source- or profile-manifest
SHA-256. The catalog artifact SHA-256 is a separately verified byte identity,
not a uniqueness key: a newly authenticated upstream root may produce a new
immutable source manifest while its exact authenticated children and normalized
catalog projection remain byte-identical. Schema 55 retains the ability for both resources to
bind those same bytes. Registration still compares complete immutable metadata
for a resource replay, and profile membership, reader pins, reachability,
GC deletion intents, and bundle paths remain keyed by the manifest resource
SHA-256. Changing provenance is therefore recorded instead of discarded, while
projection reuse cannot fail merely because the resulting catalog bytes already
exist under another manifest.

Schema 55 also binds every registered resource row to one immutable,
filesystem-independent physical attestation for the catalog artifact
named by that manifest. It records the lowercase SHA-256 and size of a
canonical `catalog.sqlite.chunks-v1` portable chunk manifest, the fixed
65,536-byte v1 chunk size, and
the chunk count implied by the artifact size. The manifest header binds
those facts back to the ordinary catalog SHA-256, and each domain-separated
chunk digest binds its position and actual length. Missing, malformed,
mis-sized, stale, or artifact-mismatched attestations fail before
registration. Publication derives the proof from the retained candidate
descriptor, durably places it beside the catalog before the bundle rename,
then independently reopens the destination through the authenticated VFS.
Reusing an already-published destination requires the same proof and
attestation; publication never repairs or replaces an existing bundle. There
is no filesystem-selected proof kind and no durable full-scan fallback. This
attestation remains local operational state and does not alter or compete with
the source/profile manifest.

After every source is authenticated and staged, refresh derives the complete
ordered `ProfileSourceMemberV2` contract from those verified source manifests
without visiting package rows. It inspects the latest successful private
candidate and active revision manifests in that order. Profile,
profile-projection version, ordinal, role, precedence, required state, source,
repository, stream, and source-snapshot identity equality makes an existing
immutable profile eligible for reuse. The selected V2 bundle is resolved
through its durable registry entry and content-addressed canonical
manifest, then independently reopened with the same physical, SQLite,
binding, and member-evidence checks described above. The registered reopen
authenticates demanded chunks and performs no complete SQLite integrity scan;
V2 publication
already required the complete logical replay, exact table cardinalities, and
explicit orphan rejection, so this registered reopen carries that durable
exact-byte attestation instead of counting, deserializing, re-digesting, or
foreign-key-scanning every row after each service restart. Externally supplied
or unregistered bundles do not receive this authority. The reader pin remains
live until the new fenced run completes as a durable candidate. Every new
request that reuses a process-cached registered reader reauthenticates the
current canonical manifest, registered top-level layout, and portable
proof, then proves that the open catalog descriptor still names the registered
path inode before handing out the reader. Source reauthentication also proves
that retained native metadata remains a real private directory boundary; it
does not rescan metadata payloads. A reader already handed out retains its
authenticated catalog descriptor and decoded proof, so a later bundle mutation
fails new handoffs without invalidating the immutable bytes already pinned by
an in-flight reader. This path
creates no profile candidate file and performs no profile-catalog
reconstruction. Any
changed member or projection version takes the normal private composition path,
and a malformed registered selection fails instead of becoming reuse
authority.

Fedora metadata acquisition is owned by
`crates/conary-core/src/repository/parsers/fedora/metadata.rs`; the parent
parser owns normalized RPM record replay and joins. Source snapshot manifests
remain `SourceSnapshotV1`, but parser projection version 2 and the retained
metadata directory are a hard cut: projection-version-1 manifests and legacy
two-entry source bundles are rejected and must be rebuilt by refresh. There is
no compatibility reader, upstream refetch fallback, or parallel source
authority.

Normalized source projections may be reused from
`<storage.root>/cache/native-projections/<key-sha256>/`. Cache key schema 2
binds the exact stream-binding SHA-256, ordered child role/path/digest/size set,
any authenticated decoded-length bound consumed by the parser, parser
projection version, catalog schema, and verified catalog binding. Every parser
supplies at least one authenticated child. The independently verified
top-level root is deliberately not a projection-byte identity: wrapper-only
root churn can produce a new source manifest over unchanged catalog
bytes. A changed child or root-derived parser bound misses and reparses. A
tampered, mixed, or noncanonical entry is removed from this cache
namespace and cannot become package authority. Cache candidates are private,
synchronized, and atomically renamed; a cache fault fails the private refresh
and leaves the active profile pointer unchanged.

One profile stages at most four native-source pipelines concurrently. Each
complete mixed pipeline runs on an independent bounded blocking worker with a
private current-thread Tokio runtime. Authenticated network waits use that
worker's I/O driver, while synchronous decode, normalization, SQLite writing,
hashing, cache materialization, and source-manifest proof stay off server
runtime workers and may use separate CPU cores. This is scheduling only; it
does not split or reorder one source's authority writer. A first failure stops
launching queued sources and the scheduler drains every already-started
blocking worker before candidate cleanup can begin. Source-completion logs
record per-source elapsed time, and the profile summary records the observed
peak worker count, configured bound, source count, outcome, and total elapsed
time so effective concurrency remains production-visible.

Construction is private beneath `catalog-candidates/<run-id>/`. Candidate
SQLite integrity, schema, ordering, counts, logical digest, and source
membership are reopened and checked before durable registration. Every source
candidate has exactly `catalog.sqlite`, `manifest.json`, and a private
`native-metadata/` directory containing only digest-named objects declared by
the manifest; profile candidates retain their two-file layout.
Publication derives and synchronizes `catalog.sqlite.chunks-v1` before the
atomic rename, so registered source bundles have exactly four entries and
registered profile bundles have three. The catalog, manifest, portable proof,
retained metadata objects, and their directories are synchronized before the
content-addressed bundle becomes durable. Only then does one short operational-database
transaction prove the current run owner and fencing epoch, register the
source/profile resources, and complete the run as a terminal `candidate`.
Refresh never advances a profile or universe pointer and never updates
`last_published_at`; checked, changed, and validated timestamps describe only
the private candidate work. A failed required member, stale fence, malformed
bundle, or durable-registration fault leaves both the previous active pointer
and the previous public universe readable.
Long parser, profile-composition, and immutable publication-verification calls
renew that fenced lease from an independent coordinator thread at the
core-owned heartbeat cadence, including while the run is `ready_to_publish`.
Candidate completion ends the lease; the current candidate survives
restart and lease expiry as typed promotion input. A successor refresh advances
the fencing scope and fences the previous writer, but an in-flight, failed, or
abandoned successor does not erase the latest successful immutable candidate.
Only a newer completed candidate or publication supersedes that proof. The
same rule owns current-candidate lookup, promotion eligibility, and catalog-GC
roots, so retained work remains activatable rather than merely undeleted. Thus
a CPU-bound metadata record stream cannot starve its own ownership proof.

Operational SQLite owns refresh runs and leases, resource metadata, ordered
profile members, the current private candidate, the active pointer, and
revision pins. It does not own
package, provide, or requirement rows for the activated native Remi catalog.
`crates/conary-core/src/repository/sync/remi/run/candidate.rs` owns the durable
candidate transition, exact run-to-revision member proof, and current-candidate
lookup; the parent run module retains lease, heartbeat, and failure fencing.
`CatalogAuthority` resolves the pointer, registered resource, and persisted
portable attestation, records a reader pin for the handle lifetime, validates
the three-file bundle and compact proof, and opens the catalog through
the read-only authenticated SQLite VFS. The registered reopen checks the
SQLite header and stored binding through authenticated reads; it performs
no complete userspace catalog hash, SQLite integrity scan, or logical replay.
Universe publication and explicit promotion validation retain their complete
candidate/destination proofs, but those full-scan readers never seed serving
state. Later opens in one authority process may share a reader only when the
profile revision and complete physical attestation agree; a mismatch
fails instead of replacing authority silently.

Each source snapshot referenced below a pinned profile is resolved again
through the durable resource registry. Its serving cache likewise requires the
snapshot digest, source profile, complete manifest, bundle path, and
physical attestation. The VFS authenticates every covering chunk before its
bytes reach SQLite and owns the verified bytes in its bounded cache, so a
mutation after open cannot turn a cached trusted bit into authority for changed
backing storage. Unregistered source candidates continue through complete
artifact, integrity, binding, and logical verification and cannot consume a
registered portable attestation.
Readers opened before promotion therefore finish on the old revision; later
readers see the complete new revision. Conversion outcomes own durable
revision pins. Catalog garbage collection computes reachability from active,
latest-successful-candidate, reader, work, and conversion pins and removes only resources
absent from that graph. A superseded candidate is collectable unless a
different typed pin retains it; age, repository names, process liveness, and
guessed retention windows are not collection authority. An absent registered bundle or
never-published profile namespace is idempotent absence during collection; a
symlink or non-directory at either boundary still fails closed.
After the schema-55 hard cut, only cleanup of an unregistered terminal
run candidate may also recognize the retired schema-54 two-file profile layout
and its source layout with `native-metadata/`. Registered schema-55
deletion intents remain current-layout-only. Extra entries, malformed proof
geometry, manifest/digest mismatches, symlinks, invalid native-metadata names,
and non-regular metadata objects still fail closed. Retired layouts never
become serving, reuse, readiness, inspection, or publication authority; all of
those paths require the current portable proof sidecar and its persisted
physical attestation.
Concurrent profile refreshes share one narrow catalog-collection coordinator,
so their plan, filesystem removal, and acknowledgement phases cannot consume
the same deletion intent while source retrieval, parsing, and catalog
construction remain parallel.
Before a profile candidate file exists, core independently validates every
reopened source reader against its snapshot manifest and derives one
canonical `CatalogProfileCandidateScratchV1` from the ordered member facts.
The requirement allocates the input catalog bytes once for destination
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
writer commits its private logical state and derives the current database
bytes from SQLite's positive `page_size` and `page_count`. Finalization writes
one compacted `VACUUM INTO` output to a private same-directory sibling. Remi
reserves one database-sized output against a process-local ledger keyed by the
owning filesystem device, re-reading available space for every admission. It
does not run in-place `VACUUM`, create a database-sized rollback journal, or
copy the compacted pages back over the unpublished input. After output sync,
the writer closes the input, atomically replaces it with the compacted file,
synchronizes the directory, and subjects that path to the normal complete
independent reopen. Structured logs record logical-digest, compaction, artifact
hash, independent-reopen, and total finalization times with row and byte
facts. Concurrent finalizers cannot collectively reserve more than the
filesystem reports available; the lease releases on success, error,
cancellation unwind, or process restart. A one-byte-short refusal is a typed
`storage_capacity` refresh failure before compaction, and candidate cleanup
preserves the active revision.
When a newly finalized source catalog has no matching projection-cache entry, the
cache derives another typed requirement from the verified catalog artifact size
and the canonical cache-manifest bytes. It reserves those bytes on the
cache filesystem through the same ledger before creating a private stage, then
retains the lease through copy, file and directory synchronization, atomic
rename, and independent reopen. A current cache hit writes and reserves
nothing. Refusal is typed and leaves no cache candidate. Immutable source and
profile publication itself moves verified candidates by same-filesystem atomic
rename, so it creates no second catalog-file copy.
Before Fedora or Debian child metadata is downloaded, the authenticated root
supplies each selected object's compressed length: signed `repomd.xml`
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
positive path requirement against the retained primary projection held by
the repository sink. The immutable sink walks its private catalog transaction
one typed requirement at a time; the compatibility sink checks its collected
projection. No second SQLite audit database or other parser work file is
created for those duplicated facts.
Arch repository databases may publish each package's desc and depends records
out of order. Before candidate creation, its read-only pass accounts for each
raw fragment, each desc projection, and every separately published
depends relation group without assuming archive order. After growth admission,
the common normalized spool replays those fragments into a strict transient
table inside the private catalog candidate transaction, rejects duplicate and
orphan fragments, replays complete pairs in source-directory order, and drops
the table before finalization. The compatibility sink performs the same typed
pairing in its existing in-memory state. No Arch-specific SQLite spool or
sidecar is created; transient fragment pages remain in the candidate high-water
mark consumed by finalization admission.
Arch database signatures and eopkg index digest sidecars authenticate
completed bytes but publish no signed size before download. Each parser binds a
typed stream subject to the metadata role and repository-relative source
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
used by candidate construction and includes Fedora filelist provides plus Arch
separately ordered requirement groups. It retains those facts in the common
run-local projection spool so candidate construction does not repeat the native
parser pass; the staged byte count and in-process digest are verified during
replay, and every exit removes the spool.
`CatalogSourceCandidateScratchV1` allocates those canonical projection
bytes once for destination payload and once for
B-tree repacking, the fixed schema roots and one 4096-byte page per package,
and a full candidate-database rollback-journal ceiling. On a projection
cache hit, the independently reopened artifact bytes and its bound package
count replace that parser preflight. Remi reserves the complete sum through the
shared candidate-filesystem ledger before the writer creates SQLite. Fedora
supplemental file capabilities bulk-load with the final capability and raw
query indexes absent; pkgid joins retain their construction-only package
index, and finalization builds each deferred query index once before hashing or
publication. The lease survives replay and committed metadata/evidence; after
file and parent sync the writer proves the actual database remains below the
admitted ceiling, releases the growth lease, and only then requests
page-derived finalization scratch. A
one-byte-short refusal leaves no native candidate file. Profile candidates use
the corresponding ordered-member contract described above.

### Readiness, Deployment, And Refresh Coordination

`apps/remi/src/server/readiness.rs` owns serving-readiness orchestration, while
`apps/remi/src/server/readiness/source_profiles.rs` owns configured-profile
and active-catalog population inspection. `/health` is an unconditional
liveness reply and proves only that the process is listening;
`/health/ready` is the evidence-bearing one. It opens the database read-only,
requires the expected schema revision, and requires usable typed repository and
canonical publication outcomes from the initial scheduler cycle. The validated
manifest supplies the required-profile policy; every required profile
must have a valid durable active pointer, strict canonical manifest,
three-file registered bundle, a regular catalog file with the signed size, an
authenticated portable proof matching persisted authority, and a nonzero
package count. This bounded inspection neither claims the process SQLite writer
nor rehashes the catalog; serving opens retain the authenticated VFS contract
above. A server without a configured profile is not ready. It
also checks the serving directories and configured free-space floor. A probe that cannot run reports
`unavailable` rather than success, so an unmeasurable resource never reads as
ready. A public package cache miss before that profile is populated returns the
typed retryable `REPOSITORY_NOT_READY` 503 response and creates no conversion
job. Deploy verification and `scripts/remi-health.sh` assert `ready == true`
from that endpoint; liveness alone is not deployment evidence. The free-space
floor is `storage.readiness_min_free`, defaulting to 10 GiB.
It is a serving-health threshold, not catalog-construction scratch admission.

`apps/remi/src/deployment.rs` owns recoverable config/schema orchestration and
read-only deployment-state assembly;
`apps/remi/src/deployment/candidate_inspection.rs` owns full and causally
publication-attested private-candidate inspection;
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
the canonical discovery fetch and contract rebuild before eligible
profile prewarm. Before creating storage subdirectories, opening the
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
nonempty candidate for every public profile, fully reopens each strict
immutable manifest and three-file registered bundle, authenticates its portable
proof, hashes the complete catalog, runs SQLite integrity and logical replay,
validates its typed member contract, and re-proves its fenced run members
against the current repository bindings.
The deployment-only
`--accept-candidates-completed-after <unix-seconds>` form is valid only with
that predicate. Candidate completion is already ordered after a complete
private-candidate proof and one independent durable-destination reopen. The
causal form therefore requires the exact candidate and terminal refresh to
have completed strictly after its positive floor, then rechecks the durable
registry, canonical manifest bytes, exact directory entries, regular catalog
file and signed size, member contract, fenced run members, and current
repository bindings without repeating catalog hashing or SQLite integrity
scans. Its typed verification object records `publication_attested`, the exact
floor, elapsed microseconds, and zero reopened/hash/integrity catalog bytes;
the ordinary form records `full_reopen` and the exact catalog work. An
active-only profile, superseded or pre-floor run, candidate-tier profile,
changed binding, missing bundle, empty catalog, or mismatched proof mode cannot
satisfy the causal predicate. The protected
`build-remi-candidate` workflow constructs the release-profile binary once per
protected `main` commit and retains a deterministic bundle plus exact source,
toolchain, compiler-cache, digest, and timing provenance. Candidate deployment
accepts only that commit's successful protected `push` artifact, reopens its
single-file bundle, and fails if locate/download/verification exceeds 60
seconds; the deployment workflow contains no compilation path. The protected
private-candidate deployment workflow then adds causal evidence that the static
predicate intentionally does not own: it records the pre-transition
inspection, starts the exact merged binary, invokes the loopback-only forced
refresh endpoint with the deployment transition completion as its causal floor,
validates the typed generation/completion/coalescing response, retries only exact
failed public profiles through the typed `profile` scope, and requires the
final Fedora, Ubuntu, and Arch fencing epochs to be strictly newer than their
recorded baselines. Each terminal run must finish after the recorded binary
transition and be bounded by a retained refresh generation that names its
profile as successful. The final evidence binds the exact merged commit, built
binary SHA-256, completion mode, and transition timestamp; the before-and-after
sanitized inspections are retained. Before that read, the root helper extracts
the staged binary and verifies its exact version and SHA-256 without opening
the live database. The versioned `deployment baseline` surface then runs from
the existing plain executable when one is installed, so the baseline reader
owns the schema revision currently on disk. A host without an installed binary
falls back to the verified staged binary, which still requires the persisted
database to match its exact current schema. This keeps a hard schema cut
deployable without letting the incoming binary interpret retired rows. The
baseline reads only installed manifest reconciliation, relational
candidate/run-member identity, and latest typed refresh state. Candidate identity is one optional
object, so a revision, run, or completion field cannot be half-present. It
performs no signing, catalog, package, conversion, or universe inspection and
reports wall/CPU/RSS, SQLite statement and logical page-read work, zero catalog
opens/bytes, and output size. The protected workflow rejects a baseline over
two seconds or any nonzero catalog access before mutation. A profile without a
candidate contributes a null identity and its latest refresh fence but never
satisfies the strict post-transition candidate predicate. Evidence schema 3
also records the typed refresh generations plus the outcome, causal failure
phase, and duration of every
completed or failed remote phase. A failed pre-deployment baseline is retained
as `predeployment-candidate-baseline` with its measured duration rather than an
empty or misclassified transport artifact. An early remote-session or transport
failure produces a failure envelope and attempts one read-only recovery inspection
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
the same ordered revision set. Retired mutable Remi package rows are not
deployment evidence, and neither predicate grants publication authority.

`apps/remi/src/server/publication_scheduler.rs` owns startup publication order
and both periodic clocks; `apps/remi/src/server/publication_coordinator.rs`
owns one process-local publication coordinator that also
serializes background cycles, repository-admin mutations, MCP canonical cycles,
and package cache-miss readiness/reservation decisions. Their network, parsing,
and mutation phases therefore cannot invalidate one another's publication
decision. Every all/profile refresh receives a monotonic process-local
generation, scope, producer force policy, timestamps, and terminal batch
or incomplete/error disposition. A deployment force request may provide
`accept_completed_after`; after acquiring the same exclusion lock it consumes
the newest all-profile generation only when that batch completed strictly
after the floor, is complete, and contains zero skipped sources. This closes
the missed-wakeup window without treating a startup no-op, partial result,
failed generation, profile retry, or older process as forced-refresh evidence.
A repository create, update, delete, or single-source sync invalidates the
retained batch before mutating, and a force request without a floor always
executes. A queued coordinator waiter releases its server-state read guard
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
complete crawl, reopens their selected candidates or already-active revisions,
and alone may change the public profile set and signed universe. No partial
candidate set becomes public merely because an admin or background refresh
completed.

Eligible profile-specific prewarm jobs run concurrently under the configured
conversion bound shared with request-driven conversions. Each profile preserves
its own top-N ordering and sequential conversion semantics, while a slow first
profile cannot starve the profiles after it. Multi-source refresh returns one
typed result or failure per source. A source failure remains visible but cannot
discard successful source commits or suppress prewarm for another
profile. Prewarm eligibility comes only from the successful result's persisted
`source_profile`; repository names, URLs, formats, and error text are not
selectors.
All conversions for one unchanged profile revision share the process's already
verified immutable catalog reader; the top-N loop does not repeat whole-catalog
digest and integrity verification for each package.

Both internal and external `POST /v1/admin/refresh` routes use the same response
projection: HTTP 200 means every source completed or was current, 207 carries a
mixed success/failure batch, and 502 means every configured source failed.
Every response includes the producer generation, scope, force policy,
start/finish timestamps, and `coalesced` disposition. The optional positive
Unix `accept_completed_after` parameter is valid only with `force=true` on the
all-profile route; other combinations are HTTP 400.
Omitting `profile` selects every configured source. Supplying one
configured native profile selects only that profile and no legacy repository;
it is the retry boundary after a partial batch and cannot upgrade global
publication readiness from its profile-local result. Global database/setup
failures remain HTTP 500. The release deployment gate
requires source reconciliation, every configured profile populated in
its active immutable catalog, the fresh signed universe matching those
revisions, and at least one validated converted artifact pinned to every
current profile revision. Result and failure arrays are sorted by repository name,
so concurrent completion order is not API order.

`apps/remi/src/server/admin_service/refresh.rs` owns the batch state, typed
per-source results, and bounded concurrent collection.
`apps/remi/src/server/handlers/admin/refresh.rs` owns refresh query validation
and the shared typed HTTP projection. `server/mod.rs` owns the
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
ordered set of public `ProfileRevisionV2` catalogs, the canonical-map
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
`remi_active_universe_revision`. Schema 55 stores the canonical
`RemiPromotionEvidenceV1` and `RemiConversionCrawlV4` digests on that universe
revision. A catalog, proof, CAS, signed-metadata, fence, canonical-map,
transaction, or reopen fault leaves the complete previous public state
selected. Replay returns the already-active revision. The background
publisher cannot create an initial universe or change profile or canonical
authority without promotion evidence; it may only renew signed freshness for
the active authority after revalidating its durable bindings.

Canonical-map schema validity is insufficient publication evidence. Before a
new universe is signed, Remi independently reopens every public profile
catalog and requires each canonical implementation's literal package name to
exist in that profile revision. It repeats the same cross-object proof
after the durable universe bundle is reopened. Presence uses the indexed literal
package name only; provides, aliases, descriptions, case folding, and discovery
caches cannot satisfy it. A missing profile or package is a typed failure, does
not create a replacement universe bundle, and preserves the prior active
pointer. This gate is structurally bounded to one verified reader per public
profile plus scalar implementation facts; it does not materialize a
distribution-sized package-name set.

### Native Oracle Input Materialization

`remi native-oracle-input` is the read-only handoff from production
private candidates to the pinned native package-manager producers. It requires
exact `PROFILE=SHA256` bindings in canonical Fedora 44, Ubuntu 26.04, and Arch
order. Before network or output mutation it reproves each current fenced run,
opens a durable reader pin on every immutable profile, and independently
reopens every ordered source snapshot catalog. Active-only revisions,
candidate-tier Solus, missing or reordered profiles, and noncanonical digests
fail closed.

Refresh retains every parser-authenticated metadata file in the immutable
source bundle that owns its digest, size, role, and source path. The
materializer reopens each strict four-entry registered source bundle,
authenticates its portable proof, revalidates every
retained file, and copies only those bytes; it performs no network request, URL
reconstruction, or mutable-mirror lookup. Missing, extra, symlinked,
wrong-sized, or digest-mismatched source metadata fails before export
publication.

`NativeOracleInputSetV1` schema 1 binds the complete profile revisions, ordered
source manifests, and digest-sorted deduplicated object inventory. Its atomic
directory contains canonical `manifest.json` and digest-named files under
`objects/`. The independent reopener rejects unknown or missing entries,
symlinks, noncanonical JSON, size drift, and byte tamper. A final fenced
candidate recheck must match the initial candidate records before success.

This bundle supplies input bytes only. It does not produce or compare
`NativeParityOracleV1` or `NativeResolutionOracleV1`, run conversion, or grant
publication authority.

Production transport is owned by the protected
`export-remi-native-oracle-inputs` workflow. The workflow accepts only one
successful protected-main private-candidate deployment run, derives all three
revision bindings from that run's typed inspection, calls the fixed root-owned
helper operation, and independently reopens every transported manifest and
object byte before retaining the short-lived handoff artifact. Its
workflow commit must equal freshly fetched protected `main` before any SSH and
again immediately at the SSH boundary. Native-oracle production accepts the
result only when the export run used its own current-main operator
revision. Callers cannot supply paths, profile order, conversion commands, or
publication operations.

The diagnostics survey binding is schema 2, the strict per-ecosystem lane
evidence is schema 5, and the assembled three-lane set is schema 2. The lane
schema embeds resolution schema 3; the assembled set binds those current lane
summaries. Survey and lane evidence require the resolution worker count,
measured per-worker load times and RSS admission inputs. Schema-1 survey
bindings, strict lanes through schema 4, and schema-1 assembled sets are fenced
as obsolete; operators must regenerate all three strict lanes once before
retained-lane subset production can resume.

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

The full-catalog package-fact and resolution contract is owned by
[the native parity oracle specification](../specs/remi-native-parity-oracle.md).
That specification defines the pinned ALPM, libsolv, and apt-pkg producers,
schema hard cuts, bounded surveys, comparison rules, and production transport.
Remi consumes only independently reopened artifacts bound to the selected
private profile candidates; survey output never authorizes promotion.
Resolution schema 3 admits `not_installable { reason: conflicting_closure }`
when conflict, break, same-name, obsoletion, or exact-root displacement makes a
root-reachable closure inconsistent. Conflict-class attribution dominates
missing requirements. The RPM projection is schema 5; Debian, ALPM, Conary
candidate, and comparison projections are schema 3. Every retained resolution,
candidate, and comparison bundle from the superseded schemas must be rebuilt;
the package oracle is unchanged. Native diagnostics surveys are schema 3 and
retain up to 5,000 solver-native conflict outcomes in `diagnostic_outcomes`,
with uncapped total and explicit retention/truncation counts, without
inflating the failure count. Candidate-resolution and resolution-comparison
surveys are schema 2 because they embed the outcome vocabulary. Native and
candidate explanations use a 32 MiB retained-evidence budget; native survey
documents remain capped at 64 MiB end to end.
For ALPM missing-first failures, the producer rechecks the libalpm-selected
root-reachable package set with libalpm's typed conflict API before accepting
an unresolved outcome.
The top-level Remi promotion-evidence envelope is schema 2 because it embeds
resolution-comparison schema 3. Retained schema-1 promotion evidence is
obsolete non-authority and must be rebuilt before activation.

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
configured store before attempting a package, attaches it to the conversion
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

After persistence, the crawl independently reopens each `.ccs` path
under the source profile's targets trust anchor. The second verification reads
the persisted bytes, rechecks the complete signed authority and every payload
object, reproduces the transport envelope, and binds format, signer, catalog
identity, foreign-conversion boundary, source digest, and CCS digest back to
the conversion result. Producer-time verification cannot substitute for this
post-persistence proof.

The command writes `RemiConversionCrawlV4`, a strict schema-4 JSON artifact
binding the complete ordered public-profile set, each pinned profile revision,
expected package counts, package identities, repository checksums,
terminal states, typed failure evidence, and one `ConversionProofV1`
with a `validated` or `reused` disposition for every success. The proof binds
its key, CCS SHA-256, `CcsArtifactReopenProofV1`, validation-origin profile
revision, and complete ordered `CcsTargetCompatibilityProofV1` set. A reused
disposition is valid only when that validation-origin revision differs from
the report's current revision; flipping a current validation to invented reuse
fails report reopen. Missing, repeated, reordered, contract-drifted,
unattempted, corrupt, or failed outcomes prevent success. The proof ledger and
per-revision bindings are one schema-55 database authority and publish
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

### Candidate Promotion Evidence

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
ordered outcomes against the candidate catalog, requiring matching revision,
count, package key, name, version, release, architecture, and repository
checksum equality. The crawl's own strict reopen proves every package
succeeded with a current proof key, independently reopened CCS artifact,
and complete supported-target preflight set.

Canonical-map validation runs against these same reopened candidate catalogs,
so an implementation name cannot be justified by another revision or a
serving cache. The resulting canonical schema-1 artifact binds the complete
crawl digest, canonical-map digest/revision/count, every profile revision and
catalog digest/size, the package-oracle manifest digest, and both resolution
manifest digests through the recomputed comparisons. The writer stages and
synchronizes canonical bytes, atomically publishes them, synchronizes the
parent directory, independently reopens the plain file, and requires
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
`RemiPromotionEvidenceV1` against the complete conversion crawl.

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

The active-universe baseline is inspected before candidate construction. A
current universe is eligible for exact replay comparison. An
`ObsoleteProfileSchema` universe is accepted only after canonical JSON,
manifest digest, universe schema, and pointer sequence validation; activation
uses that identity and sequence solely to fence concurrent replacement and to
choose the successor sequence. Future embedded profile schemas and malformed
or pointer-divergent manifests fail closed. The hard cut never deserializes an
obsolete embedded revision as `ProfileRevisionV2`.

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
artifact, physical schema, integrity, and complete manifest binding. The Remi
publisher already performed the canonical logical/schema replay, computed the
bound table cardinalities, and rejected orphan relations before the dedicated
universe role signed those exact bytes. Configured profiles are
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
`remi_active_universe_revision` through the stored-universe inspection
boundary. A canonical, digest-bound universe containing obsolete schema-2
profile revisions is typed `ObsoleteProfileSchema` and returns HTTP 503 with
`reason=obsolete_profile_schema` while refresh rebuilds schema 3; it is never
parsed as current authority and never serves content. A current universe still
validates its strict manifest and member rows before resolving the route slug
through the exact profile revision named by that signed universe. Operational
active-profile pointers may already contain the next private candidate; they
are not public read authority. Detail, sparse
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
- `catalog_authority.rs`, `catalog_authority/revision_inspection.rs`, and
  `profile_catalog.rs`: exact-revision resolution, upgrade/rebuild inspection,
  strict manifest deserialization, verified immutable readers, reader-lifetime
  pins, and catalog projections.
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
- `conversion/benchmark.rs`: exact immutable-authority subject admission,
  isolated benchmark-state construction, strict schema-v8 registered-reopen
  and conversion measurement.
- `conversion/benchmark/report.rs`: schema-v8 evidence validation plus atomic
  report publication and strict durable reopen.
- `conversion/lookup.rs`: exact immutable-catalog package selection, verified
  source-snapshot binding, prepared key-material lookup, and upstream download.
- `conversion/metadata.rs`: safe CCS filenames, profile-backed parser dispatch,
  metadata construction, catalog package identity application, and typed
  provide comparison.
- `conversion/storage.rs`: signed CCS verification streamed directly into the
  permanent local CAS, followed by missing-only optional R2 write-through.
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

Remi includes a strict, network-independent benchmark command for measuring one
exact native artifact's cold conversion and exact hot-cache path before making
latency claims. The caller supplies an immutable-catalog package key and the
corresponding local artifact. Remi admits the artifact only when its size,
digest, package identity, exact profile revision, source snapshot, parser
configuration, trust policy, and repository identity agree with the deployed
catalog authority. Omit `--revision` to select the active revision; provide it
to benchmark a registered private revision.

```bash
cargo run -p remi --release -- conversion-benchmark \
  --config /etc/conary/remi.toml \
  --work-root /var/tmp/remi-conversion-fedora-example \
  --profile fedora-44 \
  --revision <registered-profile-revision-sha256> \
  --package-key <immutable-catalog-package-key-sha256> \
  --source-artifact /var/tmp/example.rpm \
  --hardware-label remi-production-i7-8700-xfs \
  --iterations 2
```

Benchmark setup, isolation, report schemas, counters, comparison rules, and
production-adapter requirements live in [performance evidence](../performance/README.md).

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
