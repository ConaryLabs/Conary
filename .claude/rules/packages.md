---
paths:
  - "conary-core/src/packages/**"
---

# Packages Module

Format-specific parsers for RPM, DEB, and Arch packages. All parsers produce a unified
`PackageMetadata` struct defined in `common.rs`, ensuring consistent behavior regardless
of source format.

## Key Types
- `PackageMetadata` -- unified metadata: name, version, architecture, files, dependencies, scriptlets, config_files
- `PackageFile` -- single file entry from a package (from `traits` module)
- `Dependency` -- package dependency (from `traits` module)
- `Scriptlet` -- install/remove script (from `traits` module)
- `ConfigFileInfo` -- config file needing special handling (from `traits` module)

## Constants
- `MAX_EXTRACTION_FILE_SIZE` -- 512 MB limit per extracted file

## Invariants
- All parsers must return `PackageMetadata` -- never format-specific structs at API boundaries
- `normalize_architecture()` maps "all", "any", "noarch" to canonical "noarch"
- `PackageMetadata::to_trove()` is the standard conversion to database `Trove` records
- Trait types (`PackageFile`, `Dependency`, `Scriptlet`, `ConfigFileInfo`) come from `traits` module

## Gotchas
- `common.rs` imports from `crate::packages::traits`, not from the format parsers
- `dpkg_query` and `pacman_query` query the system package DB, not parse files
- `archive_utils` handles tar/cpio extraction shared across formats
- Architecture strings vary by format -- always normalize before comparison

## Files
- `common.rs` -- `PackageMetadata`, `normalize_architecture()`, `MAX_EXTRACTION_FILE_SIZE`
- `rpm.rs` -- RPM format parser
- `deb.rs` -- DEB format parser
- `dpkg_query.rs` -- queries dpkg database on Debian systems
- `pacman_query.rs` -- queries pacman database on Arch systems
- `archive_utils.rs` -- shared tar/cpio extraction utilities
- `traits.rs` -- `PackageFile`, `Dependency`, `Scriptlet`, `ConfigFileInfo` trait types
