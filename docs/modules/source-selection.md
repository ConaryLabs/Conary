---
last_updated: 2026-08-21
revision: 42
summary: Document coherent native inventory adoption, opaque native source identity, exact native trust takeover, capability-driven targets, bounded dependency acquisition, and lifecycle handoff
---

# Source Selection Module (conary-core/src/repository/ + conary-core/src/model/)

Source selection is the policy layer that decides which repositories are
eligible to satisfy a request and how allowed candidates are ranked once they
are eligible.

Conary now uses one shared source-selection model across install, update,
model diff/apply, and replatform planning instead of keeping separate policy
logic in each flow.

## Data Flow

```text
repository declaration + authenticated trust
  |
  +-- RepositorySourcePolicy
  |     opaque source_identity
  |     ecosystem + native version scheme
  |     stream + follow or authenticated pin
  |
  +-- Repository
        exact repository_identity
        optional named feed projection
                 |
                 v
        transaction provenance
                 |
                 v
        ResolutionPolicy
        root scope + exact source identity
                 |
                 +-- SAT selection / install / update
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `SystemConfig` | `model/parser/source_policy.rs` | Model-layer ownership convergence config from `[system]` |
| `ConvergenceIntent` | `model/parser/source_policy.rs` | How aggressively Conary should move packages toward Conary-managed state |
| `DependencyMixingPolicy` | `repository/resolution_policy.rs` | Closed `strict`/`guarded`/`permissive` dependency-mixing contract |
| `ResolutionPolicy` | `repository/resolution_policy.rs` | Exact repository/source request scope and transaction source identity used by the resolver |
| `EffectiveSourcePolicy` | `repository/effective_policy.rs` | Request-scoped runtime policy with no ambient distro authority |
| `DiscoveredRepositoryDeclarations` | `repository/declarations/discovery.rs` | Lossless, source-located APT, DNF5, libzypp, and ALPM declarations confined to an explicit selected root |
| `NativeTrustImportPlan` | `repository/declarations/trust_import/` | Deterministic importable, ambiguous, or unsupported role-separated trust preview for discovered native repositories |
| `NativeRepositoryTakeoverPlan` | `repository/declarations/takeover/` | Deterministic enrollment, owned-projection, follow/pin, ALPM keyring-binding, apply, and rollback authority |
| `RepositorySourcePolicy` | `db/models/repository/source/policy.rs` | Exact native source, typed ecosystem/version ordering, stream, scope, and closed follow-or-pin decision |
| `AuthenticatedSnapshotIdentity` | `repository/parsers/snapshot.rs` | SHA-256 identity of the exact top-level metadata bytes admitted by native trust |
| `SystemAffinity` | `db/models/system_affinity.rs` | Informational installed-provenance measurement that never selects mutation authority |
| `ReplatformExecutionPlan` | `model/replatform.rs` | Executable and blocked replatform transactions derived from planned replacements |

## Model Inputs

`[system]` owns only target ownership convergence. Distro pins, source
allowlists, and mixing settings are rejected as unknown fields. Source choice
belongs to enrolled repository identity and an explicit request scope, not to
a global system label. The `conary distro` command is therefore diagnostic:
`list` shows named feed presets and `info` shows measured installed affinity;
neither mutates selection policy.

## Runtime Mirrors

Conary persists source authority per repository through
`RepositorySourcePolicy`. `load_effective_policy()` starts with no ambient
source authority and binds one only from `--from`, `--repo`, or exact selected
root provenance. A native repository supplies its opaque, stream-bound
`source_identity`; static and Remi sources may project a named feed ID. Package
format is never a substitute: Fedora, Tumbleweed, Mint, and MX may share an
ecosystem with other sources without becoming interchangeable authorities.
Strict dependency resolution rejects a transitive candidate until one exact
transaction source identity is established.

`SystemAffinity` is a measured summary of exact installed-package provenance.
It is deliberately not an input to eligibility, candidate ranking, canonical
equivalence, update selection, or any other mutation decision. It exists for
informational display and to estimate how many installed packages a requested
replatform may need to realign.

The repository feed catalog makes
`crates/conary-core/src/repository/supported_profiles/` the source of truth for
configured feed IDs, package format, version scheme, and Remi route-family
mapping. Fedora 44, Ubuntu 26.04, Arch, and Solus are the currently configured public
feeds, not the only destination systems Conary supports. Internal route slugs
such as `fedora` and `ubuntu` are not feed IDs. The
`repo add --source-profile` surface accepts only those exact public IDs and
requires the declared profile's package format to match the repository parser.
Remi
sync and package fetches translate the stored public ID to the profile-owned
route slug. Remi sync pages through the sparse name index and incrementally
stages each fixed-size `include=versions` page of resolution-only
version/provide/requirement documents and trusted package-advisory metadata;
one final fenced transaction publishes the completed local candidate over the
previous offline-resolution snapshot. Every page carries one exact-profile
sparse revision with a projection-schema version, monotonic sequence, and
strict state identity, and the client rejects a mixed-revision page set even
when its name total and ordering remain unchanged. A durable per-repository run records
the process owner, fencing epoch, candidate, revision, heartbeat, state, and
typed failure; restart recovery removes only the exact candidate proven
abandoned by its expired lease. Sync never fetches the whole-distribution
metadata document, retains a distribution-sized package vector, or issues one
HTTP request per package. Persisted Remi package identity requires the exact
public ID; route slugs and generic `rpm`/`deb`/`arch` format labels are never
accepted as source-identity aliases. Native repository sync instead consumes
the exact persisted native source policy described below. A configured public
profile remains an optional feed preset during W10 and is copied into native
package rows when present; it is not required for repository identity or
refresh authority. The superseded
`data/distros.toml` catalog was deleted in M4d.

An accepted Remi conversion remains server-owned work until the typed job
status reaches `ready` or `failed`. The client continues to observe `pending`
and `converting` jobs; it does not turn elapsed wall time into conversion
failure. Cancellation and complete-operation deadlines belong to the caller.
`RemiClientCore` owns the one decision over the typed job state, so a status
outside that contract is a typed rejection rather than an active job. Any
future client rebuilds on that same authority instead of restating it. Bounded
transport and 5xx retries stay separate from that decision: exhausting them is
a transport failure, not a conversion failure.

## Remi CCS Package Authority

Authenticated source metadata and authenticated converted CCS output are
separate boundaries. `crates/conary-core/src/repository/remi_authority.rs` and
its embedded `remi_authority/catalog.toml` own the release-tracked Ed25519
package-authority pins for Conary's canonical Remi service. A catalog entry is
selected only by the exact `https://remi.conary.io` origin and one exact public
profile ID. A route slug, package format, repository name, deceptive hostname,
or non-root URL cannot select those keys.

`conary system init` creates or reconciles each Conary-owned Remi repository
and its `repository_package_keys` rows in the same transaction. An explicit
`conary repo add` for the exact canonical origin/profile pair does the same.
The self-hosted key option cannot override that release-tracked canonical
authority.
Repeated initialization compares the semantic key set without rewriting its
sync timestamps, and Remi sparse sync replaces package rows without replacing
the repository's package authority. A same-name repository whose endpoint or
strategy is operator-managed is left unchanged.

For a noncanonical Remi endpoint, `repo add` requires one or more authenticated
`targets.public` files through the repeatable `--ccs-package-key` option and
persists them transactionally with the repository. It never imports canonical
ConaryLabs keys from a profile name alone or accepts a key delivered by the
same unauthenticated package response. Install derives exact repository
provenance, admits only that repository's active keys, and fails closed for an
unknown or cross-profile signer. Retired keys remain history, not install
authority.

## Canonical Package Map Authority

Canonical package equivalence is mutation authority because it can make a
package under one source name satisfy a request made under another. Start with
`crates/conary-core/src/canonical/exchange.rs` for the Remi wire contract,
`canonical/rules.rs` for local exact contracts, and
`db/models/canonical.rs` for persistence rules.

Only two typed authorities can create an implementation mapping:

- `Contract`: a versioned local document containing literal canonical names,
  package names, and exact public profile IDs.
- `Remi`: one checksum-verified canonical-map snapshot fetched from an
  explicitly configured Remi endpoint.

The current schema permits one implementation for each
`canonical_id`/public-profile pair and rejects every other authority string.
One source package may intentionally implement multiple canonical identities;
reverse lookup of that package name is therefore ambiguous unless the caller
also supplies the canonical identity or exact profile. AppStream may attach one
globally unique application ID to an already-authorized mapping. Repology and
AppStream caches cannot create mappings, choose packages, or resolve conflicts.

Canonical map exchange is a versioned hard contract. The JSON document
requires:

- `schema_version: 1`
- a monotonic content `revision`
- the persisted rebuild timestamp as `generated_at` (`null` only for the empty
  revision-zero map)
- each canonical name's exact `kind`, optional `category`, and exact
  public-profile-to-package map

The response body is bounded and must match
`X-Conary-Canonical-Sha256`; the server reports the content revision through
`X-Conary-Canonical-Revision`. Unknown fields, unsupported schema versions,
route aliases, unknown profiles, duplicate keys, duplicate canonical entries,
and conflicting mappings fail before persistence. Remi snapshot replacement is
transactional. Identical Remi data never demotes an existing `Contract` row;
a local contract may promote an identical Remi row, and a package-name
disagreement rolls the whole snapshot back.

There is no persisted per-package override table. Cross-profile movement is an
explicit scoped request or replatform transaction, not a decorative ranking
side channel.

## Native Repository Declaration Discovery

`crates/conary-core/src/repository/declarations/` owns the lossless declaration
grammars that precede native repository enrollment. It reads APT one-line and
deb822 sources, DNF5 repo files, libzypp repository and service files, and ALPM
configuration with ordered includes from an explicit selected root. Documents
retain exact source and expose typed source locations, disabled state,
duplicates, variables, endpoint precedence, and include order. Unknown
authority, invalid UTF-8, and root escapes fail closed; libzypp extras that
upstream preserves are retained as uninterpreted evidence and refused by
authoritative discovery.

This layer is not trust, enrollment, persistence, or enablement authority. It
does not invoke a native manager or read a native database. The exact upstream
pins, grammar inventory, and consumer boundary are recorded in
[`docs/specs/native-repository-declarations.md`](../specs/native-repository-declarations.md).

## Native Trust-Import Planning

`repository/declarations/trust_import/` consumes discovered declarations and
reads only their explicit local key material below the same selected root. Its
strict JSON preview retains native declaration identity, enabled state, source
locations, exact certificate fingerprints, and separate Debian Release, RPM
metadata, RPM package, ALPM database, and ALPM package roles. An exact RPM
metalink can authenticate metadata but never substitutes for package-signing
authority.

The only dispositions are `importable`, `ambiguous`, and `unsupported`.
Implicit global trust, unpinned remote key sources, and an unbound ALPM keyring
are ambiguous. Disabled verification, permissive trust, optional package
signatures, missing or malformed key material, selector mismatches, and
selected-root escapes are unsupported. Neither status can silently become an
enabled persisted repository. This slice does not fetch, persist, enable, or
mutate; the pinned semantics and consumer boundary live in
[`docs/specs/native-repository-trust-import.md`](../specs/native-repository-trust-import.md).

Takeover consumes that preview plus explicit enrollment authority. Strict ALPM
declarations can proceed only when the sole ambiguity is the missing native
keyring binding and enrollment supplies exact pinned masters, certification
threshold, revocation input, and the same effective `SigLevel`. It never turns
an unsafe declaration or unrelated ambiguity into trust. The same takeover
path preserves enabled state, priority, parser inputs, projection bytes, and
follow-or-pin source policy for APT, DNF5, Zypper, and ALPM repositories.

## Native Source Identity And Update Policy

After declaration and trust-import planning, a native RPM, Debian, or ALPM
repository is enrolled with an exact source identity, a distinct repository
identity, a typed release/channel/rolling stream, and one closed update mode.
`RepositorySourcePolicy` is normalized in `repository_source_policies` and may
be scoped to one repository or shared by an exact repository group. Each
repository retains its own enabled state, priority, parser selectors, content
URL, ownership, authenticated snapshot, and optional member pin.

`crates/conary-core/src/db/models/repository/source.rs` owns repository
validation and persistence;
`crates/conary-core/src/db/models/repository/source/policy.rs` owns the typed
policy and schema-revision-31 stream binding. The binding commits to the exact
source/repository/stream identity, endpoints, parser configuration, and
top-level metadata authority. Refresh recalculates it before network access,
so an endpoint, parser selector, or metadata trust-root change requires
explicit `repo add --replace` re-enrollment.

Native parsers return package metadata together with the SHA-256 of the exact
authenticated top-level bytes: ALPM's served compressed database, Debian's
verified cleartext Release payload, or RPM's authenticated `repomd.xml`.
`follow` admits a new authenticated identity only within the unchanged stream
binding. `pin` additionally requires equality with the repository member's
persisted digest. Admission and package-row replacement share one SQLite
transaction, so refusal preserves the prior rows, snapshot, and `last_sync`.

Public feed profiles remain presets and conformance fixtures, not native
source identity. Native enrollment therefore works for uncatalogued
third-party repositories without adding a profile. The complete persisted,
CLI, hard-cut, and recovery contract is
[`docs/specs/native-source-identity-policy.md`](../specs/native-source-identity-policy.md).

## Native Repository Authenticity

Native source selection begins only after one typed trust contract authenticates
the repository grammar. `RepositoryTrustPolicy` in
`crates/conary-core/src/repository/trust.rs` is the persisted authority.
`trust/openpgp.rs` is a thin role-dispatch hub over three owners:
`trust/openpgp/store.rs` (trust-store layout and key-source transport),
`trust/openpgp/pinned.rs` (Debian/rpm-md/RPM pinned-certificate verification),
and `trust/openpgp/arch/` (pacman keyring grammar, trust snapshot, and ALPM
signature semantics). Parser construction, sync, package download, CLI repository
creation, Remi's admin API, and the hosted repository manifest all consume that
same tagged contract. There is no signature-check boolean, permissive mode,
runtime disable command, key lookup by short ID, or guessed key URL.

The supported chains are:

- Debian: a pinned Release certificate verifies the clearsigned `InRelease`,
  or the exact `Release` plus `Release.gpg` fallback. The signed Release
  SHA-256 and size authenticate the selected compressed `Packages` index; each
  package stanza's exact SHA-256 and size authenticate its `.deb`.
- RPM: repository metadata and packages have distinct authorities. Either a
  detached OpenPGP signature or an exact HTTPS metalink identity authenticates
  `repomd.xml`; its SHA-256 and size authenticate `primary.xml` and, by the
  same discipline, `filelists.xml`; primary metadata supplies exact package
  relations and generator-selected file providers and authenticates the RPM
  bytes; filelists supplies complete package file ownership; and an
  independently pinned package certificate verifies the RPM's embedded OpenPGP
  signature.
- Arch: one exact keyring source supplies the pinned master certificates,
  their certifications, packager certificates, and the companion
  `<keyring>-revoked` disabled-key list. An explicit threshold says how many
  distinct pinned masters must certify a packager key; the hosted Arch feeds
  require three of the current five masters, matching GnuPG's default
  `--marginals-needed`. `SigLevel` is represented directly: package signatures
  are required, database signatures are required or optional, and any
  signature that is present must be valid under `TrustedOnly`. `Never`,
  `TrustAll`, and optional package signatures are not representable.

### Arch Trust Snapshots

ALPM signature trust is not a pinned-certificate check, so it does not use the
same verifier as Debian and RPM. `ArchTrustSnapshot`
(`trust/openpgp/arch/snapshot.rs`) is the typed equivalent of a keyring
populated by `pacman-key --populate`: pinned master authority, certified
packager certificates with the exact certifying master set, subkey bindings,
revocation and expiry, disabled-key state, and one explicit trust-snapshot
time. `trust/openpgp/arch/verify.rs` evaluates a package or database signature
against that snapshot and reports pacman's own result classes
(`alpm_sigstatus_t` and `alpm_sigvalidity_t`); acceptance reproduces
`_alpm_check_pgp_helper` under `TrustedOnly`.

Two reference times are kept apart, because GnuPG keeps them apart:

- The **trust-snapshot time** (wall-clock now) decides certificate and subkey
  binding, key flags, revocation, and expiry. GnuPG builds the effective key
  from the *latest* self-signature (`g10/getkey.c:2914`, `:3427`) and compares
  expiry against `curtime` (`:3220`, `:3472`).
- The **signature creation time** is used only for the relations GnuPG ties to
  it (`g10/sig-check.c:363-435`): the signing key may not be newer than the
  signature, and the signature may not have expired.

Sequoia's streaming verifier instead binds keys at the signature's own
creation time. Because `archlinux-keyring` exports one self-signature per
component, every package signed before a packager's most recent self-signature
refresh looked unbound under that model while pacman accepted it. The typed
Arch verifier exists to remove that disagreement without loosening any check.

`crates/conary-core/tests/fixtures/arch/` holds a bounded real-package corpus
(the pinned `alpine-keyring` fixture plus packages from four more packager
keys, including a signing-subkey signer) and a bounded `archlinux-keyring`
ALPM package with the five pinned masters. Native pacman/GnuPG is the
conformance oracle for those recorded result classes; it is never invoked at
runtime or during tests.

The prepared Arch trust store is `<keyring-dir>/native/<repo>/arch/` holding
`certificates.pgp` plus `revoked`. It is disposable cache state: repositories
prepared before this layout must be re-prepared (`conary repo sync`, or Remi's
source prewarm), which refetches the keyring and rewrites both objects
together.

`crates/conary-core/src/repository/parsers/{debian,fedora,arch}.rs` owns the
three metadata chains. Fedora metalink identity parsing is isolated in
`parsers/fedora/metalink.rs`. `repository/download.rs` owns the terminal
package checks. Missing keys, a missing required signature, an unsupported
hash, duplicate authority records, invalid certification thresholds, or any
identity mismatch fail closed and remove a partially downloaded package.

The Arch repository parser and direct package parser share
`repository/package_relation.rs` as the ALPM relation authority. Runtime
dependencies and provisions are parsed through the official typed
`RelationOrSoname` grammar. Soname v1 and v2 values remain complete atomic
`Soname` identities; Conary never guesses a package-version boundary inside
them. Ordinary package relations retain their exact ALPM comparison semantics.

RPM requirement grammar is owned by
`repository/rpm_dependency.rs`. Its parser state is derived from RPM
`a8f0192aee1c08bd1454ed2ac6ebaf506004b55c`
[`rpmrichParseInternal()` and `rpmrichParseForTag()`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmds.cc#L1309-L1421),
not from dependency text or a package list. It accepts RPM's exact comparison
spellings and canonicalizes aliases before typed version evaluation; enforces
the same-operator chain and tag-context rules; and constructs repeated `with`
from the right side as RPM does. The source-pinned Fedora 44 corpus under
`crates/conary-core/tests/fixtures/rpm/` proves real repository expressions
without granting those packages exceptional behavior.

RPM-MD prerequisite authority is part of that same signed dependency record.
Pinned `createrepo_c`
[`cr_xml_dump_primary()`](https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump_primary.c#L80-L128)
emits `pre="1"` only for a Requires entry whose RPM dependency is an install
prerequisite. The Fedora parser preserves that marker as the typed
`PreDepends` relation; it does not infer order from a capability or package
name. Direct RPM parsing projects the corresponding header sense flags through
the same relation kind.

RPM file authority is derived from `createrepo_c`
`5cf41fe5d703901d78078ed18c67ab667e446c1a`: its
[`cr_xml_dump_files()`](https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump.c#L175-L225)
selects and emits package-owned `<file>` records in `primary.xml`, and its
[`STATE_FILE` parser](https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_parser_primary.c#L428-L445)
reads them independently of `rpm:provides`. Conary preserves every such signed
record as an unversioned typed file provider with `source-derived-file`
provenance on the exact package. The projection is owned by
`repository/parsers/fedora/provides.rs`. It does not reimplement createrepo's
path-selection rule or infer providers from package names, payload guesses, or
a curated path list. The provenance names the source format only: the
capability is the path, so a path-owning provenance never restates it. At this
scale that is not a stylistic point -- a duplicated path is one extra copy of
every package-owned path in the distribution, in both the synced database and
every converted artifact's signed capability list.

That selection rule is a filter, so `primary.xml` is not complete file
ownership: the same generator writes every owned path through the same
`<file>` writer into `filelists.xml` with the filter off
([`cr_xml_dump_filelists()`](https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump_filelists.c)).
Conary reads both documents. `repository/parsers/fedora/repomd.rs` admits the
`primary` and `filelists` records with one size plus SHA-256 discipline;
`repository/parsers/fedora/files.rs` owns the one `<file>` record grammar both
parsers read, so a path one document admits is never a path the other rejects;
`repository/parsers/fedora/filelists.rs` folds each `<package>` record into the
package `primary.xml` published, joined on `pkgid` (the package SHA-256), and
projects each path through the same `provides.rs` owner. A path both documents
carry becomes exactly one capability. The join is total: both signed documents
must name the same package set and agree on name, architecture, and EVR.
Directory and `%ghost` records are owned paths in both documents, so the
`type` attribute does not gate the projection.

A repository that publishes no `filelists` record is refused at sync when any
package carries a positive dependency that cannot be satisfied at all under
that filter. An RPM dependency name beginning with `/` is a dependency on a
path, so a path outside the filtered primary set has no possible repository
provider. The decision evaluates the requirement's typed boolean expression,
never its flattened alternatives: every unprovidable path atom is false, every
other atom may hold, and no atom is guaranteed to hold, so a conditional stays
satisfiable through its else branch. A group is refused only when the
expression cannot hold under that optimistic assignment, which refuses
`(cap and /unprovidable)` while admitting `(cap or /unprovidable)` and
`(/unprovidable if cap)`. Repeated atoms are evaluated independently, which can
only cost a warranted refusal and never invent one. The typed refusal names the
repository, the package, the paths, and the missing document instead of
surfacing later as an anonymous solver conflict. When filelists is published, a
missing path is an ordinary unsatisfied dependency.

Fedora 44 `Everything/x86_64` measures that cost exactly (repomd revision
1776864872): 76,354 packages, 81,720 file providers from `primary.xml`, and
9,416,909 more from `filelists.xml`, taking `repository_provides` from 657,873
rows to 10,074,782 and the synced SQLite database from 0.61 GB to 3.90 GB. The
829 MB decompressed document is never materialized: it is streamed from the
verified compressed bytes under the ceiling the signed `<open-size>` declares.

Dropping the duplicated path from file provenance is measured on that same
corpus and revision: the synced database falls from 4.78 GB to 3.90 GB
(-18.3%), the persist phase from 446.3 s to 392.3 s (-12.1%), and peak process
RSS from 6.33 GiB to 5.15 GiB, with row counts identical. What remains is not
provenance: at 4.78 GB the `repository_provides` table held 2,332 MB against
1,684 MB of index (869 MB for `(kind, capability)`, 815 MB for `(capability)`,
127 MB for the package key), so every path is still stored once in the table
and twice more in indexes.

Index consolidation is the lever that answered. `capability` was the second
column of the `(kind, capability)` composite, so that index could never serve a
capability-only seek -- which is what every provider lookup issues -- and only
the single statement that also filters `kind` could use it. Schema revision 28
retires it. Measured on the same corpus and revision, with row counts identical
at 10,074,782: the synced database falls from 3,903,258,624 to 2,991,742,976
bytes (-23.4%), and the difference is exactly the 911,515,648 bytes the retired
index occupied, with the table (1,570,963,456), the capability index
(854,863,872), and the package key (133,562,368) byte-identical across the
pair. Peak process RSS is unchanged (0.2% across six runs): an index that is
not read during a sync costs disk, not resident memory.

The kind-filtering statement now seeks the capability index and filters the
rows one capability holds. No statement in the inventory falls back to a table
scan, before or after. The corpus bounds that filter exactly: its most-provided
capability is `/usr/lib/.build-id` at 20,494 providers, and 9,498,629 of the
10,074,782 rows are `file`, so the worst measured latency change is +6.7 ms on
a hot capability whose kind does not match, while an ordinary package lookup is
unchanged at 0.014 ms.

Persist-phase timing is deliberately not quoted for this change. Five
same-binary baseline syncs on this host ranged from 450.5 s to 952.4 s under
concurrent build load, a spread far larger than any delta worth claiming, so
the persist effect of maintaining one fewer index across 10M inserts remains
unpriced rather than estimated.

The contract was derived against the package managers' and repository
generators' own documentation:

- [APT archive authentication](https://manpages.debian.org/bookworm/apt/apt-secure.8.en.html)
  and the [Debian repository format](https://wiki.debian.org/DebianRepository/Format)
- [DNF `gpgcheck` and `repo_gpgcheck`](https://dnf.readthedocs.io/en/stable/conf_ref.html),
  [RPM signatures and digests](https://rpm.org/docs/6.0.x/manual/signatures_digests.html),
  and the [createrepo_c repomd API](https://rpm-software-management.github.io/createrepo_c/c/group__repomd.html)
- [`pacman.conf` SigLevel](https://man.archlinux.org/man/pacman.conf.5.en),
  [ALPM repository database signatures](https://man.archlinux.org/man/extra/alpm-repo-db/alpm-repo-db.7.en),
  [Arch's master-key trust model](https://archlinux.org/master-keys/), and the
  [`archlinux-keyring` file list](https://archlinux.org/packages/core/any/archlinux-keyring/files/)
- pacman `a6f7467d`
  [`lib/libalpm/signing.c`](https://gitlab.archlinux.org/pacman/pacman/-/blob/a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d/lib/libalpm/signing.c#L521-L719)
  for the GPGME status/validity mapping and
  [`scripts/pacman-key.sh.in`](https://gitlab.archlinux.org/pacman/pacman/-/blob/a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d/scripts/pacman-key.sh.in)
  for keyring population, local signing, ownertrust, and disabled keys
- GnuPG `gnupg-2.4.9` `g10/getkey.c`, `g10/sig-check.c`, and `g10/trustdb.c`
  for the reference-time, expiry, revocation, and web-of-trust semantics that
  GPGME reports back to pacman

## Eligibility vs Ranking

Conary keeps eligibility and ranking separate:

- Eligibility decides whether a candidate may participate at all.
- Ranking decides which candidate wins among already-eligible candidates.

Eligibility inputs include:

- root request scope (`--repo`, `--from`)
- mixing policy (`strict`, `guarded`, `permissive`)

Strict mixing admits only the exact transaction source identity supplied by an
explicit source/repository scope or selected repository-backed root package.
The policy-explicit SAT API rejects strict dependency solving when that
identity has not been established; it never treats an absent identity as an
empty candidate set and never derives
one from a package name, repository label, URL, or package format. Local files
with repository dependencies therefore require an explicit scope. A
declarative multi-root package set has no privileged root: its only implicit
authority is the one exact source identity common to every root's
version-compatible repository candidates. Disjoint or ambiguous identity sets
fail closed and require an explicit source scope; model order never selects
source authority. Guarded mixing prefers an established identity and may fall
back; permissive mixing does not add a source
preference. An exact installed
package-name or declared capability provider that satisfies a transitive
requirement is favored before repository ranking, so dependency planning does
not reinstall its repository copy. An explicit root with an exact package-name
candidate retains that root identity instead of being replaced by a differently
named installed virtual provider. Once eligibility and installed-provider
preference are fixed, repository priority is authoritative. Candidates at the
same priority are compared only with their source ecosystem's native version
scheme. Equal-priority candidates from different schemes, or candidates whose
exact native identity remains tied, produce a typed ambiguity. Repository
names, cache iteration order, and external discovery metadata never break that
tie.

Explicit version constraints remain strict and scheme-aware. Cross-distro
identity mapping helps find equivalent packages; it does not replace native
version constraint semantics.

## Native Lifecycle Source Identity

Source policy decides which package artifact may satisfy a request. Once an RPM,
Debian, or Arch artifact is selected, its typed package-manager lifecycle ABI is
part of that artifact's identity and is not reinterpreted by repository source
identity or a target distribution label.

The lifecycle bundle records the exact source format, distro, release,
architecture, and native version scheme. Packages with source lifecycle
programs carry those programs and their exact source ABI into the Conary-owned
runtime; packages without them carry no invented hooks. Source selection
cannot change either fact.

Install, update, replatform, and model apply all pass selected lifecycle bundles
to the same typed transaction planner. The planner derives slots, argv, trigger
matches, order, and payload visibility from package metadata and transaction
state. There are no operator replay flags, mixing-policy overrides, shallow
target matrices, or command-text exceptions. If the selected root cannot
satisfy a typed interpreter or source-manager semantic, preflight reports that
semantic before mutation so the missing model can be engineered.

Model apply resolves every install and update root together, authenticates the
exact SAT-selected repository closure, and executes that closure as one batch.
Typed package relations therefore order provider payloads and dependent
lifecycle programs inside one package-set transition; model diff iteration
order is never transaction authority.

## Dependency Acquisition Boundary

`repository/dependencies.rs` prepares the complete selected closure before any
package acquisition starts. Repository rows, native package trust, Remi route
identity, and Remi signing policy are resolved serially through the
caller-owned SQLite connection into immutable per-package plans. The network,
signature/checksum verification, and CAS portion then runs with at most eight
packages in flight and never owns or shares that connection.

Acquisition completion order is not transaction order. The downloader drains
every bounded task, restores the resolver-selected input order, and aggregates
attributable failures in that same order before returning. Install, update,
model apply, and replatform consumers therefore receive the identical package
and lifecycle sequence they received from serial acquisition; payload mutation,
database commit, lifecycle execution, and generation publication remain outside
the concurrent boundary.

## Flow Behavior

### Native Package Adoption

Native repository takeover is a separate authority transition from package
adoption. `conary system repository-takeover --dry-run --root <selected-root>
--manifest <enrollment.json>` serializes the complete declaration, trust,
identity, policy, and projection plan without mutation. Apply requires that
exact preview SHA-256 and `--yes`; rollback restores the persisted prior
projection bytes and removes only takeover-owned repository rows. Native
declaration paths remain byte-identical compatibility projections, and any
missing or changed path is reported as drift rather than becoming new
authority. Legacy APT declarations without `Signed-By` can proceed only when
the enrollment manifest binds exact primary fingerprints to native global
keyring bytes below the selected root; the keyring files then join the owned,
rollback-safe projection set. The exact contract is
[`native-repository-takeover.md`](../specs/native-repository-takeover.md).

After takeover, repository declarations and trust roots installed by a package
participate in the package's selected-root and SQLite transaction. CCS carries
the signed desired-state intent; direct native installation derives the same
intent from exact package payload grammars before mutation. Update, removal,
explicit retention, shared ownership, and rollback follow
[`package-repository-enrollment.md`](../specs/package-repository-enrollment.md).
The completed filesystem is never scanned for repository-looking files.

`conary system adopt <pkg> --dry-run` builds the same package-specific plan
that apply consumes. The preview opens an existing current-schema database
read-only and never migrates it. Native discovery captures one typed
`InstalledInventorySnapshot` for the selected RPM, dpkg, pacman, or eopkg
authority. Each backend uses a fixed number of manager processes and bulk
database traversals independent of package count. The snapshot owns exact
identities, versions, architectures, install reasons, files, dependencies,
provides, configuration/lifecycle evidence, the complete path-to-owner map,
and a deterministic token over every mutation-relevant field.

Each requested package is classified as ready, already tracked, missing,
ambiguous, unsupported, or blocked by a tracked-file conflict. Shared
directories keep their existing owner instead of being reassigned. `--full`
also validates that every planned regular file or symlink is readable for CAS
capture, but preview never creates CAS state.

Adoption preserves migration continuity for a machine that already has native
package-manager state. It is not Conary's foreign-package acquisition path and
does not prove source-independent cross-distro conversion or lifecycle
execution.

`conary system adopt <pkg> --convert` is the explicit bridge from that
migration state to a portable CCS artifact. It selects the adopted trove by
exact name/version/architecture, requires its persisted native identity and
complete `AdoptedFull` payload ownership (`conary system adopt <pkg> --full`
establishes it), and resolves one exact package row from an enabled,
stream-bound enrolled native repository. The artifact is acquired into the
Conary cache under its typed authenticated source-digest authority; eopkg
retains the index's SHA-1 while conversion separately computes the internal
SHA-256 artifact identity. Cached RPM, Debian, Arch, and eopkg artifacts are
reverified with their ecosystem-specific package authority before reuse. The
parser identity and every adopted payload node,
resolved owner, content digest, symlink, hardlink topology edge, and explicit
directory must match before the canonical native converter runs.

Conversion does not transfer ownership of the currently adopted trove. It
publishes a separately installable signed CCS under the Conary runtime root and
records that path only after immediate strict verification. Same-directory
staging plus one SQLite transaction ensures a failed conversion or persistence
step leaves neither a newly published artifact nor an applied changeset. A
later install, update, rollback, or remove consumes that CCS through Conary's
ordinary source-independent transaction path without consulting the source
package manager or its database.

System-wide adoption preserves the native manager's explicit-versus-dependency
reason for every installed package. On RPM systems, Conary uses each
generation's documented frontend authority. DNF5's
`repoquery --userinstalled` is itself an installed-only selector, while DNF4
composes `repoquery --installed --userinstalled`. Configured excludes are
disabled through the corresponding generation's documented option. RPM hosts
without DNF use libzypp's exact `/var/lib/zypp/AutoInstalled` `SolvIdentFile`;
each listed package ident is dependency-installed and every absent installed
ident is explicit. A missing authority or malformed result is a typed failure;
Conary never treats every RPM as explicitly installed.

Apply consumes that plan, rechecks file ownership inside its database
transaction, and reacquires the complete native inventory immediately before
commit. Any package, version, ownership, reason, relation, configuration, or
lifecycle token change aborts the transaction before handoff. Only an unchanged
snapshot may write checkpoints, package metadata, CAS objects, changesets, and
the state snapshot. Track and full adoption both preserve native
package-manager authority until an explicit takeover or selected-generation
handoff. Takeover consumes the same snapshot from planning through CAS upgrade,
revalidates it before transfer, and removes complete RPM, dpkg, pacman, or
eopkg identity sets through one database-only batch transaction where the
manager supports it.

Complete, unfiltered `conary system adopt --system --full` scans the selected
root exactly once. Package claims bind to that one path-indexed capture rather
than rereading their declared file lists. The scanner reports paths examined,
unique regular inodes, and content streams; every hardlinked inode is read into
CAS once, and its global typed topology is retained even when different native
packages own different links. Shared directories retain every exact package
owner, and unowned content is classified during the same traversal. The scan
captures each walked node through a bounded pointwise-stable descriptor window,
then derives global hardlink topology from the captured snapshots. It
uses the same finite publication domains as generation construction: `/etc` is
config state, `/var` and `/srv` are mutable state, and other retained top-level
paths are immutable generation input. `/proc`, `/sys`, `/dev`, `/run`, `/tmp`,
`/home`, `/root`, `/mnt`, and `/media` are the exact ephemeral/API/device/user
domains and are never captured. Conary's own runtime root and database
directory are explicit normalized exclusions under both their lexical and
resolved paths, so path aliases cannot make the CAS or database inputs to their
own capture. A runtime root resolving to `/` fails closed.

Persisted package file anchors and payload claims partition that exact scan.
Package-owned paths retain their owner and claim graph while their materialized
node/content authority is reconciled to the one global scan, preserving xattrs
and hardlinks even when an inode group crosses ownership boundaries. Every
remaining path belongs to one synthetic `CapturedRoot` trove. That source is a
generation input but never substitutes for package install, update, remove, or
native-manager authority. A repeated full adoption recognizes the same exact
partition without accumulating another captured-root owner. Track-only native
troves are privately CAS-backed and promoted to `AdoptedFull` in the same
full-adoption transaction.

The synthetic trove uses the fixed Conary-authored identity
`conary-live-root=0.0.0-captured-root`; both its trove version and exact package
provide are validated under Conary's SemVer grammar before persistence.
Pre-alpha databases containing the retired invalid `snapshot` version are
disposable authority: rebuild the database and repeat full adoption. There is
no compatibility adapter for that contradictory typed state.

Package filters and `--explicit-only` deliberately do not assert whole-root
continuity: they adopt only their requested native package scope. If any exact
native query, payload capture, or package persistence fails, full-system
adoption reports an incomplete outcome and does not classify the missing
package's paths as unowned captured-root state. Complete capture also fails
closed if a metadata-only `AdoptedTrack` identity is absent from the current
native inventory, because retaining that stale non-generation owner would
silently omit its paths.

### Install

Install uses the shared effective policy and then layers root-only request
scope such as `--repo` or `--from` on top of it. `--from` accepts only an
exact lexically valid source identity and does not consult the named feed
catalog. A named feed ID may additionally enable canonical-name projection;
unknown identities use exact package names. Exact-name selection
and SAT ordering both respect the effective source-selection settings.

### Update

Update now also loads the shared effective policy.

Ordinary update is exact-source only: it may select a newer native version from
the installed repository or exact public profile, but it never infers a distro
migration. Repology status cannot switch the source. Moving an installation
from one distro profile to another is an explicit replatform plan with separate
preview, exact target rows, and confirmation.

Update also enforces the limited-preview ownership boundary:

- Conary-owned updates always delegate to the same selected-root transaction
  used by `conary install`. When no current generation exists, that transaction
  materializes authoritative DB/CAS state and publishes the first generation;
  it never mutates the host root in place.
- Adopted packages keep native package-manager authority under the default
  `satisfy`/`adopt` behavior. Update reports the skip with native-PM guidance
  instead of silently replacing the package.
- `--ownership takeover` is the explicit package-level ownership crossing.
  Package names do not create special takeover exceptions; compatibility and
  dependency proof come from the selected package and typed provider graph.
- `--security` only proceeds when each requested Conary-owned update source is
  marked as publishing supported advisory metadata. Sources marked `unknown` or
  `unsupported` cause a refusal before mutation, so security-only output never
  implies completeness for a source that cannot answer advisory questions.

### Model Diff / Apply

Model files do not own global source-selection state. Model diff/apply operate
on packages, ownership convergence, and explicit replatform actions;
repository enrollment and request-scoped selection remain source authority.

`model apply --dry-run` resolves the complete incoming package set, downloads
and authenticates its exact artifacts, and runs the same read-only batch
ordering and relation planning that apply consumes. It stops before
selected-root materialization, selected-root-relative payload normalization,
lifecycle execution, CAS storage, or database mutation, and returns the same
typed ordering error apply would return for an invalid prepared transaction.

### Replatform Planning

`model/replatform.rs` uses the shared source-selection and package-selection
logic to find visible realignment targets and build a
`ReplatformExecutionPlan`.

Measured `SystemAffinity` contributes only diagnostic package counts. The
proposals and executable transactions are derived independently from an
explicit target source identity, authenticated repository metadata, exact
package identity, dependency constraints, architecture compatibility, and
available install routes. Affinity percentages never choose or rank a mutation
candidate.

Each transaction tracks:

- target repository metadata
- exact-version install route availability
- architecture compatibility
- unresolved target dependencies
- whether remove, install, and metadata legs are ready

This makes the plan useful both for preview output and for deciding which
replatform replacements can execute immediately.

## Operator Entry Points

The main CLI entry points are:

```bash
conary distro list
conary distro info
```

`conary distro list` shows named feed presets, not a destination support list.
`conary distro info` shows measured source-affinity data. Both commands are
informational; source choice requires enrolled repository authority and an
explicit scoped install, update, or replatform operation.

## Where To Read Next

- [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md) for the workspace-level module map
- [`docs/llms/subsystem-map.md`](../llms/subsystem-map.md) for assistant-facing entry points
- `crates/conary-core/src/repository/effective_policy.rs` for runtime policy loading
- `crates/conary-core/src/repository/trust.rs` and
  `repository/trust/openpgp.rs` for native repository authority
- `crates/conary-core/src/repository/declarations/` and
  `docs/specs/native-repository-declarations.md` for lossless selected-root
  declaration discovery, then
  `docs/specs/native-repository-trust-import.md` for fail-closed trust planning
  before enrollment
- `crates/conary-core/src/repository/parsers/` and
  `repository/download.rs` for authenticated metadata and package intake
- `crates/conary-core/src/repository/supported_profiles/` for configured feed
  IDs, route slugs, parser flavor, and source version scheme
- `crates/conary-core/src/canonical/exchange.rs`,
  `canonical/rules.rs`, and `db/models/canonical.rs` for exact canonical-map
  exchange, local contract parsing, and typed persistence authority
- `crates/conary-core/src/model/parser/source_policy.rs` for `[system]` parsing
  and precedence; `model/parser.rs` owns the aggregate model and validation
- `apps/conary/src/commands/install/source_policy.rs` for install request-scope
  policy construction and canonical package name resolution
- `apps/conary/src/commands/adopt/packages.rs` for shared single-package
  adoption preview/apply planning and native-authority preservation
- `apps/conary/src/commands/adopt/system.rs` and
  `adopt/system/captured_root.rs` for complete system adoption, track-to-full
  promotion, and exact package-versus-captured-root partitioning
- `crates/conary-core/src/db/models/trove/identity.rs` for version, release,
  Debian Multi-Arch, and exact native-identity validation at trove persistence
  and decode boundaries
- `crates/conary-core/src/generation/root_manifest/scan.rs` for finite
  selected-root capture and normalized runtime exclusions
- `crates/conary-core/src/generation/builder/runtime_inputs.rs` for installed
  package and captured-root projection into immutable and mutable manifests
- `apps/conary/src/commands/update/mod.rs` for update module routing
- `apps/conary/src/commands/update/package.rs` for single-package update
  execution, delta/full update handling, and lifecycle execution preflight
- `apps/conary/src/commands/update/resolution.rs` for exact-source update selection
  preview and replatform update context
- `apps/conary/src/commands/update/selection.rs` for exact-source update
  candidate behavior
- `apps/conary/src/commands/update/adopted_authority.rs` for adopted-update
  native-authority policy
- `apps/conary/src/commands/update/collection.rs` for `update @collection`
  orchestration, member filtering, and per-member update dispatch
- `apps/conary/src/commands/model.rs` for the model command hub
- `apps/conary/src/commands/model/context.rs` for model loading and diff
  enrichment
- `apps/conary/src/commands/model/presentation.rs` for ownership-convergence and
  explicit replatform summaries
- `apps/conary/src/commands/model/apply.rs` for model apply execution and
  replatform install dispatch
- `apps/conary/src/commands/model/apply/packages.rs` for model package-set
  aggregation and atomic install/update dispatch
- `apps/conary/src/commands/model/apply/derived.rs` for persisted
  derived-package definition and build ownership
- `apps/conary/src/commands/install/package_set.rs` for multi-root SAT
  selection and `apps/conary/src/commands/install/repository_batch.rs` for
  authenticated preparation of the exact selected closure
- `apps/conary/src/commands/model/remote_diff.rs` and
  `apps/conary/src/commands/model/lock.rs` for remote include drift and
  lockfile behavior
- `crates/conary-core/src/model/replatform.rs` for executable replatform planning
