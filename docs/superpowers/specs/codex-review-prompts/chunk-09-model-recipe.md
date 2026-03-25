You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 9: Model + Recipe -- the System Model (declarative OS state management with diff, lockfile, parser, signing, replatform/convergence planning, remote sync) and the recipe system (parser, cook, PKGBUILD conversion, audit, cache, dependency graph, kitchen/archive/config/makedepends/provenance capture). Declarative state correctness and build hermeticity matter here.

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
conary-core/src/model/diff.rs
conary-core/src/model/lockfile.rs
conary-core/src/model/mod.rs
conary-core/src/model/parser.rs
conary-core/src/model/remote.rs
conary-core/src/model/replatform.rs
conary-core/src/model/signing.rs
conary-core/src/model/state.rs
conary-core/src/recipe/audit.rs
conary-core/src/recipe/cache.rs
conary-core/src/recipe/format.rs
conary-core/src/recipe/graph.rs
conary-core/src/recipe/kitchen/archive.rs
conary-core/src/recipe/kitchen/config.rs
conary-core/src/recipe/kitchen/cook.rs
conary-core/src/recipe/kitchen/makedepends.rs
conary-core/src/recipe/kitchen/mod.rs
conary-core/src/recipe/kitchen/provenance_capture.rs
conary-core/src/recipe/mod.rs
conary-core/src/recipe/parser.rs
conary-core/src/recipe/pkgbuild.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
