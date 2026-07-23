---
last_updated: 2026-07-23
revision: 10
summary: Pin the agent-supervised external tester loop to v0.12.0
---

# Agent-Assisted Tester Loop

This guide is for a person using a coding agent, such as Claude Code or Codex,
to help test the first Conary external preview loop. The agent should assist
with checks, commands, transcript capture, and issue drafting. The human tester
stays responsible for approving every command that mutates system state.

Use a disposable VM, a snapshot, or a non-critical host. Do not run this loop
first on an irreplaceable daily driver.

This guide is pinned to `v0.12.0`. Do not begin the loop unless that release
page publishes the package for this host plus `SHA256SUMS`, and do not continue
when the downloaded package fails checksum verification.

## Copy-Paste Agent Prompt

Paste this into the agent running inside the VM or snapshot:

```text
I want you to help me run the Conary first external tester loop on this host.

Read this guide first:
https://github.com/ConaryLabs/Conary/blob/v0.12.0/docs/guides/agent-assisted-tester-loop.md

Goal: validate the pinned Conary v0.12.0 package-manager preview loop and draft
a beta feedback issue. You are testing the user-facing package-manager flow,
not developing Conary itself.

Safety rules:
- Stop immediately if this is not a VM, snapshot, or explicitly non-critical
  host.
- Confirm distro, architecture, kernel, and sudo before installing anything.
- Use only the pinned v0.12.0 release from
  https://github.com/ConaryLabs/Conary/releases/tag/v0.12.0 and confirm its
  release page provides the package for this host plus SHA256SUMS.
- Verify SHA256SUMS for every downloaded artifact before installation.
- Run dry-run commands before live commands when the loop provides both.
- Ask me before every non-dry-run command that mutates system state.
- If a command output is ambiguous, surprising, or scary, stop and ask.
- Keep a local transcript of commands, exit statuses, and notable output. The
  public feedback issue should summarize this transcript, not reproduce it.
- Do not upload logs, bundles, private keys, tokens, shell history, raw
  environment dumps, or Conary databases.
- At the end, draft a GitHub beta feedback issue using
  https://github.com/ConaryLabs/Conary/issues/new?template=beta_feedback.md.
```

If the agent is running from a Conary checkout, the same guide is also at
`docs/guides/agent-assisted-tester-loop.md`.

## Stop Conditions

Stop and do not install Conary if any of these are true:

- the human has not explicitly confirmed the host is disposable, snapshotted,
  or non-critical;
- the host is not Fedora 44, Ubuntu 26.04 LTS, or Arch Linux;
- the host is not `x86_64`;
- `sudo -v` fails;
- the pinned `v0.12.0` release page does not provide the package for this host
  plus `SHA256SUMS`;
- downloaded artifact checksums do not match `SHA256SUMS`;
- the agent or human cannot explain what a live mutation command is about to
  change.

Stop after installing and file a partial report if any live command fails in a
way the human cannot safely resolve. Try to run the unadopt dry-run before
stopping when that is safe.

## Preflight

Run these read-only checks:

```bash
cat /etc/os-release
uname -m
uname -r
sudo -v
```

Expected:

- Fedora 44, Ubuntu 26.04 LTS, or Arch Linux
- `x86_64`
- a stock distribution kernel
- working `sudo`

The basic package loop does not require composefs, UEFI, or special boot-stack
support. Those only matter for generation-model features outside this test.

## Download And Verify

Create a clean work directory:

```bash
mkdir -p "$HOME/conary-preview-v0.12.0"
cd "$HOME/conary-preview-v0.12.0"
```

Download `SHA256SUMS` and the package for the current distro from:

```text
https://github.com/ConaryLabs/Conary/releases/tag/v0.12.0
```

Use exactly one package. Fedora 44:

```bash
base="https://github.com/ConaryLabs/Conary/releases/download/v0.12.0"
curl -fLO "$base/SHA256SUMS"
curl -fLO "$base/conary-0.12.0-1.fc44.x86_64.rpm"
```

Ubuntu 26.04 LTS:

```bash
base="https://github.com/ConaryLabs/Conary/releases/download/v0.12.0"
curl -fLO "$base/SHA256SUMS"
curl -fLO "$base/conary_0.12.0-1_amd64.deb"
```

Arch Linux:

```bash
base="https://github.com/ConaryLabs/Conary/releases/download/v0.12.0"
curl -fLO "$base/SHA256SUMS"
curl -fLO "$base/conary-0.12.0-1-x86_64.pkg.tar.zst"
```

Downloaded package names:

- Fedora 44: `conary-0.12.0-1.fc44.x86_64.rpm`
- Ubuntu 26.04 LTS: `conary_0.12.0-1_amd64.deb`
- Arch Linux: `conary-0.12.0-1-x86_64.pkg.tar.zst`

Verify the downloaded package against `SHA256SUMS`:

```bash
sha256sum -c SHA256SUMS --ignore-missing
```

Expected: the downloaded package prints `OK`.

## Install Conary

Ask the human before running the matching install command.

Fedora 44:

```bash
sudo dnf install ./conary-0.12.0-1.fc44.x86_64.rpm
```

Ubuntu 26.04 LTS:

```bash
sudo apt install ./conary_0.12.0-1_amd64.deb
```

Arch Linux:

```bash
sudo pacman -U ./conary-0.12.0-1-x86_64.pkg.tar.zst
```

Then record:

```bash
conary --version
conary --help | sed -n '1,80p'
```

The package post-install step initializes the system database with the exact
host profile and configures Remi plus only that profile's native repositories.
Do not run `system init` or add a second Remi repository. Inspect and sync the
configured preview source instead:

```bash
sudo conary repo list
sudo conary repo sync remi
```

## Run The Tester Loop

Run each command in order. Ask the human before every command that is not a
dry-run and can mutate system state.

```bash
sudo conary install htop --dry-run --allow-capabilities
sudo conary install htop --yes --allow-capabilities
sudo conary system adopt --system --dry-run
sudo conary system adopt --system
sudo conary list
sudo conary search htop
sudo conary update --dry-run
sudo conary system unadopt --all --dry-run
sudo conary system unadopt --all --yes
```

For `htop`, `--allow-capabilities` explicitly approves the package-declared
capability; it is not blanket permission for later packages. Review the
dry-run output first, then ask the human before the live `--yes` command.

During the run, capture:

- command text;
- exit status;
- distro and kernel;
- whether the full loop completed;
- where a partial run stopped;
- anything confusing, slow, scary, or unexpectedly pleasant.

## Report Feedback

Open a beta feedback issue:

```text
https://github.com/ConaryLabs/Conary/issues/new?template=beta_feedback.md
```

Fill in:

- **Preview Lane:** check "First external tester loop";
- **Completed the full loop:** `yes`, `no`, or `partial`;
- **Distribution:** Fedora 44, Ubuntu 26.04 LTS, or Arch Linux;
- **Kernel version:** output of `uname -r`;
- **Conary version or commit:** output of `conary --version`;
- **VM/snapshot/non-critical host:** `yes` or `no`;
- **Commands Run:** exact commands from the transcript;
- **What Happened:** short notes about results and friction.

Keep the public issue concise. Include exit statuses and only the output needed
to explain a failure, surprise, or useful result. Do not paste the complete
local transcript, a full installed-package inventory, or broad environment
output. Strip terminal color and control sequences before including excerpts.

Only attach a support bundle if it would help explain a failure and you are
running from a checkout. Review it first:

```bash
sudo -v
bash scripts/conary-support-bundle.sh target/conary-support-bundle
```

On an installed host, the script uses the cached authorization only for
allowlisted database-backed diagnostics and stops before writing if it is not
available. Do not attach private keys, tokens, SSH keys, shell history, raw
environment dumps, `/etc/conary/trust`, raw logs, package payloads, or live
`conary.db` files unless a maintainer explicitly asks for a separately reviewed
follow-up.
