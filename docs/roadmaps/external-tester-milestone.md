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

- Pinned Conary release: `v0.11.3`, published immutable at
  `2026-07-18T04:31:28Z`. Annotated tag object
  `a2a12791e695379e9313a210d2fd5eea2a39b352` peels to commit
  `0fc31c33b42a84bb00c9c8d9bdfc574ebe960ae0`. Final merge CI run
  `29628990277` passed 11/11 jobs, release-build run `29629361456` passed, and
  exact-tag deploy-and-verify run `29630694438` passed for both public sites.
  This `v0.11.3` release replaces the reopened `v0.11.2` gate after its supported `htop` path
  exposed unreachable generic SONAME evidence, an inexact or ABI-unchecked
  critical-library fallback, and discarded Arch capability constraints.
- Release payload evidence: seven assets are published. Independent downloads
  matched all five `SHA256SUMS` payloads, and every REST asset digest matched:

  | Manifest payload | SHA-256 |
  | --- | --- |
  | `conary-0.11.3-1-x86_64.pkg.tar.zst` | `92d94443f30a22eee2da06ca336f951394a1c3cdfed0d9321329c2a05a61777e` |
  | `conary-0.11.3-1.fc44.x86_64.rpm` | `5f32eeede9fc43483aa423cc5fc4f69f7577f03719de0c11e3018f847c319b6e` |
  | `conary-0.11.3.ccs` | `c152df62d93e29f6245b2e924a13a9b3650988f34ea35854cb35f556e723160d` |
  | `conary_0.11.3-1_amd64.deb` | `04da72e485992163da976ef6381a68491f75f760ee2381198b25ee7aa893204e` |
  | `metadata.json` | `c2cc2bf053c4325a530f7d4499c155abb094955163347b0e6e4e3bc6f75748b6` |

  The official `v0.11.2` binary verified the `v0.11.3` detached signature against the
  published CCS. `SHA256SUMS` and the detached signature are the other two
  assets. No SBOM or provenance sidecars are published or planned for this
  limited preview; their absence remains an explicit caveat.
- Installed-binary evidence for `v0.11.3` from the official `v0.11.2` binary:
  it detected and applied the signed CCS against an isolated schema-77 database,
  printed `Signature verified`, reported `conary 0.11.3`, preserved schema 77,
  and then reported itself current. The resulting binary SHA-256 is
  `2007bc379f98ce09c581a99a9fff182b450aa28995449a955cdca2315a281a4c`.
- Released Arch package evidence: the exact manifest-matched package installed
  natively, initialized profile `arch` at schema 77, configured Remi, and
  synchronized 15,429 Arch rows with zero foreign rows. The released resolver
  planned zero installs, five adoptions, and zero blocked or unresolved
  dependencies; it satisfied the exact versioned `libcap.so` and
  `libncursesw.so` capabilities, installed and executed `htop 3.5.1-1-arch`,
  then removed its five files and trove. This was a `bwrap`-isolated live-host
  proof, not a pristine Arch VM: the Conary database and mutation target were
  isolated while the real Arch native-package database and runtime evidence
  were exposed read-only, and the host remained untouched.
- Released Fedora-form evidence: the exact manifest-matched RPM was extracted
  into a `minimal-boot-v4` KVM guest initialized at schema 77 with profile
  `fedora-44`; Remi synchronized 76,685 rows. Live exact `libcap.so.2` and
  `libncursesw.so.6` evidence was ELF64, and Conary installed, executed, and
  removed `htop 3.4.1` without installing a `libcap` trove. The guest is
  conaryOS and lacks `rpm` and `dnf`, so this proves Fedora-form metadata and
  live-runtime probing, not a literal stock Fedora native-PM onboarding path.
- Compatible rewritten Remi commit:
  `27ec2eccb6befdf06d9a826b84cc5a6948eff5fb`. It and deployed pre-rewrite
  source commit `c001f8d69b9e8ef34fba39139576a9809800a9a6` have the same tree. The
  deployed binary SHA-256 is
  `c955a24ff6b90f98ba5f20b37e6a67b79bdde199ec0dcbfac0ce78b001d0f485`.
- Prewarmed package set: `curl`, `htop`, `nano`, and `zstd` for Fedora, Ubuntu,
  and Arch. Eleven conversion-version-6 rows are public; Ubuntu `nano` remains
  correctly fail-closed as `private-review`.
- Deployment evidence: exact-tag run `29630694438` deployed both checked sites
  successfully. Full Remi health passed 10/10, all six checked public routes
  returned HTTP 200, and the deployed pages loaded the exact `v0.11.3` site
  chunks. The self-update API CCS matched the released CCS SHA-256, size
  `16183776`, and detached signature.
- Repository-community closeout: the
  [Welcome Discussion](https://github.com/ConaryLabs/Conary/discussions/36) is
  live, and [issue #35's released-path proof](https://github.com/ConaryLabs/Conary/issues/35#issuecomment-5009942880)
  was recorded before the issue was closed. These repository actions do not
  count as a qualifying external completion or launch the broad outreach loop.
- Organic prelaunch tester evidence: [issue #37](https://github.com/ConaryLabs/Conary/issues/37)
  completed the full `v0.11.3` loop on x86_64 Ubuntu 26.04 LTS and
  [confirmed the DEB checksum](https://github.com/ConaryLabs/Conary/issues/37#issuecomment-5010174050).
  [Issue #38](https://github.com/ConaryLabs/Conary/issues/38) completed the same
  loop on x86_64 Fedora 44 from a never-synced Remi state and
  [confirmed the RPM checksum](https://github.com/ConaryLabs/Conary/issues/38#issuecomment-5010310461).
  Both reports came from the same external tester, so they establish two
  supported-host successes but count once toward the ten-person milestone.
  They do not start the broad-outreach stall clock.
- Prelaunch remediation evidence:
  [issue #41](https://github.com/ConaryLabs/Conary/issues/41) reports that
  `v0.11.3` rejects the legitimate Arch-style `/usr/lib64 -> lib` ancestor while
  validating a Fedora-form CCS payload on Artix. Artix and that cross-distro
  route are outside the supported-host claim, but the in-root symlink
  false-positive is a valid fail-closed-path defect. The attached bundle also
  showed that unprivileged database-backed diagnostics could all fail against
  the root-owned installed database. Both are `fix-now` launch blockers. The
  repaired bundle records the host profile, source pin, and complete repository
  set so the Fedora source route can be classified from evidence instead of
  assumption.
- Supported hosts: Fedora 44, Ubuntu 26.04 LTS, and Arch Linux on the release's
  supported architecture and compatibility baseline.
- Rescheduling sequence, using the venue-specific copy in
  `docs/operations/external-tester-outreach.md`:

  | Venue | New timestamp | State |
  | --- | --- | --- |
  | Show HN | TBD | postponed; the 2026-07-20 slot passed without a post |
  | r/codex | TBD | postponed; the 2026-07-21 slot passed without a post; re-check rules and posting eligibility |
  | r/ClaudeAI | TBD | postponed; the 2026-07-22 slot passed without a post; requires current showcase eligibility and posting-account karma over 50 |

- Actual post URLs and launch timestamps: not launched. Release and site
  evidence for `v0.11.3` is complete, but the former dates are retired and
  outreach is postponed. Assign replacement dates only after the fix-now
  remediation is published and verified on supported paths, its release claims
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
| 1a | 2026-07-18 | [issue #37](https://github.com/ConaryLabs/Conary/issues/37) | Ubuntu 26.04 LTS, x86_64, VM/snapshot/non-critical host confirmed | `v0.11.3` DEB; checksum confirmed | yes, 11/11 steps | No functional failure; public-report verbosity and terminal-control-sequence guidance routed through [issue #39](https://github.com/ConaryLabs/Conary/issues/39) | `validated-no-action` | maintainer |
| 1b | 2026-07-18 | [issue #38](https://github.com/ConaryLabs/Conary/issues/38) | Fedora 44, x86_64, VM/snapshot/non-critical host confirmed | `v0.11.3` RPM; checksum confirmed | yes, 11/11 steps | No functional failure; clean never-synced Remi start; same reporting-guidance follow-up as 1a | `validated-no-action` | maintainer |

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
