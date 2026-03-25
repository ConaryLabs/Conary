You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 6: Security Domain -- TUF supply chain trust (key ceremony, metadata generation, client verification), capability declarations with enforcement (landlock, seccomp) and inference (binary analysis, heuristics, well-known patterns), and provenance tracking (SLSA, DNA fingerprinting, build/content/source provenance, signatures). Crypto correctness and sandbox escape prevention are critical here.

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
- Crypto misuse: weak algorithms, nonce reuse, timing side channels, improper key handling

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
conary-core/src/trust/ceremony.rs
conary-core/src/trust/client.rs
conary-core/src/trust/generate.rs
conary-core/src/trust/keys.rs
conary-core/src/trust/metadata.rs
conary-core/src/trust/mod.rs
conary-core/src/trust/verify.rs
conary-core/src/capability/declaration.rs
conary-core/src/capability/enforcement/landlock_enforce.rs
conary-core/src/capability/enforcement/mod.rs
conary-core/src/capability/enforcement/seccomp_enforce.rs
conary-core/src/capability/inference/binary.rs
conary-core/src/capability/inference/cache.rs
conary-core/src/capability/inference/confidence.rs
conary-core/src/capability/inference/error.rs
conary-core/src/capability/inference/heuristics.rs
conary-core/src/capability/inference/mod.rs
conary-core/src/capability/inference/wellknown.rs
conary-core/src/capability/mod.rs
conary-core/src/capability/policy.rs
conary-core/src/capability/resolver.rs
conary-core/src/provenance/build.rs
conary-core/src/provenance/content.rs
conary-core/src/provenance/dna.rs
conary-core/src/provenance/mod.rs
conary-core/src/provenance/signature.rs
conary-core/src/provenance/slsa.rs
conary-core/src/provenance/source.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
