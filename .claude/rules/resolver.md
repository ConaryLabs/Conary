---
paths:
  - "conary-core/src/resolver/**"
---

# Resolver Module

SAT-only dependency resolution using the `resolvo` crate's CDCL solver.
The graph resolver (`engine.rs`, `graph.rs`) was deleted -- `solve_install()`
and `solve_removal()` in `sat.rs` are the only resolution entry points.

## Key Types
- `PackageIdentity` -- enriched package identity modeled after libsolv's Solvable: carries name, version, arch, repo_id, repo_name, version_scheme, canonical_id. Loaded from a single join across repository_packages + repositories + canonical_packages.
- `ProvidesIndex` -- pre-built capability-to-provider index modeled after libsolv's `whatprovides`. Built once at resolution start, O(1) lookup. Queries repository_provides, provides (installed), and appstream_provides.
- `SatResolution` / `SatPackage` -- SAT solver output types
- `ConaryProvider<'db>` -- resolvo `DependencyProvider` + `Interner` bridge. Solvables are `PackageIdentity` instances.
- `ResolutionPlan` / `MissingDependency` -- result types consumed by CLI
- `ComponentResolver` -- component-level resolution for independent install/remove
- `SolverDep` -- dependency enum: `Single` vs `OrGroup` for OR-dependency modeling
- `ConaryConstraint` -- solver-facing constraint type: `Legacy(VersionConstraint)` or `Repository { scheme, constraint, raw }`

## Invariants
- `solve_install()` in `sat.rs` is the sole resolution entry point -- takes `(name, constraint)` pairs
- `solve_removal()` checks what would break if packages are removed
- `ConaryProvider` loads candidates via `PackageIdentity::find_all_by_name()` -- all versions, all repos
- Canonical equivalents always included in candidate pool via `PackageIdentity::find_canonical_equivalents()`
- `sort_candidates` ranks exact-name above canonical, then by version (scheme-aware), then installed > not installed
- Version scheme comes from `PackageIdentity.version_scheme` (explicit from DB), never inferred at resolution time
- Architecture filtering uses `normalize_arch()` for Debian/RPM/Arch alias mapping

## Gotchas
- `ConaryProvider` interns names, solvables, version sets, and strings -- IDs are indices into Vecs
- `SatSource::Installed` vs `SatSource::Repository` distinguishes existing vs new packages
- `canonical.rs` handles canonical name resolution (separate concern, single file)
- OR groups from normalized requirement groups are modeled via `VersionSetUnionId` in resolvo
- `repository_packages.canonical_id` is set during sync by `link_canonical_ids()` -- NULL when no canonical mapping exists
- `plan.rs` types are populated from `SatResolution` for CLI consumption

## Files
- `sat.rs` -- `solve_install()`, `solve_removal()`, `SatResolution`, `SatPackage`
- `identity.rs` -- `PackageIdentity` struct, `find_all_by_name()`, `find_canonical_equivalents()`
- `provides_index.rs` -- `ProvidesIndex`, `ProviderEntry`, `build()`, `find_providers()`
- `provider/` -- `ConaryProvider`, resolvo trait implementations (5 files: loading.rs, matching.rs, mod.rs, traits.rs, types.rs)
- `canonical.rs` -- canonical name resolution, `CanonicalResolver`
- `plan.rs` -- `ResolutionPlan`, `MissingDependency`
- `conflict.rs` -- `Conflict` enum variants
- `component_resolver.rs` -- component-level resolution
