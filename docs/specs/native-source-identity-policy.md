---
last_updated: 2026-08-12
revision: 3
summary: Define exact native source and repository identity, shared follow or pin policy, stream binding, and authenticated snapshot persistence
---

# Native Source Identity And Update Policy

## Status And Scope

This specification owns the native repository identity and refresh contract for
W10. It begins after declaration discovery and trust-import planning have
produced an exact enabled repository declaration with an ecosystem-native trust
policy. It ends when authenticated native metadata is admitted or refused
before repository package rows are replaced.

Named public feed profiles remain configuration presets and conformance
fixtures. They are not repository identity, source identity, stream identity,
or refresh authority. Package-resolution removal of the remaining public
profile gates is a separate W10 slice.

## Persisted Owners

The current disposable schema owns three related records:

- `repository_source_policies` owns one exact source identity, typed ecosystem,
  version scheme, declared stream, and closed `follow` or `pin` update mode.
  A policy has an explicit repository or group scope. Multiple repository rows
  may reference only a group policy.
- `repositories` owns one exact repository identity, the policy reference, an
  immutable stream-binding SHA-256, and the last admitted authenticated
  snapshot SHA-256. Existing enabled state, priority, architecture filters,
  parser variables, content URL, trust, advisory support, and ownership remain
  repository properties.
- `repository_source_pins` owns the required authenticated snapshot SHA-256 for
  each repository identity in a pinned policy. A group pin therefore names one
  exact digest per member rather than pretending different repository metadata
  documents have one identity.

`repositories.name` remains the human-facing unique name used by `--repo`,
display, and existing resolver scopes. `repositories.repository_identity` is a
new distinct opaque identity column; no name, URL, or display surface is
reinterpreted as that identity. The model validates identities both before
write and after read, while SQLite constrains their length and surrounding
whitespace.

This contract applies only when the typed parser format is RPM, Debian, or
ALPM. Remi sparse, static TUF, and Conary JSON repositories have no native
source-policy reference, stream binding, or native authenticated snapshot in
this slice; those nullable columns remain absent for those rows and their sync
contracts are unchanged.

Repository and source identities are 1 to 255 printable ASCII characters with
no leading or trailing whitespace. They are opaque exact identifiers: code
does not infer them from a distro name, repository name, URL, parser format,
version scheme, or public feed profile.

The ecosystem is a closed `rpm`, `deb`, or `alpm` enum and must match the typed
repository parser. The version scheme is the existing closed `rpm`, `debian`,
or `arch` enum and must match that ecosystem. Unknown persisted enum values and
malformed identities fail the entire repository read.

## Stream Contract

A source policy declares exactly one stream:

- `release` names an independently supported release line;
- `channel` names a publisher-defined channel whose contents may advance; or
- `rolling` names a continuously advancing series.

The kind and opaque stream identifier are both required. They are descriptive
authoritative inputs, not strings from which runtime routing is guessed.

Each repository member stores a stream-binding SHA-256 calculated from the
schema-revision-31 canonical JSON encoding of its source identity, repository
identity, stream kind and identifier, metadata URL, content URL, parser
configuration, and only the authority that authenticates the top-level
snapshot: Debian Release keys, RPM metadata OpenPGP keys or exact metalink, or
the ALPM keyring and database-signature policy. RPM package-signing keys are a
separate payload authority and do not enter this binding. Refresh recalculates
the binding before network access and fails closed if it differs. Changing an
endpoint, parser selector, or snapshot trust root therefore requires explicit
policy re-enrollment; it cannot silently move a following repository to
another stream. Encoding changes require a schema hard cut.

The binding detects accidental configuration drift. It is not a trust anchor
or tamper-evident commitment because it is stored beside its own inputs.

## Authenticated Snapshot Identity

Every native parser returns packages together with the SHA-256 of the exact
top-level bytes admitted by its ecosystem trust chain:

- ALPM hashes the served compressed repository database bytes after applying
  the configured database-signature policy and before decompression.
- Debian hashes the verified cleartext `Release` payload obtained from either
  `InRelease` or `Release` plus `Release.gpg`; that payload authenticates the
  selected Packages index.
- RPM hashes the served `repomd.xml` bytes after its detached OpenPGP signature
  or exact metalink identity is verified and before XML parsing.

The hash algorithm is fixed by the current schema as SHA-256. A later algorithm
requires a schema hard cut rather than accepting an untyped algorithm string.
Parser or package data without this identity is not a native sync snapshot.

## Follow And Pin Decisions

The policy decision runs inside the same SQLite transaction that replaces
repository packages:

- `follow` admits any newly authenticated snapshot only when the recalculated
  stream binding equals the persisted binding, then records the new snapshot.
- `pin` additionally requires the candidate snapshot to equal the exact pin for
  that repository identity. A missing member pin is corruption and a different
  digest refuses the entire refresh.

Neither refusal changes package rows, requirements, provides, canonical links,
the previous observed snapshot, or `last_sync`. Equality with the previously
observed digest is valid for both modes and still permits deterministic
reconstruction of package rows.

Repository-scope policy insertion refuses a second repository member. Group
policy reuse succeeds only when source identity, ecosystem, version scheme,
stream, and update mode are exactly equal. A conflicting reuse fails before the
repository is inserted.

## CLI Enrollment

Non-static native `conary repo add` requires exact source policy input:

- `--source-id` and `--repository-id`;
- `--stream-kind release|channel|rolling` and `--stream-id`;
- exactly one of `--follow` or `--pin-snapshot-sha256`;
- optional `--policy-group` to share the policy with other repositories.

The parser selects the ecosystem and version scheme; a conflicting user
projection is not accepted. Without `--policy-group`, Conary creates a
repository-scope policy owned by that exact repository identity. With a group,
the exact group identifier is created or reused under the same source identity.

`--source-profile` may still select a configured feed preset or Remi route, but
native identity and refresh do not require it. When present it must still be an
exact configured public profile and is copied to package rows for the existing
resolver during this transitional W10 slice. When absent, native sync stores no
profile projection; the later distro-gate-removal slice moves resolution onto
source identity. The fixed three-profile SQLite CHECK is removed from
`repositories` and `repository_packages` now; source-profile validation is a
typed preset lookup rather than repository identity authority. Third-party
repositories use the same identity and refresh contract without being added to
a public profile catalog.

`--replace` becomes the explicit native re-enrollment surface as well as the
existing static trust-repin surface. Native replacement deletes and recreates
the named repository, policy membership, pins, and derived package rows in one
transaction after all new input validates. Initial pin values are authoritative
operator input obtained from the exact authenticated top-level bytes described
above; Conary never performs an unpinned first refresh and promotes the result
to a pin.

## Hard Cut And Recovery

This change introduced schema revision 31 and replaced
disposable pre-alpha state. There is no migration, compatibility reader,
implicit default, or adapter for repositories that lack an exact source
policy. Native repository takeover subsequently advances the current database
schema to revision 32 while retaining revision 31 as the stream-binding
encoding identity. Package repository enrollment advances the current database
schema to revision 33 without changing that binding grammar.

Recovery is `conary system rebuild-db --discard-state --yes`, followed by
re-enrollment from authoritative native declarations, imported trust roots,
exact source/repository identities, stream policy, and pins. Package rows and
observed snapshots are rebuilt only by an authenticated refresh.

## Required Proof

Focused proof covers:

- exact repository- and group-scope round trips and conflicting group reuse;
- rejection of unknown persisted values, invalid identities, mismatched parser
  ecosystems, and changed stream bindings;
- first-party and uncatalogued third-party enrollment;
- transactional follow advancement and pin mismatch rollback;
- parser snapshot identity for ALPM, Debian, and RPM trust chains.
- rejection of retired revisions plus documented rebuild recovery into the
  current schema;
- CLI flag exclusivity, native replacement, and group reuse behavior;
- stable revision-31 binding encoding and fail-closed encoding drift;
- pin refusal leaving package rows, canonical links, converted-package
  reconciliation, observed identity, and `last_sync` untouched;
- normal metadata expiry selecting a refresh without weakening pin admission.

All new ecosystem, scope, stream, and mode decoders use the fail-closed
persisted-value pattern. None may copy a decoder that maps unknown text to a
default enum variant.

The owning `conary-core` and `conary` suites, workspace Clippy, formatting,
feature interaction gate, and documentation truth checks remain required.
