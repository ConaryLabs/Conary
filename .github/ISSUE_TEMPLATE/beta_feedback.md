---
name: Beta Feedback
about: Share limited-preview validation results, rough edges, or tester feedback
title: '[beta] '
labels: beta-feedback
assignees: ''
---

## Preview Lane

- [ ] First external tester loop (install -> adopt -> list/search -> update --dry-run -> unadopt)
- [ ] Adoption/unadoption
- [ ] Conary-owned install/remove/update
- [ ] Selected-generation native handoff
- [ ] Generation export
- [ ] Remi conversion
- [ ] conaryd local daemon

## First External Tester Loop

Fill this in if you ran the tester loop from the preview post.

- **Completed the full loop (install -> adopt -> list/search -> update --dry-run -> unadopt)**: yes / no / partial
- **If partial or no -- where did it stop, and what did you see?**:

## Environment

- **Distribution**: (Fedora 44, Ubuntu 26.04 LTS, Arch Linux, or other)
- **Kernel version**: (output of `uname -r`)
- **Architecture**: (output of `uname -m`)
- **Conary version or commit**: (output of `conary --version` or commit SHA)
- **Release tag and package**: (for example, `v0.12.0` and the exact RPM/DEB/Arch package name)
- **Package checksum verified**: yes/no
- **VM/snapshot/non-critical host**: yes/no

## Commands Run

```bash
# List the exact commands and their exit statuses.
# Include only short output excerpts needed to explain a failure or surprise.
```

## What Happened

Describe the result, including anything confusing, slow, surprising, or good.
Keep the public report concise: do not paste a full installed-package inventory,
the complete local transcript, or broad environment output. Strip terminal
color/control sequences and include only output needed to understand the result.

## Support Bundle

Run this from the checkout when it would help maintainers understand the host
state:

```bash
sudo -v
bash scripts/conary-support-bundle.sh target/conary-support-bundle
```

- **Support bundle reviewed before attach**: yes/no

On an installed host, the script uses the cached authorization only for
allowlisted database-backed diagnostics and stops before writing if it is not
available. Review the bundle before attaching it. The script is allowlist-only
and does not copy `conary.db`, raw logs, environment dumps, shell history,
private keys, SSH keys, `/etc/conary/trust`, host-local access notes, or package
payloads. Do not attach any of those unless a maintainer explicitly asks for a
separately reviewed follow-up.
