You are performing an invariant verification review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 exhaustively verifies that critical system invariants hold everywhere.

This is chunk-i4-test-coverage-gaps.

## Contract
Every public function has meaningful test coverage. Dead code, untested error paths, and unreachable branches are identified.

## What To Verify

1. Public functions (`pub fn`) that are never called outside their own module -- these are dead API surface. Check for `pub` functions that have no callers in other modules, no test coverage, and no `#[allow(dead_code)]` annotation. Distinguish between intentionally public (trait implementations, CLI entry points) and accidentally public.
2. Error variants (enum variants in error types) that are never constructed anywhere in the codebase. These indicate dead error paths or missing error handling for conditions that should be covered.
3. Unreachable match arms that use `unreachable!()`, `panic!()`, or wildcard `_` catches where the compiler cannot prove exhaustiveness -- if the arm is truly unreachable, it should be documented why; if it is reachable, it needs a test.
4. `#[cfg(test)]` blocks that change behavior rather than just adding test functions. Any `cfg(test)` that modifies production logic (e.g., conditional compilation of struct fields, different function signatures, mock implementations) creates a divergence between tested and shipped code.
5. Functions exceeding 100 lines with no corresponding test. Long functions are high-risk for untested branches and deserve explicit test coverage.
6. Panic paths in production code: every `unwrap()`, `expect()`, `.unwrap_or_else(|| panic!())`, and `unreachable!()` in non-test code. For each one, determine whether the panic is reachable in production. If it is, it should either be converted to proper error handling or have a test proving the precondition always holds.
7. `todo!()` and `unimplemented!()` macros in non-test code. These are runtime panics that indicate incomplete implementation. Each one is a potential crash in production.
8. Error paths that are tested only for the happy case. Look for `Result`-returning functions where tests only assert `is_ok()` and never exercise the error branches.
9. Unsafe blocks without corresponding safety tests. Any `unsafe` code should have tests that exercise the safety-critical boundary conditions.
10. Integration points (database operations, filesystem operations, network calls) where error injection is absent -- the code handles errors in theory but no test verifies the error handling works in practice.
11. Generic functions or trait implementations where only one concrete type is ever used in tests, leaving other instantiations untested.
12. Conditional compilation features (`#[cfg(feature = "...")]`) where the feature-gated code paths lack dedicated tests.

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
- `conary-core/src/` (all core library modules)
- `src/commands/` (CLI command implementations)
- `src/cli/` (CLI definitions)
- `conary-server/src/` (server modules, feature-gated)
- `conary-test/src/` (test infrastructure itself)
- Every `mod tests` block and every `#[test]` function

## Summary
- Critical: N
- High: N
- Medium: N
- Low: N
