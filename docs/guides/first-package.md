---
last_updated: 2026-07-25
revision: 3
summary: First package workflow through explicit recipe authoring, signed cook, and try
---

# First Package

Conary builds source projects from an explicit `recipe.toml`. It never guesses
a build system, package identity, source URL, or command sequence from marker
files. This guide scaffolds a named recipe, fills in the exact Cargo contract,
builds a signed CCS artifact, and tries it with an explicit keep or rollback
decision.

## Create The Project And Recipe

Start with a small Cargo binary project:

```text
hello-m1b/
  Cargo.toml
  src/main.rs
```

From the directory above `hello-m1b/`, scaffold the recipe into the project:

```bash
conary new hello-m1b --output ./hello-m1b
```

Edit `hello-m1b/recipe.toml` so the source and build contract are explicit:

```toml
[package]
name = "hello-m1b"
version = "0.1.0"
release = "1"
summary = "hello-m1b"
license = "MIT"

[source]
path = "."

[build]
make = "cargo build --release"
install = "mkdir -p %(destdir)s/usr/bin && install -m 0755 target/release/hello-m1b %(destdir)s/usr/bin/hello-m1b"
```

`conary new` requires the package name. It does not accept `--from` or
`--explain`, and it does not inspect the source tree to author commands for
you.

## Build A Signed Package

Create a development package key, then cook the explicit recipe:

```bash
conary ccs keygen --output ./hello-m1b/package-key
conary cook ./hello-m1b/recipe.toml \
  --output ./hello-m1b/dist \
  --source-cache ./hello-m1b/cache \
  --key ./hello-m1b/package-key.private
```

Put the base64 public key printed by `ccs keygen` into the trust policy used
for local verification:

```toml
trusted_keys = ["<base64-public-key>"]
```

Save that file as `hello-m1b/package-policy.toml`.

Passing the project directory instead of the file is also exact:
`conary cook ./hello-m1b` succeeds only because that directory contains
`recipe.toml`. A bare source tree, archive, or URL is not a cook target.
Foreign binary package files remain a separate typed conversion input.

## Try The Artifact

Use the actual artifact filename from `hello-m1b/dist`:

```bash
conary try ./hello-m1b/dist/<artifact>.ccs \
  --policy ./hello-m1b/package-policy.toml \
  -- /usr/bin/hello-m1b
```

Direct package tries accept only current signed CCS artifacts verified by the
explicit policy. Watch mode instead derives its one-key trust policy from the
required `--key` used for every cook.

End the active session with one explicit decision:

```bash
conary try rollback
```

or:

```bash
conary try keep
```

`rollback` discards the try generation in the selected runtime; `keep`
promotes it. Do not start another mutating Conary operation against that
runtime until one of those decisions succeeds.

## Proof

- `crates/conary-core/src/recipe/scaffold.rs` proves deterministic named
  scaffolding and materialization.
- `apps/conary/src/commands/cook/tests.rs` proves only recipe files and
  directories containing `recipe.toml` are accepted.
- `apps/conary/tests/packaging_m1b.rs` proves the removed inference flags and
  bare-source targets fail, and covers try start, rollback, and keep.
