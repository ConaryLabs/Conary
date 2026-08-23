---
title: Remi native full-catalog parity oracle
summary: Define the strict content-addressed artifact that independently proves native package facts across one complete immutable profile candidate
last_updated: 2026-08-23
revision: 2
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

## Separate Slice 6 owners

This artifact covers catalog facts and member precedence. Dependency closure
and the exact unresolved-dependency set require resolver-owned comparison
against native solver output. Complete conversion crawling, conversion-proof
reuse, independent CCS reopen and target preflight, and final Remi promotion
after durable object reopen remain separate authority boundaries under #517.
