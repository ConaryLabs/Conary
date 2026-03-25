# Feature Domain Code Review Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deep code review of the entire Conary codebase (172K lines, 4 crates) split into 11 feature domains, then fix all findings P0-P3.

**Architecture:** Phase 1 dispatches 11 parallel read-only lintian agents (one per feature domain). Phase 2 merges findings into a consolidated report. Phase 3 remediates. User checkpoint between Phase 2 and Phase 3.

**Tech Stack:** Rust 1.94, Edition 2024, 4-crate workspace (conary, conary-core, conary-server, conary-test)

**Spec:** `docs/superpowers/specs/2026-03-24-feature-domain-code-review-design.md`

---

## Reference: Shared Review Methodology

Every lintian dispatch in this plan MUST include this methodology verbatim in its prompt. Do not summarize or abbreviate it — the agent needs the full text.

```
REVIEW METHODOLOGY — apply all six dimensions to every file in your scope.

1. CORRECTNESS
- Logic bugs, off-by-one errors, incorrect error propagation
- unwrap()/expect() on fallible production paths
- Race conditions, deadlock potential in async code
- SQL injection or other injection vectors

2. CODE QUALITY
- Dead code, unused imports, unreachable branches
- Copy-paste duplication (within the feature and across features)
- Functions that are too long or do too many things
- Naming consistency — does the same concept use the same word everywhere?

3. IDIOMATIC RUST
- Proper use of Result/Option combinators vs. verbose match chains
- Ownership patterns — unnecessary clones, borrow checker workarounds
- Type system usage — enum vs. string, newtype wrappers
- Edition 2024 / Rust 1.94 features that could simplify code

4. AI SLOP DETECTION
- Over-commented obvious code ("// increment the counter")
- Defensive code that can't fail ("check if vec is empty, then iterate")
- Boilerplate that should be abstracted vs. abstractions that shouldn't exist
- Inconsistent patterns between modules suggesting different generation sessions
- TODO/FIXME/placeholder stubs that were never filled in

5. SECURITY
- Input validation at system boundaries
- Path traversal, symlink attacks (CAS/filesystem code)
- Crypto usage — proper key handling, no hardcoded secrets
- Privilege handling — scriptlet sandboxing, daemon auth

6. ARCHITECTURE
- Module boundaries — anything in the wrong place?
- Public API surface — too much exposed?
- Error types — informative and consistent?
- Cross-module coupling that shouldn't exist

SEVERITY LEVELS:
- P0: Data loss, production panic, security vulnerability
- P1: Incorrect behavior, significant code smell, looks bad to reviewers
- P2: Improvement opportunity, minor inconsistency, cleanup
- P3: Nitpick, style preference

CROSS-DOMAIN FINDINGS: If a finding originates in a file outside your domain, note it with the owning domain number (e.g., "[Feature 2]"). Phase 2 triage will route it.

OUTPUT FORMAT — structure your report exactly like this:

## Feature N: [Name] — Review Findings

### Summary
[2-3 sentences: overall impression, biggest concern, how many findings per severity]

### P0 — Critical
#### [N]. [Short title]
- **File:** `path/to/file.rs:line`
- **Category:** [Correctness|Quality|Idiomatic|Slop|Security|Architecture]
- **Finding:** [What's wrong]
- **Fix:** [What to do]

### P1 — Important
[same structure]

### P2 — Medium
[same structure]

### P3 — Minor
[same structure]

### Cross-Domain Notes
[Findings that belong to other feature domains]
```

---

## Chunk 1: Parallel Feature Reviews (Phase 1)

All 11 tasks in this chunk are independent and MUST be dispatched in parallel. Each task dispatches one lintian agent with its domain-specific file scope and the shared review methodology above.

**CRITICAL — Prompt Composition:** Each task below contains a lintian dispatch prompt with the placeholder `[INSERT SHARED REVIEW METHODOLOGY HERE]`. Before dispatching, you MUST replace that placeholder with the full text from the "Reference: Shared Review Methodology" section above (everything inside the code fence, from "REVIEW METHODOLOGY" through "### Cross-Domain Notes"). Do NOT send the placeholder string literally.

**Directory Setup:** Before dispatching any reviews, create `docs/superpowers/reviews/` if it does not exist:
```bash
mkdir -p docs/superpowers/reviews
```

### Task 1: Review Feature 0 — Repo Presentation

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 0: Repo Presentation of the Conary project.

SCOPE — review these files/directories:
- CLAUDE.md, .claude/rules/*
- README.md (if present), root Cargo.toml, conary-core/Cargo.toml, conary-server/Cargo.toml, conary-test/Cargo.toml
- MCP integration points (presentation review only — are they documented, do they look intentional): conary-core/src/mcp/, conary-server/src/server/mcp.rs, conary-test/src/server/mcp.rs
- CI workflows: .github/workflows/, .forgejo/workflows/
- Packaging: packaging/rpm/, packaging/deb/, packaging/arch/, packaging/ccs/
- Scripts: scripts/, deploy/
- .gitignore, project structure (ls src/, conary-core/src/, conary-server/src/, conary-test/src/)

SPECIAL LENS: This repo will be posted to r/claudecode as a showcase of AI-assisted development. Review through the eyes of someone browsing the repo for the first time:
- Does the CLAUDE.md look like real project instructions or AI boilerplate?
- Do file counts and version numbers in documentation match the actual codebase?
- Are CI workflows clean and well-organized?
- Do scripts have clear purposes and no dead scripts?
- Does the project structure make sense to a newcomer?
- Is there anything that screams "AI generated this and nobody checked"?

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-00-repo-presentation.md`.

---

### Task 2: Review Feature 1 — Package Management Core

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 1: Package Management Core of the Conary project.

SCOPE — review ALL .rs files in these directories:
- conary-core/src/db/ (SQLite schema, migrations v57, 69 tables, models — 44 files)
- conary-core/src/packages/ (RPM/DEB/Arch parsers, unified PackageMetadata — 13 files)
- conary-core/src/repository/ (remote repos, metadata sync, mirror health, GPG verification, Remi client, chunk fetcher, resolution policy — 27 files)
- conary-core/src/resolver/ (SAT-based dependency resolution via resolvo — 13 files)
- conary-core/src/dependencies/ (language-specific dependency support — 3 files)
- conary-core/src/version/ (version parsing, constraints — 1 file)
- conary-core/src/transaction/ (composefs transaction engine — 2 files)
- conary-core/src/compression/ (unified decompression with format detection — 1 file)
- conary-core/src/lib.rs (crate root, public API surface)

This is the foundation of the package manager — database state, package parsing, dependency resolution, and transaction execution. Pay extra attention to:
- SQL injection in db/ queries
- Error propagation through the resolution pipeline
- Parser correctness for all three package formats
- unwrap()/expect() in production paths (these caused P0s in a prior review)

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-01-package-management-core.md`.

---

### Task 3: Review Feature 2 — CCS Native Format

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 2: CCS Native Format of the Conary project.

SCOPE — review ALL .rs files in:
- conary-core/src/ccs/ (builder, CDC chunking, conversion from legacy formats, Ed25519 signing, policy engine, hooks, OCI export — 37 files)

This is Conary's native package format — the thing that distinguishes it from being a wrapper around apt/dnf/pacman. Pay extra attention to:
- Crypto correctness in Ed25519 signing/verification
- CDC chunking edge cases (empty files, huge files, binary vs text)
- Conversion fidelity from RPM/DEB/Arch to CCS
- Policy engine logic (are policies actually enforced?)
- Hook execution safety

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-02-ccs-native-format.md`.

---

### Task 4: Review Feature 3 — Filesystem & Generations

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 3: Filesystem & Generations of the Conary project.

SCOPE — review ALL .rs files in:
- conary-core/src/filesystem/ (CAS, VFS tree, fsverity — 6 files)
- conary-core/src/generation/ (EROFS image building, composefs mounting, /etc merge, GC — 9 files)
- conary-core/src/delta/ (binary delta updates with zstd dictionary compression — 4 files)

This is the immutable deployment layer — content-addressable storage backing composefs/EROFS generations. Pay extra attention to:
- Path traversal and symlink attacks in CAS operations
- Correctness of /etc merge logic (three-way merge is notoriously tricky)
- GC safety — can it delete objects still referenced by a generation?
- Delta application correctness — can a bad delta corrupt the CAS?
- fsverity usage — is it actually verified or just computed?

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-03-filesystem-generations.md`.

---

### Task 5: Review Feature 4 — Source Building

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 4: Source Building of the Conary project.

SCOPE — review ALL .rs files in:
- conary-core/src/recipe/ (recipe system with TOML specs, build cache, kitchen, PKGBUILD conversion — 13 files)
- conary-core/src/derivation/ (CAS-layered derivation engine, pipeline, provenance, trust levels — 19 files)
- conary-core/src/bootstrap/ (6-phase LFS-aligned bootstrap pipeline — 15 files)
- conary-core/src/derived/ (derived package builder with patches + overrides — 2 files)

This is the source building pipeline — from recipes to derivations to full bootstrap. Pay extra attention to:
- Build isolation — can a recipe escape the kitchen/sandbox?
- PKGBUILD conversion correctness (translating Arch syntax to Conary recipes)
- Derivation ID computation — is it truly deterministic/content-addressed?
- Bootstrap stage ordering — can stages run out of order?
- Path handling in chroot builds

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-04-source-building.md`.

---

### Task 6: Review Feature 5 — Supply Chain Security

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 5: Supply Chain Security of the Conary project.

SCOPE — review ALL .rs files in:
- conary-core/src/trust/ (TUF implementation, key management, metadata verification — 7 files)
- conary-core/src/provenance/ (package DNA tracking, SLSA, reproducibility — 7 files)
- conary-core/src/capability/ (capability declarations, audit mode — 14 files)

This is the security layer. Crypto bugs here are P0. Pay extra attention to:
- TUF metadata verification — does it follow the spec? Are there bypass paths?
- Key management — are private keys ever logged, serialized to disk unencrypted, or exposed in error messages?
- Signature verification — is it possible to skip verification?
- Capability enforcement — are capabilities actually checked or just declared?
- Provenance chain integrity — can it be forged?

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-05-supply-chain-security.md`.

---

### Task 7: Review Feature 6 — Cross-Distro & Extensibility

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 6: Cross-Distro & Extensibility of the Conary project.

SCOPE — review ALL .rs files in:
- conary-core/src/canonical/ (cross-distro name mapping, Repology/AppStream — 7 files)
- conary-core/src/model/ (System Model TOML, diff, replatform, lockfile, signing — 8 files)
- conary-core/src/automation/ (automated maintenance, AI assistance — 5 files)
- conary-core/src/components/ (component classification — 3 files)
- conary-core/src/flavor/ (build variation specs — 1 file)
- conary-core/src/label.rs (package provenance labels)
- conary-core/src/trigger/ (post-install trigger system — 1 file)
- conary-core/src/scriptlet/ (scriptlet execution, sandbox — 1 file)
- conary-core/src/container/ (namespace isolation for scriptlets — 1 file)
- conary-core/src/hash.rs (multi-algorithm hashing)
- conary-core/src/self_update.rs (self-update logic)
- conary-core/src/json.rs (canonical JSON serialization)
- conary-core/src/util.rs (utilities)
- conary-core/src/progress.rs (progress tracking)
- conary-core/src/error.rs (centralized error types)

This is a broad domain covering cross-distro support, system model, and many small utility modules. Pay extra attention to:
- Scriptlet/trigger sandbox escapes (container/ and scriptlet/ are security-critical)
- Self-update integrity — is the download verified before replacing the binary?
- Canonical JSON determinism — is it actually canonical or does it have edge cases?
- Hash module — are the right algorithms used in the right contexts?
- Error type consistency across this grab-bag of modules

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-06-cross-distro-extensibility.md`.

---

### Task 8: Review Feature 7 — CLI Layer

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 7: CLI Layer of the Conary project.

SCOPE — review ALL .rs files in:
- src/cli/ (all CLI definitions — clap structs, subcommand enums)
- src/commands/ (all command implementations)
- src/main.rs (entrypoint and dispatch)
- tests/*.rs, tests/common/ (root-crate integration tests)

This is the user-facing layer with 200+ commands. It is the first thing someone will read when they clone the repo. Pay extra attention to:
- Consistent command patterns — do similar commands handle args/errors/output the same way?
- Dead or stub commands — anything that's defined but not implemented?
- Help text quality — do commands have useful descriptions?
- Error messages — are they actionable or do they dump internal details?
- Integration test coverage — are the tests testing real behavior or just exercising code paths?
- AI slop — with 200+ commands, watch for copy-paste patterns or inconsistent styles between command groups

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-07-cli-layer.md`.

---

### Task 9: Review Feature 8 — Remi Server

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 8: Remi Server of the Conary project.

SCOPE — review ALL .rs files in:
- conary-server/src/server/**/*.rs (all server modules: conversion proxy, LRU cache, Bloom filter, config, routes, auth, admin service, MCP 24 tools, rate limiting, audit logging, forgejo CI client, analytics, canonical fetch/job, chunk GC, delta manifests, federated index, search, security, R2 storage, all HTTP handlers)
- conary-server/src/bin/remi.rs (Remi binary entrypoint)
- conary-server/src/lib.rs (crate root)

This is a production server handling real traffic behind Cloudflare. Pay extra attention to:
- Auth bypass paths — can any admin endpoint be reached without a valid bearer token?
- Rate limiting correctness — can it be trivially bypassed?
- Input validation on all HTTP endpoints (path params, query strings, request bodies)
- MCP tool security — can an LLM agent do anything destructive via MCP?
- Conversion correctness — does the RPM/DEB/Arch-to-CCS pipeline produce valid packages?
- Cache eviction logic — can the LRU cache grow unbounded?
- Error responses — do they leak internal paths or stack traces?

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-08-remi-server.md`.

---

### Task 10: Review Feature 9 — Daemon & Federation

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 9: Daemon & Federation of the Conary project.

SCOPE — review ALL .rs files in:
- conary-server/src/daemon/**/*.rs (REST API, SSE events, job queue, systemd, auth, lock)
- conary-server/src/bin/conaryd.rs (daemon binary entrypoint)
- conary-server/src/federation/**/*.rs (hierarchical P2P, rendezvous hashing, circuit breakers, mDNS)

The daemon runs as a privileged local service. The federation routes CAS chunks across peers. Pay extra attention to:
- Daemon auth — peer credentials check, privilege escalation paths
- SystemLock correctness — can two operations run concurrently and corrupt state?
- Job queue — can jobs be lost, duplicated, or stuck?
- Federation trust — can a malicious peer inject bad chunks?
- Circuit breaker logic — does it actually protect against cascading failures?
- mDNS — is local discovery safe from spoofing?
- Deadlocks in async code (SSE streams, job queue, federation requests)

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-09-daemon-federation.md`.

---

### Task 11: Review Feature 10 — Test Infrastructure

**Files:** Read-only review, no modifications.

- [ ] **Step 1: Dispatch lintian agent**

Dispatch a `lintian` agent with this prompt:

```
You are reviewing Feature 10: Test Infrastructure of the Conary project.

SCOPE — review ALL .rs files in:
- conary-test/src/**/*.rs (engine: runner, executor, coordinator, assertions, variables, QEMU; config: manifests, distros; container backend: Bollard, mock; server: HTTP API, MCP, WAL, Remi client; reporting; error taxonomy)
- conary-test/src/lib.rs (crate root)
Also review the test manifest structure:
- tests/integration/remi/config.toml
- A sample of TOML manifests in tests/integration/remi/manifests/ (read at least phase1-core.toml, phase2-group-a.toml, phase3-group-g.toml, phase4-group-a.toml to cover all phases)
- tests/integration/remi/containers/Containerfile.fedora43

This is the test infrastructure running 278 tests across 3 distros in Podman containers with an MCP server. Pay extra attention to:
- Container cleanup — can leaked containers accumulate and exhaust resources?
- WAL correctness — can results be lost or duplicated when Remi is unreachable?
- Assertion logic — are test assertions actually checking what they claim?
- Error taxonomy — are errors categorized correctly?
- TOML manifest parsing — edge cases in variable substitution, step execution order
- MCP tool safety — can a tool call leave containers running or data inconsistent?
- Mock backend fidelity — does the mock actually behave like real Podman?

[INSERT SHARED REVIEW METHODOLOGY HERE]

Write your findings report and output it. Do NOT modify any files.
```

- [ ] **Step 2: Save report**

Save the agent's output to `docs/superpowers/reviews/feature-10-test-infrastructure.md`.

---

## Chunk 2: Triage (Phase 2)

Depends on ALL tasks in Chunk 1 completing.

### Task 12: Consolidate findings

**Files:**
- Create: `docs/superpowers/reviews/consolidated-findings.md`
- Read: `docs/superpowers/reviews/feature-*.md` (all 11 reports)

- [ ] **Step 1: Read all 11 feature review reports**

Read every file in `docs/superpowers/reviews/feature-*.md`.

- [ ] **Step 2: Deduplicate cross-cutting issues**

Identify patterns that appear in multiple reports. For example:
- "unwrap() on fallible path" found in 4 domains = one cross-cutting finding
- "inconsistent error type" found in 3 domains = one cross-cutting finding
- Any "[Feature N]" cross-domain notes get routed to the correct domain

- [ ] **Step 3: Write consolidated findings document**

Write `docs/superpowers/reviews/consolidated-findings.md` with this structure:

```markdown
# Consolidated Code Review Findings

## Statistics
- Total findings: N
- By severity: P0=N, P1=N, P2=N, P3=N
- By category: Correctness=N, Quality=N, Idiomatic=N, Slop=N, Security=N, Architecture=N

## Cross-Cutting Patterns
[Issues that appear across multiple domains — fix once, apply everywhere]

## P0 — Critical
[All P0 findings from all domains, deduplicated]

## P1 — Important
[same]

## P2 — Medium
[same]

## P3 — Minor
[same]

## Per-Domain Summary
[One paragraph per domain: domain name, finding count, biggest concern]
```

- [ ] **Step 4: Verify no findings were lost**

Count findings across all 11 individual reports. Count findings in consolidated report. The consolidated count should be <= individual total (deduplication reduces count) but every unique finding must appear.

---

### Task 13: User checkpoint

- [ ] **Step 1: Present consolidated findings to user**

Show the user the statistics and cross-cutting patterns from the consolidated report. Ask them to review `docs/superpowers/reviews/consolidated-findings.md` before proceeding to remediation.

- [ ] **Step 2: Wait for approval**

Do not proceed to Chunk 3 until the user confirms.

---

## Chunk 3: Remediation (Phase 3)

Depends on Task 13 (user approval). The specific fix tasks are generated from the consolidated findings — they cannot be pre-written since they depend on what the reviews discover.

### Task 14: Generate remediation plan

**Files:**
- Read: `docs/superpowers/reviews/consolidated-findings.md`
- Create: remediation tasks (inline or as a follow-up plan)

- [ ] **Step 1: Group findings by file**

Map each finding to its file(s). Findings touching the same file must be fixed sequentially. Findings in different files can be parallelized.

- [ ] **Step 2: Order by dependency**

If fixing finding A requires finding B to be fixed first (e.g., restructuring a module before fixing a function in it), order accordingly.

- [ ] **Step 3: Generate fix tasks**

For each finding or group of related findings, create a task with:
- Exact file paths and line numbers
- What to change (concrete code, not "fix the issue")
- How to verify (compile, test, clippy)

- [ ] **Step 4: Dispatch emerge for parallel remediation**

Dispatch `emerge` with the fix list. Emerge parallelizes by file ownership — fixes in different files run concurrently, fixes in the same file run sequentially.

---

### Task 15: Final verification

Depends on Task 14 completing.

- [ ] **Step 1: Run full build**

Run: `cargo build`
Expected: Success, no warnings.

- [ ] **Step 2: Run full build with server features**

Run: `cargo build --features server`
Expected: Success, no warnings.

- [ ] **Step 3: Run all unit tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: Zero warnings.

- [ ] **Step 5: Run format check**

Run: `cargo fmt --check`
Expected: No formatting issues.

- [ ] **Step 6: Report results**

Present the final build/test/clippy/fmt results to the user. If any step fails, diagnose and fix before declaring done.
