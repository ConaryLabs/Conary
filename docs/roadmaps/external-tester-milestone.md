---
last_updated: 2026-07-27
revision: 5
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

The pinned tester release is the verified immutable `v0.13.0` recorded in
`docs/operations/release-artifact-matrix.md`. Its release gate is complete:

- annotated tag object `f8298522fd7fe95a4994184ae20c34cf64096818`
  peels to `6f1429c362ac161f1ef817233e72ee9c9a031c11`;
- release-build run `30261256730` published the exact seven-asset suite, and
  independent downloads matched all five checksum entries and every GitHub
  digest;
- the detached CCS signature, current 0.13 binary identity, signed forced
  self-update, deployed release directory, and public self-update endpoint
  agree;
- the schema hard cut is explicit: install the 0.13 native package fresh
  instead of attempting to self-update the incompatible 0.12 CCS parser;
- deploy-and-proof run `30263948968` installed the immutable RPM, DEB, and
  Arch packages natively and passed the Cartesian lifecycle on Fedora 44,
  Ubuntu 26.04 LTS, and Arch;
- exact-tag Conary sites serve 0.13.0 with a real branded 404, and compatible
  `remi-v0.8.5` production passes full health with all five sources populated
  and real conversions for every public profile;
- signature, SBOM, and provenance statuses are explicit in the release matrix.

Release proof is not an external-user completion. The result therefore remains
0/10, and broad outreach remains postponed by the separate cached-history and
venue-eligibility gates.

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
