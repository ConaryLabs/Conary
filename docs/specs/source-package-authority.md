---
last_updated: 2026-08-03
revision: 3
summary: Define lossless RPM, Debian, ALPM, and CCS package authority plus explicit consumer projections
---

# Source Package Authority And Consumer Projections

## Status And Scope

This specification is the design authority for roadmap workstream W5 and
[issue #108](https://github.com/ConaryLabs/Conary/issues/108). It defines the
hard-cut target implemented by issues
[#104](https://github.com/ConaryLabs/Conary/issues/104) and
[#105](https://github.com/ConaryLabs/Conary/issues/105). Issues #104 and #105
have shipped the source-specific identity, provision, and configuration
records, CCS v3 provenance, resolver distinction, provider and config
persistence, and transaction projections. W5 is one complete hard cut.

RPM, Debian, and ALPM identity and declared provisions now live in their
format-specific authority modules behind `SourcePackageAuthority`.
`PackageFormat::resolution_capabilities()` and
`PackageFormat::config_declarations()` are explicit fallible consumer
projections. `ForeignConversionInput` is scoped to conversion; native parsers
retain format-specific authority directly. The retired `PackageMetadata`,
`ConfigFileInfo`, and `config_files()` adapters do not remain.

This specification owns package identity, dependency/provision authority,
payload and configuration declarations, and their consumer boundaries.
[`foreign-package-lifecycle-contracts.md`](foreign-package-lifecycle-contracts.md)
continues to own lifecycle event order, arguments, configuration transactions,
and selected-root execution. [`ccs-format-v3.md`](ccs-format-v3.md) describes
the current signed CCS v3 envelope.

## Governing Rule

> Normalize at the boundary of one named consumer, never while initially
> parsing source authority.

An RPM fact remains an RPM fact, a Debian fact remains a Debian fact, and an
ALPM fact remains an ALPM fact until a fallible consumer projection asks for a
specific target contract. Similar spelling does not establish shared
semantics. In particular:

- exact package identity is not an entry in a package's declared-provision
  list;
- a configuration declaration is not necessarily a materialized payload node;
- a source path is not a host `std::path::Path`;
- a payload record, a hardlink-set member, and the effective installed inode
  are distinct facts; and
- diagnostic or repository-discovery data is never mutation, publication, or
  compatibility authority.

No projection may silently discard a fact it needs, fill a missing fact from a
filename or package name, or preserve an unrepresentable source semantic as an
untyped string. It either produces the named consumer contract or returns a
typed projection error before persistence, signing, publication, resolution,
or mutation.

## Authority Layers

The package path on the build host, authenticated repository location, mirror
URL, and download checksum identify transport and provenance. They are not
package semantics. After trust verification, parsing produces one closed
source-format sum:

```text
SourcePackageAuthority
  +-- Rpm(RpmPackageAuthority)
  +-- Debian(DebianPackageAuthority)
  +-- Alpm(AlpmPackageAuthority)
  +-- Ccs(CcsPackageAuthority)
```

The outer enum provides format dispatch only. It has no common `provides` or
`config_files` fields and no lowest-common-denominator constructor. Each
variant owns its exact source identity, relations, declarations, lifecycle,
payload evidence, and source-path spelling. Shared payload primitives such as
`PayloadNode`, `PayloadContentAuthority`, and reopenable content streams remain
shared because they describe the same installed filesystem facts. Shared
source-package fields do not imply a shared source ontology.

`SourcePathBytes` preserves archive spelling and `DeploymentPath` is the
fallible, traversal-checked materialization projection. Source parsing never
passes untrusted archive bytes through a host path parser or lossy Unicode
conversion. A future non-UTF-8 persistence contract must be an explicit signed
and database schema decision; it cannot be introduced through diagnostics.

## RPM Authority

The RPM model is pinned to upstream RPM commit
[`a8f0192aee1c08bd1454ed2ac6ebaf506004b55c`](https://github.com/rpm-software-management/rpm/tree/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c)
and the corresponding [tag grammar](https://rpm.org/docs/latest/manual/tags).
The format-specific model preserves:

- exact NEVRA identity, including whether the epoch was absent, version,
  release, and architecture;
- each aligned dependency-tag record with its native sense flags, EVR,
  architecture/color context, header index, and relation family;
- every explicit `PROVIDENAME`/`PROVIDEVERSION`/`PROVIDEFLAGS` record as a
  declared capability, separate from NEVRA identity and from RPM runtime
  features;
- each file-header record, source path, file flags, verify flags, node
  metadata, payload association, and hardlink device/inode membership;
- the content-bearing hardlink member and effective inode projection described
  by `rpmfilesBuildNLink`, `rpmfiArchiveHasContent`, and `fsmMkfile` without
  erasing the source records that established it;
- `%config`, `%config(noreplace)`, `%ghost`, and `%missingok` as independent
  source flags attached to their exact file record; and
- the exact RPM lifecycle ABI owned by the lifecycle specification.

RPM tag arrays that must align are validated as one indexed record set. A
missing mate, impossible flag combination, conflicting payload association, or
unsupported relation is a typed RPM parse error. The parser never synthesizes
a self-provider entry: package-name satisfaction is derived from NEVRA by the
resolution projection.

## Debian Authority

The Debian binary-package model is pinned to
[Debian Policy 4.7.4.1 package relationships](https://www.debian.org/doc/debian-policy/ch-relationships.html),
the [binary control-field contract](https://www.debian.org/doc/debian-policy/ch-controlfields.html),
the [configuration-file contract](https://www.debian.org/doc/debian-policy/ch-files.html#configuration-files),
and dpkg's pinned dependency implementation at commit
[`7004a048f4b122c133f1b08661be1399ce0a4dd7`](https://git.dpkg.org/cgit/dpkg/dpkg.git/tree/lib/dpkg/depcon.c?id=7004a048f4b122c133f1b08661be1399ce0a4dd7).
The format-specific model preserves:

- exact `Package`, `Version`, `Architecture`, and `Multi-Arch` identity fields;
- each relationship field as its own native family, retaining alternatives,
  qualifiers, version relation, version boundary, and source order;
- every explicit `Provides` atom as a virtual capability, including its
  optional exact-equality version and architecture qualifier, separate from
  concrete package identity;
- each data-tar payload record and exact source path;
- each `DEBIAN/conffiles` declaration as a declaration record with its exact
  absolute path and optional `remove-on-upgrade` flag; and
- the exact maintainer-script, trigger, and control-artifact ABI owned by the
  lifecycle specification.

An unflagged conffile declaration normally refers to one incoming payload
node. A `remove-on-upgrade` declaration must not have incoming payload.
Contradictions fail as typed Debian source-authority errors. The declaration is
preserved separately from the matched node so validation and transaction
planning can distinguish declaration evidence, incoming bytes, installed old
state, and removal policy.

## ALPM Authority

The ALPM model is pinned to pacman commit
[`a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d`](https://gitlab.archlinux.org/pacman/pacman/-/tree/a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d),
the versioned [PKGINFO grammar](https://man.archlinux.org/man/PKGINFO.5.en), and
the [PKGBUILD provision and backup contracts](https://man.archlinux.org/man/PKGBUILD.5.en).
The format-specific model preserves:

- exact `pkgname`, full `pkgver`, `arch`, and package-type identity;
- each `depend`, `optdepend`, `conflict`, `replaces`, and `provides` assignment
  as an ordered, typed ALPM relation;
- every explicit `provides` assignment as a virtual or package capability,
  even when it deliberately reuses `pkgname` with a compatibility version;
- each archive payload record, libarchive xattr, source path, and exact node;
- every `backup` assignment as an ordered relative source declaration,
  separately from payload matching and the installed three-hash state; and
- `.INSTALL` functions and ALPM hooks under the lifecycle specification.

A `backup` declaration does not create a payload node. The payload projection
records whether an exact archive entry matches it; an unmatched declaration is
retained as signed source evidence and produces no invented file. Update,
remove, and rollback consume the declaration together with old, current, and
new content identities according to pacman's documented three-hash behavior.

## CCS Authority

CCS is already a consumer contract, not another foreign-source normalization
step. Direct CCS authoring produces CCS authority from explicit author input;
foreign conversion produces it only through the CCS projection below.

W5 establishes one current-only signed authority epoch in which:

- `identity` is the sole exact package identity;
- `capabilities` contains author-declared, source-declared, or source-derived
  capabilities with an explicit provenance tag;
- exact identity is never duplicated inside `capabilities`;
- source configuration declarations are represented separately from matched
  materialized config nodes and installed config state; and
- payload, lifecycle, relations, configuration declarations, and capability
  provenance participate in signed content identity.

The implementation hard cut promotes CCS v3 as the sole signed package
authority and deletes v2 reading, writing, fixtures, projections, and fallback
paths in the same W5 sequence. No release may be cut between the #104 and #105
sibling slices: together they establish the complete v3 identity/capability
and configuration shape. Existing v2 artifacts are rebuilt or reconverted;
they are not adapted.

Capability provenance is a closed enum with at least these roles:

| Role | Meaning |
|---|---|
| `author-declared` | Explicit capability in a directly authored CCS package |
| `source-declared` | Exact RPM, Debian, or ALPM provision with source-format identity and source-record position |
| `source-derived-file` | Capability derived by one pinned source-format rule from an exact payload record |

Package-name satisfaction comes directly from signed `identity`. It is not a
fourth capability role. A duplicate or contradictory identity, an unknown
provenance role, or a provenance/source-format mismatch fails signed-authority
validation.

## Consumer Projections

### Dependency Resolution

The resolution projection consumes exact source identity, requirements,
relations, explicit provisions, source version algebra, and architecture
semantics. It emits the repository/SAT model only after tagging each emitted
provider with its origin. It derives one exact package-name match from identity
without adding it to the source-declared provision list.

It may omit payload bytes, xattrs, configuration declarations, lifecycle
bodies, build-only diagnostics, and transport provenance because none decide a
SAT match. It may not omit relation alternatives, ordering strength,
architecture qualifiers, version scheme, provider relation, or capability
origin.

Failures are typed `ResolutionProjectionError` variants, including malformed
source relation, unrepresentable architecture qualifier, unsupported version
relation, and contradictory identity. They retain source format and record
location; message text is diagnostic only.

### CCS Authoring And Conversion

The CCS projection consumes the complete authenticated source model plus its
reopenable payload. It must represent exact identity, payload nodes and
content, relations, capability provenance, configuration declarations,
lifecycle ABI, and conversion provenance. This projection has no permitted
semantic loss. Source-only transport details may be omitted only after their
authenticated digest and conversion boundary are recorded where required.

Failures are typed `CcsProjectionError` variants, including
`UnrepresentableIdentity`, `UnrepresentableCapability`,
`UnrepresentableConfigDeclaration`, `PayloadDeclarationMismatch`,
`MissingLifecycleAuthority`, and `BudgetExceeded`. A failure stops before
signing or Remi publication. Package-name, path, or distro exceptions are not
valid recovery.

### Native Transaction Planning

The transaction projection consumes verified CCS authority, installed old
state, selected-root facts, and the target capability inventory. It derives
install, update, remove, rollback, trigger, and configuration events. It may
omit source build metadata, repository fetch hints, and diagnostic evidence;
it may not omit source lifecycle entries, capability origin needed for
resolution or persistence, payload ownership, config declarations, old/new
content identities, source-format order, or recovery actions.

Failures are typed `NativeTransactionProjectionError` variants, including
missing installed authority, unsupported source event, config state
contradiction, payload ownership conflict, and missing target capability. They
stop preflight before mutation. They never become a warning, manual-review
queue, source-package-manager fallback, or best-effort transaction.

## Ownership And Module Boundaries

The implementation decomposition is ownership-based:

- `packages/rpm/authority.rs`, `packages/deb/authority.rs`, and
  `packages/arch/authority.rs` own source-format identity, relations,
  provisions, and config declarations. Existing parser hubs retain dispatch
  and re-exports only.
- `packages/source_authority.rs` owns the closed format-dispatch enum. It does
  not own shared flattened fields.
- `packages/payload.rs`, `payload.rs`, and `filesystem/source_path.rs` retain
  their existing shared payload-stream, node, and byte-exact path ownership.
- `repository/dependency_model.rs` remains a resolution-consumer DTO. Native
  parsers stop constructing it as their source authority.
- `ccs/convert/source_projection/` owns the explicit RPM, Debian, and ALPM to
  CCS projections; `convert/converter.rs` remains orchestration only.
- the current signed CCS schema directory is replaced by `ccs/v3/`, with
  identity/capability validation and configuration-declaration validation in
  separate focused children.
- native transaction and config transaction modules consume verified CCS
  authority; they do not parse source packages or recover declarations from
  payload paths.

New business logic does not go into the existing parser, converter, package,
or validation hubs. This keeps the affected files below the 1,000-line
planning gate and makes each source ontology independently reviewable.

## Hard-Cut Deletion And Replacement Map

The W5 hard cut deleted these ambiguous surfaces:

| Retired surface | Current replacement |
|---|---|
| `packages/common.rs::PackageMetadata` and its `packages::PackageMetadata` re-export | Format-specific authority structs plus closed `SourcePackageAuthority` dispatch |
| `packages/traits.rs::ProvidedCapability` and `PackageFormat::provides()` | Per-format declared-provision records; resolution and CCS projection DTOs exist only at their consumer boundaries |
| `packages/traits.rs::ConfigFileInfo` and `PackageFormat::config_files()` | Per-format config declaration records plus separate matched payload/config transaction authority |
| `RpmPackage::{extract_provides, extract_config_files}` | RPM authority parsing in `packages/rpm/authority.rs` |
| `DebPackage::{convert_provides, parse_conffiles}` | Debian authority parsing in `packages/deb/authority.rs` |
| `ArchPackage::parse_provides` and the backup-to-`ConfigFileInfo` loop | ALPM authority parsing and explicit backup matching in `packages/arch/authority.rs` |
| `ccs/convert/converter.rs::signed_native_provides` | Format-specific CCS capability projection with provenance |
| `ccs/package/v2_projection.rs::{provides_from_v2_authority, config_files_from_v2_authority}` | Verified v3 install and transaction projections that preserve the authority distinction |
| v2 validation's name-equality self-provider inference | Identity validation plus independent capability-provenance validation |
| CCS v2 reader, writer, fixtures, and format routing | Sole current CCS v3 contract; v2 artifacts are rejected and rebuilt |

Callers such as batch install, conversion preparation, installed-authority
snapshots, `provides` persistence, and config persistence move to the named
consumer DTOs. Renaming the old structs or retaining adapters beside the new
model does not satisfy this deletion gate.

## Persistence And Rebuild Impact

The signed CCS contract and installed database both change intentionally.

- CCS v3 replaces v2; every local, cached, static-repository, and Remi v2
  artifact must be rebuilt or reconverted from authenticated source.
- SQLite schema revision 24 replaces the current schema in place.
  Installed and repository provider rows must distinguish identity-derived
  matches from declared/derived capabilities, and config persistence must
  distinguish source declarations from materialized node and transaction
  state.
- Existing pre-W5 databases are disposable and must be recreated with
  `conary system init` plus repository resync/reinstallation. No migration,
  compatibility decoder, dual write, or fallback reader is added.
- Remi conversion records and indexes tied to v2 authority are rebuilt before
  public serving. A mixed v2/v3 serving state is invalid.

The rebuild property is intentional: revision 24 has no migration or dual-read
path from the retired provider representation.

## Conformance And Closeout

The implementation issues prove the property across formats, not only the two
reported packages:

- the pinned ASP.NET ALPM fixtures retain exact package identity and their
  same-name compatibility provision, and resolution distinguishes both;
- the pinned `bash-completion` ALPM fixture retains its unmatched backup
  declaration without inventing a payload node;
- RPM fixtures distinguish NEVRA identity, explicit provides, config/ghost
  flags, and effective hardlink inode authority;
- Debian fixtures distinguish concrete identity, versioned virtual provides,
  ordinary conffiles, and `remove-on-upgrade` declarations;
- each consumer projection has contract tests for every source variant and
  every typed refusal; and
- stale searches prove the flattened structs, helpers, adapters, v2 schema,
  and compatibility routes are absent.

Focused package parser, resolution, CCS signing/reopen, native transaction,
Remi conversion/publication, strict workspace Clippy/format, and documentation
truth gates all pass before W5 closes.
