# Limited Preview Subreddit Tester Post - 2026-05-19

This is draft copy for a narrow public tester ask. It is not release policy and
should be revised for the norms of whichever subreddit receives it.

## Title Options

- Looking for a few Linux package-manager testers for Conary
- Conary: a reversible package-manager preview for Fedora, Ubuntu, and Arch VMs
- I am building a Rust package manager that can adopt/unadopt existing Linux systems

## Draft

I am looking for a small number of testers for Conary, a Rust package manager and
Linux system manager I have been building.

The part I want feedback on right now is deliberately narrow: can Conary feel
safe and unsurprising as a package manager on an existing Fedora, Ubuntu, or Arch
system?

Conary can install/remove/update packages it owns, adopt packages already owned
by the native package manager, and unadopt them again so the native package
manager remains the authority. The risk-reduction idea is that trying Conary
should not require permanently replacing dnf, apt, or pacman.

Tested preview targets:

- Fedora 44
- Ubuntu 26.04 LTS
- Arch Linux

What I would love people to try in a VM, snapshot, or spare system:

```bash
conary --allow-live-system-mutation system adopt --system --full
conary list
conary search <package>
conary update --dry-run
conary --allow-live-system-mutation install <small-package> --yes
conary --allow-live-system-mutation remove <small-package> --yes
conary --allow-live-system-mutation system unadopt --all
```

Please do not test this first on an irreplaceable daily driver. This is preview
software, and package-manager work touches real system state.

A few important caveats:

- The local CLI package-manager flow is the preview surface.
- dnf, apt, and pacman remain authoritative for adopted packages unless you
  explicitly choose takeover behavior.
- `system unadopt --all` is the escape hatch before selecting a Conary
  generation.
- After selecting a Conary generation, use `system native-handoff --dry-run`,
  then `system native-handoff --yes`; if interrupted, resume with
  `system native-handoff --recover --yes`.
- Local `conaryd` package mutation routes now queue daemon jobs, but CLI flows
  remain the simplest preview path and non-dry-run daemon jobs require the same
  explicit live-host mutation acknowledgement.
- x86_64 ISO generation-carrier export exists, but it is not the thing I am
  asking people to evaluate here.
- Selected-generation native handoff preserves native package files and native
  package-manager databases, but it does not import native transaction history
  or silently take over adopted packages.

The feedback I care about most:

- Does adoption feel understandable or scary?
- Are the command names and warning messages honest?
- Which everyday dnf/apt/pacman commands do you miss immediately?
- Did anything appear to work while actually doing nothing?
- Where does the risk-reversal story still feel weak?

Repo and docs:

- https://github.com/ConaryLabs/Conary
- https://conary.io

I am happy to hear sharp criticism. The goal of this preview is to find the
rough edges before pretending the tool is more ready than it is.
