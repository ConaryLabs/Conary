---
last_updated: 2026-07-27
revision: 4
summary: Canonical signed CCS v2 authority, archive, payload, and trust contract
---

# CCS Package Format v2

CCS v2 is Conary's sole native package format. There is no supported CCS v1
authoring, signing, parsing, installation, migration, or publication path.
Pre-alpha v1 artifacts must be rebuilt from source or discarded.

The Rust schema in `crates/conary-core/src/ccs/v2/schema.rs` is the executable
owner of individual fields. This document owns the stable archive, authority,
and trust boundaries between producers and consumers.

## Archive Grammar

A package is a gzip-compressed tar archive. Its accepted entries are:

| Path | Requirement | Authority |
|---|---|---|
| `MANIFEST` | exactly one regular file | canonical CBOR `AuthorityDocumentV2` |
| `MANIFEST.sig` | exactly one regular file for every usable package | Ed25519 signature over the exact `MANIFEST` bytes |
| `MANIFEST.toml` | optional regular file | diagnostic projection only; its digest must match signed authority when present |
| `MANIFEST.attestation.json` | optional regular file | build-attestation envelope bound by signed provenance |
| `MANIFEST.conversion-boundary.json` | optional regular file | foreign-conversion boundary bound by signed provenance |
| `components/<name>.json` | one per signed component | diagnostic payload index checked exactly against signed component summaries |
| `objects/<aa>/<62 lowercase hex>` | one per signed regular-file digest | content-addressed payload bytes |

The reader rejects duplicate authority, signatures, projections, components,
or objects; unknown files and directories; non-canonical object paths; archive
path escapes; oversized entries; extraction-limit overflow; object/hash
disagreement; and any entry type not admitted for its path.

Tar ordering, timestamps, ownership, and compression are transport details.
Writers emit deterministic ordering and normalized timestamps. They never
become package authority.

## Signed Authority

`MANIFEST` decodes as `AuthorityDocumentV2` with `format_version = 2`. It
contains:

- exact identity: name, version, version scheme, positive decimal CCS build
  release, optional platform, package kind, exact source-native architecture
  token, and required Debian `Multi-Arch` authority for Debian identities;
- one typed package, group, or redirect body whose tag agrees with identity;
- exact provides, requirements, relations, and component summaries;
- exact payload paths, node variants, content digests and sizes, component
  ownership, per-path config semantics (`noreplace`, `ghost`, and
  `remove_on_upgrade`), and conflict policy;
- source-independent declarative lifecycle plus any converted package's exact
  native lifecycle ABI;
- provenance identities and hashes for hermetic evidence, build attestations,
  and foreign conversion boundaries; and
- the optional diagnostic TOML digest.

Unknown or structurally inconsistent authority is rejected. Consumers do not
fill missing authority from filenames, component JSON, TOML, payload paths,
repository metadata, distro identity, script text, or defaults from an older
schema.

An authored package names exactly one non-empty default component. The
`components.default` manifest field is that single authority name, not a list
of components installed by default. Authoring lint rejects missing, blank, or
ambiguous default-component authority before payload construction.

The architecture token is never normalized while converting a native package:
Debian `all`, Arch `any`, and RPM `noarch` remain distinct signed values.
Compatibility interprets a token together with its `version_scheme`; no token
is a format-independent wildcard. The release remains part of exact identity
through repository selection, SAT output, installation, state snapshots,
rollback, and display, so two packages with the same name/version but different
release cannot collide or be re-resolved to one another.

Each provided capability signs its exact kind, name, version scheme,
architecture qualifier, and either no version authority or a paired typed
relation and version boundary. RPM may use all five ordered relations; Debian,
Arch, and Conary providers may use only equality. The package self-provider is
exact equality with the signed package version. Package version is never a
fallback for a missing provider version. Requirements, relations, and this
complete provider authority all participate in CCS content identity.

An ordinary config declaration identifies exactly one signed regular-file or
symlink payload whose `FileAuthorityV2.config` repeats the same semantics.
RPM ghost config and Debian remove-on-upgrade declarations are package
authority without incoming payload and require the corresponding signed native
source contract. Verified package construction projects these exact
declarations into the package transaction interface.

`ConflictPolicyV2::Replace` and `PackagePolicyV2.allow_host_mutation = true`
are rejected until typed transaction consumers implement them. Signed fields
cannot be accepted as inert metadata.

## Payload Authority

Every signed file uses the shared `PayloadNode` contract in
`crates/conary-core/src/payload.rs`. A regular file has exactly one SHA-256 and
byte length. Non-regular variants carry their own exact data and do not carry
regular-file content authority.

For regular files, the archive contains exactly one object at the canonical
path derived from the lowercase digest. Verification recomputes every digest
and size, rejects missing or unreferenced objects, and proves that every
component projection has the signed name, file count, and byte total.

`MANIFEST.toml` and `components/*.json` are readable projections. A projection
may help inspection, but it can neither add nor replace install behavior.

## Signatures And Trust

`MANIFEST.sig` is JSON containing the algorithm, signature, public key, and
optional key ID and timestamp. The algorithm is Ed25519. The signature covers
the exact archived `MANIFEST` bytes.

Structure inspection returns an explicitly untrusted value. Install, update,
restore, try, export, self-update, conversion intake, and publication require
a `VerifiedCcsArchive` produced by complete verification against an explicit
`TrustPolicy`. A filename extension, valid signature from an unknown key,
repository checksum, or successful structural parse never grants that
capability.

Repository installs use the package-authority keys established by that
repository's verified trust chain. Local authoring uses an explicit
user-local development key and policy. Release publication uses its accepted
release signer set and attestation policy. An empty trust set, missing
signature, untrusted key, invalid signature, or required timestamp failure is
fatal.

The signing command accepts only a structurally valid v2 archive, replaces the
signature in a staged copy, verifies the complete authority and payload under
the new key, and atomically publishes the result only after that proof passes.

## Lifecycle Authority

Declarative lifecycle is signed source-independent intent: users, groups,
directories, services, systemd unit enablement, tmpfiles, sysctl,
alternatives, and sandboxed target-root scripts with explicit capabilities.

Converted RPM, Debian, and Arch packages additionally carry their exact typed
source lifecycle ABI in signed authority. Source format owns event semantics;
the destination supplies typed capabilities. Neither diagnostic command
classification nor a destination distro name may decide execution.

Lifecycle application targets the selected root and records generation
activation intents. It does not run against the mutable live root and does not
delegate transaction ownership to the source package manager.

## Authoring And Consumption

The sole build-result writer is
`write_signed_current_ccs_package`. Low-level tests and already-projected
producers may use `write_v2_ccs_package`; both require a signing key. There is
no unsigned writer.

The normal local loop is:

```text
conary ccs init --template minimal-file
conary ccs lint
conary ccs build --local-dev
conary ccs verify package.ccs
conary ccs test package.ccs --dry-run
```

Mutation and publication consumers must verify first and construct
`CcsPackage` from the resulting capability. `CcsPackage::parse` intentionally
does not open an archive without trust.

## Rejection Contract

Readers reject every `format_version` other than `2`. The format-1 rejection
test uses a small hand-authored retired header so the repository does not need
a v1 writer, schema, projection, fixture factory, or format specification.
Git history is the only source for the removed pre-alpha implementation.

## Proof

Focused verification for this contract is:

```bash
cargo test -p conary-core ccs::v2
cargo test -p conary-core ccs::archive_reader
cargo test -p conary-core ccs::verify
cargo test -p conary-core ccs::package
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4b
cargo test -p conary-core repository::static_repo::publish_gate
```
