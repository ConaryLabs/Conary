# Limited Preview Release Checkpoint - 2026-05-16

## Scope

This checkpoint answers whether the current `main` baseline is ready for a
small public trial post, such as asking a limited number of subreddit users to
try Conary and report sharp edges.

This is not a release tag, a broad stability claim, or a recommendation for
critical machines. The preview claim being checked is narrower: Conary should
be a low-friction, reversible package-manager trial for Fedora 44, Ubuntu
26.04 LTS, and Arch Linux users who understand that this is still preview
software.

- Branch: `limited-preview-checkpoint-2026-05-16`
- Base commit checked out at start: `50b3ccee771908df36c88b542da5010fde1dff3c`
- Date: 2026-05-16

## Recommendation

Decision as of 2026-05-16: **no-go for the current documented limited-preview
surface.**

The package-manager and adoption flows have encouraging evidence, including
fresh Fedora 44, Ubuntu 26.04 LTS, and Arch adoption/unadoption proof plus the
existing native package-manager parity matrix. The current README/roadmap
surface still treats local QEMU validation and raw/qcow2 generation export as
part of the preview readiness bar, though, and the refreshed Group O QEMU run
found a real installed-runtime export boot failure.

A narrower **package-manager-only tester post** could be reconsidered if the
public wording deliberately excludes generation export from the ask, avoids any
"release ready" claim, and names the unresolved `tough`/`sigstore` advisory
caveat. Until then, do not ask subreddit users to treat the whole documented
preview surface as ready.

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

- `bash scripts/release-cargo-audit.sh` passed with the existing
  `proc-macro-error` unmaintained warning allowed by the release waiver.
- GitHub Dependabot still reports two open high alerts for `tough` in
  `Cargo.lock`.
- `cargo tree -i tough` shows `tough v0.21.0` is pulled through
  `sigstore v0.13.0`.
- `cargo update -p tough --precise 0.22.0 --dry-run` fails because
  `sigstore v0.13.0` requires `tough = "^0.21"`.
- Conclusion: the Dependabot highs are not reducible with a simple Cargo
  package update today. The next real fix is a `sigstore` upgrade, a temporary
  fork/patch, isolation/removal of the affected signing path, or an explicit
  preview waiver.

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

Local QEMU release gate:

- Run id: `limited-preview-checkpoint-20260516-124447`
- Evidence directory: `target/local-validation/limited-preview-checkpoint-20260516-124447`
- `phase3-composefs-modernization`: passed `TCM01` and `TCM02`, 2 passed, 0 failed, 0 skipped, 0 cancelled.
- `phase3-group-n-qemu`: after a manifest refresh for explicit generation
  build/switch semantics, passed 5, failed 0, skipped 0, cancelled 0.
- `phase3-group-o-generation-export`: passed `TGE01`, `TGE02`, and `TGE03`;
  failed `TGE04` `installed_runtime_generation_export_boots`.

Group O `TGE04` details:

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
- This is a blocker for the generation-export readiness claim, not a flaky
  marker-only failure.

## Preview Caveats

- `conaryd` package install/remove/update routes intentionally return
  `501 Not Implemented`; local CLI package-manager flows are the preview
  surface.
- ISO generation remains a proof-of-concept follow-up, not a limited-preview
  requirement.
- Installed-runtime raw/qcow2 generation export is not green in the current
  checkpoint. Treat it as a blocker for the existing documented preview
  surface or explicitly remove it from any package-manager-only public ask.
- Active-generation handoff back to native package-manager authority remains
  fail-closed. `system unadopt --all` is the low-risk escape hatch before a
  Conary generation is selected; handoff after selected generations still needs
  a separate plan.
- Security-only updates are truthful about repositories that cannot provide
  advisory metadata, but the current open `tough` advisories mean the preview
  should not be marketed as security-clean until that dependency path is
  resolved or waived.

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
- Be explicit that installed-runtime generation export hit a current QEMU
  blocker and is not part of the package-manager-only ask.
- Mention the unresolved `tough`/`sigstore` advisory path if posting before it
  is fixed or waived.

## Next Actions

1. Triage and fix or intentionally descope Group O `TGE04` installed-runtime
   generation export boot. The immediate failure to investigate is the exported
   image kernel panic: `No working init found`.
2. Decide the `tough` advisory path: upgrade `sigstore`, patch/fork, isolate
   the signing path, or add a dated preview waiver.
3. If generation export is descoped from the public ask, update README/roadmap
   wording before posting so the preview surface is not over-promised.
4. Draft the subreddit post as a narrow call for testers with the caveats
   above.
5. Keep active-generation native handoff as the next roadmap risk-reduction
   slice after this checkpoint.
