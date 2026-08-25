---
title: Remi native full-catalog parity oracle
summary: Define strict content-addressed artifacts for independent native package facts and dependency resolution across one complete immutable profile candidate
last_updated: 2026-08-25
revision: 5
status: active
---

# Remi native full-catalog parity oracle

## Boundary

The native full-catalog parity oracle is independent release evidence for one
exact `ProfileRevisionV2`. It is distinct from the hosted
`phase4-native-pm-parity` suite: that suite proves deterministic one-package
lifecycle and CLI behavior, while this contract covers every package admitted
to one immutable profile candidate.

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
native display text alone cannot establish parity. Exact-identity duplicates
obey profile precedence only when every projected fact agrees. A contradictory
duplicate fails the complete crawl. The private SQLite spool, canonical bundle
write, and independent complete reopen use the same bounded contract as the
ALPM producer.

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

Schema 1 fixes the resolution policy rather than accepting solver flags or
free-form policy: the installed state is empty, every exact package variant is
requested as its exact root, only required and pre-required groups enter the
positive solve, optional and build groups are excluded, and provider choice
uses native repository precedence. A different policy requires a schema
change.

There is exactly one canonical row for every package key in the bound package
oracle. A row records exactly one outcome:

- `resolved`, with a strictly ordered duplicate-free closure of exact package
  keys that includes the root; or
- `unresolved`, with a strictly ordered duplicate-free set of requiring
  package keys and canonical required-group digests.

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
dependency-closure, or unresolved-dependency drift. Diagnostic strings and
native solver error prose never establish the result.

ALPM resolution evidence is produced by the same explicit
`native-alpm-oracle` feature and pinned libalpm runtime as the package-fact
oracle. The resolver helper independently reopens the supplied package bundle,
then reproduces that entire package oracle from the authenticated database
objects and requires exact manifest equality before solving. For each exact
package row, it prepares a database-only libalpm transaction against an empty
local database with the target architecture and profile databases registered
in precedence order. Prepared transaction packages become exact closure keys;
typed libalpm missing-dependency records become exact requiring-package keys
and canonical required-group digests. Invalid architecture, conflicting
transactions, ambiguous identities, unbound requirements, and unexpected
native error classes fail the complete crawl.

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

Every package-oracle key binds through a private disk-backed index to one exact
native solvable root. Weak relations are disabled. Successful transaction IDs
become exact closure package keys. Typed libsolv problem-rule and dependency
IDs become exact requiring-package keys and canonical required or pre-required
group digests. A typed missing file requirement triggers an exact lookup in
libsolv's independently reopened complete filelists and one re-solve before it
may remain unresolved. Architecture rejection, conflicts, non-installable
roots, unexpected rule classes, native identity ambiguity, and input or oracle
drift fail the complete crawl. Diagnostic strings never establish an outcome.

Invoke the resolver helper with the exact RPM package bundle produced above:

```bash
cargo run -p conary-core --features native-rpm-oracle \
  --bin conary-rpm-resolution-oracle -- \
  --profile-manifest profile.json \
  --source-snapshot fedora-source.json \
  --primary primary.xml.gz --filelists filelists.xml.zst \
  --package-oracle rpm-oracle \
  --architecture x86_64 \
  --output rpm-resolution-oracle
```

Success has the same complete per-root and independent reopen meaning as the
ALPM producer. Neither native helper reads Conary catalog rows or invokes a
source package manager executable.

## Separate Slice 6 owners

The shared resolver artifact and comparator do not produce native evidence.
ALPM and RPM now have pinned native solver helpers. Debian now has pinned
package-fact production; Debian solver production remains a separate
resolver-owned boundary. Complete conversion crawling, conversion-proof reuse,
independent CCS reopen and target preflight, and final Remi promotion after
durable object reopen remain separate authority boundaries under #517.
