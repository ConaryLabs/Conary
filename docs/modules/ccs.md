---
last_updated: 2026-03-11
revision: 3
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

- Native-package corruption coverage now uses per-distro format rejection:
  Group G's T81 fixture set is built as truncated RPM/DEB/Arch packages rather
  than checksum-mismatched native packages. This still exercises the intended
  native-format rejection path, but the current builder produces parse/format
  failures, not repo-style checksum diagnostics.
- Missing malicious fixture variants:
  Group I expects dedicated fixtures for proc-environ access, outside-root
  writes, expired signatures, capability policy violations, decompression bombs,
  and intentionally failing scriptlets. Those fixture packages are referenced by
  the manifest plan but are not built yet.
- Install-time capability policy currently fails closed:
  Group I now preserves capability declarations through the CCS binary-manifest
  round trip and rejects packages that declare Linux capabilities at install
  time. That is safer than silently accepting them, but it is still a temporary
  enforcement posture until Conary grows a real capability allow/deny and
  application model for CCS installs.
- Manifest/runtime ergonomics:
  Adversarial manifests currently need to hard-code in-container fixture paths
  such as `/opt/remi-tests/fixtures/...` because the Rust test engine does not
  yet expose a dedicated adversarial fixture-root variable the way Phase 2 uses
  named fixture variables.

The current mock-server support used by Phase 3 Groups G, H, and K is also
intentionally minimal. It can serve static routes with optional headers, delay,
and body truncation, but it cannot yet model:

- Stateful retries or per-request response sequences
- Mirror pools or first-success failover behavior
- TLS handshakes, certificate chains, or hostname validation failures
- Large generated bodies created during setup before the server starts

The lifecycle manifests in Group L also exposed a few execution-environment
limits that make the current tests more "robustness probes" than strict end-to-
end assertions:

- Unprivileged generation switching:
  `system generation switch` and `rollback` often hit mount or namespace
  permission failures inside the integration containers, so the manifests can
  only assert meaningful output and absence of panics rather than guaranteed
  successful generation transitions.
- Self-update artifact modeling:
  The current self-update tests synthesize local HTTP payloads from the running
  binary or placeholder bytes. That is good enough for checksum/truncation
  handling, but it is not yet a faithful signed update artifact pipeline.
- Bootstrap artifact validation:
  Phase 3 can currently assert that stage0 starts, produces files, and yields
  executable-looking output, but not yet that a full stage0 artifact set is
  complete across all distros in CI.
- Kernel and bootloader package assumptions:
  The container half of Group N relies on per-distro package-name overrides for
  kernels and bootloaders (`kernel`, `linux-image-generic`, `linux`, `grub2`,
  `grub-efi-amd64`, `grub`) and verifies deployed files plus generation/BLS
  artifacts. Actual boot correctness, kernel activation, and reboot semantics
  still depend on the QEMU half of Group N.
- QEMU boot execution assumptions:
  The first `qemu_boot` engine pass shells out to host `curl`,
  `qemu-system-x86_64`, and `ssh`, caches qcow2 images locally, and assumes the
  test image exposes SSH on port 22 for `root@127.0.0.1` via user-mode port
  forwarding. That is enough for the Phase 3 manifest flow, but it is not yet a
  repo-native orchestrator abstraction with richer auth, snapshot, or serial
  console handling.
- Adversarial fixture publishing path:
  The initial Task 24 publish attempt reached `https://packages.conary.io`, but
  every upload to `/test-fixtures/adversarial/` failed, and a direct GET to
  that path returned the main HTML package index rather than a browsable static
  artifact directory. The Remi-side publish/hosting path for adversarial
  fixtures still needs to be wired up before those artifacts can be verified
  end-to-end from CI.

These should be treated as follow-up work after the Phase 3 plan lands so the
manifest coverage can move from "planned and parseable" to fully executable
without placeholder paths, approximated attack cases, or mock-server workarounds.

## Phase 3 Workarounds Summary

The following implementation-time workarounds were taken during the 2026-03-11
Phase 3 adversarial-test rollout and should be revisited after the plan lands:

- Manifests reference some future fixture paths directly:
  Several Phase 3 manifests intentionally point at fixture names and output
  paths that match the design, even when the corresponding fixture builders are
  not fully implemented yet.
- Group J dependency semantics are approximated:
  Conflict and OR-dependency scenarios are represented with versioned packages
  and shared capabilities rather than first-class manifest syntax.
- Group K server-failure cases are approximated with a static mock server:
  Retry, failover, rollback-protection, TLS, and large-metadata behaviors are
  modeled with the nearest expressible static HTTP responses instead of a fully
  stateful test server.
- Group L lifecycle tests are robustness probes more than strict success tests:
  Generation switching, rollback, self-update artifacts, and bootstrap outputs
  are asserted conservatively because container permissions and bootstrap cost
  make fully strict end-to-end checks impractical in the current environment.
- Group N container tests validate file/layout state, not actual boot:
  Real boot correctness is deferred to the QEMU manifest and image pipeline.
- The first `qemu_boot` implementation is intentionally thin:
  It shells out to host tools, assumes SSH availability in the guest, and lacks
  richer VM orchestration features like snapshots, serial-pattern matching, or
  guest-specific authentication flows.
- `build-all.sh` currently skips missing or expensive fixture families:
  Boot images are still skipped unless their prerequisites exist or explicit
  opt-in environment variables are provided.
- Task 24 could only complete locally:
  Adversarial fixtures build successfully after local script fixes, but the
  Remi publish target for `/test-fixtures/adversarial/` is not yet accepting or
  serving those artifacts as a dedicated static fixture directory.
- Local Phase 3 smoke is still partially blocked on Podman compatibility:
  On 2026-03-11, `conary-test` could be pointed at a live Podman API socket,
  but the Bollard-backed image build failed because Podman rejected the
  `X-Registry-Config` header on `/build`. A local full-phase smoke pass still
  needs either a Podman-compatible build path in `conary-test` or a Docker
  socket environment.
- Real converted-package installs still need explicit base-system adoption:
  Group M showed that syncing and installing real distro packages from Remi is
  not enough by itself in a fresh test DB because dependency checks do not yet
  treat already-installed host packages like `glibc` or `libc.so.6` as
  satisfied. The current manifest workaround is to seed the DB with
  `conary system adopt --system` before those native-package install tests.

See also: [docs/specs/ccs-format-v1.md](/docs/specs/ccs-format-v1.md),
[docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
