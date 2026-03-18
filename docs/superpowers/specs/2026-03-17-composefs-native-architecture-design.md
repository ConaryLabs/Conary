---
last_updated: 2026-03-17
revision: 3
summary: Incorporate ecosystem research -- mount-time digest, bloom filter, /etc merge, UKI, composefs-rs
---

# Composefs-Native Architecture

## Overview

Redesign Conary's core architecture to make EROFS/composefs the primary deployment
mechanism for all operations — not just generation snapshots. Every transaction
(install, remove, update) produces a new EROFS image and mounts it via composefs.
The mutable filesystem is eliminated for all managed content.

This is a clean-slate redesign. No backward compatibility is required (no current
users). composefs is treated as a hard requirement (EROFS mainline since kernel 5.4,
composefs since 6.6; all target distros ship 6.6+).

## Motivation

Today Conary has an architectural split: generations are immutable composefs
snapshots, but the path to getting there goes through mutable filesystem operations
(hardlink files from CAS to root, backup-before-overwrite, journal-based crash
recovery). The conary-erofs crate is a self-contained, forward-thinking EROFS
builder that only participates in the optional generation-build step.

Making EROFS the transaction primitive:

- Eliminates the journal, backup phase, staging phase, and crash recovery replay
- Gives every file read kernel-enforced integrity via fs-verity
- Unifies the artifact format across transactions, bootstrap, deltas, and OCI export
- Produces a genuinely novel package manager architecture (no other PM uses composefs
  as the primary transaction mechanism as of 2026-03-17)

## Core Mental Model

The EROFS image is the deployment mechanism, not a snapshot of one.

**Old flow:**
```
install package -> deploy files to mutable root -> (optionally) snapshot into EROFS
```

**New flow:**
```
install package -> update database -> build EROFS image -> mount it
```

Files never touch the live filesystem directly. The database is the source of truth
for what's installed. The EROFS image is a derived artifact — a materialized view of
the database. The CAS store holds actual file content. composefs wires them together
at mount time.

### System Layout

```
/conary/
  objects/           # CAS store (content-addressed files by SHA-256)
  generations/
    1/root.erofs     # EROFS metadata image (~20-50MB, no file content)
    2/root.erofs
    3/root.erofs
  current -> 3       # Symlink to active generation
  mnt/               # composefs mount point (kernel resolves CAS reads)
  etc-state/         # Persistent /etc overlay (upper + work)
```

Eliminated:
- `/conary/txn/` (transaction working directories)
- `/conary/journal/` (journal files)
- Backup directories
- Staging directories
- Hardlinked files on the mutable root

Every `conary install`, `conary remove`, `conary update` produces a new generation.
Generations are cheap (metadata-only EROFS images). Rollback = remount a previous
generation. GC cleans up old ones.

## Transaction Lifecycle

### New Flow

```
1. Resolve     — dependency resolution (unchanged)
2. Fetch       — download packages, extract files into CAS (idempotent)
3. DB Commit   — single SQLite transaction:
                   insert Trove records
                   insert FileEntry records
                   create SystemState snapshot
                   increment generation number
4. Build EROFS — query all FileEntry rows, feed to ErofsBuilder
                   write to /conary/generations/{N}/root.erofs
                   enable fs-verity on image, record digest in DB
5. Mount       — composefs mount with digest verification:
                   mount -t composefs -o basedir=/conary/objects,
                     digest=<erofs_verity_digest>,verity=on,
                     upperdir=/conary/etc-state/upper,
                     workdir=/conary/etc-state/work
                   bind-mount /usr read-only from composefs
6. Symlink     — atomically update /conary/current -> N
```

### Crash Recovery

Every step after the DB commit is re-derivable. The EROFS image is a pure function
of the database state. The mount is a pure function of the EROFS image.

| Crash point | State | Recovery |
|---|---|---|
| During fetch | CAS has some new objects, DB unchanged | Retry. CAS writes are idempotent. Orphaned CAS objects cleaned by GC. |
| During DB commit | SQLite rolls back automatically | Retry. Nothing changed. |
| After DB commit, before EROFS build | DB has new state, no image yet | Rebuild EROFS from DB state. Deterministic — same state always produces same image. |
| After EROFS build, before mount | Image exists, old generation still mounted | Mount the new image. |
| After mount, before symlink | New generation mounted but symlink stale | Update symlink. |

No journal is needed. No rollback replay. No backup restoration.

**Recovery on boot** (implemented as a Dracut module + early systemd unit):

1. Dracut initramfs reads `/conary/current` symlink to find active generation
2. If its EROFS image exists and passes basic validation (superblock magic + size
   check), mount it via composefs — done
3. If the image is missing or truncated (crash during build):
   a. Query DB for latest SystemState with `is_active = true`
   b. Rebuild EROFS image deterministically from that state
   c. Mount the rebuilt image
4. If the database is corrupted (unlikely — SQLite WAL + fsync), fall back to the
   most recent intact EROFS image on disk (scan `/conary/generations/` by number,
   descending)

### Conflict Detection

VfsTree remains for preflight conflict detection. Before the DB commit, the
transaction engine builds the proposed filesystem state in memory and checks for:

- File owned by another package (two packages claiming the same path)
- Path type mismatches (directory where file should go, or vice versa)
- Duplicate providers (two packages providing the same capability)

If conflicts are found, the transaction aborts before any state changes. This is
pure in-memory validation — no disk I/O. Note: "untracked file on disk" conflicts
are no longer relevant since files are never written to a mutable root.

### Locking

One transaction at a time via lockfile (unchanged). The lock is held for the full
transaction (resolve through mount) to prevent concurrent modifications. However,
the critical section where mutable state changes (the SQLite transaction) is much
shorter than the old backup/stage/deploy sequence. Steps outside the DB commit
(fetch, EROFS build, mount) are idempotent and safe to retry if interrupted.

### Mutable State: /etc, /var, and Other Writable Paths

The EROFS image is read-only. Mutable state lives outside it on separate mounts.

**Path classification:**

| Path | Treatment |
|---|---|
| `/usr` | Read-only bind mount from composefs. All package-managed binaries, libraries, share. |
| `/etc` | Handled by composefs `upperdir`/`workdir` mount options — EROFS lower (package defaults) + persistent upper (user modifications). No separate overlayfs mount needed. |
| `/var` | Separate persistent mount. Never in the EROFS image. |
| `/tmp`, `/run` | tmpfs. Never in the EROFS image. `/run` is writable for PIDs, daemon reload. |
| `/home`, `/root` | Separate persistent mount. Never in the EROFS image. |
| `/srv`, `/opt` | Separate persistent mounts. Never in the EROFS image. |
| `/proc`, `/sys`, `/dev` | Virtual filesystems. Never in the EROFS image. |
| `/mnt`, `/media` | Mount points. Never in the EROFS image. |
| `/bin`, `/sbin`, `/lib`, `/lib64` | Symlinks to `/usr/*` (USR merge). Included in EROFS as symlinks. |

The EROFS builder's `EXCLUDED_DIRS` list will be updated as part of this work. The
current list (`home`, `proc`, `sys`, `dev`, `run`, `tmp`, `mnt`, `media`, `var/lib`)
was a compromise for the mutable-root model. In the composefs-native model, the
exclusion list becomes: `var`, `tmp`, `run`, `home`, `root`, `srv`, `opt`, `proc`,
`sys`, `dev`, `mnt`, `media`. The change from `var/lib` to `var` (full directory)
reflects that all of `/var` is a separate persistent mount.

**/etc overlay behavior during transactions:**

When a package installs or updates files under `/etc`, those files appear in the
EROFS image's `/etc` directory (the overlay lower layer). The overlay upper layer
holds user modifications. overlayfs semantics handle the interaction:

- Package installs new `/etc/nginx/nginx.conf`: appears in lower, visible to user
  (no upper override exists)
- User modifies `/etc/nginx/nginx.conf`: copy-up to upper, user version takes
  precedence
- Package updates `/etc/nginx/nginx.conf`: three-way merge — compare previous
  generation's lower, new generation's lower, and current upper. If only the package
  changed (user didn't modify), the new package version wins silently. If only the
  user changed, the user version wins. If both changed, the user is prompted to
  resolve the conflict (with the option to accept package version, keep user version,
  or merge). This follows the bootc/ostree model and is more robust than `.rpmnew`
  sidecar files.
- Package removes a file from `/etc`: gone from lower in new generation. If user had
  modified it, the upper copy persists as an orphan (GC can detect these)

The `/etc` overlay uses composefs's native `upperdir`/`workdir` mount options
(no separate overlayfs mount needed). The three-way merge runs between steps 4 and 5
in the transaction flow, comparing the previous and new EROFS images' `/etc` trees
against the upper layer before mounting the new generation.

### Scriptlet Execution

Package scriptlets (pre-install, post-install, triggers) run after the EROFS image
is built and mounted but before the transaction is considered complete.

**Execution environment:**
- Scriptlets execute in a namespace with the new composefs generation mounted
- Writable paths available: `/etc` (overlay upper), `/var`, `/tmp`
- `/usr` is read-only (as it should be — scriptlets should not modify managed content)
- Standard scriptlet operations work:
  - `ldconfig` — reads `/usr/lib` (read-only), writes `/etc/ld.so.cache` (writable)
  - `systemctl daemon-reload` — writes to `/run` (tmpfs, writable)
  - `update-alternatives` — writes to `/etc/alternatives` (writable overlay)
  - `useradd`/`groupadd` — writes to `/etc/passwd`, `/etc/group` (writable overlay)

**Updated transaction flow with scriptlets:**

```
1. Resolve
2. Fetch
3. DB Commit
4. Build EROFS (enable fs-verity, record digest)
5. Three-way /etc merge (previous lower vs new lower vs upper)
6. Mount (composefs with digest, upperdir, workdir, verity=on)
7. Run scriptlets (against newly mounted generation)
8. Symlink update
```

**Scriptlet failure handling:**
- If a scriptlet fails, the new generation's EROFS image and DB state remain valid
- The mount can be rolled back to the previous generation (remount previous EROFS)
- The failed generation is marked in metadata but not deleted (allows debugging)
- The DB commit is not reversed — the generation exists but is not active

This is simpler than the old model where scriptlet failure required journal-based
filesystem rollback. Here, the previous generation is always intact and mountable.

## CAS, Generations & GC

### CAS (Unchanged)

CAS stays as-is: content-addressed, atomic writes, SHA-256 keyed. The only
behavioral change is that nothing hardlinks out of CAS to a mutable root. CAS
objects are read exclusively through composefs at mount time.

### Generations

Every transaction produces a generation. They are cheap:

- An EROFS image with 100K files is ~15-20MB (inodes, dirents, xattr CAS references)
- Building one is CPU-bound metadata serialization — expected sub-second for typical
  systems, a few seconds for very large ones (should be benchmarked early in
  implementation to validate)
- Disk cost is trivial compared to the CAS objects they reference

Generation numbering is sequential from `system_states.state_number`. One generation
= one SystemState = one EROFS image. The mapping is 1:1.

### GC

Two jobs:

**Generation GC** — delete old EROFS images. Keep current, booted, pinned, and last
N generations (same policy as today).

**CAS GC** — delete CAS objects not referenced by any surviving generation. Today CAS
liveness is inferred from filesystem hardlink counts (nlink > 1). That goes away.
Instead, liveness is a database query:

```sql
SELECT DISTINCT f.sha256_hash
FROM files f
JOIN troves t ON f.trove_id = t.id
JOIN state_members sm ON sm.trove_name = t.name AND sm.trove_version = t.version
WHERE sm.state_id IN (/* surviving generation state IDs */)
```

Three-table join through name/version (state_members records membership by name, not
foreign key). All columns are indexed. Pure database query. No filesystem walk.

### fs-verity

Enabled lazily on CAS objects during EROFS build (same as today). Once enabled,
composefs verifies integrity on every read at the kernel level. A corrupted CAS
object produces an I/O error — the kernel refuses to serve bad data.

## Bootstrap, CCS, Deltas & OCI

### Bootstrap

Today the bootstrap pipeline produces qcow2 images through an 8-stage process. The
new output is the same artifact type as any generation: EROFS image + CAS store.
A bootstrapped conaryOS system is "generation 1."

Pipeline becomes:
1. Resolve the base package set (unchanged)
2. Download packages, populate a CAS store
3. Insert troves + file_entries into a fresh SQLite database
4. Build EROFS image from that database
5. Package: CAS directory + EROFS image + database + bootloader config

A bootable image (ISO, VM, cloud) wraps this in whatever container format is needed,
but the core payload is always: CAS + EROFS + DB. The bootstrap output is a portable
generation, not a special artifact.

### CCS Packages

CCS carries file content + file metadata + trove metadata (unchanged). On install,
files go into CAS, metadata goes into the DB. The EROFS image is rebuilt from the
full system state afterward.

CCS does not need to carry EROFS fragments. The image is always rebuilt from the
complete system state, not assembled from per-package pieces. This keeps CCS simple
and format-agnostic.

### Delta Updates

EROFS determinism enables binary deltas between generations:

- Server has generation N and generation N+M EROFS images
- Delta = binary diff (bsdiff or zstd-patched) — very small since most metadata is
  shared between generations
- Client applies delta to their current EROFS image to get the new one
- New CAS objects are fetched separately (already handled by `src/delta/`)

Two-track update model:
1. **Metadata delta** — small EROFS image patch, instant to apply
2. **Content delta** — new/changed CAS objects, streamed and stored

For a typical update touching 50 packages out of 2000, the EROFS delta is tiny
(changed inodes + dirents only) while CAS deltas carry actual new file content.

**Determinism constraint:** for server-to-client EROFS deltas to work, both sides
must produce byte-identical images from the same logical state. This requires that
`ErofsBuilder`'s sort order, alignment, and xattr encoding remain stable across
versions. Any change to the builder's output format is a breaking change for deltas
(clients must do a full image rebuild instead of applying a patch). The existing
`deterministic_output` test in `conary-erofs` enforces this property.

### OCI/Container Export

An EROFS image + CAS store maps directly to an OCI container image:
- EROFS image = layer metadata
- CAS objects = layer content
- composefs mounts it directly

`conary export --oci` produces a standards-compliant container image. Useful for
deploying conaryOS as a container, CI builds, or distributing immutable appliance
images. The conary-erofs crate already produces valid EROFS — the work is OCI
framing (manifest JSON, tar wrapping).

## Integrity & Security

### Integrity Chain

```
Package signature -> DB file_entries (hash) -> EROFS image (CAS digest xattrs)
    -> mount-time digest verification -> fs-verity (kernel reads)
```

Each layer verifies the next:
- Package signatures verify metadata and hashes are authentic
- The database stores trusted hashes from verified packages
- The EROFS image embeds those hashes as CAS xattrs (deterministically derived from DB)
- **Mount-time digest verification**: composefs's `digest` mount option validates the
  EROFS image's fs-verity digest before mounting. The kernel refuses to mount a
  tampered image. This is free — no code beyond passing the digest to the mount call.
  The digest is stored in the database alongside the generation's SystemState.
- fs-verity ensures every byte read from CAS matches the digest at the kernel level

A compromised file on disk produces an I/O error, not bad data. A tampered EROFS
image refuses to mount. Stronger than traditional package managers that verify at
install time but trust the filesystem afterward.

### EROFS Image Signing

Since images are deterministic and small (~20-50MB), userspace signature verification
is the recommended approach (per the composefs community and the fsverity maintainer,
who advised against kernel-based `CONFIG_FS_VERITY_BUILTIN_SIGNATURES`):

- Enable fs-verity on the EROFS image file itself
- Compute the fs-verity digest of the image
- Sign the digest with an ed25519 key in userspace
- Store the signature alongside the image (`root.erofs.sig`)
- Before mounting, verify the signature against a trusted public key in userspace
- Pass the verified digest to `mount -t composefs -o digest=<hash>` — the kernel
  then enforces that the mounted image matches

This follows the same pattern that OSTree/bootc uses for composefs signature
validation. No kernel keyring or `CONFIG_FS_VERITY_BUILTIN_SIGNATURES` required.

### Secure Boot via UKI (Future)

The natural secure boot integration path is Unified Kernel Images (UKI) — a single
EFI binary containing kernel + initramfs + cmdline + composefs digest. The boot
chain becomes:

1. Firmware measures the UKI (Secure Boot / TPM)
2. UKI contains the expected composefs digest in its cmdline or embedded metadata
3. Initramfs (Dracut) mounts composefs with `-o digest=<hash>` from the UKI
4. Kernel refuses to mount if the EROFS image doesn't match
5. fs-verity verifies every CAS file read

This extends the chain of trust from firmware through kernel to the entire userspace
filesystem — the same model composefs-rs and bootc are converging on for Fedora
Atomic and other immutable distros. Puts conaryOS in the same category as
ChromeOS/Android verified boot, but with a real package manager underneath.

Image signing and UKI integration are natural extensions of this architecture, not
required for initial implementation.

## EROFS Enhancements

### Xattr Name Bloom Filter

The Linux kernel (since ~5.19) supports a bloom filter in the EROFS superblock that
accelerates negative xattr lookups. Since every file in a composefs image has
`trusted.overlay.redirect` xattrs, workloads that probe for other xattrs (e.g.,
`system.posix_acl_access` during `ls -l`) benefit significantly — up to 20%
improvement in uncached metadata operations.

`conary-erofs` should emit the `EROFS_FEATURE_COMPAT_XATTR_FILTER` flag and populate
the bloom filter data in the superblock. The hash function is
`xxh32(name, strlen(name), EROFS_XATTR_FILTER_SEED + index)`. This is a small,
contained enhancement to `superblock.rs` and `xattr.rs`.

### composefs-rs Compatibility

The composefs-rs project (Rust, under `containers/composefs-rs`) is positioned to
become the reference implementation of composefs, replacing the C implementation. It
provides EROFS image building, OCI integration, fs-verity, and boot infrastructure.

We maintain our own EROFS builder (`conary-erofs`) because it is standalone, has no
external dependencies, and is tailored to our CAS-reference-only images. However, we
must ensure our EROFS output is mountable by standard `mount.composefs` and
compatible with the composefs ecosystem. Specifically:

- EROFS superblock format and feature flags must match what the kernel expects
- CAS xattr encoding (`trusted.overlay.redirect`) must match composefs conventions
- fs-verity digest format must be interoperable

The `verify` module in `conary-erofs` should include a compatibility check that
validates images against `mount.composefs` expectations. If composefs-rs stabilizes
and provides clear advantages (e.g., built-in OCI support, UKI generation), we
should evaluate adopting it as a dependency rather than maintaining a parallel
implementation.

## Code Impact

### Deleted

| Module | Reason |
|---|---|
| `conary-core/src/transaction/journal.rs` | No journal — DB is the source of truth |
| `conary-core/src/transaction/recovery.rs` | Recovery = rebuild EROFS from DB |
| `conary-core/src/filesystem/deployer.rs` | No file deployment to mutable root |
| Transaction backup/staging logic in `transaction/mod.rs` | No backup-before-overwrite |

### Rewritten

| Module | Change |
|---|---|
| `conary-core/src/transaction/mod.rs` | New lifecycle: resolve -> fetch -> DB commit -> EROFS build -> mount. Roughly one-fifth the current size. |
| `src/commands/generation/builder.rs` | Core of every transaction. Extracted to `conary-core/src/generation/builder.rs` so the transaction engine calls it directly. |
| `conary-core/src/generation/mount.rs` | Mount logic extracted from `src/commands/generation/switch.rs`. Called after every transaction, not manually. |
| `conary-core/src/bootstrap/` | Outputs EROFS + CAS + DB instead of qcow2. Same pipeline stages, different final artifact. |

### Unchanged

| Module | Reason |
|---|---|
| `conary-erofs/` | Minor enhancement: add xattr name bloom filter to superblock (see EROFS Enhancements below). Core builder logic unchanged. |
| `conary-core/src/filesystem/cas.rs` | CAS is the right abstraction. |
| `conary-core/src/filesystem/vfs/` | Still used for preflight conflict detection. |
| `conary-core/src/resolver/` | Dependency resolution is orthogonal to deployment. |
| `conary-core/src/db/` | Schema mostly unchanged (file_entries, troves, system_states). |
| `conary-core/src/packages/` | RPM/DEB/ALPM parsing unchanged. |
| `conary-core/src/repository/` | Repo sync, metadata fetch unchanged. |
| `conary-core/src/delta/` | CAS-level deltas still work. EROFS deltas are additive. |

### New Code

| Module | Purpose |
|---|---|
| Boot recovery (small) | On boot: check DB vs mounted generation, rebuild EROFS if needed |
| CAS GC (revised) | Liveness from DB query instead of filesystem nlink count |
| EROFS delta support | Binary diff/patch between generation images |
| OCI export | Wrap EROFS + CAS as OCI container image |
| /etc three-way merge | Compare previous/new EROFS lower + upper, resolve conflicts |
| Image signing (future) | Userspace ed25519 signature verification of EROFS images |
| UKI integration (future) | Embed composefs digest in Unified Kernel Image for measured boot |

### Net Effect

Transaction engine shrinks to roughly one-fifth its current size. Generation builder
moves from CLI convenience into the core transaction loop at
`conary-core/src/generation/`. conary-erofs stays untouched.
