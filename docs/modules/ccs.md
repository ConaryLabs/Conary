---
last_updated: 2026-03-28
revision: 7
summary: Refresh CCS conversion, runtime helper, and selective-component install notes
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
hooks from scriptlets where possible and preserves remaining scripts for
sandboxed execution when they cannot be safely captured. Tracks conversion
fidelity (High/Medium/Low) via `FidelityReport`.

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

## Install

CCS packages are installed via `conary ccs install`. The installer verifies
signatures, evaluates capability policy, deploys files to CAS, and runs
declarative hooks.

```bash
conary ccs install package.ccs               # Standard install
conary ccs install package.ccs --reinstall   # Reinstall same version (replaces files in CAS)
conary ccs install package.ccs --dry-run     # Preview without applying
```

The `--reinstall` flag forces reinstallation even when the same version is
already present. This is useful for repairing corrupted files or re-running
hooks without bumping the version.

CCS also exposes two package-scoped runtime helpers that are positively covered
in Phase 4:

```bash
conary ccs shell package-name          # Interactive environment with package contents
conary ccs run package-name -- cmd     # One-shot execution under that environment
conary ccs install package.ccs --components runtime,config
```

Selective component installs persist only the requested components and skip
runtime hooks when a purely non-runtime slice is installed.

See also: [docs/specs/ccs-format-v1.md](/docs/specs/ccs-format-v1.md),
[docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
