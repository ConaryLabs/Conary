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
- `Stage0Builder` -- cross-compilation toolchain via crosstool-ng
- `Stage1Builder` -- self-hosted native toolchain (built with Stage 0)
- `Stage2Builder` -- optional pure rebuild for reproducibility
- `ConaryStageBuilder` -- builds Conary itself in the bootstrap environment
- `BaseBuilder` -- base system (kernel, systemd, coreutils)
- `ImageBuilder` -- bootable image generation (`ImageFormat`, `ImageSize`)

## Constants
- `DEFAULT_TOOLS_DIR` -- `/tools`, `DEFAULT_STAGE1_DIR` -- `/conary/stage1`, `DEFAULT_SYSROOT_DIR` -- `/conary/sysroot`

## Invariants
- Recipe format is TOML with `%(version)s` and `%(destdir)s` variable interpolation
- All builds run in isolated containers (user namespace, private /tmp, resource limits)
- Source archives require SHA-256 checksums
- Bootstrap stages must run in order: 0 -> 1 -> (optional 2) -> base -> image

## Gotchas
- `pkgbuild.rs` converts Arch PKGBUILD files to Conary recipe TOML
- `kitchen/cook.rs` handles build execution; `kitchen/archive.rs` handles source fetching
- `build_helpers.rs` and `conary_stage.rs` are bootstrap-specific helpers
- `repart` submodule is `pub(crate)` only

## Files (recipe)
- `parser.rs` -- `parse_recipe()`, `validate_recipe()`
- `format.rs` -- `Recipe`, `SourceSection`, `BuildSection`
- `graph.rs` -- `RecipeGraph`, `BootstrapPlan`
- `kitchen/` -- `Cook`, `Kitchen`, `KitchenConfig`
- `pkgbuild.rs` -- PKGBUILD conversion
- `cache.rs` -- `BuildCache`, `CacheConfig`

## Files (bootstrap)
- `stage0.rs`, `stage1.rs`, `stage2.rs` -- staged toolchain builders
- `conary_stage.rs` -- Conary self-build
- `base.rs` -- base system builder
- `image.rs` -- bootable image generation
- `config.rs` -- `BootstrapConfig`, `TargetArch`
