---
title: Remi native full-catalog parity oracle
summary: Define producer-bound strict native parity lanes, selective same-export assembly, and deterministic bounded-parallel private collect-all native, candidate-resolution, and native/candidate comparison surveys for one complete immutable profile candidate
last_updated: 2026-09-03
revision: 54
status: active
---

# Remi native full-catalog parity oracle

## Boundary

The native full-catalog parity oracle is independent release evidence for one
exact `ProfileRevisionV2`. It is distinct from the hosted
`phase4-native-pm-parity` suite: that suite proves deterministic one-package
lifecycle and CLI behavior, while this contract covers every package admitted
to one immutable profile candidate.

The supported-profile registry is the sole target-architecture authority.
Its closed `ProfileTargetArchitecture` value declares Fedora 44 `x86_64`,
Ubuntu 26.04 `amd64`, and Arch `x86_64`; profile-revision schema 3 carries that
typed value into every `ProfileRevisionV2`. Profile catalog projection version
3 is the only current producer. Schema-2 profile revisions and
projection-version-2 profile catalogs lack this binding, are invalid, and must
be rebuilt from authenticated sources. There is no compatibility reader.

## Exact input handoff

The production native producers consume `NativeOracleInputSetV1`, not mutable
mirror state or Conary's normalized catalog bytes. The strict schema-1 bundle
binds the canonical ordered Fedora 44, Ubuntu 26.04, and Arch private candidate
revisions, every ordered `SourceSnapshotV1`, and the digest-sorted union of
their authenticated native metadata objects. Candidate construction retains
each authenticated object as a digest-named file inside the immutable source
bundle. Export independently reopens that bundle, resolves only those retained
paths, and copies an object only after its exact SHA-256 and size match. It
performs no upstream network request or URL reconstruction. Debian Release
member names remain distribution-relative for signed SHA-256 lookup, while
the recorded `source_path` preserves the exact `dists/<distribution>/` prefix
that identified the authenticated object during candidate construction.

The writer publishes canonical `manifest.json` plus digest-named files beneath
one exact `objects/` directory, synchronizes and atomically renames the complete
bundle, then independently reopens every byte. Extra or missing entries,
symlinks, noncanonical JSON, object tamper, and candidate supersession fail the
operation. The bundle is an input carrier only; the pinned ALPM, libsolv, or
apt-pkg implementation remains the sole native fact and resolution authority.

The protected production producer accepts only one successful exact export
run. It independently reopens the transport and deployment evidence and
requires the artifact-owned deployed commit to be merged. The export's
canonical operator attestation must bind its exact run ID, attempt, workflow
commit, export identity, and `protected-pinned-known-hosts-v1` contract. An
older export without that attestation cannot become strict oracle authority.
The export operator must equal freshly fetched protected `main` at initial
authorization and immediately before SSH. Production accepts the export only
when its run head equals the producer workflow's own exact current-main commit,
so rerunning a historical workflow cannot mint new input authority.
Dispatch also names
one explicit full `producer_commit`; operators use the deployed commit by
default and name a newer commit only for an intended producer advance. The
workflow fetches `origin/main`, requires the producer commit to descend from
the deployed commit and already be an ancestor of `origin/main`, then checks
out that exact clean tree separately from the protected workflow/operator
checkout. Floating branches, workflow heads, malformed SHAs, unmerged commits,
non-descendants, and dirty producer trees are inadmissible.
`scripts/verify-native-oracle-producer.py` owns this exact reusable full-SHA,
fetch, and two-direction ancestry predicate for protected native-oracle and
resolution-survey producers; workflows may not fork a weaker local version.
Three pinned container lanes derive every member and object argument from the
canonical input contract: Fedora 44 uses libsolv 0.7.36, Ubuntu 26.04 uses
apt-pkg 3.2.0, and Arch uses the pinned archive/libalpm image. A lane succeeds
only after both its package-fact producer and exact-architecture resolution
producer complete their strict output reopen. Short-lived sanitized evidence
binds both manifests and artifacts to the input manifest, profile revision,
export identity, implementation version, architecture, deployed commit,
producer commit, and independently recomputed SHA-256 digests of both producer
binaries.
The operation is read-only and carries no refresh, conversion, proof,
activation, or public-pointer authority.

The accepted producer-binding decision deliberately separates immutable input
authority from producer implementation provenance. A merged descendant may fix
producer-only behavior without forcing a semantically identical Remi deploy
and export, while the exact export continues to own every candidate, source,
and metadata byte. Merged provenance alone grants no schema latitude: package
schema 1, resolution schema 2, and every ecosystem implementation/projection
pin remain mandatory. A three-lane set may contain different producer
commits per lane only when each is a merged descendant of the same deployed
commit and every lane passes those identical pins; each lane records its own
commit and binary digests.

Every selected production lane produces diagnostics before deciding strict
authority. It reopens one exact staged export, creates and reopens the package
oracle once, runs the resolution producer with `--survey`, validates and
uploads the canonical survey plus a separate binding manifest, and only then
runs strict resolution against that same package oracle. Survey findings cause
the survey process to return non-zero after writing; the lane adapter accepts
that status only when it agrees with the validated failure inventory. A strict
failure still fails the lane and emits no strict lane artifact, but it cannot
discard an already validated survey. Survey artifacts are named separately,
carry the export/deployment/producer/image/schema/implementation/binary-digest
bindings, and remain diagnostics-only. Their type can never satisfy assembly,
comparison, promotion, activation, or publication.

Native-resolution survey binding evidence is schema 2, and strict native-oracle
lane evidence is schema 4. Those versions add the required worker count,
per-worker pool-load timings, measured worker RSS, and admitted memory budget.
This is a hard cut: schema-1 survey bindings and schema-3 strict lanes are
obsolete non-authority and must be regenerated. In particular, the first
subset production after this cut must rebuild all three strict lanes before
later subset runs may retain an unselected lane.

Dispatch input `lanes` is an optional comma-separated, non-empty,
duplicate-free subset of `fedora-44,ubuntu-26.04,arch`; its default is that
complete canonical set. Each successful strict artifact is named by exact
export identity, lane, and producer commit. Assembly always requires exactly
one strict artifact for each canonical lane. A selected lane must come from
the current run. For each unselected lane, assembly queries Actions artifacts
and chooses the newest unexpired artifact with the exact export/lane prefix,
then requires its exact producer job to have succeeded in a completed
protected-main production run. It verifies the API-recorded SHA-256 of the
downloaded archive before safe extraction and independently reopens every
canonical evidence, manifest, and artifact digest.

All three lane records must bind the same export run, export identity,
transport digest, deployment run, deployed commit, and input manifest. Each
producer commit must separately satisfy deployed-to-producer-to-`origin/main`
ancestry. Mixed descendant producer commits are accepted as decided above;
package schema 1, resolution schema 2, lane images, implementation versions,
and projection schemas remain identical per lane contract. Different exports,
non-descendants, unmerged producers, digest drift, missing or duplicate lanes,
and survey substitution fail closed. The assembled evidence records each
source workflow artifact ID/run/name/archive digest, lane evidence digest,
producer commit, both producer binary digests, and both strict oracle binding
records.

`NativeParityOracleV1` is the sole parity manifest authority. It binds the
exact profile revision digest, profile logical digest, ordered source members,
member roles and precedence, pinned native implementation and version, oracle
projection schema, normalized fact counts, and the SHA-256 and size of the
line-oriented package artifact. Unknown fields and unsupported schemas fail
closed.

Each `NativeParityPackageV1` row carries:

- exact package identity, source profile, version scheme, and architecture
  variant;
- the exact contributing source member and authenticated snapshot;
- source artifact checksum, size, and download authority;
- typed providers; and
- grouped positive and negative requirements, including conflicts, breaks,
  replacements, and obsoletes.

Package architecture is validated before a native package row can enter the
oracle writer. RPM and Debian boundaries require the exact token to exist in
their pinned format-wide tables. ALPM boundaries require the exact token in
the source profile's declared set: its target architecture plus `any`.
`NativeParityPackageV1` validation repeats the same profile-aware guard on
reopen. An absent token returns typed
`UnknownArchitectureToken { scheme, token }` before any resolution evidence
can exist; it is never normalized, admitted, or treated as a non-native
package.

The checked-in architecture fixtures pin the source authority without runtime
parsing or test-time fetches:

- RPM 6.0.1 `rpmrc.in` at tag `rpm-6.0.1-release`, commit
  `58a917a6c5e24e9e8a01976c17d2eee06249b9b6`, contributes every
  `arch_canon`, `arch_compat`, and `buildarch_compat` line from
  [the exact upstream file](https://github.com/rpm-software-management/rpm/blob/rpm-6.0.1-release/rpmrc.in).
  The pinned Fedora 44 image ships `rpm-6.0.1-2.fc44.x86_64`.
- dpkg 1.23.7 tag `1.23.7`, commit
  `ef4d59f5925661818484ac666014ee3e665aadcf`, contributes
  [`data/cputable`](https://git.dpkg.org/cgit/dpkg/dpkg.git/tree/data/cputable?h=1.23.7)
  and
  [`data/tupletable`](https://git.dpkg.org/cgit/dpkg/dpkg.git/tree/data/tupletable?h=1.23.7).
  The pinned Ubuntu 26.04 image ships `dpkg 1.23.7ubuntu1`.
- The Arch producer pins `pacman 7.1.0.r9.g54d9411-2`; its installed
  `CARCH=x86_64` derives from
  [`etc/makepkg.conf.in`](https://gitlab.archlinux.org/pacman/pacman/-/blob/54d94116164b0b2202c6061c4a59c6f3e70820d8/etc/makepkg.conf.in)
  at commit `54d94116164b0b2202c6061c4a59c6f3e70820d8`.
  [`pacman.conf(5)`](https://man.archlinux.org/man/pacman.conf.5.en#Architecture)
  defines `Architecture` as `auto`/`uname -m` or an explicit list, and the
  [pinned libalpm comparison](https://gitlab.archlinux.org/pacman/pacman/-/blob/54d94116164b0b2202c6061c4a59c6f3e70820d8/lib/libalpm/trans.c#L69-106)
  compares `%ARCH%` literally while admitting `any`. The supported `arch`
  profile therefore owns `x86_64` plus `any`; the 2026-08-02 databases in the
  fixture header prove that exact profile snapshot rather than a format-wide
  vocabulary.

Conformance tests parse those vendored files and require every RPM table token,
every dpkg CPU and tuple expansion, and every pinned Arch package `arch` value
to project to a typed class. Tokens outside the supported x86_64/amd64 machine
profiles, including Debian `x32` and RPM micro-architecture levels, remain
known typed non-native classes rather than literal fallback values.

Rows are canonical JSON ordered by the exact profile package key. The writer
and verifier retain one complete package projection at a time; neither may
construct a profile-sized package or relation collection.

## Independence

Native extraction uses the pinned native package-manager implementation named
by the manifest. Conary catalog projection may serialize and verify the strict
contract, but it cannot serve as the evidence producer for release parity. A
catalog logical digest proves deterministic Conary output, not independent
native agreement.

The ALPM producer is built only with the explicit
`native-alpm-oracle` feature, reads exact profile-member database artifacts
through pinned upstream Rust bindings to libalpm, and records the linked
libalpm runtime version. Ordinary Conary and Remi builds do not acquire a
libalpm dependency. The helper may share the strict oracle serializer and
typed fact vocabulary; it may not read Conary catalogs, Conary Arch parser
output, or operational repository SQLite as native evidence.

The producer takes one `SourceSnapshotV1` and one local database file for each
profile member in exact ordinal order. The source-snapshot manifest digest must
match the member binding. Separately, the database bytes must match the exact
`ArchDatabase` authenticated-object digest and size inside that snapshot; the
two digests describe different objects and are never substituted for one
another. The snapshot's content URL, or metadata URL when no content URL is
declared, owns package download authority.

Build and invoke the host-linked helper explicitly:

```bash
cargo run -p conary-core --features native-alpm-oracle \
  --bin conary-alpm-oracle -- \
  --profile-manifest profile.json \
  --source-snapshot core-source.json --database core.db \
  --source-snapshot extra-source.json --database extra.db \
  --source-snapshot multilib-source.json --database multilib.db \
  --output alpm-oracle
```

The helper registers the verified databases with libalpm in profile precedence
order. Every package returned by libalpm is projected or participates in exact
conflict-checked deduplication; there is no skip input. A private bounded spool
orders selected rows by package key without retaining the complete profile in
Rust memory. Success means the two-file bundle has been durably written and
independently reopened through the strict shared verifier.

ALPM soname-v1 provides containing `=`, including `libacl.so=1-64`, are atomic
soname identities rather than package-version relations. libalpm's generic
dependency handle exposes the same bytes as a name, equality mode, and version;
the producer requires those fields to reconstruct the exact native text, then
uses the pinned ALPM grammar's soname classification as semantic authority.
Soname-v2 identities remain atomic by the same rule. Ordinary package and
virtual relations continue to require exact normalized libalpm name and version
agreement, so this distinction adds no permissive fallback.

RPM package-fact evidence is produced only with the explicit
`native-rpm-oracle` feature and exact libsolv 0.7.36 runtime. Ordinary Conary
and Remi builds do not acquire a libsolv dependency. For every profile member
in exact ordinal order, the producer requires one `SourceSnapshotV1` that
binds exactly the compressed `RpmPrimary` and `RpmFilelists` objects in that
order. It copies both objects to private staging, independently verifies their
exact authenticated sizes and SHA-256 digests, and only then lets libsolv
reopen the staged bytes with filelists extending the primary solvables.

The producer projects every libsolv package and variant, payload location,
SHA-256 and size, declared and complete file providers, required and
prerequisite relations, recommends, suggests, supplements, enhances,
conflicts, and obsoletes. Rich dependency trees are decoded through libsolv's
typed relation IDs and must agree with Conary's canonical typed RPM grammar;
native display text alone cannot establish parity. The producer derives its
canonical RPM text from that typed tree, flattening only RPM's right-associated
`with` spine and retaining parentheses wherever omission would change
association. It reparses that lossless text through the canonical RPM grammar
and requires exact typed agreement. The typed source projection canonicalizes
RPM's empty serialized epoch and explicit epoch zero to omitted epoch zero,
while retaining positive epochs and the strict persisted grammar.
Exact-identity duplicates obey profile
precedence only when every projected fact agrees. A contradictory duplicate
fails the complete crawl. The private SQLite spool, canonical bundle write,
and independent complete reopen use the same bounded contract as the ALPM
producer.

Pinned libsolv also derives
`namespace:splitprovides(prefix with /path)` supplements from atomic
source-declared `prefix:/path` provides. That `REL_NAMESPACE` tree is legacy
installed-package update machinery, not an authenticated RPM `Supplements:`
record. Before excluding it from source package facts, the producer requires an
exact `namespace:splitprovides` wrapper, exact nested `REL_WITH` atoms, the
matching atomic declared capability, and same-package authenticated file
coverage. Coverage is either the exact declared path or a strict descendant
separated by `/`; lexical-prefix lookalikes and files owned only by another
package fail closed. Unknown namespaces, malformed trees or paths, and missing
source facts also fail closed.
Source-declared rich supplements remain projected through typed relation IDs and
must still agree with canonical RPM grammar.

Build and invoke the host-linked RPM helper explicitly:

```bash
cargo run -p conary-core --features native-rpm-oracle \
  --bin conary-rpm-oracle -- \
  --profile-manifest profile.json \
  --source-snapshot fedora-source.json \
  --primary primary.xml.gz --filelists filelists.xml.zst \
  --output rpm-oracle
```

Debian package-fact evidence is produced only with the explicit
`native-debian-oracle` feature and exact apt-pkg 3.2.0 from the pinned Ubuntu
26.04 image. Ordinary Conary and Remi builds do not acquire an apt-pkg
dependency. Each ordered profile member supplies one `SourceSnapshotV1` and
exactly one local object bound as `DebianPackages`. The producer copies the
compressed object to private staging, verifies its exact authenticated size
and SHA-256, and then independently reopens the staged bytes through apt-pkg's
compression, strict deb822, and dependency-expression APIs. It does not invoke
`apt`, `apt-get`, `dpkg`, their databases, the Conary Debian parser, a Conary
catalog, or operational repository SQLite.

Every deb822 stanza becomes one native row before profile deduplication. The
producer projects exact package/version/architecture and `Multi-Arch`
identity, payload location/SHA-256/size, package and declared providers,
comma-separated groups and alternatives, architecture qualifiers, required
and pre-required relations, recommends, suggests, enhances, conflicts,
breaks, and replacements. Empty, malformed, repeated-authority, or unsupported
native shapes fail the complete input. apt-pkg process globals remain behind
one ownership lock for the complete native handle lifetime. Exact-identity
duplicates use the same fact equality, precedence, bounded SQLite spool,
canonical write, and complete independent reopen contract as the ALPM and RPM
producers.

Build and invoke the host-linked Debian helper explicitly:

```bash
cargo run -p conary-core --features native-debian-oracle \
  --bin conary-debian-oracle -- \
  --profile-manifest profile.json \
  --source-snapshot ubuntu-main-source.json \
  --packages main-Packages.xz \
  --source-snapshot ubuntu-updates-source.json \
  --packages updates-Packages.xz \
  --output debian-oracle
```

## Dependency resolution evidence

`NativeResolutionOracleV1` is the separate resolver-owned authority for exact
dependency closure and unresolved dependencies. Its manifest binds the exact
`ProfileRevisionV2`, the exact `NativeParityOracleV1` manifest digest, the
solver implementation and version, its projection schema, the target
architecture, normalized counts, and the SHA-256 and size of `roots.jsonl`.
The resolution policy architecture must equal the bound profile revision's
typed target architecture during manifest binding, comparison, and promotion
proof validation.

Schema 2 fixes the resolution policy rather than accepting solver flags or
free-form policy: the installed state is empty, every exact package variant is
requested as its exact root, only required and pre-required groups enter the
positive solve, optional and build groups are excluded, and provider choice
uses native repository precedence. `architecture_admission: native_only`
admits only equality of the complete source-derived machine identity and the
source scheme's architecture-independent token (`noarch`, `all`, or `any`),
which resolves to the target identity for comparison. Its decision is the
closed runtime enum
`Admitted`, `Excluded { identity: NativeMachineIdentityV1 }`, or
`UnknownArchitectureToken { scheme, token }`. Only `Excluded` may produce
`not_installable { reason: architecture_excluded }`. Unknown tokens return the
typed producer error and the diagnostics-only survey records the separate
`unknown_architecture_token` error kind. This resolution-time branch is an
invariant guard because the package oracle must already have rejected the row.
`NativeMachineIdentityV1` contains only CPU, pointer width, endianness, and
32-bit ARM float ABI. The executable's libc and OS never enter it. Package ABI
is a separate typed dimension: dpkg contributes `gnu`, `musl`, or `uclibc`
from `tupletable`; RPM and ALPM contribute their profile-declared implied
glibc ABI. The profile registry owns the required target package ABI.
Native-only admission requires both package/profile-target machine equality
and package/profile ABI equality. The shared decision accepts the selected
profile and package architecture; it does not accept a host architecture. RPM
`arch_compat` and `buildarch_compat` still only establish that a token is known
and never grant native-only equality. Native solvers apply their pinned
package-manager architecture policy to provider selection, while the Conary
candidate resolver uses the same profile-bound full identities for RPM,
Debian, and Arch roots and providers. Conary and Eopkg repository candidates
remain on their explicit scheme-owned machine-matching paths because those
schemes have no typed foreign-profile architecture authority. A different
policy requires a schema change.
The three native producers and the Conary candidate producer derive the solver
architecture from the profile revision. Their `--architecture` or operator
input is only an assertion: a mismatch returns the typed
`ProfileArchitectureMismatch` error before any root is walked or output bundle
is created.

There is exactly one canonical row for every package key in the bound package
oracle. A row records exactly one outcome:

- `resolved`, with a strictly ordered duplicate-free closure of exact package
  keys that includes the root; or
- `unresolved`, with a strictly ordered duplicate-free set of requiring
  package keys and canonical required-group digests; or
- `not_installable { reason: architecture_excluded }`, when native-only policy
  excludes the exact root before Conary's SAT solver or a Debian/ALPM native
  solve is invoked, or when libsolv reports the matching exact-root
  `SOLVER_RULE_PKG_NOT_INSTALLABLE` rule.

The writer and reader retain one root outcome at a time. Complete reopen uses
a private disk-backed membership index to prove that every closure reference,
requiring package, and unresolved required group exists in the exact package
oracle; root completeness is a separate bounded merge walk. Unknown fields,
mixed or empty outcomes, reordered or duplicate roots/references, count drift,
noncanonical bytes, tamper, extra bundle entries, symlinks, and package-oracle
drift fail closed.

Comparison applies the same exact profile, package oracle, architecture, and
typed policy to native and Conary evidence. It merge-walks one root pair at a
time and reports typed oracle-only root, candidate-only root, outcome,
dependency-closure, unresolved-dependency, or not-installable-reason drift.
Diagnostic strings and native solver error prose never establish the result.

This schema is a hard cut. Resolution-oracle schema 1, RPM projection schema
3, Conary candidate projection schema 1, Debian and ALPM projection schema 1,
comparison schema 1, and survey schema 1 have no compatibility readers. Every
retained native-resolution and Conary candidate bundle is invalid and must be
regenerated before comparison or promotion proof.

### Diagnostics-only resolution survey

The three native resolution binaries also accept `--survey <FILE>`. Exactly
one of `--output <DIRECTORY>` and `--survey <FILE>` is required. Survey mode
walks every exact package-oracle root even when a native result cannot be
projected into the strict resolution contract. It writes one create-only
canonical `NativeResolutionSurveyV1` JSON file, refuses to replace an existing
path, and exits non-zero after writing when any root failed so unattended
diagnostics cannot look successful.

`NativeResolutionSurveyV1` schema 2 binds the profile identity and revision
digest, package-oracle manifest digest, native implementation and projection
schema, fixed resolution policy, and target architecture. Its counts record
roots walked, resolved, unresolved, not-installable, and failed plus a canonical histogram
keyed by the originating typed Conary `Error` variant and a stable short
reason. Each retained failure records the exact root package key,
name/version/release/architecture, full sanitized error message, and typed
native explanation. The inventory retains at most 5,000 failure records while
reporting the uncapped `total_failures`, retained count, limit, and explicit
`truncated` state.

RPM explanations preserve every libsolv problem and every rule in that
problem, including numeric and symbolic `SOLVER_RULE_*` type, native index,
from/to package key plus name-EVR-architecture, dependency ID, and dependency
text. Native-only provider admission removed the strict-priority multilib
problem shape, so there is no residual solve without strict priority and no
ancillary package-conflict or inferior-architecture tolerance. Either rule is
fatal if it appears. Any native field that cannot safely be
projected carries an explicit unavailability reason. Debian explanations retain
the selected native package identities or typed missing requirements when
apt-pkg returns them; an
apt-pkg failure that exposes no typed result says so. ALPM explanations retain
prepared package identities, typed missing requirements, and package-conflict
records. The pinned Rust ALPM binding cannot safely dereference its
invalid-architecture detail list, so that typed result records the detail as
unavailable rather than inventing or unsafely reading it.

Survey collection and strict writing share the same per-root resolution path.
libsolv clears the previous solver/transaction before each solve, apt-pkg
clears result storage and constructs fresh dependency caches per root, and
libalpm releases every transaction before the next root. A failed root
therefore cannot contaminate a later solve.

Every strict and survey root walk is parallel behind one bounded,
sequence-numbered sink. Input dispatch follows package-oracle order and only
the parent/calling thread updates writers, collectors, histograms, record caps,
or the 64 MiB explanation budget. The next sequence goes to the first available
worker, so an uneven solve cannot strand idle capacity behind a busy worker's
private queue. Results may finish out of order, but the sink does not observe
root `n + 1` before root `n`; strict mode stops dispatch after the first failing
canonical root, drains workers, and returns that failure.
Consequently worker scheduling cannot change `roots.jsonl`, manifest bytes or
digests, survey JSON, counts, histograms, caps, or budget decisions.

RPM and ALPM use threads with a private libsolv pool or libalpm handle and a
private read-only SQLite index connection per worker. Conary workers likewise
open one read-only SQLite connection apiece and construct fresh resolvo state
per root. apt-pkg configuration and system pointers are process-global, so the
Debian lane uses child processes; each builds its own cache and solver from the
same staged authenticated `Packages` inputs. No native handle crosses a thread
or process boundary.

`--workers <positive-integer>` is a typed input on all three native resolution
binaries and on `remi resolution-survey`. Omission selects the minimum of
`available_parallelism()`, the cgroup-v2 CPU quota, root count, and memory
capacity. Memory capacity subtracts `memory.current` from every bounded
cgroup-v2 ancestor (or uses host `MemAvailable`), reserves 25% of that remaining
capacity, caps the worker-pool budget at 8 GiB, and divides it by the retained
Fedora single-pool allowance of 1.5 GiB (rounded above the measured 1,271,280
KiB one-worker root-walk RSS observation). Native binaries require a separate
`--implementation-evidence <FILE>` destination. Its create-only schema-1 JSON
records the selected worker count, every worker's pool/cache load milliseconds,
the effective memory budget, and the measured allowance; those run-dependent
facts never enter canonical oracle or survey bytes.

The retained Fedora measurement used the fresh export from successful
`export-remi-native-oracle-inputs` run `33699383309`, profile-manifest SHA-256
`9004072f1fc9b1b932616a4b8b33a2277241c481734670f4172aa378433ba084`,
and all 101,187 roots. Both passes used release binary SHA-256
`407485a67107802a670561db60b4fbcb3cc2f05a11c6b0baef58bbdd4e387198`
from commit `23f702c3` inside `conary-oracle-fedora-slice6`. The observed 12 CPUs
and 8 GiB worker budget made five workers the automatic capacity.

| Workers | Wall seconds | User seconds | System seconds | Peak RSS KiB | Per-worker pool load ms |
| ---: | ---: | ---: | ---: | ---: | :--- |
| 1 | 10,832.801 | 3,231.511 | 184.536 | 1,656,556 | 26,540 |
| 5 | 6,774.840 | 3,458.632 | 192.119 | 3,801,984 | 31,460; 31,540; 31,676; 31,479; 31,519 |

The five-worker end-to-end speedup was 1.60x. Both runs recorded the same 45
typed failures and produced byte-identical 167,998-byte survey JSON with
SHA-256
`0fd754eed04d6cd9bfa5e7a58d392b1eec4c9b3bf4b31b91805e833c3826874c`.
The mandatory authenticated package-oracle reprojection was storage-bound and
varied between the sequential runs, so wall time is retained with CPU, RSS,
and pool-load evidence rather than treated as an isolated solver benchmark.

Survey JSON is a diagnostics aid only. It never creates `manifest.json` or
`roots.jsonl`, is not a `NativeResolutionOracleV1` bundle, and has no parity,
comparison, promotion-proof, activation, or publication authority. Promotion
continues to require the strict bundle and complete independent reopen. Survey
records contain package identities and native solver evidence only; private
paths, credentials, tokens, environment data, and host details are forbidden.

For any ecosystem, use the same authenticated inputs as strict production and
replace the output destination, for example:

```bash
cargo run -p conary-core --features native-rpm-oracle \
  --bin conary-rpm-resolution-oracle -- \
  --profile-manifest profile.json \
  --source-snapshot fedora-source.json \
  --primary primary.xml.gz --filelists filelists.xml.zst \
  --package-oracle rpm-oracle \
  --architecture x86_64 \
  --workers 4 \
  --implementation-evidence rpm-resolution-implementation.json \
  --survey rpm-resolution-survey.json
```

The Debian and ALPM binaries use the same `--survey <FILE>` alternative with
their existing `--packages` and `--database` member inputs respectively.

### Candidate-resolution and comparison surveys

`ConaryResolutionSurveyV1` schema 1 is the diagnostics-only Conary counterpart
to the native survey. It binds the exact profile revision, package-oracle
manifest, `conary-sat` implementation and projection schema, native-only
policy, and the profile's typed target architecture. The policy architecture,
operator assertion, and profile target must agree before output creation. The
producer walks every exact package-oracle key through the same per-root code as
strict candidate production. Every successful root retains its exact
name/version/release/architecture/key and one complete `resolved`,
`unresolved`, or `not_installable` outcome. A failed root contributes to
uncapped counts and a canonical histogram keyed by typed Conary error variant
and stable producer reason; up to 5,000 failures retain the same exact root
identity, full error message, and native explanation.

Candidate native explanations are projected directly from resolvo's typed
`ConflictGraph`, `ConflictNode`, `ConflictEdge`, and `ConflictCause` values.
They retain unresolved-node incoming edges with the requiring solvable and
rendered requirement/version sets, conflict edges with both solvable
identities and typed conflict kind, and excluded solvables with their typed
provider reason. No `display_user_friendly` text is parsed. Explanations share
the native survey's exact canonical-JSON accounting, 64 MiB budget,
failure-record cap, first-exhaustion withholding rule, and independently
validated count/truncation invariants. They are built only on a hard per-root
failure.

`NativeResolutionComparisonSurveyV1` schema 1 first reopens two complete,
package-oracle-bound resolution bundles, then walks every root pair in
canonical key order. Every retained mismatch records the exact root identity,
typed mismatch kind, both complete outcomes, and the manifest SHA-256 that
identifies each side's evidence. It retains at most 5,000 mismatch records
while preserving uncapped totals, a canonical histogram by mismatch kind, a
canonical histogram by native/candidate outcome-kind pair, and explicit
truncation. Strict comparison still aborts on its first mismatch.

`remi resolution-survey` owns both surveys under the normal exclusive
stopped-runtime lock and mirrors `promotion-prove`'s ordered private bindings:

```text
remi resolution-survey \
  --config /etc/conary/remi.toml \
  --candidate fedora-44=<profile-revision-sha256> \
  --candidate ubuntu-26.04=<profile-revision-sha256> \
  --candidate arch=<profile-revision-sha256> \
  --package-oracle fedora-44=<directory> \
  --package-oracle ubuntu-26.04=<directory> \
  --package-oracle arch=<directory> \
  --native-resolution fedora-44=<directory> \
  --native-resolution ubuntu-26.04=<directory> \
  --native-resolution arch=<directory> \
  --architecture fedora-44=x86_64 \
  --architecture ubuntu-26.04=amd64 \
  --architecture arch=x86_64 \
  --workers 4 \
  --output-dir <new-private-survey-directory>
```

The output directory is create-only and mode `0700` on Unix; each canonical
survey file is create-only and mode `0600`. Per-profile candidate and
comparison implementation JSON files record the selected workers and their
load times separately from those canonical surveys. A profile with candidate failures
cannot produce a complete candidate bundle, so its comparison survey is
skipped while later profiles are still surveyed. Complete candidates are
materialized only below an automatically removed temporary directory. The
command reports all written findings and returns Remi's top-level failure status
`101` when any candidate failure or comparison mismatch exists.

Neither survey is evidence authority. Their JSON cannot be opened as a strict
resolution bundle or `NativeResolutionComparisonV1`; promotion proof,
activation, publication, and every binding/validation path reject it. Survey
files also carry no private paths, credentials, environment data, or host
identity.

The protected production consumer is `.github/workflows/survey-remi-resolution.yml`.
Its single `oracle_run_id` selects one successful three-lane
`produce-remi-native-oracles` run. The workflow authenticates that run's
head as its own exact current protected-main operator commit, then authenticates
the assembled three-lane artifact and derives and reopens the exact export and
deployment runs, then verifies the API metadata, successful producer job, and
archive digest for each referenced strict lane. Retained same-export lanes from
earlier successful runs remain valid only through those bindings. It
authenticates the lane files into one manifest-bound transport and requires the
export's typed operator attestation to bind that run's exact workflow commit to
the protected pinned-host-key SSH contract.
Pre-attestation exports are non-authority. The survey workflow requires the
helper from its exact protected `github.workflow_sha` to be byte-identical to
the helper at its freshly fetched protected `origin/main` and requires the
complete workflow revision to equal that exact current-main commit. It stages
the helper, refetches protected main, repeats both equalities immediately before the existing
`install-helper` action. The root helper independently resolves protected main
through GitHub's HTTPS API, fetches that exact commit's helper, matches its
digest, and installs those root-fetched bytes rather than caller-staged code.
The workflow then calls the three-argument
`conary-remi-deploy survey-resolution` action with the survey identity, export
identity, and typed oracle transport path. The root-owned helper reads the exact
candidate revisions from the stopped deployment's own pointers, uses the
profile-bound architectures, and freezes the survey JSON under root ownership
before restarting. Cleanup owns every exit from the instant root staging is
created, and deployment-inspection and survey stderr remain in one mode-`0600`
staging diagnostic that is never transported or logged. It accepts status `101`
only when the typed outcome records
at least one finding, polls `/health/ready` to a bounded successful result
regardless of those findings, and returns only survey JSON
and separate resolution-walk implementation JSON plus a digest, size,
deployment, candidate, and oracle binding manifest. Survey transport manifest
and verification evidence schema 2 bind every candidate/comparison survey to
its implementation file; the independent reader validates the worker count,
per-worker load-time vector, effective memory budget, and retained worker RSS
allowance. The
workflow independently reopens that transport, enforces the complete typed Rust
survey schemas and their cross-count, retention, evidence-budget, and mismatch
relationships, including the fixed 5,000-record and 64-MiB evidence limits. It
binds candidate implementation to the profile ecosystem, `conary-sat`, and
projection schema 2. Comparison counts must cover the exact complete
zero-failure candidate root population, and every retained mismatch root,
identity, and candidate outcome must come from that candidate survey. It then
compares every authority binding with its authenticated input verification, and
its seven-day artifact
also retains the authenticated three-lane assembly. Neither helper input
admission nor runner output verification imposes an aggregate transport limit
absent from the producer contract. The uncompressed input archive uses GNU
base-256 tar headers to avoid USTAR's unsupported 8-GiB per-member ceiling
without admitting PAX metadata. The runner chunk-copies and authenticates each
declared member into private mode-`0700` staging, maps those files read-only,
and decodes large root-record arrays one canonical record at a time.
Its comparison join keeps only the fixed retained-mismatch envelope rather
than indexing the complete candidate population. Remi returns bounded
per-profile summaries to the helper, including the comparison candidate
manifest digest, so transport construction never reparses a whole survey with
`jq`. The workflow reader reconstructs the exact strict candidate root stream
and manifest from streamed outcomes and the authenticated package manifest,
and it requires those zero-failure roots and identities to cover the mapped
authenticated package rows exactly. Nested closure and dependency vectors are
streamed element by element, and copied survey files are discarded after each
profile; the comparison digest must match even for zero mismatches. All
profiles bind their total and retained root identities to the package
stream before the findings branch. Zero-failure profiles additionally replay
the authenticated native root stream against candidate outcomes and recompute
the comparison totals, ordered histograms, and retained evidence.
Aggregate and per-profile summary counts retain exact JSON integer types. The
helper archives the frozen root-owned survey files directly after service
restoration, so transport construction does not allocate another survey-sized
staging copy. Authenticated oracle members are materialized in private
root-owned staging on the `/conary/evidence` capacity domain, leaving `/tmp`
to hold only the caller-owned ingress transport and sanitized egress archive.
Runner assembly removes each authenticated artifact ZIP after extraction and
consumes each extracted lane member after writing it to the transport, avoiding
a three-copy unbounded full-catalog working set.
Raw deployment-inspection
and survey stderr remain confined to
mode-`0600` root-controlled helper staging, are destroyed during helper cleanup,
and are never emitted through SSH or
workflow logs; public failures contain only a typed helper message. Neither side
has promotion, activation, or publication authority.
An older workflow rerun fails before root mutation even when helper bytes are
unchanged, and any protected-main advance during input processing fences the
run. Stale verifier code therefore cannot certify evidence or leave the root
entry point downgraded.

ALPM resolution evidence is produced by the same explicit
`native-alpm-oracle` feature and pinned libalpm runtime as the package-fact
oracle. The resolver helper independently reopens the supplied package bundle,
then reproduces that entire package oracle from the authenticated database
objects and requires exact manifest equality before solving. For each exact
package row, it prepares a database-only libalpm transaction against an empty
local database with the target architecture and profile databases registered
in precedence order. Prepared transaction packages become exact closure keys;
typed libalpm missing-dependency records become exact requiring-package keys
and canonical required-group digests. A non-native exact root becomes the
typed architecture-excluded outcome before transaction setup. Conflicting
transactions, ambiguous identities, unbound requirements, and unexpected
native error classes fail the complete crawl. The public Arch profile's three
authenticated database inputs are all `/os/x86_64`; their package rows are
`x86_64` or architecture-independent `any` under the pinned lane.

Invoke the resolver helper with the exact package bundle produced above:

```bash
cargo run -p conary-core --features native-alpm-oracle \
  --bin conary-alpm-resolution-oracle -- \
  --profile-manifest profile.json \
  --source-snapshot core-source.json --database core.db \
  --source-snapshot extra-source.json --database extra.db \
  --source-snapshot multilib-source.json --database multilib.db \
  --package-oracle alpm-oracle \
  --architecture x86_64 \
  --implementation-evidence alpm-resolution-implementation.json \
  --output alpm-resolution-oracle
```

Success means every exact package-oracle row has one canonical outcome and the
two-file resolution bundle has been durably written, reopened, and fully
cross-checked against that exact package oracle.

RPM resolution evidence is produced by the same explicit
`native-rpm-oracle` feature and exact libsolv 0.7.36 runtime as the RPM
package-fact oracle. The resolver independently reopens the supplied package
bundle, freshly reproduces its entire manifest from the authenticated primary
and filelists objects, and loads those objects again into a target-architecture
solver pool. Profile member precedence becomes native repository priority;
distinct versions remain native candidates, while exact duplicate identities
retain the already-proved higher-precedence provenance.

After `pool_setarch`, the shim calls libsolv 0.7.36
`pool_setarchpolicy(pool, architecture)` with the single native architecture.
Pinned `poolarch.c` initializes `noarch` as installable independently of that
policy string. Cross-machine solvables remain inspectable exact roots but are
not installable and are absent from the prepared provider index.

Every package-oracle key binds through a private disk-backed index to one exact
native solvable root. Weak relations are disabled. Successful transaction IDs
become exact closure package keys. Typed libsolv problem-rule and dependency
IDs become exact requiring-package keys and canonical required or pre-required
group digests. An excluded exact root must carry libsolv's matching
`SOLVER_RULE_PKG_NOT_INSTALLABLE` and becomes the typed architecture-excluded
outcome; the same rule for an admitted root is fatal. A typed missing file requirement triggers an exact lookup in
libsolv's independently reopened complete filelists and one re-solve before it
may remain unresolved. `SOLVER_RULE_INFARCH`, package conflicts, unexpected
rule classes, native identity ambiguity, and input or oracle drift fail the
complete crawl. Diagnostic strings never establish an outcome.

Invoke the resolver helper with the exact RPM package bundle produced above:

```bash
cargo run -p conary-core --features native-rpm-oracle \
  --bin conary-rpm-resolution-oracle -- \
  --profile-manifest profile.json \
  --source-snapshot fedora-source.json \
  --primary primary.xml.gz --filelists filelists.xml.zst \
  --package-oracle rpm-oracle \
  --architecture x86_64 \
  --implementation-evidence rpm-resolution-implementation.json \
  --output rpm-resolution-oracle
```

Success has the same complete per-root and independent reopen meaning as the
ALPM producer. Neither native helper reads Conary catalog rows or invokes a
source package manager executable.

Debian resolution evidence is produced by the same explicit
`native-debian-oracle` feature and exact apt-pkg 3.2.0 runtime as the Debian
package-fact oracle. The resolver independently reopens the supplied package
bundle, freshly reproduces its entire manifest from the authenticated
`Packages` objects, and loads private volatile apt-pkg source indexes for the
target architecture. It uses an empty status file and never reads an installed
package database.

Profile member order is projected into apt-pkg candidate policy and native
provider priority. Every exact package-oracle key binds through a private
disk-backed index to one exact native package version and becomes a root.
The pinned apt 3.0 dependency solver receives that exact root as a protected
forced version and runs with non-strict pinning against the complete
authenticated version universe. Native policy still orders dependency choices,
so the highest-precedence candidate is selected whenever it permits a complete
transaction; a lower authenticated version remains eligible only when the
forced exact root cannot close with the candidate.
Required and pre-required groups participate in resolution; weak groups do
not. Successful native transactions become exact closure package keys. When
the complete solver fails, the producer inspects the retained protected exact
root in apt-pkg's post-solver dependency cache. A broken root-level required or
pre-required group becomes typed missing evidence only when apt-pkg exposes no
authenticated candidate version satisfying any alternative, as decided by
`DepIterator::IsSatisfied`. This covers both an absent target name and a target
name available only at incompatible versions. Every broken hard group retained
on the root must meet that rule; a separate broken group with a satisfying
candidate keeps the complete failure fatal. Each `AptMissingRequirement`
carries the exact-root identity, relation kind, and parser-owned native
dependency text; the Rust boundary binds that text to the exact package-oracle
group recorded by the same Debian parser, without textual normalization.

Pinned apt-pkg 3.2.0 does not expose solver3's typed failure reason graph as a
public API: solver state, work, trail, and clause registration are protected or
private, `DependencySolver` is final, and its exported reason interface renders
strings. Diagnostic text is not parsed into authority. Consequently a failure
that cannot be attributed from a broken hard dependency on the retained exact
root remains a fatal native solver classification. This includes transitive
no-candidate dependencies as well as conflict-, break-, policy-, or
version-coexistence failures. Solver timeout attribution uses a steady
monotonic duration and always remains a fatal `NativeSolverFailed` survey
record. A policy-excluded exact root becomes the typed
architecture-excluded outcome before apt-pkg resolution. The
Ubuntu 26.04 profile supplies only sixteen `binary-amd64` indexes; apt-pkg is
likewise configured with only `APT::Architecture(s)=amd64`, while
`Architecture: all` remains admitted.
Conflicts, native identity ambiguity, unsupported profile cardinality, and
input or package-oracle drift fail the complete crawl. Diagnostic strings never
establish an outcome.

Invoke the resolver helper with the exact Debian package bundle produced
above:

```bash
cargo run -p conary-core --features native-debian-oracle \
  --bin conary-debian-resolution-oracle -- \
  --profile-manifest profile.json \
  --source-snapshot ubuntu-main-source.json --packages main-Packages.xz \
  --source-snapshot ubuntu-updates-source.json --packages updates-Packages.xz \
  --package-oracle debian-oracle \
  --architecture amd64 \
  --implementation-evidence debian-resolution-implementation.json \
  --output debian-resolution-oracle
```

Success has the same complete per-root and independent reopen meaning as the
ALPM and RPM producers. The helper invokes no `apt`, `apt-get`, or `dpkg`
executable and reads neither their databases nor Conary catalog rows.

## Conary candidate resolution evidence

`produce_conary_resolution_candidate` is the candidate-side owner. It first
independently reopens the exact package and native-resolution oracle bundles,
requires the verified profile catalog to match every package-oracle fact, and
requires the native oracle to use schema 2's exact target policy. It cannot
resolve an unproved catalog or silently substitute another architecture.

The producer replays the catalog into a private temporary current-schema
resolver database. Two private mapping tables retain exact catalog package
keys and canonical requirement-group digests beside their temporary persisted
IDs. This database is evidence-generation machinery, never package or
publication authority. Every package-oracle key becomes an exact persisted-ID
root constraint, so another version, release, architecture, or repository
variant cannot stand in for it. The existing typed Conary SAT provider owns
native version, architecture, provider, Boolean grouped-requirement, and
negative-relation semantics. Optional and build groups remain outside the
positive solve.

Before constructing an exact SAT root, the producer applies the bound
native-only rule to that package through `PackageSelector`. An excluded root is
written as `architecture_excluded`, so the SAT invariant error for an exact
root with no eligible candidate remains unreachable on that path. The SAT
provider then applies repository-row admission once when each name-,
canonical-, declared-capability-, file-, soname-, AppStream-, or exact-root-
discovered row would become a solvable. A rejected provider never receives a
solver ID or enters a provider index; an unknown token is a typed load error.
An admitted root whose only provider is excluded therefore retains the
ordinary typed unresolved required edge. Debian Multi-Arch dependency
qualifiers remain match-time semantics over already-admitted solvables.

Successful SAT selections map back to a strictly ordered set of catalog
package keys. An unsatisfiable dependency maps Resolvo's typed conflict graph
back to the exact persisted required or pre-required group; diagnostic text is
never parsed. Package conflict, a missing mapping, an untyped unsatisfiable
result, or any selected identity outside the catalog is a hard crawl failure
rather than an unresolved row.

The producer writes one complete `NativeResolutionOracleV1` bundle using the
`conary-sat` implementation identity and projection schema 2, durably closes
it, independently reopens and cross-checks every package and group reference,
and compares it with the pinned native bundle. Success therefore proves one
canonical outcome for every exact catalog variant and returns the exact
candidate/native comparison record. A closure, unresolved-set, policy, root,
package-fact, or binding mismatch fails closed.

## Separate Slice 6 owners

ALPM, RPM, and Debian own independent pinned native evidence. Conary owns the
complete candidate crawl, durable reopen, and exact comparison. Initial
conversion crawling, exact proof reuse, independent CCS reopen, and target
preflight remain separate evidence owners. `RemiPromotionEvidenceV1`
independently reopens their artifacts, recomputes both parity comparisons, and
binds them with the complete crawl and canonical-map validation to the same
exact ordered public candidate set. The promotion owner consumes that evidence,
reopens exact proof-bound CCS bytes and every referenced durable CAS object,
publishes and reopens the signed universe bundle, and changes every selected
profile pointer plus the universe pointer in one transaction. Evidence-free
publication is limited to exact-active-authority metadata renewal.
