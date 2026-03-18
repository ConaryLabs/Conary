# Conary Self-Update Design

## Goal

Once conary is installed via a native package (.rpm/.deb/.pkg.tar.zst), it handles its own upgrades via CCS packages served from Remi. Native packages are the "get on the train" mechanism; self-update is "stay on the train."

## Architecture

`conary self-update` is a top-level CLI command that checks Remi for a newer CCS package of conary, downloads it, verifies it, and atomically replaces the running binary.

Self-update operates independently from the generation/composefs system. The conary binary lives outside EROFS images at `/usr/bin/conary` — it must survive generation switches and be available to perform them. After self-update, the new binary's hash is stored in CAS for consistency with the rest of the system.

The update channel defaults to a hardcoded Remi URL but is overridable via `conary config set update-channel <url>`.

## Command Interface

```
conary self-update              # Check + update if newer available
conary self-update --check      # Check only, don't install
conary self-update --force      # Reinstall even if same version
conary self-update --version X  # Install specific version
```

## Update Flow

```
 1. Read update channel URL from DB config table (fallback: hardcoded default)
 2. GET /v1/ccs/conary/latest -> receive version, download URL, sha256
 3. Compare remote version vs current (compiled-in) version
 4. If not newer: print "already up to date" and exit
 5. Download CCS package to temp dir
 6. Extract /usr/bin/conary to temp file (same filesystem as /usr/bin/)
 7. Verify: exec ./conary-new --version, confirm it runs and reports expected version
 8. rename() temp file -> /usr/bin/conary (atomic on same filesystem)
 9. Store new binary hash in CAS (/conary/objects/...)
10. Update DB: record new conary version in installed packages
11. Clean up temp files
12. Print: "Updated conary v0.1.0 -> v0.2.0"
```

## Remi Endpoints

New dedicated endpoints for self-update (simpler than the full package conversion flow):

```
GET  /v1/ccs/conary/latest              -> { version, download_url, sha256, size }
GET  /v1/ccs/conary/versions            -> { versions: [...], latest: "..." }
GET  /v1/ccs/conary/{version}/download  -> binary CCS package stream
```

## Atomic Binary Replacement

The `rename()` syscall is atomic on Linux when source and target are on the same filesystem. The flow ensures safety:

- **Before rename**: Nothing has changed. Any failure is harmless.
- **The rename itself**: Atomic. Either the old or new binary is at `/usr/bin/conary`.
- **After rename**: New binary is in place. CAS registration and DB update are bookkeeping — if we crash here, the new binary works fine. Next invocation can detect and repair the CAS/DB state.

Verification step (exec `./conary-new --version`) catches bad builds before the swap.

## Update Channel Configuration

- **Default**: `https://packages.conary.io/v1/ccs/conary` (hardcoded in binary)
- **Override**: `conary config set update-channel <url>`
- **Storage**: SQLite config table (key: `update-channel`)
- Enables enterprises to point at internal mirrors or developers to use test channels.

## Relationship to Generations

- Generations snapshot managed packages into EROFS images mounted at `/usr`.
- The conary binary is excluded from generation images (lives outside the mount).
- After self-update, the new binary hash is registered in CAS so future generation rebuilds reference it correctly.
- Self-update works regardless of whether generations are enabled.

## Error Handling

| Failure Point | Impact | Recovery |
|---------------|--------|----------|
| Download fails | Nothing changed | Clean up temp, print error |
| Verification fails (won't run) | Nothing changed | Clean up temp, print error |
| rename() fails | Nothing changed | Print error, suggest sudo |
| Crash after rename, before CAS | New binary works | Next run re-registers in CAS |

## What's Not Included (YAGNI)

- No auto-update daemon or scheduled checks
- No rollback command (reinstall via native package if needed)
- No signature verification beyond what CCS already provides
- No delta updates for the binary (12MB full download is fine)
- No multi-arch support yet (x86_64 only, matching current CCS manifest)
