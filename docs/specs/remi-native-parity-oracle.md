---
title: Remi native full-catalog parity oracle
summary: Define strict content-addressed artifacts for independent native package facts and dependency resolution across one complete immutable profile candidate
last_updated: 2026-08-23
revision: 4
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

The first producer boundary is ALPM. It is built only with the explicit
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

## Separate Slice 6 owners

The shared resolver artifact and comparator do not produce native evidence.
ALPM now has a pinned native solver helper. RPM and Debian solver helpers remain
separate owners and must derive their rows from the named native libraries over
the exact package universe. Complete conversion crawling, conversion-proof
reuse, independent CCS reopen and target preflight, and final Remi promotion
after durable object reopen remain separate authority boundaries under #517.
