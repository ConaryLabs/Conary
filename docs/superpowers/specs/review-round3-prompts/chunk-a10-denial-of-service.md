You are performing an adversarial security review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 adopts an attacker's perspective to find exploitation chains.

This is Chunk A10: Denial of Service.

## Attacker Profile

You are anyone who can trigger a conary operation. This includes: a remote attacker who controls a repository mirror, a local user who can run `conary` CLI commands, a malicious package author whose package is being installed, or a network attacker who can serve crafted HTTP responses. You do NOT need code execution -- you only need to provide input that conary processes.

## Attack Goal

Exhaust system resources (memory, disk, CPU, file descriptors, or database locks) to the point where the system becomes unresponsive, conary becomes unusable, or other system services are disrupted. Secondary goals: trigger OOM kills of unrelated processes, fill the root filesystem causing system instability, or lock the conary database permanently.

## Attack Vectors to Explore

1. **Unbounded collections from parser output** -- When parsing RPM, DEB, Arch, or CCS packages, are the resulting collections (file lists, dependency lists, provide lists, changelog entries) bounded? Can a crafted package with millions of file entries cause OOM? Is there a limit on the number of dependencies a single package can declare?

2. **Unbounded archive entries** -- When extracting archives (cpio from RPM, tar from DEB, tar from Arch), is the number of entries bounded? Can a crafted archive contain millions of zero-byte files that exhaust inodes or memory? Are archive entry names validated for length?

3. **Unbounded dependency tree expansion** -- During SAT dependency resolution, can a crafted dependency graph cause exponential expansion? Is there a depth or breadth limit on transitive dependency resolution? Can circular dependencies cause infinite loops? What is the maximum resolution time before timeout?

4. **Large file handling** -- Can a multi-gigabyte package be served that exhausts memory if loaded entirely into memory? Are package downloads streamed to disk or buffered in memory? Is there a maximum package size? What about delta packages -- can a small delta expand to a huge result?

5. **Regex DoS (ReDoS)** -- Are there user-controlled inputs (package names, version strings, search queries, glob patterns) that are matched against regular expressions? Are those regexes vulnerable to catastrophic backtracking with crafted input? Check all uses of `Regex::new()` with dynamic patterns.

6. **Infinite loops in resolution and retry logic** -- Can dependency resolution enter an infinite loop with crafted dependency graphs? Is there a maximum iteration count? Do HTTP retry loops have a maximum retry count and backoff? Can metalink/mirror failover loop indefinitely?

7. **Disk fill via uploads and temp files** -- Can the attacker fill disk by:
   - Repeatedly triggering package downloads that are cached but never cleaned
   - Causing temp files to be created but not deleted (crash between create and cleanup)
   - Uploading large packages to the server (if the server is in scope)
   - Causing CAS deduplication to fail, storing duplicate content
   - Triggering EROFS image builds for many generations without GC

8. **Disk fill via CAS store** -- Can an attacker craft packages where every file has unique content (defeating deduplication), causing the CAS store to grow without bound? Is there a CAS size limit or quota?

9. **Connection and file descriptor exhaustion** -- Can the attacker:
   - Open many simultaneous connections to the Remi server
   - Trigger many concurrent downloads that each hold a file descriptor
   - Cause the client to open many database connections
   - Fill the process file descriptor table via leaked handles

10. **Zip bomb / compression bomb** -- Can a crafted package contain compressed data that expands to an enormous size? Are there limits on decompressed size? For each compression format (gzip, xz, zstd):
    - Is the output size checked during streaming decompression?
    - Is there a maximum decompressed-to-compressed ratio?
    - Can nested compression (compressed archive containing compressed files) multiply the effect?

11. **SQLite lock starvation** -- Can a long-running read transaction (e.g., `conary list` on a huge database) prevent write transactions from completing? Can the attacker hold the database locked by:
    - Running a query that never completes
    - Opening a connection and holding a transaction without committing
    - Triggering a WAL checkpoint that blocks writes

12. **Repository metadata bombs** -- Can a malicious repository serve metadata (package index, dependency index, metalink XML) that is:
    - Extremely large (gigabytes of package metadata)
    - Deeply nested (XML/JSON with extreme nesting depth causing stack overflow)
    - Self-referential (causing infinite parsing loops)
    - Contains millions of entries that exhaust memory during sync

13. **CPU exhaustion via cryptographic operations** -- Can the attacker force expensive operations by:
    - Providing many packages that each require signature verification
    - Serving packages with invalid signatures that are checked before rejection
    - Triggering fsverity computation on many large files
    - Causing repeated hash computations on large CAS objects

14. **String/path length attacks** -- Are there limits on:
    - Package name length
    - File path length within packages
    - Version string length
    - Description/metadata field lengths
    - URL lengths for repositories

## Output Format

For each finding, report:

### [SEVERITY] FILE_A:LINE -> FILE_B:LINE -- Short title

**Boundary:** Which two modules/files this crosses
**Category:** MemoryExhaust | DiskExhaust | CPUExhaust | FDExhaust | LockStarvation | CompressionBomb | InfiniteLoop
**Exploitation chain:** Step-by-step scenario showing how the attacker provides input that leads to resource exhaustion.
**Description:** What is wrong and why it matters.
**Suggested fix:** Concrete change at one or both sides of the boundary.

Severity levels:
- CRITICAL: System-wide DoS (OOM kills other services, root filesystem full, system unresponsive)
- HIGH: Conary permanently unusable until manual intervention (locked database, corrupted state)
- MEDIUM: Temporary conary unavailability or significant performance degradation
- LOW: Minor resource waste or theoretical DoS requiring sustained attacker effort

## Scope

You are NOT limited to specific files. Follow the attack wherever it leads across the entire codebase. Key starting points include `conary-core/src/packages/` (parsers), `conary-core/src/compression/`, `conary-core/src/resolver/` (SAT solver), `conary-core/src/repository/` (download, sync, metadata), `conary-core/src/filesystem/cas.rs`, `conary-core/src/db/`, `conary-core/src/generation/`, `conary-core/src/delta/`, and `conary-server/src/server/`, but trace into any file that processes external input or manages system resources.

## Summary

- Critical: N
- High: N
- Medium: N
- Low: N

Top 3 denial-of-service risks:
1. ...
2. ...
3. ...
