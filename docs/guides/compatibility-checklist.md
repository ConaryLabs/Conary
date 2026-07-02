---
last_updated: 2026-07-01
revision: 1
summary: Host requirements for the Conary limited preview, split by tier
---

# Compatibility Checklist

Check this before trying the preview so an unsupported host hits a doc, not
a wall.

## Tier 1 - Basic package loop

Covers: `install`, `remove`, `update`, `search`, `list`, adopt/unadopt, and
`try`. This is everything the first external tester loop asks you to run.

- Fedora 44, Ubuntu 26.04 LTS, or Arch Linux
- Stock distribution kernel - no composefs, UEFI, or special boot-stack
  requirement
- x86_64
- Root access (`sudo`)
- A VM, snapshot, or non-critical host (preview etiquette, not a technical
  requirement - adopt/unadopt is designed to be reversible)

## Tier 2 - Generation-model features

Covers: generation build/switch/rollback, `system generation export`, and
next-boot activation. NOT required for the basic package loop above.

- Linux 6.2+ with composefs support, overlayfs, and `CONFIG_EROFS_FS`
- systemd
- UEFI boot stack
- Sufficient disk for generation artifacts under `/conary`

If your host fails a Tier 2 item, everything in Tier 1 still works.
