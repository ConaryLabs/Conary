---
last_updated: 2026-06-14
revision: 2
summary: Recipe parsing, M2a hermetic cook, Kitchen execution, and provenance-aware source builds
---

# Recipe Module (conary-core/src/recipe/)

Source-based package building. Parses TOML recipe files, materializes local or
remote sources, executes host/sandboxed/hermetic Kitchen builds, and caches
artifacts.

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
  Kitchen::new(config, optional MakedependsResolver)
     |
  resolve_makedepends() -- install missing build deps
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
  cleanup_makedepends() -- remove temporarily installed build deps
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `Recipe` | format.rs | Complete build spec (package, source, build, cross, patches) |
| `PackageSection` | format.rs | Name, version, release, license, homepage |
| `SourceSection` | format.rs | Archive URL, checksum, additional sources, extract_dir |
| `BuildSection` | format.rs | Commands: configure, make, install, check, setup, post_install |
| `CrossSection` | format.rs | Cross-compilation: target triple, sysroot, tool overrides |
| `BuildStage` | format.rs | Enum: stage0, stage1, stage2, final |
| `Kitchen` | kitchen/mod.rs | Build orchestrator with makedepends resolution |
| `Cook` | kitchen/cook.rs | Single recipe execution through fetch/build/package phases |
| `StageConfig` | kitchen/config.rs | Per-stage sysroot, tools_dir, tool_prefix, target_triple |
| `MakedependsResolver` (trait) | kitchen/makedepends.rs | Pluggable build dependency installer |
| `HermeticBuildEvidence` | hermetic/evidence.rs | Unsigned M2a evidence embedded in CCS provenance |
| `HermeticBuildPlan` | hermetic/plan.rs | Assembles source identity, ecosystem policy, command-risk, reproducibility, and Kitchen hermetic config |
| `HostBuildRecord` | hermetic/divergence.rs | Local host-build comparison input for diagnostic-only M2a divergence reports |
| `RecipeGraph` | graph.rs | Directed dependency graph with topological sort |
| `BuildCache` | cache.rs | Artifact cache keyed by recipe + toolchain + dependency hashes |
| `CacheEntry` | cache.rs | Cached package path, cache key, created timestamp, size |
| `ProvenanceCapture` | kitchen/provenance_capture.rs | Records full build metadata for CCS provenance |
| `ConversionResult` | pkgbuild.rs | PKGBUILD-to-recipe conversion output + warnings |

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

## PKGBUILD Converter

Regex-based extraction from Arch Linux PKGBUILDs. Converts variables
(pkgname, pkgver, depends, makedepends, source, checksums) and build
functions (build, package, prepare, check) to Recipe TOML. Warns on
unsupported features (split packages, VCS sources, dynamic pkgver).

## M2a Hermetic Cook

After M2a, `conary cook --isolated` is the hermetic build path. The CLI loads
`apps/conary/src/commands/hermetic_config.rs`, refuses build dependencies until
content-identity locks exist, and asks `HermeticBuildPlan` to produce the
unsigned evidence stored under
`crates/conary-core/src/recipe/hermetic/`. Kitchen then prefetches sources
while downloads are allowed and switches the build to
`SourceDownloadPolicy::OfflineCacheOnly`, `allow_network = false`, and
pristine/no-host-mount execution before it may emit
`hardening_level = "hermetic"`.

The `hermetic/` module owns evidence DTOs, source identity, ecosystem policy,
command-risk reports, reproducibility controls, and host-vs-hermetic divergence
diagnostics. Kitchen remains the execution owner: `cook_hermetic()` applies the
plan, materializes local sources from the hashed canonical file list, injects
reproducibility environment controls, runs the build, and records the final
Merkle-root comparison after plating.

Project-form `conary publish <target>` uses the same hermetic Kitchen path and
then publishes the cooked CCS package to a static repository. It remains
pre-M2b: the CCS manifest can carry unsigned M2a hermetic evidence, but there
is no signed build-attestation envelope yet and artifact-form
`conary publish <pkg.ccs> <target>` still rejects.

## Architecture Context

Recipes produce CCS packages, feeding into the same CAS and transaction
pipeline as any other installation. The Kitchen uses Linux namespace isolation
through the container module for sandboxed builds and pristine sysroot-only
mounts for M2a hermetic builds. Provenance data captured during cooking is
embedded in the output CCS manifest.

See also: [docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
