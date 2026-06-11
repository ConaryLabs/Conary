---
last_updated: 2026-06-10
revision: 1
summary: Normative specification of the static (file-based) Conary repository format — layout, conary-repo.toml, index.json, TUF metadata profile, publish algorithm, client behavior, and operator key lifecycle. M0 deliverable of the packaging toolchain design.
---

# Conary Static Repository Format Specification v1

## Overview

A static Conary repository is a directory of files servable by any dumb HTTP
server (nginx, GitHub Pages, S3 bucket, `file://` path) with no server-side
logic. It carries CCS packages, a package index (`index.json`), and TUF
1.0.31 metadata that protects everything a client consumes. Producers:
`conary publish` (M1a) and, later, Remi (M2). Consumer: the conary client
(`repo add` / sync / install).

Parent design: `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`.

## Scope

In scope: directory layout; `conary-repo.toml`; `index.json` schema; the TUF
metadata profile (roles, keys, filenames, expirations); the publish
algorithm including atomic-upload ordering and `--refresh`; client add /
update / install / reset-trust behavior; operator key lifecycle.

Out of scope (deferred): chunk-level delta fetch semantics (`chunks/` layout
is reserved here, semantics defined with delta publishing); TUF delegations
(not implemented; `delegations` is absent from targets metadata in v1);
consistent-snapshot versioned filenames (v2 path, §11); Remi's push/upload
protocol (M2); repo federation.

## Terminology

MUST / MUST NOT / SHOULD / MAY per RFC 2119. "Operator" = the person or CI
system that publishes the repo. "Client" = the conary package manager.
"Canonical JSON" = OLPC canonical form (sorted keys, no whitespace), as
implemented in `crates/conary-core/src/json.rs`.

## 1. Repository Layout

    <repo-root>/
      conary-repo.toml                  # identity + trust fingerprints (§2)
      index.json                        # package index (§3); TUF-protected target
      keys/
        package-keys.json               # package-signing pubkeys (§6.4); TUF target
      metadata/
        root.json                       # latest root, for bootstrap (copy of N.root.json)
        1.root.json                     # every historical root version, immutable
        2.root.json                     #   (present only after a rotation)
        targets.json                    # unversioned filenames (consistent_snapshot=false)
        snapshot.json
        timestamp.json
      packages/
        <name>/
          <name>-<version>-<release>-<arch>.ccs
      chunks/                           # RESERVED (§11); absent in v1 repos

Rules:

- All paths are repo-root-relative; URL = `<repo-url>/<path>` with no
  rewriting. A repo MUST be fully functional when served as plain files.
- `packages/` artifacts and `{N}.root.json` files are **immutable once
  published**: a publish MUST NOT overwrite an existing `.ccs` or historical
  root. Same name-version-release-arch republished with different content
  MUST be rejected by the publisher.
- `index.json`, `keys/package-keys.json`, `metadata/targets.json`,
  `metadata/snapshot.json`, `metadata/timestamp.json`, and
  `metadata/root.json` are replaced atomically per publish (§5.3).
- Clients MUST support `http://`, `https://`; and MUST support `file://`
  URLs and bare local paths for every fetch in this spec (repo identity,
  TUF metadata, index, packages). Implementation note (M1a): this requires
  lifting HTTP-only checks in `repository/client.rs::validate_url_scheme`,
  `recipe/kitchen/archive.rs::download_file`, and adding a filesystem
  fallback to `trust/client.rs` metadata fetching.
