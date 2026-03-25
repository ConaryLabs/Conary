You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 16: Test Infrastructure -- the conary-test crate providing a declarative TOML-based test engine with container management (bollard/Podman), step execution, assertions, mock HTTP server, QEMU boot support, variable substitution, container coordination/setup, JSON reporting, SSE streaming, HTTP API server (14 endpoints), MCP server (23 tools), write-ahead log for result buffering, Remi client for result pushing, auth, and CLI entrypoint. While this is test infrastructure (not production code path), correctness matters for CI reliability.

Review the following files with maximum thoroughness across ALL of these dimensions:

## 1. Correctness
- Logic bugs, off-by-one errors, incorrect boundary conditions
- Error handling gaps: unwrap() on fallible operations, silent error swallowing, missing error propagation
- Race conditions or unsafe concurrent access
- Unsound unsafe blocks (if any)
- Integer overflow/underflow, truncation issues
- Resource leaks (file handles, connections, temp files, containers)

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
- Container escape vectors

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
conary-test/src/cli.rs
conary-test/src/config/distro.rs
conary-test/src/config/manifest.rs
conary-test/src/config/mod.rs
conary-test/src/container/backend.rs
conary-test/src/container/image.rs
conary-test/src/container/lifecycle.rs
conary-test/src/container/mock.rs
conary-test/src/container/mod.rs
conary-test/src/engine/assertions.rs
conary-test/src/engine/container_coordinator.rs
conary-test/src/engine/container_setup.rs
conary-test/src/engine/executor.rs
conary-test/src/engine/mock_server.rs
conary-test/src/engine/mod.rs
conary-test/src/engine/qemu.rs
conary-test/src/engine/runner.rs
conary-test/src/engine/suite.rs
conary-test/src/engine/variables.rs
conary-test/src/error.rs
conary-test/src/error_taxonomy.rs
conary-test/src/lib.rs
conary-test/src/report/json.rs
conary-test/src/report/mod.rs
conary-test/src/report/stream.rs
conary-test/src/server/auth.rs
conary-test/src/server/handlers.rs
conary-test/src/server/mcp.rs
conary-test/src/server/mod.rs
conary-test/src/server/remi_client.rs
conary-test/src/server/routes.rs
conary-test/src/server/service.rs
conary-test/src/server/state.rs
conary-test/src/server/wal.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
