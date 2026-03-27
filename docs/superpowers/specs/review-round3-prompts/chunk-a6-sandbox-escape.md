You are performing an adversarial security review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 adopts an attacker's perspective to find exploitation chains.

This is Chunk A6: Chroot/Sandbox Escape.

## Attacker Profile

You are a malicious package author who controls the content of recipe TOML files, PKGBUILD files, or scriptlet scripts that run during package installation. You can craft arbitrary content in these inputs. You do NOT have direct access to the host system -- your code only executes inside Conary's build sandbox or scriptlet execution environment.

## Attack Goal

Escape the build chroot or scriptlet sandbox to gain access to the host filesystem, host network, or host process namespace. Secondary goals: read sensitive files from the host, write to host paths outside the sandbox, establish persistent backdoors that survive sandbox teardown.

## Attack Vectors to Explore

1. **Namespace isolation gaps** -- Which Linux namespaces (mount, PID, network, user, UTS, IPC, cgroup) does the sandbox actually create? Are any missing? Can the attacker detect they are in a sandbox and exploit a missing namespace to reach the host?

2. **Mount leakage** -- Are `/proc`, `/sys`, `/dev`, or `/run` bind-mounted into the sandbox? If so, which subdirectories? Can the attacker read `/proc/1/root` to traverse to the host root? Can they use `/sys/fs/cgroup` to escape? Is `/dev/shm` shared with the host?

3. **Environment variable injection** -- Can recipe TOML or scriptlet content set `LD_PRELOAD`, `LD_LIBRARY_PATH`, `PATH`, `HOME`, `SHELL`, or `TMPDIR` that influence host-side processes after sandbox teardown? Are these variables scrubbed before sandbox entry and after sandbox exit?

4. **Shell metacharacter injection** -- Are recipe fields or scriptlet bodies passed through shell expansion? Can `$(command)`, backticks, `${var:-$(payload)}`, semicolons, pipes, or newlines in recipe fields break out of intended command boundaries?

5. **Resource exhaustion as escape aid** -- Can a fork bomb inside the sandbox affect the host (missing PID namespace or cgroup limits)? Can filling `/tmp` or the sandbox root inside the container cause the host's disk to fill (shared mount)?

6. **Network isolation** -- Does the sandbox have network access? If so, can the attacker exfiltrate data, download additional payloads, or connect back to an attacker-controlled server? Is there a firewall or network namespace?

7. **Seccomp profile** -- Is a seccomp-bpf filter applied to sandbox processes? Which syscalls are allowed? Can the attacker use `ptrace`, `clone` with dangerous flags, `mount`, `pivot_root`, `unshare`, or `nsenter` from inside the sandbox?

8. **Recipe interpolation injection** -- Recipe TOML uses variable interpolation (e.g., `%(version)s`, `${name}`). Can crafted variable values inject shell commands or path traversal sequences through the interpolation engine? What happens with nested interpolation or recursive expansion?

9. **Bootstrap chroot lifetime and cleanup** -- During bootstrap's multi-phase chroot building, how long do chroot environments persist? Are they cleaned up on failure? Can a process left running inside a chroot prevent cleanup and maintain access? Are temporary directories created with secure permissions?

10. **Scriptlet privilege boundaries** -- Scriptlets run during install/remove. What UID/GID do they execute under? If they run as root inside the sandbox, can they exploit shared resources (sockets, shared memory, signals) to affect host processes? Can a pre-remove scriptlet interfere with the removal of its own package to maintain persistence?

11. **Symlink and hardlink attacks** -- Can the attacker create symlinks inside the sandbox that point to host paths, which are then followed by post-sandbox cleanup code running on the host? Can hardlinks to sandbox-internal files create references that persist after sandbox teardown?

12. **Capability and ambient authority** -- What Linux capabilities does the sandbox process have? Are ambient capabilities cleared? Can the attacker use `CAP_SYS_ADMIN`, `CAP_NET_ADMIN`, or `CAP_DAC_OVERRIDE` to break out?

## Output Format

For each finding, report:

### [SEVERITY] FILE_A:LINE -> FILE_B:LINE -- Short title

**Boundary:** Which two modules/files this crosses
**Category:** NamespaceEscape | MountLeak | Injection | ResourceExhaust | NetworkEscape | PrivilegeEscalation
**Exploitation chain:** Step-by-step attack description showing how the attacker moves from controlled input to sandbox escape.
**Description:** What is wrong and why it matters.
**Suggested fix:** Concrete change at one or both sides of the boundary.

Severity levels:
- CRITICAL: Full sandbox escape, host filesystem write, or code execution outside sandbox
- HIGH: Partial escape (read-only host access, network access, information leak of host state)
- MEDIUM: Weakened isolation that enables escape in combination with another bug
- LOW: Defense-in-depth gap unlikely to be directly exploitable

## Scope

You are NOT limited to specific files. Follow the attack wherever it leads across the entire codebase. Key starting points include `conary-core/src/container/`, `conary-core/src/scriptlet/`, `conary-core/src/recipe/`, `conary-core/src/bootstrap/`, and `conary-core/src/derivation/`, but trace into any file that participates in sandbox setup, execution, or teardown.

## Summary

- Critical: N
- High: N
- Medium: N
- Low: N

Top 3 sandbox escape risks:
1. ...
2. ...
3. ...
