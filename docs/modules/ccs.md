---
last_updated: 2026-06-18
revision: 17
summary: Route CCS v2 authority, manifest provenance, and release attestations
---

# CCS Module (conary-core/src/ccs/)

Conary's native package format. Handles building, signing, policy enforcement,
declarative hooks, legacy format conversion, and OCI export.

## Data Flow: Package Build

```
ccs.toml (manifest)
     |
CcsBuilder::new(manifest, source_dir)
     |
  Walk source directory
     |
  For each file:
     +-- Compute SHA-256 hash
     +-- Apply PolicyChain (Keep / Replace / Skip / Reject)
     +-- Classify into component (explicit override or auto)
     +-- Optional: split into CDC chunks (FastCDC)
     |
  Group files by component -> ComponentData
     |
  BuildResult { manifest, components, files, blobs, chunk_stats }
     |
  Sign manifest (Ed25519) -> embed PackageSignature
     |
  Output .ccs archive (tar.gz with MANIFEST.cbor + MANIFEST.toml + objects/)
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `CcsManifest` | manifest.rs | Root ccs.toml structure (package, provides, requires, hooks, policy, etc.) |
| `ManifestProvenance` | manifest_provenance.rs | Provenance DTOs embedded by the root manifest, including hermetic evidence, build attestations, and foreign conversion boundaries |
| `BuildAttestationEnvelope` | attestation.rs | Signed M2 release-publish attestation payload and verification helpers |
| `CcsBuilder` | builder.rs | Builds a CCS package from manifest + source directory |
| `BuildResult` | builder.rs | Output: manifest, components, files, blobs, total_size |
| `CcsPackage` | package.rs | Parsed .ccs file ready for installation via PackageFormat trait |
| `AuthorityDocumentV2` | v2/schema.rs | Signed CCS v2 native package authority |
| `BinaryManifest` | binary_manifest.rs | CBOR-encoded compact manifest (FORMAT_VERSION=1) |
| `SigningKeyPair` | signing.rs | Ed25519 key generation, signing, file I/O |
| `PackageSignature` | signing.rs | Embedded signature with algorithm, key_id, timestamp |
| `HookExecutor` | hooks/ | Runs declarative hooks with rollback tracking |
| `LegacyScriptletBundle` | legacy_scriptlets.rs | Converted RPM/DEB/Arch scriptlet decisions and local replay policy |
| `BuildPolicy` (trait) | policy.rs | Pluggable build policy (DenyPaths, StripBinaries, FixShebangs, etc.) |
| `EnhancementEngine` (trait) | enhancement/ | Post-conversion enhancement (capabilities, provenance, subpackages) |

## Submodules

**manifest.rs and manifest_provenance.rs** -- `ccs::manifest` remains the root
manifest schema and validation owner. The provenance DTOs live in
`ccs::manifest_provenance` and are re-exported from `ccs::manifest` so existing
imports keep working. M2 release publish stores hermetic evidence, signed
build-attestation envelopes, and foreign conversion boundaries in manifest
provenance. Artifact-form `conary publish <pkg.ccs> <target>` is allowed only
after `repository::static_repo::publish_gate` verifies package signatures, TOML
integrity, attestation authority, output identity, command-risk evidence, and
foreign-boundary hashes.

**hooks/** -- Declarative hook executors. Pre-install order: groups, users,
directories. Post-install order: systemd, tmpfiles, sysctl, alternatives.
All operations respect a target_root parameter for bootstrap/container use.

Hook types: User, Group, Directory, Systemd, Tmpfiles, Sysctl, Alternatives.

**convert/** -- Legacy (RPM/DEB/Arch) to CCS conversion. Builds scriptlet
decisions from the adapter registry, blocked-class registry, support matrix,
replay policy, and target compatibility checks. Declarative manifest hooks are
emitted only from adapter-backed or curated evidence; text-pattern detections
remain advisory metadata for review diagnostics. Remaining scripts are
preserved for guarded local replay or review when they cannot be safely
captured. Tracks conversion fidelity (High/Medium/Low) via `FidelityReport`.

**legacy_scriptlets.rs** -- Versioned metadata for converted package scriptlet
semantics and local replay planning. The v1 bundle lives in the TOML manifest as
`[legacy_scriptlets]` and records source package identity, target
compatibility, per-entry decisions, effects, reserved trigger/purge metadata,
timeouts, and evidence digests. It is TOML-only in this revision; the CBOR
`BinaryManifest` remains unchanged and archive reads overlay the TOML field
when both manifest formats are present.

**v2/** -- CCS v2 native package authority. Start in
`crates/conary-core/src/ccs/v2/` for v2 authority, validation, diagnostics,
archive reading, and content identity. Use `archive_reader.rs` and `package.rs`
only as version-routing/adaptation surfaces.

Native v2 authoring from `ccs.toml` starts in
`apps/conary/src/commands/ccs/{templates.rs,lint.rs,build.rs,test.rs,local_dev.rs}`
for command ergonomics and local-dev state, and
`crates/conary-core/src/ccs/v2/authoring.rs` for projection from `BuildResult`
into signed v2 authority.

**enhancement/** -- Post-conversion enrichment via trait-based plugins.
Adds capabilities, provenance, and subpackage relationships that the
original format lacked. Uses EnhancementRunner with a registry pattern.

**export/** -- OCI image export. Produces OCI-layout archives with gzipped
tar layers, image config, and manifest. ContainerConfig controls entrypoint,
cmd, env, ports, user.

## Architecture Context

CCS sits at the center of Conary's format pipeline. All package formats
(RPM, DEB, Arch) convert to CCS before installation. The builder produces
CAS-compatible content (SHA-256 keyed blobs), and the chunking system
enables delta-efficient distribution via the Remi server.

## Fixture Ownership

The first fixture ownership map for CCS conversion lives in
`docs/modules/test-fixtures.md`. Start there before changing golden conversion
cases, support-matrix fixture names, adapter-backed public-ready evidence, or
legacy scriptlet bundle fixtures. The fast proof for map-only or table-only
changes is:

```bash
cargo test -p conary-core golden_fixtures
cargo test -p conary-core support_matrix
```

If conversion output changes, also run:

```bash
cargo test -p conary --test conversion_integration golden_conversion
```

## CCS v2 Native Authority

CCS v2 packages use the CBOR `MANIFEST` with `format_version = 2` as signed
install-time authority. `MANIFEST.toml` may be present for source/debug
visibility, but TOML-only install behavior is not native authority. The v2
implementation lives under `crates/conary-core/src/ccs/v2/`; legacy v1
`BinaryManifest` parsing remains a migration/fixture surface.

### Native CCS v2 Local Authoring Loop

The first supported native authoring loop is:

```text
conary ccs init --template minimal-file
conary ccs lint
conary ccs build --format v2 --local-dev
conary ccs verify
conary ccs test --dry-run
```

`--local-dev` signs with a user-local development key for iteration.
Local-dev artifacts can verify and dry-run-test locally, but static publish and
Remi release paths still require accepted release trust and build attestation.

## Legacy Scriptlet Bundles And Replay

Converted CCS packages may carry a `[legacy_scriptlets]` section. Local Conary
clients consume this bundle during install, update, remove, restore, batch, and
autoremove planning. Entries with `review`, `blocked`, or unknown decisions
refuse before mutation. Entries with `legacy` decisions are replayed only after
the bundle passes target, sandbox, lifecycle, timeout, and ordering preflight
and the operator explicitly provides `--allow-legacy-replay`.

Passive conversion bundle construction lives under
`crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` and
`crates/conary-core/src/ccs/convert/scriptlet_bundle/`. The hub preserves the
public conversion API while child modules own public DTOs, entry decisions,
native ABI metadata projection, evidence digests, summaries, and fixtures.

Core replay planning lives in
`crates/conary-core/src/ccs/legacy_replay.rs`. The install-side adapter that
binds that planner to local install/update/remove replay execution and audit
metadata lives in `apps/conary/src/commands/install/legacy_replay.rs`.

Public-ready conversion is narrower than local replay acceptance. The supported
public source targets are `fedora-44`, `ubuntu-26.04`, and `arch`. A converted
artifact is public-ready only when the scriptlet outcome is native-free or
fully replaced by adapter/support-matrix evidence for the exact source and
target. Legacy replay, review-required, blocked, malformed, or local-only
scriptlet outcomes remain private conversion results.

Foreign raw replay has a second gate. If the bundle source target differs from
the host target and the host is not listed in `allowed_targets`, the operation
also requires `--allow-foreign-legacy-replay` plus compatible bundle and host
mixing policy. `--no-scripts` is not a bypass for required raw replay: it
suppresses ordinary CCS hooks for replaced-only bundles, but refuses when the
selected lifecycle needs a raw legacy entry.

Converted CCS packages can carry metadata about legacy native scriptlets, but
CCS format does not make raw native scriptlets portable across distributions.
Raw replay of `family-compatible` legacy scriptlets is accepted only when an
explicit target compatibility matrix entry authorizes the source and host target
pair and the shallow compatibility preflight succeeds. The default production
matrix is empty, so Conary fails closed unless a later release ships or
configures validated compatibility evidence.

Accepted bundles are persisted with the installed trove so remove and upgrade
can replay or refuse safely even if the original `.ccs` archive is no longer in
the cache. Remi publication remains a separate gate; review, blocked, and raw
legacy replay requirements do not become public-serving approval merely because
the local client can consume the bundle.

Operators can inspect a local CCS package with:

```bash
conary query scripts ./nginx.ccs
conary query scripts ./nginx.ccs --verbose
conary query scripts ./nginx.ccs --entry rpm:%post
conary query scripts ./nginx.ccs --json
```

The CCS query output shows decisions, reasons, effects, body digests, and
reserved metadata summaries. It does not print preserved raw script bodies in
text or JSON output by default. Existing RPM/DEB/Arch package-file scriptlet
inspection keeps its current default behavior.

## Install

CCS packages are installed via `conary ccs install`. The installer verifies
signatures, evaluates capability policy, stores content in CAS, reuses the
shared composefs generation transaction, and runs declarative hooks.

```bash
conary ccs install package.ccs --yes         # Standard install
conary ccs install package.ccs --reinstall --yes # Reinstall same version (replaces files in CAS)
conary ccs install package.ccs --dry-run     # Preview without applying
```

The `--reinstall` flag forces reinstallation even when the same version is
already present. This is useful for repairing corrupted files or re-running
hooks without bumping the version.

Implementation routing: `apps/conary/src/commands/ccs/install.rs` is the
stable command hub. Command execution lives in
`apps/conary/src/commands/ccs/install/command.rs`; dependency/version policy
lives in `apps/conary/src/commands/ccs/install/dependency.rs`; component
selection lives in `apps/conary/src/commands/ccs/install/component_selection.rs`;
capability-policy enforcement lives in
`apps/conary/src/commands/ccs/install/capability_policy.rs`; and payload path
normalization remains in `apps/conary/src/commands/ccs/payload_paths.rs`.

CCS also exposes two package-scoped runtime helpers that are positively covered
in Phase 4:

```bash
conary ccs shell package-name          # Interactive environment with package contents
conary ccs run package-name -- cmd     # One-shot execution under that environment
conary ccs install package.ccs --components runtime,config --yes
```

Selective component installs persist only the requested components and skip
runtime hooks when a purely non-runtime slice is installed.

See also: [docs/specs/ccs-format-v1.md](/docs/specs/ccs-format-v1.md),
[docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
