---
last_updated: 2026-06-12
summary: First package workflow through M1b inference, cook, and try
---

# First Package

This guide uses the M1b package-authoring path: infer a recipe from a small
Cargo source tree, inspect the materialized recipe, build a CCS artifact, and
try it with an explicit keep or rollback decision. The command flow is covered
by `apps/conary/tests/packaging_m1b.rs`.

## Create A Tiny Cargo Project

Create a small Cargo binary project and run the remaining commands from that
project directory. The tested fixture uses this shape:

```text
hello-m1b/
  Cargo.toml
  src/main.rs
```

```toml
[package]
name = "hello-m1b"
version = "0.1.0"
edition = "2021"
```

```rust
fn main() {
    println!("hello m1b");
}
```

`Cargo.toml` and `src/main.rs` are enough for the Cargo detector in
`crates/conary-core/src/recipe/inference/`.

## Materialize The Inferred Recipe

```bash
conary new --from . --explain
```

This writes `recipe.toml` in the source tree and prints the inference decisions
that led to it. Open `recipe.toml` before building and check the package name,
version, source, and Cargo build/install steps.

## Build The Package

```bash
conary cook . --output ./dist --source-cache ./cache
```

The output directory receives a `.ccs` package artifact. Use the actual artifact
filename from `./dist` in the next command.

## Try The Artifact

```bash
conary try ./dist/<artifact>.ccs -- /usr/bin/hello-m1b
```

`conary try` records an active try session in the selected database/runtime and
runs the command inside that try context. End the session by choosing one of:

```bash
conary try rollback
```

or:

```bash
conary try keep
```

`rollback` discards the active try session for that selected database/runtime;
`keep` promotes the try generation there. Do not start another mutating Conary
operation against the same runtime until one of those decisions succeeds.

## Test Coverage

The M1b integration tests in `apps/conary/tests/packaging_m1b.rs` back this
guide's Conary commands:

- `new_from_local_tree_then_cook_recipe_builds_same_package` covers
  `conary new --from .`, materialized `recipe.toml`, and cooking that recipe.
- `cook_local_cargo_tree_from_inference_builds_ccs` covers inferred
  `conary cook . --output ./dist --source-cache ./cache`.
- `try_package_creates_session`, `try_rollback_clears_session`, and
  `try_keep_promotes_generation` cover `conary try`, `conary try rollback`,
  and `conary try keep` against the selected runtime state.
