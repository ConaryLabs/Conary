---
last_updated: 2026-07-19
status: scheduled
summary: Multi-venue launch packet for the first external tester loop
---

# External Tester Launch Packet

> **SCHEDULED FOR MANUAL LAUNCH:** begin with Show HN on Monday, 2026-07-20 at
> 15:00 CEST (`13:00 UTC`), then use the venue-specific Reddit follow-ups below.
> The W2 compatible-Remi, rollback, prewarm, and clean-host baselines are
> recorded. The `v0.11.2` gate was reopened for `v0.11.3` after the supported
> `htop` path exposed unreachable generic SONAME evidence; review also found an
> inexact or ABI-unchecked critical-library fallback and discarded Arch
> capability constraints. The immutable replacement release is published, and
> its exact tag, workflow runs, hashes, native-package onboarding,
> installed-binary self-update, deployment, and public-site evidence are now
> verified. Do not publish automatically or until GitHub Support has
> dereferenced the cached pull-request and commit views that still expose
> pre-rewrite history and the venue-specific eligibility checks pass. No listed
> external venue post has been published. Organic prelaunch testing has one
> unique qualifying tester across Ubuntu 26.04 LTS and Fedora 44, so the
> milestone tracker is 1/10; this does not start the broad-outreach stall clock.

The maintainer posts this manually and remains available to answer comments.
After submission, record the actual HN URL and timestamp in the milestone
tracker before treating W3 as launched.

## Show HN Submission

- **Title:** `Show HN: Conary - reversible package management for Fedora, Ubuntu, and Arch`
- **URL:** `https://github.com/ConaryLabs/Conary`
- **Planned submission:** Monday, 2026-07-20 at 15:00 CEST (`13:00 UTC`)

The title is 76 characters and keeps the required `Show HN:` prefix. Submit the
repository URL, then add the following as the opening comment.

## Opening Comment

```text
I've been building Conary, a Rust package manager and Linux system manager.
This is an independent project, not a resurrection or continuation of the old
rPath Conary codebase, and it is not affiliated with rPath, SAS, or the original
Conary developers.

The larger project includes generation-style system management, a native CCS
package format, repository tooling, and Remi conversion services. For this Show
HN I want to keep the test deliberately narrow: does an adoption-led package
manager loop feel safe and unsurprising on an existing Linux system?

Conary can track packages already owned by dnf, apt, or pacman without silently
taking authority from the native package manager. You can inspect the result
and then unadopt everything again. The native package manager remains
authoritative unless you explicitly choose takeover.

The current limited preview supports x86_64 Fedora 44, Ubuntu 26.04 LTS, and
Arch Linux. Please use a disposable VM, snapshot, spare system, or other
non-critical host, not an irreplaceable daily driver.

The bounded loop is:

  sudo conary repo sync remi
  sudo conary install htop --dry-run --allow-capabilities
  sudo conary install htop --yes --allow-capabilities
  sudo conary system adopt --system --dry-run
  sudo conary system adopt --system
  sudo conary list
  sudo conary search htop
  sudo conary update --dry-run
  sudo conary system unadopt --all --dry-run
  sudo conary system unadopt --all --yes

For htop, --allow-capabilities explicitly approves its package-declared
capability. Review the dry-run first and run the live install only after the
human tester approves that capability.

The agent-assisted walkthrough, including downloads and checksum verification:
https://github.com/ConaryLabs/Conary/blob/v0.11.3/docs/guides/agent-assisted-tester-loop.md

Host compatibility checklist:
https://github.com/ConaryLabs/Conary/blob/v0.11.3/docs/guides/compatibility-checklist.md

Pinned v0.11.3 release:
https://github.com/ConaryLabs/Conary/releases/tag/v0.11.3

Privacy-safe feedback form:
https://github.com/ConaryLabs/Conary/issues/new?template=beta_feedback.md

The immutable v0.11.3 release publishes RPM, DEB, Arch, and CCS artifacts,
SHA256SUMS, and a detached CCS signature. I independently verified the
checksums, signature, supported package paths, deployment, and installed-binary
self-update. SBOM and provenance sidecars are not published or planned, which
remains an explicit preview caveat.

The feedback I care about most is where adoption feels risky, where dry runs or
warnings are unclear, which everyday package-manager operation is missing
first, and whether anything appears to succeed while doing nothing. Failed or
partial attempts are useful too. I'll be around to answer questions and triage
reports.
```

## r/codex Follow-Up

- **Planned submission:** Tuesday, 2026-07-21 at 15:00 CEST (`13:00 UTC`)
- **Title:** `I used Codex to help build a Rust package manager; now I want agent-assisted testers`
- **Post type:** text post with the closest project or use-case flair available
- **Pre-post check:** confirm the account can submit, re-read the current rules,
  and select the required flair. Post manually; do not use a bot.

```text
I've been using Codex as one of the coding and review agents while building
Conary, a free, MIT-licensed Rust package manager and Linux system manager.

This is a real multi-crate systems project rather than a generated demo. The
workflow I used with Codex is deliberately evidence-heavy: AGENTS.md defines the
repository-wide safety and verification contract, a path router points an agent
to the owning subsystem and focused tests, and the human keeps control of scope
and any live or destructive action. I used Codex for repository orientation,
bounded implementation and review passes, documentation/release truth checks,
and verification-backed closeout.

Conary itself can install packages, track packages already owned by dnf, apt,
or pacman, and unadopt them without silently taking authority from the native
package manager. It also has broader generation, CCS package, and repository
work, but the external test is intentionally limited to the reversible local
package-manager loop.

I'm now trying the same agent contract from the tester side. The guide below
asks Codex to preflight a disposable VM, verify the pinned release checksum,
explain each command, ask the human before every live mutation, keep a
transcript, and draft privacy-safe feedback.

Agent-assisted tester guide:
https://github.com/ConaryLabs/Conary/blob/v0.11.3/docs/guides/agent-assisted-tester-loop.md

Repository:
https://github.com/ConaryLabs/Conary

Pinned v0.11.3 release:
https://github.com/ConaryLabs/Conary/releases/tag/v0.11.3

The supported test hosts are x86_64 Fedora 44, Ubuntu 26.04 LTS, and Arch
Linux. Please use a VM, snapshot, spare system, or other non-critical host. The
immutable v0.11.3 release has independently verified checksums and a verified
detached CCS signature. It does not publish SBOM or provenance sidecars; this
is explicitly an early preview.

I'm interested in two kinds of feedback: where Conary's adoption/reversal flow
feels unclear, and whether the repo-level instructions plus explicit human
approval make Codex useful as a supervised systems-test operator. Failed and
partial attempts are useful too.
```

## r/ClaudeAI Follow-Up

- **Planned submission:** Wednesday, 2026-07-22 at 15:00 CEST (`13:00 UTC`)
- **Title:** `Built with Claude Code: a reversible Linux package-manager preview to test in a VM`
- **Post type:** text post with the `Showcase` flair, if that remains the
  matching flair at submission time
- **Pre-post check:** the posting account must have more than 50 karma. Re-read
  the current showcase rules and confirm the post is still eligible.

```text
I built Conary, a Rust package manager and Linux system manager, with Claude
Code as one of the coding and review agents used during development. It is free
to try, MIT-licensed, has no paid tier, and requires no sign-up.

Claude Code helped me work through a large multi-crate repository using a
repo-owned contract instead of one enormous prompt. CLAUDE.md imports the
shared AGENTS.md rules, and a path router points the agent to the owning
subsystem, its safety invariants, focused tests, and cross-system verification
gate. I used Claude Code for repository navigation, scoped implementation and
review passes, and checking that documentation and release claims matched the
code and test evidence. The human remained responsible for scope and approval
of live or destructive actions.

What Conary does: it installs packages, can track packages already owned by
dnf, apt, or pacman, and can unadopt them again without silently taking package
authority from the native manager. It has broader immutable-generation and
native-package work, but the test I'm launching is deliberately smaller.

I'd like Claude Code users to try the bounded package-manager loop as a
supervised operator. The guide tells Claude Code to check that the host is a
disposable VM or snapshot, verify the pinned release checksum, explain each
step, ask before every live mutation, retain a transcript, and draft
privacy-safe feedback.

Agent-assisted tester guide:
https://github.com/ConaryLabs/Conary/blob/v0.11.3/docs/guides/agent-assisted-tester-loop.md

Repository and source:
https://github.com/ConaryLabs/Conary

Pinned v0.11.3 release:
https://github.com/ConaryLabs/Conary/releases/tag/v0.11.3

The supported hosts are x86_64 Fedora 44, Ubuntu 26.04 LTS, and Arch Linux.
Please use a VM, snapshot, spare system, or other non-critical host. This is an
early preview: v0.11.3 has independently verified checksums and a verified
detached CCS signature, and SBOM and provenance sidecars are not published or
planned.

The useful feedback is both product-level and workflow-level: where adoption
or reversal feels risky, whether dry runs and warnings are clear, and whether
Claude Code's repo instructions and approval boundary make the test easier to
understand without hiding what changes on the machine.
```

## Scope And Safety Notes

Native package managers remain authoritative for adopted packages unless a
user explicitly chooses takeover. Before selecting a Conary generation,
`conary system unadopt --all --yes` is the one-command escape hatch. After a
generation is selected, use `conary system native-handoff --dry-run` and then
`conary system native-handoff --yes`; an interrupted handoff resumes with
`conary system native-handoff --recover --yes`.

When finished or blocked, file privacy-safe feedback using the beta-feedback
template. Record whether the full loop completed, the distro, the pinned
release, the exact failing command, and the refusal or error text. Review the
support bundle before attaching it. Do not include credentials, private keys,
host-local secret files, broad environment dumps, or a live Conary database.

The local CLI path is the preview surface. conaryd fleet behavior, federation,
and generation-carrier export are not part of this tester ask. Sharp criticism
and failed attempts are useful evidence.

## Launch Checklist

- [x] Replace every launch-copy release reference with the published `v0.11.3`
  target and pin public guides to that exact tag.
- [x] Link the release artifact matrix and checksum/signature instructions.
- [x] Pin the compatible Remi commit and prewarmed package set.
- [x] Link the compatibility checklist, tester guide, and beta-feedback template.
- [x] Retain the W2 clean-host and rollback evidence as superseded baselines.
- [x] Record Show HN, r/codex, r/ClaudeAI, and their planned launch timestamps
  in the milestone tracker.
- [x] Publish `v0.11.3` and record its exact tag, workflow runs, checksums,
  detached CCS signature, profile-correct native-package initialization, and
  installed-binary self-update evidence in the milestone tracker and artifact
  matrix.
- [x] Build and deploy the checked `conary.io` and `remi.conary.io` production
  sites for the exact release.
- [ ] Obtain GitHub Support confirmation that cached pre-rewrite pull-request
  and commit views have been dereferenced.
- [ ] Re-check each venue's current rules and account eligibility immediately
  before posting.
- [ ] After launch, record every actual post URL and launch timestamp.

## Closeout

After W3 records the durable venue, date, pinned release, and tester findings in
the milestone tracker, release history, and detailed roadmap, delete this
draft. Published copy remains at its venue; the repository does not retain a
permanent post archive.
