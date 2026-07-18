---
last_updated: 2026-07-18
status: scheduled
current_result: 0/10
summary: Outcome tracker for Conary's first external tester milestone
---

# First External Tester Milestone

The milestone closes when ten unique people outside the existing project
circle complete the full supported flow and report friction, or when a
maintainer records an evidence-backed pivot for a reproducible systemic
blocker. Interest, downloads, partial attempts, and repeated runs by the same
person do not count as separate completions.

**Current result: 0/10 qualifying completions.**

## Qualifying Flow

Each qualifying report covers this sequence on one supported host:

```text
install -> adopt -> list/search -> update --dry-run -> unadopt
```

The report must confirm that the tester used the pinned release, stayed within
the supported host scope, and completed every stage. A failed attempt remains
useful evidence but does not count as a completion.

## Launch Record

- Pinned Conary release target: `v0.11.3`, not yet published or verified. The
  `v0.11.2` gate was reopened for `v0.11.3` after the real supported `htop`
  path exposed unreachable generic SONAME evidence; review also found an
  inexact or ABI-unchecked critical-library fallback and discarded Arch
  capability constraints. The exact annotated tag and commit, release-build
  run, and deploy-and-verify run are pending.
- Release payload evidence: seven assets are expected. Publication and
  independent hashes for all five `SHA256SUMS` payloads are pending:

  | Manifest payload | SHA-256 |
  | --- | --- |
  | `conary-0.11.3-1-x86_64.pkg.tar.zst` | pending |
  | `conary-0.11.3-1.fc44.x86_64.rpm` | pending |
  | `conary-0.11.3.ccs` | pending |
  | `conary_0.11.3-1_amd64.deb` | pending |
  | `conary-0.11.3.metadata.json` | pending |

  Offline verification of `conary-0.11.3.ccs.sig` against the published CCS is
  pending. `SHA256SUMS` and the detached signature are the other two expected
  assets. No SBOM or provenance sidecars are planned for this limited preview;
  their absence remains an explicit caveat.
- Installed-binary evidence: pending. The preceding official binary must
  detect and apply the signed `v0.11.3` CCS against an isolated schema-77
  database, print `Signature verified`, report `conary 0.11.3`, preserve the
  schema, and then report itself up to date. Record both CCS and resulting
  binary SHA-256 values.
- Candidate SONAME repair: the live-runtime path now requires an exact SONAME
  cache entry and compatible ELF class, while Arch's versioned SONAME
  capability uses its full constraint in the `pacman` proof. Release-asset and
  supported-host `htop` evidence remain pending.
- Compatible rewritten Remi commit:
  `27ec2eccb6befdf06d9a826b84cc5a6948eff5fb`. It and deployed pre-rewrite
  source commit `c001f8d69b9e8ef34fba39139576a9809800a9a6` have the same tree. The
  deployed binary SHA-256 is
  `c955a24ff6b90f98ba5f20b37e6a67b79bdde199ec0dcbfac0ce78b001d0f485`.
- Prewarmed package set: `curl`, `htop`, `nano`, and `zstd` for Fedora, Ubuntu,
  and Arch. Eleven conversion-version-6 rows are public; Ubuntu `nano` remains
  correctly fail-closed as `private-review`.
- Superseded clean-host and onboarding baseline: the preceding release proved
  Fedora installation/execution/removal, profile-correct Arch initialization,
  and Arch Remi synchronization, but that evidence no longer closes the launch
  gate after the supported `htop` SONAME flaw. Repeat the real package path and
  released native-package initialization with the `v0.11.3` artifacts.
- Deployment evidence: pending for `v0.11.3`. The prior self-update API, full
  Remi-health 10/10, and checked production-site deployments remain a
  superseded baseline; repeat deploy-and-verify, health, and checked site
  deployment against the exact candidate release.
- Supported hosts: Fedora 44, Ubuntu 26.04 LTS, and Arch Linux on the release's
  supported architecture and compatibility baseline.
- Planned launch sequence, using the venue-specific copy in
  `docs/operations/external-tester-outreach.md`:

  | Venue | Planned timestamp | State |
  | --- | --- | --- |
  | Show HN | Monday, 2026-07-20 at 15:00 CEST (`13:00 UTC`) | scheduled |
  | r/codex | Tuesday, 2026-07-21 at 15:00 CEST (`13:00 UTC`) | scheduled; re-check rules and posting eligibility |
  | r/ClaudeAI | Wednesday, 2026-07-22 at 15:00 CEST (`13:00 UTC`) | scheduled; requires current showcase eligibility and posting-account karma over 50 |

- Actual post URLs and launch timestamps: not launched. Outreach remains gated
  until the exact `v0.11.3` release evidence is complete, GitHub Support
  dereferences the cached pull-request and commit views that still expose
  pre-rewrite history, and the per-venue eligibility checks pass. Record each
  post immediately after submission. The three-week stall clock starts from
  the first actual launch timestamp.
- Privacy-safe feedback path: the beta-feedback issue template and a reviewed
  support bundle; never request secrets, credential files, private keys, broad
  environment dumps, or a live database by default.

## Outcomes

Use an opaque report or issue reference rather than a person's name or host
identity. The triage owner is responsible for keeping secrets and broad machine
dumps out of linked evidence.

| Attempt | Date | Privacy-safe report | Distro and host scope | Pinned release | Full flow completed | Friction or failure | Triage status | Triage owner |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| - | - | - | - | - | - | - | - | - |

Triage statuses are `fix-now`, `next-slice`, and `declined-with-reason`.
Qualifying and failed attempts both require an owner and a triage disposition.

## Stall and Pivot Rules

If no qualifying completion is recorded for three consecutive weeks after the
launch timestamp, the maintainer reviews venue reach, onboarding friction, and
observed failures. The review records whether to revise outreach, repair a
blocker, or invoke the pivot exit.

A pivot requires a reproducible failure inside the supported scope, the number
and shape of affected attempts, and either a chosen remediation or an explicit
support-scope change. Ordinary outreach difficulty or one unexplained partial
attempt is not sufficient.

## Closeout

At milestone closeout, durable launch facts and findings move to the detailed
roadmap, release history, and owning product documentation. Delete this tracker
after that transfer; Git history remains the record of individual status
updates.
