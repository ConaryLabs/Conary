You are performing an adversarial security review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 adopts an attacker's perspective to find exploitation chains.

This is Chunk A9: Daemon/Socket Exploitation.

## Attacker Profile

You are an unprivileged local user on a system running `conaryd` (the conary daemon). The daemon runs as root and exposes a REST API over a Unix domain socket (and possibly a TCP port). You can connect to the socket, send arbitrary HTTP requests, and observe SSE event streams. You have normal user-level access to the filesystem but no sudo or polkit privileges.

## Attack Goal

Execute privileged operations through the daemon (install/remove packages, modify generations, write to system paths) without proper authorization. Ultimate goal: escalate to root or achieve equivalent system modification capabilities. Secondary goals: leak sensitive information, deny service to legitimate users, or inject commands into the daemon's job queue.

## Attack Vectors to Explore

1. **SO_PEERCRED validation** -- Does the daemon use `SO_PEERCRED` or equivalent to verify the identity of the connecting process? If so, is the check applied to every endpoint or only some? Can the attacker connect from a different user namespace where their UID appears to be 0? Is the credential check performed at connection time and cached, or re-verified per request?

2. **Job queue access control** -- The daemon has a job queue for operations like install, remove, and update. Can any local user submit jobs? Is there a permission model (e.g., polkit, group membership, or token-based auth) that gates access? Can the attacker submit a job that runs a scriptlet containing arbitrary commands as root?

3. **SSE event data leakage** -- The daemon streams Server-Sent Events for operation progress. Do these events contain sensitive information (package names being installed, file paths, error messages with internal details, database queries)? Can an unprivileged user subscribe to all events, including those from other users' operations?

4. **REST API auth gating** -- Enumerate every REST API endpoint exposed by the daemon. For each: Is authentication required? Is authorization checked (is the authenticated user allowed THIS operation)? Are there endpoints that should require auth but do not? Are there debug or admin endpoints accidentally exposed?

5. **SystemLock bypass** -- The system lock prevents concurrent operations. Can an unprivileged user acquire the lock (denial of service to root operations)? Can they hold the lock indefinitely? Is the lock released if the holder disconnects or crashes? Can the attacker bypass the lock by connecting directly to the database file?

6. **RFC 7807 error response information leakage** -- Error responses should follow RFC 7807 (Problem Details). Do they leak internal information such as:
   - Full SQL queries or database paths
   - Filesystem paths revealing system layout
   - Stack traces or panic messages
   - Internal IP addresses or hostnames
   - Version information enabling targeted exploits

7. **Request smuggling and injection** -- Can the attacker craft HTTP requests that:
   - Inject headers that bypass auth checks
   - Send oversized headers or bodies that cause buffer issues
   - Use HTTP/1.1 pipelining to interleave requests
   - Exploit chunked transfer encoding edge cases

8. **Path traversal through API parameters** -- Do any API endpoints accept path parameters (package names, generation IDs, file paths) that could be manipulated with `../` sequences, null bytes, or symlink references to access or modify resources outside their intended scope?

9. **Privilege escalation via scriptlet execution** -- If the attacker can trigger package installation through the daemon, the scriptlets run as root. Even if the daemon requires auth for install, can the attacker:
   - Modify a package in a local repository to include malicious scriptlets
   - Trigger an auto-update that pulls a package with malicious scriptlets
   - Exploit a TOCTOU gap between auth check and scriptlet execution

10. **Signal and file descriptor leaks** -- Does the daemon properly close file descriptors when forking child processes (scriptlets)? Are there file descriptors to the database, socket, or sensitive files that a scriptlet process inherits and could exploit?

11. **Rate limiting and DoS** -- Is there rate limiting on the socket? Can the attacker:
    - Open thousands of connections to exhaust file descriptors
    - Send slow requests (slowloris) to tie up worker threads
    - Submit many large jobs to fill the job queue
    - Trigger expensive database queries via the API

12. **Daemon restart exploitation** -- When the daemon restarts (e.g., after a crash or upgrade), does it:
    - Re-read and validate its configuration safely
    - Verify the integrity of in-progress operations
    - Clear stale locks and temporary state
    - Resume with the same privilege level (no accidental setuid behavior)

## Output Format

For each finding, report:

### [SEVERITY] FILE_A:LINE -> FILE_B:LINE -- Short title

**Boundary:** Which two modules/files this crosses
**Category:** AuthBypass | PrivilegeEscalation | InfoLeak | DoS | Injection | LockManipulation
**Exploitation chain:** Step-by-step attack description showing how the unprivileged user escalates or exfiltrates.
**Description:** What is wrong and why it matters.
**Suggested fix:** Concrete change at one or both sides of the boundary.

Severity levels:
- CRITICAL: Unauthenticated privilege escalation to root or arbitrary system modification
- HIGH: Authenticated-only bypass, significant information leak, or persistent DoS
- MEDIUM: Minor information leak, temporary DoS, or requires unlikely preconditions
- LOW: Defense-in-depth gap or information disclosure of low-value data

## Scope

You are NOT limited to specific files. Follow the attack wherever it leads across the entire codebase. Key starting points include `conary-server/src/daemon/` (REST API, job queue, SSE, systemd integration), `conary-server/src/server/auth.rs`, `conary-server/src/server/rate_limit.rs`, `conary-server/src/server/routes.rs`, `conary-server/src/server/handlers/`, and `conary-core/src/scriptlet/`, but trace into any file that participates in daemon operation, authentication, or privilege boundaries.

## Summary

- Critical: N
- High: N
- Medium: N
- Low: N

Top 3 daemon exploitation risks:
1. ...
2. ...
3. ...
