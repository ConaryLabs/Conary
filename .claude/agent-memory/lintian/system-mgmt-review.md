# System Management Review (2026-03-10)

Modules: model, transaction, filesystem, trigger, scriptlet, container (~13k lines, 21 files)

## P0 Findings
- container/mod.rs:509 double-wait bug (wait_with_output after wait_timeout = ECHILD)
- signing.rs:75,77 expect() in canonical_json production path
- journal.rs:129 FileMoved/FileRemoved map to Staged not FsApplied (wrong recovery decision)

## P1 Findings
- recovery.rs:424 get_changeset_id_by_uuid unwrap_or(0) hides DB errors
- recovery.rs:202 O(n^2) rollback (linear backup scan per Stage record)
- container/mod.rs:565 fork-based isolation loses all child stdout/stderr
- model/mod.rs:355 diamond includes falsely detected as cycles (visited set never shrinks)
- scriptlet/mod.rs:469 seccomp warn-only, no actual enforcement

## P2 Findings
- fsverity.rs uses anyhow instead of thiserror (convention violation)
- planner.rs:209-229 hash computed twice per file
- deployer.rs:255 silently skips symlink over existing directory
- lockfile.rs:87 model_hash always empty string
- journal.rs:183-201 placeholder files never cleaned on crash
- planner.rs:288,313 BackupInfo.size silently coerces negative to 0
- recovery.rs:429+ symlink validation more permissive than staging

## Patterns
- trigger/mod.rs wait_and_capture correctly reads pipes after wait_timeout (reference pattern)
- All 21 files: headers compliant, path traversal guards solid, CAS atomic, journal CRC solid
- No SQL injection, no unwrap in non-test code (except signing.rs noted above)
