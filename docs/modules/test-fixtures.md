---
last_updated: 2026-08-12
revision: 31
summary: Map fixture ownership, including authenticated Debian-derivative roots and cross-source lifecycle proof
---

# Test Fixtures And Proof Maps

This module records fixture families that future contributors and agents can
treat as stable proof surfaces. It does not replace the tests themselves. It
answers where a fixture lives, what behavior it proves, which tests consume it,
and which verification command is the right first gate.

CCS means Conary Content Store in this map.

## Fixture Map Schema

Each fixture family should record:

- **Family ID:** stable lowercase id used by child plans.
- **Owner:** subsystem and first source file to inspect.
- **Purpose:** behavior the fixture proves.
- **Fixture sources:** checked-in files or in-test builders.
- **Consumes:** tests or commands that use the fixtures.
- **Fast proof:** narrow local command for small edits.
- **Medium proof:** package-level or cross-package command.
- **Slow proof:** integration or QEMU command when applicable.
- **Regeneration:** command or hand-maintained status.
- **Safety notes:** public-target, scriptlet, trust, host mutation, private-path,
  or publication boundaries.

## Remi And CCS Conversion/Publication Fixture Families

| Family ID | Owner | Fast proof |
|-----------|-------|------------|
| `foreign-package-lifecycle-contracts` | Source parsers, CCS lifecycle bundle, and transaction planner | `cargo test -p conary-core native_abi`; `cargo test -p conary-core native_lifecycle`; `cargo test -p conary-core native_transaction` |
| `host-executable-interface-fixtures` | Core test support and checked-in host tools | `cargo test -p conary-core bootstrap::guest_profile::tests`; `cargo test -p conary-core ccs::hooks` |
| `ccs-v3-native-authority-fixtures` | CCS v3 native authority | `cargo test -p conary-core ccs::v3`; `cargo test -p conary --test packaging_m4a` |
| `ccs-v3-local-authoring-smoke` | CCS v3 local authoring | `cargo test -p conary --test packaging_m4b` |
| `ccs-v3-lifecycle-authoring-proof` | CCS v3 lifecycle authoring | `cargo test -p conary --test packaging_m4e`; `cargo test -p conary-core ccs::v3` |
| `m4d-supported-profile-cutover` | Repository feed profiles | `cargo test -p conary-core supported_profiles`; `cargo test -p conary --test packaging_m4d`; `cargo test -p remi route` |
| `native-lifecycle-query-fixtures` | Native lifecycle query surfaces | `cargo test -p conary --test query_scripts` |
| `rpm-hardlink-transaction` | RPM payload projection and CCS conversion | `cargo test -p conary-core packages::rpm::payload`; `cargo test -p conary --test conversion_integration golden_conversion` |
| `rpm-root-anchor-conversion` | RPM root ownership parsing and CCS omission | `cargo test -p conary-core packages::rpm::payload`; opt-in pinned-artifact test below |
| `remi-native-ccs-publication` | Remi native publication | `cargo test -p remi release_upload_`; `cargo test -p conary --test packaging_m4c` |
| `remi-converted-artifact-serving` | Remi conversion persistence and serving | `cargo test -p remi conversion` |
| `remi-test-artifact-fixtures` | Remi artifact handlers | `cargo test -p remi test_upload_fixture`; `cargo test -p remi test_public_fixture_get_and_head` |
| `qemu-source-image-fixtures` | Bootstrap/QEMU fixture maintenance | `bash -n scripts/bootstrap-vm/rotate-qemu-test-identity.sh`; `cargo run -p conary-test -- list` |
| `supported-host-generation-export` | Generation export boot proofs | `cargo test -p conary-core --test supported_host_generation_export_fixture_contract` |
| `conary-test-remi-manifests` | Integration harness | `cargo run -p conary-test -- list`; `cargo test -p conary-test suite_inventory` |

### foreign-package-lifecycle-contracts

- **Owner:** source ABI:
  `crates/conary-core/src/packages/native_abi.rs`; format parsers under
  `crates/conary-core/src/packages/{rpm,deb,arch}/`; durable bundle:
  `crates/conary-core/src/ccs/native_lifecycle.rs`; transaction planner:
  `crates/conary-core/src/ccs/native_transaction.rs`.
- **Purpose:** Preserve the complete RPM, Debian, and Arch lifecycle ABI and
  prove exact event selection, order, arguments, trigger input, and payload
  boundaries without deriving correctness from program-text heuristics.
- **Fixture sources:** parser fixtures beside each format implementation;
  bundle round-trip fixtures under
  `crates/conary-core/src/ccs/native_lifecycle/tests.rs`; transaction graphs
  under `crates/conary-core/src/ccs/native_transaction/tests.rs`.
- **Consumes:** source parser, conversion, bundle validation, install preflight,
  and transaction planner tests.
- **Fast proof:** `cargo test -p conary-core native_abi`;
  `cargo test -p conary-core native_lifecycle`;
  `cargo test -p conary-core native_transaction`.
- **Medium proof:** `cargo test -p conary-core ccs::convert`;
  `cargo test -p conary --test conversion_integration`.
- **Slow proof:** Run the appropriate native-package-manager parity suite when
  payload visibility or live-root behavior changes.
- **Regeneration:** Package fixtures are generated from documented
  package-manager metadata; hand-authored planner fixtures name the exact
  source lifecycle slot and typed transaction inputs.
- **Safety notes:** Command evidence and risk classification may accompany a
  fixture, but neither is expected-output authority. A fixture for an
  missing executable semantic must assert a typed preflight failure before
  payload or database mutation and remain required implementation work.

### rpm-hardlink-transaction

- **Owner:** RPM payload projection:
  `crates/conary-core/src/packages/rpm/payload/hardlinks.rs`; archive order:
  `crates/conary-core/src/packages/rpm/payload/stream.rs`; persisted CCS proof:
  `apps/conary/tests/conversion_integration/authority.rs`.
- **Purpose:** Prove that RPM hardlink sets group by device/inode, retain
  archive order, and project the last packaged header-index member as the
  content-bearing transaction completion and effective inode metadata owner.
  Per-path header evidence is parsed without requiring invented metadata
  equality; malformed counts, digest disagreement, conflicting non-authority
  payload bytes, impossible identities, and missing completion still fail
  closed.
- **Fixture source:** Fedora 44
  `2ping-4.5.1-24.fc44.noarch.rpm` at
  `crates/conary-core/tests/fixtures/rpm/`, pinned by SHA-256
  `cf48a9380416daf02e934cbddd15d5356b3b6ea6b5f0824187074b66dd0fe14a`.
- **Consumes:** RPM hardlink projection unit tests, the pinned parser
  regression, and the signed CCS golden conversion round-trip.
- **Fast proof:** `cargo test -p conary-core packages::rpm::payload`.
- **Medium proof:** `cargo test -p conary-core packages::rpm`;
  `cargo test -p conary --test conversion_integration golden_conversion`.
- **Slow proof:** Install the pinned fixture in a disposable Fedora 44 root
  with native `rpm`, capture `stat` plus `rpm -V`, and compare the observed
  inode owner metadata with the projected CCS node.
- **Regeneration:** Replace the binary only with an artifact having the exact
  pinned checksum, then rerun both parser and conversion gates.
- **Safety notes:** The fixture is public package data. Native-oracle commands
  run only in a disposable test image/root; never install it into the host,
  production Remi, `/conary/data`, or `/etc/conary`.

### rpm-root-anchor-conversion

- **Owner:** RPM header and CPIO projection:
  `crates/conary-core/src/packages/rpm/payload/{header,stream}.rs`; CCS omission:
  `crates/conary-core/src/packages/rpm/payload.rs`.
- **Purpose:** Prove that RPM's exact `/` ownership entry remains part of
  source header/payload validation but cannot become selected-root install or
  remove authority.
- **Fixture source:** Fedora 44
  [`filesystem-3.18-52.fc44.x86_64.rpm`](https://dl.fedoraproject.org/pub/fedora/linux/releases/44/Everything/x86_64/os/Packages/f/filesystem-3.18-52.fc44.x86_64.rpm),
  downloaded only for the opt-in test. Exact size: 1,398,770 bytes. SHA-256:
  `7abc643d653bf2773c38e183e0e33489cab4880c8a540b315fcd7fff09c6c52c`.
  Unit tests construct the neighboring malformed metadata and CPIO path forms
  without network access.
- **Consumes:** RPM payload unit tests and
  `pinned_fedora_filesystem_root_anchor_parses_and_converts` in
  `crates/conary-core/tests/native_abi.rs`.
- **Fast proof:** `cargo test -p conary-core packages::rpm::payload`.
- **External artifact proof:** set `CONARY_RPM_ROOT_ANCHOR_FIXTURE` to the
  exact pinned RPM, then run `cargo test -p conary-core --test native_abi
  pinned_fedora_filesystem_root_anchor_parses_and_converts -- --ignored`.
- **Medium proof:** `cargo test -p conary --test conversion_integration
  golden_conversion`; `cargo test -p remi conversion`.
- **Regeneration:** Do not replace the pinned identity in response to repository
  drift. Add a separately identified artifact when a later RPM grammar needs
  proof.
- **Safety notes:** Download and parse/convert only. Do not install the native
  RPM on the host or let its root metadata modify a test destination.

### host-executable-interface-fixtures

- **Owner:** typed linker and fixture catalog:
  `crates/conary-core/src/test_support.rs`; stable executable sources:
  `crates/conary-core/tests/fixtures/host-tools/`.
- **Purpose:** Exercise guest-profile key generation and the exact systemd,
  sysusers, tmpfiles, sysctl, and ldconfig executable-interface contracts
  without writing an executable inode immediately before production code
  launches it.
- **Fixture sources:** checked-in executable shell fixtures selected through
  `HostToolFixture`; each test creates only a per-case symlink and ordinary
  target-root output. The systemd fixture records systemctl argv under
  `var/lib/conary-test/systemctl-calls` and copies the selected tmpfiles
  declaration to
  `var/lib/conary-test/systemd-tmpfiles-captured.conf`.
- **Consumes:** `bootstrap::guest_profile::tests`;
  `ccs::hooks::capabilities::tests`;
  `ccs::hooks::capabilities::interface_contract::tests`;
  `ccs::hooks::systemd::tests`; and `ccs::hooks::tmpfiles::tests`.
- **Fast proof:** `cargo test -p conary-core
  bootstrap::guest_profile::tests`; `cargo test -p conary-core ccs::hooks`.
- **Medium proof:** `cargo test -p conary-core --lib`.
- **Slow proof:** the pull-request `workspace-tests` job under parallel hosted
  CI.
- **Regeneration:** hand-maintained, typed fixture selection. Add a distinct
  checked-in executable when behavior or executable identity must differ; do
  not generate executable shell files during the consuming test.
- **Safety notes:** These files are test-only host command doubles. They do not
  define production capability support, weaken executable digest/version/path
  revalidation, or authorize host mutation. Do not replace this boundary with
  retries, sleeps, test serialization, or error-string exceptions.

### ccs-v3-native-authority-fixtures

- **Owner:** CCS v3 contract:
  `crates/conary-core/src/ccs/v3/`; archive/package routing:
  `crates/conary-core/src/ccs/archive_reader.rs` and
  `crates/conary-core/src/ccs/package.rs`.
- **Purpose:** Signed native CCS v3 authority, exact-byte signature
  verification, verified install parsing, publish-gate compatibility, and
  fail-closed rejection of format-v1 or default-reconstructed authority.
- **Fixture sources:** in-test builders under
  `crates/conary-core/src/ccs/v3/test_support.rs`;
  `apps/conary/tests/packaging_m4a.rs`; targeted unit fixtures in
  `crates/conary-core/src/ccs/{archive_reader,package,verify}.rs`.
- **Consumes:** CCS v3 schema/reader/validation/identity tests, verifier tests,
  static publish-gate tests, and M4a CLI install integration tests.
- **Fast proof:** `cargo test -p conary-core ccs::v3`;
  `cargo test -p conary --test packaging_m4a`.
- **Medium proof:** `cargo test -p conary-core ccs::verify`;
  `cargo test -p conary-core repository::static_repo::publish_gate`.
- **Slow proof:** No slow gate for fixture-map-only changes.
- **Regeneration:** Hand-maintained Rust builders until M4b authoring emits
  native v3 packages directly.
- **Safety notes:** v3 native fixtures are signed `format_version = 3`
  authority with complete file, component, dependency, provenance,
  TOML-debug-hash, and content-identity coverage. The sole format-v1 rejection
  fixture is a hand-authored CBOR header; the retired writer, schema,
  projection, and general fixture factory do not remain in the tree.

### ccs-v3-local-authoring-smoke

- **Owner:** CCS v3 local authoring commands:
  `apps/conary/src/commands/ccs/{templates.rs,lint.rs,build.rs,test.rs,local_dev.rs}`;
  authority projection: `crates/conary-core/src/ccs/v3/authoring.rs`.
- **Purpose:** Minimal-file native authoring loop from `ccs.toml` through lint,
  local-dev or explicit-key v3 build, local-dev verify, isolated dry-run test,
  typed source-to-install-prefix mapping without implicit ancestor ownership,
  and static publish rejection for local-dev/host-hardened artifacts.
- **Fixture sources:** in-test project builder in
  `apps/conary/tests/packaging_m4b.rs`.
- **Consumes:** M4b CLI smoke, signing guardrail, source-independent
  declarative lifecycle, typed dependency authoring, local-dev trust, and
  isolated dry-run tests.
- **Fast proof:** `cargo test -p conary --test packaging_m4b`.
- **Medium proof:** `cargo test -p conary-core ccs::v3`;
  `cargo test -p conary-core repository::static_repo::publish_gate`.
- **Slow proof:** No slow gate for M4b fixture-map-only changes.
- **Regeneration:** Temporary source trees are generated during tests.
- **Safety notes:** Local-dev keys are isolated with test HOME/XDG directories.
  Local-dev v3 artifacts are for local verify/test only and must remain
  rejected by static publish and Remi release trust.

### ccs-v3-lifecycle-authoring-proof

- **Owner:** CCS v3 native authoring commands:
  `apps/conary/src/commands/ccs/`; v3 projection/validation:
  `crates/conary-core/src/ccs/v3/authoring.rs`;
  debug projection: `crates/conary-core/src/ccs/v3/debug_projection.rs`;
  repository feed catalog:
  `crates/conary-core/src/repository/supported_profiles/`.
- **Purpose:** M4e proof that config-only and lifecycle-bearing native packages
  author without a destination distro gate, debug TOML remains a checked
  projection of signed authority, and declarative lifecycle remains signed
  source-independent intent.
- **Fixture sources:** generated `minimal-file`, `config-noreplace`, and
  `service` projects in `apps/conary/tests/packaging_m4b.rs` and
  `apps/conary/tests/packaging_m4e.rs`; debug projection unit fixtures in
  `crates/conary-core/src/ccs/v3/debug_projection.rs`.
- **Consumes:** M4e CLI lint/build/verify/test corpus, arbitrary declarative
  lifecycle, and v3 reader/debug-projection consistency tests.
- **Fast proof:** `cargo test -p conary --test packaging_m4e`;
  `cargo test -p conary-core ccs::v3::debug_projection`.
- **Medium proof:** `cargo test -p conary-core ccs::v3`;
  `cargo test -p conary-core supported_profiles`;
  `cargo test -p conary --test packaging_m4a`;
  `cargo test -p conary --test packaging_m4b`;
  `cargo test -p conary --test packaging_m4d`.
- **Slow proof:** No slow gate for M4e fixture-map-only changes.
- **Regeneration:** Temporary source projects are generated during tests.
- **Safety notes:** Repository feed IDs and route slugs are not destination
  compatibility authority. Debug TOML is never authoritative; it must match
  signed CBOR config and lifecycle authority exactly.

### m4d-repository-feed-catalog

- **Owner:** Repository feed catalog:
  `crates/conary-core/src/repository/supported_profiles/`; CLI smoke:
  `apps/conary/tests/packaging_m4d.rs`; Remi route proof:
  `apps/remi/src/server/handlers/`.
- **Purpose:** Prove the three currently configured upstream feed IDs
  (`fedora-44`, `ubuntu-26.04`, and `arch`), route/feed agreement for `fedora`,
  `ubuntu`, and `arch`, and exact parser/version-scheme selection.
- **Fast proof:** `cargo test -p conary-core supported_profiles`;
  `cargo test -p conary --test packaging_m4d`;
  `cargo test -p remi route`.
- **Medium proof:** `cargo test -p conary-core ccs::v3`;
  `cargo test -p remi conversion`;
  `cargo test -p conary-core remi_sync`.
- **Safety notes:** `debian` is a valid version-scheme string for Ubuntu
  package comparison, not a configured feed or Remi route slug. The catalog
  does not enumerate destination operating systems supported by Conary.

### remi-native-ccs-publication

- **Owner:** Remi native publication:
  `apps/remi/src/server/native_publish/`; release upload route/staging:
  `apps/remi/src/server/release_publish.rs`.
- **Purpose:** Release-eligible CCS v3 artifacts published through local Remi
  without conversion-shaped storage, including source-independent lifecycle
  authority validated structurally rather than by a route allowlist.
- **Fixture sources:** in-test release-eligible v3 builders in
  `apps/remi/src/server/release_publish.rs` and
  `apps/conary/tests/packaging_m4c.rs`.
- **Consumes:** native release upload, arbitrary declarative lifecycle upload,
  local-dev publish-gate refusal,
  replacement, public metadata/download, client dry-run install,
  sparse/search/index, and chunk-GC tests.
- **Fast proof:** `cargo test -p remi release_upload_`;
  `cargo test -p conary --test packaging_m4c`.
- **Medium proof:** `cargo test -p remi native_publish`;
  `cargo test -p remi publication`;
  `cargo test -p remi metadata_includes_native_only_package_as_native_not_converted`;
  `cargo test -p remi sparse_index_preserves_native_sibling_releases`;
  `cargo test -p remi search_rebuild_preserves_native_release_identity_and_converted_false`.
- **Slow proof:** No cloud or QEMU proof is required for local native
  publication; run `cargo test -p remi` when public serving behavior changes.
- **Regeneration:** Temporary signed and attested v3 packages are generated in
  Rust tests.
- **Safety notes:** fixtures must prove no `converted_packages` row is written,
  local-dev or otherwise publish-gate-rejected artifacts write no public state,
  failed replacement preserves the last public native row, and active native
  chunks remain protected from serving and garbage collection regressions.
  Repository routes must never act as destination compatibility allowlists.

### native-lifecycle-query-fixtures

- **Owner:** lifecycle query command:
  `apps/conary/src/commands/query/scripts.rs`; fixture builders and integration
  assertions: `apps/conary/tests/query_scripts.rs`.
- **Purpose:** Prove that package and installed-state queries expose the
  current typed lifecycle bundle, entry digests, source slots, and diagnostic
  effects without inventing execution or publication policy.
- **Fixture sources:** local builders in
  `apps/conary/tests/query_scripts.rs`.
- **Consumes:** `apps/conary/tests/query_scripts.rs`.
- **Fast proof:** `cargo test -p conary --test query_scripts`.
- **Medium proof:** `cargo test -p conary commands::query`.
- **Slow proof:** No slow gate for query-only fixture changes.
- **Regeneration:** Hand-maintained Rust builders validated against the current
  lifecycle schema.
- **Safety notes:** Redact private paths and environment values in diagnostic
  output. Query fixtures do not define planner order or serving eligibility;
  the typed transaction fixtures and Remi serving tests own those contracts.

### remi-converted-artifact-serving

- **Owner:** conversion persistence:
  `apps/remi/src/server/conversion/persistence.rs`; artifact reachability:
  `apps/remi/src/server/publication.rs`; public handlers under
  `apps/remi/src/server/handlers/`.
- **Purpose:** Prove that a successfully converted current-schema artifact and
  its reachable chunks appear consistently in index, detail, search, sparse,
  and download surfaces.
- **Fixture sources:** `apps/remi/src/server/conversion/test_support.rs`;
  conversion, index, search, prewarm, and handler test builders.
- **Consumes:** conversion persistence, generated-index, sparse, detail,
  search, chunk serving, and prewarm tests.
- **Fast proof:** `cargo test -p remi conversion`.
- **Medium proof:** `cargo test -p remi`.
- **Slow proof:** Use a deployed Remi probe only when route or storage behavior
  changes.
- **Regeneration:** Reconvert authoritative package inputs into a fresh
  current-epoch database.
- **Safety notes:** Lifecycle summary diagnostics are not serving gates.
  Serving still validates current schema, object identity, path safety, and CAS
  reachability; public diagnostic responses remain privacy-normalized.

### remi-test-artifact-fixtures

- **Owner:** Remi artifact handlers:
  `apps/remi/src/server/handlers/admin/artifacts.rs`.
- **Purpose:** Upload and serve static test fixture artifacts through admin and
  public routes.
- **Fixture sources:** `apps/remi/src/server/handlers/admin/artifacts.rs`;
  `apps/remi/src/server/handlers/artifacts.rs`;
  `apps/remi/src/server/artifact_paths.rs`.
- **Consumes:** Admin upload tests, public fixture GET/HEAD tests, audit action
  tests.
- **Fast proof:** `cargo test -p remi test_upload_fixture`;
  `cargo test -p remi test_public_fixture_get_and_head`.
- **Medium proof:** `cargo test -p remi artifacts`.
- **Slow proof:** No slow gate for map-only changes.
- **Regeneration:** Generated in temporary directories during tests.
- **Safety notes:** Keep path traversal rejection and fixture-size limits
  intact.

### conary-test-remi-manifests

- **Owner:** Integration harness: `apps/conary-test/src/config/`,
  `apps/conary-test/src/engine/corpus.rs`, `apps/conary-test/src/report/`, and
  `apps/conary-test/src/suite_inventory.rs`.
- **Purpose:** Declarative Remi and package-manager integration suites. Corpus
  tests declare exact source/target/stage and artifact-digest authority,
  consume a versioned runtime evidence file, and emit `CorpusCaseResult` plus
  typed aggregation with an exact declared-case count; generic stdout and
  error messages remain diagnostics only.
- **Fixture sources:** `apps/conary/tests/integration/remi/manifests/`;
  `apps/conary/tests/integration/remi/containers/`;
  `apps/conary/tests/fixtures/conary-test-fixture/`;
  `apps/conary/tests/fixtures/native/`;
  `apps/conary/tests/fixtures/native-lifecycle-parity/`;
  `apps/conary/tests/fixtures/distro-roots/`;
  `apps/conary/tests/fixtures/phase4-runtime-fixture{,-v2}/`;
  `apps/conary/tests/fixtures/native-selected-root-layout/`;
  `apps/conary/tests/fixtures/adversarial/`; disposable signing authority and
  policy under `apps/conary/tests/fixtures/ccs-test-authority/`.
- **Consumes:** `cargo run -p conary-test -- list`, manifest parser tests,
  suite runner, local QEMU validation scripts.
- **Fast proof:** `cargo run -p conary-test -- list`;
  `cargo test -p conary-test suite_inventory`;
  `cargo test -p conary-test focused_native_cross_source_manifest_runs_the_shared_lifecycle_contract`;
  `cargo test -p conary-test native_cross_source_`.
- **Medium proof:**
  `cargo test -p conary-test config::tests::test_load_phase1_core_manifest`;
  `cargo test -p conary-test config::tests::test_load_phase3_group_m_manifest_installs_local_fixture_ccs`.
- **Slow proof:** Suite-specific commands such as
  `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4`
  and
  `cargo run -p conary-test -- run --suite native-cross-source-lifecycle --distro fedora44 --phase 4`
  when behavior changes require live integration proof. Run the focused
  lifecycle suite on every configured target for complete target-image
  coverage. Linux Mint 22.3 and Pop!_OS 24.04 additionally run
  `debian-derivative-acceptance`, which exercises their actual APT declarations,
  trust roots, native package adoption, and repository takeover. `fedora44` is the
  existing `conary-test` runner distro key; public CCS target IDs remain
  `fedora-44`, `ubuntu-26.04`, and `arch`.
  Published artifacts use
  `cargo run -p conary-test -- images build --distro <distro> --native-package <path>`
  before the same focused suite. Only
  `.github/workflows/release-artifact-proof.yml` binds that package to
  published metadata, `SHA256SUMS`, and the GitHub digest.
- **Regeneration:** Manifests are hand-maintained TOML. Rotate the test-only
  authority with `apps/conary/tests/fixtures/ccs-test-authority/generate.sh`,
  then rebuild both `conary-test-fixture/build-all.sh` and
  `adversarial/build-all.sh` before running the parser/list proof. Fixture
  packages may also be built or published through `conary-test fixtures`
  commands documented in `docs/INTEGRATION-TESTING.md`. Suite result JSON is
  generated locally under the ignored
  `apps/conary/tests/integration/remi/results/` directory.
- **Safety notes:** Treat manifest schema and semantics as persisted test
  configuration; changes need parser/list proof and an explicit migration or
  defaulting decision. Every successful CCS fixture build is signed with the
  one disposable fixture key and every install or verify names its generated
  trust policy. Directory labels `v1` and `v2` are package versions 1 and 2,
  not retired/current CCS format identifiers. Corrupt and malicious archive
  cases start from signed current authority and mutate afterward. Scriptlet
  failure tests require nonzero install status plus absent package state and
  payload; degraded installed-state success is not a fixture contract. Native
  cross-source lifecycle assertions read selected-generation manifests and CAS
  objects; the container's incidental root filesystem is not package-state
  authority. Fedora/RPM, Ubuntu/dpkg, and Arch/pacman each capture the fixture's
  exact ordered argv/stdin/payload-boundary trace and byte-verify the matching
  `native-lifecycle-parity/expected/` contract before that contract is used for
  all nine source-format/target-profile Conary rows. Each manifest lane supplies
  its native oracle format explicitly; fixture execution does not infer
  authority from distro-name matching. The typed workflow test owns the three
  source-format cases across all six required target lanes and the stable
  all-lane aggregator context; do not weaken either in workflow-only edits.
  Derivative roots are assembled from digest-pinned transport images, exact
  release identity/keyring packages, and byte-pinned APT declarations captured
  from authenticated release media. Product code must not branch on their
  distro names. A successful derivative run records typed `target_release`
  artifact identities, signing fingerprints, and preflight stages; configuration
  alone is not acceptance evidence. Artifact digest provenance remains typed as
  pinned release authority, pinned build input, or running-target bytes.

### supported-host-generation-export

- **Owner:** `apps/conary/tests/fixtures/supported-host-generation-export/`;
  contract test
  `crates/conary-core/tests/supported_host_generation_export_fixture_contract.rs`.
- **Purpose:** Prove that Conary's bootable-image export works from a root
  assembled entirely by ordinary supported-host installs. It replaced the
  bootstrap-run export fixture, so bootable export is a generation capability
  with no bootstrap dependency.
- **Fixture sources:** `fixture-system.toml`, the Fedora 44 system model the
  suites apply; `conary-qemu-test-access/`, a CCS package built at test time
  that ships harness SSH access, a DHCP `.network` file, a tmpfiles fragment
  recreating openssh's privilege-separation directory on carrier `/var`, and
  explicit unit-enablement symlinks plus a package-owned networkd preset. The
  preset preserves DHCP enablement when Fedora's native networkd post-install
  lifecycle applies unit preset policy. The symlinks are produced by
  `stage-links.sh` rather than checked in, because the harness copies fixtures
  with `scp -r`, which would materialize a checked-in link as a copy of its
  target. `publish-selected-generation.sh` is the shared idempotent
  publication-convergence and selected-generation assertion used by both
  carriers.
- **Consumes:** Group O `TGE02`
  (`supported_host_generation_export_boots`) and Group P `TISO01`
  (`supported_host_generation_iso_export_boots`).
- **Fast proof:**
  `cargo test -p conary-core --test supported_host_generation_export_fixture_contract`;
  `cargo run -p conary-test -- list`.
- **Medium proof:** `conary ccs build` of `conary-qemu-test-access/` against a
  substituted stage, then `conary ccs verify` with the fixture trust policy.
- **Slow proof:**
  `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`;
  `cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3`.
  Both need a boot lane: a host with `/dev/kvm` and OVMF.
- **Regeneration:** hand-maintained. The model entries are pinned to real
  Fedora 44 package facts; changing the set means re-deriving them from Fedora
  44 headers, not from documentation.
- **Transaction contract:** `model apply` resolves the changed entries as one
  exact-source SAT package set and executes one lifecycle transaction. The
  suite verifies the selected generation contains networkd enablement before
  export, then requires an independently reachable SSH boot of the qcow2 or ISO
  artifact.
- **Current evidence:** the 2026-07-31 PR #151 implementation proof passed all
  five Group O qcow2 cases and the Group P ISO case on Fedora 44. The shared
  final helper is idempotent, requires zero pending publication debt, and
  validates the exact numeric selected-generation target. Full timings and
  artifact identities belong on the owning pull request rather than in this
  map.
- **Safety notes:** The staged `authorized_keys` carries the
  `__CONARY_QEMU_TEST_PUBLIC_KEY__` placeholder and is substituted in the guest
  with the disposable harness key; no real credential belongs in this fixture.
  The suites assemble into a scratch-disk `--db-path` root and never mutate the
  guest's live root. That scratch filesystem must be ext4 with the `verity`
  feature: composefs generation publication enables fs-verity on `root.erofs`
  and truthfully fails closed when the backing filesystem lacks it. A failed
  convergence attempt must retain and print the typed
  `generation_publications.last_error`.

### qemu-source-image-fixtures

- **Owner:** image construction: `scripts/build-qemu-guest-image.sh`;
  unprivileged identity/size rotation:
  `scripts/bootstrap-vm/rotate-qemu-test-identity.sh`; QEMU orchestration and
  download cache: `apps/conary-test/src/engine/qemu.rs`; complete-step deadline
  and owned-helper termination authority:
  `apps/conary-test/src/engine/qemu/deadline.rs`; bounded live
  console capture: `apps/conary-test/src/engine/qemu/console.rs`; cross-guest
  static client staging: `scripts/build-static-conary.sh`.
- **Purpose:** Keep a versioned, generation-builder-ready qcow2 source image
  paired with a disposable SSH identity and enough root-filesystem headroom
  for full live-root adoption into CAS.
- **Fixture sources:** the pinned official Fedora Cloud Base 44 qcow2 and
  provisioning contract in `scripts/build-qemu-guest-image.sh`, producing the
  active immutable `fedora44-guest-v2` artifact served by Remi with the
  `conaryos-test-key-v4` disposable identity. The historical
  `minimal-boot-vN.qcow2` artifacts and matching versioned keys remain evidence
  only. Active consumers are the Phase 3 QEMU manifests under
  `apps/conary/tests/integration/remi/manifests/`.
- **Consumes:** Groups N, O, and P plus composefs modernization QEMU steps.
- **Fast proof:** `bash scripts/test-build-qemu-guest-image.sh`;
  `bash -n scripts/bootstrap-vm/rotate-qemu-test-identity.sh`;
  `scripts/bootstrap-vm/rotate-qemu-test-identity.sh --help`;
  `cargo run -p conary-test -- list`;
  `cargo test -p conary-test suite_inventory`.
- **Medium proof:** direct KVM boot with the generated key, verification of
  root free space, and presence of `sqlite3`, `cpio`, `dracut`, `depmod`,
  `systemd-repart`, `qemu-img`, ext4/FAT mkfs helpers, `composefs-info`, and
  `/usr/lib/dracut`.
- **Slow proof:** `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`.
- **Active image:** `fedora44-guest-v2` — an official Fedora Cloud Base 44
  qcow2 provisioned by `scripts/build-qemu-guest-image.sh`, on the
  `conaryos-test-key-v4` disposable identity (the key name records the identity,
  not the image lineage; the Fedora image reuses it rather than rotating). It
  ships **no** `/var/lib/conary`, which is why it can run the current build at
  all — see the lineage note below. V2 also carries the packaged
  `/usr/lib/systemd/boot/efi/systemd-bootx64.efi` that generation export
  consumes after full adoption. Its immutable public artifact is
  `https://remi.conary.io/test-artifacts/fedora44-guest-v2.qcow2`, SHA-256
  `f688ac2a02b0b0558e28de1c97bbcb2e45b6772a4f019b037f72ec584a420174`;
  an empty-cache KVM host can therefore reproduce the lane.
- **Regeneration:** `scripts/build-qemu-guest-image.sh --output PATH
  --private-key PATH` builds a guest image from scratch. It downloads a pinned
  official Fedora Cloud Base qcow2, verifies it against a pinned SHA-256, grows
  it to `--disk-size-gib` (default 20, the CAS headroom full adoption needs),
  opens root SSH for the supplied disposable identity through a NoCloud seed,
  installs the generation toolchain and the running kernel's driver packages at
  their exact NEVRA, runs the phase-3 manifests' own preflight inside the guest,
  disables cloud-init, boots once more without the seed, and finishes with
  `qemu-img check` plus a compressing convert.
  `scripts/bootstrap-vm/rotate-qemu-test-identity.sh` remains the in-place
  identity/size rotation for an existing image: it starts from the prior
  immutable qcow2, relocates GPT backup data, grows the final `CONARY_ROOT`
  partition and ext4 filesystem without mounting it, replaces only
  `/root/.ssh/authorized_keys`, verifies the inserted public key, compresses the
  new qcow2, and runs `qemu-img check` before promotion.
- **Image lineage:** the `minimal-boot-v1` through `minimal-boot-v5` images are
  conaryOS artifacts produced by the bootstrap pipeline; `minimal-boot-v5`
  additionally carries host libraries injected by hand to keep a
  glibc-linked staged binary running. Neither the lineage nor that patch has a
  regeneration path once the bootstrap pipeline is removed, which is why
  `scripts/build-qemu-guest-image.sh` exists. Fedora 44 is the base because the
  phase-3 rotation runs `--distro fedora44`, the generation builder shells out
  to Fedora-native `dracut`, and the supported-host export fixture assembles a
  Fedora 44 root. The lineage is also unusable with the current build for a
  second, independent reason: those images ship a bootstrap-built
  `/var/lib/conary/conary.db` at migration-chain schema version 66, and the
  schema epoch hard cut in df607ee8 (#61, 2026-07-26) retired it, so
  `conary system init` refuses inside them. When the fixture state is
  disposable, `conary system rebuild-db --discard-state --yes` snapshots that
  database and initializes current repository/host-capability authority.
- **Current evidence:** the 2026-07-16 Fedora 44 Group O local KVM run passed
  all five cases against `minimal-boot-v4`, the version the suites targeted at
  the time; a focused recompiled-harness TGE01 rerun also passed with the
  `conaryos-test-key-v4` cache/artifact name. Remi
  serves the v4 image and private/public disposable test-key artifacts from its
  public test-artifact path; an isolated cache downloaded the image and private
  key with matching hashes and passed TGE01 under KVM in 63,320 ms. That run
  predates both the #61 schema epoch cut and the Fedora re-base. The v1
  bring-up exposed that Fedora Cloud omitted systemd-boot's packaged EFI
  binary; v2 added that exact source asset and superseded v1. The
  `minimal-boot-v5` stopgap never produced a green run — its TGE01 attempt on
  2026-07-28 failed at `conary system init` on the retired schema described
  above.
- **Safety notes:** Never overwrite the source image. Keep the generated
  private key mode `0600`; it is a disposable test credential, not a Remi,
  federation, release-signing, or operator identity. Publish small image/key
  replacements through the authenticated Remi admin test-artifact route, or
  large images through the digest-pinned, immutable
  `conary-remi-deploy publish-test-artifact` operation after authenticated SSH
  staging. Update every active manifest to one version before accepting the
  gate.

## How To Use This Map

- For docs-only edits to this map, run `bash scripts/check-doc-truth.sh` and
  `git diff --check`.
- For CCS conversion fixture edits, start with the core fast proof and add the
  Conary conversion integration filter when conversion output changes.
- For local lifecycle or query fixture edits, start with the focused Conary test
  that consumes the fixture family and then run the full owning integration test
  file.
- For Remi native publication edits, run `cargo test -p remi release_upload_`
  and `cargo test -p conary --test packaging_m4c`.
- For Remi converted publication or serving edits, run the focused Remi filter
  that names the gate being changed, then run `cargo test -p remi` when public
  listing, chunk serving, or conversion state changes.
- For `conary-test` manifest edits, run `cargo run -p conary-test -- list`
  before any suite execution. If a manifest schema or semantic changes, run the
  parser tests named above before a live suite.
- For broader integration-test expectations, see `docs/INTEGRATION-TESTING.md`.

## Deferred Fixture Families

The following families are known but not mapped in detail in this first slice.
They are candidate future ownership rows; later slices must validate source
roots and proof commands before treating them as committed gates:

- Native package corpus fixtures under
  `apps/conary/tests/fixtures/phase4-daily-driver-corpus/` and
  `apps/conary/tests/fixtures/phase4-runtime-fixture/`.
- Native package-manager daily-driver and CLI daily UX fixture patterns under
  `apps/conary/tests/native_pm_daily_driver.rs` and
  `apps/conary/tests/cli_daily_ux.rs`.
- `conary-test` bootstrap check and smoke fixtures documented in
  `docs/INTEGRATION-TESTING.md`.
- Recipe and source-selection fixtures.
- conaryd daemon job fixtures.
- Agent/MCP operation fixtures.
- TUF trust and signature verification fixtures under `apps/conary/tests/fixtures/trust/`.

Add these in later Phase 3 slices using the same schema.
