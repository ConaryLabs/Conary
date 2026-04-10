---
last_updated: 2026-04-09
revision: 2
summary: Design for removing unsupported NixOS deployment/support messaging from active docs while preserving legitimate product comparisons to Nix/NixOS and leaving unrelated implementation details intact
---

# Unsupported Nix/NixOS Cleanup

## Context

Conary no longer supports the old NixOS-based Remi deployment story, but the
repository still contains active references that imply otherwise.

Today, the tree mixes three different kinds of `nix`/`nixos` mentions:

- stale deployment/runtime references such as `deploy/CLOUDFLARE.md` pointing
  at `deploy/nixos/remi.nix`
- active product and marketing copy comparing Conary to Nix or NixOS as other
  package-manager models
- implementation details that use the Rust `nix` crate for namespaces, mounts,
  signals, chroot, and related Linux primitives

Those categories should not be treated the same way. The deployment/runtime
references create a false support story. The comparison copy can stay when it is
clearly framed as comparison rather than support guidance. The Rust crate
references do not need cleanup.

## Goal

Remove active NixOS deployment/support messaging from the repository so the docs
and site no longer imply:

- NixOS is a supported Remi deployment target
- Nix or NixOS is a supported Conary runtime or operator surface
- old NixOS-based deployment artifacts are part of the supported operator path

At the end of this cleanup, active docs should tell a consistent story:
Nix/NixOS is not a supported Conary deployment or product surface.

## Non-Goals

This cleanup does not attempt to:

- remove the Rust `nix` crate from the workspace
- rename internal comments such as "Nix-style substituter chain" when they
  describe an implementation pattern rather than a support claim
- rewrite historical archives unless an active doc still depends on them
- remove legitimate Conary-vs-Nix/NixOS comparison material when it is clearly
  framed as product comparison rather than support guidance

## Decision

Adopt a support purge rather than a total mention purge.

Specifically:

- delete the stale NixOS deployment artifact under `deploy/nixos/`
- remove active deployment docs that mention NixOS as an operator option
- keep legitimate user-facing comparisons to Nix/NixOS where they help explain
  Conary's package-manager model
- remove any wording that turns those comparisons into implied support,
  deployment guidance, or an active integration story
- leave code-level `nix` crate references and other implementation details
  untouched

This means active docs may still say "Conary differs from Nix/NixOS in X/Y/Z,"
but they should not say or imply "Conary supports Nix/NixOS deployment" or keep
stale operational breadcrumbs for that path.

## Options Considered

### 1. Support purge, keep legitimate comparisons

Delete stale deployment artifacts and remove support/deployment wording while
keeping legitimate comparisons to Nix/NixOS.

Pros:

- clearest support boundary
- preserves useful conceptual framing
- removes dead operator breadcrumbs entirely

Cons:

- requires judgment about which comparisons are explanatory versus misleading

### 2. Hard purge all active Nix/NixOS mentions

Pros:

- maximum clarity
- no judgment calls about borderline phrasing

Cons:

- throws away useful conceptual comparisons the project still wants
- unnecessary churn in active marketing/docs copy

### 3. Archive the old NixOS material and leave active copy with disclaimers

Move the NixOS deployment artifact to a historical subtree and rewrite active
docs to say unsupported.

Pros:

- preserves historical traceability

Cons:

- more process for little value
- leaves more stale material around
- still requires nearly all active copy to change

Recommended option: `1`.

## Scope

### In scope

- active docs that mention NixOS as supported deployment or live operator
  guidance
- active docs or site copy that blur the line between product comparison and
  supported Nix/NixOS operation
- stale deployment artifacts that exist only to support the retired NixOS path

### Out of scope

- `Cargo.toml` / `Cargo.lock` dependency entries for the Rust `nix` crate
- source comments and code identifiers referring to `nix` APIs
- tests that mention `nixos` only to reject or ignore it as unsupported

## Proposed Changes

### 1. Remove the stale NixOS deployment artifact

Delete:

- `deploy/nixos/remi.nix`

This file encodes a retired deployment path and should not remain in the active
tree as if it were a supported operator option.

### 2. Remove active deployment-doc references to NixOS

Clean up deployment/operator docs so they no longer point readers at NixOS
material.

Known target:

- `deploy/CLOUDFLARE.md`

If any other active operator docs still describe NixOS deployment or Nix-backed
Remi hosting as current, remove those references as well.

### 3. Keep comparisons, remove support implications

Review active user-facing docs and site pages that compare Conary to Nix/NixOS.
Keep them if they are plainly comparative. Rewrite them only when they drift
into support, deployment, or integration implications.

Known targets:

- `README.md`
- `docs/conaryopedia-v2.md`
- `site/src/routes/+page.svelte`
- `site/src/routes/features/+page.svelte`
- `site/src/routes/compare/+page.svelte`

This includes:

- keeping feature/comparison tables or sections when they are honest product
  comparisons
- removing or rewriting any line that suggests Conary currently supports Nix or
  NixOS as a deployment/runtime target
- removing "see our NixOS deployment" style breadcrumbs or any equivalent
  operator guidance

### 4. Preserve implementation details that are not support claims

Do not change:

- workspace dependency declarations on the Rust `nix` crate
- internal comments that use "Nix-style" as shorthand for an implementation
  pattern
- negative tests that confirm `nixos` is not a supported distro flavor

## Verification

The cleanup should be verified with repository grep rather than assumption.

At minimum:

- grep active docs/site files for `nix` / `nixos` before and after edits
- confirm deleted deployment artifacts are no longer tracked
- confirm any remaining `nix` mentions in active materials are implementation
  details rather than support/comparison claims

Representative checks:

```bash
rg -n -i '\bnixos\b|\bnix\b' README.md docs deploy site/src/routes
git ls-files deploy/nixos
```

## Success Criteria

- no active deployment docs mention NixOS as a supported Remi path
- `deploy/nixos/remi.nix` is removed from the tracked tree
- active product docs and site pages may still compare Conary to Nix/NixOS, but
  none of that copy implies support, deployment guidance, or integration
- code-level `nix` crate usage remains untouched
- any remaining `nix`/`nixos` mentions are clearly implementation-internal or
  comparison-only cases, not support claims
