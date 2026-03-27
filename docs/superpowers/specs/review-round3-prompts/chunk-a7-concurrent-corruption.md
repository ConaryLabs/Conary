You are performing an adversarial security review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 adopts an attacker's perspective to find exploitation chains.

This is Chunk A7: Concurrent State Corruption.

## Attacker Profile

You are an attacker who can trigger two concurrent CLI invocations of conary on the same system (e.g., two terminals, a cron job racing with a manual install, or a carefully timed kill signal). You can also crash the conary process at the worst possible moment (power loss, SIGKILL, OOM kill). You understand SQLite internals, filesystem atomicity semantics, and Linux signal handling.

## Attack Goal

Corrupt the system database, CAS store, or generation state such that the system is left in an inconsistent or unrecoverable state. Secondary goals: create orphaned CAS objects that waste disk indefinitely, cause a generation to reference non-existent files, make rollback impossible, or silently lose package metadata.

## Attack Vectors to Explore

1. **SQLite WAL busy_timeout** -- What is the configured `busy_timeout` for database connections? If two operations race, does one fail gracefully or corrupt state? Can a long-running read (e.g., `conary list`) block a write (`conary install`) past the timeout, leaving a partial transaction?

2. **Transaction scope completeness** -- Round 2 found transaction scope issues. Verify that every multi-step database mutation (install, remove, update, adopt, rollback) is wrapped in a single transaction. Look for patterns where CAS operations happen outside the transaction, creating a window where DB and CAS are inconsistent.

3. **CAS orphan creation** -- If a CAS file is stored but the DB transaction that references it rolls back, the CAS file becomes an orphan. Map every code path where `CasStore::store()` is called and verify the corresponding DB insert is in the same failure domain. Does GC correctly find and remove these orphans?

4. **Generation number races** -- Two concurrent installs both read "current generation is N" and both try to create "generation N+1". What happens? Is there a database-level or filesystem-level lock that prevents this? Can this result in two different EROFS images both claiming to be generation N+1?

5. **File-based lock coverage** -- Is there a system-wide lock file that prevents concurrent mutating operations? If so, where is it, what is its scope, and are there operations that bypass it? Is the lock advisory or mandatory? Can it be broken by a stale lock from a crashed process?

6. **Crash recovery paths** -- For every multi-step mutation, identify the crash points and verify recovery:
   - Crash after CAS store, before DB commit -> orphaned CAS objects
   - Crash after DB commit, before EROFS build -> generation references files but no image
   - Crash after EROFS build, before symlink switch -> stale "current" pointer
   - Crash during /etc merge -> partially merged /etc
   - Crash during rollback -> rolled back to what state?

7. **state_cas_hashes consistency** -- The `state_cas_hashes` table (or equivalent) maps installed trove state to CAS objects. Under concurrent mutation, can this mapping become stale? If trove A is removed while trove B is being installed, and both reference the same CAS object, is the reference count correct?

8. **Signal handling during transactions** -- If the user hits Ctrl-C (SIGINT) during an install, is the current transaction rolled back cleanly? Are signal handlers registered? Do they set a flag that is checked at safe points, or do they interrupt mid-transaction?

9. **Temp file and staging directory races** -- Are temporary files and staging directories created with unique names (mkstemp/mkdtemp), or could two concurrent operations collide on the same path? Are they cleaned up on failure?

10. **EROFS image atomicity** -- Is the EROFS image written to a temp file and then renamed, or is it written in place? If the latter, a crash mid-write produces a corrupt image that cannot be mounted but is referenced by the DB.

11. **Lock starvation** -- Can a series of rapid read operations starve a write operation indefinitely? Can the reverse happen (writer starvation)? What is the fairness model?

12. **Journal mode and WAL checkpointing** -- Is SQLite configured in WAL mode? If so, when does checkpointing happen? Can a crash during checkpointing corrupt the database? Is the WAL file cleaned up on startup?

## Output Format

For each finding, report:

### [SEVERITY] FILE_A:LINE -> FILE_B:LINE -- Short title

**Boundary:** Which two modules/files this crosses
**Category:** TransactionGap | RaceCondition | CrashRecovery | LockCoverage | Orphan
**Exploitation chain:** Step-by-step scenario showing the race condition or crash timing that leads to corruption.
**Description:** What is wrong and why it matters.
**Suggested fix:** Concrete change at one or both sides of the boundary.

Severity levels:
- CRITICAL: Database corruption, unrecoverable state, or silent data loss
- HIGH: Inconsistent state that causes incorrect behavior on next operation
- MEDIUM: Orphaned resources or degraded performance under contention
- LOW: Theoretical race unlikely to manifest, or defense-in-depth gap

## Scope

You are NOT limited to specific files. Follow the attack wherever it leads across the entire codebase. Key starting points include `conary-core/src/db/`, `conary-core/src/filesystem/cas.rs`, `conary-core/src/generation/`, `conary-core/src/transaction/`, `src/commands/`, and any file that opens a database connection or acquires a lock, but trace into any file that participates in state mutation.

## Summary

- Critical: N
- High: N
- Medium: N
- Low: N

Top 3 concurrency/crash risks:
1. ...
2. ...
3. ...
