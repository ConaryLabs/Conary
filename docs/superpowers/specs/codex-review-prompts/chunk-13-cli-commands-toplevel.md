You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 13: CLI Commands (top-level) -- single-file command implementations for model (2K lines), provenance, update, system, export, federation, automation, capability, collection, config, derivation, derived, distro, label, groups, trust, remove, repo, state, verify, triggers, self-update, and more. These wire CLI arguments to core library calls.

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
src/commands/automation.rs
src/commands/cache.rs
src/commands/canonical.rs
src/commands/capability.rs
src/commands/collection.rs
src/commands/composefs_ops.rs
src/commands/config.rs
src/commands/convert_pkgbuild.rs
src/commands/cook.rs
src/commands/derivation.rs
src/commands/derivation_sbom.rs
src/commands/derived.rs
src/commands/distro.rs
src/commands/export.rs
src/commands/federation.rs
src/commands/groups.rs
src/commands/label.rs
src/commands/model.rs
src/commands/mod.rs
src/commands/package_parsing.rs
src/commands/profile.rs
src/commands/progress.rs
src/commands/provenance.rs
src/commands/recipe_audit.rs
src/commands/redirect.rs
src/commands/registry.rs
src/commands/remove.rs
src/commands/replatform_rendering.rs
src/commands/repo.rs
src/commands/restore.rs
src/commands/self_update.rs
src/commands/state.rs
src/commands/system.rs
src/commands/test_helpers.rs
src/commands/triggers.rs
src/commands/trust.rs
src/commands/update_channel.rs
src/commands/update.rs
src/commands/verify.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
