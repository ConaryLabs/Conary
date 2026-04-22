---
last_updated: 2026-04-22
revision: 1
summary: Query-oriented CLI surfaces, local database read paths, and the related SBOM command placement
---

# Query Module (apps/conary/src/commands/query/)

Package, file, dependency, and repository queries against the local SQLite
database. Label management remains nested under `conary query`, while the
CycloneDX SBOM surface now lives at the top-level `conary sbom` command even
though its implementation still lives in `src/commands/query/sbom.rs`.

## Data Flow: Query Dispatch

```
conary query <subcommand> [args]
        |
  apps/conary/src/cli/query.rs -- Clap definition (QueryCommands enum)
        |
  apps/conary/src/commands/query/mod.rs -- Dispatch to handler
        |
  +-- depends / rdepends    -> dependency.rs   (DependencyEntry table)
  +-- deptree               -> deptree.rs      (recursive traversal, cycle detection)
  +-- whatprovides           -> dependency.rs   (ProvideEntry table)
  +-- whatbreaks             -> dependency.rs   (reverse dependency impact)
  +-- reason                 -> reason.rs       (Trove.install_reason filter)
  +-- repquery               -> repo.rs         (RepositoryPackage table)
  +-- component / components -> components.rs   (Component + FileEntry tables)
  +-- scripts                -> package.rs      (parse scriptlets from .rpm/.deb/.pkg)
  +-- conflicts              -> dependency.rs   (file ownership overlap detection)
  +-- delta-stats            -> dependency.rs   (delta update statistics)
  +-- label                  -> cli/label.rs + dispatch.rs   (label path, delegation, and provenance management)

Related top-level command:
conary sbom [--profile ... | --derivation ...]
        |
  apps/conary/src/dispatch.rs -> sbom.rs       (CycloneDX derivation export)
```

## Key Types

| Type | Source | Purpose |
|------|--------|---------|
| `Trove` | db/models/ | Installed package record (name, version, reason, pinned) |
| `DependencyEntry` | db/models/ | Typed dependency link (runtime, build, etc.) |
| `ProvideEntry` | db/models/ | Capability declaration (soname, pkgconfig, virtual) |
| `FileEntry` | db/models/ | Installed file (path, hash, perms, component) |
| `Component` | db/models/ | Logical subpackage (:runtime, :lib, :devel, :doc) |
| `RepositoryPackage` | db/models/ | Available package from synced repo metadata |

## Database Tables

Primary tables hit by queries:

| Table | Indexed On | Used By |
|-------|-----------|---------|
| `troves` | name, type | All queries |
| `dependencies` | trove_id, depends_on_name | depends, rdepends, deptree, whatbreaks |
| `provides` | trove_id, capability | whatprovides |
| `files` | path, trove_id, component_id | component, conflicts |
| `components` | parent_trove_id, name | component, components |
| `repository_packages` | name, repository_id | repquery |

## Query Patterns

- **Direct lookup**: `Trove::find_by_name()` for single-package queries
- **Relationship traversal**: JOIN across dependencies/provides/files
- **Pattern matching**: LIKE queries with wildcards on names and paths
- **Recursive traversal**: deptree uses HashSet-based cycle detection with configurable depth
- **Reverse lookup**: `WHERE depends_on_name = ?` for rdepends/whatbreaks

## Related SBOM Command

`cmd_sbom()` lives in this module tree, but the user-facing surface is the
top-level `conary sbom` command rather than `conary query sbom`.

It produces CycloneDX JSON from derivation data, targeting either a single
derivation or a named profile.

Each component includes:
- Package URL (PURL): `pkg:conary/name@version?arch=x86_64`
- SHA-256 hash from first file entry
- Tool metadata (vendor: ConaryLabs, version from Cargo)

Output to stdout or file via `--output`.

## Architecture Context

The query module is read-only -- it never modifies the database. All queries
run against the local SQLite instance populated by install/remove/sync
operations. Repository queries (`repquery`) hit the `repository_packages`
table, which is refreshed by `conary repo sync`.

See also: [docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
