You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 10: Filesystem Domain -- CAS (content-addressable storage), EROFS generation building, composefs mounting, /etc merge logic, generation GC, mount operations, transaction planning, binary deltas (zstd dictionary compression), unified decompression, VFS tree, and fsverity. Data integrity at the storage layer and mount correctness are critical.

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
conary-core/src/generation/builder.rs
conary-core/src/generation/composefs.rs
conary-core/src/generation/composefs_rs_eval.rs
conary-core/src/generation/delta.rs
conary-core/src/generation/etc_merge.rs
conary-core/src/generation/gc.rs
conary-core/src/generation/metadata.rs
conary-core/src/generation/mod.rs
conary-core/src/generation/mount.rs
conary-core/src/transaction/mod.rs
conary-core/src/transaction/planner.rs
conary-core/src/filesystem/cas.rs
conary-core/src/filesystem/fsverity.rs
conary-core/src/filesystem/mod.rs
conary-core/src/filesystem/path.rs
conary-core/src/filesystem/vfs/mod.rs
conary-core/src/filesystem/vfs/tests.rs
conary-core/src/delta/applier.rs
conary-core/src/delta/generator.rs
conary-core/src/delta/metrics.rs
conary-core/src/delta/mod.rs
conary-core/src/compression/mod.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
