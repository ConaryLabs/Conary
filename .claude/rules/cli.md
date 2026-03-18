---
paths:
  - "src/commands/**"
---

# CLI Commands Module

One file per command group, with subdirectories for complex commands (e.g.,
`install/`, `adopt/`, `generation/`). Commands are thin wrappers that call
conary-core -- no business logic lives in the CLI layer.

## Structure
- Each command file exports `cmd_*` functions (e.g., `cmd_install`, `cmd_remove`)
- Complex commands use subdirectories: `install/batch.rs`, `adopt/conflicts.rs`
- All handlers re-exported from `commands/mod.rs`
- CLI definitions live in `src/cli/` (separate from handlers)

## Key Command Groups
- `install/` -- `cmd_install`, `InstallOptions`, `DepMode`, batch operations
- `remove.rs` -- package removal
- `update.rs` -- package updates
- `adopt/` -- system adoption (convert, conflicts, refresh, status, takeover)
- `config.rs` -- config file management (backup, check, diff, list, restore)
- `system.rs` -- system-level operations
- `canonical.rs` -- canonical package name management
- `registry.rs` -- repository registry
- `automation.rs` -- automated maintenance (check, apply, configure, daemon, history)
- `federation.rs` -- federation management
- `distro.rs` -- distribution-specific operations
- `generation/` -- system generation switching

## Invariants
- No business logic in CLI -- all logic lives in conary-core
- Output formatting follows consistent patterns (use `progress` module)
- Feature-gated commands: `cmd_ai_*` require `--features experimental`
- Commands open DB via `open_db()` helper (defined in `mod.rs`) -- never bare `db::open()`
- Lookup installed troves via `Trove::find_one_by_name()` -- not manual `find_by_name()` + `.first()`

## Output Standards
- `println!()` for user-facing results only
- `tracing::info!()` / `warn!()` for diagnostics and progress
- `eprintln!()` only for usage errors
- Never use `dbg!()` or `print!()` in committed code

## Function Size
- Command handlers should be < 300 lines
- Extract sub-functions for logical sections (resolution, validation, execution)
- Long orchestration functions should delegate to well-named helpers

## Gotchas
- `SandboxMode` is re-exported from `conary_core::scriptlet` at module level
- `progress.rs` and `groups.rs` are `pub` modules (used by other crates)
- Some modules are `pub` for cross-crate visibility: `canonical`, `ccs`, `distro`, `registry`, `trust`, `generation`
- `convert_pkgbuild.rs` wraps `conary_core::recipe::pkgbuild`
- `cook.rs` wraps `conary_core::recipe::Cook`

## Files
- `mod.rs` -- re-exports all `cmd_*` functions, defines `open_db()` helper
- `install/` -- install command with `batch.rs` for multi-package
- `adopt/` -- system adoption with `conflicts.rs`, `convert.rs`
- `generation/` -- generation management with `switch.rs`, `builder.rs`, `boot.rs`, `composefs.rs`, `metadata.rs`, `takeover.rs`, `commands.rs`
- `export.rs` -- OCI image export
- `composefs_ops.rs` -- rebuild_and_mount helper for composefs operations
- `replatform_rendering.rs` -- replatform plan rendering
- `package_parsing.rs` -- shared package parsing helpers
- Individual files: `remove.rs`, `update.rs`, `config.rs`, `system.rs`, `self_update.rs`, `update_channel.rs`, etc.
