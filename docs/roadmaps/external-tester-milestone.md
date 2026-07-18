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

- Pinned Conary release: immutable `v0.11.2`, published at
  `2026-07-18T02:07:37Z`. Its annotated tag peels to exact commit
  `df8bb13dce759180081f38c6c78506e0a182fcd8`. Release-build run
  `29623998496` attempt 2 and deploy-and-verify run `29626490327` both passed
  at that commit.
- Release payload evidence: the release has seven assets. Independent downloads
  matched all five payload entries in `SHA256SUMS`:

  | Manifest payload | SHA-256 |
  | --- | --- |
  | `conary-0.11.2-1-x86_64.pkg.tar.zst` | `bae694e0fd02acde12ee3fdd8efe7fe31df94fb47c67b480709e26bf8bdff991` |
  | `conary-0.11.2-1.fc44.x86_64.rpm` | `be570c4d6ace9c76f35ac85ce6d5ec07650e5d5be11ad0f3ab90af6ea22fe308` |
  | `conary-0.11.2.ccs` | `d00702da873192b4a5ad8836658f9ea8fe1dba1fa667bccdbd105561dfe5adc0` |
  | `conary_0.11.2-1_amd64.deb` | `b5ea9bd5d0b642b3b4a1fc6371f9563eda44fe5e18a40e4908b1f9fb09b1515f` |
  | `conary-0.11.2.metadata.json` | `37b56b06769a05a90c767b60f6820c75b0fb8d09856cd2380f1448cb8ad882c0` |

  The detached `conary-0.11.2.ccs.sig` verified offline against the published
  CCS with the preceding official binary. `SHA256SUMS` and the detached
  signature are the other two assets. No SBOM or provenance sidecars are
  published for this limited preview.
- Installed-binary baseline: the official `v0.11.1 -> v0.11.2` Fedora RPM
  upgrade path, using the preceding release's binary and
  an isolated schema-77 database, detected the signed `v0.11.2` CCS with hash
  `d00702da873192b4a5ad8836658f9ea8fe1dba1fa667bccdbd105561dfe5adc0`,
  printed `Signature verified`, replaced itself, reported `conary 0.11.2`,
  preserved schema 77, and then reported itself up to date. The resulting
  binary SHA-256 was
  `b075fc4464430bfed1342c6516b1efa193ce94fac014cb288142d2e95e01e227`.
- Compatible rewritten Remi commit:
  `27ec2eccb6befdf06d9a826b84cc5a6948eff5fb`. It and deployed pre-rewrite
  source commit `c001f8d69b9e8ef34fba39139576a9809800a9a6` have the same tree. The
  deployed binary SHA-256 is
  `c955a24ff6b90f98ba5f20b37e6a67b79bdde199ec0dcbfac0ce78b001d0f485`.
- Prewarmed package set: `curl`, `htop`, `nano`, and `zstd` for Fedora, Ubuntu,
  and Arch. Eleven conversion-version-6 rows are public; Ubuntu `nano` remains
  correctly fail-closed as `private-review`.
- Clean-host and onboarding baseline: Fedora 44 installed, executed, and
  removed the public `htop 3.4.1` conversion after the exact public-target
  compatibility fix. The released `v0.11.2` Arch package's actual
  `post_install` initialized schema 77 with `system.host-profile=arch`, added
  only the three Arch native repositories plus Remi, and left native repos
  disabled pending signing trust. The released binary then synchronized Remi
  and recorded 15,423 packages, all with distro `arch`.
- Deployment baseline: deploy-and-verify reported the public self-update API at
  `v0.11.2` with CCS SHA-256
  `d00702da873192b4a5ad8836658f9ea8fe1dba1fa667bccdbd105561dfe5adc0`;
  full Remi health passed 10/10. Checked production builds for both
  `conary.io` and `remi.conary.io` completed without warnings or errors and
  were deployed through the repository helper.
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
  until GitHub Support dereferences the cached pull-request and commit views
  that still expose pre-rewrite history, followed by the per-venue eligibility
  checks. Record each post immediately after submission. The three-week stall
  clock starts from the first actual launch timestamp.
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
