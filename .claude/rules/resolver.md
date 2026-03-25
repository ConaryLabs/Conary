---
paths:
  - "conary-core/src/resolver/**"
---

# Resolver Module

SAT-based dependency resolution using the `resolvo` crate's CDCL solver.
The resolver builds a dependency graph from the database, then uses SAT solving
for install/removal with backtracking support.

## Key Types
- `Resolver<'db>` -- main entry point, wraps `DependencyGraph` + `Connection`
- `DependencyGraph` -- directed graph of `PackageNode` with `DependencyEdge`
- `PackageNode` -- node with name + `RpmVersion`
- `DependencyEdge` -- edge with `VersionConstraint`
- `ResolutionPlan` -- result: `install_order`, `missing` deps, `conflicts`
- `ConaryProvider<'db>` -- resolvo `DependencyProvider` + `Interner` bridge
- `SatResolution` / `SatPackage` -- SAT solver output types
- `ComponentResolver` -- component-level resolution for independent install/remove
- `SolverDep` -- dependency enum: `Single` vs `OrGroup` for OR-dependency modeling
- `ConaryPackageVersion::InstalledNative` -- variant for non-RPM installed packages

## Invariants
- `Resolver::new()` builds the full graph from DB at construction
- `resolve_install()` checks only the new package's edges, not full system
- `solve_install()` in `sat.rs` is the SAT entry point -- takes `(name, constraint)` pairs
- `ConaryProvider` lazily loads dependencies from DB as solver requests them
- Topological sort provides installation order; cycles are detected as errors

## Gotchas
- `Resolver::resolve_install()` uses graph-based local check; `sat::solve_install()` uses full SAT
- `ConaryProvider` interns names, solvables, version sets, and strings -- IDs are indices into Vecs
- `SatSource::Installed` vs `SatSource::Repository` distinguishes existing vs new packages
- `provider/` queries DB on demand -- not all packages are loaded upfront
- `canonical.rs` handles canonical name resolution (separate concern, single file)
- OR groups from normalized requirement groups are modeled via `VersionSetUnionId` in resolvo

## Files
- `engine.rs` -- `Resolver` struct, `resolve_install()`, `resolve()`
- `sat.rs` -- `solve_install()`, `solve_removal()`, `SatResolution`, `SatPackage`
- `provider/` -- `ConaryProvider`, resolvo trait implementations (5 files: loading.rs, matching.rs, mod.rs, traits.rs, types.rs)
- `canonical.rs` -- canonical name resolution
- `graph.rs` -- `DependencyGraph`, `PackageNode`, `DependencyEdge`, topological sort
- `plan.rs` -- `ResolutionPlan`, `MissingDependency`
- `conflict.rs` -- `Conflict` enum variants
- `component_resolver.rs` -- component-level resolution
