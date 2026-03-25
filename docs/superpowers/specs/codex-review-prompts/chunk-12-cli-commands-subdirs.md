You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 12: CLI Commands (subdirectories) -- complex multi-file command implementations: install (batch, blocklist, conversion, dependencies, execution, preparation, resolution, scriptlets, system PM integration), adopt (conflicts, conversion, hooks, packages, refresh, status, system), CCS (build, enhance, init, inspect, install, runtime, signing), generation (boot, builder, commands, composefs, metadata, switch, takeover), query (components, dependency, deptree, history, package, reason, repo, sbom), and bootstrap.

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
src/commands/install/batch.rs
src/commands/install/blocklist.rs
src/commands/install/conversion.rs
src/commands/install/dependencies.rs
src/commands/install/dep_mode.rs
src/commands/install/dep_resolution.rs
src/commands/install/execute.rs
src/commands/install/mod.rs
src/commands/install/prepare.rs
src/commands/install/resolve.rs
src/commands/install/scriptlets.rs
src/commands/install/system_pm.rs
src/commands/adopt/conflicts.rs
src/commands/adopt/convert.rs
src/commands/adopt/hooks.rs
src/commands/adopt/mod.rs
src/commands/adopt/packages.rs
src/commands/adopt/refresh.rs
src/commands/adopt/status.rs
src/commands/adopt/system.rs
src/commands/ccs/build.rs
src/commands/ccs/enhance.rs
src/commands/ccs/init.rs
src/commands/ccs/inspect.rs
src/commands/ccs/install.rs
src/commands/ccs/mod.rs
src/commands/ccs/runtime.rs
src/commands/ccs/signing.rs
src/commands/generation/boot.rs
src/commands/generation/builder.rs
src/commands/generation/commands.rs
src/commands/generation/composefs.rs
src/commands/generation/metadata.rs
src/commands/generation/mod.rs
src/commands/generation/switch.rs
src/commands/generation/takeover.rs
src/commands/query/components.rs
src/commands/query/dependency.rs
src/commands/query/deptree.rs
src/commands/query/history.rs
src/commands/query/mod.rs
src/commands/query/package.rs
src/commands/query/reason.rs
src/commands/query/repo.rs
src/commands/query/sbom.rs
src/commands/bootstrap/mod.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
