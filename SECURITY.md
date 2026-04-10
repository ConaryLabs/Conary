# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Conary, **please do not open a public issue.** Security vulnerabilities disclosed publicly before a fix is available put all users at risk.

Instead, report vulnerabilities through **GitHub Security Advisories**:

1. Go to [https://github.com/ConaryLabs/Conary/security/advisories](https://github.com/ConaryLabs/Conary/security/advisories)
2. Click "New draft security advisory"
3. Fill in the details of the vulnerability
4. Submit the advisory

This ensures the report is private and visible only to the maintainers until a fix is ready.

## What to Include

A good vulnerability report helps us respond quickly. Please include:

- **Description**: What the vulnerability is and its potential impact
- **Affected component**: Which module or feature is affected (e.g., signature verification, scriptlet sandboxing, daemon API)
- **Reproduction steps**: How to trigger the vulnerability
- **Environment**: Conary version, Linux distribution, kernel version
- **Suggested fix**: If you have one (optional but appreciated)

## Response Process

We triage security reports privately, confirm impact, and coordinate disclosure
before publishing fixes. Timing depends on severity, exploitability, report
quality, and maintainer availability. Critical issues may be handled
out-of-band; lower-risk fixes may ship in the next planned release.

We will coordinate disclosure with you. You will be credited in the advisory unless you prefer to remain anonymous.

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.7.x   | Yes       |
| < 0.7   | No        |

Only the latest release receives security updates. We recommend always running the most recent version.

## Security Architecture

Conary is a system-level package manager that executes with elevated privileges. Security is a core design concern, not an afterthought. Key security features include:

- **Ed25519 package signatures** -- cryptographic verification of package authenticity
- **Signed repository metadata** -- remote metadata is verified before install planning
- **Self-update signature verification** -- downloaded update artifacts are verified before replacement
- **Linux namespace isolation** -- scriptlets run in sandboxed mount, PID, IPC, UTS, network, and user namespaces where supported
- **Landlock filesystem restrictions** -- kernel-enforced limits on which paths scriptlets can access
- **Seccomp-BPF syscall filtering** -- restrict which system calls scriptlets can make
- **TUF trust metadata** -- repository metadata follows The Update Framework for supply chain integrity
- **Content-addressable storage** -- files stored by cryptographic hash, preventing tampering
- **Atomic transactions** -- journaled changesets prevent partial installs from leaving the system in a broken state
- **Peer credential authentication** -- daemon API uses SO_PEERCRED to verify caller identity
- **Pinned federation peer identity** -- HTTPS federation peers are bound to pinned TLS certificate fingerprints
- **Rate limiting and CORS** -- server endpoints are protected against abuse

## Scope

The following are considered in scope for security reports:

- Signature verification bypasses
- Sandbox escapes (namespace, landlock, seccomp)
- Privilege escalation through the daemon API or CLI
- Path traversal or arbitrary file write during package installation
- Dependency confusion or supply chain attacks through the resolution system
- Denial of service against the daemon or server
- Authentication or authorization bypasses

The following are generally out of scope:

- Vulnerabilities in upstream dependencies (report these to the relevant project, but let us know so we can update)
- Issues requiring physical access to the machine
- Social engineering attacks
- Vulnerabilities in configurations explicitly documented as unsafe (e.g., `--no-isolation`)

## Acknowledgments

We appreciate the security research community's efforts in responsibly disclosing vulnerabilities. Contributors who report valid security issues will be acknowledged in the release notes and security advisory, unless they request otherwise.
