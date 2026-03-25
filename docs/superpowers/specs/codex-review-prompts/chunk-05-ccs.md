You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is Chunk 5: CCS Format -- the native package format including builder, archive reader, signing, verification, policy engine, manifest handling, conversion from legacy formats (RPM, DEB, Arch), OCI export, enhancement hooks (systemd, sysctl, tmpfiles, alternatives, user/group), lockfile, chunking, and binary manifest. Package integrity and signing correctness are critical here.

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
conary-core/src/ccs/archive_reader.rs
conary-core/src/ccs/binary_manifest.rs
conary-core/src/ccs/builder.rs
conary-core/src/ccs/chunking.rs
conary-core/src/ccs/convert/analyzer.rs
conary-core/src/ccs/convert/capture.rs
conary-core/src/ccs/convert/converter.rs
conary-core/src/ccs/convert/fidelity.rs
conary-core/src/ccs/convert/legacy_provenance.rs
conary-core/src/ccs/convert/mock.rs
conary-core/src/ccs/convert/mod.rs
conary-core/src/ccs/enhancement/context.rs
conary-core/src/ccs/enhancement/error.rs
conary-core/src/ccs/enhancement/mod.rs
conary-core/src/ccs/enhancement/registry.rs
conary-core/src/ccs/enhancement/runner.rs
conary-core/src/ccs/export/mod.rs
conary-core/src/ccs/export/oci.rs
conary-core/src/ccs/hooks/alternatives.rs
conary-core/src/ccs/hooks/directory.rs
conary-core/src/ccs/hooks/mod.rs
conary-core/src/ccs/hooks/sysctl.rs
conary-core/src/ccs/hooks/systemd.rs
conary-core/src/ccs/hooks/tmpfiles.rs
conary-core/src/ccs/hooks/user_group.rs
conary-core/src/ccs/inspector.rs
conary-core/src/ccs/legacy/arch.rs
conary-core/src/ccs/legacy/deb.rs
conary-core/src/ccs/legacy/mod.rs
conary-core/src/ccs/legacy/rpm.rs
conary-core/src/ccs/lockfile.rs
conary-core/src/ccs/manifest.rs
conary-core/src/ccs/mod.rs
conary-core/src/ccs/package.rs
conary-core/src/ccs/policy.rs
conary-core/src/ccs/signing.rs
conary-core/src/ccs/verify.rs
```

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
