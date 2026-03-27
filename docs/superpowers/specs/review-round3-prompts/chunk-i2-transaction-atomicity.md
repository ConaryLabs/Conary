You are performing an invariant verification review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 exhaustively verifies that critical system invariants hold everywhere.

This is chunk-i2-transaction-atomicity.

## Contract
Every `db::transaction()` closure fully commits or fully rolls back. No partial state is observable. Non-DB side effects are correctly ordered relative to transaction boundaries.

## What To Verify

1. Every `transaction()` closure performs ONLY database operations (queries, inserts, updates, deletes). No filesystem writes, network calls, subprocess spawns, or other side effects inside the closure that would not be rolled back if the transaction fails.
2. Non-DB side effects (CAS writes, subprocess execution, file creation, mount operations) happen BEFORE the transaction (prepare, then commit atomically) or AFTER (commit first, then perform effect) -- never inside.
3. Every error path within a transaction closure propagates the error correctly so the transaction rolls back. No swallowed errors (e.g., `unwrap()`, `let _ =`, `ok()`, silent `match` arms) that would allow the transaction to commit despite a failed operation.
4. No nested transactions that could cause subtle commit/rollback ordering issues. If nested transactions are used intentionally (savepoints), verify they are correctly scoped.
5. Sequential transactions that together represent a logical operation maintain consistency. If transaction A commits but transaction B fails, the system is left in a valid intermediate state, not a corrupted one.
6. `state_cas_hashes` is populated atomically with its corresponding state snapshot -- the hash list and the state it describes are written in the same transaction, never in separate transactions where one could commit without the other.
7. Transaction closures do not hold locks, mutexes, or other synchronization primitives that could deadlock if the transaction retries or rolls back.
8. Long-running operations (large batch inserts, full table scans) inside transactions do not cause lock contention or timeouts that would affect concurrent readers.
9. Every `batch_insert()` or bulk operation uses the same connection/transaction context -- no accidental use of a separate connection that would bypass the transaction boundary.
10. Return values from transaction closures correctly propagate the committed data. No pattern where the closure returns computed values that become stale if the transaction is retried.
11. WAL mode and journal settings are consistent with the atomicity guarantees expected by the code.
12. No `PRAGMA` statements inside transaction closures that could alter transaction behavior mid-flight.

## Output Format

For each violation, report:

### [SEVERITY] Invariant violation title

**Location:** file:line
**Violation:** What breaks the contract and how
**Impact:** What goes wrong when this invariant is violated
**Suggested fix:** Concrete change to restore the invariant

Severity levels:
- CRITICAL: Invariant violated on a reachable code path with significant impact
- HIGH: Invariant violated but requires specific conditions to trigger
- MEDIUM: Invariant weakly held (defense-in-depth gap)
- LOW: Invariant technically holds but is fragile/undocumented

## Scope
Search the ENTIRE codebase for every location where this invariant is relevant. Do not limit to specific files. Pay particular attention to:
- `conary-core/src/db/` (database layer, schema, migrations)
- `conary-core/src/transaction/` (transaction engine)
- `conary-core/src/generation/` (generation building involves DB + filesystem)
- `conary-core/src/repository/` (repo sync writes metadata to DB)
- `conary-core/src/bootstrap/` (bootstrap pipeline)
- Any file that calls `db.transaction()`, `conn.execute_batch()`, or `rusqlite::Transaction`

## Summary
- Critical: N
- High: N
- Medium: N
- Low: N
