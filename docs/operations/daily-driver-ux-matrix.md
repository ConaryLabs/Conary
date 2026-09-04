---
last_updated: 2026-09-03
revision: 3
summary: Daily-driver CLI routes, presentation slices, shell completion checks, and focused tests
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
| `install <pkg>` | Conary-owned package install or dry-run plan | Adopted package already belongs to native authority | `conary system adopt --refresh` before retry; `conary install <pkg> --ownership takeover --yes` for explicit package takeover; `conary system takeover --yes` for generation-level takeover | `cargo test -p conary --test cli_daily_ux adopted_install_refusal_routes_to_refresh_and_takeover` |
| `remove <pkg>` | Conary-owned package removal; Debian residual conffiles are preserved | Adopted package removal without `--purge` | Use `--purge` to delete residual config state or externally owned adopted files; use `conary system unadopt <pkg> --yes` to stop adopted tracking without deleting files | `cargo test -p conary --test cli_daily_ux adopted_remove_refusal_routes_to_unadopt_or_purge` |
| `update [pkg]` | Conary-owned update or security update from trusted advisory metadata | Adopted package update remains externally owned, unsupported advisory source fails before mutation | Refresh adoption after external changes; use `--ownership takeover` only for explicit Conary takeover | `cargo test -p conary --test cli_daily_ux adopted_update_routes_to_native_pm_and_refresh` |
| `search <pattern>` | Repository search results from synced metadata | Empty or stale repository metadata | Run `conary repo sync` before assuming a package is unavailable | Existing query/search tests plus `cargo run -p conary -- search --help` |
| `list [pkg]` | Installed package identity, files, path owner, pinned state | Ambiguous installed package variants | Use `--version` and `--arch` to select a specific installed variant | Existing `cargo test -p conary --test query list_info_refuses_ambiguous_variants_until_selector_is_given` |
| `autoremove` | Removes Conary-owned orphaned dependency packages | Adopted orphaned packages remain native-PM owned | Native package-manager authority is preserved for adopted orphans | Existing `cargo test -p conary --test native_pm_daily_driver autoremove_dry_run_lists_conary_owned_orphans_and_skips_adopted` |
| `pin <pkg>` | Pins a selected installed variant | Ambiguous installed variants | Use `--version` and `--arch` to pin the intended variant | Existing `cargo test -p conary --test query pin_and_unpin_use_same_variant_selector` |
| `unpin <pkg>` | Releases a selected installed variant | Ambiguous installed variants | Use `--version` and `--arch` to unpin the intended variant | Existing `cargo test -p conary --test query pin_and_unpin_use_same_variant_selector` |

## Cross-Cutting Routes

- Live-host mutation refusal should offer three clear paths: use `--dry-run`
  for preview, rerun the specific apply command with `--yes` when mutating the
  real machine is intended, or use conaryd package jobs when the operator needs
  durable background execution with the same intent boundary.
- Every applied install, update, remove, autoremove, automation, CCS, and
  conaryd package operation executes the complete typed lifecycle graph. There
  is no script-suppression flag or daemon request field; `--dry-run` is the
  non-mutating planning route.
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

## Ranked UI Slices

These slices change rendering and presentation, not package, publication,
query, download, or boot behavior. Each lands separately under #132 unless a
focused issue is created first. Proof for every slice includes before/after
evidence plus `cargo test -p conary --test output_vocabulary_guard` and
`cargo test -p conary --test cli_daily_ux`; snapshot changes also run
`cargo test -p conary --test cli_output_snapshots`.

1. **Fix TTY progress rendering** — For `install`, `update`, and `remove`, stop
   rendering zero-length bars. Single-package operations get one spinner line
   that clears to the final summary; bars appear only with known non-zero
   totals. Keep the primitive capable of a bounded aggregate-plus-worker layout
   for #535. Add a pty capture with `script -qec`.
2. **One warning/error voice** — Route deferred or stuck publication warnings
   once through `ui::warn`, retain tracing for logs rather than duplicate
   default output, render application failures through `ui::error_line`, align
   clap's visible vocabulary, and state each fact and remedy once. #534 owns
   publication behavior; this slice owns rendering.
3. **Transaction summary block** — Give `install --dry-run`, `install --yes`,
   `update`, and `remove` one shared summary renderer for install, upgrade, and
   remove groups; version, architecture, source format, file count, size, and
   disk delta; and a closing line that distinguishes planning from apply.
4. **Typed preflight rendering** — Render signature, authority, and preflight
   refusals from their fields: one `error:` line naming the cause, indented
   facts without debug wrappers or repeated paths, and one `note:` remedy.
5. **Field/heading unification and empty-state phrasing** — Route `list --info`,
   `ccs build`, `system history`, and list/search/update empty states through
   `ui::field` and `ui::heading`; preserve guarded ASCII tags and one phrasing
   pattern per empty state. Core returns typed CCS summary data for rendering at
   the application boundary. History drops hand-rolled tags and repeated retry
   prose. Update snapshots in the same slice.
6. **Structured refusal layout** — Keep the live-host refusal routes from this
   matrix, presented as a short cause plus `note:` next steps. Update
   `live_host_mutation_safety` expectations in the same slice.

## Release Honesty

Do not mark an unsupported route as implemented in docs unless the focused test
target above or the referenced integration suite proves it. Keep active docs
clear that native package managers remain authoritative for adopted packages
until the user chooses explicit takeover.
