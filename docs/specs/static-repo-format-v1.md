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
        package-keys.json               # package-signing pubkeys (§4.4); TUF target
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

## 5. Publish Algorithm (Producer Requirements)

Any producer (`conary publish` M1a, Remi M2, third-party tooling) MUST
behave as follows. The publisher is destination-derived with a local rollback
watermark: current versions are read from the destination, while the local
watermark only gates regressions and never derives the next version.

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
   uploading the new timestamp (§5.3 step 4(d)) and abort if its version
   changed since §5.1.

### 5.2 Initial publish (ceremony)

1. Ensure the root and publish keys exist; generate if absent.
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
   `generate_targets(target_entries, publish_key, targets_next,
   targets_expires_days)` → `generate_snapshot(root_version,
   targets_metadata, publish_key, snapshot_next, snapshot_expires_days)` →
   `generate_timestamp(snapshot_metadata, publish_key, timestamp_next,
   timestamp_expires_hours)`; each bumped role's version = its destination
   version + 1. **`root_version` is NOT a role-to-bump during ordinary
   publishes:** it is the version of the currently published root (read in
   §5.1), except when this publish also updates root metadata (root rotation
   or root refresh), in which case snapshot pins the new root version. A
   snapshot pinning a nonexistent root version hard-fails every client's
   consistency check.
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
   On root metadata update (rotation or root refresh): new
   `metadata/{root_next}.root.json` and `metadata/root.json` upload during
   step (a).
   A client reading mid-publish either sees a complete hash chain whose
   referenced files are already uploaded, or a fail-safe hash mismatch caused
   by mixed old/new mutable files. Torn states fail verification — clients
   never act on them.
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
- Every targets/index path passes the §3 normalization rule (resolve,
  RFC 3986 normalize, reject root-escape / absolute / scheme-carrying /
  percent-encoded-traversal paths) — the producer enforces exactly the
  rule clients verify.

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
during a refresh). Upload ordering of §5.3 step 4 applies.

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
Do not assume every S3-compatible backend, CDN path, or gateway guarantees
overwrite visibility at the point the upload call returns: each §5.3 step 4
upload step completes only when the uploaded object is confirmed visible
(read-back or ETag check) before the next step begins, and the §5.1
destination reads SHOULD bypass caches (no-cache request headers or
cache-busting query). CDN-served production repos are the primary motivation
for a future v2 consistent-snapshot upgrade.

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
   `tuf_root_url = <url>/metadata`, `gpg_check = false`,
   `gpg_strict = false`, and `gpg_key_url = NULL`; bootstrap the verified
   root via the existing `TufClient::bootstrap` path (persists root, role
   keys, pinned versions). Static repo install/sync paths MUST use TUF
   metadata plus CCS package signatures only; legacy GPG state is disabled,
   not merely ignored by convention.

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
   then parse); reject any key entry whose `status` is not `"active"` or
   `"retired"`; update the repo's package trust policy with
   `TrustPolicy::strict(<all active + retired package-keys public_key
   values>)`.
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
  torn-publish window of §5.3 step 4 failing safe. Persistent mismatch
  through a CDN usually means mixed-TTL caching; see §5.6.

### 6.5 `conary repo reset-trust <name>`

Explicit operator-initiated unpinning, required after a repo's root key is
lost/replaced (§7.4): deletes the repo's rows from `tuf_roots`,
`tuf_metadata`, `tuf_keys`, `tuf_targets`, and its package trust keys, while
leaving the repository URL configured as a static repo. Re-establishment is
an explicit bootstrap, not a silent repair: either `reset-trust` marks the
repo so the next sync detects "no trusted root" and re-runs §6.1
fingerprint/TOFU establishment before `trust/client.rs::update`, or the CLI
provides an explicit `repo add --replace` path that performs §6.1 for the
existing name. A plain duplicate-name `repo add` rejection cannot be the only
re-pin path. A root-key change without reset-trust keeps hard-failing
verification.

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
extending `trust key gen`. Generation MUST print both generated key IDs
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
