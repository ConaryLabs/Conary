You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 2: Server Core -- Remi server infrastructure including auth, routing, rate limiting, caching, security hardening, MCP endpoint, analytics, background jobs, canonical mapping, chunk GC, and conversion. This is the network-facing attack surface.

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
conary-server/src/server/admin_service.rs
conary-server/src/server/analytics.rs
conary-server/src/server/artifact_paths.rs
conary-server/src/server/audit.rs
conary-server/src/server/auth.rs
conary-server/src/server/bloom.rs
conary-server/src/server/cache.rs
conary-server/src/server/canonical_fetch.rs
conary-server/src/server/canonical_job.rs
conary-server/src/server/chunk_gc.rs
conary-server/src/server/config.rs
conary-server/src/server/conversion.rs
conary-server/src/server/delta_manifests.rs
conary-server/src/server/federated_index.rs
conary-server/src/server/forgejo.rs
conary-server/src/server/index_gen.rs
conary-server/src/server/jobs.rs
conary-server/src/server/lite.rs
conary-server/src/server/mcp.rs
conary-server/src/server/metrics.rs
conary-server/src/server/mod.rs
conary-server/src/server/negative_cache.rs
conary-server/src/server/popularity.rs
conary-server/src/server/prewarm.rs
conary-server/src/server/r2.rs
conary-server/src/server/rate_limit.rs
conary-server/src/server/routes.rs
conary-server/src/server/search.rs
conary-server/src/server/security.rs
conary-server/src/server/test_db.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
