You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 8: Derivation + Bootstrap -- the CAS-layered derivation engine (19 files: pipeline, compose, capture, build_order, executor, environment, convergence, graph, index, etc.) and the 6-phase bootstrap pipeline (toolchain, temp tools, chroot env, cross tools, final system, image building). Build reproducibility, chroot isolation, and convergence correctness are critical.

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
conary-core/src/derivation/build_order.rs
conary-core/src/derivation/capture.rs
conary-core/src/derivation/compose.rs
conary-core/src/derivation/convergence.rs
conary-core/src/derivation/environment.rs
conary-core/src/derivation/executor.rs
conary-core/src/derivation/graph.rs
conary-core/src/derivation/id.rs
conary-core/src/derivation/index.rs
conary-core/src/derivation/install.rs
conary-core/src/derivation/manifest.rs
conary-core/src/derivation/mod.rs
conary-core/src/derivation/output.rs
conary-core/src/derivation/pipeline.rs
conary-core/src/derivation/profile.rs
conary-core/src/derivation/recipe_hash.rs
conary-core/src/derivation/seed.rs
conary-core/src/derivation/substituter.rs
conary-core/src/derivation/test_helpers.rs
conary-core/src/bootstrap/adopt_seed.rs
conary-core/src/bootstrap/build_helpers.rs
conary-core/src/bootstrap/build_runner.rs
conary-core/src/bootstrap/chroot_env.rs
conary-core/src/bootstrap/config.rs
conary-core/src/bootstrap/cross_tools.rs
conary-core/src/bootstrap/final_system.rs
conary-core/src/bootstrap/image.rs
conary-core/src/bootstrap/mod.rs
conary-core/src/bootstrap/repart.rs
conary-core/src/bootstrap/stages.rs
conary-core/src/bootstrap/system_config.rs
conary-core/src/bootstrap/temp_tools.rs
conary-core/src/bootstrap/tier2.rs
conary-core/src/bootstrap/toolchain.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
