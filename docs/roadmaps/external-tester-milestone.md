---
last_updated: 2026-08-19
revision: 11
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

**Current result: 0/10 qualifying cross-distro completions.** Two earlier
supported-host reports from one person remain valid adoption and onboarding
evidence, but neither crossed a source-package and host-package format
boundary.

## Qualifying Flow

Each qualifying report covers this sequence on one supported host:

```text
foreign artifact install -> list/query -> update --dry-run -> remove
```

The report confirms that the tester used the pinned release, stayed within the
supported host scope, named both the source package format and host native
format, and completed every stage. A failed attempt remains useful evidence
but does not count as a completion.

## Release Gate

The publication gate for synchronized suite `v0.16.1` is complete. The
`docs/operations/release-artifact-matrix.md` records its exact reviewed commit,
annotated tag, immutable 15-asset release across four products, checksums and
GitHub digests, release attestation, detached CCS signature, deployments,
signed bootstrap manifest, build-only routes, and three-distro
released-package proof. Production Remi runs the exact tagged binary.

Protected failed tag `v0.16.0` remains reserved and has no release; it was not
moved or reused when the strictly higher `v0.16.1` suite was published.

That complete release closeout does not by itself open outreach. The
ordinary-package corpus gate owned by #110/W7 must also pass before this
tracker names a pinned tester version. No tester version is assigned in
advance, and `v0.16.1` must not be presented as one merely because it is the
current release authority.

Release proof is not an external-user completion. The result therefore remains
0/10, and broad outreach remains postponed by the W7 corpus gate plus the
separate cached-history and venue-eligibility gates.

Supported tester hosts are x86_64 Fedora 44, Ubuntu 26.04 LTS, and Arch Linux.
Use a disposable VM, snapshot, spare system, or other non-critical host.

## Launch State

No broad-outreach post has been published and no new date is assigned. The
venue copy remains in `docs/operations/external-tester-outreach.md`. Fresh
dates require:

- GitHub Support dereferencing of cached pre-rewrite history;
- current venue-rule and posting-eligibility checks.

Record each actual post URL and timestamp immediately after submission. The
three-week stall clock starts from the first actual launch timestamp, not from
release publication.

## Outcomes

Use an opaque report or issue reference rather than a person's name or host
identity. The triage owner keeps secrets and broad machine dumps out of linked
evidence.

| Attempt | Date | Privacy-safe report | Distro and host scope | Pinned release | Full qualifying flow completed | Friction or failure | Triage status | Triage owner |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1a | 2026-07-18 | [issue #37](https://github.com/ConaryLabs/Conary/issues/37) | Ubuntu 26.04 LTS, x86_64, VM/snapshot/non-critical host confirmed | then-current DEB; checksum confirmed | no; same-format onboarding flow | No functional failure; reporting guidance advanced through issue #39 | `validated-no-action` | maintainer |
| 1b | 2026-07-18 | [issue #38](https://github.com/ConaryLabs/Conary/issues/38) | Fedora 44, x86_64, VM/snapshot/non-critical host confirmed | then-current RPM; checksum confirmed | no; same-format onboarding flow | No functional failure; clean never-synced Remi start; same reporting follow-up as 1a | `validated-no-action` | maintainer |

Attempts 1a and 1b came from the same person and contribute zero qualifying
cross-distro completions. Triage statuses are `fix-now`, `next-slice`,
`validated-no-action`, and `declined-with-reason`. Qualifying and failed
attempts both require an owner and a triage disposition.

The public feedback path is the pre-alpha tester-feedback issue template plus a
reviewed support bundle. Never request secrets, credential files, private keys,
broad environment dumps, or a live database by default.

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
