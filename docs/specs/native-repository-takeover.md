---
last_updated: 2026-08-12
revision: 5
summary: Define deterministic native repository takeover, exact APT and Zypper global trust binding, owned projections, drift detection, and rollback
---

# Native Repository Takeover And Projections

## Status And Boundary

Issue #381 established the fourth bounded W10 slice. Issue #384 extended its
model conformance boundary; issue #396 owns execution on authentic Linux Mint
and Pop!_OS roots, while issue #398 extends that proof to CachyOS and openSUSE
Tumbleweed. Takeover consumes lossless selected-root
declarations, their trust-import dispositions, and an explicit versioned
enrollment manifest. It produces a complete preview before mutation and can
then make Conary the sole repository authority while preserving the native
manager's repository files as deterministic compatibility projections.

Discovery evidence is not enrollment authority. Source identity, repository
identity, stream policy, parser selectors, authenticated pins, and target
architecture cannot be inferred from distribution names, paths, URLs, or
repository aliases. The enrollment manifest supplies those exact values and
binds each one to a lossless `NativeRepositoryReference` emitted by discovery.

## Preview Contract

The takeover preview is versioned, serializable, and deterministic. It records:

- the selected root and exact declaration provenance;
- every native repository's enabled state, trust disposition, source
  locations, priority, filters, and ownership transition;
- the exact Conary repository rows, source policies, parser inputs, trust
  roots, follow-or-pin policy, and authenticated snapshot input to persist;
- every projected path with its exact SHA-256 and ownership transition; and
- closed blockers, including ambiguous or unsupported enabled trust,
  dynamically generated libzypp service repositories, missing or duplicate
  enrollment bindings, conflicting existing Conary authority, and projection
  drift.

For ALPM only, an enrollment manifest may resolve the single typed
`AlpmKeyringBindingMissing` ambiguity by supplying Conary's exact Arch keyring
policy: keyring format, pinned master fingerprints, certification threshold,
revoked-key authority, and an effective `SigLevel` identical to the discovered
native declaration. This cannot override optional package signatures,
`TrustAll`, `Never`, another ambiguity, or any unsupported finding.

Disabled repositories remain in the preview and projection universe. Their
unresolved trust does not enable mutation authority, but their enrollment may
not be silently enabled. Every enabled declaration must have exactly one
enrollment binding, and one declaration may expand into multiple exact Conary
repository rows when the native grammar declares a typed product such as APT
URI, suite, and component combinations. Coverage validation derives that
product set from the parsed declaration, binds each manifest row through its
exact Debian parser inputs and semantically identical base URI, and blocks both
missing and duplicate products. A manifest cannot establish completeness merely
by naming a declaration once.

Preview reads the current-schema database and selected root only. It does not
write SQLite, stage files, invoke a native package manager, fetch metadata or
keys, or inspect the live host when another selected root was requested.
The CLI opens SQLite through an immutable read-only connection. If the database
has uncheckpointed WAL frames, preview fails closed instead of creating SQLite
sidecars or ignoring newer authority in the WAL.

The CLI emits a versioned preview envelope containing both the complete typed
preview and its exact SHA-256. Apply consumes that typed digest; diagnostic
field rendering is not mutation or aggregation authority.

APT declarations without `Signed-By` retain a typed
`ImplicitGlobalAuthority` ambiguity. An enrollment manifest may resolve only
that ambiguity by binding exact primary certificate fingerprints to regular
native keyring files at `/etc/apt/trusted.gpg` or directly under
`/etc/apt/trusted.gpg.d/`. The file URL must resolve below the selected root,
the named certificate must exist in those exact bytes, and every other trust
finding remains non-overridable. This preserves legacy distro declarations
without treating the whole global keyring or a filename as repository trust.

Zypper repositories may omit per-repository `gpgcheck`, `repo_gpgcheck`,
`pkg_gpgcheck`, and `gpgkey` because
[libzypp's canonical configuration](https://github.com/openSUSE/libzypp/blob/master/zypp.conf)
documents globally enabled signature checking and per-repository overrides. An enrollment manifest may resolve only the
resulting `MissingRequiredAuthority` findings when `/etc/zypp/zypp.conf` is
absent, both RPM roles name the same exact primary fingerprints, and every file
URL resolves directly below `/usr/lib/rpm/gnupg/keys/` in the selected root.
Any explicit global override, disabled verification, different ambiguity, key
outside that native store, or missing certificate remains a blocker. This
models Tumbleweed's native authority without inferring trust from its distro
name or repository aliases.

## Apply And Projection Ownership

Apply consumes the exact preview digest. Before mutation it repeats discovery
and planning and refuses if the serialized preview changed. Projection content
on first takeover is the exact UTF-8 declaration content already admitted by
the lossless grammar. Selected-root key files referenced by importable trust
evidence become owned projections too. APT embedded OpenPGP blocks remain in
their exact declaration projection and are persisted as bounded exact
`application/pgp-keys` data authority for Conary's pinned certificate loader.
Exact native global-keyring files admitted by an explicit APT or Zypper binding
also become owned projections.
Projection content is persisted beside its SHA-256 and prior path state;
subsequent operations render from that persisted Conary authority rather than
scraping the filesystem into a second owner.

Projection writes are staged in the destination directory, flushed, and
atomically exchanged with the destination. Conary verifies the digest and mode
of the displaced inode after the exchange; a mismatch is exchanged back before
returning an error. If a later exchange or database operation fails, every
earlier exchange is rolled back from its exact displaced bytes and mode before
the database transaction is abandoned. The transaction persists repository
authority, projection content and digests, and takeover membership. Existing
projection state is checked before staging: a missing path, digest mismatch, or
mode mismatch is reported with its exact guest path and expected and observed
state. Drift is never accepted as a new declaration.

Takeover-created repository rows use the closed `native-projection` ownership
value. Apply refuses to overwrite an operator- or Remi-owned row. While a row
belongs to a takeover, schema authority rejects generic repository-definition
updates (including enablement, priority, parser, trust, endpoints, and source
policy); authenticated snapshot and last-sync observations may still advance.
Definition changes require a projection-enrollment transaction. A repeated apply
with the same preview is an idempotent verification; it creates no new
repository, policy, membership, or projection rows.

## Rollback

Rollback is scoped to one persisted takeover. It first verifies that every
owned projection still matches Conary's expected digest. It then stages the
recorded pre-takeover bytes (or a removal for paths that were absent), removes
only the takeover's `native-projection` repository rows and policy membership
inside one SQLite transaction, atomically restores the prior path state, and
deletes the takeover record. Conflicting ownership or projection drift blocks
rollback without changing SQLite or the selected root.

This slice never deletes or mutates a native package-manager database. The
native manager retains its normal enabled repository universe and update
behavior because it continues reading the same declaration paths and bytes;
only their mutation authority changes.

## Schema Hard Cut

Schema revision 32 introduced `native-projection` repository ownership plus normalized
takeover, membership, and projection-state tables. Projection paths are unique
within the current Conary database, content and prior bytes are stored as BLOBs,
and SHA-256 values are lowercase exact-length values. Schema revision 33 retains
that authority while adding package enrollment. Current revision 36 retains
the same takeover authority; every earlier database is retired pre-alpha
state. Recovery is:

```bash
conary system rebuild-db --discard-state --yes
```

followed by a new preview and apply from authoritative declarations, trust, and
enrollment input. There is no compatibility migration or legacy reader.

## Proof

Focused proof must cover deterministic JSON and unknown-field rejection,
complete enabled-declaration enrollment, ambiguous and unsupported trust
blocking, selected-root confinement, staged write failure, database conflict,
repeat apply, exact drift reporting, and rollback restoration of both database
authority and prior projection bytes. The adoption interaction gate, workspace
Clippy, formatting, current-schema rejection/rebuild proof, and documentation
truth checks remain required.

The model conformance corpus covers APT, ALPM, and Zypper derivative shapes.
Authentic Linux Mint, Pop!_OS, CachyOS, and openSUSE Tumbleweed execution is a
separate target gate: it must retain release-owned declaration and signing-root
bytes, exercise typed preview/apply/repeat/rollback, and must not use a
distro-name selector in product code. CachyOS additionally binds its CachyOS
and Arch ALPM keyrings through exact master fingerprints and certification
thresholds. A configured lane is not verified evidence until its exact-head
hosted result passes.
