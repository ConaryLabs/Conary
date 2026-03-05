# Recipe Module (conary-core/src/recipe/)

Source-based package building. Parses TOML recipe files, resolves build
dependencies, executes builds in isolated environments, and caches artifacts.

## Data Flow: Recipe Cook

```
recipe.toml
     |
  parser.rs -- Deserialize TOML via serde, validate
     |
  Recipe { package, source, build, cross, patches, components }
     |
  Kitchen::new(config, optional MakedependsResolver)
     |
  resolve_makedepends() -- install missing build deps
     |
  Cook::new(recipe, kitchen_config)
     |
  Phase pipeline:
     1. Fetch   -- download archive + additional sources + patches
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

## Architecture Context

Recipes produce CCS packages, feeding into the same CAS and transaction
pipeline as any other installation. The Kitchen uses Linux namespace
isolation (via the container module) for hermetic builds. Provenance
data captured during cooking is embedded in the output CCS manifest.

See also: [docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
