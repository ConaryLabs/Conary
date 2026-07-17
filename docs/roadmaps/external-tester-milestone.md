---
last_updated: 2026-07-17
status: ready
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

- Pinned Conary release: `v0.11.1`, exact commit
  `4d4b422b45b055fa07a3885a68a4ab8e8d16b526`. Release-build run
  `29540722051` published the Fedora 44, Ubuntu 26.04 LTS, Arch, and signed CCS
  artifacts; deploy-and-verify run `29542934278` installed the release bundle
  and verified Remi's self-update endpoint.
- Installed-binary proof: the official preceding-preview Fedora RPM binary
  verified the update signature, replaced itself with `v0.11.1`, and then
  reported itself up to date. Independent downloads matched `SHA256SUMS`, and
  the detached CCS signature verified offline.
- Compatible Remi commit: `c001f8d69b9e8ef34fba39139576a9809800a9a6`,
  deployed with binary SHA-256
  `c955a24ff6b90f98ba5f20b37e6a67b79bdde199ec0dcbfac0ce78b001d0f485`.
- Prewarmed package set: `curl`, `htop`, `nano`, and `zstd` for Fedora, Ubuntu,
  and Arch. Eleven conversion-version-6 rows are public; Ubuntu `nano` remains
  correctly fail-closed as `private-review`.
- Clean-host proof: Fedora 44 installed, executed, and removed the public
  `htop 3.4.1` conversion after the exact public-target compatibility fix.
- Supported hosts: Fedora 44, Ubuntu 26.04 LTS, and Arch Linux on the release's
  supported architecture and compatibility baseline.
- Venue: not launched.
- Launch timestamp: not launched.
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
