You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 14: CLI Definitions + Entrypoint -- Clap CLI argument definitions (28 subcommand modules), the main dispatch logic, and the binary entrypoint. This is mostly declarative but check for argument validation gaps, default value correctness, and dispatch errors.

Review the following files with maximum thoroughness across ALL of these dimensions:

## 1. Correctness
- Logic bugs, off-by-one errors, incorrect boundary conditions
- Error handling gaps: unwrap() on fallible operations, silent error swallowing, missing error propagation
- Race conditions or unsafe concurrent access
- Unsound unsafe blocks (if any)
- Integer overflow/underflow, truncation issues
- Resource leaks (file handles, connections, temp files)

## 2. Architecture
- Module boundary violations, inappropriate coupling between modules
- Abstraction quality: leaky abstractions, wrong abstraction level
- Dead code, unreachable branches, unused imports/parameters
- Inconsistent patterns across the module (different error handling styles, naming, etc.)
- Functions that are too long or do too many things

## 3. Security
- Input validation gaps on untrusted data (network, file, user)
- Path traversal, symlink attacks, TOCTOU
- Injection risks (SQL, command, format string)
- Credential/secret handling issues
- Unsafe deserialization, type confusion
- Denial of service vectors (unbounded allocations, infinite loops, regex DoS)

## 4. Rust Idiom & Quality
- Non-idiomatic Rust: unnecessary clones, Box where not needed, String where &str suffices
- Missing or incorrect derives (Clone, Debug, etc.)
- Lifetime issues, unnecessary 'static bounds
- match arms that should be if-let or vice versa
- Opportunities to use standard library APIs (iterators, Entry API, etc.)
- Clippy-level issues (manual implementations of standard patterns)

## Output Format

For each finding, report:

```
### [SEVERITY] FILE:LINE -- Short title

**Category:** Correctness | Architecture | Security | Idiom
**Description:** What's wrong and why it matters.
**Suggested fix:** Concrete code change or approach.
```

Severity levels:
- CRITICAL: Will cause data loss, security breach, or crash in production
- HIGH: Significant bug or security issue, likely to manifest
- MEDIUM: Code smell, minor bug, or issue that makes future bugs likely
- LOW: Style, idiom, or minor improvement

At the end, provide a summary:

```
## Summary
- Critical: N
- High: N
- Medium: N
- Low: N

Top 3 most important findings:
1. ...
2. ...
3. ...
```

## Files to Review

```
src/cli/automation.rs
src/cli/bootstrap.rs
src/cli/cache.rs
src/cli/canonical.rs
src/cli/capability.rs
src/cli/ccs.rs
src/cli/collection.rs
src/cli/config.rs
src/cli/derivation.rs
src/cli/derive.rs
src/cli/distro.rs
src/cli/federation.rs
src/cli/generation.rs
src/cli/groups.rs
src/cli/label.rs
src/cli/model.rs
src/cli/mod.rs
src/cli/profile.rs
src/cli/provenance.rs
src/cli/query.rs
src/cli/redirect.rs
src/cli/registry.rs
src/cli/repo.rs
src/cli/state.rs
src/cli/system.rs
src/cli/trigger.rs
src/cli/trust.rs
src/cli/verify.rs
src/main.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
