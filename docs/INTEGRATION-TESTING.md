---
last_updated: 2026-08-13
revision: 49
summary: Document authenticated derivative targets, transport normalization, typed release evidence, and native lifecycle gates
---

# Integration Testing

Conary uses Podman containers to run integration tests on real Linux distributions. Tests exercise the full install/remove/update/adopt/generation lifecycle against a live Remi server.

Run these commands from the repository root, which is now a virtual Cargo
workspace. The product entrypoints live under `apps/`, with the shared package
domain in `crates/conary-core`.

## Prerequisites

- A **Docker** service or compatible **Podman** API socket (tests run as root
  inside containers)
- **Network access** to `remi.conary.io` (Remi server)
- A built conary binary (`cargo build -p conary`)
- The conary-test app crate (`cargo build -p conary-test`)

## Running Tests

```bash
# Run Phase 1 core tests on Fedora 44
cargo run -p conary-test -- run --suite phase1-core --distro fedora44 --phase 1

# Run all Phase 1 tests
cargo run -p conary-test -- run --suite phase1-core --distro fedora44 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1

# Run Phase 2 (deep E2E) tests
cargo run -p conary-test -- run --suite phase2-group-a --distro fedora44 --phase 2

# Run generation artifact export QEMU validation
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3

# Run ISO generation export QEMU validation
cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3

# Run focused composefs atomic modernization QEMU validation
cargo run -p conary-test -- run --suite phase3-composefs-modernization --distro fedora44 --phase 3

# Run selected-generation native authority handoff validation
cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro fedora44 --phase 3
cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro ubuntu-26.04 --phase 3
cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro arch --phase 3

# Run trusted security advisory ingestion and update validation
cargo run -p conary-test -- run --suite phase4-security-advisory-pipeline --distro fedora44 --phase 4

# Run the focused RPM/DEB/Arch lifecycle contract on one target image
cargo run -p conary-test -- run --suite native-cross-source-lifecycle --distro fedora44 --phase 4

# Run authentic Debian-derivative repository and native APT acceptance
cargo run -p conary-test -- run --suite debian-derivative-acceptance --distro linux-mint-22.3 --phase 4
cargo run -p conary-test -- run --suite debian-derivative-acceptance --distro pop-os-24.04 --phase 4

# Run all tests for a phase
cargo run -p conary-test -- run --distro fedora44 --phase 1

# List available suites
cargo run -p conary-test -- list
```

The generation artifact export QEMU suite is the operational proof for both
the 2026-04-22 generation-export unification slice and the later
self-contained installed-runtime export slice. The superseded 2026-04-30
baseline covered `TGE01` and `TGE02`. The active Fedora 44 suite now covers:

- `TGE01`: metadata-only installed generations fail closed before artifact publication
- `TGE03`: CAS-backed installed generations fail closed when an included CAS object is missing
- `TGE04`: a full CAS-backed installed runtime generation exports to qcow2 and boots under UEFI
- `TGE05`: exported installed-runtime generations retain full-adoption
  `CapturedRoot` lifecycle aliases, service enablement, and SSH authorization
  state in both carriers, then preserve capability-absent and
  capability-present `security.capability` state for the same package-owned
  path with rollback-equivalent proof through separate exported artifacts
- `TGE02`: a supported-host Fedora 44 root, assembled on a scratch disk from
  ordinary repository installs, publishes as a generation, exports to qcow2,
  and boots under UEFI

Keep this suite in the Phase 3 rotation for regressions in generation artifact
export, QEMU fixture copying, scratch-disk handling, CAS integrity checks,
guest SSH access, exported-image boot, and generation file-capability xattr
preservation. TGE05 must not reconstruct adopted lifecycle state in the
carrier fixture: it directly asserts the `/etc` aliases, wants links, and SSH
configuration captured before the initial generation build. Group P adds the
focused ISO generation-carrier path:

The active source fixture for the phase-3 QEMU suites is `fedora44-guest-v2`,
an official Fedora Cloud Base 44 qcow2 provisioned by
`scripts/build-qemu-guest-image.sh` and carrying the `conaryos-test-key-v4`
disposable identity. It supersedes v1 by including Fedora's packaged
systemd-boot EFI binary, which generation export consumes after full adoption.
The immutable public artifact is served from
`https://remi.conary.io/test-artifacts/fedora44-guest-v2.qcow2`; its SHA-256 is
`f688ac2a02b0b0558e28de1c97bbcb2e45b6772a4f019b037f72ec584a420174`,
so an empty-cache KVM host can reproduce the lane.

It replaces the `minimal-boot-*` lineage, which the current build cannot use for
two independent reasons found on 2026-07-28:

- Those images are conaryOS artifacts of the bootstrap pipeline, so their
  staged-binary path depends on a frozen glibc that a rolling build host
  outgrows (#153). `minimal-boot-v5` patched that by hand by injecting host
  libraries — a workaround, not a contract, and it never produced a green suite
  run.
- They ship a bootstrap-built `/var/lib/conary/conary.db` at migration-chain
  schema version 66. The schema epoch hard cut in df607ee8 (#61, 2026-07-26)
  retired it, so `conary system init` refuses inside them with
  `database uses retired migration-chain schema version 66`. Use the explicit
  `conary system rebuild-db --discard-state --yes` path only when that fixture's
  active Conary state is disposable; it preserves the retired database as a
  snapshot and initializes current repository/host-capability authority.

The Fedora image ships no `/var/lib/conary` at all, so `system init` creates a
fresh database at the current schema.

The 2026-07-31 supported-host evidence below is current. Earlier evidence
predates both the re-base and the #61 schema cut and was recorded against
`minimal-boot-v4`.

Historical 2026-07-16 W1 Group O local KVM evidence was green. The source fixture
was `minimal-boot-v4`, expanded to 20 GiB for full CAS adoption and paired with
the versioned disposable `conaryos-test-key-v4` identity. The runner discovers
Fedora's `/usr/share/edk2/x64/OVMF_CODE.4m.fd`, attaches OVMF as read-only
pflash, fails when no supported firmware exists, and waits for the systemd boot
transaction to finish before it stages binaries or adopts the live root.

The complete Fedora 44 suite passed 5 / failed 0 / skipped 0 / cancelled 0.
`TGE05` passed twice: a focused run in 3,060,588 ms and the complete-suite run
in 3,024,480 ms. Both exported generations booted, the baseline executable had
no file capability, and the enabled executable reported
`cap_net_bind_service=ep`. A focused TGE01 rerun with the recompiled versioned
key contract passed in 36,068 ms. The v4 image and private/public disposable
test-key artifacts were then staged to Remi over authenticated SSH, verified
against their local SHA-256 values, and atomically published under
`https://remi.conary.io/test-artifacts/`. An isolated empty cache downloaded
the image and private test key from those public URLs with matching hashes;
TGE01 booted the downloaded image under KVM and passed in 63,320 ms.

- `TISO01`: a supported-host generation exports to ISO, emits an output
  provenance sidecar, and boots under UEFI through `image_format = "iso"`

`TGE02` and `TISO01` share one basis, the supported-host fixture in
`apps/conary/tests/fixtures/supported-host-generation-export/`. Each suite runs
`conary system init --db-path` against a scratch-disk root, adds the Fedora 44
Remi profile, installs a test-time-built `conary-qemu-test-access` CCS package
for harness SSH and unit enablement, applies `fixture-system.toml` with
`conary model apply --yes --no-autoremove`, then converges publication with
`conary system generation publish --yes`. `model apply` resolves the changed
entries as one exact-source SAT package set and executes one lifecycle
transaction; successful apply may therefore publish the complete closure
automatically. The shared convergence helper treats the final `publish` as
idempotent: it must either clear residual debt or confirm publication is
already current, after which `generation pending` must report no debt and the
exact numeric target of the selected-generation link is validated. No
bootstrap surface is involved and the manifest system has no cross-suite
artifact passing, so each suite assembles its own root.

Both supported-host manifests format the scratch root as ext4 with the
`verity` feature before initializing it. Composefs publication enables
fs-verity on `root.erofs` and fails closed when the selected root's backing
filesystem cannot provide it; plain ext4 is therefore not a valid fixture
substitute. The convergence helper preserves the publish exit status, prints
its full log, and queries the latest typed
`generation_publications.last_error` so a failed gate records the exact
publication cause.

The source QEMU image for Groups N, O, and P must already include the runtime
generation toolchain (`cpio`, `dracut`, `depmod`, `systemd-repart`, `qemu-img`,
FAT/ext4 mkfs tools, and composefs inspection tools as needed). Group P uses the
same source fixture and provisions ISO helper packages through Conary when
`xorriso`/`mtools` are absent before it assembles the generation.

A step with `stage_conary = true` copies a `conary` into the guest. A host-built
binary would couple the guest to the build host's glibc and to a matching
`libseccomp.so.2`, so the harness stages the artifact from
`scripts/build-static-conary.sh` instead: it builds a static libseccomp for musl
once, caches it, and produces a statically linked `conary` for
`x86_64-unknown-linux-musl` that runs on any guest. `CONARY_HOST_BIN` overrides
that default to stage a specific binary. The build is debug profile and the
binary is a test-staging artifact, not a release artifact. Distro container
images stage the same artifact through `build_context = "static-binary"`.

`scripts/build-qemu-guest-image.sh` builds such an image from a pinned official
Fedora Cloud Base qcow2 plus provisioning, and `bash
scripts/test-build-qemu-guest-image.sh` is its fast proof. The current
manifests name `fedora44-guest-v2`; the builder is the regeneration authority
for that Fedora-based fixture. The retired `minimal-boot-*` conaryOS artifacts
have no regeneration path once the bootstrap pipeline is removed and remain
historical evidence only. See `docs/modules/test-fixtures.md` under
`qemu-source-image-fixtures` for the image contract and the identity/size
rotation helper. The focused
2026-05-21 KVM run passed the superseded bootstrap-run form of `TISO01`: it
exported that generation to ISO, copied the ISO and provenance sidecar back to
`target/local-validation/group-p-iso-export/`, booted the ISO with
`image_format = "iso"`, verified the readonly carrier kernel arguments, and
proved the writable `/etc` overlay. The re-based `TGE02` and `TISO01` have not
yet had a live run; their first bring-up is the proof that the fail-forward
accumulation path works end to end.

The focused composefs atomic modernization suite covers the stricter runtime
contract added on 2026-05-13:

- `TCM01`: OCI export and generation switching reject a generation artifact
  after `root.erofs` is removed
- `TCM02`: rollback fails before mutating state when no active composefs
  generation exists

`scripts/local-qemu-validation.sh` runs this focused suite before the broader
Group N, Group O, and Group P gates so fail-closed composefs behavior is
recorded even when a source-image fixture preflight blocks the longer
boot/export suites.

The selected-generation native authority handoff suite covers the Goal 1
handoff contract added on 2026-05-22:

- `THND01`: dry-run reports selected generation, adopted package plan, and
  native package-manager preservation without clearing `/conary/current`
- `THND02`: apply mode refuses before mutation unless the operator confirms
  `--yes`
- `THND03`: confirmed handoff clears `/conary/current`, removes adopted
  Conary tracking, writes a completion record, and preserves native package
  files and native package-manager queries
- `THND04`: simulated interruption after current-link clearing is recovered
  with `conary system native-handoff --recover --yes`

Current selected-generation handoff evidence from 2026-05-22:

- `cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro fedora44 --phase 3`:
  passed 4 / failed 0 / skipped 0 / cancelled 0
- `cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro ubuntu-26.04 --phase 3`:
  passed 4 / failed 0 / skipped 0 / cancelled 0
- `cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro arch --phase 3`:
  passed 4 / failed 0 / skipped 0 / cancelled 0

## CLI Subcommands

Common conary-test operations have CLI equivalents for human use:

| Command | Purpose |
|---------|---------|
| `conary-test bootstrap check [--json]` | Inspect local developer prerequisites and smoke-readiness status |
| `conary-test bootstrap smoke [--dry-run] [--json]` | Preview or run the local developer smoke proof loop |
| `conary-test run --suite <name> --distro <distro> --phase <N>` | Execute a test suite |
| `conary-test deploy status` | Show local binary, checkout, and managed-rollout status |
| `conary-test fixtures build [--groups all]` | Build test fixture CCS packages |
| `conary-test fixtures publish` | Publish fixtures to Remi |
| `conary-test logs <test-id> [--run <id>] [--step <N>]` | Retrieve test logs |
| `conary-test health` | Normalized local/Remi health envelope with local `deploy_status`, optional `remi`, and optional `reason` |
| `conary-test images build --distro <name>` | Build a container image for a configured distro |
| `conary-test images list` | List locally built container images |
| `conary-test images prune [--keep <N>]` | Remove old container images |
| `conary-test images info <image>` | Inspect container image |
| `conary-test manifests reload` | Reload TOML manifests without restart |

Commands with structured output accept `--json`; `conary-test run` emits a JSON
result report by default.

## CCS Fixture Trust

All checked-in and runtime-generated CCS integration fixtures share the
disposable Ed25519 authority under
`apps/conary/tests/fixtures/ccs-test-authority/`. The harness derives
`${FIXTURE_CCS_KEY}`, `${FIXTURE_CCS_POLICY}`, and
`${FIXTURE_CCS_EXPIRED_POLICY}` from `[paths].fixture_dir`; fixture builds pass
the key explicitly, and every install or verify passes one of those policies.
This authority is public test data and must never authorize release,
repository, federation, update, or production packages.

Rotate and rebuild the complete CCS fixture corpus together:

```bash
cargo build -p conary
apps/conary/tests/fixtures/ccs-test-authority/generate.sh
apps/conary/tests/fixtures/conary-test-fixture/build-all.sh
apps/conary/tests/fixtures/adversarial/build-all.sh
cargo run -p conary-test -- list
```

The `conary-test-fixture/v1` and `v2` directory names mean package version 1
and package version 2; both emit the current signed CCS format. Adversarial
archive fixtures are built as signed current packages first and only then
mutated, so failures exercise their named integrity boundary. Failing
post-install scriptlet fixtures must return nonzero and leave package database
state and payload absent.

From a checkout, use
`cargo run -p conary-test -- bootstrap check --json` before smoke validation to
inspect local prerequisites such as Cargo, manifest availability, container
runtime readiness, and optional QEMU/KVM support.

Use the local developer smoke proof loop when you want one command to preview
or execute the default `phase1-core` Fedora 44 validation path:

```bash
cargo run -p conary-test -- bootstrap check --json
cargo run -p conary-test -- bootstrap smoke --dry-run --json
cargo run -p conary-test -- bootstrap smoke --json
```

`bootstrap smoke` may build images, start containers, and write conary-test
result files through the normal runner. It is not package publishing, does not
publish fixtures, and does not require cloud credentials.

Remote Forge control-plane validation and conary-test deployment are
decommissioned. Use the local QEMU/KVM gate for temporary release evidence;
there is no replacement Forge rollout path.

Do not describe local evidence as hosted CI. Any QEMU release evidence must
name the absolute run date, distro, suite name, and pass/fail/skip/cancel
counts.

For the temporary local QEMU release gate, run this on a development machine
with `/dev/kvm`:

```bash
scripts/local-qemu-validation.sh
```

Historical local release evidence is
`target/local-validation/qemu-blocker-fix-20260509-201100`, recorded on
2026-05-09. That run predates the stricter composefs install and activation
contract. It passed Group N (`T150`, `T151`, `T153`, `T154`, `T156`) and
Group O (`TGE01`, `TGE02`, `TGE03`, `TGE04`) with 0 failures and 0 skipped
results, emitted the required boot/export markers, and finished with
`[local-qemu-validation] ok`.

Current composefs modernization evidence from 2026-05-21:

- `cargo run -p conary-test -- list`: passed; includes
  `phase3-composefs-modernization`
- `scripts/local-qemu-validation.sh`:
  passed `TCM01` and `TCM02`, 2 passed / 0 failed / 0 skipped
- Passed cases:
  - `TCM01` `partial_generation_artifacts_rejected`: 69299ms
  - `TCM02` `no_active_generation_rollback_rejected`: 24486ms

Current Group N QEMU evidence from 2026-05-21:

- `minimal-boot-v3`: source fixture used by this historical run, with the
  generation-builder toolchain baked in for Groups N and O; Group P provisions
  ISO helper packages through Conary when `xorriso`/`mtools` are absent
- `scripts/local-qemu-validation.sh`:
  passed 5 / failed 0 / skipped 0
- Passed cases:
  - `T150` `kernel_file_deployment`: 1101193ms
  - `T151` `bls_entry_created`: 1110350ms
  - `T153` `kernel_generation_rollback`: 1183803ms
  - `T154` `bootloader_config_deployed`: 1180481ms
  - `T156` `boot_minimal_image`: 20686ms
- The historical `T154` run installed `grub2` after full CAS-backed live-root
  adoption, but its dependency proof relied on synthetic
  `conary-live-root` provides inferred from file paths. That heuristic
  authority has been deleted; this result does not count as current dependency
  proof until the fixture supplies exact package identities and the gate is
  rerun.
- This historical manifest predated mandatory selected-root publication.
  Current installs materialize DB/CAS state when no generation exists and
  publish `/conary/current` through the same atomic package transaction.

Current Group O QEMU export evidence from 2026-07-31:

- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`:
  passed 5 / failed 0 / skipped 0 / cancelled 0 against
  `fedora44-guest-v2`
- Passed cases:
  - `TGE01` `installed_generation_export_fails_closed_without_self_contained_root`: 116022ms
  - `TGE03` `installed_generation_build_rejects_missing_runtime_cas_object`: 552439ms
  - `TGE04` `installed_runtime_generation_export_boots`: 994129ms
  - `TGE05` `installed_runtime_generation_export_preserves_file_capability_xattrs`: 2973162ms
  - `TGE02` `supported_host_generation_export_boots`: 2383246ms
- TGE02 assembled the Fedora root through ordinary signed Remi CCS installs,
  published with zero debt, exported the selected generation, and booted the
  qcow2 under UEFI. TGE04 and TGE05 retained their installed-runtime and
  file-capability carrier proof.
- Full artifact hashes and the exact implementation head are recorded on PR
  #151; this is local x86_64 KVM evidence, not hosted CI.

Historical Group O QEMU export evidence from 2026-07-16:

- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`:
  passed 5 / failed 0 / skipped 0 / cancelled 0 against `minimal-boot-v4`
- Passed cases:
  - `TGE01` `installed_generation_export_fails_closed_without_self_contained_root`: 36281ms
  - `TGE03` `installed_generation_build_rejects_missing_runtime_cas_object`: 625760ms
  - `TGE04` `installed_runtime_generation_export_boots`: 1725736ms
  - `TGE05` `installed_runtime_generation_export_preserves_file_capability_xattrs`: 3024480ms
  - `TGE02` `bootstrap_run_generation_export_boots`: 1914811ms
- TGE04 booted generation 1 from the exported installed-runtime qcow2 under
  UEFI; TGE05 booted capability-absent and capability-enabled generations and
  observed `cap_net_bind_service=ep` only in the enabled artifact; TGE02
  substituted the rotated fixture public key into the bootstrap recipe before
  export and boot.
- The v4 qcow2 and versioned private/public test-key artifacts are publicly
  available from `https://remi.conary.io/test-artifacts/`. An isolated-cache
  download matched the source hashes and passed a 63,320 ms TGE01 KVM boot.
- The `TGE02` in this and the older dated block is the superseded
  `bootstrap_run_generation_export_boots` case. Current supported-host proof is
  recorded above. The `TGE01`, `TGE03`, `TGE04`, and `TGE05` results remain
  useful historical evidence.

Historical Group O QEMU export evidence from 2026-05-21:

- `scripts/local-qemu-validation.sh`:
  passed 4 / failed 0 / skipped 0 / cancelled 0
- Passed cases:
  - `TGE01` `installed_generation_export_fails_closed_without_self_contained_root`: 24272ms
  - `TGE03` `installed_generation_build_rejects_missing_runtime_cas_object`: 479388ms
  - `TGE04` `installed_runtime_generation_export_boots`: 2182397ms
  - `TGE02` `bootstrap_run_generation_export_boots`: 3168791ms
- Root evidence: TGE04 now generates a Conary-aware initramfs for the exported
  installed-runtime image, boots the qcow2 under UEFI, reaches SSH, and emits the
  `installed-runtime-generation-export-booted` marker.
- Local wrapper log: `target/local-validation/qemu-20260519203933/group-o-generation-export.log`

Keep Group O in the release-candidate rotation because it is still the full
boot/export proof for installed-runtime and supported-host generation
artifacts.

Current Group P ISO export evidence from 2026-07-31:

- `cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3`:
  passed `TISO01`, 1 passed / 0 failed / 0 skipped / 0 cancelled against
  `fedora44-guest-v2` in 2326789ms
- `TISO01` assembled the supported-host generation through ordinary signed
  Remi CCS installs, emitted the ISO and provenance sidecar, copied both back,
  booted the read-only ISO under UEFI, selected the exact generation, and
  retained a writable `/etc` overlay.
- The ISO was 667713536 bytes with SHA-256
  `ff6bd95bcd3ac9ceb27712f4acda0328ef4edc97139a8047d310cb415998338b`;
  the provenance sidecar records the same size and hash. Full provenance and
  exact-head identity are recorded on PR #151.

Historical Group P ISO export evidence from 2026-05-21, against the superseded
bootstrap-run form of `TISO01`:

- `cargo run -p conary-test -- list`: passed; includes
  `ISO Generation Export QEMU` with one test
- Focused Rust checks for QEMU ISO support and manifest parsing passed under
  `cargo test -p conary-test qemu_image` and
  `cargo test -p conary-test qemu_boot_`
- `cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3`:
  passed `TISO01`, 1 passed / 0 failed / 0 skipped / 0 cancelled
- The run exported `/mnt/conary-scratch/export/bootstrap-run-generation.iso`,
  emitted `/mnt/conary-scratch/export/bootstrap-run-generation.iso.conary-provenance.json`,
  copied both files to `target/local-validation/group-p-iso-export/`, booted
  the ISO under UEFI, reached SSH, and emitted the
  `bootstrap-run-generation-iso-export-booted` marker.
- The ISO boot verified `conary.generation=1`, `conary.carrier=readonly`,
  `rootfstype=iso9660`, generation artifact files, and a writable `/etc`
  overlay on the read-only carrier.
- `TISO01` is now `supported_host_generation_iso_export_boots` on the
  supported-host fixture. Its current proof is recorded above; the carrier and
  provenance assertions survived the re-base while the generation basis
  changed.

Fast workspace verification from 2026-05-14:

- `cargo fmt --check`: passed
- `cargo run -p conary-test -- list`: passed
- `cargo test -p remi`: passed
- `cargo test -p conary-core --test generation_composefs_runtime_contract -- --nocapture`: passed
- `cargo test -p conary-core missing_regular_file_cas_object -- --nocapture`: passed
- `cargo test -p conary-core`: passed
- `cargo test -p conary`: passed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- `git diff --check`: passed

Focused Slice C daily-driver CLI proof:

- `apps/conary/tests/native_pm_daily_driver.rs` proves Tier 1 list/info/files/path,
  pin/unpin, autoremove, and query diagnostics for Conary-owned packages before
  the broader Slice D distro matrix.
- Provider-resolution unit coverage proves package-manager decisions use exact
  package names and declared installed/repository provider metadata instead of
  soname/package-name guessing.

Focused Goal 7 daily-driver UX proof:

- `apps/conary/tests/cli_daily_ux.rs` proves the checked-in
  `docs/operations/daily-driver-ux-matrix.md` routes for rendered root help,
  bash/zsh completion output, live-host mutation refusal, adopted-package
  install takeover guidance, adopted-package remove unadopt/purge guidance, and
  adopted-package update native-PM/adoption-refresh guidance.
- Completion rendering is verified by command execution, not visual review:
  `cargo run -p conary -- system completions bash` and
  `cargo run -p conary -- system completions zsh`.

Focused Slice D native package-manager parity proof:

- `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4`
- `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4`
- `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4`
- Focused PR/local lane:
  `cargo run -p conary-test -- run --suite native-cross-source-lifecycle --distro <distro> --phase 4`

The current `phase4-native-pm-parity` manifest has 19 tests. `TNPM02X` exports
one lifecycle-bearing v1/v2 fixture as RPM, DEB, and Arch artifacts on every
target image. Fedora captures the RPM-owned trace, Ubuntu captures the
dpkg-owned trace, and Arch captures the libalpm/pacman-owned trace. Each trace
records exact event order, script argv, the package-script stdin contract, and
payload visibility at the event boundary; the matching checked-in trace must
byte-match that native run before it can serve as an oracle. Every target then
completes install, update, rollback, and remove for all three formats through
Conary and matches the native-verified trace through the selected generation's
exact manifest hashes and CAS objects. Failing `rpm`, `rpmdb`, `dpkg`, `apt`,
`pacman`, and native build-tool shims remain first on `PATH` during Conary
mutation, and every manager executable present in the image is temporarily
replaced by an exact recording shim so absolute-path delegation also fails the
run. The manifest passes the native oracle format as an explicit typed lane
input; the gate never infers package authority from a distro name. The three
distro jobs therefore prove all 36 source-format, target-image, and
lifecycle-state cells, backed by three source-manager oracle captures rather
than payload-only equivalence.
Rollback is checked as restoration of the exact native-v1 installed trace and
payload snapshot, not mislabeled as a native package-manager downgrade.
`TNPM01` through `TNPM12` otherwise retain the repository, host-native package,
update, query, security-refusal, and autoremove parity proof. Repository update
selection uses a signed CCS package synchronized through typed JSON metadata;
the unknown-security case also enters through normal typed repository sync,
not a synthetic package row. `TNPM13` through `TNPM18` add the daily-driver
corpus group. That corpus builds a package in the host-native format and then
proves:

- systemd unit file deployment and trigger matching
- tracked `/etc` config file metadata
- native dependency metadata for a real package dependency
- an exact installed native-lifecycle bundle plus install/remove hook effects
- system user and group creation in the selected generation
- conflict refusal before an overlapping native package can mutate selected
  state
- a 2 MiB payload file through the native package parser and file database
- a QEMU-safe kernel-adjacent `kernel/install.d` file without mutating boot state
- an alternative target binary (`/usr/bin/phase4-corpus-alt`) as packaged file
  coverage

Corpus coverage boundaries (not product support exemptions):

- native alternatives registration is still a foreign-format conversion note
  (`alternatives`/`update-alternatives` on RPM/DEB; no Arch equivalent here);
  the corpus covers the target file, not registration ownership
- bootloader or kernel post-install hooks that regenerate initrds, boot entries,
  or host boot configuration
- arbitrary uncurated repository package selections and full ecosystem parity
- dependency installation from the generated local corpus package; this suite
  records dependency metadata and installs with `--no-deps`

The focused `native-cross-source-lifecycle` manifest contains three non-flaky
corpus cases, one for each exact source format. A failed case does not suppress
the remaining source-format evidence. Each case executes the same shared
lifecycle helper as `TNPM02X`, writes a versioned evidence
envelope carrying both install and update artifact digests, the typed fixture
build-manifest authority that produced them, and stage checkpoints. It is
projected by `conary-test` into a typed
`CorpusCaseResult`. The suite JSON carries the typed aggregate beside the
ordinary test results and records the manifest-declared case count; a missing,
malformed, skipped, failed, or unclassified corpus case makes the command fail
even if generic command output happened to look successful.

The adopted-artifact bridge has a separate hermetic command test:

```bash
cargo test -p conary --lib commands::adopt::convert
```

Its lifecycle case re-executes in an unprivileged user and private mount
namespace when the parent test is not root. For RPM, Debian, and Arch it
publishes two adoption-produced signed CCS versions, installs and updates them
in a clean selected root, rolls the update back, removes the restored package,
and confirms the persisted native lifecycle bundle. The execution path uses
only CCS operations after publication and therefore never consults a source
package manager or its database at runtime.
The `pr-gate` workflow builds each configured distro image first and then runs
that focused manifest across `fedora44`, `ubuntu-26.04`, `arch`, the OpenRC
`artix` lane, Linux Mint 22.3, and Pop!_OS 24.04. Together, the required lanes
authenticate all three checked-in source-ABI traces and run the full 6x3
Conary Cartesian product. Corpus target
facts resolve from explicit per-lane manifest authority, so Artix records
OpenRC rather than inheriting a systemd default. Missing native manager
authority, container support, an exact trace match, or an image build fails its
matrix job; there is no manifest skip fallback. A stable
`native-cross-source-lifecycle` aggregator fails unless every distro matrix job
succeeds.

The two Debian-derivative jobs also run `debian-derivative-acceptance`. Their
shared Containerfile starts from one digest-pinned Ubuntu transport root, then
installs exact release-owned identity and keyring packages and restores the
actual APT declarations captured from authenticated release media. Before any
release package is installed, the builder verifies and removes the pinned OCI
transport's dpkg payload-exclusion policy; release-root package records must
refer to materialized payload rather than Docker-reduced bytes. Build and
runtime preflight require exact `/etc/os-release`, package versions,
declaration bytes, signing-root bytes and fingerprints, and `conary --version`.
Linux Mint's legacy global APT trust is resolved only by declaration-entry and
fingerprint bindings in typed test configuration; Pop!_OS retains its explicit
`Signed-By` declarations. The suite executes repository
preview/apply/repeat/rollback plus native dpkg query, adoption, refresh,
takeover preview, unadoption, and removal. Distro names select test data only;
product trust and parser selection remain typed declaration authority.

Successful derivative reports carry a `target_release` schema with the exact
release-media, base-image, package, APT declaration, signing-root, checkout,
and running Conary artifact identities plus typed preflight stages. Each
artifact distinguishes pinned release authority, pinned build input, and bytes
observed in the running target; configuration is not mislabeled as a runtime
read. Merely configuring these lanes is not acceptance evidence; the first exact-head
hosted runs remain required before public support claims.

Published-release proof uses the same contract through
`.github/workflows/release-artifact-proof.yml`. Its explicit
`conary-test images build --native-package <path>` input resolves the target
format from the typed supported-profile catalog, stages one canonical package
filename, and makes the image install that package through `dnf`, `apt`, or
`pacman`. The workflow independently matches `SHA256SUMS` and the GitHub asset
digest before the installed `/usr/bin/conary` runs all three source formats.
Source-built PR evidence is not a substitute for this published-byte lane.

Solus uses the separate `solus-eopkg-acceptance` QEMU suite because its
release authority is an official bootable image rather than a container
facsimile. The suite boots the pinned current Polaris root, proves native
eopkg and update state, applies/repeats/rolls back exact repository takeover,
syncs the authenticated 11,907-record index, and converts an authentic
installed eopkg only after exact adopted-payload equivalence. Its image is the
published `solus-4.9-polaris-2026-08-12-v3.qcow2` test artifact; a skipped
download or QEMU step is not acceptance evidence. The published image's ext4
root carries the `verity` feature before the suite starts, and the suite proves
that fixture contract before Conary initialization so generation publication
cannot degrade to an unprotected composefs mount.

The bounded `selected-root-overlay-profile-solus` QEMU suite reuses that
immutable fixture without running takeover. It functionally mounts the exact
selected-root OverlayFS profile and proves indexed hardlink copy-up, complete
metadata copy-up, character-device whiteouts, logical opaque replacement, and
`EXDEV` for lower-directory rename. Against the fixture's selected generation,
it proves the verified composefs image is mounted directly beneath typed mutable
state and that both nested mounts are gone before publication failure returns.
The bounded artifact is reconstructed by
`apps/conary/tests/fixtures/selected-root-current-generation/prepare.py`; a
core integration test loads that exact artifact contract before the privileged
lane consumes it.
It also stages an isolated first-generation runtime, installs and removes a
signed lifecycle-free CCS through the materialized-candidate path, verifies the
changed path appears and disappears in successive manifest-only candidates,
and proves that no session mount or workspace survives either commit. This is
the positive kernel/filesystem and transaction counterpart to the runtime typed
preflight; a kernel version or registered filesystem name alone is not
capability evidence.

Each full parity run must pass `scripts/check-conary-test-result-gate.sh` and
`scripts/check-conary-corpus-result-gate.sh`,
which requires zero failed, skipped, and cancelled results before the matrix
can count as limited-preview release evidence. The `conary-test run` command also exits
unsuccessfully for skipped or cancelled results. Distro images rebuild by
default so the matrix uses the current checkout; set `CONARY_TEST_REUSE_IMAGE=1`
only for local iterative debugging where stale-image risk is acceptable.

Evidence from the former 18-test, host-native-only matrix does not certify the
current 19-test cross-source contract. Record new evidence only after all six
current distro jobs pass the result gate.

Focused Goal 3 security advisory pipeline proof:

- `cargo run -p conary-test -- run --suite phase4-security-advisory-pipeline --distro fedora44 --phase 4`

The `phase4-security-advisory-pipeline` manifest builds a v1/v2 native fixture,
serves JSON repository metadata with a trusted `security_advisory_source`,
proves an `unknown` source refuses before mutation, then syncs the same
repository as `--security-advisories supported` and verifies persisted severity,
CVE, advisory ID, fixed version, and source-trust metadata before
`conary update --security` applies the trusted fix.

Fresh Goal 3 evidence from May 19, 2026:

- Fedora 44/RPM: `phase4-security-advisory-pipeline`, 7 passed, 0 failed, 0
  skipped, 0 cancelled.

The former Forge control-plane smoke depended on the removed conary-test
network server and is no longer a validation path. Use the CLI run/list proof
and the hosted Remi checks below; no replacement Forge runner or rollout path
is part of the supported surface.

## Release Evidence Block

Before publishing a limited-preview tester post or refreshing the release
artifact/source matrix, run the current evidence command block from
[docs/operations/release-artifact-matrix.md](operations/release-artifact-matrix.md):

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-truth.sh
bash scripts/check-release-matrix.sh
bash scripts/release-cargo-audit.sh
```

For shared tester feedback, prefer the pre-alpha tester issue template and the allowlist
support bundle:

```bash
sudo -v
bash scripts/conary-support-bundle.sh target/conary-support-bundle
```

On an installed host, the script uses the cached authorization only for
allowlisted database-backed diagnostics and stops before writing if it is not
available. Review the generated bundle before attaching it. It does not copy
`conary.db`, raw logs, environment dumps, shell history, private keys, SSH keys,
`/etc/conary/trust`, host-local access notes, or package payloads.

## Validation Modes

- `merge-validation` is the trusted on-merge lane. The former Forge
  control-plane server smoke is retired; the workflow currently runs hosted
  build/list/Remi smoke checks instead.
- `scheduled-ops` keeps hosted Remi health, audit, and manifest-inventory
  checks active. Forge-backed Phase 1-3 and QEMU jobs are retired; local QEMU
  evidence must name its host, date, distro, suite, and counts.
- Raw `cargo run -p conary-test -- run ...` remains useful for local debugging
  and evidence collection.

### Available Distros

| Distro | Container | Base | `build_context` |
|--------|-----------|------|-----------------|
| `fedora44` | `Containerfile.fedora44` | Fedora 44 | `static-binary` |
| `ubuntu-26.04` | `Containerfile.ubuntu-26.04` | Ubuntu 26.04 LTS | `static-binary` |
| `arch` | `Containerfile.arch` | Arch Linux (rolling) | `static-binary` |
| `artix` | `Containerfile.artix` | Artix Linux (rolling, OpenRC) | `static-binary` |
| `linux-mint-22.3` | `Containerfile.debian-derivative` | Linux Mint 22.3 release-owned root | `static-binary` |
| `pop-os-24.04` | `Containerfile.debian-derivative` | Pop!_OS 24.04 release-owned root | `static-binary` |
| `solus` | QEMU artifact | Solus 4.9 updated to current Polaris | `static-binary` |

`build_context` is a required typed distro capability in `config.toml` that
selects which Conary binary an image receives. Distro names do not select this
behavior.

- `static-binary` stages the static `x86_64-unknown-linux-musl` artifact from
  `scripts/build-static-conary.sh`. It carries no glibc or `libseccomp.so.2`
  coupling, so one artifact runs in every image. Build it before
  `images build`; staging fails closed when it is missing or not actually
  static.
- `binary` stages the host-built Conary binary. It only runs in an image whose
  userland matches the build host.

Every distro uses `static-binary`. Ubuntu was the forcing case: its container
userland does not match the build host, which is why that image used to compile
Conary from source in a builder stage and made its CI leg roughly three times
slower than the others.

## Test Structure

### Phase 1: Core Integration (T01-T37)

Always runs. Tests basic conary operations against a live Remi server:

| Range | Category | Tests |
|-------|----------|-------|
| T01 | Health check | Remi endpoint reachable |
| T02-T04 | Repository | Add, list, sync |
| T05-T06 | Search | Package search |
| T07-T12 | Install/Remove | Install, verify files, list, remove, verify cleanup |
| T13-T17 | Package Info | Version, info, file list, path ownership |
| T18-T19 | Multi-package | Install second package, verify coexistence |
| T20-T21c | Adopt/Unadopt | Preview/adopt system package, check status, dry-run/apply single-package unadopt, re-adopt, dry-run/apply all-package unadopt |
| T22-T23 | Pin | Pin/unpin package |
| T24 | History | Changeset history |
| T25-T27 | Dependencies | Install with deps, verify, multi-package coexist |
| T28, T30 | Recorded ownership modes | Preserve, explicit takeover |
| T32 | Update | Update with adopted packages; native PM authority remains in effect unless takeover is explicit |
| T33-T37 | Generations | List, GC, info, takeover dry-run, composefs format |

`T21a`, `T21b`, and `T21c` are the Adopt Without Regret proof points for
Phase 1. When `phase1-advanced` is run on `fedora44`, `ubuntu-26.04`, and
`arch`, they prove that `curl` can be unadopted one package at a time and with
`--all` without deleting native package files: `curl --version` still works,
while `conary list curl` no longer reports Conary tracking.

The focused CLI regression
`apps/conary/tests/live_host_mutation_safety.rs` runs
`conary system adopt <pkg> --dry-run` through a query-only fake native package
manager. It snapshots every SQLite schema entry and table cell plus the
surrounding test filesystem before and after the real binary runs. The test
proves package identity and planned record counts are rendered without an
acknowledgement prompt or changes to SQLite, checkpoints, CAS, native package
manager state, hooks, generations, or live-root paths.

The focused CLI integration test
`apps/conary/tests/native_pm_live_root.rs` complements the manifest matrix for
source-owned update policy. Its package mutations now prove selected-root
publication from an initially generation-free DB/CAS state. The security cases
still prove refusal before mutation when a repository cannot prove advisory
metadata support and successful application from a trusted JSON advisory
fixture with advisory ID, CVE, fixed version, and source output.

### Phase 2: Deep E2E (T38-T76)

Requires test fixture packages published to Remi.

| Group | Range | Category |
|-------|-------|----------|
| A | T38-T50 | Deep install flow (fixture packages, update, rollback, orphans, pin) |
| B | T51-T57 | Generation lifecycle (build, list, switch, rollback, GC, ready-to-activate takeover) |
| C | T58-T61 | Bootstrap pipeline (dry-run, stage 0) |
| D | T62-T66 | Recipe & build (cook, PKGBUILD convert, hermetic build) |
| E | T67-T71 | Remi client (sparse index, chunk fetch, OCI manifests) |
| F | T72-T76 | Self-update (channel get/set/reset, version check, mock server) |

### Phase 3: Adversarial (Groups G-P)

Adversarial and stress tests.

| Group | Category |
|-------|----------|
| G-M | Container-based adversarial tests |
| N (container) | Container-based adversarial tests |
| N (QEMU) | Kernel and boot QEMU tests |
| O (QEMU) | Generation artifact export QEMU tests |
| P (QEMU) | ISO generation export QEMU tests |
| Composefs modernization (QEMU) | Focused atomic-generation fail-closed checks |
| Active generation handoff | Selected-generation native authority handoff proof |

### Phase 4: Feature Validation

Phase 4 currently contains 155 tests across eight manifests. It validates the
active, user-facing command surface and checks that claimed features still match
the current binary. Where a flow is intentionally preview-only or not yet
implemented, the manifest asserts that it fails cleanly with an explicit
message rather than pretending it is production-ready.

| Suite | IDs | Count | Category |
|-------|-----|-------|----------|
| A | T160-T176 | 17 | Config, distro, canonical, groups, registry |
| B | T177-T195 plus suffix IDs | 20 | Label, model, collection, derive |
| C | T196-T220 plus suffix IDs | 27 | CCS ops, query, repo management |
| D | T221-T255 plus suffix IDs | 38 | Provenance, capability, trust, system ops, federation, automation |
| E | T256-T273 and T276-T277 plus suffix IDs | 22 | Cross-source compatibility overlay: native package parity, source identity, model convergence, and takeover |
| Native package-manager parity | TNPM01-TNPM18 plus TNPM02X | 19 | Cross-distro native PM parity and daily-driver corpus |
| Native cross-source lifecycle | TNPMX01R, TNPMX01D, TNPMX01A | 3 | Attributable native-oracle install/update/rollback/remove corpus evidence on every target image |
| Security advisory pipeline | TSEC01-TSEC07 | 7 | Trusted advisory ingestion and security update proof |

Phase 4 is intentionally mixed:

- Positive-path coverage proves real flows such as tracked-config backup/restore,
  read-only `conary distro` diagnostics, label mutation, trigger mutation,
  selective CCS component installs, native local RPM/DEB/Arch installs, TUF
  bootstrap with a signed test root, provenance diff, pinned-fingerprint
  federation peers, model package convergence, ready-to-activate takeover,
  and the cross-distro takeover ownership ladder.
- Preview-only flows are still exercised, but the assertions check for the
  expected explanatory output. Current examples include empty automation
  history and persisting automation configuration changes.

Group E is intentionally richer than a simple “portability” smoke test. It
covers canonical mapping, repository-scoped source identity, read-only affinity
diagnostics, package ownership convergence, takeover across source ecosystems,
and native-format package handling on the host distribution.

In addition to the container-backed suites, `apps/conary/tests/bootstrap_workflow.rs`
exercises the `conary` binary directly for manifest-run record loading,
`bootstrap verify-convergence`, and `bootstrap diff-seeds` using synthetic
completed-run metadata. Those tests do not replace the container suites, but
they do keep the command contracts green even when a container runtime is not
available.

## QEMU Boot Tests

Tests requiring kernel/boot file deployment use the `qemu_boot` step type:

```toml
[[test.step]]
[test.step.qemu_boot]
image = "minimal-boot-v1"
memory_mb = 2048
timeout_seconds = 240
ssh_port = 2222
commands = ["uname -r", "ls /boot/vmlinuz*"]
expect_output = ["vmlinuz"]
```

QEMU images are downloaded from `https://remi.conary.io/test-artifacts/` and
cached locally. Plain `conary-test` runs report an explicit skipped result when
host tools or remote images are unavailable, and
`scripts/local-qemu-validation.sh` treats any skipped QEMU result as a failed
release gate while separately requiring boot/export markers in the logs. Keep
that wrapper pointed only at published or generated fixtures that must be
reproducible on a KVM-capable development host.

The enclosing test `timeout`, or a `[[test.step]] timeout` override, is one
monotonic deadline for the complete `qemu_boot` step. Boot readiness, system
readiness, Conary staging, every guest copy and command, and shutdown consume
that same budget. `qemu_boot.timeout_seconds` remains a per-operation cap; it
can shorten one operation but never resets or extends the enclosing step
deadline. Deadline expiry kills the QEMU child and any owned host-helper
process group, then records exit code 124 as a failed test result.

Generated-image suites can attach a scratch disk, copy files into or out of a
guest, and then boot a host-local qcow2 or ISO produced by an earlier step:

```toml
[[test.step]]
[test.step.qemu_boot]
image = "fedora44-guest-v2"
scratch_disk_mb = 65536
local_image_path = "/tmp/conary-generation-export/generated.qcow2"
copy_to_guest = [
  { source = "/tmp/input.txt", dest = "/tmp/input.txt" },
]
copy_from_guest = [
  { source = "/tmp/out.qcow2", dest = "/tmp/conary-generation-export/out.qcow2" },
]
commands = ["true"]
```

When `local_image_path` is set, `image` remains a required logical name but the
engine uses the local path directly instead of downloading a cached Remi
artifact. `scratch_disk_mb` adds a virtio scratch disk for large exports, and
`copy_from_guest.dest` parent directories are created automatically.

Set `image_format = "iso"` to boot a host-local ISO with QEMU `-cdrom`; omit it
for the default qcow2 path. `image_format = "raw"` is also accepted for raw disk
images.

## Configuration

All test parameters live in `apps/conary/tests/integration/remi/config.toml`:

```toml
[remi]
endpoint = "https://remi.conary.io"

[paths]
db = "/var/lib/conary/conary.db"
conary_bin = "/usr/bin/conary"
results_dir = "/results"
fixture_dir = "/opt/remi-tests/fixtures"

[distros.fedora44]
remi_distro = "fedora-44"
repo_name = "remi"
test_package = "which"
test_binary = "/usr/bin/which"
# ... more test packages
```

### Environment Overrides

Override any config value via environment variables:

| Variable | Overrides |
|----------|-----------|
| `REMI_ENDPOINT` | `[remi] endpoint` |
| `DB_PATH` | `[paths] db` |
| `CONARY_BIN` | `[paths] conary_bin` |
| `RESULTS_DIR` | `[paths] results_dir` |
| `DISTRO` | Which `[distros.*]` section to use |

For admin-backed operations such as health checks, log queries, and fixture
publishing, also set. The retained result-streaming path will use the same
configuration once issue #354 wires it into local runs:

| Variable | Purpose |
|----------|---------|
| `REMI_ADMIN_ENDPOINT` | Base URL for the Remi admin REST API |
| `REMI_ADMIN_TOKEN` | Bearer token for the Remi admin API |

## Results

Test results are written as JSON under
`apps/conary/tests/integration/remi/results/`, using filenames such as
`<distro>-phase<N>.json`:

```json
{
  "suite_name": "phase-1",
  "phase": 1,
  "status": "completed",
  "summary": {
    "total": 1,
    "passed": 1,
    "failed": 0,
    "skipped": 0,
    "cancelled": 0
  },
  "results": [
    {
      "id": "T01",
      "name": "health_check",
      "status": "passed",
      "duration_ms": 206,
      "message": null,
      "stdout": null,
      "stderr": null,
      "attempts": []
    }
  ]
}
```

## Result Persistence

The Remi result-streaming and SQLite WAL path (`/tmp/conary-test-wal.db`) is
retained, but it is currently unconstructed for local CLI runs. Local runs
write their JSON reports locally and do not stream results to Remi or populate
the WAL. Wiring the streaming path is tracked in issue #354.

## CI Integration

Trusted integration validation belongs to GitHub Actions, with any runner used
as execution capacity rather than as an independent control plane. The PR gate
runs the focused native cross-source lifecycle on hosted Docker across all six
distro images and the derivative-specific APT gate on Mint and Pop!_OS. The
rest of the TOML inventory still requires a local or
hosted container/QEMU-capable runner; do not describe a normal PR or merge run
as having executed all 324 manifest tests unless that runner path is present in
the specific workflow run.

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `pr-gate` | Pull request + manual dispatch | Unit/static gates plus the focused six-distro native lifecycle matrix and authentic derivative APT proof |
| `merge-validation` | Every push to `main` + manual dispatch | Trusted on-merge smoke validation for `conary`, `remi`, `conaryd`, and `conary-test` |
| `release-artifact-proof` | Conary deployment + manual dispatch | Install each published native package and run the three-distro Cartesian lifecycle with those exact bytes |
| `scheduled-ops` | Nightly/scheduled + manual dispatch | Deep validation, health checks, and scheduled operational audits |

`conary-test deploy status` is internal infrastructure state, not a product
release identity. Operators should read it as local checkout and managed-rollout
provenance; the removed conary-test server no longer provides a live service
identity, and no deploy execution command remains.

Current JSON semantics:

- `conary-test deploy status --json` computes binary metadata from the local
  invoking binary and reports checkout plus managed-rollout provenance. It has
  no remote-runner or HTTP-degraded branch.
- `conary-test health --json` always returns valid JSON with local
  `deploy_status`, `mode`, optional `remi`, and optional `reason`.

## Adding Tests

1. Create or edit a TOML manifest in `apps/conary/tests/integration/remi/manifests/`
2. Define test steps using the manifest schema (run, assert, mock_server, etc.)
3. For a local proof, run `cargo run -p conary-test -- list`
4. For deeper manual debugging, run `cargo run -p conary-test -- run --suite <manifest> --distro <distro> --phase <N>`

## Adding Distros

1. Create `apps/conary/tests/integration/remi/containers/Containerfile.<name>`
   or select a shared capability-driven Containerfile.
2. Add `[distros.<name>]` to `config.toml` with an explicit typed
   `build_context = "static-binary"` or `build_context = "binary"`
3. For reconstructed derivative roots, declare the digest-pinned base,
   authenticated media, exact identity/keyring packages, APT declaration
   fixtures, signing roots, and exact legacy global-trust bindings when used.
4. Add the target to CI workflow matrices and typed workflow tests.

## Troubleshooting

**"cannot start a transaction within a transaction"** during repo sync:
Fixed in commit 942c4b2. If seen again, check that `batch_insert()` doesn't nest transactions.

**"unexpected argument '--db-path'":**
The subcommand doesn't accept `--db-path`. Check `apps/conary/src/cli/` to see which subcommands have `DbArgs`.

**Remote test-fixture downloads return 404:**
Build and publish the fixture corpus to Remi's dedicated test-fixture surface:
```bash
./scripts/publish-test-fixtures.sh
```
