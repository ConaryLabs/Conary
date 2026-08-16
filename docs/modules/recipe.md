---
last_updated: 2026-08-16
revision: 8
summary: Explicit recipe scaffolding, parsing, hermetic cook, Kitchen execution, and source provenance
---

# Recipe Module (conary-core/src/recipe/)

Source-based package building. Parses TOML recipe files, materializes local or
remote sources, executes host/sandboxed/hermetic Kitchen builds, and caches
artifacts.

Recipe identity and build behavior are authored facts. `conary new <name>`
creates only a deterministic named scaffold. `conary cook` accepts an explicit
recipe file, a directory containing `recipe.toml`, or the default current
directory only when `./recipe.toml` exists. It does not inspect build-system
markers, clone or extract a target, download a source target, or synthesize a
recipe. Foreign RPM, DEB, and Arch binary inputs remain a separate typed
conversion path. When that conversion needs distribution-owned lifecycle ABI,
`conary cook <native-package> --source-profile <exact-public-id>` passes the
explicit typed profile to the converter; package-format inference cannot stand
in for that source identity.

## Data Flow: Recipe Cook

```
recipe.toml
     |
  parser.rs -- Deserialize TOML via serde, validate
     |
  Recipe { package, source, build, cross, patches, components }
     |
  Optional HermeticBuildPlan -- source identity, policy, risk, reproducibility
     |
  Kitchen::new(config)
     |
  Caller-provided builder environment -- exact locked identities for hermetic builds
     |
  Cook::new(recipe, kitchen_config)
     |
  Phase pipeline:
     1. Fetch   -- download/prefetch archive + additional sources + patches
     2. Unpack  -- extract archive, detect source directory
     3. Patch   -- apply patches with strip levels
     4. Build   -- run configure/make/install in sandbox
     5. Package -- collect output, build CCS package
     |
  ProvenanceCapture -- record sources, patches, deps, timestamps
     |
  BuildCache::store() -- cache artifact by recipe+toolchain hash
     |
```

Recipe build requirements describe the builder environment; they do not
authorize Kitchen to mutate the host. Kitchen never invokes a distro package
manager, invents a dependency identity, or claims an unresolved build
dependency was present. Hermetic builds consume exact dependency identities
from their locked input.

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `Recipe` | format.rs | Complete build spec (package, source, build, cross, patches) |
| `PackageSection` | format.rs | Name, version, release, license, homepage |
| `SourceSection` | format.rs | Archive URL, checksum, additional sources, extract_dir |
| `BuildSection` | format.rs | Commands: configure, make, install, check, setup, post_install |
| `CrossSection` | format.rs | Cross-compilation: target triple, sysroot, tool overrides |
| `BuildStage` | format.rs | Enum: stage0, stage1, stage2, final |
| Named scaffold | scaffold.rs | Validates an explicit package name and deterministically writes `recipe.toml` |
| `Kitchen` | kitchen/mod.rs | Build orchestrator over a caller-supplied builder environment |
| `Cook` | kitchen/cook.rs | Single recipe source preparation and build-phase execution |
| CCS package finalization | kitchen/package_output.rs | Projects recipe metadata, builds exact payload authority, records its provenance Merkle root, and writes the CCS artifact |
| Cook behavior tests | kitchen/cook/tests.rs | Focused source, environment, patch, and hermetic-execution regression coverage |
| Kitchen behavior tests | kitchen/tests.rs | Orchestration, cache, source, and hermetic-boundary regression coverage |
| `StageConfig` | kitchen/config.rs | Per-stage sysroot, tools_dir, tool_prefix, target_triple |
| `HermeticBuildEvidence` | hermetic/evidence.rs | Closed schema-2 build evidence embedded in signed CCS provenance |
| `HermeticBuildPlan` | hermetic/plan.rs | Assembles exact source and dependency identities, diagnostic command-risk, reproducibility, and Kitchen hermetic config |
| `HostBuildRecord` | hermetic/divergence.rs | Local host-build comparison input for diagnostic-only M2a divergence reports |
| `RecipeGraph` | graph.rs | Directed dependency graph with topological sort |
| `BuildCache` | cache.rs | Artifact cache keyed by recipe + toolchain + dependency hashes |
| `CacheEntry` | cache.rs | Cached package path, cache key, created timestamp, size |
| `ProvenanceCapture` | kitchen/provenance_capture.rs | Records full build metadata for CCS provenance |

## Build Graph

`RecipeGraph` supports multi-recipe build ordering via Kahn's topological
sort. Circular dependencies (e.g., glibc <-> gcc) are broken by marking
bootstrap edges with `mark_bootstrap_edge()`. The graph also provides
`find_cycles()` for diagnostics and `transitive_dependencies()` for
computing full build closures.

## Build Cache

Cache keys are deterministic hashes of:
- Package identity (name, version, release)
- Source info (URL, checksum, additional sources)
- Patches (file, checksum, strip level -- order-sensitive)
- Build commands (configure, make, install, check)
- Environment variables (sorted)
- Dependencies (sorted)
- Cross-compilation settings
- Optional: dependency content hashes for reproducibility

Default location: `/var/cache/conary/builds`, sharded by first 2 chars
of cache key. Configurable max_size (10GB) and max_age (30 days).

## M2a Hermetic Cook

After M2a, `conary cook --isolated` is the hermetic build path. The CLI loads
`apps/conary/src/commands/hermetic_config.rs`, requires exact content-identity
locks for build dependencies, and asks `HermeticBuildPlan` to produce the
unsigned evidence stored under
`crates/conary-core/src/recipe/hermetic/`. Kitchen then prefetches sources
while downloads are allowed and switches the build to
`SourceDownloadPolicy::OfflineCacheOnly`, `allow_network = false`, and
pristine/no-host-mount execution before it may emit
`hardening_level = "hermetic"`.

The `hermetic/` module owns evidence DTOs, source identity, command-risk
diagnostics, reproducibility controls, and host-vs-hermetic divergence
diagnostics. It does not derive build authority from marker files or command
text. Hermetic recipe evidence identifies an explicit recipe path and hash;
foreign binary conversion uses its own typed conversion identity. Exact source
content identity, caller-supplied repository dependency locks, and the actual
offline build boundary are authoritative. Kitchen remains
the execution owner: `cook_hermetic()` applies the plan, materializes local
sources from the hashed canonical file list, injects reproducibility
environment controls, runs the build, and records the final Merkle-root
comparison after plating.

Schema 2 is the final unreleased pre-alpha evidence shape for this reset. It
rejects the removed generated-recipe/inference identity instead of migrating
it. Discard and rebuild any local artifacts carrying that superseded shape.

Project-form `conary publish <target>` uses the same hermetic Kitchen path and
then attaches the signed build-attestation envelope before publishing the
cooked CCS package to a static repository. Artifact-form
`conary publish <pkg.ccs> <target>` accepts only a current signed artifact that
passes the shared release publish gate and the destination's trust policy.

Artifact-form destination routing is owned once by
`apps/conary/src/commands/publish/target.rs`. The CLI and packaging agent
service consume the same parsed type: local paths and `file://` select static
publication, while only the exact HTTP(S) route
`/v1/admin/releases/{route}` selects Remi release upload. URL substrings and
diagnostic text are not publication authority.

## Architecture Context

Recipes produce CCS packages, feeding into the same CAS and transaction
pipeline as any other installation. The Kitchen uses Linux namespace isolation
through the container module for sandboxed builds and pristine sysroot-only
mounts for M2a hermetic builds. Provenance data captured during cooking is
embedded in the output CCS manifest.

See also: [docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
