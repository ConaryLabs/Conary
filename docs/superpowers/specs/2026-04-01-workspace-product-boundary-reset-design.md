---
last_updated: 2026-04-01
revision: 2
summary: Reset the workspace around product-owned app crates, package-local features, and smaller shared boundaries
---

# Workspace Product-Boundary Reset

## Summary

This design resets the Conary repository around the products people actually
run instead of around the historical shape of the workspace.

The repository root becomes a virtual Cargo workspace only. Each shipped tool
or service becomes its own app package with its own binary, dependencies, and
feature decisions:

- `apps/conary/` -> package `conary`, binary `conary`
- `apps/remi/` -> package `remi`, binary `remi`
- `apps/conaryd/` -> package `conaryd`, binary `conaryd`
- `apps/conary-test/` -> package `conary-test`, binary `conary-test`
- `crates/conary-core/` -> shared domain and infrastructure library

The design also removes the current root-level `server` feature model. Features
become package-local only. App crates own app behavior; shared crates own only
genuinely shared logic.

The goal is not a cosmetic rename pass. The goal is to make the workspace
structure match the mental model a future Rust developer will naturally bring
to the repository in 2026.

## Problem Statement

Conary started from a shape that made sense early on, but the repository has
grown into a state where the package, feature, and module boundaries no longer
teach the right story.

The main issues are structural:

- the repository root is both the workspace hub and the main CLI package
- the root package owns a `server` feature that conceptually controls another
  package
- `conary-server` contains two separate products, `remi` and `conaryd`
- `conary-core` has accumulated both shared logic and app-owned orchestration
- several top-level files and modules are large enough that ownership is no
  longer obvious
- architecture docs still describe the current shape as if it were intentional
  and easy to reason about

This creates practical confusion for maintenance:

- `cargo` commands do not behave where a maintainer expects
- package names do not line up cleanly with binaries or product surfaces
- feature flags cross conceptual ownership boundaries
- future refactors are harder because the package graph does not reflect real
  product boundaries

One concrete example is enough to show the mismatch: a maintainer would
reasonably expect `cargo test --features server -p conary-server` to work, but
it fails because the `server` feature is defined on the root package rather
than on the package whose name suggests it owns the server surface.

## Goals

- Make the repository root a virtual workspace only.
- Align packages with real user-facing products and binaries.
- Keep features package-local and conceptually honest.
- Reduce `conary-core` to shared domain and infrastructure concerns.
- Move product-owned orchestration back into the app that owns it.
- Improve Cargo ergonomics so common build and test commands become obvious.
- Use naming and layout that fit modern Rust workspace expectations.
- Create a stable architecture map that future contributors can follow.

## Non-Goals

- Preserving internal crate names or layout for compatibility.
- A deep micro-crate decomposition of every subsystem in the same pass.
- Redesigning package-manager behavior unrelated to workspace structure.
- Large schema, protocol, or on-disk format changes unless a move requires it.
- A long-lived compatibility layer that preserves the old structure.
- Keeping `main` shippable at every intermediate migration step.

## Current State

### Workspace Shape

The current workspace has four packages:

- root package `conary`
- `conary-core`
- `conary-server`
- `conary-test`

The root package is also the workspace root, which mixes two separate concerns:

- workspace orchestration
- product packaging for the main CLI

### Current Feature Model

The current root `Cargo.toml` defines:

- `server = ["dep:conary-server", "conary-core/server"]`
- `polkit = ["conary-server/polkit"]`
- `experimental = []`

This means the root package acts as a feature switchboard for other packages.
That is legal Cargo, but it is a poor mental model for long-term maintenance.

### Current Product Bundling

`conary-server` currently contains both:

- the Remi server
- the `conaryd` daemon

That package exports two binaries:

- `remi`
- `conaryd`

This makes one package own two separate operational products with different
responsibilities, dependencies, and future change patterns.

### Current Shared-Library Drift

`conary-core` is the shared library, but it currently contains optional
integration seams such as:

- `conary-core/server`
- `conary-core/mcp`
- `conary-core/composefs-rs`

Some of those are reasonable optional integrations. Some are signals that app-
specific helpers leaked into the shared layer.

### Current Hotspots

Several files are large enough that ownership and boundaries are already under
strain:

- `src/main.rs`
- `src/commands/mod.rs`
- `src/commands/install/mod.rs`
- `src/commands/ccs/install.rs`
- `conary-server/src/daemon/routes.rs`
- `conary-server/src/server/routes.rs`
- `conary-test/src/engine/runner.rs`

Large files are not inherently wrong, but at this scale they are correlated
with mixed responsibility and architecture drift.

## Design Principles

### 1. Organize Around Products First

The first boundary should answer: what executable or service does this package
produce?

If a package exists primarily to build or run a user-facing tool, it should be
an app crate. If code is shared across multiple tools, it belongs in a shared
crate only when the sharing is real and stable.

### 2. Features Belong To The Package They Change

Features should modify the package that defines them. They should not be used
as a conceptual indirection layer for another app package.

Package-local features are easier to reason about, easier to test, and closer
to how experienced Rust developers expect a workspace to behave.

### 3. Shared Crates Should Stay Boring

`conary-core` should feel like a dependable shared library, not like a dumping
ground for everything that has not found a better home yet.

Shared crates should bias toward:

- reusable domain logic
- shared data models
- storage and transaction primitives
- repository clients
- format parsing and verification

They should avoid owning app-specific routing, dispatch, or service wiring.

### 4. Add Support Crates Only When They Remove Real Friction

The right answer is not to explode the workspace into many small crates.
However, if one narrow support area is clearly shared by multiple apps and does
not belong in `conary-core`, a small support crate is preferable to polluting
the core library.

The most likely candidate is a tiny shared MCP helper crate if both Remi and
`conary-test` continue to share transport-agnostic MCP helpers.

## Approach Options

### Option 1: Conservative Cleanup Inside The Current Workspace

Keep the four-package shape. Improve docs, split files, and make feature flags
less surprising without changing the product/package layout.

Pros:

- smallest migration
- lowest short-term churn

Cons:

- preserves the root-package dual role
- preserves the two-products-in-one-package server story
- keeps the mental model mismatch mostly intact

### Option 2: Product-Boundary Workspace Reset

Turn the root into a virtual workspace and map each product to its own app
package. Reduce `conary-core` to shared logic and add at most a very small
support crate when justified.

Pros:

- best alignment between packages and products
- cleaner Cargo ergonomics
- easier onboarding for future maintainers
- resolves the current feature-ownership mismatch directly

Cons:

- significant repo churn
- requires a coordinated doc and CI update

### Option 3: Deep Library Decomposition Immediately

Reset the workspace and also split `conary-core` into many specialized library
crates in the same pass.

Pros:

- potentially very clean final graph on paper

Cons:

- high yak-shave risk
- likely to overfit before product boundaries stabilize
- harder to review and verify honestly

## Chosen Direction

Choose Option 2.

Do one decisive product-boundary workspace reset first. Only add small support
crates when they remove real confusion. Postpone any deeper library
decomposition until the new product boundaries have proven themselves.

## Target End State

### Workspace Layout

The repository root becomes a virtual workspace:

```text
Cargo.toml                  # workspace only
apps/
  conary/
  remi/
  conaryd/
  conary-test/
crates/
  conary-core/
  conary-mcp/              # optional; only if shared MCP helpers remain
```

The initial reset should assume only `crates/conary-core/` is required. A new
support crate should be added only if, during the migration, it becomes clear
that two or more apps share code that is not domain logic and does not belong
in `conary-core`.

### App Package Responsibilities

#### `apps/conary`

Owns:

- CLI entrypoint
- clap argument definitions
- command dispatch
- operator-facing orchestration
- any logic that is specific to the `conary` UX rather than to shared domain
  behavior

Does not own:

- shared DB models
- package parsing
- repository protocols
- trust primitives
- generic transaction and generation primitives

#### `apps/remi`

Owns:

- Remi public and admin HTTP surfaces
- Remi-specific service wiring
- federation service behavior
- server config loading specific to Remi deployment
- Remi operational binaries and startup

Does not own:

- daemon-specific socket/job/process management
- generic repository or package-domain logic that belongs in shared code

#### `apps/conaryd`

Owns:

- daemon API surface
- daemon socket activation and process lifecycle
- daemon job queue and local orchestration
- daemon-specific auth and transport concerns

Does not own:

- Remi HTTP service wiring
- shared package-manager domain logic

#### `apps/conary-test`

Owns:

- test manifest and distro config loading
- test engine and runner orchestration
- container and VM test orchestration
- test reporting and server surfaces

Does not own:

- shared package-manager domain models
- Remi-specific production server logic

### Shared Package Responsibilities

#### `crates/conary-core`

Owns:

- database layer and models
- repository clients and parsing
- package format parsing
- resolver logic
- transaction primitives
- generation building primitives
- trust and provenance primitives
- recipe/build infrastructure
- reusable filesystem and CAS logic

Should not own:

- top-level CLI dispatch
- Remi route wiring
- daemon route wiring
- test harness orchestration
- MCP transport or service wiring that is not fundamentally domain logic

#### Optional `crates/conary-mcp`

Only create this crate if the current `conary-core/mcp` helpers remain shared
between multiple apps after the reset.

If created, it should stay small and transport-agnostic:

- shared `server_info` helpers
- shared MCP JSON formatting helpers
- shared request/response utility code

It should not become a generic dumping ground for unrelated service code.

Create this crate only if all of the following are true:

- at least two app packages still depend on the helpers after ownership cleanup
- the helpers are transport-agnostic and not product policy
- moving them into `conary-core` would make `conary-core` less domain-focused
- duplicating them in app crates would create meaningful maintenance friction

## Feature Model

### Root Workspace

The workspace root defines no package features because it is no longer a
package.

### App Packages

Each app package owns only its own optional behavior.

Examples of the intended command ergonomics:

- `cargo build -p conary`
- `cargo build -p remi`
- `cargo build -p conaryd`
- `cargo build -p conary-test`
- `cargo run -p conary -- ...`
- `cargo run -p remi -- ...`

### `conary-core`

Keep only truly local optional features.

Current candidates:

- `composefs-rs`: keep only if there is still a real supported mode where core
  must build without the composefs-backed implementation or where benches/tests
  intentionally exercise both backends; otherwise remove the feature and make
  the chosen implementation unconditional
- `mcp`: likely remove from `conary-core`; replace with an optional small
  support crate if the sharing is still real
- `server`: remove; server-specific code should either become unconditional
  shared code or move into app-owned/shared support layers

### Naming Rule

No package should need another package's feature flag to make conceptual sense.

If a future developer has to remember "turn on package A's feature so package B
works the way its name implies," the boundary is wrong.

## Module Boundary Rules

The workspace reset should use the following ownership rules.

### Rule 1: Apps May Depend On Shared Crates, Not On Other Apps

`conary`, `remi`, `conaryd`, and `conary-test` should not depend on one
another as application packages.

If two apps share code, extract the shared code into a shared crate rather than
creating app-to-app coupling.

### Rule 2: Shared Crates Must Expose Intentional APIs

Shared crates should expose clear public APIs. App crates should not reach
deeply into private internals out of convenience.

### Rule 3: Large Mixed-Responsibility Files Must Be Split When Ownership Moves

This reset is the right time to split the worst files where ownership is
already muddy, especially:

- root CLI dispatch and command registration
- mixed install orchestration files
- Remi route and daemon route mega-files
- `conary-test` runner/service hotspots

The split should follow ownership boundaries, not arbitrary line counts.

### Rule 4: Docs Must Describe The New Mental Model

Architecture docs, README build/test instructions, and contributor guidance
must be updated as part of the refactor. The docs are part of the boundary
contract.

## Migration Strategy

This refactor is intentionally a reset branch, not a piecemeal compatibility
exercise.

### Phase 1: Workspace Reset

- convert the root `Cargo.toml` into a virtual workspace manifest
- create `apps/` and `crates/` directories
- move the current root package into `apps/conary`
- move `conary-core` into `crates/conary-core`
- move `conary-test` into `apps/conary-test`
- split `conary-server` into `apps/remi` and `apps/conaryd`

At the end of this phase, the repository layout should already teach the right
product story even if some internals are still temporarily awkward.

### Phase 2: Package And Feature Reset

- remove the root-level `server` feature entirely
- rename package manifests and binary targets to match their product names
- make each app package own its own dependencies directly
- remove cross-package feature indirection
- re-evaluate `conary-core` features and delete the ones that no longer match
  the new structure

### Phase 3: Boundary Cleanup

- move app-owned orchestration back out of `conary-core`
- add one small support crate only if shared helper duplication justifies it
- split the largest mixed-responsibility files while moving ownership
- normalize import paths and re-export surfaces

### Phase 4: Tooling, Docs, And CI Reset

- update README and contributor docs
- update architecture docs
- update build, test, and CI commands
- normalize developer workflows around package-owned commands
- update non-Cargo release and packaging surfaces that assume the old layout,
  including shell completions, manpage generation, packaging scripts, deploy
  helpers, and any path-sensitive release automation

## Verification Strategy

The migration must prove more than "the tree compiles."

### Structural Verification

- `cargo metadata` shows the intended workspace graph
- each app package builds directly by package name
- no root package features remain
- package dependencies match the intended ownership model

### Behavioral Verification

- `cargo test -p conary`
- `cargo test -p conary-core`
- `cargo test -p remi`
- `cargo test -p conaryd`
- `cargo test -p conary-test`

Where packages share coverage today through workspace commands, add package-
specific verification so the new boundaries are exercised honestly.

### Operational Verification

- the main CLI still runs as `conary`
- the Remi server still runs as `remi`
- the daemon still runs as `conaryd`
- test harness commands still run as `conary-test`

### Documentation Verification

- README commands match actual package names
- architecture docs describe the new graph correctly
- contributor docs no longer refer to root-level feature toggles that do not
  exist

## Risks And Mitigations

### Risk: Refactor Churn Hides Real Regressions

Mitigation:

- keep the reset structured in phases
- run package-specific tests after each major ownership move
- avoid mixing unrelated behavior changes into the same branch

### Risk: `conary-core` Remains Too Broad Even After The Reset

Mitigation:

- accept that `conary-core` may still be broad after the first reset
- evaluate further decomposition only after product boundaries stabilize

### Risk: Overusing Small Support Crates

Mitigation:

- add support crates only when multiple apps clearly need shared code that does
  not belong in `conary-core`
- prefer one obvious support crate over several speculative ones

## Expected Outcome

After this reset, a future maintainer should be able to infer the workspace
shape from package names alone:

- `conary` is the package manager CLI
- `remi` is the package server
- `conaryd` is the local daemon
- `conary-test` is the test harness
- `conary-core` is shared infrastructure

That clarity is the point of the refactor. The best outcome is not simply
fewer lines or fewer crates. The best outcome is that the repository finally
looks like the system it actually is.
