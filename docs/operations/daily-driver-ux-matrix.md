---
last_updated: 2026-05-21
revision: 1
summary: Daily-driver CLI UX matrix for Goal 7 diagnostics, unsupported-case routes, shell completion checks, and focused tests
---

# Daily-Driver UX Matrix

## Purpose

This matrix is the Goal 7 contract for daily operator wording. It keeps
common package-manager commands boring, testable, and honest after the
structural readiness goals. It does not expand support claims: when a workflow
still belongs to the native package manager, adoption refresh, explicit
takeover, generation activation, or conaryd, the CLI should say that directly.

## Command Matrix

| Command | Success Route | Refusal Or Unsupported Route | Operator Guidance Phrase | Focused Test Target |
|---|---|---|---|---|
| `install <pkg>` | Conary-owned package install or dry-run plan | Adopted package already belongs to native authority | `conary --allow-live-system-mutation system adopt --refresh` before retry; `conary install <pkg> --dep-mode takeover` for explicit package takeover; `conary system takeover` for generation-level takeover | `cargo test -p conary --test cli_daily_ux adopted_install_refusal_routes_to_refresh_and_takeover` |
| `remove <pkg>` | Conary-owned package removal | Adopted package removal without `--purge-files` | Native package-manager authority is preserved; use `conary system unadopt <pkg>` to stop tracking or `--purge-files` only when file deletion is intentional | `cargo test -p conary --test cli_daily_ux adopted_remove_refusal_routes_to_unadopt_or_purge` |
| `update [pkg]` | Conary-owned update or security update from trusted advisory metadata | Adopted package update remains native-PM owned, unsupported advisory source fails before mutation | Native package-manager authority owns adopted updates; run `conary --allow-live-system-mutation system adopt --refresh` after native PM changes; use `--dep-mode takeover` only for explicit Conary takeover | `cargo test -p conary --test cli_daily_ux adopted_update_routes_to_native_pm_and_refresh` |
| `search <pattern>` | Repository search results from synced metadata | Empty or stale repository metadata | Run `conary repo sync` before assuming a package is unavailable | Existing query/search tests plus `cargo run -p conary -- search --help` |
| `list [pkg]` | Installed package identity, files, path owner, pinned state | Ambiguous installed package variants | Use `--version` and `--arch` to select a specific installed variant | Existing `cargo test -p conary --test query list_info_refuses_ambiguous_variants_until_selector_is_given` |
| `autoremove` | Removes Conary-owned orphaned dependency packages | Adopted orphaned packages remain native-PM owned | Native package-manager authority is preserved for adopted orphans | Existing `cargo test -p conary --test native_pm_daily_driver autoremove_dry_run_lists_conary_owned_orphans_and_skips_adopted` |
| `pin <pkg>` | Pins a selected installed variant | Ambiguous installed variants | Use `--version` and `--arch` to pin the intended variant | Existing `cargo test -p conary --test query pin_and_unpin_use_same_variant_selector` |
| `unpin <pkg>` | Releases a selected installed variant | Ambiguous installed variants | Use `--version` and `--arch` to unpin the intended variant | Existing `cargo test -p conary --test query pin_and_unpin_use_same_variant_selector` |

## Cross-Cutting Routes

- Live-host mutation refusal should offer three clear paths: use `--dry-run`
  for preview, rerun with `--allow-live-system-mutation` only when mutating the
  real machine is intended, or use conaryd package jobs when the operator needs
  durable background execution with the same acknowledgement boundary.
- Shell integration is verified by rendering completion output, not by visual
  review. Goal 7 requires at least:

```bash
cargo run -p conary -- system completions bash >/tmp/conary-completion.bash
cargo run -p conary -- system completions zsh >/tmp/conary-completion.zsh
```

- Generation guidance should stay in the generation command family. Daily
  package commands may point to `conary system generation build` or
  `conary system generation switch` only when the next user action is genuinely
  generation activation, rollback, or export.
- conaryd guidance is operator routing text for durable package jobs. It is not
  a new UI client and does not loosen the live-host mutation acknowledgement.

## Release Honesty

Do not mark an unsupported route as implemented in docs unless the focused test
target above or the referenced integration suite proves it. Keep active docs
clear that native package managers remain authoritative for adopted packages
until the user chooses explicit takeover.
