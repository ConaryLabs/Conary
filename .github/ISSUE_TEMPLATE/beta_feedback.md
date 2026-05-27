---
name: Beta Feedback
about: Share limited-preview validation results, rough edges, or tester feedback
title: '[beta] '
labels: beta-feedback
assignees: ''
---

## Preview Lane

- [ ] Adoption/unadoption
- [ ] Conary-owned install/remove/update
- [ ] Selected-generation native handoff
- [ ] Generation export
- [ ] Remi conversion
- [ ] conaryd local daemon

## Environment

- **Distribution**: (Fedora 44, Ubuntu 26.04 LTS, Arch Linux, or other)
- **Kernel version**: (output of `uname -r`)
- **Conary version or commit**: (output of `conary --version`, release tag, or commit SHA)
- **VM/snapshot/non-critical host**: yes/no

## Commands Run

```bash
# Paste the exact commands you ran.
```

## What Happened

Describe the result, including anything confusing, slow, surprising, or good.

## Support Bundle

Run this from the checkout when it would help maintainers understand the host
state:

```bash
bash scripts/conary-support-bundle.sh target/conary-support-bundle
```

- **Support bundle reviewed before attach**: yes/no

Review the bundle before attaching it. The script is allowlist-only and does
not copy `conary.db`, raw logs, environment dumps, shell history, private keys,
SSH keys, `/etc/conary/trust`, host-local access notes, or package payloads.
Do not attach any of those unless a maintainer explicitly asks for a separately
reviewed follow-up.
