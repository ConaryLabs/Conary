---
last_updated: 2026-07-23
status: active
current_result: 1/10
summary: Outcome tracker for Conary's first external tester milestone
---

# First External Tester Milestone

The milestone closes when ten unique people outside the existing project
circle complete the full supported flow and report friction, or when a
maintainer records an evidence-backed pivot for a reproducible systemic
blocker. Interest, downloads, partial attempts, and repeated runs by the same
person do not count as separate completions.

**Current result: 1/10 qualifying completions.** Two supported-host reports from
the same person count as one unique external tester.

## Qualifying Flow

Each qualifying report covers this sequence on one supported host:

```text
install -> adopt -> list/search -> update --dry-run -> unadopt
```

The report must confirm that the tester used the pinned release, stayed within
the supported host scope, and completed every stage. A failed attempt remains
useful evidence but does not count as a completion.

## Launch Record

- Pinned Conary release candidate: `v0.12.0`. The local annotated candidate
  contains the safe in-root symlink and installed-host support-bundle
  remediation. Its exact remote tag object, peeled commit, publication time,
  release-build run, and deploy-and-verify run remain pending.
- Release payload evidence: pending. Before this candidate becomes the pinned
  preview, independently match every `SHA256SUMS` payload and REST digest,
  verify the detached CCS signature with the preceding official binary, and
  record the exact hashes. No SBOM or provenance sidecars are planned for this
  limited preview; their absence remains an explicit caveat.
- Installed-binary evidence for `v0.12.0`: pending. Prove a signed self-update
  from the preceding official binary against an isolated schema-77 database,
  then prove the resulting binary reports itself current.
- Supported Arch package evidence for `v0.12.0`: pending. The shipped Arch
  artifact must initialize profile `arch`, synchronize only Arch rows, resolve
  `htop` without blocked or unresolved dependencies, install through the real
  `/usr/lib64 -> lib` ancestor, execute, remove cleanly, and leave the host
  untouched. Record whether the proof uses a disposable VM or an isolated
  `bwrap` target with host evidence exposed read-only.
- Fedora-form evidence for `v0.12.0`: pending. Any conaryOS guest proof must
  retain the caveat that it is not literal stock Fedora native-PM onboarding.
- Compatible rewritten Remi commit:
  `27ec2eccb6befdf06d9a826b84cc5a6948eff5fb`. It and deployed pre-rewrite
  source commit `c001f8d69b9e8ef34fba39139576a9809800a9a6` have the same tree. The
  deployed binary SHA-256 is
  `c955a24ff6b90f98ba5f20b37e6a67b79bdde199ec0dcbfac0ce78b001d0f485`.
- Prewarmed package set: `curl`, `htop`, `nano`, and `zstd` for Fedora, Ubuntu,
  and Arch. Eleven conversion-version-6 rows are public; Ubuntu `nano` remains
  correctly fail-closed as `private-review`.
- Deployment evidence for `v0.12.0`: pending exact-tag deployment, full Remi
  health, all six public routes, checked site chunks, and self-update API CCS
  hash, size, and signature identity.
- Repository-community closeout: the
  [Welcome Discussion](https://github.com/ConaryLabs/Conary/discussions/36) is
  live, and [issue #35's released-path proof](https://github.com/ConaryLabs/Conary/issues/35#issuecomment-5009942880)
  was recorded before the issue was closed. These repository actions do not
  count as a qualifying external completion or launch the broad outreach loop.
- Organic prelaunch tester evidence: [issue #37](https://github.com/ConaryLabs/Conary/issues/37)
  completed the full then-current loop on x86_64 Ubuntu 26.04 LTS and
  [confirmed the DEB checksum](https://github.com/ConaryLabs/Conary/issues/37#issuecomment-5010174050).
  [Issue #38](https://github.com/ConaryLabs/Conary/issues/38) completed the same
  loop on x86_64 Fedora 44 from a never-synced Remi state and
  [confirmed the RPM checksum](https://github.com/ConaryLabs/Conary/issues/38#issuecomment-5010310461).
  Both reports came from the same external tester, so they establish two
  supported-host successes but count once toward the ten-person milestone.
  They do not start the broad-outreach stall clock.
- Prelaunch remediation evidence:
  [issue #41](https://github.com/ConaryLabs/Conary/issues/41) reports that
  the preceding preview rejects the legitimate Arch-style `/usr/lib64 -> lib`
  ancestor while validating a Fedora-form CCS payload on Artix. Artix and that
  cross-distro route are outside the supported-host claim, but the in-root
  symlink false-positive is a valid fail-closed-path defect. The `v0.12.0`
  candidate carries that repair. The repaired bundle also uses targeted cached
  authorization for root-owned database diagnostics and records the host
  profile, source pin, and complete repository set.
- Supported hosts: Fedora 44, Ubuntu 26.04 LTS, and Arch Linux on the release's
  supported architecture and compatibility baseline.
- Rescheduling sequence, using the venue-specific copy in
  `docs/operations/external-tester-outreach.md`:

  | Venue | New timestamp | State |
  | --- | --- | --- |
  | Show HN | TBD | postponed; the 2026-07-20 slot passed without a post |
  | r/codex | TBD | postponed; the 2026-07-21 slot passed without a post; re-check rules and posting eligibility |
  | r/ClaudeAI | TBD | postponed; the 2026-07-22 slot passed without a post; requires current showcase eligibility and posting-account karma over 50 |

- Actual post URLs and launch timestamps: not launched. The former dates are
  retired and outreach is postponed. Assign replacement dates only after
  `v0.12.0` is published and verified on supported paths, its release claims
  are refreshed, GitHub Support dereferences the cached pull-request and commit
  views that still expose pre-rewrite history, and the per-venue eligibility
  checks pass. Record each post immediately after submission. The three-week
  stall clock starts from the first actual launch timestamp.
- Privacy-safe feedback path: the beta-feedback issue template and a reviewed
  support bundle; never request secrets, credential files, private keys, broad
  environment dumps, or a live database by default.

## Outcomes

Use an opaque report or issue reference rather than a person's name or host
identity. The triage owner is responsible for keeping secrets and broad machine
dumps out of linked evidence.

| Attempt | Date | Privacy-safe report | Distro and host scope | Pinned release | Full flow completed | Friction or failure | Triage status | Triage owner |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1a | 2026-07-18 | [issue #37](https://github.com/ConaryLabs/Conary/issues/37) | Ubuntu 26.04 LTS, x86_64, VM/snapshot/non-critical host confirmed | then-current DEB; checksum confirmed | yes, 11/11 steps | No functional failure; public-report verbosity and terminal-control-sequence guidance routed through [issue #39](https://github.com/ConaryLabs/Conary/issues/39) | `validated-no-action` | maintainer |
| 1b | 2026-07-18 | [issue #38](https://github.com/ConaryLabs/Conary/issues/38) | Fedora 44, x86_64, VM/snapshot/non-critical host confirmed | then-current RPM; checksum confirmed | yes, 11/11 steps | No functional failure; clean never-synced Remi start; same reporting-guidance follow-up as 1a | `validated-no-action` | maintainer |

Attempts 1a and 1b came from the same person and therefore contribute one, not
two, to the unique-tester result. Triage statuses are `fix-now`, `next-slice`,
`validated-no-action`, and `declined-with-reason`. Qualifying and failed
attempts both require an owner and a triage disposition.

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
