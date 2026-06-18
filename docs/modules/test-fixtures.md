---
last_updated: 2026-06-18
revision: 5
summary: Map M4d supported profiles, Remi, CCS v2, and install replay proof gates
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
| `ccs-convert-golden-cases` | CCS convert | `cargo test -p conary-core golden_fixtures`; `cargo test -p conary-core support_matrix` |
| `ccs-v2-native-authority-fixtures` | CCS v2 native authority | `cargo test -p conary-core ccs::v2`; `cargo test -p conary --test packaging_m4a` |
| `ccs-v2-local-authoring-smoke` | CCS v2 local authoring | `cargo test -p conary --test packaging_m4b` |
| `m4d-supported-profile-cutover` | Supported distro profiles | `cargo test -p conary-core supported_profiles`; `cargo test -p conary --test packaging_m4d`; `cargo test -p remi route` |
| `legacy-scriptlet-bundle-fixtures` | Install replay adapter and Conary CLI tests | `cargo test -p conary --test bundle_replay synthetic_legacy_bundle_fixtures_cover_task5_matrix` |
| `remi-native-ccs-publication` | Remi native publication | `cargo test -p remi release_upload_`; `cargo test -p conary --test packaging_m4c` |
| `remi-scriptlet-publication-gate` | Remi server publication | `cargo test -p remi publication` |
| `remi-test-artifact-fixtures` | Remi artifact handlers | `cargo test -p remi test_upload_fixture`; `cargo test -p remi test_public_fixture_get_and_head` |
| `conary-test-remi-manifests` | Integration harness | `cargo run -p conary-test -- list`; `cargo test -p conary-test suite_inventory` |

### ccs-convert-golden-cases

- **Owner:** CCS convert:
  `crates/conary-core/src/ccs/convert/golden_fixtures.rs`.
- **Purpose:** Stable expected outcomes for native-free, fully replaced,
  legacy replay, review-required, blocked, and rejected conversion cases.
- **Fixture sources:**
  `crates/conary-core/src/ccs/convert/golden_fixtures.rs`;
  `crates/conary-core/src/ccs/convert/support_matrix.rs`; adapter and
  blocked-class registries.
- **Consumes:** Core conversion tests and CLI conversion integration tests.
- **Fast proof:** `cargo test -p conary-core golden_fixtures`;
  `cargo test -p conary-core support_matrix`.
- **Medium proof:** `cargo test -p conary --test conversion_integration golden_conversion`.
- **Slow proof:** No slow gate for map-only changes.
- **Regeneration:** Hand-maintained Rust tables guarded by uniqueness,
  supported-target, and support-matrix alignment tests.
- **Safety notes:** Public-ready fixtures must use exact supported target IDs:
  `fedora-44`, `ubuntu-26.04`, or `arch`.

### ccs-v2-native-authority-fixtures

- **Owner:** CCS v2 contract:
  `crates/conary-core/src/ccs/v2/`; archive/package routing:
  `crates/conary-core/src/ccs/archive_reader.rs` and
  `crates/conary-core/src/ccs/package.rs`.
- **Purpose:** Signed native CCS v2 authority, exact-byte signature
  verification, verified install parsing, publish-gate compatibility, and
  fail-closed rejection of legacy/default-reconstructed authority.
- **Fixture sources:** in-test builders under
  `crates/conary-core/src/ccs/v2/test_support.rs`;
  `apps/conary/tests/packaging_m4a.rs`; targeted unit fixtures in
  `crates/conary-core/src/ccs/{archive_reader,package,verify}.rs`.
- **Consumes:** CCS v2 schema/reader/validation/identity tests, verifier tests,
  static publish-gate tests, and M4a CLI install integration tests.
- **Fast proof:** `cargo test -p conary-core ccs::v2`;
  `cargo test -p conary --test packaging_m4a`.
- **Medium proof:** `cargo test -p conary-core ccs::verify`;
  `cargo test -p conary-core repository::static_repo::publish_gate`.
- **Slow proof:** No slow gate for fixture-map-only changes.
- **Regeneration:** Hand-maintained Rust builders until M4b authoring emits
  native v2 packages directly.
- **Safety notes:** v2 native fixtures are signed `format_version = 2`
  authority with complete file, component, dependency, provenance,
  TOML-debug-hash, and content-identity coverage. Legacy rejection fixtures are
  v1 `BinaryManifest` packages and CBOR-only default-reconstruction packages
  that prove fail-closed diagnostics.

### ccs-v2-local-authoring-smoke

- **Owner:** CCS v2 local authoring commands:
  `apps/conary/src/commands/ccs/{templates.rs,lint.rs,build.rs,test.rs,local_dev.rs}`;
  authority projection: `crates/conary-core/src/ccs/v2/authoring.rs`.
- **Purpose:** Minimal-file native authoring loop from `ccs.toml` through lint,
  local-dev or explicit-key v2 build, local-dev verify, isolated dry-run test,
  and static publish rejection for local-dev/host-hardened artifacts.
- **Fixture sources:** in-test project builder in
  `apps/conary/tests/packaging_m4b.rs`.
- **Consumes:** M4b CLI smoke, signing guardrail, lifecycle/dependency
  profile-deferred, local-dev trust, and isolated dry-run tests.
- **Fast proof:** `cargo test -p conary --test packaging_m4b`.
- **Medium proof:** `cargo test -p conary-core ccs::v2`;
  `cargo test -p conary-core repository::static_repo::publish_gate`.
- **Slow proof:** No slow gate for M4b fixture-map-only changes.
- **Regeneration:** Temporary source trees are generated during tests.
- **Safety notes:** Local-dev keys are isolated with test HOME/XDG directories.
  Local-dev v2 artifacts are for local verify/test only and must remain
  rejected by static publish and Remi release trust.

### m4d-supported-profile-cutover

- **Owner:** Supported target profile catalog:
  `crates/conary-core/src/repository/supported_profiles/`; CLI smoke:
  `apps/conary/tests/packaging_m4d.rs`; Remi route proof:
  `apps/remi/src/server/handlers/`.
- **Purpose:** Prove exactly three public IDs (`fedora-44`, `ubuntu-26.04`,
  and `arch`), route/profile agreement for `fedora`, `ubuntu`, and `arch`,
  unsupported derivative refusal, and profile-backed lifecycle diagnostics.
- **Fast proof:** `cargo test -p conary-core supported_profiles`;
  `cargo test -p conary --test packaging_m4d`;
  `cargo test -p remi route`.
- **Medium proof:** `cargo test -p conary-core ccs::v2`;
  `cargo test -p remi conversion`;
  `cargo test -p conary-core remi_sync`.
- **Safety notes:** `debian` is a valid version-scheme string for Ubuntu
  package comparison, not a public supported target or Remi route slug.

### remi-native-ccs-publication

- **Owner:** Remi native publication:
  `apps/remi/src/server/native_publish/`; release upload route/staging:
  `apps/remi/src/server/release_publish.rs`.
- **Purpose:** Release-eligible CCS v2 artifacts published through local Remi
  without conversion-shaped storage.
- **Fixture sources:** in-test release-eligible v2 builders in
  `apps/remi/src/server/release_publish.rs` and
  `apps/conary/tests/packaging_m4c.rs`.
- **Consumes:** native release upload, replacement, public metadata/download,
  client dry-run install, sparse/search/index, and chunk-GC tests.
- **Fast proof:** `cargo test -p remi release_upload_`;
  `cargo test -p conary --test packaging_m4c`.
- **Medium proof:** `cargo test -p remi native_publish`;
  `cargo test -p remi publication`;
  `cargo test -p remi metadata_includes_native_only_package_as_native_not_converted`;
  `cargo test -p remi sparse_index_preserves_native_sibling_releases`;
  `cargo test -p remi search_rebuild_preserves_native_release_identity_and_converted_false`.
- **Slow proof:** No cloud or QEMU proof is required for local native
  publication; run `cargo test -p remi` when public serving behavior changes.
- **Regeneration:** Temporary signed and attested v2 packages are generated in
  Rust tests.
- **Safety notes:** fixtures must prove no `converted_packages` row is written,
  local-dev or otherwise publish-gate-rejected artifacts write no public state,
  failed replacement preserves the last public native row, and active native
  chunks remain protected from serving and garbage collection regressions.

### legacy-scriptlet-bundle-fixtures

- **Owner:** Install replay adapter:
  `apps/conary/src/commands/install/legacy_replay.rs`; fixture builders:
  `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`.
- **Purpose:** Synthetic legacy scriptlet bundles for install, remove, upgrade,
  foreign replay, and query safety behavior.
- **Fixture sources:** `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`;
  local builders in `apps/conary/tests/bundle_replay.rs` and
  `apps/conary/tests/query_scripts.rs`.
- **Consumes:** `apps/conary/tests/bundle_replay.rs`;
  `apps/conary/tests/foreign_replay.rs`; `apps/conary/tests/query_scripts.rs`.
- **Fast proof:**
  `cargo test -p conary --test bundle_replay synthetic_legacy_bundle_fixtures_cover_task5_matrix`.
- **Medium proof:** `cargo test -p conary --test bundle_replay`;
  `cargo test -p conary --test foreign_replay`;
  `cargo test -p conary --test query_scripts`.
- **Slow proof:** No slow gate for map-only changes; use focused
  `conary-test` suites only when install/remove behavior changes active host
  flows.
- **Regeneration:** Hand-maintained Rust builders.
- **Safety notes:** Do not weaken review, blocked, raw replay, target
  compatibility, or private-path redaction gates. CLI replay fixtures are not
  Remi publication fixtures; see `remi-scriptlet-publication-gate` for
  server-side gates.

### remi-scriptlet-publication-gate

- **Owner:** Remi server: `apps/remi/src/server/publication.rs`.
- **Purpose:** Public-ready filtering for converted packages and chunks based
  on scriptlet metadata.
- **Fixture sources:** `apps/remi/src/server/publication.rs`;
  `apps/remi/src/server/conversion.rs`;
  `apps/remi/src/server/conversion/test_support.rs`;
  `apps/remi/src/server/conversion/persistence.rs`;
  `apps/remi/src/server/conversion/workflow.rs`;
  `apps/remi/src/server/index_gen.rs`;
  `apps/remi/src/server/prewarm.rs`; handler tests under
  `apps/remi/src/server/handlers/`.
- **Consumes:** Remi publication, conversion, generated-index,
  sparse/detail/search/chunk serving, and prewarm tests.
- **Fast proof:** `cargo test -p remi publication`.
- **Medium proof:**
  `cargo test -p remi persisted_goal8a_golden_outcomes_respect_publication_gate`;
  `cargo test -p remi generated_index_includes_public_scriptlets_without_private_path`.
- **Slow proof:** No slow gate for map-only changes; run
  `cargo test -p remi` for behavior changes that affect serving.
- **Regeneration:** Hand-maintained test rows and helper builders.
- **Safety notes:** Public listing and chunk serving must not expose non-public
  scriptlet rows or private `review_artifact_path` values. Server-side
  publication fixtures are not CLI replay fixtures; see
  `legacy-scriptlet-bundle-fixtures` for local replay behavior.

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

- **Owner:** Integration harness: `apps/conary-test/src/config/` and
  `apps/conary-test/src/suite_inventory.rs`.
- **Purpose:** Declarative Remi and package-manager integration suites.
- **Fixture sources:** `apps/conary/tests/integration/remi/manifests/`;
  `apps/conary/tests/integration/remi/containers/`.
- **Consumes:** `cargo run -p conary-test -- list`, manifest parser tests,
  suite runner, local QEMU validation scripts.
- **Fast proof:** `cargo run -p conary-test -- list`;
  `cargo test -p conary-test suite_inventory`.
- **Medium proof:**
  `cargo test -p conary-test config::tests::test_load_phase1_core_manifest`;
  `cargo test -p conary-test config::tests::test_load_phase3_group_m_manifest_installs_local_fixture_ccs`.
- **Slow proof:** Suite-specific commands such as
  `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4`
  when behavior changes require live integration proof. `fedora44` is the
  existing `conary-test` runner distro key; public CCS target IDs remain
  `fedora-44`, `ubuntu-26.04`, and `arch`.
- **Regeneration:** Manifests are hand-maintained TOML. Fixture packages are
  built or published through `conary-test fixtures` commands and scripts
  documented in `docs/INTEGRATION-TESTING.md`. Suite result JSON is generated
  locally under the ignored `apps/conary/tests/integration/remi/results/`
  directory.
- **Safety notes:** Treat manifest schema and semantics as persisted test
  configuration; changes need parser/list proof and an explicit migration or
  defaulting decision.

## How To Use This Map

- For docs-only edits to this map, run docs-audit and diff hygiene.
- For CCS conversion fixture edits, start with the core fast proof and add the
  Conary conversion integration filter when conversion output changes.
- For local replay or query fixture edits, start with the focused Conary test
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
- Bootstrap and QEMU source-image fixtures.
- Generation export and ISO carrier fixtures.
- Recipe and source-selection fixtures.
- conaryd daemon job fixtures.
- Agent/MCP operation fixtures.
- TUF trust and signature verification fixtures under `apps/conary/tests/fixtures/trust/`.

Add these in later Phase 3 slices using the same schema.
