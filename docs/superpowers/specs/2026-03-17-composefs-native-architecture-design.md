---
last_updated: 2026-03-17
revision: 1
summary: Unified design for composefs/EROFS-native architecture throughout Conary
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
5. Mount       — composefs mount, bind /usr, rebuild /etc overlay
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

**Recovery on boot:**
1. Read `/conary/current` symlink to find active generation
2. If its EROFS image exists, mount it — done
3. If not (crash during build), query DB for latest SystemState, rebuild EROFS, mount

### Conflict Detection

VfsTree remains for preflight conflict detection. Before the DB commit, the
transaction engine builds the proposed filesystem state in memory and checks for:

- File owned by another package
- Untracked file on disk blocking a new file
- Path type mismatches (directory where file should go, or vice versa)

If conflicts are found, the transaction aborts before any state changes. This is
pure in-memory validation — no disk I/O.

### Locking

One transaction at a time via lockfile (unchanged). The "danger window" (mutable
state changes) shrinks from the entire backup/stage/deploy sequence to a single
SQLite transaction, so the lock is held for much less time.

## CAS, Generations & GC

### CAS (Unchanged)

CAS stays as-is: content-addressed, atomic writes, SHA-256 keyed. The only
behavioral change is that nothing hardlinks out of CAS to a mutable root. CAS
objects are read exclusively through composefs at mount time.

### Generations

Every transaction produces a generation. They are cheap:

- An EROFS image with 100K files is ~15-20MB (inodes, dirents, xattr CAS references)
- Building one is CPU-bound metadata serialization — sub-second for typical systems,
  a few seconds for very large ones
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
SELECT DISTINCT hash FROM file_entries
WHERE trove_id IN (
    SELECT trove_id FROM state_members
    WHERE state_id IN (/* surviving generation state IDs */)
)
```

Pure database query. No filesystem walk. Fast and correct.

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
Package signature -> DB file_entries (hash) -> EROFS image (CAS digest xattrs) -> fs-verity (kernel reads)
```

Each layer verifies the next:
- Package signatures verify metadata and hashes are authentic
- The database stores trusted hashes from verified packages
- The EROFS image embeds those hashes as CAS xattrs (deterministically derived from DB)
- fs-verity ensures every byte read from CAS matches the digest at the kernel level

A compromised file on disk produces an I/O error, not bad data. Stronger than
traditional package managers that verify at install time but trust the filesystem
afterward.

### EROFS Image Signing (Future)

Since images are deterministic and small (~20-50MB):
- Compute SHA-256 of the EROFS image
- Sign with ed25519 key
- Store signature alongside the image
- Verify signature before mounting

Closes the loop: verify that a generation was produced by a trusted build.

### Secure Boot Integration (Future)

Signed EROFS image hash embedded in the boot chain. Dracut verifies the generation
signature before mounting. Puts conaryOS in the same category as ChromeOS/Android
verified boot, but with a real package manager underneath.

Image signing and secure boot are natural extensions of this architecture, not
required for initial implementation.

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
| `conary-core/src/transaction/mod.rs` | New lifecycle: resolve -> fetch -> DB commit -> EROFS build -> mount. ~300 lines replacing ~1500. |
| `src/commands/generation/builder.rs` | Core of every transaction. Extracted into conary-core so the transaction engine calls it directly. |
| `src/commands/generation/switch.rs` | Called after every transaction, not manually. Mount logic stays the same. |
| `conary-core/src/bootstrap/` | Outputs EROFS + CAS + DB instead of qcow2. Same pipeline stages, different final artifact. |

### Unchanged

| Module | Reason |
|---|---|
| `conary-erofs/` | Already does exactly what's needed. No changes. |
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
| Image signing (future) | Sign/verify EROFS images |

### Net Effect

Transaction engine: ~1500 lines of journal/backup/recovery -> ~300 lines of
resolve/fetch/commit/build/mount. Generation builder moves from CLI convenience
into core loop. conary-erofs stays untouched.
