---
name: Bug Report
about: Report a bug in Conary
title: ''
labels: bug
assignees: ''
---

## Environment

- **Conary version**: (output of `conary --version`)
- **Distribution**: (e.g., Fedora 44, Ubuntu 26.04 LTS, Arch Linux)
- **Kernel version**: (output of `uname -r`)

## Description

A clear description of the bug.

## Steps to Reproduce

1. ...
2. ...
3. ...

## Expected Behavior

What you expected to happen.

## Actual Behavior

What actually happened.

## Logs

<details>
<summary>Support bundle or reviewed logs</summary>

```bash
bash scripts/conary-support-bundle.sh target/conary-support-bundle
```

Review the generated bundle before attaching it. It is allowlist-only and does
not copy `conary.db`, raw logs, environment dumps, shell history, private keys,
SSH keys, `/etc/conary/trust`, host-local access notes, or package payloads.
If a maintainer asks for `RUST_LOG=debug` output, review and redact it before
posting.

</details>

## Additional Context

Any other relevant information (package names, repository configuration, etc.).
