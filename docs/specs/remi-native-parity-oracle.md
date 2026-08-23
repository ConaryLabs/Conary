---
title: Remi native full-catalog parity oracle
summary: Define the strict content-addressed artifact that independently proves native package facts across one complete immutable profile candidate
revision: 1
status: draft
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

## Separate Slice 6 owners

This artifact covers catalog facts and member precedence. Dependency closure
and the exact unresolved-dependency set require resolver-owned comparison
against native solver output. Complete conversion crawling, conversion-proof
reuse, independent CCS reopen and target preflight, and final Remi promotion
after durable object reopen remain separate authority boundaries under #517.
