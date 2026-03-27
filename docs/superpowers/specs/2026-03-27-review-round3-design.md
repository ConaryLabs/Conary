# Deep Code Review Round 3 -- Adversarial + Invariant Verification

## Goal

Third and final review pass of the Conary codebase (~218K lines Rust, 501 files, 4 crates). Round 1 reviewed modules in isolation. Round 2 traced end-to-end data flows. Round 3 adopts attacker and contract-auditor perspectives to find exploitation chains and invariant violations that neither prior approach could surface.

## Approach: Hybrid adversarial + invariant (Approach C)

14 chunks: 10 adversarial red-team scenarios + 4 invariant contract verifications.

### Key differences from rounds 1 and 2

| | Round 1 | Round 2 | Round 3 |
|---|---------|---------|---------|
| Lens | "Is this module correct?" | "Does data survive this crossing?" | "How do I break this system?" |
| Unit | Directory | End-to-end path | Attack goal / invariant contract |
| Scope per chunk | Files in one directory | Files along one data flow | Any file relevant to the goal |
| Reviewer role | Code reviewer | Data-flow tracer | Red team / contract auditor |
| Finds | Per-module bugs | Cross-boundary wiring | Exploitation chains, invariant gaps |

### Execution model

- Claude/Opus lintian subagents dispatched in parallel (read-only)
- Findings collected, triaged, fixed in priority order
- Codex post-review on squashed commit
- Optional Gemini/Minimax independent passes

## Chunk Definitions

### Adversarial Chunks (A1-A10)

| # | Name | Attacker | Goal |
|---|------|----------|------|
| A1 | Malicious Package Install | Crafted CCS/RPM/DEB/Arch file | Code execution, path traversal, privilege escalation |
| A2 | Repository Metadata Poisoning | Controls a mirror/MITM | Serve malicious package as update, downgrade, inject phantom dep |
| A3 | Self-Update MITM | MITM on update channel | Replace conary binary with malicious one |
| A4 | Server Exploitation | Remote HTTP attacker | RCE, data exfil, auth bypass, DoS on Remi |
| A5 | Federation Peer Compromise | Controls a peer with valid cert | Serve malicious chunks, poison routing, impersonate peers |
| A6 | Chroot/Sandbox Escape | Controls recipe or scriptlet content | Escape build sandbox to host filesystem/network |
| A7 | Concurrent State Corruption | Two concurrent ops or crash timing | Corrupt DB, CAS, or generation state |
| A8 | Rollback/Generation Persistence | Installed malware, user rolls back | Persist across rollback or GC |
| A9 | Daemon/Socket Exploitation | Local unprivileged user | Execute privileged ops, escalate to root via conaryd |
| A10 | Denial of Service | Any input interface | Exhaust memory, disk, CPU, file descriptors |

### Invariant Chunks (I1-I4)

| # | Name | Contract |
|---|------|----------|
| I1 | CAS Integrity | Every CAS object is content-addressed, verified, never silently lost |
| I2 | Transaction Atomicity | Every DB transaction fully commits or fully rolls back |
| I3 | Signature Trust Chain | Every crypto verification is strict, trusted, non-bypassable |
| I4 | Test Coverage Gaps | Every public function tested; identify dead code and untested paths |

## Prompt Template

Each adversarial prompt follows:
1. **Attacker profile** -- who you are, what access you have
2. **Attack goal** -- what you're trying to achieve
3. **What to probe** -- specific attack vectors to explore
4. **Scope** -- not limited to specific files; follow wherever the attack leads
5. **Output format** -- findings with exploitation chain, severity, and suggested fix

Each invariant prompt follows:
1. **Contract statement** -- the invariant that must hold
2. **What to verify** -- specific checks to perform exhaustively
3. **Scope** -- every file where the contract is relevant
4. **Output format** -- violations with location, impact, and suggested fix

## Workflow

1. Write 14 prompt files to `docs/superpowers/specs/review-round3-prompts/`
2. Dispatch all 14 as parallel lintian subagents
3. Collect findings into unified report
4. Triage and fix (CRITICAL -> HIGH -> MEDIUM -> LOW)
5. Codex post-review on squashed commit
6. Optional Gemini/Minimax independent passes

## Chunk Tracker

| # | Chunk | Status |
|---|-------|--------|
| A1 | Malicious Package Install | Pending |
| A2 | Repository Metadata Poisoning | Pending |
| A3 | Self-Update MITM | Pending |
| A4 | Server Exploitation | Pending |
| A5 | Federation Peer Compromise | Pending |
| A6 | Chroot/Sandbox Escape | Pending |
| A7 | Concurrent State Corruption | Pending |
| A8 | Rollback/Generation Persistence | Pending |
| A9 | Daemon/Socket Exploitation | Pending |
| A10 | Denial of Service | Pending |
| I1 | CAS Integrity | Pending |
| I2 | Transaction Atomicity | Pending |
| I3 | Signature Trust Chain | Pending |
| I4 | Test Coverage Gaps | Pending |
