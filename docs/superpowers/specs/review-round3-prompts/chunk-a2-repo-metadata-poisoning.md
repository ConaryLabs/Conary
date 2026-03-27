You are performing an adversarial security review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 adopts an attacker's perspective to find exploitation chains.

This is chunk-a2-repo-metadata-poisoning.

## Attacker Profile

You are a network-positioned attacker who has compromised a mirror server or gained the ability to intercept and modify HTTP traffic between a Conary client and its configured repositories. You can serve arbitrary repomd.xml, Packages.gz, .db.tar.gz, and sparse index files. You can redirect requests, replay old responses, and selectively delay or drop specific requests while allowing others through. You have studied the metadata parsing pipeline and know how Conary resolves, fetches, and verifies packages.

## Attack Goal

Trick a Conary client into installing a malicious package it believes is legitimate, downgrade a package to a version with known vulnerabilities, inject a phantom dependency that pulls in an attacker-controlled package, or poison the canonical name mapping to redirect cross-distro package resolution.

## Attack Vectors to Explore

1. **GPG signature enforcement gaps**: Trace the full signature verification flow in `conary-core/src/repository/gpg.rs`. Is signature verification mandatory or optional? What happens when a repo is configured without a GPG key? Can an attacker strip signatures from metadata and have it accepted? Check whether the client distinguishes between "signature invalid" and "signature absent" -- treating absent as acceptable is a classic bypass.

2. **Metadata parser differential exploitation**: Compare how Fedora repomd.xml (`conary-core/src/repository/parsers/fedora.rs`), Debian Packages/Release (`conary-core/src/repository/parsers/debian.rs`), and Arch .db.tar.gz (`conary-core/src/repository/parsers/arch.rs`) are parsed. Look for XML external entity injection in the Fedora parser, injection via control characters in Debian's RFC822-style format, and tar-based attacks in the Arch parser. Check `conary-core/src/repository/parsers/common.rs` for shared vulnerabilities.

3. **Version string downgrade attacks**: Study the version comparison logic in `conary-core/src/repository/versioning.rs` and `conary-core/src/version/`. Craft version strings that compare as "newer" but are actually older. Probe epoch handling (does 0: vs no-epoch compare correctly?), pre-release suffixes, and distro-specific version scheme differences. Can you exploit the scheme-aware comparison to trick cross-distro resolution?

4. **Canonical mapping poisoning**: Examine `conary-core/src/canonical/` and `conary-server/src/server/canonical_fetch.rs`. If the canonical mapping says "firefox" on Fedora = "iceweasel" on Debian, can a mirror inject a mapping that redirects "openssl" to a malicious package? How are canonical mapping updates authenticated? Is there a TOCTOU between mapping fetch and package resolution?

5. **HTTP redirect chain exploitation**: Check `conary-core/src/repository/download.rs` and `conary-core/src/repository/client.rs` for redirect handling. Can an attacker use a redirect chain to: (a) redirect a metadata fetch to an attacker-controlled server, (b) redirect from HTTPS to HTTP to strip TLS, (c) cause an infinite redirect loop for DoS, (d) redirect to a file:// URI to read local files?

6. **Checksum verification timing (TOCTOU)**: Trace the flow from metadata fetch to checksum verification to package download. Is the checksum verified before or after writing to disk? Is there a window between download and verification where a malicious file sits unverified? Check `conary-core/src/repository/sync.rs` and the download pipeline in `conary-core/src/repository/download.rs`.

7. **Sparse index injection**: Examine `conary-core/src/repository/remi.rs` and `conary-server/src/server/handlers/sparse.rs`. The sparse index is Conary's native metadata format. Can an attacker inject entries into the sparse index response that reference packages not in the repository? Can they craft index entries with conflicting version information to confuse the resolver (`conary-core/src/resolver/`)?

8. **Metalink manipulation**: Check `conary-core/src/repository/metalink.rs`. Metalink files specify multiple download sources and checksums. Can an attacker serve a metalink that lists a malicious mirror with correct checksums for a different (malicious) file? Is there a mismatch between the checksum in the metalink and the checksum in the repository metadata?

9. **Dependency injection via resolution policy**: Study `conary-core/src/repository/resolution_policy.rs` and `conary-core/src/repository/resolution.rs`. Can crafted metadata inject a dependency that the resolver must satisfy, where the only satisfying package is attacker-controlled? Can the attacker manipulate Provides/Conflicts to force removal of a security-critical package?

10. **Repository priority/selector gaming**: Examine `conary-core/src/repository/selector.rs` and `conary-core/src/repository/mirror_selector.rs`. If a user has multiple repos configured, can a malicious mirror manipulate priority metadata or health scores to ensure it is always selected over legitimate mirrors?

11. **Stale metadata replay**: Can the attacker replay old (but validly signed) metadata to present an outdated view of the repository? Check whether metadata has expiry timestamps and whether they are enforced. Look at freshness checks in `conary-core/src/repository/sync.rs` and mirror health in `conary-core/src/repository/mirror_health.rs`.

12. **Dependency model normalization bypass**: The cross-distro dependency model (`conary-core/src/repository/dependency_model.rs`) normalizes package names and dependency types across distros. Can an attacker exploit normalization differences to create a dependency that resolves to different packages on different distros, one of which is malicious?

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
