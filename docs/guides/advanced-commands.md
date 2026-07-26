---
last_updated: 2026-07-25
revision: 2
summary: The advanced packaging and platform command surface hidden from default CLI help
---

# Advanced Commands

The default `conary --help` shows the daily-driver surface: install, remove,
update, search, list, autoremove, pin/unpin, try, system, repo, config,
distro, and self-update.

The advanced packaging and platform surface is hidden from default help but
fully supported at its existing paths. List it any time with:

```bash
conary --help-advanced
```

The listing is rendered from the CLI's own command tree, so this page does
not duplicate it; run the command for the current surface. Broad areas:

- **Packaging and recipes:** `cook`, `new`, `publish`, `recipe-audit`, `ccs`
- **System modeling and composition:** `model`, `collection`, `groups`,
  `derive`, `derivation`, `profile`, `cache`
- **Provenance and trust:** `provenance`, `capability`, `trust`,
  `verify-derivation`, `sbom`, `canonical`, `registry`
- **Platform and distribution:** `bootstrap`, `federation`, `export`,
  `query`, `automation`, `mcp`

Every command keeps `conary <command> --help`.
