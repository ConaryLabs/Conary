You are performing an adversarial security review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 adopts an attacker's perspective to find exploitation chains.

This is Chunk A8: Rollback/Generation Persistence.

## Attacker Profile

You are an attacker who has already achieved code execution on the target system (e.g., via a malicious package install or compromised scriptlet). The system administrator detects the compromise and uses `conary rollback` or `conary generation switch` to revert to a known-good state. You anticipated this and planted persistence mechanisms that survive rollback.

## Attack Goal

Maintain code execution, data exfiltration, or backdoor access after the user rolls back to a previous generation or runs garbage collection. Secondary goals: make the rollback appear successful while persistence remains hidden, corrupt the rollback mechanism itself to prevent recovery, or cause GC to delete legitimate packages while preserving malicious ones.

## Attack Vectors to Explore

1. **/etc overlay survival** -- Conary uses composefs with an overlayfs layer for `/etc`. User modifications to `/etc` survive generation switches by design. Can the attacker plant files in `/etc` (cron jobs, systemd units, shell profiles, PAM modules, sudoers.d entries, ld.so.conf.d entries) that survive rollback because the /etc merge treats them as "user modifications"?

2. **CAS persistence via state references** -- Can the attacker create a trove or state entry in the database that references malicious CAS objects, ensuring GC never collects them? Can they create circular references or pin entries that prevent cleanup?

3. **Scriptlet side effects not reversed** -- Scriptlets can create system users, install systemd units, add cron jobs, modify PAM configuration, write to `/var`, create Unix sockets, or register D-Bus services. On rollback, are the reverse scriptlets (pre-remove, post-remove) executed for packages being rolled back? If not, these side effects persist indefinitely.

4. **State snapshot drift** -- After rollback, does the database accurately reflect only the packages in the target generation? Or do metadata remnants from the malicious generation persist (e.g., dependency entries, provide entries, file entries pointing to attacker-controlled paths)?

5. **Composefs image immutability** -- EROFS images are supposed to be immutable. Can the attacker modify the EROFS image of a previous generation (the one that will be rolled back to) while the current (compromised) generation is active? Are EROFS images verified before mounting during rollback?

6. **/etc merge attacker-planted files** -- The three-way /etc merge compares previous generation's /etc, new generation's /etc, and user modifications. Can the attacker manipulate the merge by:
   - Planting a file that looks like a "user modification" to the merge algorithm
   - Modifying the "previous generation" baseline so the merge preserves attacker changes
   - Creating a merge conflict that is resolved in the attacker's favor

7. **/var and other mutable state** -- `/var` is typically not managed by the generation system. Can the attacker place persistence mechanisms in `/var/lib`, `/var/spool`, `/var/cache`, or other mutable directories that the generation system does not track or rollback?

8. **Generation metadata manipulation** -- Can the attacker modify generation metadata (timestamps, trove lists, parent pointers) to make a malicious generation appear to be the "known good" target for rollback?

9. **GC evasion** -- The garbage collector removes unreferenced generations and CAS objects. Can the attacker:
   - Create references from a legitimate generation to malicious CAS objects
   - Modify reference counts to prevent collection
   - Plant a CAS object whose hash collides with a legitimate object (SHA-256 second preimage -- theoretically infeasible, but is the hash verified on read?)

10. **Boot-time persistence** -- If conary manages boot entries (kernel, initramfs), can the attacker modify the bootloader configuration or initramfs of a previous generation to include a backdoor that activates on rollback+reboot?

11. **Rollback to attacker-controlled generation** -- Can the attacker create a fake generation that appears legitimate (correct metadata, valid EROFS image) but contains malicious content, then manipulate the generation list so the user rolls back to it instead of the real known-good state?

12. **Post-rollback hook exploitation** -- Are there any hooks, triggers, or automatic operations that run after a rollback completes? Can the attacker register a trigger that re-installs the malicious package after rollback?

## Output Format

For each finding, report:

### [SEVERITY] FILE_A:LINE -> FILE_B:LINE -- Short title

**Boundary:** Which two modules/files this crosses
**Category:** EtcPersistence | CASEvasion | ScriptletSideEffect | MetadataManipulation | MutableState | GCEvasion
**Exploitation chain:** Step-by-step attack description showing how the attacker plants persistence before rollback and how it survives.
**Description:** What is wrong and why it matters.
**Suggested fix:** Concrete change at one or both sides of the boundary.

Severity levels:
- CRITICAL: Full persistence across rollback -- attacker retains code execution or backdoor access
- HIGH: Partial persistence -- attacker retains data exfiltration or can re-establish access
- MEDIUM: Attacker can degrade rollback reliability or hide traces
- LOW: Theoretical persistence path that requires additional privileges or unlikely conditions

## Scope

You are NOT limited to specific files. Follow the attack wherever it leads across the entire codebase. Key starting points include `conary-core/src/generation/` (builder, mount, gc, metadata, etc_merge), `conary-core/src/filesystem/cas.rs`, `conary-core/src/db/models/`, `conary-core/src/scriptlet/`, `conary-core/src/trigger/`, and `src/commands/rollback.rs`, but trace into any file that participates in generation lifecycle, state management, or cleanup.

## Summary

- Critical: N
- High: N
- Medium: N
- Low: N

Top 3 rollback persistence risks:
1. ...
2. ...
3. ...
