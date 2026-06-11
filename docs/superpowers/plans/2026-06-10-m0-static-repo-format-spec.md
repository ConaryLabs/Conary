# M0: Static Repo Format Child Spec — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce `docs/specs/static-repo-format-v1.md` — the normative specification for TUF-protected static Conary repositories — satisfying every M0 requirement in the parent spec (`docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`, "Static repo spec" section). M0 is the hard gate before any M1a publish/repo-add code.

**Architecture:** This milestone's deliverable is a document, not code. Tasks write spec sections whose schemas/algorithms are fully specified *in this plan*; verification steps are `grep`/read checks proving every wire format, type name, and function the spec cites actually exists in the codebase with the stated shape. TDD adapts as: claim → verify against code → commit.

**Tech Stack:** Markdown spec (conventions of `docs/specs/ccs-format-v1.md`); grounded against `crates/conary-core/src/trust/` (TUF 1.0.31 impl), `repository/metadata.rs`, `ccs/signing.rs`.

---

## Decisions made in this plan (refinements the child spec records)

The parent spec delegated these to M0. This plan decides them so the spec tasks have no open questions:

1. **`consistent_snapshot = false` in v1.** The existing `TufClient` fetches unversioned `timestamp.json`/`snapshot.json`/`targets.json` and only versioned `{N}.root.json` (rotation probing in `trust/client.rs::check_root_rotation`). Mandating versioned snapshot/targets filenames would require client fetch changes beyond M1a scope. Reverse-verification-order upload + hash pinning means a torn read fails verification cleanly (retry later) — fail-safe, never mix-and-match. Versioned-filename consistent snapshot is documented as the v2 upgrade path.
2. **Two operator keypairs, not one:** `root` (signs root.json only; backup-critical; rarely used) and `publish` (fills the targets, snapshot, and timestamp roles, and signs packages + future attestations — "the same authority that signs packages" per parent spec). All role thresholds = 1 in v1. This refines the parent's "an Ed25519 keypair" (singular): without a separate root key, a publish-key compromise would be unrecoverable.
3. **`--fingerprint` value = TUF root key ID** — SHA-256 of the OLPC-canonical JSON of the root `TufKey` (exactly `trust/keys.rs::compute_key_id`), 64 lowercase hex chars. Repeatable flag; the provided set must equal the root role's keyid set.
4. **Package-signing public keys distribute via a TUF target** `keys/package-keys.json`, not via `conary-repo.toml` (parent: identity file carries fingerprints only, never key material). Client imports them into the repo's `TrustPolicy.trusted_keys` only after TUF verification.
5. **Bootstrap fetches the latest `metadata/root.json`** and verifies it self-signed against the pinned/TOFU fingerprints (the fingerprint is the trust anchor, so chain-walking from `1.root.json` is unnecessary at first contact). All historical `{N}.root.json` files remain published so already-bootstrapped clients can walk rotations.
6. **Default expirations:** root 365 d, targets 90 d, snapshot 90 d, timestamp 30 d. The 30-day timestamp trades freeze-protection window for hobby-operator viability; `conary publish --refresh <target>` re-signs metadata without rebuilding content, and publish warns when any metadata has <25% lifetime remaining.
7. **The publisher is stateless-by-default, with a rollback tripwire:** publish reads current metadata versions from the *destination* (verifying with the operator's own public keys) and derives next versions from them — but a compromised destination could replay old, validly-signed state and trick the publisher into re-signing a rollback fresh. So the publisher also keeps a local **version watermark** (beside the keys) recording the last versions it published: destination behind the watermark → hard error (`--accept-destination-state` to override, loudly). CI pipelines MAY pass `--state-file <path>` (committed to VCS) as the authoritative watermark. The watermark gates; it is never the version source.

Task 12 reflects refinements 1, 2 and 6 back into the parent spec's revision notes (one paragraph) so parent and child never contradict.

---

### Task 1: Verify grounding facts

The plan's schemas cite exact code shapes. Confirm each before writing prose. Any mismatch → STOP, report, fix plan first.

**Files:** none created; read-only checks.

- [ ] **Step 1: TUF wire format facts**

```bash
grep -n '_type\|spec_version\|consistent_snapshot\|pub struct Signed\|pub keyid\|pub sig' crates/conary-core/src/trust/metadata.rs | head -20
```
Expected: `#[serde(rename = "_type")]`; `spec_version: String`; `consistent_snapshot: bool` in RootMetadata; `Signed<T> { signed, signatures }`; `TufSignature { keyid, sig }`.

- [ ] **Step 2: generation + ceremony functions exist in conary-core (not Remi)**

```bash
grep -n 'pub fn generate_targets\|pub fn generate_snapshot\|pub fn generate_timestamp' crates/conary-core/src/trust/generate.rs
grep -n 'pub fn create_initial_root\|pub fn rotate_key' crates/conary-core/src/trust/ceremony.rs
grep -n 'pub fn compute_key_id' crates/conary-core/src/trust/keys.rs
```
Expected: all six functions present.

- [ ] **Step 3: client rotation probing + bootstrap + unversioned fetch names**

```bash
grep -n 'root.json\|timestamp.json\|snapshot.json\|targets.json' crates/conary-core/src/trust/client.rs | head -15
grep -n 'pub fn bootstrap' crates/conary-core/src/trust/client.rs
```
Expected: `format!("{next_version}.root.json")` rotation probe; literal unversioned `timestamp.json`/`snapshot.json`/`targets.json` fetches; `bootstrap(&self, conn, root_json: &[u8])`.

- [ ] **Step 4: key file format + index shapes + url validation**

```bash
grep -n 'struct KeyFile\|algorithm\|key_id' crates/conary-core/src/ccs/signing.rs | head -8
grep -n 'pub struct RepositoryMetadata\|pub struct PackageMetadata' -A 14 crates/conary-core/src/repository/metadata.rs | head -40
grep -n 'pub fn validate_url_scheme' crates/conary-core/src/repository/client.rs
```
Expected: TOML `KeyFile { algorithm, key, key_id: Option }` (base64 key); `RepositoryMetadata { name, version: String, security_advisory_source, packages }`; `PackageMetadata { name, version, architecture: Option, description: Option, checksum, size: i64, download_url, dependencies: Option, delta_from, security_advisory }`; `validate_url_scheme` ~line 108.

- [ ] **Step 5: spec-doc conventions**

```bash
head -8 docs/specs/ccs-format-v1.md && ls docs/specs/
```
Expected: YAML frontmatter `last_updated / revision / summary`; H1 title; `ccs-format-v1.md` is currently the only spec.

No commit (read-only task).

---

### Task 2: Spec skeleton — Overview, Scope, Terminology

**Files:**
- Create: `docs/specs/static-repo-format-v1.md`

- [ ] **Step 1: Write frontmatter + Overview + Scope + Terminology**

```markdown
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
```

- [ ] **Step 2: Verify file renders and frontmatter parses**

```bash
head -8 docs/specs/static-repo-format-v1.md
```
Expected: frontmatter matching ccs-format-v1.md conventions.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: scaffold static repo format spec (M0)"
```

---

### Task 3: Section — Repository Layout

**Files:**
- Modify: `docs/specs/static-repo-format-v1.md` (append)

- [ ] **Step 1: Write the layout section**

```markdown
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
  `metadata/root.json` are replaced atomically per publish (§7).
- Clients MUST support `http://`, `https://`; and MUST support `file://`
  URLs and bare local paths for every fetch in this spec (repo identity,
  TUF metadata, index, packages). Implementation note (M1a): this requires
  lifting HTTP-only checks in `repository/client.rs::validate_url_scheme`,
  `recipe/kitchen/archive.rs::download_file`, and adding a filesystem
  fallback to `trust/client.rs` metadata fetching.
```

- [ ] **Step 2: Verify the three named lift-sites are real**

```bash
grep -n 'fn validate_url_scheme' crates/conary-core/src/repository/client.rs
grep -n 'fn download_file' crates/conary-core/src/recipe/kitchen/archive.rs
grep -n 'reqwest::get' crates/conary-core/src/trust/client.rs
```
Expected: one hit each.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: static repo spec - repository layout"
```

---

### Task 4: Section — `conary-repo.toml`

**Files:**
- Modify: `docs/specs/static-repo-format-v1.md` (append)

- [ ] **Step 1: Write the identity-file section**

```markdown
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
```

- [ ] **Step 2: Verify key-ID derivation claim**

```bash
grep -n -A 4 'pub fn compute_key_id' crates/conary-core/src/trust/keys.rs
```
Expected: `canonical_json(key)` then `hash::sha256(&canonical)`.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: static repo spec - conary-repo.toml schema"
```

---

### Task 5: Section — `index.json`

**Files:**
- Modify: `docs/specs/static-repo-format-v1.md` (append)

- [ ] **Step 1: Write the index section**

```markdown
## 3. Package Index: `index.json`

The resolver's view of the repo. It is a TUF target: clients MUST verify its
sha256+length against the verified `targets.json` entry for path
`index.json` **before parsing** (§6.2). No code path may parse an unverified
index.

    {
      "schema": 1,
      "name": "acme-tools",
      "index_version": 7,
      "generated": "2026-06-10T18:00:00Z",
      "packages": [
        {
          "name": "acme-widget",
          "version": "1.4.2",
          "release": "1",
          "arch": "x86_64",
          "path": "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
          "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
          "size": 1048576,
          "description": "Widget frobnicator",
          "dependencies": ["libfoo >= 2.0"]
        }
      ]
    }

Field rules:

- `schema` (u64): MUST be `1`; reject unknown majors.
- `index_version` (u64): MUST equal the `version` of the targets metadata
  generated in the same publish (§5.3). Clients enforce the equality only;
  rollback protection is inherited from TUF targets-version monotonicity,
  so no separate `index_version` history is kept. The field exists because
  the legacy client shape (`RepositoryMetadata.version`) is a free-form
  string with no monotonicity guarantee, and the equality check is what
  binds the parsed index to the verified TUF state.
- `generated`: RFC 3339 UTC; informational only — clients MUST NOT make
  trust decisions on it (expiry lives in TUF metadata).
- Package entries: `name`, `version`, `release`, `arch` are required and
  MUST match the artifact filename `<name>-<version>-<release>-<arch>.ccs`.
  `arch` values follow `uname -m` (`x86_64`, `aarch64`, `riscv64`) or
  `noarch`. `sha256` is bare lowercase hex of the `.ccs` file; `size` in
  bytes. `path` is repo-root-relative: consumers MUST resolve it against
  the repo base, normalize per RFC 3986, and reject the entry if the
  normalized result escapes the repo root, is absolute, carries a scheme,
  or contains percent-encoded sequences decoding to `/` or `..` (a naive
  `..`-substring check both misses encoded traversals and false-positives
  on names like `foo..bar`).
  `description` and `dependencies` are optional; dependency strings use the
  CCS manifest dependency syntax.

Consistency invariants (publisher MUST enforce, client MUST check on use):

1. Every package entry has a `targets.json` entry at the identical path
   whose sha256 and length equal the index entry's `sha256`/`size`.
2. `index.json` itself and `keys/package-keys.json` have `targets.json`
   entries.
3. `index_version == targets.version`.

Mapping note (M1a client work, non-normative): index entries map onto the
existing client model `repository/metadata.rs::PackageMetadata` as
name→name, version→version, arch→architecture, sha256→checksum,
size→size, `<repo-url>/<path>`→download_url, dependencies→dependencies;
`release` rides alongside; `delta_from`/`security_advisory` are absent in
static-repo v1.
```

- [ ] **Step 2: Verify mapping claims against the client model**

```bash
grep -n -A 12 'pub struct PackageMetadata' crates/conary-core/src/repository/metadata.rs
```
Expected fields: name, version, architecture, description, checksum, size, download_url, dependencies, delta_from, security_advisory — all named in the mapping note.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: static repo spec - index.json schema"
```

---

### Task 6: Section — TUF Metadata Profile

**Files:**
- Modify: `docs/specs/static-repo-format-v1.md` (append)

- [ ] **Step 1: Write the TUF profile section**

```markdown
## 4. TUF Metadata Profile

The repo embeds TUF 1.0.31 metadata exactly as implemented in
`crates/conary-core/src/trust/` — this spec adds no wire-format extensions.

### 4.1 Wire format

Every metadata file is `{"signed": {...}, "signatures": [{"keyid": "<64-hex>",
"sig": "<128-hex>"}]}`. Signatures are Ed25519 over the OLPC-canonical JSON of
`signed`, hex-encoded. The `signed` body carries `_type`
("root"/"targets"/"snapshot"/"timestamp"), `spec_version` ("1.0.31"),
`version` (u64, monotonic per role), `expires` (RFC 3339 UTC), plus
role-specific fields:

- **root**: `consistent_snapshot` (MUST be `false` in v1), `keys`
  (keyid → `{keytype:"ed25519", scheme:"ed25519", keyval:{public:"<64-hex>"}}`),
  `roles` (role → `{keyids:[...], threshold:1}`).
- **targets**: `targets` (path → `{length, hashes:{"sha256":"<hex>"}}`).
  Delegations are absent in v1.
- **snapshot**: `meta` with entries `"root.json": {version}` and
  `"targets.json": {version, length, hashes:{"sha256":...}}`.
- **timestamp**: `meta` with entry
  `"snapshot.json": {version, length, hashes:{"sha256":...}}`.

### 4.2 Roles and keys

Two operator keypairs (Ed25519 only):

| Keypair  | TUF roles                      | Also signs                      |
|----------|--------------------------------|---------------------------------|
| root     | root                           | — (root.json only; keep offline)|
| publish  | targets, snapshot, timestamp   | packages (CCS), attestations (M2)|

All thresholds are 1 in v1. Multi-key roles and higher thresholds are valid
TUF and MAY be produced by other tooling; the conary client already
enforces arbitrary thresholds, but `conary publish` v1 only generates the
two-key layout.

### 4.3 Filenames and rotation

`targets.json`, `snapshot.json`, `timestamp.json` are served unversioned
(`consistent_snapshot = false` — the existing client only fetches
unversioned snapshot/targets filenames, and reverse-order upload + hash
pinning already fails safe; §11 covers the v2 versioned-filename path).
Roots are dual-published:
`metadata/{N}.root.json` for every version N (immutable, complete history)
plus `metadata/root.json` as a copy of the latest, used for bootstrap (§6.1).
Already-bootstrapped clients discover rotations by probing
`{current+1}.root.json` (existing client behavior).

### 4.4 Package-key distribution: `keys/package-keys.json`

    {
      "schema": 1,
      "keys": [
        {
          "algorithm": "ed25519",
          "public_key": "<base64 32-byte Ed25519 public key>",
          "key_id": "publish",
          "status": "active",
          "comment": "primary publishing key"
        }
      ]
    }

`status` is `"active"` (signs new packages) or `"retired"` (no longer
signs, still trusted so previously published artifacts keep verifying).
Clients import both into `trusted_keys`. A **compromised** key is neither:
it is removed from the file entirely, and every artifact it signed MUST be
removed or republished (re-signed, new release number) — §7.3. A `retired`
key MAY be dropped from the file once no entry in the current index
references an artifact signed by it (fully superseded).

`public_key` is base64 — matching the CCS `PackageSignature.public_key` and
`TrustPolicy.trusted_keys` encoding (`ccs/verify.rs`), which differs from
TUF's hex `keyval.public`; both encodings of the same publish key appear in
a default repo. Clients import these into the repo's package trust policy
only after TUF verification of this file (§6.2), and verify installed
packages with `allow_unsigned = false` for static repos.

Authority is **flat** in v1: every listed key may sign any package in the
repo (no per-path constraint until TUF delegations, v2). Operators SHOULD
keep a single publishing key; teams needing per-publisher authority SHOULD
partition into separate repos until delegations land.

### 4.5 Expirations (publisher defaults)

| Metadata  | Default lifetime |
|-----------|------------------|
| root      | 365 days         |
| targets   | 90 days          |
| snapshot  | 90 days          |
| timestamp | 30 days          |

Generation functions are parametric (`trust/generate.rs`); these are the
`conary publish` defaults, overridable per-publish. The 30-day timestamp is
a deliberate freeze-protection vs. operator-burden tradeoff for static
repos; see §5.5 (refresh) and §9 (threats). Expired metadata is a hard
client error whose message MUST name the remedy (operator runs
`conary publish --refresh`).
```

- [ ] **Step 2: Verify wire-name fidelity (the critical check)**

```bash
grep -n 'rename = "_type"' crates/conary-core/src/trust/metadata.rs
grep -n 'pub keytype\|pub scheme\|pub public\|pub keyids\|pub threshold\|pub length\|pub hashes\|pub meta\|pub targets\|pub keys\|pub roles' crates/conary-core/src/trust/metadata.rs
```
Expected: every field name used in §4.1 appears verbatim (serde uses field names except `_type`).

- [ ] **Step 3: Verify snapshot/timestamp meta-entry shapes match the generators**

```bash
grep -n -B2 -A8 'root.json\|targets.json\|snapshot.json' crates/conary-core/src/trust/generate.rs | head -40
```
Expected: snapshot inserts `"root.json"` MetaFile (version only) + `"targets.json"` (version, length, sha256); timestamp inserts `"snapshot.json"` (version, length, sha256) — exactly as §4.1 states.

- [ ] **Step 4: Commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: static repo spec - TUF metadata profile"
```

---

### Task 7: Section — Publish Algorithm

**Files:**
- Modify: `docs/specs/static-repo-format-v1.md` (append)

- [ ] **Step 1: Write the publish-algorithm section**

```markdown
## 5. Publish Algorithm (Producer Requirements)

Any producer (`conary publish` M1a, Remi M2, third-party tooling) MUST
behave as follows. The publisher is **stateless**: current versions are read
from the destination, not from local state.

### 5.1 Read destination state

1. Fetch `metadata/timestamp.json`, `metadata/snapshot.json`,
   `metadata/targets.json`, `metadata/root.json` from the destination.
2. All absent → initial publish (§5.2). Partially absent → destination is
   damaged; refuse unless `--force-reinit` (which re-runs §5.2 and is loud
   about it).
3. Verify fetched metadata with the operator's own public keys (the
   operator trusts their own repo; this check catches destination
   tampering/corruption before building on top of it). Verification
   failure → hard error, never silently re-sign.
4. Compare destination versions against the local version watermark
   (`~/.config/conary/keys/<repo-name>/last-published.toml`, or the
   `--state-file <path>` override for CI). Destination versions **lower**
   than the watermark → hard error naming both (a compromised or rolled-back
   destination is replaying old signed state; re-signing on top would
   launder the rollback into fresh signatures). `--accept-destination-state`
   overrides, loudly. A missing watermark (first publish from this machine)
   skips the check with a notice. Version regression implies **content**
   regression too: a publisher rebuilding from a rolled-back index would
   silently drop packages published in the hidden versions — the watermark
   gate is what prevents that, which is why overriding it is loud.
5. **Single-writer rule:** concurrent publishes to one destination are
   unsupported and MUST fail rather than interleave (two writers can both
   derive version N+1 and clobber each other). Where the backend supports
   it, the publisher SHOULD use conditional writes (S3 `If-Match`/ETag;
   atomic rename for file/rsync destinations); regardless of backend, the
   publisher MUST re-fetch `metadata/timestamp.json` immediately before
   uploading the new timestamp (§5.3.4 step d) and abort if its version
   changed since §5.1.

### 5.2 Initial publish (ceremony)

1. Ensure keys exist (§10.1); generate if absent.
2. Build root v1 via `trust/ceremony.rs::create_initial_root(root_key,
   publish_key, publish_key, publish_key, 365 days)` — publish key fills
   targets/snapshot/timestamp roles. `consistent_snapshot = false`.
3. Write `conary-repo.toml` with the root-role key IDs.
4. Proceed to §5.3 with all role versions starting at 1.

### 5.3 Incremental publish (per publish)

1. Stage new `.ccs` files under `packages/<name>/`. Refuse to overwrite an
   existing artifact path with different content (immutability, §1).
2. Build the new target set: every `.ccs` under `packages/`,
   `index.json`, `keys/package-keys.json` — each with length + sha256.
   (Compute the index bytes first: `index_version = targets_next`, where
   `targets_next = current targets.version + 1`, or 1 on initial publish.)
3. Generate metadata via `trust/generate.rs`:
   `generate_targets(targets_next)` → `generate_snapshot(root_version,
   targets, snapshot_next)` → `generate_timestamp(snapshot,
   timestamp_next)`; each role's version = its destination version + 1.
   **`root_version` is NOT a role-to-bump:** it is the version of the
   currently published root (read in §5.1) — incremented only when this
   publish itself performs a root rotation (§7.2). A snapshot pinning a
   nonexistent root version hard-fails every client's consistency check.
   Expiry-parameter footgun: `generate_timestamp` takes **`expires_hours`**
   (720 = the 30-day default) while `generate_targets`/`generate_snapshot`
   take `expires_days` — passing 30 meaning "days" yields a 30-hour
   timestamp.
4. Upload in **reverse verification order** — each step completes before
   the next begins:
   a. `packages/**` (new artifacts) and `keys/package-keys.json`
   b. `index.json` and `metadata/targets.json`
   c. `metadata/snapshot.json`
   d. `metadata/timestamp.json`
   On rotation only: new `metadata/{N}.root.json` and `metadata/root.json`
   upload during step (a).
   A client reading mid-publish sees old timestamp → old, complete state;
   or new timestamp whose hash chain only references already-uploaded
   files. Torn states fail hash verification — clients never act on them.
   One exception: during a **root rotation**, a client that probes the new
   `{N+1}.root.json` (uploaded in step a) before the new snapshot lands
   will fail snapshot consistency (old snapshot pins root vN) — a brief
   retryable window, fail-safe but not "old complete state".
5. A failed publish is re-run from the top; §5.1 re-reads whatever
   landed, and immutable artifacts already uploaded are skipped.
6. After a fully successful publish, write the new role versions to the
   local watermark (§5.1 step 4).

### 5.4 Publisher lints (MUST pass before upload)

- Index invariants of §3 hold (index↔targets path/hash/size equality,
  `index_version == targets.version`).
- `conary-repo.toml` `root_key_ids` equals the root-role keyid set.
- Every package entry's filename parses as `<name>-<version>-<release>-<arch>.ccs`
  and matches its entry fields.
- No targets path escapes the repo root (`..`, absolute, or URL paths).

### 5.5 Refresh (`conary publish --refresh <target>`)

Re-signs without content change. Roles are selected by expiry (within 25%
of lifetime), then expanded to the **minimal closed cascade set** — the
snapshot pins the root and targets versions (`generate_snapshot` writes
`meta["root.json"].version`; the client's `verify_snapshot_consistency`
checks it against the *current* root), so a role bump that isn't cascaded
strands clients on a consistency failure:

- root bump ⇒ snapshot + timestamp bump
- targets bump (also: any index/package-keys change) ⇒ snapshot + timestamp
- snapshot bump ⇒ timestamp
- timestamp always bumps

If targets is re-signed, `index_version` bumps with it and the index is
re-uploaded — invariant 3 of §3 always holds. The re-uploaded index is
byte-identical to the previous one except `index_version` and `generated`
(package entries, hashes, and sizes MUST NOT be recomputed or reordered
during a refresh). Upload ordering of §5.3.4 applies.

### 5.6 Serving and caching guidance (non-normative)

With `consistent_snapshot = false`, a client syncing mid-publish can fetch a
new `targets.json` against an old `snapshot.json` (or vice versa) and fail
hash verification — fail-safe, but a transient denial of service. CDNs with
per-file TTLs widen this window. Operators serving via CDN SHOULD set short
TTLs (≤60 s) on `conary-repo.toml`, `index.json`, `keys/package-keys.json`,
and everything under `metadata/` except `{N}.root.json` — concretely
`Cache-Control: max-age=60` (or `no-cache` for `timestamp.json`), and
`Cache-Control: public, max-age=31536000, immutable` for `packages/**` and
`{N}.root.json`. Publish pipelines targeting a CDN SHOULD invalidate
`metadata/*`, `index.json`, and `keys/*` as a post-upload step.
S3-compatible backends do not guarantee read-after-**overwrite**
consistency: each §5.3.4 step completes only when the uploaded object is
confirmed visible (read-back or ETag check) before the next step begins,
and the §5.1 destination reads SHOULD bypass caches (no-cache request
headers or cache-busting query). CDN-served
production repos are the primary motivation for the v2 consistent-snapshot
upgrade (§11).
```

- [ ] **Step 2: Verify generator signatures match the calls described**

```bash
grep -n -A 6 'pub fn generate_targets\|pub fn generate_snapshot\|pub fn generate_timestamp' crates/conary-core/src/trust/generate.rs | head -30
grep -n -A 7 'pub fn create_initial_root' crates/conary-core/src/trust/ceremony.rs
```
Expected: `generate_targets(packages, key, version, expires_days)`; `generate_snapshot(root_version, targets, key, version, expires_days)`; `generate_timestamp(snapshot, key, version, expires_hours)`; `create_initial_root(root_key, targets_key, snapshot_key, timestamp_key, expires_days)`. (Param order in spec prose is descriptive, not a code contract — but arity/inputs must match.)

- [ ] **Step 3: Commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: static repo spec - publish algorithm"
```

---

### Task 8: Section — Client Behavior

**Files:**
- Modify: `docs/specs/static-repo-format-v1.md` (append)

- [ ] **Step 1: Write the client-behavior section**

```markdown
## 6. Client Behavior

### 6.1 `repo add` (static repo detection and trust establishment)

`conary repo add <name> <url|path> [--fingerprint <64-hex>]...`

1. Probe `<url>/conary-repo.toml`. Present → static repo flow (below).
   Absent → existing repo-type flows; not this spec.
2. Static repos use TUF exclusively. Enforcement is two-stage because
   static-ness isn't knowable at parse time: clap conflict rules reject
   GPG flags (`--gpg-key`, `--no-gpg-check`, `--gpg-strict`) combined with
   `--fingerprint`; and after the probe identifies a static repo, command
   execution rejects any GPG flags that were passed without
   `--fingerprint`.
3. Fetch `<url>/metadata/root.json`; parse as `Signed<RootMetadata>`;
   verify self-signed (root-role threshold met by its own keys) and
   unexpired.
4. Compute the root-role keyid set; verify it equals
   `conary-repo.toml::trust.root_key_ids` — mismatch is a hard error
   naming both sets.
5. Trust pinning:
   - With `--fingerprint` (repeatable): provided set MUST equal the
     root-role keyid set; mismatch → hard error, nothing persisted.
   - Without: display name, description, and the keyid set; require
     explicit interactive confirmation (TOFU). Non-interactive contexts
     MUST fail instead of prompting. The confirmation text MUST note that
     TOFU cannot detect a replayed *old* root whose keys were later
     rotated or compromised — an on-path attacker can pin a stale
     identity. `--fingerprint` is the production path; TOFU is for
     casual/first-look use. "Non-interactive" is defined: stdin is not a
     terminal, or `CONARY_NON_INTERACTIVE=1` is set.
6. Persist: repository row with `tuf_enabled = true`,
   `tuf_root_url = <url>/metadata`; bootstrap the verified root via the
   existing `TufClient::bootstrap` path (persists root, role keys,
   pinned versions).

### 6.2 Update (sync)

1. Run the existing TUF update flow (`trust/client.rs::update`) against
   `<url>/metadata`: root-rotation probe → timestamp → snapshot →
   targets, with signature, expiry, monotonicity, and snapshot-consistency
   checks. M1a strengthening: for static repos the client MUST hard-fail
   when the snapshot lacks `meta` entries for `root.json` or
   `targets.json` (the current `verify_snapshot_consistency` checks the
   root pin only **if the entry exists** — presence itself must become a
   static-repo requirement, or the §4.1 invariant is unenforced).
2. Fetch `<url>/index.json`; verify length and sha256 against the
   verified targets entry for path `index.json` **before parsing**.
3. Parse; verify `schema == 1` and `index_version == targets.version`.
4. Fetch + verify `keys/package-keys.json` the same way (targets entry,
   then parse); update the repo's package trust policy
   (`TrustPolicy { trusted_keys: <base64 keys>, allow_unsigned: false }`).
5. Map package entries into the client package model (§3 mapping note).

### 6.3 Install

1. Download `<url>/<path>` for the selected entry.
2. Verify sha256 + length against the **targets** entry (the index served
   resolution; targets is the verification source — they are equal by §3
   invariant 1, and disagreement is treated as repo corruption).
3. Verify the CCS package signature against the repo's trust policy
   (existing `ccs/verify.rs::verify_package`).

### 6.4 Failure semantics

- Expired metadata: hard error; message MUST say the repo's metadata
  expired and that the operator must run `conary publish --refresh`.
- Version decrease (any TUF role): hard error naming the stored vs.
  offered versions (rollback protection; existing client behavior).
  `index_version` needs no separate history tracking — it MUST equal the
  verified `targets.version` (§3), so its rollback protection is inherited
  from the TUF layer.
- Hash mismatch on index/keys/package fetch: retryable error ("repository
  is being updated or is corrupted; try again shortly") — this is the
  torn-publish window of §5.3.4 failing safe. Persistent mismatch through a
  CDN usually means mixed-TTL caching; see §5.6.

### 6.5 `conary repo reset-trust <name>`

Explicit operator-initiated unpinning, required after a repo's root key is
lost/replaced (§10.4): deletes the repo's rows from `tuf_roots`,
`tuf_metadata`, `tuf_keys`, `tuf_targets`, and its package trust keys, then
prints that the next `repo add`/sync re-establishes trust per §6.1. There is
no silent re-pin: a root-key change without reset-trust keeps hard-failing
verification.
```

- [ ] **Step 2: Verify the cited client machinery exists**

```bash
grep -n 'pub async fn update\|pub fn bootstrap' crates/conary-core/src/trust/client.rs
grep -rl 'CREATE TABLE tuf_roots' crates/conary-core/src/db/ | head -1 | xargs grep -n 'tuf_roots\|tuf_metadata\|tuf_keys\|tuf_targets' | head -6
grep -n 'pub fn verify_package' crates/conary-core/src/ccs/verify.rs
```
Expected: all present (tables may live in a different consolidated migration file — if so, locate with `grep -rn "CREATE TABLE tuf_roots" crates/conary-core/src/db/` and substitute; the spec text does not name the migration file, so no spec change needed).

- [ ] **Step 3: Commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: static repo spec - client behavior"
```

---

### Task 9: Section — Key Lifecycle

**Files:**
- Modify: `docs/specs/static-repo-format-v1.md` (append)

- [ ] **Step 1: Write the key-lifecycle section**

```markdown
## 7. Operator Key Lifecycle

### 7.1 Generation and storage

First publish (`conary publish` against a fresh destination) creates,
under `~/.config/conary/keys/<repo-name>/`:

    root.private     root.public
    publish.private  publish.public

Files use the existing CCS key format (`ccs/signing.rs::KeyFile` TOML):
`algorithm = "ed25519"`, `key = "<base64 32 bytes>"`, optional `key_id`.
The key directory MUST be created mode 0700 **before** any key is written,
and private key files MUST be created 0600 at open time — not written then
chmod'd (the current `save_to_files` writes first and tightens permissions
after, a transient exposure window M1a fixes). The existing
`conary trust key gen` (single TUF role → `{role}.private`/`{role}.public`
in an output dir) remains low-level plumbing; `conary publish` wraps the
same `KeyFile` format and is the documented path — there is no
two-key ceremony command today, and M1a builds it into publish rather than
extending `trust key gen`. Generation MUST print both root key IDs
(fingerprints) and this exact warning: the root key **is** the repo's
identity — store `root.private` offline if possible, and back up the whole
directory; losing it means clients must manually re-trust (§7.4).

### 7.2 Rotation (keys still held)

- Rotate publish key: generate new keypair; produce root vN+1 via
  `trust/ceremony.rs::rotate_key` updating targets/snapshot/timestamp role
  keyids; publish per §5.3 including the new `{N+1}.root.json` and updated
  `root.json`, regenerated `keys/package-keys.json` (old key moves to
  `status: "retired"` so existing artifacts keep verifying; new key is
  `"active"`), and a `conary-repo.toml` left unchanged (root keys did not
  change). Clients pick up the rotation via root-version probing; no user
  action.
- Rotate root key: same mechanism; root vN+1 MUST be signed by **both**
  old and new root keys (TUF rotation rule, enforced by the existing
  client root-chain verification); `conary-repo.toml::root_key_ids` is
  updated to the new set. Out-of-band fingerprints SHOULD be re-published.

### 7.3 Revocation

Revocation = rotation that **removes** the compromised key (not
"retired" — retired keys stay trusted; compromised keys must not). The
operator MUST also: remove or republish (re-sign under the new key, new
release) every artifact the compromised key signed; bump
targets/snapshot/timestamp versions past anything the attacker may have
signed; and SHOULD shorten timestamp expiry for the next publishes.

### 7.4 Loss matrix

| Lost                  | Recoverable? | Procedure |
|-----------------------|--------------|-----------|
| publish key (root ok) | Yes          | §7.2 publish-key rotation |
| root key              | No           | New repo identity: re-run ceremony (§5.2) with new keys; clients hard-fail until each runs `conary repo reset-trust` + re-adds with the new fingerprint |
| both                  | No           | Same as root loss |

The spec deliberately provides no root-loss escape hatch that skips
client-side reset-trust: an unverifiable "trust my new key" path is the
attack this format exists to prevent.
```

- [ ] **Step 2: Verify KeyFile + rotate_key + key gen command claims**

```bash
grep -n -B2 -A6 'struct KeyFile' crates/conary-core/src/ccs/signing.rs
grep -n 'pub fn rotate_key' crates/conary-core/src/trust/ceremony.rs
grep -n 'cmd_trust_key_gen' apps/conary/src/commands/trust.rs
```
Expected: all present; KeyFile has algorithm/key/key_id.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: static repo spec - operator key lifecycle"
```

---

### Task 10: Sections — Chunks (reserved), Security Considerations, Conformance, Evolution

**Files:**
- Modify: `docs/specs/static-repo-format-v1.md` (append)

- [ ] **Step 1: Write the remaining sections**

```markdown
## 8. Chunk Store (`chunks/`) — RESERVED

Layout reserved for delta fetch. Two existing conventions are candidates:
the core CAS layout `chunks/<hh>/<rest-of-sha256>`
(`filesystem/cas.rs::object_path`) and Remi's
`chunks/objects/<hh>/<rest-of-sha256>` (`apps/remi/src/server/handlers/
mod.rs::cas_object_path`); the exact sublayout is decided with delta
semantics, not here. v1 repos MUST NOT
require chunks for correct operation; semantics (which chunks exist, how
clients discover them, TUF protection strategy for high-cardinality chunk
sets) are defined alongside delta publishing and are explicitly out of
scope here.

## 9. Security Considerations

| Threat | Mitigation |
|--------|-----------|
| MITM at first contact | `--fingerprint` (out-of-band root key IDs); interactive-only TOFU otherwise |
| Tampered index / package list | index.json and keys file are TUF targets; verify-before-parse |
| Tampered package | targets sha256+length; CCS Ed25519 signature |
| Rollback (downgrade metadata) | per-role version monotonicity + `index_version` monotonicity |
| Freeze (replay stale repo) | metadata expirations (§4.5) |
| Mix-and-match | snapshot pins targets hash; timestamp pins snapshot hash; snapshot-consistency check |
| Torn publish | reverse-order upload (§5.3.4): partial states fail hash verification, fail-safe |
| Publish-key compromise | root-signed rotation (§7.2/§7.3) |
| Root-key compromise/loss | new identity + explicit client reset-trust (§7.4); no silent re-pin |
| Destination replays old signed state to the publisher | version watermark / `--state-file` gate (§5.1); re-signing a rollback requires explicit override |
| One compromised package key signs any package (flat authority) | single-publish-key default; repo partitioning; delegations in v2 (§4.4) |
| Path traversal via index/targets paths | path rules (§3) + publisher lints (§5.4); clients MUST reject non-relative paths |

## 10. Conformance

**Producer** MUST: emit the layout of §1; satisfy §3 invariants and §5.4
lints; publish in §5.3.4 order; never mutate published artifacts or
historical roots; keep `conary-repo.toml::root_key_ids` synchronized with
root.json.

**Client** MUST: implement §6.1 trust establishment (GPG/TUF exclusivity
included); never parse index/keys before hash verification; enforce
expiry + monotonicity; treat §6.4 failure semantics; support file:// and
local paths; and validate every index/targets path **before** any URL or
filesystem join, per the §3 normalization rule (resolve, RFC 3986
normalize, reject root-escape / absolute / scheme-carrying /
percent-encoded-traversal paths — with `file://` repos a traversal escapes
into the local filesystem, so this is load-bearing, not hygiene).

## 11. Compatibility and Evolution

- v2 candidates: `consistent_snapshot = true` with versioned
  `{N}.targets.json`/`{N}.snapshot.json` filenames (removes the publish-
  window transient failures and makes aggressive CDN caching safe — the
  recommended path for production CDN-served repos; requires client fetch
  support first); TUF delegations (multi-publisher repos / per-path
  authority); chunk/delta semantics (§8).
- Versioning: `schema` majors in `conary-repo.toml`/`index.json` gate
  breaking changes; TUF `spec_version` stays 1.0.31 per the in-tree
  implementation.
- Remi (M2) MUST produce byte-format-identical repos (it is "one producer
  of the same format" per the parent spec); its DB-backed TUF serving and
  this file-based layout share `trust/` types and generation functions.
```

- [ ] **Step 2: Verify chunk-layout claim**

```bash
grep -n -A 5 'pub fn object_path' crates/conary-core/src/filesystem/cas.rs
```
Expected: `split_at(2)`, `root.join(prefix).join(suffix)`.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: static repo spec - security, conformance, evolution"
```

---

### Task 11: Spec self-review against parent-spec M0 requirements

**Files:**
- Modify: `docs/specs/static-repo-format-v1.md` (fixes only)

- [ ] **Step 1: Coverage check — every parent M0 bullet maps to a section**

Parent spec (`docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`, static-repo section) requires the child spec to define. Verify each:

| Parent requirement | Spec section |
|---|---|
| index↔TUF bridge (`index_version: u64`, index sha256 as target, verify-before-parse) | §3, §6.2 |
| Atomic publish, reverse verification order | §5.3.4 |
| `conary-repo.toml` exact schema + root.json relationship | §2, §6.1.4 |
| Expirations, refresh, rollback/freeze protection | §4.5, §5.5, §6.4, §9 |
| Key lifecycle: placement, rotation, revocation, backup, loss, reset-trust | §7, §6.5 |
| File-based TUF generator (distinct from Remi DB-backed) | §5 (+§11 Remi note) |

- [ ] **Step 2: Internal consistency checks**

- Section numbering is sequential and all `§` cross-references resolve.
- Every JSON/TOML example field appears in its field-rules list.
- The example sha256 values are syntactically valid (64 lowercase hex).
- Terms used identically with the parent spec: "hardening level", `sandboxed`/`hermetic`/`attested`, `--fingerprint`, `reset-trust`.
- No "TBD"/"TODO": `grep -n 'TBD\|TODO' docs/specs/static-repo-format-v1.md` returns nothing.

- [ ] **Step 3: Fix anything found inline, then commit**

```bash
git add docs/specs/static-repo-format-v1.md
git commit -m "docs: static repo spec - self-review fixes"
```
(Skip the commit if no fixes were needed.)

---

### Task 12: Register the spec and sync the parent

**Files:**
- Modify: `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`
- Possibly modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Point the parent's M0 milestone at the deliverable**

In the parent spec, M0 milestone bullet: append `Deliverable: docs/specs/static-repo-format-v1.md (drafted; gate opens on review approval).`

- [ ] **Step 2: Record plan refinements in the parent's revision notes**

Append to the parent's Revision notes:

```markdown
**M0 drafting (2026-06-10):** the child spec resolves three delegated
decisions: `consistent_snapshot = false` in v1 (the current client only
fetches unversioned snapshot/targets filenames; reverse-order upload + hash
pinning fail safe — versioned filenames are the v2 path); two operator
keypairs (root + publish) rather than one, so a publish-key compromise is
root-recoverable; timestamp expiry defaults to 30 days with
`conary publish --refresh` as the re-sign path.
```

- [ ] **Step 3: Register the new spec in the docs-audit gates (CI-blocking)**

The PR gate (`.github/workflows/pr-gate.yml`) diffs the committed inventory
against `scripts/docs-audit-inventory.sh` output and requires every tracked
`.md` in the ledger (`scripts/check-doc-audit-ledger.sh --require-complete`).

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Append a ledger row (TSV, 9 columns: origin_path, path, family, audience,
claim_clusters, evidence_sources, status, disposition, notes) to
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` for
`docs/specs/static-repo-format-v1.md`, family `spec` matching whatever the
regenerated inventory assigned it, mirroring the `ccs-format-v1.md` row's
style; evidence sources: the parent spec, the M0 plan, and
`crates/conary-core/src/trust/{metadata,generate,ceremony,client}.rs`.

Then verify both gates pass:

```bash
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
```
Expected: empty diff; "Documentation audit ledger check passed"; truth check passes.

- [ ] **Step 4: Mirror any other registration conventions**

```bash
grep -rn 'ccs-format-v1' docs/superpowers/documentation-accuracy-audit-summary.md docs/ARCHITECTURE.md AGENTS.md 2>/dev/null
```
Wherever `ccs-format-v1.md` is listed as a tracked/canonical doc, add `static-repo-format-v1.md` in the same style. If it appears nowhere actionable (only in archived plans), skip.

- [ ] **Step 5: Commit**

```bash
git add docs/specs/static-repo-format-v1.md docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md docs/superpowers/documentation-accuracy-audit-*.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: register static repo spec and sync parent design"
```

---

### Task 13: Handoff

- [ ] **Step 1: Report completion**

M0 deliverable drafted. The gate does **not** open yet: per the parent spec, the child spec must be reviewed and approved. Hand the spec to the external review gauntlet (GPT / Gemini / DeepSeek, same prompt pattern as the parent spec used, with the repo-grounding instructions). Triage findings with verification against `crates/conary-core/src/trust/` before adopting. M1a planning starts only after approval.

---

## Review amendments (2026-06-10)

**Gemini round:** stateless publisher gained a local version watermark +
`--state-file` rollback tripwire (a compromised destination could otherwise
launder old signed state into fresh signatures); §5.6 CDN/TTL guidance added
and the mid-publish transient-failure window documented; flat package-key
authority called out with single-key recommendation (§4.4, §9); TOFU
confirmation must warn about old-root replay; client path validation made an
explicit conformance MUST (load-bearing for file:// repos); key directory
0700-before-write and 0600-at-open required (current `save_to_files` chmods
after write); `index_version` rollback tracking simplified to the equality
check (protection inherited from TUF targets monotonicity) — which also
fixed a stale §7.2 cross-reference.

**GPT round:** docs-audit registration made an explicit CI-gated Task 12
step (inventory regen + ledger row + gate commands) — and the M0 plan
itself was registered immediately since HEAD was already failing the
ledger check; `--refresh` gained mandatory version cascades (root ⇒
snapshot+timestamp, targets ⇒ snapshot+timestamp, snapshot ⇒ timestamp —
snapshot pins the root version, so un-cascaded bumps strand clients);
single-writer rule + re-fetch-timestamp-before-final-upload + conditional
writes for concurrent-publisher safety; package-keys.json gained a
`status: active|retired` field (retired keys keep old artifacts verifying;
compromised keys are removed and their artifacts republished); GPG/TUF
exclusivity split into parse-time (clap, --fingerprint conflicts) and
execution-time (post-probe) enforcement; static repos must hard-fail on
snapshots missing root.json/targets.json meta entries (current verifier
only checks the pin if present); key-generation story corrected
(`trust key gen` is single-role plumbing; publish owns the two-key
ceremony); root-rotation transient failure window documented;
Cache-Control values and CDN invalidation specified; chunks layout no
longer falsely claims to match Remi (which uses chunks/objects/<hh>/<rest>).

**DeepSeek round:** two blockers — a second dead `§13` cross-reference in
§4.3 (rationale now inlined) and `root_version` in the §5.3.3 generation
chain made explicit (it is the *current* published root version, not
+1, except during rotation — a wrong value hard-fails every client);
`expires_hours` vs `expires_days` footgun called out; retired-key removal
criterion added (droppable once fully superseded); watermark rationale
extended with the content-regression consequence; S3
read-after-overwrite verification required between upload steps; path
rule rewritten as normalize-then-check with percent-encoding coverage
(replacing the naive `..` substring rule); "non-interactive" defined
(stdin not a TTY or CONARY_NON_INTERACTIVE=1); migration-file grep made
filename-agnostic; refresh declared byte-stable for the index except
`index_version`/`generated`; example fingerprint labeled as
sha256("abc"). **Declined:** a `version` field in package-keys.json
(the file's hash is pinned by the *current* verified targets entry, so a
stale file fails verification — a version field would be a redundant
second mechanism, same reasoning as the index_version simplification);
markdown-anchor cross-references (§ refs are the established style here).
