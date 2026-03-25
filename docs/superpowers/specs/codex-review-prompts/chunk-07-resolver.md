You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 7: Resolver + Dependencies -- SAT-based dependency resolution using resolvo, including the resolution engine, conflict analysis, plan generation, graph building, canonical name resolution, provider loading/matching/traits, dependency type definitions, detection, version parsing with constraints, and flavor (build variation) specs. Resolution correctness directly affects install safety.

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
conary-core/src/resolver/canonical.rs
conary-core/src/resolver/component_resolver.rs
conary-core/src/resolver/conflict.rs
conary-core/src/resolver/engine.rs
conary-core/src/resolver/graph.rs
conary-core/src/resolver/mod.rs
conary-core/src/resolver/plan.rs
conary-core/src/resolver/provider/loading.rs
conary-core/src/resolver/provider/matching.rs
conary-core/src/resolver/provider/mod.rs
conary-core/src/resolver/provider/traits.rs
conary-core/src/resolver/provider/types.rs
conary-core/src/resolver/sat.rs
conary-core/src/dependencies/classes.rs
conary-core/src/dependencies/detection.rs
conary-core/src/dependencies/mod.rs
conary-core/src/version/mod.rs
conary-core/src/flavor/mod.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
