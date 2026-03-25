You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 3: Server Handlers -- all HTTP/API request handlers for the Remi package server, including public endpoints (chunks, OCI, packages, index, sparse, TUF, search, self-update) and admin endpoints (tokens, CI, repos, federation, audit, events, artifacts, packages, test data). Focus on input validation, authorization, and response construction.

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
conary-server/src/server/handlers/admin/artifacts.rs
conary-server/src/server/handlers/admin/audit.rs
conary-server/src/server/handlers/admin/ci.rs
conary-server/src/server/handlers/admin/events.rs
conary-server/src/server/handlers/admin/federation.rs
conary-server/src/server/handlers/admin/mod.rs
conary-server/src/server/handlers/admin/packages.rs
conary-server/src/server/handlers/admin/repos.rs
conary-server/src/server/handlers/admin/test_data.rs
conary-server/src/server/handlers/admin/tokens.rs
conary-server/src/server/handlers/artifacts.rs
conary-server/src/server/handlers/canonical.rs
conary-server/src/server/handlers/chunks.rs
conary-server/src/server/handlers/derivations.rs
conary-server/src/server/handlers/detail.rs
conary-server/src/server/handlers/federation.rs
conary-server/src/server/handlers/index.rs
conary-server/src/server/handlers/jobs.rs
conary-server/src/server/handlers/models.rs
conary-server/src/server/handlers/mod.rs
conary-server/src/server/handlers/oci.rs
conary-server/src/server/handlers/openapi.rs
conary-server/src/server/handlers/packages.rs
conary-server/src/server/handlers/profiles.rs
conary-server/src/server/handlers/recipes.rs
conary-server/src/server/handlers/search.rs
conary-server/src/server/handlers/seeds.rs
conary-server/src/server/handlers/self_update.rs
conary-server/src/server/handlers/sparse.rs
conary-server/src/server/handlers/tuf.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
