---
paths:
  - "src/**"
---

# Conary Architecture Reference

## Key Modules

| Module | Purpose |
|--------|---------|
| `src/db/` | SQLite schema, models, migrations |
| `src/packages/` | RPM/DEB/Arch parsers (unified via `common.rs` PackageMetadata) |
| `src/compression/` | Unified decompression (Gzip, Xz, Zstd) with format detection |
| `src/repository/` | Remote repos, metadata sync, mirror health, metalink, substituters |
| `src/resolver/` | SAT-based dependency resolution (resolvo) |
| `src/filesystem/` | CAS, file deployment, VFS tree |
| `src/delta/` | Binary delta updates (zstd dictionary compression) |
| `src/version/` | Version parsing, constraints |
| `src/container/` | Scriptlet sandboxing, namespace isolation |
| `src/trigger/` | Post-install trigger system |
| `src/scriptlet/` | Scriptlet execution, cross-distro support |
| `src/label.rs` | Package provenance labels |
| `src/flavor/` | Build variation specs |
| `src/components/` | Component classification |
| `src/dependencies/` | Dependency type definitions, provider matching |
| `src/derived/` | Derived package builder |
| `src/transaction/` | Crash-safe atomic operations, journal-based recovery |
| `src/model/` | System Model - declarative OS state |
| `src/ccs/` | CCS native package format, builder, policy engine, OCI export |
| `src/server/` | Remi server (feature-gated: `--features server`) |
| `src/cli/` | CLI definitions |
| `src/commands/` | Command implementations |
| `src/recipe/` | Recipe system for building packages from source |
| `src/capability/` | Capability declarations - audit, enforcement, inference |
| `src/provenance/` | Package DNA / full provenance tracking |
| `src/trust/` | TUF supply chain trust |
| `src/automation/` | Automated maintenance (security updates, orphan cleanup) |
| `src/bootstrap/` | Bootstrap a complete Conary system (8-stage pipeline) |
| `src/federation/` | CAS federation - peer discovery, chunk routing, mTLS |
| `src/daemon/` | conaryd daemon (feature-gated: `--features daemon`) |
| `src/hash.rs` | Multi-algorithm hashing (SHA-256, Blake3, XXH128) |
