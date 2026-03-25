# Codex 5.4 Deep Code Review -- Chunking Design

## Goal

Full deep review of the entire Conary codebase (~212K lines Rust, 501 files, 4 crates) using OpenAI Codex CLI with GPT 5.4 at `xhigh` reasoning effort. Review covers correctness, architecture, security, idiom/quality. Findings are fixed per-chunk before moving to the next.

## Constraints

- GPT 5.4 standard context: 272K tokens (1M experimental)
- `xhigh` reasoning burns tokens fast -- smaller cohesive chunks preferred
- Codex has full repo access; prompts specify which files to focus on
- Fix-as-you-go: review chunk N, fix chunk N, then move to chunk N+1

## Approach: Module-domain chunking (Approach A)

16 chunks following natural module boundaries. Ordered by risk/criticality so the most important code is reviewed first. Target ~8-18K lines per chunk to leave headroom for xhigh reasoning.

## Chunk Definitions

### Chunk 1: Database Layer (18K lines, 44 files)

**Domain:** SQLite schema, migrations, all data models.
**Why first:** Foundation -- every bug here cascades through the entire system.

```
conary-core/src/db/
```

---

### Chunk 2: Server Core (17K lines, 30 files)

**Domain:** Remi server infrastructure -- auth, routing, rate limiting, caching, security, MCP, analytics, background jobs.
**Why early:** Network-facing attack surface, auth logic.

```
conary-server/src/server/*.rs  (not handlers/)
```

---

### Chunk 3: Server Handlers (12K lines, 30 files)

**Domain:** All HTTP/API request handlers including admin endpoints.
**Why early:** Input validation, authorization checks, response construction.

```
conary-server/src/server/handlers/
```

---

### Chunk 4: Repository (15K lines, 27 files)

**Domain:** Remote repository sync, metadata parsing, mirror health, download, retry, GPG, versioning, resolution policy.
**Why important:** Network I/O, untrusted input parsing, version comparison correctness.

```
conary-core/src/repository/
```

---

### Chunk 5: CCS Format (16K lines, 37 files)

**Domain:** Native package format -- builder, reader, signing, verification, policy, conversion from legacy formats, OCI export, enhancement hooks.
**Why important:** Package integrity, signing correctness, format parsing from untrusted sources.

```
conary-core/src/ccs/
```

---

### Chunk 6: Security Domain (12K lines, 27 files)

**Domain:** TUF trust, capability declarations/enforcement (landlock, seccomp), provenance/SLSA/DNA tracking.
**Why important:** Crypto, sandbox enforcement, supply chain integrity.

```
conary-core/src/trust/
conary-core/src/capability/
conary-core/src/provenance/
```

---

### Chunk 7: Resolver + Dependencies (10K lines, 18 files)

**Domain:** SAT-based dependency resolution (resolvo), version parsing, flavor specs, dependency types, provider matching.
**Why important:** Correctness of resolution directly affects install safety.

```
conary-core/src/resolver/
conary-core/src/dependencies/
conary-core/src/version/
conary-core/src/flavor/
```

---

### Chunk 8: Derivation + Bootstrap (15K lines, 34 files)

**Domain:** CAS-layered derivation engine (19 files), 6-phase bootstrap pipeline (15 files).
**Why important:** Build reproducibility, chroot isolation, convergence correctness.

```
conary-core/src/derivation/
conary-core/src/bootstrap/
```

---

### Chunk 9: Model + Recipe (13K lines, 21 files)

**Domain:** System Model (declarative OS state, diff, lockfile, signing, replatform), recipe system (parser, cook, PKGBUILD, audit, graph).
**Why important:** Declarative state correctness, build hermeticity.

```
conary-core/src/model/
conary-core/src/recipe/
```

---

### Chunk 10: Filesystem Domain (8K lines, 22 files)

**Domain:** CAS storage, EROFS generation building, composefs mounting, /etc merge, GC, transactions, deltas, compression, VFS tree, fsverity.
**Why important:** Data integrity at the storage layer, mount correctness.

```
conary-core/src/generation/
conary-core/src/transaction/
conary-core/src/filesystem/
conary-core/src/delta/
conary-core/src/compression/
```

---

### Chunk 11: Packages + Core Utilities (17K lines, 42 files)

**Domain:** RPM/DEB/Arch parsers, canonical name mapping, automation, container sandboxing, scriptlets, triggers, components, derived packages, hashing, self-update, progress, labels, errors.
**Why grouped:** Individually small modules, utility code that supports everything above.

```
conary-core/src/packages/
conary-core/src/canonical/
conary-core/src/automation/
conary-core/src/container/
conary-core/src/scriptlet/
conary-core/src/trigger/
conary-core/src/components/
conary-core/src/derived/
conary-core/src/mcp/
conary-core/src/*.rs  (top-level: error, hash, json, label, lib, progress, self_update, util)
```

---

### Chunk 12: CLI Commands -- Subdirectories (14K lines, 45 files)

**Domain:** Complex multi-file commands: install, adopt, ccs, generation, query, bootstrap.
**Why grouped:** Each subdirectory is a self-contained command implementation.

```
src/commands/install/
src/commands/adopt/
src/commands/ccs/
src/commands/generation/
src/commands/query/
src/commands/bootstrap/
```

---

### Chunk 13: CLI Commands -- Top-level (15K lines, 38 files)

**Domain:** Single-file command implementations: model, provenance, update, system, export, federation, capability, automation, etc.
**Why separate from chunk 12:** These are simpler, self-contained files.

```
src/commands/*.rs  (top-level files only, not subdirectories)
```

---

### Chunk 14: CLI Definitions + Entrypoint (6K lines, 29 files)

**Domain:** Clap CLI definitions, argument parsing, main dispatch.
**Why last in CLI:** Low risk, mostly declarative.

```
src/cli/
src/main.rs
```

---

### Chunk 15: Daemon + Federation (11K lines, 20 files)

**Domain:** conaryd daemon (REST API, jobs, systemd, socket), CAS federation (peer discovery, chunk routing, circuit breakers, mDNS).

```
conary-server/src/daemon/
conary-server/src/federation/
conary-server/src/bin/
conary-server/src/lib.rs
```

---

### Chunk 16: Test Infrastructure (14K lines, 34 files)

**Domain:** conary-test engine, TOML manifest parsing, container management (bollard), step execution, assertions, mock server, HTTP API, MCP server, WAL, reporting.
**Why last:** Test infra, not production code path.

```
conary-test/src/
```

---

## Review Prompt

The following prompt is reused for each chunk. Replace `CHUNK_NAME`, `CHUNK_DESCRIPTION`, and `FILES` for each run.

````
You are performing a deep code review of a Rust codebase (Conary, a next-generation Linux package manager). This is CHUNK_NAME: CHUNK_DESCRIPTION.

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

FILES

Review every file listed. Do not skip files. Do not summarize files without reviewing them.
````

## Workflow

1. Copy the prompt template
2. Fill in CHUNK_NAME, CHUNK_DESCRIPTION, and FILES for the current chunk
3. Run: `codex "PROMPT"` (or pipe via stdin)
4. Bring results back to Claude Code session
5. Triage and fix findings together
6. Move to the next chunk
7. Repeat until all 16 chunks are complete

## Chunk Tracker

| # | Chunk | Status |
|---|-------|--------|
| 1 | Database | Pending |
| 2 | Server Core | Pending |
| 3 | Server Handlers | Pending |
| 4 | Repository | Pending |
| 5 | CCS Format | Pending |
| 6 | Security Domain | Pending |
| 7 | Resolver + Deps | Pending |
| 8 | Derivation + Bootstrap | Pending |
| 9 | Model + Recipe | Pending |
| 10 | Filesystem Domain | Pending |
| 11 | Packages + Utilities | Pending |
| 12 | CLI Commands (subdirs) | Pending |
| 13 | CLI Commands (top-level) | Pending |
| 14 | CLI Definitions | Pending |
| 15 | Daemon + Federation | Pending |
| 16 | Test Infrastructure | Pending |
