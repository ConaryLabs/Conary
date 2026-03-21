---
paths:
  - "conary-core/src/recipe/**"
  - "conary-core/src/bootstrap/**"
---

# Recipe and Bootstrap Modules

Recipe system for building packages from source using culinary metaphors (recipe,
cook, kitchen). Bootstrap provides staged toolchain building from scratch.

## Recipe Key Types
- `Recipe` -- build spec with `SourceSection`, `BuildSection`, `CrossSection`
- `Cook` / `CookResult` -- executes a recipe in a `Kitchen`
- `Kitchen` / `KitchenConfig` -- isolated build environment
- `RecipeGraph` -- dependency graph with `BootstrapPhase` and `BootstrapPlan`
- `BuildCache` / `CacheConfig` -- cached build artifacts with `ToolchainInfo` hashing

## Bootstrap Key Types
- `Bootstrap` -- top-level orchestrator for the full pipeline
- `CrossToolsBuilder` -- cross-compilation toolchain
- `TempToolsBuilder` -- temporary tools built with cross toolchain
- `FinalSystemBuilder` -- self-hosted native toolchain
- `Tier2Builder` -- optional pure rebuild for reproducibility
- `ImageBuilder` -- bootable image generation (`ImageFormat`, `ImageSize`)
- `BootstrapConfig` / `TargetArch` -- configuration and target architecture

## Invariants
- Recipe format is TOML with `%(version)s` and `%(destdir)s` variable interpolation
- All builds run in isolated containers (user namespace, private /tmp, resource limits)
- Source archives require SHA-256 checksums

## Gotchas
- `pkgbuild.rs` converts Arch PKGBUILD files to Conary recipe TOML
- `kitchen/cook.rs` handles build execution; `kitchen/archive.rs` handles source fetching
- `build_helpers.rs` provides shared bootstrap build utilities
- `repart` submodule is `pub(crate)` only

## Files (recipe)
- `parser.rs` -- `parse_recipe()`, `validate_recipe()`
- `format.rs` -- `Recipe`, `SourceSection`, `BuildSection`
- `graph.rs` -- `RecipeGraph`, `BootstrapPlan`
- `audit.rs` -- recipe dependency auditing
- `kitchen/` -- `Cook`, `Kitchen`, `KitchenConfig`
- `kitchen/makedepends.rs` -- makedepends resolution
- `kitchen/provenance_capture.rs` -- provenance capture during builds
- `pkgbuild.rs` -- PKGBUILD conversion
- `cache.rs` -- `BuildCache`, `CacheConfig`

## Files (bootstrap)
- `mod.rs` -- `Bootstrap` orchestrator
- `cross_tools.rs` -- `CrossToolsBuilder`
- `temp_tools.rs` -- `TempToolsBuilder`
- `final_system.rs` -- `FinalSystemBuilder`
- `tier2.rs` -- `Tier2Builder`
- `image.rs` -- bootable image generation
- `config.rs` -- `BootstrapConfig`, `TargetArch`
- `stages.rs`, `build_runner.rs` -- stage definitions and build execution
- `chroot_env.rs`, `system_config.rs` -- chroot and system setup
- `toolchain.rs` -- toolchain management
