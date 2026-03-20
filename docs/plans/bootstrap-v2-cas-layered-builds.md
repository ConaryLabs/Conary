---
last_updated: 2026-03-19
revision: 1
summary: Design note for CAS-layered bootstrap replacing LFS-style mutable sysroot
---

# Bootstrap v2: CAS-Layered Immutable Build Pipeline

## Context

Phase 1 cross-toolchain builds successfully with the current LFS-style pipeline
(March 19, 2026). But the experience exposed fundamental problems with the
mutable-sysroot approach: DESTDIR confusion, PATH ordering sensitivity, ambient
environment pollution, and no isolation between build stages. Every flag matters,
every env var matters, and debugging is hours of "which thing leaked where."

Meanwhile, we already have the infrastructure for something better:
- **CAS** (content-addressable storage) for files
- **EROFS** image building from file hashes
- **composefs** mounts with CAS as content backend
- **Generation system** for immutable system snapshots
- **Sandbox/namespace isolation** in the container module

## Core Idea

Instead of building into a mutable `/mnt/lfs` root, each package build produces
a set of files stored in CAS. Build stages are composed as immutable EROFS
overlays. The "sysroot" for each build IS a composefs mount of the previous
stage's packages — hermetic by construction, not by careful scripting.

```
Package build:
  Input:  EROFS image (previous stage) + source tarball
  Output: Set of files in CAS (keyed by content hash)

Stage composition:
  Stage N = EROFS(packages from stage N) + composefs(CAS)
  Stage N+1 builds AGAINST Stage N as an immutable mount
```

## What Changes

**Old model (current):**
```
Host -> cross-compile -> /mnt/lfs (mutable)
  Each package modifies /mnt/lfs in place
  Environment leaks between builds
  Failed builds corrupt the sysroot
```

**New model:**
```
Host -> cross-compile -> CAS objects
  Each package produces isolated file set
  Build runs in mount namespace against EROFS overlay
  Failed builds produce nothing (no corruption)
  Stages are immutable snapshots
```

## Key Benefits

1. **No DESTDIR confusion** — builds install to `/` inside the mount namespace,
   files are captured to CAS after install
2. **No PATH pollution** — the mount namespace only contains declared dependencies
3. **Hermetic by construction** — if a dependency isn't in the EROFS image, it
   doesn't exist, period
4. **Failed builds are free** — nothing was modified, just discard the output
5. **Incremental rebuilds** — only rebuild packages whose inputs changed
   (source hash + dependency hashes)
6. **Reproducible** — same inputs = same EROFS image = same CAS hashes
7. **Already built** — CAS, EROFS, composefs, Sandbox are all working code

## Existing Infrastructure to Build On

- `conary-core/src/filesystem/cas.rs` — CAS store (SHA256-keyed)
- `conary-erofs/` — EROFS image builder
- `conary-core/src/generation/` — composefs mount, EROFS from DB hashes
- `conary-core/src/container/` — Sandbox with mount/pid/user namespaces
- `conary-core/src/recipe/kitchen/` — recipe execution engine (working)
- `conary-core/src/bootstrap/build_runner.rs` — source fetch + checksum
- `recipes/` — 114 LFS 13-aligned recipes (cross-tools, temp-tools, system, tier2)

## Build Seed

There was earlier work on a binary build seed concept — a minimal trusted binary
that bootstraps the first stage without requiring a host compiler. This aligns
with the Guix approach (bootstrap from minimal binary seed) and with
reproducible-builds.org guidance. Worth exploring whether the CAS-layered model
can start from a verified seed rather than trusting the host toolchain.

Check: `conary-core/src/bootstrap/` for any seed-related code, and git history
for "seed" or "binary seed" commits.

## Relationship to Nix/Guix

This is essentially the Nix store model implemented with EROFS/composefs:
- Nix store path = CAS hash
- Nix derivation = recipe + input hashes
- Nix profile = EROFS generation
- Nix sandbox = our Sandbox/namespace isolation

The difference: we use EROFS as the on-disk format (which gives us composefs
integration, fs-verity, and kernel-level integrity) rather than a directory tree.

## Open Questions

- How to handle the cross-compilation bootstrap? The first stage (cross-tools)
  must build against the HOST, not an EROFS image. Transition point is when
  we enter the chroot — that's where CAS-layered builds start.
- Should each package be its own EROFS layer, or should stages be monolithic
  images? Per-package layers enable incremental rebuilds but add composition
  overhead.
- How does this interact with the existing generation system? A bootstrap stage
  IS a generation — can we reuse the generation builder?
- Performance: composefs mount per build step vs. one mount per stage?

## Next Steps

1. Brainstorm the detailed design (spec)
2. Prototype: build one package (e.g., binutils) into CAS, compose as EROFS,
   mount as composefs, build next package against that mount
3. If prototype works, migrate Phase 2+ to CAS-layered model
4. Phase 1 (cross-tools) stays LFS-style since it builds against the host
