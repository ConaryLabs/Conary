---
last_updated: 2026-07-08
revision: 11
summary: Document file-capability public-policy review boundary
---

# Scriptlet Security Model

This document describes Conary's security model for executing package scriptlets (install/remove hooks).

Code owners for this model live under `crates/conary-core/src/scriptlet/`:
`sandbox.rs` owns sandbox mode and protected live-root policy, `process.rs`
owns direct/target-root/chroot execution, `legacy.rs` owns legacy replay
invocation contracts, `arguments.rs` owns distro argument mapping, and
`runtime.rs` owns subprocess/seccomp helper plumbing.

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

Risk analysis is performed by `analyze_script()` in
`crates/conary-core/src/container/mod.rs`.

### 2. Sandbox Modes

Scriptlet execution supports three sandbox modes:

```rust
pub enum SandboxMode {
    None,    // Direct execution, no sandboxing
    Auto,    // Sandbox based on script risk analysis
    #[default]
    Always,  // Always sandbox all scripts
}
```

Configure via CLI or environment:
- `--sandbox=always` - Protected mode, sandbox all scripts (default)
- `--sandbox=auto` - Risk-based sandboxing; scripts at medium risk or higher
  use the same protected mode as `always`
- `--sandbox=never` - Legacy direct execution; scriptlets can mutate the live
  host with the package manager's privileges

### 3. Container Isolation

Protected mode, target-root execution, and direct legacy execution are distinct
boundaries. Changeset metadata records both the requested sandbox mode
(`always`, `auto`, or `never`) and the effective sandbox (`protected-live-root`,
`target-root`, or `direct`), so `--sandbox=auto` direct execution cannot be
confused with protected sandboxing.

### Protected Live-Root Execution

When protected sandboxing is enabled for the live root (`/`), scripts run in a
lightweight Linux container with:

#### Namespace Isolation
- **PID namespace**: Isolated process tree, script cannot see/signal host processes
- **UTS namespace**: Isolated hostname (`conary-sandbox`)
- **IPC namespace**: Isolated System V IPC and POSIX message queues
- **Mount namespace**: Isolated filesystem view
- **Network namespace**: Hermetic execution blocks outbound network access
- **User namespace**: Privilege isolation is used when the host/kernel supports it

#### Filesystem Isolation
- **Root transition**: Root/no-userns execution requires `pivot_root`; `chroot`
  fallback is fatal in enforce mode. When an unprivileged user namespace is
  available, setup may enter the prepared root with `chroot` only after sandbox
  root maps to a non-host UID/GID.
- **composefs mount**: On composefs-native systems, `/usr` is a read-only EROFS
  mount, providing an additional layer of protection -- scriptlets cannot modify
  system binaries even if they escape the sandbox
- **Bind mounts**: Controlled access to host paths:
  - Read-only host tooling: `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`
  - Read-only host identity files layered into the private `/etc`:
    `/etc/passwd`, `/etc/group`, `/etc/hosts`, `/etc/shadow`, `/etc/sudoers`
  - Private writable live-root layers: `/etc` and `/var` are backed by owned
    temporary directories, so protected scriptlet writes are discarded with the
    sandbox instead of mutating the host
- **Seccomp profile**: Protected live-root scriptlets install the `scriptlet`
  seccomp profile in enforce mode. That profile excludes `chroot`, `mount`,
  `umount2`, `pivot_root`, kernel module loading, reboot, BPF, and other
  privileged escape primitives from the scriptlet process.

Protected live-root scriptlets are preflighted before package file/DB mutation.
If namespace, private writable layer, or enforcement setup is unavailable, the
operation aborts before mutation with an operator-facing diagnostic.

### Target-Root Execution

Target-root installs (`--root=/path`) use the alternate-root execution path for
building or modifying another filesystem. That path can use chroot-style
execution for the target root and is separate from the protected live-root
sandbox boundary.

### Direct Legacy Execution

`--sandbox=never` runs scriptlets directly on the live host after stdin
nullification and environment filtering. This is an explicit legacy escape
hatch, not a filesystem sandbox. `--sandbox=auto` may also choose direct
execution for low-risk live-root scripts; those runs record
`effective_sandbox=direct`.

#### Resource Limits (setrlimit)
| Resource | Default Limit | Purpose |
|----------|--------------|---------|
| Memory (RLIMIT_AS) | 512 MB | Prevent memory exhaustion |
| CPU time (RLIMIT_CPU) | 60 seconds | Prevent CPU exhaustion |
| File size (RLIMIT_FSIZE) | 100 MB | Prevent disk filling |
| Processes (RLIMIT_NPROC) | 1024 | Prevent fork bombs |

#### Timeout Protection
- Wall-clock timeout: 60 seconds (configurable)
- Scripts exceeding timeout are killed with SIGKILL

### 4. Scriptlet Capture Mode

When adopting legacy packages (RPM/DEB), Conary can **capture** the intent of imperative scriptlets instead of running them on the user's system.

This mode runs the scriptlet in a strict, ephemeral sandbox with **mocked system tools** (`useradd`, `systemctl`, etc.).

#### How Capture Works
1.  **Mock Environment:** A temporary root is created with fake binaries that log their arguments instead of modifying the system.
2.  **Execution:** The script runs in a network-isolated sandbox.
3.  **Diff:** Files created by the script (e.g., config generation) are captured and added to the package payload.
4.  **Intent Parsing:** Calls to mock tools are parsed and converted to declarative CCS Hooks (e.g., `useradd nginx` -> `[[hooks.users]] name="nginx"`).
5.  **Discard:** The original imperative script is discarded.

This transforms unsafe runtime scripts into safe, atomic build-time declarations.

### Boot And Security Scriptlet Evidence

Kernel-module, initramfs, bootloader, and unsupported SELinux scriptlet effects
are boot/security critical. Conary classifies these commands and preserves
sanitized command evidence in legacy scriptlet summaries. Public Remi serving
remains blocked for kernel, initramfs, bootloader, and unsupported SELinux
classes until native adapter and target-profile evidence proves complete
replacement. Raw replay, `--no-scripts`, or malformed summary metadata must not
make these packages public-ready.

Supported SELinux forms are different: `selinux-policy/v1` records
payload-scoped label refresh, payload-backed file-context rules, persistent boolean
declarations, and payload-backed policy-module installs as optional policy
intent. That evidence is safe on Arch or Debian because it is dormant when
SELinux is absent and applies only through native policy handling when SELinux
is present. Conversion never runs `restorecon`, `semanage`, `setsebool`, or
`semodule` against the host policy store.

Supported AppArmor forms are narrow. `apparmor-policy/v1` records
payload-backed `apparmor_parser -r|--replace /etc/apparmor.d/<profile>` as
optional policy intent. That evidence is dormant when AppArmor is absent and is
eligible for native policy handling only when AppArmor is present. Conversion
never runs `apparmor_parser` against the host policy store. Mode changes such
as `aa-enforce` and `aa-complain`, profile disable/status helpers, broad
directory reloads, and non-payload profile paths remain blocked/private and use
`block-on-enforcing-target` fallback when captured as review intent.

Sysctl handling has a narrow native-authority bridge. Conversion may replace a
simple `sysctl -w <key>=<value>` scriptlet with a validated native
`hooks.sysctl` declaration when the key and value pass the same CCS sysctl
validators used at install time, and one validated write still counts as
complete native replacement evidence for `sysctl/v1`. Public-ready serving is
narrower: the converted package is public only when the target profile accepts
that exact sysctl key. Missing target-profile context or keys the profile does
not allow stay `private-review`, even when the native hook projection is
otherwise complete. Today the public catalog proof key is `kernel.example`;
`net.ipv4.ip_forward` remains valid private-review evidence unless a future
target profile explicitly allows it. Broad loads such as `sysctl -p`, multiple
assignment batches, and denied security-sensitive keys remain blocked and can
only be fetched through the non-public admin/test lane.

Setuid handling has a similarly narrow file-mode bridge. Conversion may replace
`chmod u+s <payload-executable>` or `chmod 4xxx <payload-executable>` only when
the target is an executable shipped by the package payload. The converter moves
that authority into the CCS file mode and adds the exact path to
`policy.allow_setuid_paths`, so the normal build policy can preserve that one
setuid bit while still stripping unapproved setuid and all setgid bits.

File-capability handling is separate manifest authority, not package dependency
capabilities. `file-capability/v1` still recognizes known Linux
`setcap cap_*=+ep <payload-executable>` grants as complete replacement
evidence, but only `cap_net_bind_service` is public-ready by default. High-risk
known capabilities such as `cap_sys_admin`, `cap_sys_module`, `cap_sys_rawio`,
`cap_sys_boot`, `cap_sys_ptrace`, `cap_bpf`, `cap_net_admin`, `cap_setpcap`,
and `cap_setfcap` remain private-review until a future target-profile policy
explicitly allows them. The converter records recognized authority in
`[[file_capabilities]]`, and mutable live-root CCS installs apply it through a
controlled `setcap` invocation after file deployment and before DB commit.
Generation-aware CCS installs now support the same manifest authority only when
the install persists file-capability rows, publishes a generation immediately
instead of using `--defer-generation`, attaches `security.capability` during
generation runtime-input collection, and writes the generation image through an
xattr-preserving format. `conary system generation info` reports the resulting
capability-xattr count for published generations. Group O includes a QEMU
export proof for both capability-present and capability-absent exported
artifacts as rollback-equivalent evidence; the fresh local run is currently
blocked before that case by the `minimal-boot-v3` source image SSH readiness
failure.
Capability removal, inheritable/process/ambient capability forms, `setpriv`,
setgid modes, broad `chmod +s`, unknown capability names, and non-payload
targets remain blocked/private.

This evidence is harvested from package-authored metadata and static command
signals during conversion. It is an advisory trace for refusal diagnostics, not
runtime execution and not permission to mutate the host boot or security state.
Environment values are not surfaced in public boot/security evidence.

Maintainers may use Remi's non-public admin/test serving lane to fetch blocked,
private-review, or local-only converted CCS files for inspection. That lane is
disabled by default, requires admin access, and preserves the original
scriptlet publication status. It is not public publication authority and does
not permit raw legacy scriptlet replay.

### 5. Basic Protections (Always Active)

Even without sandboxing, these protections are always enforced:

#### stdin Nullification
```rust
cmd.stdin(Stdio::null())  // CRITICAL: Prevent stdin hangs
```
Scripts cannot read from stdin, preventing interactive prompts that would hang the package manager.

#### Environment Filtering
Direct execution paths clear the inherited environment and repopulate only the minimal variables Conary needs (`PATH`, `HOME`, `LANG`, scriptlet context variables). This reduces ambient secret leakage and makes direct execution closer to the sandboxed environment.

#### Target-Root Execution
```rust
if self.is_live_root() {
    self.execute_sandbox_live(...)
} else {
    self.execute_in_target(...)
}
```
Installing into an alternate root (for example `--root=/mnt/target`) uses the
target-root execution path instead of mutating the host `/`. That path chroots
into the target root and requires the privileges needed to enter that target
environment safely.

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

### DEB-family packages
Arguments are action words per Debian Policy; Ubuntu packages use these DEB
scriptlet semantics in the current public support matrix:
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

## Namespace Availability

Protected modes require namespace isolation. When the kernel cannot provide the
needed mount/network/user namespace guarantees, Conary fails before running the
scriptlet rather than silently falling back to direct host mutation.

The diagnostic names the missing protected-sandbox requirement:

```text
Protected scriptlet sandboxing requires mount and user namespace support.
Enable the required kernel/container namespace support or run inside a VM.
Dangerous legacy direct execution is available only with --sandbox=never plus
the live-host mutation acknowledgement, and it records effective_sandbox=direct.
```

Direct execution via `--sandbox=never` still uses stdin nullification,
environment filtering, timeouts, and resource limits where available, but it is
not a filesystem sandbox.

## Post-Scriptlet Degradation

Post-install and post-remove scriptlets from legacy packages can fail after
package file state has changed only when the sandbox setup succeeded and the
script process itself exited nonzero. Conary records those failures in changeset
metadata as `scriptlet_warning` entries with `phase`, `failure_kind`,
`requested_sandbox_mode`, and `effective_sandbox`, and `conary history` marks
the changeset with `[scriptlet-warning]`.

Sandbox setup, namespace preflight, interpreter setup, timeout, and enforcement
failures do not degrade to warning-only scriptlet side effects. They fail the
command instead of being recorded as successful package operations.

## Assurance Notes

Protected live-root scriptlets do not receive the `chroot` syscall in the
enforced live-root seccomp profile. When unprivileged user namespaces are used,
setup may enter the prepared root with chroot after root maps to a non-host
UID/GID; that is distinct from allowing the scriptlet process to call `chroot`.
Target-root build/install flows may still use chroot-style execution for
alternate roots; that is not the protected live-root sandbox boundary.

## Security Recommendations

### For Package Maintainers
1. Keep scriptlets minimal - prefer triggers for common operations
2. Avoid network access in scriptlets
3. Use absolute paths for all commands
4. Handle errors gracefully (set -e)

### For System Administrators
1. Keep the default `--sandbox=always` for packages from untrusted sources
2. Review scriptlets before installing unknown packages:
   `conary query scripts ./package.rpm`
3. Monitor `/var/log/conary.log` for scriptlet warnings
4. Consider `--sandbox=always` for high-security environments

### For Repository Operators
1. Scan packages for dangerous patterns before publishing
2. Sign packages and repository metadata with the configured trust roots
3. Use separate repositories for trusted vs. community packages

## Implementation Files

| File | Purpose |
|------|---------|
| `crates/conary-core/src/scriptlet/mod.rs` | Public scriptlet API hub and re-exports |
| `crates/conary-core/src/scriptlet/types.rs` | Package format and execution mode value types |
| `crates/conary-core/src/scriptlet/outcome.rs` | Typed scriptlet outcomes and failure classification |
| `crates/conary-core/src/scriptlet/phases.rs` | Scriptlet phase string conversions |
| `crates/conary-core/src/scriptlet/executor.rs` | Public `ScriptletExecutor` orchestration |
| `crates/conary-core/src/scriptlet/arguments.rs` | RPM, Debian, and Arch argument mapping |
| `crates/conary-core/src/scriptlet/sandbox.rs` | Sandbox mode and protected live-root policy |
| `crates/conary-core/src/scriptlet/process.rs` | Direct, target-root, chroot, and sandboxed process execution |
| `crates/conary-core/src/scriptlet/legacy.rs` | Legacy replay invocation contracts |
| `crates/conary-core/src/scriptlet/runtime.rs` | Subprocess, seccomp, and chroot helper plumbing |
| `crates/conary-core/src/container/mod.rs` | Container isolation, risk analysis |
| `crates/conary-core/src/trigger/mod.rs` | Post-install triggers (preferred over scriptlets) |
| `crates/conary-core/src/db/models/scriptlet_entry.rs` | Scriptlet database storage |

## Implemented Since Initial Design

The following features, originally planned as future enhancements, are now implemented:

- **seccomp-BPF syscall filtering** -- See `crates/conary-core/src/capability/enforcement/seccomp_enforce.rs`
- **Network namespace isolation** -- `CLONE_NEWNET` blocks all network access in hermetic builds
- **Landlock filesystem enforcement** -- Kernel-enforced path restrictions via `crates/conary-core/src/capability/enforcement/landlock_enforce.rs`
- **Capability declarations** -- Packages declare network, filesystem, and syscall requirements
- **Protected live-root writable layers** -- `/etc` and `/var` writes in
  protected scriptlet modes go to private sandbox directories instead of the
  live host
- **Structured scriptlet warning metadata** -- warning-only legacy
  post-scriptlet failures are stored on changesets and surfaced in history
- **Scriptlet-scoped host integration declarations** -- CCS manifests can
  declare the narrow `systemd-service-registration`, `tmpfiles-registration`,
  and `dbus-service-registration` scriptlet capabilities; install fails closed
  until enforcement exists unless the operator chooses direct legacy execution

## Future Enhancements

- cgroups v2 for finer resource control
- Script signing and verification
