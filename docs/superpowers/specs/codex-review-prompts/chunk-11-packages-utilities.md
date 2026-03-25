You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 11: Packages + Core Utilities -- RPM/DEB/Arch package parsers, distro package queries (dpkg, pacman, rpm), canonical name mapping (AppStream, Repology), automated maintenance (security updates, orphan cleanup), container sandboxing (namespace isolation), scriptlet execution (cross-distro), triggers, component classification, derived package building, multi-algorithm hashing (SHA-256, XXH128), self-update, progress reporting, labels, and core error/util/json types. This is the utility foundation that everything else builds on.

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
conary-core/src/packages/archive_utils.rs
conary-core/src/packages/arch.rs
conary-core/src/packages/common.rs
conary-core/src/packages/cpio.rs
conary-core/src/packages/deb.rs
conary-core/src/packages/dpkg_query.rs
conary-core/src/packages/mod.rs
conary-core/src/packages/pacman_query.rs
conary-core/src/packages/query_common.rs
conary-core/src/packages/registry.rs
conary-core/src/packages/rpm_query.rs
conary-core/src/packages/rpm.rs
conary-core/src/packages/traits.rs
conary-core/src/canonical/appstream.rs
conary-core/src/canonical/client.rs
conary-core/src/canonical/discovery.rs
conary-core/src/canonical/mod.rs
conary-core/src/canonical/repology.rs
conary-core/src/canonical/rules.rs
conary-core/src/canonical/sync.rs
conary-core/src/automation/action.rs
conary-core/src/automation/check.rs
conary-core/src/automation/mod.rs
conary-core/src/automation/prompt.rs
conary-core/src/automation/scheduler.rs
conary-core/src/container/mod.rs
conary-core/src/scriptlet/mod.rs
conary-core/src/trigger/mod.rs
conary-core/src/components/classifier.rs
conary-core/src/components/filters.rs
conary-core/src/components/mod.rs
conary-core/src/derived/builder.rs
conary-core/src/derived/mod.rs
conary-core/src/mcp/mod.rs
conary-core/src/error.rs
conary-core/src/hash.rs
conary-core/src/json.rs
conary-core/src/label.rs
conary-core/src/lib.rs
conary-core/src/progress.rs
conary-core/src/self_update.rs
conary-core/src/util.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
