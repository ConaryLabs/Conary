---
last_updated: 2026-03-11
revision: 2
summary: Document current ccs.toml schema and fixture gaps found while building Phase 3 adversarial tests
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
| `CcsBuilder` | builder.rs | Builds a CCS package from manifest + source directory |
| `BuildResult` | builder.rs | Output: manifest, components, files, blobs, total_size |
| `CcsPackage` | package.rs | Parsed .ccs file ready for installation via PackageFormat trait |
| `BinaryManifest` | binary_manifest.rs | CBOR-encoded compact manifest (FORMAT_VERSION=1) |
| `SigningKeyPair` | signing.rs | Ed25519 key generation, signing, file I/O |
| `PackageSignature` | signing.rs | Embedded signature with algorithm, key_id, timestamp |
| `HookExecutor` | hooks/ | Runs declarative hooks with rollback tracking |
| `BuildPolicy` (trait) | policy.rs | Pluggable build policy (DenyPaths, StripBinaries, FixShebangs, etc.) |
| `EnhancementEngine` (trait) | enhancement/ | Post-conversion enhancement (capabilities, provenance, subpackages) |

## Submodules

**hooks/** -- Declarative hook executors. Pre-install order: groups, users,
directories. Post-install order: systemd, tmpfiles, sysctl, alternatives.
All operations respect a target_root parameter for bootstrap/container use.

Hook types: User, Group, Directory, Systemd, Tmpfiles, Sysctl, Alternatives.

**convert/** -- Legacy (RPM/DEB/Arch) to CCS conversion. Extracts declarative
hooks from scriptlets, runs original scripts as-is (assumed idempotent).
Tracks conversion fidelity (High/Medium/Low) via FidelityReport.

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

## Known Schema Gaps

The current `ccs.toml` manifest schema was sufficient for the initial Phase 3
fixture work, but the dependency-fixture pass on 2026-03-11 exposed two areas
that still need first-class schema support:

- Package-level conflicts:
  There is no clear manifest field for declaring that one CCS package conflicts
  with another package by name/version, which limits direct coverage for tests
  like "install B that conflicts with installed A".
- Explicit OR dependencies:
  The manifest supports package dependencies and provided capabilities, but not
  a first-class `foo | bar` dependency expression. Current fixtures approximate
  this with shared capabilities, which is useful but not a full substitute for
  package-level preference ordering semantics.

If we want the Phase 3 Group J fixtures to model the resolver cases exactly as
specified, `conary-core/src/ccs/manifest.rs` will likely need a schema extension
for package conflicts and OR-dependency expressions.

## Known Fixture And Coverage Gaps

The broader Phase 3 adversarial-test pass on 2026-03-11 also exposed several
fixture and coverage gaps that are not purely `ccs.toml` schema problems:

- Missing large-package fixtures:
  Group H crash-recovery tests assume a large install target under
  `tests/fixtures/adversarial/large/`, but `build-large.sh` and its generated
  CCS outputs do not exist yet.
- Missing tampered-fixture coverage:
  Group G includes a tampered-after-signing scenario in the design, but the
  checked-in corrupted fixture set currently only builds bad-checksum,
  truncated, and size-lie variants.
- Missing malicious fixture variants:
  Group I expects dedicated fixtures for proc-environ access, outside-root
  writes, expired signatures, capability policy violations, decompression bombs,
  and intentionally failing scriptlets. Those fixture packages are referenced by
  the manifest plan but are not built yet.
- Manifest/runtime ergonomics:
  Adversarial manifests currently need to hard-code in-container fixture paths
  such as `/opt/remi-tests/fixtures/...` because the Rust test engine does not
  yet expose a dedicated adversarial fixture-root variable the way Phase 2 uses
  named fixture variables.

These should be treated as follow-up work after the Phase 3 plan lands so the
manifest coverage can move from "planned and parseable" to fully executable
without placeholder paths or approximated attack cases.

See also: [docs/specs/ccs-format-v1.md](/docs/specs/ccs-format-v1.md),
[docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
