---
last_updated: 2026-07-25
revision: 3
status: active
current_result: 0/10
summary: Outcome tracker for Conary's first cross-distro external tester milestone
---

# First External Tester Milestone

The milestone closes when ten unique people outside the existing project
circle install and remove a package whose source format differs from the
supported host's native format and report friction, or when a maintainer
records an evidence-backed pivot for a reproducible systemic blocker. Interest,
downloads, partial attempts, and repeated runs by the same person do not count
as separate completions.

**Current result: 0/10 qualifying cross-distro completions.** The previous two
supported-host reports from one person remain valid adoption/onboarding
evidence, but neither crossed a source-package/host-package format boundary.

## Qualifying Flow

Each qualifying report covers this sequence on one supported host:

```text
foreign artifact install -> list/query -> update --dry-run -> remove
```

The report must confirm that the tester used the pinned release, stayed within
the supported host scope, named both the source package format and host native
format, and completed every stage. A failed attempt remains useful evidence but
does not count as a completion.

## Launch Record

- Pinned Conary release: immutable `v0.12.0`, published at
  `2026-07-23T21:39:20Z`. Annotated tag object
  `8411169b40d8523ee716518cb3dc3e51acddb019` peels to commit
  `eb256b19b4f04ca1d03b6af39a2819d746d3a22a`. Remediation merge CI
  `30041401268` and exact release-commit CI `30042990554` passed 11/11 jobs;
  release-build `30043930486` and exact-tag deploy-and-verify `30047027525`
  passed.
- Release payload evidence: all seven assets are published. Independent
  downloads matched every manifest payload and every GitHub REST digest:

  | Manifest payload | SHA-256 |
  | --- | --- |
  | `conary-0.12.0-1-x86_64.pkg.tar.zst` | `60cb2a4bfc804e7d2f80950b2b60902643c0d33bc1df63fc66d7e5c389bf2256` |
  | `conary-0.12.0-1.fc44.x86_64.rpm` | `23c2946e68b124092f0d5f32e573b8f6b2ad9695611052a04b4cdf4d7937cc3c` |
  | `conary-0.12.0.ccs` | `c973fb654b67da0619d6837b34e2f5f78bbea90dfd9fb8de19b6edf9cbe9582a` |
  | `conary_0.12.0-1_amd64.deb` | `0c2b2ea1a42753cf398b9119fc85b28c757cc7ec4ecc1dc1b328e737729e65a1` |
  | `metadata.json` | `11337f7d9e0ee5abdc270c21259be90a8dffeaa676c5b6d5fa3ec376f7572231` |

  The official preceding binary verified the detached CCS signature. No SBOM
  or provenance sidecars are published or planned for this limited preview;
  their absence remains an explicit caveat.
- Installed-binary evidence: the official preceding-preview binary initialized
  an isolated schema-77/profile-`arch` database, verified the signed update,
  replaced itself with `v0.12.0`, preserved schema 77, and then reported
  `Already up to date (v0.12.0)`. The updated binary SHA-256 is
  `5f790b11d8137f293cbf53fce210af17e7ed8d1d6648f1075ecb621a7283be9e`.
- Supported Arch evidence: the exact manifest-matched Arch package binary
  initialized schema 77/profile `arch` and synchronized 15,462 Remi rows with
  zero foreign-distro rows. It planned, installed, and executed
  `htop 3.5.2-1-arch` with five dependencies and five files, then removed all
  five files and the trove. A shipped-binary package probe installed
  `/usr/lib64/safe-proof.txt` through the real target symlink
  `/usr/lib64 -> lib`, recorded `/usr/lib/safe-proof.txt`, removed it cleanly,
  and preserved the symlink. A paired out-of-root symlink probe failed with a
  path-safety error, wrote nothing outside the root, and recorded no trove.
  Host pacman inventory, linker cache, installed Conary binary, and host
  `/usr/lib64` stayed unchanged.
- Arch caveat: this proof used the exact released package binary in an
  isolated writable root with the current Arch host's pacman evidence
  read-only. It was not a native `pacman -U` into the host or a pristine VM;
  the two path probes used locally built unsigned one-file CCS packages so the
  shipped installer could exercise the exact safe and escaping ancestors.
- Fedora-form evidence was not rerun for `v0.12.0`; the preceding conaryOS
  `minimal-boot-v4` proof remains a regression baseline, not literal stock
  Fedora native-PM onboarding.
- Compatible rewritten Remi commit:
  `27ec2eccb6befdf06d9a826b84cc5a6948eff5fb`. It and deployed pre-rewrite
  source commit `c001f8d69b9e8ef34fba39139576a9809800a9a6` have the same tree. The
  deployed binary SHA-256 is
  `c955a24ff6b90f98ba5f20b37e6a67b79bdde199ec0dcbfac0ce78b001d0f485`.
- Pinned `v0.12.0` prewarm set: `curl`, `htop`, `nano`, and `zstd` for Fedora,
  Ubuntu, and Arch. Eleven conversion-version-6 rows were public; Ubuntu
  `nano` was held in the now-retired `private-review` workflow. That is
  historical release evidence, not the current source contract. The
  current-only typed lifecycle path needs fresh deployment proof before broad
  outreach resumes.
- Deployment evidence: exact-tag run `30047027525` deployed the Conary bundle.
  Full Remi health passed 10/10, all six checked public routes returned HTTP
  200, and the checked Conary pages carried `v0.12.0`. The self-update API CCS
  matched release hash
  `c973fb654b67da0619d6837b34e2f5f78bbea90dfd9fb8de19b6edf9cbe9582a`,
  size `16183371`, and detached signature.
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
  ancestor while validating a Fedora-form CCS payload on Artix. Artix remains
  outside the supported-host claim, but cross-distro package installation is
  now the product contract and the in-root symlink false-positive is a valid
  path-safety defect. Immutable
  `v0.12.0` carries the repair, and the shipped-binary safe/escape probes above
  close its supported Arch-path gate. Support-bundle self-tests passed, and an
  isolated schema-77 bundle recorded integrity, table, repository, and
  source-selection summaries without including the database. This proof host had
  no installed `/var/lib/conary/conary.db`, so the successful root-owned
  cached-sudo path remains regression-tested; affected reporters must run
  `sudo -v` before collecting a fresh reviewed bundle.
- Supported hosts: Fedora 44, Ubuntu 26.04 LTS, and Arch Linux on the release's
  supported architecture and compatibility baseline.
- Rescheduling sequence, using the venue-specific copy in
  `docs/operations/external-tester-outreach.md`:

  | Venue | New timestamp | State |
  | --- | --- | --- |
  | Show HN | TBD | postponed; the 2026-07-20 slot passed without a post |
  | r/codex | TBD | postponed; the 2026-07-21 slot passed without a post; re-check rules and posting eligibility |
  | r/ClaudeAI | TBD | postponed; the 2026-07-22 slot passed without a post; requires current showcase eligibility and posting-account karma over 50 |

- Actual post URLs and launch timestamps: not launched. The release,
  supported-Arch remediation, deployment, and public-claim refresh are
  complete, but the former dates remain retired and outreach stays postponed.
  Assign replacement dates only after GitHub Support dereferences the cached
  pull-request and commit views that still expose pre-rewrite history and the
  per-venue eligibility checks pass. Record each post immediately after
  submission. The three-week stall clock starts from the first actual launch
  timestamp.
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
