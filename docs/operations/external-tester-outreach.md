---
last_updated: 2026-07-17
status: blocked
summary: Gated draft copy for the first external tester launch
---

# External Tester Outreach Draft

> **DO NOT PUBLISH:** W2 must first record and repin the post-integration
> `v0.11.0` artifact publication and installed-binary self-update proof. The
> compatible Remi commit, prewarmed package set, and clean-host smoke are
> recorded, but they are not launch approval by themselves.

Automation may prepare and verify this copy, but a maintainer decides when and
where to post it. Adapt the title and tone to the chosen venue without
weakening the scope or safety caveats.

## Title Options

- Looking for a few Linux package-manager testers for Conary
- Conary: a reversible package-manager preview for Fedora, Ubuntu, and Arch VMs
- Try a bounded Conary package-manager loop in a disposable Linux VM

## Draft

I am looking for a small number of testers for Conary, a Rust package manager
and Linux system manager I have been building.

This is an independent project, not a resurrection or continuation of the old
rPath Conary codebase. It is not affiliated with, endorsed by, or maintained
by rPath, SAS, or the original Conary developers.

Conary has broader generation, package-format, and repository work, but this
ask is intentionally narrow: can its existing-system package-manager loop feel
safe and unsurprising on a supported Fedora, Ubuntu, or Arch system?

The tested preview targets are Fedora 44, Ubuntu 26.04 LTS, and Arch Linux. The
pinned release is `v0.11.0`, but this draft stays blocked until its artifacts
and installed-binary self-update path are verified. At launch, use only the
exact pinned release and verification instructions in the release artifact
matrix.

Please start with a VM, snapshot, spare system, or other non-critical host. Do
not test first on an irreplaceable daily driver.

The bounded flow is:

```bash
conary install htop --dry-run
conary install htop --yes
conary system adopt --system --dry-run
conary system adopt --system --yes
conary list
conary search htop
conary update --dry-run
conary system unadopt --all --dry-run
conary system unadopt --all --yes
```

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

The feedback that matters most:

- Does adoption feel understandable or risky?
- Are command names, dry runs, and warnings honest?
- Which everyday `dnf`, `apt`, or `pacman` operation is missing first?
- Did anything appear to succeed while doing nothing?
- Where does the reversal story remain weak?

The local CLI path is the preview surface. conaryd fleet behavior, federation,
and generation-carrier export are not part of this tester ask. Sharp criticism
and failed attempts are useful evidence.

## Launch Checklist

- Replace every release reference with the exact W2 release.
- Link the release artifact matrix and checksum/signature instructions.
- Pin the compatible Remi commit and prewarmed package set.
- Link the compatibility checklist, tester guide, and beta-feedback template.
- Record the clean-host smoke and rollback evidence.
- Record the maintainer-chosen venue and launch timestamp in the milestone
  tracker.

## Closeout

After W3 records the durable venue, date, pinned release, and tester findings in
the milestone tracker, release history, and detailed roadmap, delete this
draft. Published copy remains at its venue; the repository does not retain a
permanent post archive.
