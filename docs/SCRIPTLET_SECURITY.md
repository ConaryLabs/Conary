# Scriptlet Security Model

This document describes Conary's security model for executing package scriptlets (install/remove hooks).

## Threat Model

Package scriptlets execute arbitrary code with the privileges of the package manager (typically root). Threats include:

1. **Malicious packages**: Intentionally harmful scripts from compromised or malicious repositories
2. **Supply chain attacks**: Legitimate packages with injected malicious code
3. **Buggy scripts**: Well-intentioned scripts that cause unintended damage
4. **Resource exhaustion**: Scripts that hang, loop infinitely, or consume excessive resources

## Defense Layers

Conary implements multiple defense layers:

### 1. Script Risk Analysis

Before execution, scripts are analyzed for dangerous patterns:

| Risk Level | Patterns | Action |
|------------|----------|--------|
| Critical | `curl\|sh`, `wget\|sh`, `eval $` | Remote code execution |
| High | `rm -rf /`, `mkfs`, `dd if=* of=/dev/`, fork bombs | System destruction |
| Medium | `chmod u+s`, `crontab`, `/etc/shadow`, `/etc/sudoers` | Privilege escalation |
| Low | `nc`, `/dev/tcp/`, `base64 -d` | Network backdoors, obfuscation |

Risk analysis is performed by `analyze_script()` in `src/container/mod.rs`.

### 2. Sandbox Modes

Scriptlet execution supports three sandbox modes:

```rust
pub enum SandboxMode {
    None,    // Direct execution (default, for compatibility)
    Auto,    // Sandbox if risk >= Medium
    Always,  // Always sandbox all scripts
}
```

Configure via CLI or environment:
- `--sandbox=auto` - Recommended for untrusted packages
- `--sandbox=always` - Maximum security
- `--sandbox=never` - Legacy behavior (default)

### 3. Container Isolation

When sandboxing is enabled, scripts run in a lightweight Linux container with:

#### Namespace Isolation
- **PID namespace**: Isolated process tree, script cannot see/signal host processes
- **UTS namespace**: Isolated hostname (`conary-sandbox`)
- **IPC namespace**: Isolated System V IPC and POSIX message queues
- **Mount namespace**: Isolated filesystem view

#### Filesystem Isolation
- **chroot**: Script sees only the container root
- **Bind mounts**: Controlled access to host paths:
  - Read-only: `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`, `/etc/passwd`, `/etc/group`
  - Writable (for scriptlets): `/var`, `/etc` (package config)

#### Resource Limits (setrlimit)
| Resource | Default Limit | Purpose |
|----------|--------------|---------|
| Memory (RLIMIT_AS) | 512 MB | Prevent memory exhaustion |
| CPU time (RLIMIT_CPU) | 60 seconds | Prevent CPU exhaustion |
| File size (RLIMIT_FSIZE) | 100 MB | Prevent disk filling |
| Processes (RLIMIT_NPROC) | 64 | Prevent fork bombs |

#### Timeout Protection
- Wall-clock timeout: 60 seconds (configurable)
- Scripts exceeding timeout are killed with SIGKILL

### 4. Basic Protections (Always Active)

Even without sandboxing, these protections are always enforced:

#### stdin Nullification
```rust
cmd.stdin(Stdio::null())  // CRITICAL: Prevent stdin hangs
```
Scripts cannot read from stdin, preventing interactive prompts that would hang the package manager.

#### Non-Root Install Safety
```rust
if self.root != Path::new("/") {
    warn!("Skipping scriptlet: execution in non-root paths not supported");
    return Ok(());
}
```
Scriptlets are skipped when installing to non-root destinations (e.g., `--root=/mnt/target`), as they would incorrectly affect the host system.

#### Interpreter Validation
```rust
if !Path::new(&interpreter_path).exists() {
    return Err(Error::ScriptletError(format!(
        "Interpreter not found: {}",
        interpreter_path
    )));
}
```
No fallback interpreters - if the specified interpreter doesn't exist, the scriptlet fails rather than using a potentially incompatible alternative.

## Cross-Distro Argument Handling

Conary supports packages from multiple distributions, each with different scriptlet conventions:

### RPM (Red Hat, Fedora, SUSE)
Arguments are integer counts of package versions remaining:
- `$1 = 1`: Fresh install
- `$1 = 2`: Upgrade (new package scripts)
- `$1 = 1`: Upgrade removal (old package scripts) - NOT 0!
- `$1 = 0`: Complete removal

### DEB (Debian, Ubuntu)
Arguments are action words per Debian Policy:
- preinst: `install` or `upgrade <old-version>`
- postinst: `configure [<old-version>]`
- prerm: `remove` or `upgrade <new-version>`
- postrm: `remove` or `upgrade <new-version>`

### Arch Linux
Arguments are version strings:
- Install: `$1 = <new-version>`
- Remove: `$1 = <old-version>`
- Upgrade: `$1 = <new-version>`, `$2 = <old-version>`

Arch `.INSTALL` files define functions rather than executable scripts, so Conary generates wrapper scripts that source the file and call the appropriate function.

## Rootless Execution Fallback

Full namespace isolation requires root privileges. When running as non-root:

1. Check for unprivileged user namespaces (`/proc/sys/kernel/unprivileged_userns_clone`)
2. If unavailable, fall back to resource limits only (no namespace isolation)
3. Warning is logged: "Namespace isolation requires root privileges, falling back to resource limits only"

## Security Recommendations

### For Package Maintainers
1. Keep scriptlets minimal - prefer triggers for common operations
2. Avoid network access in scriptlets
3. Use absolute paths for all commands
4. Handle errors gracefully (set -e)

### For System Administrators
1. Use `--sandbox=auto` for packages from untrusted sources
2. Review scriptlets before installing unknown packages: `conary query --scripts <package>`
3. Monitor `/var/log/conary.log` for scriptlet warnings
4. Consider `--sandbox=always` for high-security environments

### For Repository Operators
1. Scan packages for dangerous patterns before publishing
2. Sign packages with GPG
3. Use separate repositories for trusted vs. community packages

## Implementation Files

| File | Purpose |
|------|---------|
| `src/scriptlet/mod.rs` | Scriptlet executor, cross-distro handling |
| `src/container/mod.rs` | Container isolation, risk analysis |
| `src/trigger/mod.rs` | Post-install triggers (preferred over scriptlets) |
| `src/db/models/scriptlet.rs` | Scriptlet database storage |

## Future Enhancements

- seccomp-bpf syscall filtering
- Capability dropping (CAP_NET_RAW, etc.)
- Network namespace isolation (block all network access)
- cgroups v2 for finer resource control
- Script signing and verification
