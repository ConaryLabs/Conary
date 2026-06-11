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

## 2. Repo Identity: `conary-repo.toml`

Human-readable identity plus the trust fingerprints used at `repo add` time.
This file is **not** TUF-protected (it is what you read before trust exists);
it MUST NOT carry key material, URLs, or anything a client uses after add.
Everything security-relevant in it is cross-checked against root.json (§6.1).

    schema = 1

    [repo]
    name = "acme-tools"
    description = "ACME internal tools for conaryOS"

    [trust]
    # TUF root-role key IDs: SHA-256 of the OLPC-canonical JSON of each root
    # TufKey, lowercase hex (64 chars) — identical to the key IDs that appear
    # in root.json. These are the values `conary repo add --fingerprint`
    # checks against. (The value below is illustrative — it is
    # sha256("abc") — not a real key ID.)
    root_key_ids = [
      "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    ]

Field rules:

- `schema` (integer) MUST be `1`. Clients MUST reject unknown majors.
- `repo.name`: `[a-z0-9][a-z0-9-]*`, max 64 chars. Shown at add time; the
  client-side repo name remains whatever the user passed to `repo add`.
- `trust.root_key_ids`: non-empty array. MUST exactly equal (as a set) the
  keyids of the `root` role in the current root.json. A mismatch is a hard
  client error (§6.1) and a publisher lint error (§5.4).
- Unknown keys/tables MUST be ignored by clients (forward compatibility).

The fingerprint format everywhere in this spec is the bare 64-hex key ID.
Operators SHOULD publish it out-of-band (website, README) for
`--fingerprint` verification.
