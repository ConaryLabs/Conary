# Level 3: Full System Takeover — Design Document

**Date:** 2026-03-04
**Status:** Approved
**Goal:** [#189] Full System Takeover (Level 3)
**Depends on:** Levels 0-2 dependency resolution (Goal #188, completed)

## Summary

Add generation-based atomic system management to Conary. Users can convert an entire running system to Conary-managed generations, switch between them live or at boot, and roll back instantly. This is the final level of the coexist → adopt → takeover → full-system spectrum.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Use case | Greenfield + gradual migration | Both paths share the same generation infrastructure |
| Filesystem strategy | Reflink trees | Simple, proven, zero extra disk via CoW. Requires btrfs/xfs |
| Boot integration | BLS primary, GRUB fallback | BLS is standard on Fedora/Arch. GRUB fallback covers others |
| Scope | Full root (minus /home, /var/lib) | Maximum control for users who want it |
| Switch model | Live switch + reboot recommended | renameat2(RENAME_EXCHANGE) for instant swap; reboot for full consistency |

## Generation Storage Layout

```
/conary/
  generations/
    1/                    # Full root tree (reflinked from CAS)
      usr/
      etc/
      bin -> usr/bin
      lib -> usr/lib
      sbin -> usr/sbin
      lib64 -> usr/lib64
      boot/               # Kernel + initramfs (if managed)
    2/
      ...
  current -> generations/2   # Active generation symlink
  gc-roots/               # Pins preventing GC (booted gen, user pins)
```

- Generations numbered monotonically via existing `system_states.state_number`
- Each generation is a complete root tree with all files reflinked from CAS
- Excluded: `/home`, `/var/lib`, `/proc`, `/sys`, `/dev`, `/run`, `/tmp`
- `/var` partially managed: `/var/lib` excluded, structure dirs included
- GC roots prevent deletion of in-use generations

## Generation Lifecycle

### Creating a Generation

1. Query `SystemState` for all packages in the system
2. Allocate next `state_number` as generation number
3. `mkdir /conary/generations/{N}/`
4. For each package file manifest:
   - Reflink from CAS (`ioctl(FICLONE)`) into `generations/{N}/{path}`
   - Fall back to copy on non-CoW filesystems
5. Create standard root symlinks (`bin -> usr/bin`, etc.)
6. Write metadata: `generations/{N}/.conary-gen.json`
7. Record in DB via `StateEngine::create_snapshot()`

### Live Switching

```rust
renameat2(AT_FDCWD, "/conary/generations/{N}/usr", AT_FDCWD, "/usr", RENAME_EXCHANGE)
renameat2(AT_FDCWD, "/conary/generations/{N}/etc", AT_FDCWD, "/etc", RENAME_EXCHANGE)
// ... other top-level dirs
```

After exchange, old content lands in `generations/{N}/` (swapped in). Update `/conary/current` symlink. Running processes keep old file descriptors; new processes get the new tree.

### Reboot Switching

Write BLS entry → reboot → initramfs hook bind-mounts generation paths.

### Garbage Collection

- Remove generations not booted, not `current`, not pinned, beyond retention count (default: 3)
- Deletion is `rm -rf /conary/generations/{N}/` — reflinks don't affect CAS

## `conary system takeover` Command

### Pre-flight

- Verify reflink support (test reflink, delete)
- Verify space in `/conary/`
- Verify root privileges
- Verify no pending transactions

### Inventory

- Query all system packages via `SystemPackageManager::detect()`
- Cross-reference Conary DB (installed/adopted/taken already tracked)
- Classify remaining packages:
  - Blocklisted → adopt only (never convert glibc/systemd/etc.)
  - Available on Remi → convert to CCS
  - Not on Remi → adopt from system PM (hash files into CAS)

### Execute

1. Present summary: "Converting 847 packages (312 from CCS, 535 adopted from RPM)"
2. `[Y/n]` confirm (skip with `--yes`)
3. Adopt all un-tracked packages into CAS
4. Download/convert available CCS packages from Remi
5. Build Generation 1 (reflink all files from CAS)
6. Write BLS boot entry
7. Live switch via `renameat2(RENAME_EXCHANGE)`
8. Update `/conary/current`
9. Print: "System takeover complete. Reboot recommended for full consistency."

### CLI

```
conary system takeover [--yes] [--dry-run] [--skip-conversion]
```

`--skip-conversion` adopts everything without Remi conversion (faster, offline-capable).

## Boot Integration

### BLS Entries (Primary)

Written to `/boot/loader/entries/conary-gen-{N}.conf`:

```ini
title    Conary Generation {N} ({date})
version  {kernel_version}
linux    /vmlinuz-{kernel_version}
initrd   /initramfs-{kernel_version}.img
options  root=UUID={root_uuid} conary.generation={N} {existing_cmdline}
```

Active generation gets highest `sort-key` to appear first.

### Initramfs Hook

Dracut module at `/usr/lib/dracut/modules.d/90conary/`:

1. Read `conary.generation=N` from `/proc/cmdline`
2. Bind-mount `/conary/generations/{N}/{dir}` over `/sysroot/{dir}` for each managed dir
3. Fall back to `/conary/current` if no parameter
4. If generation missing, boot latest valid generation + log warning

### GRUB Fallback

If BLS unavailable:

1. Write `/etc/grub.d/42_conary` script generating menu entries per generation
2. Run `grub-mkconfig -o /boot/grub/grub.cfg`
3. Same `conary.generation=N` kernel parameter

### Detection

```
if /boot/loader/entries/ exists AND (systemd-boot OR grub+blscfg):
    BLS
elif grub-mkconfig exists:
    GRUB fallback
else:
    warn, skip boot entry
```

## CLI Surface

```
conary system takeover [--yes] [--dry-run] [--skip-conversion]
conary generation list
conary generation build
conary generation switch {N} [--reboot]
conary generation rollback
conary generation gc [--keep N]
conary generation info {N}
```

## Error Handling

| Scenario | Behavior |
|----------|----------|
| No reflink support | Error: "Reflink support required. Use btrfs or xfs for /conary/" |
| renameat2 fails (old kernel) | Fall back to non-atomic rename + fsync. Warn about crash risk |
| Reflink fails mid-build | Transaction journal rollback. Remove partial generation |
| Power loss during live switch | Initramfs reads `/conary/current` symlink for consistent state |
| Insufficient space | Pre-flight estimate from CAS manifest. Reflinks are CoW, report estimated vs available |
| Package missing from CAS | Error with list. Suggest `conary system adopt --refresh` |

## Existing Infrastructure (reused)

| Component | Location | Role |
|-----------|----------|------|
| CAS store | `src/filesystem/cas.rs` | Source of truth for all file content |
| File deployer | `src/filesystem/deployer.rs` | Extended with reflink support |
| System states | `src/db/models/state.rs` | Generation numbering, diff, restore plans |
| Transaction journal | `src/transaction/` | Crash-safe generation builds |
| Bootstrap image builder | `src/bootstrap/image.rs` | Reference for partition/boot layout |
| Blocklist | `src/commands/install/blocklist.rs` | Critical package protection |
| System PM queries | `src/commands/install/system_pm.rs` | Package inventory |

## Testing

- Unit tests: generation metadata, reflink detection, BLS entry generation, GC logic
- Integration: btrfs loopback image — create filesystem, build generation, verify files, switch, rollback
- Existing transaction journal tests cover crash recovery
