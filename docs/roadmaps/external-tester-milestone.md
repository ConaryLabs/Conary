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

- Pinned Conary release candidate: `v0.11.2`. The exact tag commit,
  release-build run, deploy-and-verify run, and final artifact hashes will be
  recorded here immediately after publication; outreach remains gated until
  that evidence is complete.
- Installed-binary baseline: W2 proved the preceding official Fedora RPM could
  verify the update signature, replace itself with the then-current preview,
  and report itself up to date. The `v0.11.2` installed-package and self-update
  evidence is pending publication.
- Compatible Remi commit: `c001f8d69b9e8ef34fba39139576a9809800a9a6`,
  deployed with binary SHA-256
  `c955a24ff6b90f98ba5f20b37e6a67b79bdde199ec0dcbfac0ce78b001d0f485`.
- Prewarmed package set: `curl`, `htop`, `nano`, and `zstd` for Fedora, Ubuntu,
  and Arch. Eleven conversion-version-6 rows are public; Ubuntu `nano` remains
  correctly fail-closed as `private-review`.
- Clean-host baseline: Fedora 44 installed, executed, and removed the public
  `htop 3.4.1` conversion after the exact public-target compatibility fix.
  The `v0.11.2` native-package onboarding proof is pending publication.
- Supported hosts: Fedora 44, Ubuntu 26.04 LTS, and Arch Linux on the release's
  supported architecture and compatibility baseline.
- Planned launch sequence, using the venue-specific copy in
  `docs/operations/external-tester-outreach.md`:

  | Venue | Planned timestamp | State |
  | --- | --- | --- |
  | Show HN | Monday, 2026-07-20 at 15:00 CEST (`13:00 UTC`) | scheduled |
  | r/codex | Tuesday, 2026-07-21 at 15:00 CEST (`13:00 UTC`) | scheduled; re-check rules and posting eligibility |
  | r/ClaudeAI | Wednesday, 2026-07-22 at 15:00 CEST (`13:00 UTC`) | scheduled; requires current showcase eligibility and posting-account karma over 50 |

- Actual post URLs and launch timestamps: not launched; record each
  immediately after submission. The three-week stall clock starts from the
  first actual launch timestamp.
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
