# Limited Preview Release Checkpoint - 2026-05-16, refreshed 2026-05-19 and 2026-05-22

## Scope

This checkpoint answers whether the current `main` baseline is ready for a
small public trial post, such as asking a limited number of subreddit users to
try Conary and report sharp edges.

This is not a release tag, a broad stability claim, or a recommendation for
critical machines. The preview claim being checked is narrower: Conary should
be a low-friction, reversible package-manager trial for Fedora 44, Ubuntu
26.04 LTS, and Arch Linux users who understand that this is still preview
software.

- Original checkpoint branch: `limited-preview-checkpoint-2026-05-16`
- Base commit checked out at original checkpoint start: `50b3ccee771908df36c88b542da5010fde1dff3c`
- Original checkpoint date: 2026-05-16
- Refresh date: 2026-05-19
- Goal 1 handoff refresh: 2026-05-22

## Recommendation

Decision as of 2026-05-19: **go for a narrow package-manager tester post, not
a broad release claim.**

The package-manager and adoption flows have encouraging evidence, including
fresh Fedora 44, Ubuntu 26.04 LTS, and Arch adoption/unadoption proof plus the
existing native package-manager parity matrix. The 2026-05-19 refresh also
restored the Group O QEMU generation-export gate to green: installed-runtime
and bootstrap-run raw/qcow2 exports both booted under UEFI.

The public ask should still be deliberately constrained. Ask for testers on VMs,
snapshotted systems, or non-critical machines; make adoption/unadoption the main
story; avoid "release ready" language; and keep selected-generation handoff as
an explicit `system native-handoff` recovery route rather than a silent promise.
conaryd package execution and ISO export remain outside this package-manager
tester ask.

## Evidence Summary

Fast workspace gates passed:

- `cargo fmt --check`
- `git diff --check`
- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `cargo run -p conary-test -- list`
- `bash scripts/check-release-matrix.sh`
- `bash scripts/test-release-matrix.sh`
- `cargo clippy --workspace --all-targets -- -D warnings`

Security/advisory triage:

- The first triage found `tough v0.21.0` pulled only through
  `sigstore-trust-root`; `cargo update -p tough --precise 0.22.0 --dry-run`
  could not resolve because `sigstore v0.13.0` required `tough = "^0.21"`.
- The 2026-05-19 patch removed the `sigstore-trust-root` feature, uses bundled
  Fulcio trust anchors for provenance verification, and removed `tough` from
  `Cargo.lock`.
- `cargo tree --locked -i tough` now reports no matching package.
- `cargo audit` still reports `RUSTSEC-2023-0071` for `rsa 0.9.10`; no fixed
  `rsa` release is available, and the remaining paths are through
  `sigstore`/`openidconnect` and `sequoia-openpgp`. Keep the dated preview
  waiver in `docs/superpowers/release-security-waivers-2026-05-06.md`.

Adoption/unadoption proof refreshed:

- Fedora 44 `phase1-advanced`: 31 passed, 0 failed, 0 skipped, 0 cancelled.
- Ubuntu 26.04 `phase1-advanced`: 31 passed, 0 failed, 0 skipped, 0 cancelled.
- Arch `phase1-advanced`: 31 passed, 0 failed, 0 skipped, 0 cancelled.

The manifest needed one honesty refresh before the matrix passed: takeover dry
run tests were still using the removed `--skip-conversion` flag, and one
generation-list assertion expected older wording. The product behavior was
current; the test manifest was stale.

Native package-manager parity evidence already recorded in the result files:

- Fedora 44/RPM `phase4-native-pm-parity`: 12 passed, 0 failed, 0 skipped, 0 cancelled.
- Ubuntu 26.04/DEB `phase4-native-pm-parity`: 12 passed, 0 failed, 0 skipped, 0 cancelled.
- Arch/package format `phase4-native-pm-parity`: 12 passed, 0 failed, 0 skipped, 0 cancelled.

Local QEMU release gate from 2026-05-16:

- Run id: `limited-preview-checkpoint-20260516-124447`
- Evidence directory: `target/local-validation/limited-preview-checkpoint-20260516-124447`
- `phase3-composefs-modernization`: passed `TCM01` and `TCM02`, 2 passed, 0 failed, 0 skipped, 0 cancelled.
- `phase3-group-n-qemu`: after a manifest refresh for explicit generation
  build/switch semantics, passed 5, failed 0, skipped 0, cancelled 0.
- `phase3-group-o-generation-export`: passed `TGE01`, `TGE02`, and `TGE03`;
  failed `TGE04` `installed_runtime_generation_export_boots`.

Resolved Group O `TGE04` blocker:

- The first TGE04 step adopted the source fixture, built an installed runtime
  generation, exported
  `/tmp/conary-generation-export/installed-runtime-generation.qcow2`, and
  copied the expected generation marker back to the host.
- The exported image then booted its kernel with
  `conary.generation=1`, but panicked before userspace:
  `Kernel panic - not syncing: No working init found.`
- The harness timed out waiting for SSH on port `2246` because the exported
  image never reached a working init or the expected
  `installed-runtime-generation-export-booted` marker.
- At the 2026-05-16 checkpoint, this was a blocker for the generation-export
  readiness claim, not a flaky marker-only failure.
- Root cause: default installed-runtime exports reused the adopted host
  initramfs from CAS, but that image did not include Conary generation
  activation. After forcing a Conary initramfs, dracut still omitted `/init`
  because `inst_script` resolved module sources through `--sysroot`.
- Fix: generate a Conary-aware initramfs for default `/boot` installed-runtime
  exports, copy the dracut module scripts directly into the initramfs image, and
  include a minimal `/init` that mounts the exported generation before
  `switch_root`.

Refreshed Group O evidence from 2026-05-19:

- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`:
  passed 4 / failed 0 / skipped 0 / cancelled 0.
- Passed cases:
  - `TGE01` `installed_generation_export_fails_closed_without_self_contained_root`: 149758ms
  - `TGE03` `installed_generation_build_rejects_missing_runtime_cas_object`: 482836ms
  - `TGE04` `installed_runtime_generation_export_boots`: 1558474ms
  - `TGE02` `bootstrap_run_generation_export_boots`: 2923418ms

## Preview Caveats

- Historical checkpoint note: on 2026-05-16, `conaryd` package
  install/remove/update routes intentionally returned `501 Not Implemented`.
  Goal 5 has since replaced that with queued daemon package jobs while keeping
  the CLI's explicit live-host mutation acknowledgement boundary.
- x86_64 ISO generation-carrier export exists, but it is not a limited-preview
  requirement.
- Installed-runtime and bootstrap-run raw/qcow2 generation export are green in
  the 2026-05-19 local QEMU run, but generation export is still supporting
  evidence rather than the main public ask.
- Active-generation handoff back to native package-manager authority remains
  fail-closed. `system unadopt --all` is the low-risk escape hatch before a
  Conary generation is selected; handoff after selected generations still needs
  a separate plan.
- Security-only updates are truthful about repositories that cannot provide
  advisory metadata. The former `tough` advisory path has been removed from the
  lockfile; the remaining `rsa` RustSec advisory is covered by the dated preview
  waiver because no fixed compatible path exists today.

## Public Trial Framing

Recommended public wording if a narrower package-manager-only post is approved:

- Ask for testers, not adopters.
- Encourage VMs or non-critical machines first.
- Say Fedora 44, Ubuntu 26.04 LTS, and Arch Linux are the tested preview
  targets.
- Make the reversible path prominent: install, adopt, try, and
  `conary system unadopt --all` before selecting a Conary generation.
- Be explicit that conaryd remote package execution and ISO export are not the
  thing being previewed.
- Keep generation export out of the headline even though the refreshed QEMU gate
  is green; the post is asking for package-manager feedback first.
- Do not market the preview as security-clean while the `rsa` waiver remains.

## Next Actions

1. Draft and review the subreddit post as a narrow call for package-manager
   testers with the caveats above.
2. Run final local verification for the TGE04/dracut and provenance dependency
   changes before publishing or merging.
3. Keep active-generation native handoff as the next roadmap risk-reduction
   slice after this checkpoint.
4. Keep the `rsa` waiver under review and remove it as soon as the dependency
   graph has a compatible fixed path.
