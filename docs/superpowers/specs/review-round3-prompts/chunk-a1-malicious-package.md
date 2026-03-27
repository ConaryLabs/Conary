You are performing an adversarial security review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 adopts an attacker's perspective to find exploitation chains.

This is chunk-a1-malicious-package.

## Attacker Profile

You are a malicious package maintainer who has gained the ability to publish packages to a repository that a target system trusts. You can craft arbitrary CCS, RPM, DEB, or Arch packages with any content, metadata, file paths, permissions, scriptlets, and hooks you choose. You have full knowledge of Conary's archive extraction pipeline, CCS format internals, and scriptlet execution model. You do NOT have direct access to the target system -- your only attack surface is the package content itself.

## Attack Goal

Achieve arbitrary code execution on the target system during package install, upgrade, or removal. Failing that, achieve privilege escalation from the conary process to root, write to arbitrary filesystem paths outside the package's declared scope, or corrupt system state to enable a second-stage attack.

## Attack Vectors to Explore

1. **Tar-slip / path traversal in archive extraction**: Craft archive entries with paths containing `../`, absolute paths, or null bytes. Trace how `conary-core/src/ccs/archive_reader.rs` and `conary-core/src/packages/archive_utils.rs` handle path sanitization. Check whether the CCS builder (`conary-core/src/ccs/builder.rs`) validates paths on pack vs. the reader on unpack -- asymmetry here is exploitable.

2. **Symlink-following during extraction**: Create a package that installs a symlink to `/etc/shadow` in one component, then a second component writes through that symlink. Examine how `conary-core/src/filesystem/cas.rs` and `conary-core/src/filesystem/vfs/mod.rs` handle symlinks during CAS materialization. Check whether extraction is done in a single pass (vulnerable) or two-pass with symlink resolution.

3. **Scriptlet sandbox escape**: Conary runs scriptlets in a sandboxed container (`conary-core/src/container/mod.rs`, `conary-core/src/scriptlet/mod.rs`). Probe the namespace isolation -- are all namespaces (mount, network, pid, user, cgroup) properly isolated? Can a scriptlet escape via `/proc/self/exe`, `nsenter`, shared mount propagation, or leaked file descriptors? Check if scriptlets have access to the host's `/dev`, `/sys`, or `CAP_SYS_ADMIN`.

4. **CCS hook abuse**: Examine the declarative hook system in `conary-core/src/ccs/hooks/`. Can a malicious package declare a `systemd` hook (`hooks/systemd.rs`) that installs a service with `ExecStart=/bin/sh -c 'malicious command'`? Can `tmpfiles` hooks (`hooks/tmpfiles.rs`) create world-writable setuid files? Can `sysctl` hooks (`hooks/sysctl.rs`) weaken kernel security parameters? Are hook contents validated against an allowlist or just passed through?

5. **setuid/setgid/sticky bit preservation**: Craft a package with files that have setuid-root bits set. Follow the extraction path through `conary-core/src/ccs/archive_reader.rs` -> `conary-core/src/filesystem/cas.rs` -> generation building (`conary-core/src/generation/builder.rs`). Does anything strip dangerous permission bits? Does the CCS policy engine (`conary-core/src/ccs/policy.rs`) check for this?

6. **Unicode and encoding tricks**: Use Unicode characters that normalize to `../` (e.g., fullwidth solidus U+FF0F, overlong UTF-8 sequences) in file paths. Check if path sanitization in `conary-core/src/filesystem/path.rs` handles these before or after normalization. Test with mixed-encoding paths (Latin-1 in an otherwise UTF-8 archive).

7. **Legacy format parser exploitation**: The RPM parser (`conary-core/src/packages/rpm.rs`, `conary-core/src/ccs/legacy/rpm.rs`), DEB parser (`conary-core/src/packages/deb.rs`, `conary-core/src/ccs/legacy/deb.rs`), Arch parser (`conary-core/src/packages/arch.rs`, `conary-core/src/ccs/legacy/arch.rs`), and CPIO parser (`conary-core/src/packages/cpio.rs`) all handle untrusted binary data. Look for integer overflow in header size fields, unbounded allocations from attacker-controlled lengths, and buffer over-reads from truncated archives.

8. **Component misclassification for privilege escalation**: Conary classifies files into components (`conary-core/src/components/`). Can a malicious package trick the classifier into putting a setuid binary into a `:lib` component that gets auto-installed, or putting malicious content into a `:doc` component to bypass security review?

9. **CCS binary manifest manipulation**: Examine `conary-core/src/ccs/binary_manifest.rs` and `conary-core/src/ccs/manifest.rs`. Can a crafted manifest declare one set of files for verification but install a different set? Is there a TOCTOU between manifest parsing and file extraction?

10. **Trigger system abuse**: The trigger system (`conary-core/src/trigger/mod.rs`) fires post-install actions. Can a malicious package register triggers that execute on OTHER packages' install events? Can triggers be crafted to chain into a persistent backdoor that survives package removal?

11. **Decompression bombs**: Craft a CCS with a zstd/gzip/xz payload that has an extreme compression ratio (e.g., 1 KB compressed -> 10 GB decompressed). Check if `conary-core/src/compression/` has decompression limits. Also check for zip-bomb equivalents in the CCS chunking layer (`conary-core/src/ccs/chunking.rs`).

12. **EROFS/composefs image injection**: Since generations are EROFS images (`conary-core/src/generation/builder.rs`, `conary-core/src/generation/composefs.rs`), can a malicious package inject entries into the EROFS image that appear outside its declared file list? Examine whether the composefs mount captures the exact VFS tree or could be manipulated.

## Output Format

For each finding, report:

### [SEVERITY] Exploitation chain title

**Attack:** Step-by-step description of how an attacker exploits this
**Entry point:** Where the attack begins (file:line)
**Impact:** What the attacker achieves
**Suggested fix:** Concrete change to prevent the attack

Severity levels:
- CRITICAL: Achievable with reasonable effort, high impact (code execution, privilege escalation, data loss)
- HIGH: Achievable but requires specific conditions, significant impact
- MEDIUM: Theoretical or requires unlikely conditions
- LOW: Defense-in-depth improvement, no direct exploitation

## Scope

You are NOT limited to specific files. Follow the attack wherever it leads across the entire codebase. Start with the most promising entry points and trace each attack chain to completion or dead end.

## Summary

- Critical: N
- High: N
- Medium: N
- Low: N

Top 3 most exploitable findings:
1. ...
2. ...
3. ...
