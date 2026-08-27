---
last_updated: 2026-08-27
revision: 10
status: postponed
target_release: unassigned
summary: Postponed multi-venue launch packet for the first cross-distro tester loop
---

# External Tester Launch Packet

> **POSTPONED; NO NEW DATE IS ASSIGNED:** the current qualifying milestone is
> 0/10. The two earlier supported-host reports remain useful adoption and
> onboarding evidence, but neither installed a source package whose format
> differed from the host's native format. Release `v0.16.1` is published and
> artifact-verified and W7's #110 ordinary-package gate passed. The canonical
> [launch status](../roadmaps/launch-status.json) assigns no tester release
> while its engineering gates remain open. Do not publish the copy below until
> that status assigns an exact release and the external cached-history,
> guided-pilot, and venue-eligibility gates are closed.

The venue copy below references the current `v0.16.1` release as an
unpublishable draft, not as assigned tester authority. The maintainer re-pins
the exact current release if necessary and assigns fresh dates only after every
remaining gate closes, then posts manually and remains available to answer
comments.

## Show HN Submission

- **Title:** `Show HN: Conary - install RPM, DEB, and Arch packages across distros`
- **URL:** `https://github.com/ConaryLabs/Conary`
- **Reschedule state:** TBD after external launch-gate clearance

Submit the repository URL, then add the following opening comment.

```text
I've been building Conary, a Rust package manager and Linux system manager.
This is an independent project, not a resurrection or continuation of the old
rPath Conary codebase, and it is not affiliated with rPath, SAS, or the
original Conary developers.

The first thing I want people to test is straightforward to describe: on a
supported Fedora, Ubuntu, or Arch host, choose a package from one of the other
package ecosystems and let Conary install, query, update-preview, and remove it.

This is not filename repacking or handing authority back to dnf, apt, or
pacman. The source package keeps its native version, dependency, payload,
configuration, and lifecycle semantics. The target system offers explicit
typed capabilities. Conary resolves the graph and owns the transaction,
rollback, and installed state. Unsupported contracts fail as exact contracts;
package-name guesses, script string matching, and a manual review queue are not
runtime authority.

The pinned limited preview supports x86_64 Fedora 44, Ubuntu 26.04 LTS, and
Arch Linux. Please use a disposable VM, snapshot, spare system, or other
non-critical host, not an irreplaceable daily driver.

Choose a source whose package format differs from the host:

  source=ubuntu-26.04  # Fedora/Arch hosts; use fedora-44 on Ubuntu
  sudo conary repo list
  sudo conary repo sync remi
  sudo conary install htop --from "$source" --dry-run
  sudo conary install htop --from "$source" --yes
  sudo conary list htop --info
  sudo conary query depends htop
  sudo conary update htop --dry-run
  sudo conary remove htop --yes

Review the dry-run first and ask before each live mutation. Conary validates
htop's exact typed declaration against the target during preflight and applies
the executor enforcement contract automatically. Unsupported requirements fail
before mutation; there is no capability-approval bypass.

Agent-assisted walkthrough, including download and checksum verification:
https://github.com/ConaryLabs/Conary/blob/main/docs/guides/agent-assisted-tester-loop.md

Pinned release:
https://github.com/ConaryLabs/Conary/releases/tag/v0.16.1

Privacy-safe feedback form:
https://github.com/ConaryLabs/Conary/issues/new?template=pre_alpha_feedback.md

Adoption still exists as a migration path for systems already owned by another
package manager, but it is not this test. The feedback I care about here is
whether cross-distro install actually works end to end, which exact source
contract fails when it does not, whether the dry-run explains the transaction,
and whether update/remove preserve the source package's semantics. Failed and
partial attempts are useful too.
```

## r/codex Follow-Up

- **Title:** `I used Codex to build a cross-distro Rust package manager; now I need hostile testers`
- **Reschedule state:** TBD after the Show HN slot is assigned
- **Pre-post check:** confirm the account can submit, re-read the current
  rules, and select the required flair. Post manually; do not use a bot.

```text
I've been using Codex as one of the coding and review agents while building
Conary, a free, MIT-licensed Rust package manager and Linux system manager.

This is a real multi-crate systems project. AGENTS.md defines the repository
contract, a path router identifies each subsystem's owner and proof, and the
human retains scope and live-mutation approval. I used agents for repository
orientation, bounded implementation, architecture audits, documentation truth,
and verification-backed closeout.

The product test is cross-distro package installation: install a Debian package
on Fedora or Arch, or an RPM package on Ubuntu, while preserving the source
package's typed dependency, configuration, and lifecycle contracts. Conary,
not the source package manager, owns the transaction and rollback.

The pinned guide asks an agent to preflight a disposable supported VM, verify
the release checksum, select a source format different from the host format,
explain the complete dry-run, ask before live mutations, keep a private
transcript, and draft privacy-safe feedback:
https://github.com/ConaryLabs/Conary/blob/main/docs/guides/agent-assisted-tester-loop.md

Repository:
https://github.com/ConaryLabs/Conary

Pinned release:
https://github.com/ConaryLabs/Conary/releases/tag/v0.16.1

I'm interested both in product defects and in whether repo-owned instructions
make Codex useful as a supervised systems-test operator. Exact failures,
partial attempts, and unpleasant surprises are useful evidence.
```

## r/ClaudeAI Follow-Up

- **Title:** `Built with Claude Code: test cross-distro package installs in a VM`
- **Reschedule state:** TBD after the r/codex slot is assigned
- **Pre-post check:** re-read the current showcase rules, confirm the account
  remains eligible, and select the required flair.

```text
I built Conary, a Rust package manager and Linux system manager, with Claude
Code as one of the coding and review agents. It is MIT-licensed, has no paid
tier, and requires no sign-up.

Claude Code worked from a repo-owned contract: CLAUDE.md imports AGENTS.md, and
a path router points the agent to the subsystem owner, safety invariants,
focused tests, and interaction gate. The human remained responsible for scope
and approval of live or destructive commands.

The bounded product test installs a package from a different distro ecosystem:
Debian on Fedora or Arch, or RPM on Ubuntu. Conary preserves the source
package's typed version, dependency, payload, configuration, and lifecycle
contracts while owning the transaction and rollback on the target.

The guide tells Claude Code to confirm a disposable supported VM, verify the
pinned release checksum, inspect the complete dry-run, ask before every live
mutation, keep a private transcript, and draft privacy-safe feedback:
https://github.com/ConaryLabs/Conary/blob/main/docs/guides/agent-assisted-tester-loop.md

Repository and pinned release:
https://github.com/ConaryLabs/Conary
https://github.com/ConaryLabs/Conary/releases/tag/v0.16.1

The useful feedback is where a source-native contract fails, whether Conary
explains it precisely, whether install/update/remove behave consistently, and
whether the agent instructions help without hiding what changes on the host.
```

## Scope And Safety Notes

The qualifying loop requires a source package format different from the host's
native format. Conary owns packages it installs. Native package-manager
adoption and explicit takeover remain migration features, not part of this
tester ask.

When finished or blocked, file privacy-safe feedback using the pre-alpha tester-feedback
template. Record whether the full loop completed, the distro, source and host
formats, pinned release, exact failing command, and exact contract error.
Review any support bundle before attaching it. Do not include credentials,
private keys, host-local secret files, broad environment dumps, or a live
Conary database.

The local CLI path is the preview surface. conaryd fleet behavior, federation,
and generation-carrier export are outside this tester ask. Sharp criticism and
failed attempts are useful evidence.

## Launch Checklist

- [x] Reset the qualifying milestone to 0/10 and preserve the former adoption
  reports as non-qualifying historical evidence.
- [x] Rewrite the venue copy around the cross-distro package loop.
- [x] Retire the passed 2026-07-20 through 2026-07-22 dates.
- [x] Publish immutable `v0.16.1` with RPM, DEB, Arch, CCS, checksums, and the
  required signature and installed-binary evidence.
- [x] Record exact cross-distro install/query/update-preview/remove proof for
  the released binary on supported hosts.
- [x] Deploy the exact release sites and independently verify live status and
  body claims.
- [x] Update the release artifact matrix and milestone tracker with exact
  release, deployment, and Remi population evidence.
- [x] Publish and independently verify the synchronized `v0.16.1` suite that
  contains the supported-host fixes, then replace every release version and
  URL in this draft.
- [x] Pass #110's ordinary-package corpus gate through PR #487 without treating
  release proof as corpus proof.
- [ ] Activate #598's signed zero-exclusion public universe behind #638's typed
  read surface; complete the #122/#534/#132/#642/#643 daily-driver floor,
  #639's synchronized release, and #121/#149 launch proof; then assign the
  re-proven suite as exact tester authority.
- [ ] Obtain GitHub Support confirmation that cached pre-rewrite pull-request
  and commit views have been dereferenced.
- [ ] Re-check each venue's current rules and account eligibility immediately
  before posting.
- [ ] Complete a staggered guided pilot with five unaffiliated qualifying
  testers; record intervention count and the other milestone measures, with
  zero live maintainer intervention as the target by tester five.
- [ ] Assign a staggered schedule only after every preceding gate passes.
- [ ] After launch, record every actual post URL and timestamp.

## Closeout

After the milestone tracker records the durable venue, date, pinned release,
and tester findings, move any stable product truth to its canonical owner and
delete this draft. Published copy remains at its venue; Git history retains
the planning record.
