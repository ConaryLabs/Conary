---
last_updated: 2026-07-29
revision: 21
summary: Document exact profile-owned source policy, Remi CCS package authority, canonical map authority, native repository authority, package identity, full-adoption root continuity, and lifecycle handoff
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
system.toml [system]
  |
  +-- SystemConfig
  |     allowed_distros
  |     pin / distro / mixing
  |     convergence
  |
  +-- model apply mirrors explicit runtime state
          |
          +-- DistroPin table
          |     current source pin + mixing policy
          |
          +-- settings table
                source.allowed-distros
                       |
                       v
              load_effective_policy()
                       |
                       v
              EffectiveSourcePolicy
                       |
                       +-- ResolutionPolicy eligibility
                       +-- root install / SAT ordering / update / replatform
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `SystemConfig` | `model/parser/source_policy.rs` | Model-layer source-policy config from `[system]` |
| `SourcePinConfig` | `model/parser/source_policy.rs` | Explicit source pin plus source strength |
| `ConvergenceIntent` | `model/parser/source_policy.rs` | How aggressively Conary should move packages toward Conary-managed state |
| `DependencyMixingPolicy` | `repository/resolution_policy.rs` | Closed `strict`/`guarded`/`permissive` dependency-mixing contract |
| `ResolutionPolicy` | `repository/resolution_policy.rs` | Exact request scope, dependency mixing, and source allowlist used by the resolver |
| `EffectiveSourcePolicy` | `repository/effective_policy.rs` | Runtime policy assembled from DB state with one exact transaction profile |
| `SystemAffinity` | `db/models/distro_pin.rs` | Informational installed-provenance measurement used for display and replatform estimates |
| `ReplatformExecutionPlan` | `model/replatform.rs` | Executable and blocked replatform transactions derived from planned replacements |

## Model Inputs

The user-facing source-policy surface lives under `[system]` in `system.toml`.

Important fields:

- `allowed_distros`: allowlist of exact supported public profile IDs Conary may
  use when selecting packages.
- `pin`: the source-pin contract, with an exact supported public profile ID
  plus typed dependency-mixing strength.
- `convergence`: how aggressively package ownership should move during source
  transitions.

The only mixing values are `strict`, `guarded`, and `permissive`. Model parsing
rejects unknown profile IDs, profile route aliases, unknown mixing values, and
unknown source-pin fields. The CLI parses the same typed mixing enum before
changing state, and the current schema independently constrains both persisted
profile IDs and mixing values. No invalid value silently becomes a default.

The removed flat `[system].distro` and `[system].mixing` aliases are rejected;
write `[system.pin]` directly. The former `profile` and `selection_mode`
surfaces are also rejected: after external ranking signals were removed from
mutation authority, both fields became decorative aliases for the same exact
selection algorithm.

## Runtime Mirrors

Conary persists the runtime source-policy mirror in SQLite:

- `DistroPin`: one exact supported public profile ID plus a typed mixing policy
- `settings["source.allowed-distros"]`: JSON-encoded allowlist

`load_effective_policy()` merges those tables into one `EffectiveSourcePolicy`
and carries the exact pinned profile into `ResolutionPolicy` for strict or
guarded mixing. Package format is never a substitute for that profile: Fedora
and openSUSE packages may both use RPM while remaining distinct source
authorities. Strict policy rejects an unprofiled transitive candidate instead
of inferring authority from its repository name or version scheme. A corrupt
persisted mixing value is a read error, not a fallback policy.

`SystemAffinity` is a measured summary of exact installed-package provenance.
It is deliberately not an input to eligibility, candidate ranking, canonical
equivalence, update selection, or any other mutation decision. It exists for
informational display and to estimate how many installed packages a requested
replatform may need to realign.

The repository feed catalog makes
`crates/conary-core/src/repository/supported_profiles/` the source of truth for
configured feed IDs, package format, version scheme, and Remi route-family
mapping. Fedora 44, Ubuntu 26.04, and Arch are the currently configured public
feeds, not the only destination systems Conary supports. Internal route slugs
such as `fedora` and `ubuntu` are not feed IDs. The
`repo add --source-profile` surface accepts only those exact public IDs and
requires the declared profile's package format to match the repository parser.
Remi
sync and package fetches translate the stored public ID to the profile-owned
route slug. Remi sync pages through the sparse name index and incrementally
stages each fixed-size `include=versions` page of resolution-only
version/provide/requirement documents and trusted package-advisory metadata;
one final transaction publishes the completed local candidate over the
previous offline-resolution snapshot. The page set is not yet pinned to one
server revision; `docs/modules/remi.md` records that boundary and its tracked
lease/revision fix. Sync never fetches the whole-distribution metadata
document, retains a distribution-sized package vector, or issues one HTTP
request per package. Persisted package identity requires the exact public ID;
route slugs and generic `rpm`/`deb`/`arch` format labels are never accepted as
source-identity aliases. Native repository sync writes the repository's exact
profile into every package row and rejects a missing or conflicting profile.
The superseded
`data/distros.toml` catalog was deleted in M4d.

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
  `repomd.xml`; its SHA-256 and size authenticate `primary.xml`; primary
  metadata supplies exact package relations and generator-selected file
  providers and authenticates the RPM bytes; and an independently pinned
  package certificate verifies the RPM's embedded OpenPGP signature.
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

RPM primary-file authority is derived from `createrepo_c`
`5cf41fe5d703901d78078ed18c67ab667e446c1a`: its
[`cr_xml_dump_files()`](https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump.c#L175-L225)
selects and emits package-owned `<file>` records in `primary.xml`, and its
[`STATE_FILE` parser](https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_parser_primary.c#L428-L445)
reads them independently of `rpm:provides`. Conary preserves every such signed
record as an unversioned typed file provider on the exact package. It does not
reimplement createrepo's path-selection rule or infer providers from package
names, payload guesses, or a curated path list.

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
- explicit allowlist from `allowed_distros`

Strict mixing admits only the exact transaction profile supplied by an
explicit source scope, persisted system pin, or the exact profile carried by
the selected repository-backed root package. The policy-explicit SAT API
rejects strict dependency solving when that profile has not been established;
it never treats an empty profile as an empty candidate set and never derives
one from a package name, repository label, URL, or package format. Local files
with repository dependencies therefore require an explicit scope or system
pin. Guarded mixing prefers an established profile and may fall back;
permissive mixing does not add a profile preference. An exact installed
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
part of that artifact's identity and is not reinterpreted by `DistroPin`
mixing policy.

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

## Flow Behavior

### Native Package Adoption

`conary system adopt <pkg> --dry-run` builds the same package-specific plan
that apply consumes. The preview opens an existing current-schema database
read-only and never migrates it. Native discovery resolves the exact installed
package identity, version, and architecture, then inventories files,
dependencies, provides, existing Conary tracking, and tracked-file ownership.

Each requested package is classified as ready, already tracked, missing,
ambiguous, unsupported, or blocked by a tracked-file conflict. Shared
directories keep their existing owner instead of being reassigned. `--full`
also validates that every planned regular file or symlink is readable for CAS
capture, but preview never creates CAS state.

Adoption preserves migration continuity for a machine that already has native
package-manager state. It is not Conary's foreign-package acquisition path and
does not prove source-independent cross-distro conversion or lifecycle
execution.

System-wide adoption preserves the native manager's explicit-versus-dependency
reason for every installed package. On RPM systems, Conary uses each
generation's documented DNF contract rather than reading DNF's internal state:
DNF5's `repoquery --userinstalled` is itself an installed-only selector, while
DNF4 composes `repoquery --installed --userinstalled`. Configured excludes are
disabled through the corresponding generation's documented option. A missing
manager or malformed result is a typed failure; Conary never treats every RPM
as explicitly installed.

Apply consumes that plan, rechecks file ownership inside its database
transaction, and only then writes checkpoints, package metadata, CAS objects,
changesets, and the state snapshot. Track and full adoption both preserve
native package-manager authority until an explicit takeover or
selected-generation handoff.

Complete, unfiltered `conary system adopt --system --full` additionally scans
the selected root once, after all native package payload captures succeed. The
scan captures each walked node through a bounded pointwise-stable descriptor
window, then derives global hardlink topology from the captured snapshots. It
uses the same finite publication domains as generation construction: `/etc` is
config state, `/var` and `/srv` are mutable state, and other retained top-level
paths are immutable generation input. `/proc`, `/sys`, `/dev`, `/run`, `/tmp`,
`/home`, `/root`, `/mnt`, and `/media` are the exact ephemeral/API/device/user
domains and are never captured. Conary's own runtime root and database
directory are explicit normalized exclusions under both their lexical and
resolved paths, so path aliases cannot make the CAS or database inputs to their
own capture. A runtime root resolving to `/` fails closed.

Persisted package file anchors and directory claims partition that exact scan.
Package-owned paths retain their owner and claim graph while their materialized
node/content authority is reconciled to the one global scan, preserving xattrs
and hardlinks even when an inode group crosses ownership boundaries. Every
remaining path belongs to one synthetic `CapturedRoot` trove. That source is a
generation input but never substitutes for package install, update, remove, or
native-manager authority. A repeated full adoption recognizes the same exact
partition without accumulating another captured-root owner. Track-only native
troves are privately CAS-backed and promoted to `AdoptedFull` in the same
full-adoption transaction.

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
exact public source-profile ID; Remi route slugs are not install aliases.
Exact-name selection
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

Model diff captures source-policy drift as structured actions such as:

- `SetSourcePin`
- `ClearSourcePin`
- `SetAllowedDistros`
- `ClearAllowedDistros`
- `ReplatformReplace`

Model apply persists source-policy changes first, then executes any
replatform transactions that are actually executable through the shared install
path. Blocked transactions remain visible in the rendered plan and in follow-up
warnings.

### Replatform Planning

`model/replatform.rs` uses the shared source-selection and package-selection
logic to find visible realignment targets and build a
`ReplatformExecutionPlan`.

Measured `SystemAffinity` contributes only the informational package-count
estimate. The proposals and executable transactions are derived independently
from the explicit target profile, authenticated repository metadata, exact
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
conary distro set fedora-44 --mixing guarded
conary distro mixing permissive
conary distro info
```

`conary distro info` shows the exact pin, typed mixing policy, and measured
source-affinity data. The affinity rows are informational; changing system
state requires an explicit scoped install, update, or replatform operation.

## Where To Read Next

- [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md) for the workspace-level module map
- [`docs/llms/subsystem-map.md`](../llms/subsystem-map.md) for assistant-facing entry points
- `crates/conary-core/src/repository/effective_policy.rs` for runtime policy loading
- `crates/conary-core/src/repository/trust.rs` and
  `repository/trust/openpgp.rs` for native repository authority
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
- `crates/conary-core/src/generation/root_manifest/scan.rs` for finite
  selected-root capture and normalized runtime exclusions
- `crates/conary-core/src/generation/builder/runtime_inputs.rs` for installed
  package and captured-root projection into immutable and mutable manifests
- `apps/conary/src/commands/update/mod.rs` for update module routing
- `apps/conary/src/commands/update/package.rs` for single-package update
  execution, delta/full update handling, and lifecycle execution preflight
- `apps/conary/src/commands/update/source_policy.rs` for source-policy update
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
- `apps/conary/src/commands/model/presentation.rs` for source-policy and
  replatform summaries
- `apps/conary/src/commands/model/apply.rs` for model apply execution and
  replatform install dispatch
- `apps/conary/src/commands/model/apply/derived.rs` for persisted
  derived-package definition and build ownership
- `apps/conary/src/commands/model/remote_diff.rs` and
  `apps/conary/src/commands/model/lock.rs` for remote include drift and
  lockfile behavior
- `crates/conary-core/src/model/replatform.rs` for executable replatform planning
