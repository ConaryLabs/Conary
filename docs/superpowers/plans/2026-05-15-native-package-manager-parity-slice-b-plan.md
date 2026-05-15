# Native Package Manager Parity Slice B Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `conary update` honest and usable for Conary-owned packages after the Slice A no-generation install/remove foundation.

**Architecture:** Keep update as an orchestration layer over the existing install path, so no-generation update succeeds by resolving a newer package and calling `cmd_install`, which now materializes files through `MutableLiveRoot`. Add source-level security metadata support to repositories so `update --security` can refuse before mutation when a source cannot answer security-advisory questions. Make multi-package update outcomes distinguish clean success, partial success, skipped authority, and required package failures.

**Tech Stack:** Rust, rusqlite migrations/models, existing `apps/conary/src/commands/update.rs` command flow, CLI integration tests, `conary-test` inventory validation.

---

## Source Documents

- Spec: `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`
- Previous slice: `docs/superpowers/plans/2026-05-14-native-package-manager-parity-slice-a-plan.md`
- Main code path: `apps/conary/src/commands/update.rs`
- Install delegation path: `apps/conary/src/commands/install/mod.rs`
- Repository model: `crates/conary-core/src/db/models/repository.rs`

## File Structure

- Modify `crates/conary-core/src/db/schema.rs`: bump schema version and route migration v68.
- Modify `crates/conary-core/src/db/migrations/v41_current.rs`: add `repositories.security_advisory_support`.
- Modify `crates/conary-core/src/db/models/repository.rs`: add `SecurityAdvisorySupport` and persist it with `Repository`.
- Modify `crates/conary-core/src/db/models/mod.rs`: export `SecurityAdvisorySupport`.
- Modify `apps/conary/src/cli/repo.rs`, `apps/conary/src/commands/repo.rs`, and `apps/conary/src/dispatch.rs`: let tests/users set support with `repo add --security-advisories supported|unsupported|unknown`, and show it in `repo list`.
- Modify `apps/conary/src/commands/update.rs`: security-only candidate classification, refusal-before-mutation, truthful no-update messages, and partial failure reporting.
- Modify `apps/conary/tests/native_pm_live_root.rs`: add CLI proof for no-generation update and security metadata refusal.
- Update this plan as tasks land.

## Task 1: Add Source-Level Security Advisory Support

**Files:**
- Modify: `crates/conary-core/src/db/schema.rs`
- Modify: `crates/conary-core/src/db/migrations/v41_current.rs`
- Modify: `crates/conary-core/src/db/models/repository.rs`
- Modify: `crates/conary-core/src/db/models/mod.rs`

- [x] **Step 1: Write migration/model tests**

Add tests near the repository model and migration tests:

```rust
#[test]
fn repository_defaults_security_advisory_support_to_unknown() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::migrate(&conn).unwrap();

    let mut repo = Repository::new("test".to_string(), "https://example.test".to_string());
    let id = repo.insert(&conn).unwrap();
    let loaded = Repository::find_by_id(&conn, id).unwrap().unwrap();

    assert_eq!(
        loaded.security_advisory_support,
        SecurityAdvisorySupport::Unknown
    );
}

#[test]
fn repository_round_trips_security_advisory_support() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::migrate(&conn).unwrap();

    let mut repo = Repository::new("test".to_string(), "https://example.test".to_string());
    repo.security_advisory_support = SecurityAdvisorySupport::Supported;
    let id = repo.insert(&conn).unwrap();

    let loaded = Repository::find_by_id(&conn, id).unwrap().unwrap();
    assert_eq!(loaded.security_advisory_support, SecurityAdvisorySupport::Supported);
}
```

Run:

```bash
cargo test -p conary-core repository_security_advisory_support -- --nocapture
```

Expected: fails before implementation because the field and enum do not exist.

- [x] **Step 2: Add migration v68**

In `crates/conary-core/src/db/schema.rs`, bump:

```rust
pub const SCHEMA_VERSION: i32 = 68;
```

Add match arm:

```rust
68 => migrations::migrate_v68(conn),
```

In `crates/conary-core/src/db/migrations/v41_current.rs`, add:

```rust
/// Version 68: repository security-advisory metadata support
pub fn migrate_v68(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 68");

    conn.execute_batch(
        "
        ALTER TABLE repositories
            ADD COLUMN security_advisory_support TEXT NOT NULL DEFAULT 'unknown'
            CHECK(security_advisory_support IN ('unknown', 'unsupported', 'supported'));
        ",
    )?;

    info!("Schema version 68 applied successfully (repository security advisory support)");
    Ok(())
}
```

Add a migration test:

```rust
#[test]
fn test_migrate_v68_adds_repository_security_advisory_support() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();

    conn.execute(
        "INSERT INTO repositories (name, url) VALUES ('security-test', 'https://example.test')",
        [],
    )
    .unwrap();

    let support: String = conn
        .query_row(
            "SELECT security_advisory_support FROM repositories WHERE name = 'security-test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(support, "unknown");
}
```

- [x] **Step 3: Add enum and model field**

In `repository.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityAdvisorySupport {
    Unknown,
    Unsupported,
    Supported,
}

impl SecurityAdvisorySupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::Supported => "supported",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "supported" => Self::Supported,
            "unsupported" => Self::Unsupported,
            _ => Self::Unknown,
        }
    }

    pub fn is_supported(self) -> bool {
        self == Self::Supported
    }
}
```

Add `pub security_advisory_support: SecurityAdvisorySupport` to `Repository`, default it to `Unknown`, include it in `COLUMNS`, `insert`, `update`, and `from_row`, and export it from `models/mod.rs`.

- [x] **Step 4: Verify**

Run:

```bash
cargo test -p conary-core repository_security_advisory_support test_migrate_v68 -- --nocapture
```

Expected: model and migration tests pass.

- [x] **Step 5: Commit**

```bash
git add crates/conary-core/src/db/schema.rs crates/conary-core/src/db/migrations/v41_current.rs crates/conary-core/src/db/models/repository.rs crates/conary-core/src/db/models/mod.rs
git commit -m "feat(repo): track security advisory support"
```

## Task 2: Expose Security Advisory Support In Repository CLI

**Files:**
- Modify: `apps/conary/src/cli/repo.rs`
- Modify: `apps/conary/src/commands/repo.rs`
- Modify: `apps/conary/src/dispatch.rs`

- [x] **Step 1: Add a focused command test**

Add a focused test in `apps/conary/src/commands/repo.rs` that creates a repository with `SecurityAdvisorySupport::Supported` through `RepoAddOptions`, then reloads the row and verifies the enum.

Expected failing assertion before implementation:

```rust
assert_eq!(repo.security_advisory_support, SecurityAdvisorySupport::Supported);
```

- [x] **Step 2: Add CLI enum and option**

In `apps/conary/src/cli/repo.rs`, import `clap::ValueEnum` and add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliSecurityAdvisorySupport {
    Unknown,
    Unsupported,
    Supported,
}
```

Add to `RepoCommands::Add`:

```rust
/// Whether this repository publishes security-advisory metadata
#[arg(long, value_enum, default_value_t = CliSecurityAdvisorySupport::Unknown)]
security_advisories: CliSecurityAdvisorySupport,
```

- [x] **Step 3: Persist and render the option**

Add `security_advisory_support` to `RepoAddOptions`, map the CLI enum in `dispatch_repo_command`, assign it before `repo.insert`, and print both add/list output:

```rust
println!("  Security Advisories: {}", repo.security_advisory_support.as_str());
```

- [x] **Step 4: Verify**

Run:

```bash
cargo test -p conary repo_security_advisory_support -- --nocapture
cargo build -p conary
```

Expected: tests pass and clap compiles with the new option.

- [x] **Step 5: Commit**

```bash
git add apps/conary/src/cli/repo.rs apps/conary/src/commands/repo.rs apps/conary/src/dispatch.rs
git commit -m "feat(repo): expose security advisory support"
```

## Task 3: Make `update --security` Refuse Unknown Or Unsupported Sources

**Files:**
- Modify: `apps/conary/src/commands/update.rs`

- [x] **Step 1: Add candidate classification tests**

Add tests in `update.rs` proving:

```rust
#[test]
fn security_update_refuses_unknown_source_metadata_before_mutation() {
    let (_temp, db_path) = create_test_db();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let trove = seed_security_update_fixture(
        &conn,
        SecurityAdvisorySupport::Unknown,
        false,
    );
    let policy = ResolutionPolicy::new();

    let result = select_update_candidate(
        &conn,
        &trove,
        true,
        &policy,
        Some(RepositoryDependencyFlavor::Rpm),
    )
    .unwrap();

    assert!(matches!(
        result,
        UpdateCandidateSelection::SecurityMetadataUnavailable { .. }
    ));
}

#[test]
fn security_update_selects_supported_security_candidate() {
    let (_temp, db_path) = create_test_db();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let trove = seed_security_update_fixture(
        &conn,
        SecurityAdvisorySupport::Supported,
        true,
    );
    let policy = ResolutionPolicy::new();

    let result = select_update_candidate(
        &conn,
        &trove,
        true,
        &policy,
        Some(RepositoryDependencyFlavor::Rpm),
    )
    .unwrap();

    assert!(matches!(result, UpdateCandidateSelection::Selected(_)));
}
```

Expected: fails before the selection enum and helper exist.

- [x] **Step 2: Replace `Option<SelectedUpdateCandidate>` with a selection enum**

Add:

```rust
#[derive(Debug, Clone)]
enum UpdateCandidateSelection {
    Selected(SelectedUpdateCandidate),
    NoEligibleUpdate,
    SecurityMetadataUnavailable {
        package: String,
        repository: String,
        support: SecurityAdvisorySupport,
        candidate_version: String,
    },
}
```

Change `select_update_candidate` to return `Result<UpdateCandidateSelection>`.

For `security_only`, search all newer eligible candidates first. If a newer candidate exists from a repository where `security_advisory_support` is `Unknown` or `Unsupported`, return `SecurityMetadataUnavailable` before selecting or mutating. If support is `Supported`, only select candidates with `is_security_update == true`.

- [x] **Step 3: Refuse before mutation in `cmd_update` and `cmd_update_group`**

In both loops, collect `SecurityMetadataUnavailable` rows before pushing updates. If any are present, print each source and bail before dry-run/apply:

```rust
anyhow::bail!(
    "Cannot run security-only update because {} source(s) cannot prove security metadata support. Mark the source supported only after its repository metadata publishes advisory data.",
    unavailable.len()
);
```

Do not print `No security updates available` when this category exists.

- [x] **Step 4: Verify**

Run:

```bash
cargo test -p conary security_update -- --nocapture
cargo test -p conary adopted_update_tests -- --nocapture
```

Expected: security metadata tests pass and adopted update behavior remains unchanged.

- [x] **Step 5: Commit**

```bash
git add apps/conary/src/commands/update.rs
git commit -m "fix(update): refuse unverifiable security metadata"
```

## Task 4: Make Multi-Package Update Failures Truthful

**Files:**
- Modify: `apps/conary/src/commands/update.rs`

- [x] **Step 1: Add summary tests**

Extract a small outcome helper rather than testing all network paths directly:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdatePackageFailure {
    package: String,
    version: String,
    reason: String,
}

fn update_required_failure_message(failures: &[UpdatePackageFailure], total: usize) -> Option<String> {
    if failures.is_empty() {
        None
    } else {
        Some(format!(
            "{} of {} requested package update(s) failed: {}",
            failures.len(),
            total,
            failures
                .iter()
                .map(|failure| failure.package.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}
```

Tests:

```rust
#[test]
fn partial_update_failure_message_is_not_clean_success() {
    let failures = vec![UpdatePackageFailure {
        package: "broken".to_string(),
        version: "2.0.0".to_string(),
        reason: "resolver failed".to_string(),
    }];

    let message = update_required_failure_message(&failures, 2).unwrap();

    assert!(message.contains("1 of 2"));
    assert!(message.contains("broken"));
    assert!(!message.contains("All packages are up to date"));
}
```

- [x] **Step 2: Track required failures separately from delta fallback**

Replace `had_failures` with `required_failures: Vec<UpdatePackageFailure>`. Delta download/apply failures increment `delta_failures` and fall back to full package download when possible, but do not become required failures if the full install succeeds. Resolver failures, unsupported `LocalCas`, missing fallback repository, and `cmd_install` errors append `UpdatePackageFailure`.

- [x] **Step 3: Return nonzero for required failures**

After inserting `DeltaStats` and printing the update summary, if `required_failures` is not empty, return `Err(anyhow!(...))`. Preserve already-applied package installs and mark the outer update changeset `Applied` when at least one package succeeded, `RolledBack` when no package succeeded and required failures occurred.

- [x] **Step 4: Verify**

Run:

```bash
cargo test -p conary partial_update_failure update_required_failure -- --nocapture
cargo test -p conary mark_pending_changeset_rolled_back -- --nocapture
```

Expected: tests pass.

- [x] **Step 5: Commit**

```bash
git add apps/conary/src/commands/update.rs
git commit -m "fix(update): report partial package failures"
```

## Task 5: Prove No-Generation Update Uses The Live-Root Install Path

**Files:**
- Modify: `apps/conary/tests/native_pm_live_root.rs`

- [ ] **Step 1: Add a CLI integration test**

Use a temporary root and DB with no active generation. Build the existing fixture sources into the tempdir with the CLI:

```bash
conary ccs build apps/conary/tests/fixtures/conary-test-fixture/v1 --source apps/conary/tests/fixtures/conary-test-fixture/v1/stage --output <temp>/packages/v1
conary ccs build apps/conary/tests/fixtures/conary-test-fixture/v2 --source apps/conary/tests/fixtures/conary-test-fixture/v2/stage --output <temp>/packages/v2
```

Install v1 with `conary ccs install`, seed a supported repository candidate for v2, serve the v2 `.ccs` over local HTTP, and run `conary update conary-test-fixture`. The update should call the existing install path, materialize the v2 files under the test root, and keep DB/history coherent.

Expected test shape:

```rust
#[test]
fn no_generation_update_replaces_conary_owned_ccs_v1_with_v2() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let v1 = build_fixture_ccs(root.path(), "v1");
    let v2 = build_fixture_ccs(root.path(), "v2");
    install_fixture_v1(&db_path, root.path(), &v1);
    let server = serve_file_over_http(v2);
    seed_repository_fixture_v2(&db_path, server.url());

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "update",
        "conary-test-fixture",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--sandbox",
        "never",
        "--yes",
    ]);

    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        std::fs::read_to_string(root.path().join("usr/share/conary-test/hello.txt")).unwrap(),
        "Hello from Conary test fixture v2!\n"
    );
    assert!(root.path().join("usr/share/conary-test/added.txt").exists());
}
```

The helper must build the package in the test tempdir so the test is hermetic and does not depend on prebuilt fixture artifacts. Do not use `file://` in `repository_packages.download_url`; the repository client expects HTTP(S).

- [ ] **Step 2: Add security refusal CLI proof**

In the same integration file, seed an installed package with a newer candidate from a default `Unknown` repository and run:

```bash
conary --allow-live-system-mutation update fixture --security --db-path <db> --root <root> --sandbox never --yes
```

Expected: nonzero exit, stdout/stderr mention security metadata support, file contents remain v1, and installed trove version remains v1.

- [ ] **Step 3: Verify**

Run:

```bash
cargo test -p conary --test native_pm_live_root -- --nocapture
```

Expected: all CLI live-root tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/conary/tests/native_pm_live_root.rs
git commit -m "test(update): prove no-generation live-root parity"
```

## Task 6: Final Gates And Completion Audit

**Files:**
- Modify: this plan checklist if task status changes.

- [ ] **Step 1: Run focused tests**

```bash
cargo test -p conary-core repository_security_advisory_support test_migrate_v68 -- --nocapture
cargo test -p conary security_update adopted_update_tests partial_update_failure update_required_failure -- --nocapture
cargo test -p conary --test native_pm_live_root -- --nocapture
```

Expected: all pass.

- [ ] **Step 2: Run inventory and workspace gates**

```bash
cargo run -p conary-test -- list
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: all pass.

- [ ] **Step 3: Completion audit against goal**

Confirm with real evidence:

- `conary update` works on no-generation live hosts by materializing v2 files through `cmd_install`.
- Adopted updates remain native authoritative unless `--dep-mode takeover` is explicitly requested.
- Critical adopted takeover/update remains blocked before mutation.
- Multi-package required failures return a truthful partial-failure error.
- `update --security` refuses before mutation for `unknown` or `unsupported` security advisory support.
- Focused tests, `conary-test` inventory, formatting, and Clippy all pass.

- [ ] **Step 4: Commit final plan/checklist updates**

```bash
git add docs/superpowers/plans/2026-05-15-native-package-manager-parity-slice-b-plan.md
git commit -m "docs: plan native package manager parity slice b"
```
