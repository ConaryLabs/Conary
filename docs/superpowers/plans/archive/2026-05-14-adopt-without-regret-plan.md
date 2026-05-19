# Adopt Without Regret Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Conary's adoption-led risk-free trial path with `conary system unadopt`, native package-manager authority safeguards, roadmap/docs alignment, and real RPM/DEB/Arch tests.

**Architecture:** Add `system unadopt` as a focused CLI/command module that removes only `AdoptedTrack` and `AdoptedFull` tracking rows, leaves package files untouched, and removes sync hooks for all-package unadoption. It must be behind the live-mutation acknowledgement gate and must fail closed before DB mutation when a Conary generation is selected for the runtime root. Add native-manager identity and update-boundary tests so adopted packages cannot silently cross into Conary-owned writes except under explicit takeover. Extend integration coverage so Fedora 44, Ubuntu 26.04 LTS, and Arch prove the same adoption escape contract.

**Tech Stack:** Rust, clap, rusqlite, Conary command modules, conary-test TOML manifests, Markdown docs.

---

## Suggested `/goal`

Use this when launching the implementation:

```text
/goal Implement Conary Adopt Without Regret: adoption mode preserves dnf/apt/pacman authority for RPM, DEB, and Arch systems; `conary system unadopt` provides a tested one-command non-destructive escape hatch when no Conary generation is selected and fails closed otherwise; update/install paths cannot silently take over adopted packages; roadmap/docs/tests prove the contract.
```

The goal is complete only after the final verification block in this plan passes or any skipped validation is recorded with a concrete reason.

## Files

- Modify: `ROADMAP.md`
- Modify: `README.md`
- Modify: `apps/conary/src/cli/system.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/commands/adopt/mod.rs`
- Create: `apps/conary/src/commands/adopt/unadopt.rs`
- Modify: `apps/conary/src/commands/adopt/hooks.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `apps/conary-test/src/config/mod.rs`
- Modify: `apps/conary/tests/integration/remi/manifests/phase1-advanced.toml`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

## Task 1: Add CLI Surface And Parser Tests

**Files:**
- Modify: `apps/conary/src/cli/system.rs`
- Test: `apps/conary/src/cli/mod.rs`

- [ ] **Step 1: Add failing parser tests**

Add tests in `apps/conary/src/cli/mod.rs` near the existing CLI parser tests:

```rust
#[test]
fn parses_system_unadopt_all() {
    let cli = Cli::try_parse_from(["conary", "system", "unadopt", "--all"]).unwrap();
    match cli.command {
        Some(Commands::System(SystemCommands::Unadopt { all, packages, dry_run, .. })) => {
            assert!(all);
            assert!(packages.is_empty());
            assert!(!dry_run);
        }
        _ => panic!("expected system unadopt command"),
    }
}

#[test]
fn parses_system_unadopt_package_dry_run() {
    let cli = Cli::try_parse_from([
        "conary",
        "system",
        "unadopt",
        "curl",
        "--dry-run",
    ])
    .unwrap();
    match cli.command {
        Some(Commands::System(SystemCommands::Unadopt { all, packages, dry_run, .. })) => {
            assert!(!all);
            assert_eq!(packages, vec!["curl"]);
            assert!(dry_run);
        }
        _ => panic!("expected system unadopt command"),
    }
}

#[test]
fn rejects_system_unadopt_without_scope() {
    let err = Cli::try_parse_from(["conary", "system", "unadopt"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

#[test]
fn rejects_system_unadopt_all_with_packages() {
    let err = Cli::try_parse_from(["conary", "system", "unadopt", "--all", "curl"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}
```

- [ ] **Step 2: Run the parser tests and verify they fail**

Run:

```bash
cargo test -p conary parses_system_unadopt -- --nocapture
```

Expected: tests fail because `SystemCommands::Unadopt` does not exist.

- [ ] **Step 3: Add the `Unadopt` CLI variant**

Add this variant to `SystemCommands` in `apps/conary/src/cli/system.rs` after `Adopt`:

```rust
    /// Remove Conary tracking for adopted native packages without deleting files
    Unadopt {
        /// Adopted package name(s) to stop tracking
        #[arg(required_unless_present = "all")]
        #[arg(conflicts_with = "all")]
        packages: Vec<String>,

        #[command(flatten)]
        db: DbArgs,

        /// Unadopt every package currently tracked as adopted
        #[arg(long, conflicts_with = "packages")]
        all: bool,

        /// Show what would be unadopted without changing the database or hooks
        #[arg(long)]
        dry_run: bool,

        /// Keep native package-manager sync hooks installed when using --all
        #[arg(long)]
        keep_hooks: bool,
    },
```

- [ ] **Step 4: Run the parser tests and verify they pass**

Run:

```bash
cargo test -p conary system_unadopt -- --nocapture
```

Expected: all four parser tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/cli/system.rs apps/conary/src/cli/mod.rs
git commit -m "feat(cli): add system unadopt surface"
```

## Task 2: Implement Non-Destructive Unadopt

**Files:**
- Modify: `apps/conary/src/commands/adopt/mod.rs`
- Create: `apps/conary/src/commands/adopt/unadopt.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary-test/src/config/mod.rs`
- Test: `apps/conary/src/commands/adopt/unadopt.rs`
- Test: `apps/conary-test/src/config/mod.rs`

- [ ] **Step 1: Add failing unit tests for unadoption and wire the module**

Create `apps/conary/src/commands/adopt/unadopt.rs` with the tests below first. Immediately wire the module in `apps/conary/src/commands/adopt/mod.rs` so Rust compiles and discovers the tests:

```rust
mod unadopt;
pub use unadopt::{UnadoptOptions, UnadoptSummary, cmd_unadopt};
```

The implementation stubs can stay intentionally failing until Step 4.

```rust
// src/commands/adopt/unadopt.rs

use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnadoptOptions {
    pub packages: Vec<String>,
    pub all: bool,
    pub dry_run: bool,
    pub keep_hooks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnadoptSummary {
    pub unadopted: Vec<String>,
    pub skipped_conary_owned: Vec<String>,
    pub missing: Vec<String>,
    pub hooks_removed: bool,
    pub dry_run: bool,
}

pub async fn cmd_unadopt(_db_path: &str, _opts: UnadoptOptions) -> Result<UnadoptSummary> {
    anyhow::bail!("cmd_unadopt is not implemented yet")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::{
        DependencyEntry, FileEntry, InstallSource, ProvideEntry, Trove, TroveType,
    };

    fn insert_trove(conn: &rusqlite::Connection, name: &str, source: InstallSource) -> i64 {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            source,
        );
        trove.insert(conn).unwrap()
    }

    #[tokio::test]
    async fn unadopt_named_package_removes_adopted_tracking_only() {
        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let curl_id = insert_trove(&conn, "curl", InstallSource::AdoptedFull);
        let vim_id = insert_trove(&conn, "vim", InstallSource::Repository);

        FileEntry::new("/usr/bin/curl".to_string(), "abc".to_string(), 3, 0o755, curl_id)
            .insert(&conn)
            .unwrap();
        DependencyEntry::new(
            curl_id,
            "libcurl".to_string(),
            None,
            "runtime".to_string(),
            None,
        )
            .insert(&conn)
            .unwrap();
        ProvideEntry::new(curl_id, "curl".to_string(), None)
            .insert(&conn)
            .unwrap();

        let summary = cmd_unadopt(
            &db_path,
            UnadoptOptions {
                packages: vec!["curl".to_string()],
                all: false,
                dry_run: false,
                keep_hooks: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(summary.unadopted, vec!["curl"]);
        assert_eq!(summary.skipped_conary_owned, Vec::<String>::new());
        assert!(Trove::find_by_id(&conn, curl_id).unwrap().is_none());
        assert!(Trove::find_by_id(&conn, vim_id).unwrap().is_some());
        assert!(FileEntry::find_by_trove(&conn, curl_id).unwrap().is_empty());
        assert!(DependencyEntry::find_by_trove(&conn, curl_id).unwrap().is_empty());
        assert!(ProvideEntry::find_by_trove(&conn, curl_id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn unadopt_all_leaves_conary_owned_packages() {
        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let adopted_track = insert_trove(&conn, "bash", InstallSource::AdoptedTrack);
        let adopted_full = insert_trove(&conn, "curl", InstallSource::AdoptedFull);
        let repo = insert_trove(&conn, "tree", InstallSource::Repository);
        let file = insert_trove(&conn, "local", InstallSource::File);
        let taken = insert_trove(&conn, "taken", InstallSource::Taken);

        let summary = cmd_unadopt(
            &db_path,
            UnadoptOptions {
                packages: Vec::new(),
                all: true,
                dry_run: false,
                keep_hooks: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(summary.unadopted, vec!["bash", "curl"]);
        assert_eq!(summary.skipped_conary_owned, vec!["local", "taken", "tree"]);
        assert!(Trove::find_by_id(&conn, adopted_track).unwrap().is_none());
        assert!(Trove::find_by_id(&conn, adopted_full).unwrap().is_none());
        assert!(Trove::find_by_id(&conn, repo).unwrap().is_some());
        assert!(Trove::find_by_id(&conn, file).unwrap().is_some());
        assert!(Trove::find_by_id(&conn, taken).unwrap().is_some());
    }

    #[tokio::test]
    async fn unadopt_dry_run_does_not_mutate_database() {
        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let curl_id = insert_trove(&conn, "curl", InstallSource::AdoptedFull);

        let summary = cmd_unadopt(
            &db_path,
            UnadoptOptions {
                packages: vec!["curl".to_string()],
                all: false,
                dry_run: true,
                keep_hooks: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(summary.unadopted, vec!["curl"]);
        assert!(summary.dry_run);
        assert!(Trove::find_by_id(&conn, curl_id).unwrap().is_some());
    }

    #[tokio::test]
    async fn unadopt_refuses_selected_generation_before_db_mutation() {
        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let curl_id = insert_trove(&conn, "curl", InstallSource::AdoptedFull);
        let runtime_root =
            conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(&db_path);
        std::fs::create_dir_all(runtime_root.generation_path(1)).unwrap();
        std::os::unix::fs::symlink(runtime_root.generation_path(1), runtime_root.current_link())
            .unwrap();

        let err = cmd_unadopt(
            &db_path,
            UnadoptOptions {
                packages: vec!["curl".to_string()],
                all: false,
                dry_run: false,
                keep_hooks: true,
            },
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("selected Conary generation"));
        assert!(Trove::find_by_id(&conn, curl_id).unwrap().is_some());
    }

    #[tokio::test]
    async fn unadopt_named_conary_owned_package_is_rejected() {
        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        insert_trove(&conn, "tree", InstallSource::Repository);

        let err = cmd_unadopt(
            &db_path,
            UnadoptOptions {
                packages: vec!["tree".to_string()],
                all: false,
                dry_run: false,
                keep_hooks: true,
            },
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("is Conary-owned"));
    }
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test -p conary unadopt_ -- --nocapture
```

Expected: tests compile and fail with `cmd_unadopt is not implemented yet`.

- [ ] **Step 3: Wire dispatch with live-mutation safety**

In `apps/conary/src/dispatch.rs`, add a `SystemCommands::Unadopt` arm near the adopt arm. The live-mutation guard is required for apply mode and dry-run must bypass it:

```rust
        cli::SystemCommands::Unadopt {
            packages,
            db,
            all,
            dry_run,
            keep_hooks,
        } => {
            require_live_mutation(
                allow_live_system_mutation,
                Cow::Borrowed("conary system unadopt"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::cmd_unadopt(
                &db.db_path,
                commands::UnadoptOptions {
                    packages,
                    all,
                    dry_run,
                    keep_hooks,
                },
            )
            .await
            .map(|_| ())
        }
```

In `apps/conary-test/src/config/mod.rs`, add `system unadopt` to `is_system_mutation_segment` so manifests that apply unadoption must include `--allow-live-system-mutation`.

- [ ] **Step 4: Implement `cmd_unadopt`**

Replace the stub with implementation that:

- opens the DB with `crate::commands::open_db`
- derives `ConaryRuntimeRoot::from_db_path(db_path)` and calls `current_generation(runtime_root.root())`
- returns an error before DB mutation when apply mode sees a selected Conary generation
- queries `Trove::list_all` for `--all`, or `Trove::find_by_name` for named packages
- sorts names for stable output
- deletes only `InstallSource::AdoptedTrack` and `InstallSource::AdoptedFull`
- creates a `Changeset` with `ChangesetStatus::Applied`
- calls `crate::commands::create_state_snapshot`
- prints `Native package files were not deleted.`

Use existing model methods rather than raw SQL except for a simple sorted query if no model helper exists.

- [ ] **Step 5: Run focused unadopt tests**

Run:

```bash
cargo test -p conary unadopt_ -- --nocapture
```

Expected: all unadopt tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/adopt/mod.rs apps/conary/src/commands/adopt/unadopt.rs apps/conary/src/dispatch.rs apps/conary-test/src/config/mod.rs
git commit -m "feat(adopt): add non-destructive unadopt"
```

## Task 3: Make Hook Removal Reusable And Safe

**Files:**
- Modify: `apps/conary/src/commands/adopt/hooks.rs`
- Modify: `apps/conary/src/commands/adopt/unadopt.rs`
- Test: `apps/conary/src/commands/adopt/hooks.rs`

- [ ] **Step 1: Add hook removal tests**

Add tests in `apps/conary/src/commands/adopt/hooks.rs` proving hook paths exist for all three package managers and that removable hook path pairs remove every file:

```rust
#[test]
fn hook_paths_cover_all_supported_native_package_managers() {
    assert!(hook_paths(SystemPackageManager::Rpm).is_some());
    assert!(hook_paths(SystemPackageManager::Dpkg).is_some());
    assert!(hook_paths(SystemPackageManager::Pacman).is_some());
    assert!(hook_paths(SystemPackageManager::Unknown).is_none());
}

#[test]
fn remove_hook_path_pair_removes_script_and_optional_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("conary-sync.script");
    let filter = tmp.path().join("conary-sync.filter");
    std::fs::write(&script, "script").unwrap();
    std::fs::write(&filter, "filter").unwrap();

    let removed = remove_hook_path_pair(&script, Some(&filter)).unwrap();

    assert!(removed);
    assert!(!script.exists());
    assert!(!filter.exists());
}

#[test]
fn remove_hook_path_pair_reports_false_when_nothing_existed() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("missing.script");

    let removed = remove_hook_path_pair(&script, None).unwrap();

    assert!(!removed);
}
```

- [ ] **Step 2: Expose a hook removal helper**

Adjust `remove_file_if_exists` so it returns whether it removed a file, add a testable path-pair helper, then expose a public detected-manager helper:

```rust
fn remove_file_if_exists(path: &std::path::Path) -> Result<bool> {
    if path.exists() {
        fs::remove_file(path)?;
        println!("  Removed: {}", path.display());
        return Ok(true);
    }
    Ok(false)
}

fn remove_hook_path_pair(script: &std::path::Path, filter: Option<&std::path::Path>) -> Result<bool> {
    let mut removed = remove_file_if_exists(script)?;
    if let Some(filter) = filter {
        removed |= remove_file_if_exists(filter)?;
    }
    Ok(removed)
}

pub(crate) async fn remove_detected_sync_hooks() -> Result<bool> {
    let pkg_mgr = SystemPackageManager::detect();
    let Some(paths) = hook_paths(pkg_mgr) else {
        return Ok(false);
    };

    remove_hook_path_pair(
        std::path::Path::new(paths.script),
        paths.filter.map(std::path::Path::new),
    )
}
```

Update the existing `cmd_sync_hook_install(remove = true)` call sites in the same file to pass `Path::new(...)` into `remove_file_if_exists`, or keep a tiny string wrapper if that makes the diff cleaner.

- [ ] **Step 3: Use it from `cmd_unadopt --all`**

Make the command testable by routing hook removal through a small internal helper. `cmd_unadopt` should call it with `super::hooks::remove_detected_sync_hooks`; unit tests can pass a fake hook remover.

```rust
async fn cmd_unadopt_with_hook_remover<F, Fut>(
    db_path: &str,
    opts: UnadoptOptions,
    hook_remover: F,
) -> Result<UnadoptSummary>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    // implementation body
}
```

Inside that helper, after successful all-package apply:

```rust
let hooks_removed = if opts.all && !opts.keep_hooks && !opts.dry_run {
    hook_remover().await?
} else {
    false
};
```

Return that value in `UnadoptSummary`.

Add unadopt tests for default hook removal and `--keep-hooks`:

```rust
#[tokio::test]
async fn unadopt_all_removes_hooks_by_default() {
    let (_tmp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    insert_trove(&conn, "curl", InstallSource::AdoptedFull);

    let summary = cmd_unadopt_with_hook_remover(
        &db_path,
        UnadoptOptions {
            packages: Vec::new(),
            all: true,
            dry_run: false,
            keep_hooks: false,
        },
        || async { Ok(true) },
    )
    .await
    .unwrap();

    assert!(summary.hooks_removed);
}

#[tokio::test]
async fn unadopt_all_keep_hooks_skips_hook_removal() {
    let (_tmp, db_path) = create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    insert_trove(&conn, "curl", InstallSource::AdoptedFull);

    let summary = cmd_unadopt_with_hook_remover(
        &db_path,
        UnadoptOptions {
            packages: Vec::new(),
            all: true,
            dry_run: false,
            keep_hooks: true,
        },
        || async { panic!("hook remover should not run with --keep-hooks") },
    )
    .await
    .unwrap();

    assert!(!summary.hooks_removed);
}
```

- [ ] **Step 4: Run hook and unadopt tests**

Run:

```bash
cargo test -p conary hook_paths_cover_all_supported_native_package_managers -- --nocapture
cargo test -p conary unadopt_ -- --nocapture
```

Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/adopt/hooks.rs apps/conary/src/commands/adopt/unadopt.rs
git commit -m "feat(adopt): remove sync hooks during full unadopt"
```

## Task 4: Guard Adopted Update Behavior And Native Authority

**Files:**
- Modify: `apps/conary/src/commands/update.rs`
- Test: `apps/conary/src/commands/update.rs`
- Test: `crates/conary-core/src/packages/mod.rs`

- [ ] **Step 1: Add package-manager identity and command tests**

Add helpers and tests in `crates/conary-core/src/packages/mod.rs` so adopted packages can prefer their recorded native-manager identity instead of relying only on live binary detection:

```rust
impl SystemPackageManager {
    pub fn from_version_scheme(scheme: Option<&str>) -> Option<Self> {
        match scheme {
            Some("rpm") => Some(Self::Rpm),
            Some("debian") => Some(Self::Dpkg),
            Some("arch") => Some(Self::Pacman),
            _ => None,
        }
    }
}

#[test]
fn native_update_commands_cover_supported_package_managers() {
    assert_eq!(SystemPackageManager::Rpm.update_command("curl"), "dnf update curl");
    assert_eq!(SystemPackageManager::Dpkg.update_command("curl"), "apt upgrade curl");
    assert_eq!(SystemPackageManager::Pacman.update_command("curl"), "pacman -Syu curl");
}

#[test]
fn native_manager_identity_comes_from_recorded_version_scheme() {
    assert_eq!(SystemPackageManager::from_version_scheme(Some("rpm")), Some(SystemPackageManager::Rpm));
    assert_eq!(SystemPackageManager::from_version_scheme(Some("debian")), Some(SystemPackageManager::Dpkg));
    assert_eq!(SystemPackageManager::from_version_scheme(Some("arch")), Some(SystemPackageManager::Pacman));
}
```

- [ ] **Step 2: Add adopted update classification and queuing tests**

In `apps/conary/src/commands/update.rs`, extract the adopted-package decision into a small pure helper and use it to gate whether an adopted package can enter the apply list:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdoptedUpdateDisposition {
    NativeAuthority,
    TrackOnly,
    ExplicitTakeover,
    BlockedCriticalTakeover,
}

fn classify_adopted_update(dep_mode: DepMode, is_critical: bool) -> AdoptedUpdateDisposition {
    match dep_mode {
        DepMode::Satisfy => AdoptedUpdateDisposition::NativeAuthority,
        DepMode::Adopt => AdoptedUpdateDisposition::TrackOnly,
        DepMode::Takeover if is_critical => AdoptedUpdateDisposition::BlockedCriticalTakeover,
        DepMode::Takeover => AdoptedUpdateDisposition::ExplicitTakeover,
    }
}

fn should_queue_adopted_update(disposition: AdoptedUpdateDisposition) -> bool {
    matches!(disposition, AdoptedUpdateDisposition::ExplicitTakeover)
}

#[cfg(test)]
mod adopted_update_tests {
    use super::*;

    #[test]
    fn adopted_updates_do_not_take_over_without_explicit_takeover_mode() {
        assert_eq!(
            classify_adopted_update(DepMode::Satisfy, false),
            AdoptedUpdateDisposition::NativeAuthority
        );
        assert_eq!(
            classify_adopted_update(DepMode::Adopt, false),
            AdoptedUpdateDisposition::TrackOnly
        );
    }

    #[test]
    fn adopted_updates_take_over_only_under_explicit_takeover_mode() {
        assert_eq!(
            classify_adopted_update(DepMode::Takeover, false),
            AdoptedUpdateDisposition::ExplicitTakeover
        );
    }

    #[test]
    fn critical_adopted_packages_are_blocked_even_under_takeover_mode() {
        assert_eq!(
            classify_adopted_update(DepMode::Takeover, true),
            AdoptedUpdateDisposition::BlockedCriticalTakeover
        );
    }

    #[test]
    fn adopted_updates_are_not_queued_under_satisfy_or_adopt() {
        assert!(!should_queue_adopted_update(classify_adopted_update(DepMode::Satisfy, false)));
        assert!(!should_queue_adopted_update(classify_adopted_update(DepMode::Adopt, false)));
        assert!(should_queue_adopted_update(classify_adopted_update(DepMode::Takeover, false)));
        assert!(!should_queue_adopted_update(classify_adopted_update(DepMode::Takeover, true)));
    }
}
```

- [ ] **Step 3: Use the helper in update selection**

Replace the inline adopted-package `match dep_mode` with `classify_adopted_update(dep_mode, super::install::is_package_blocked(&trove.name))`. Only push into `updates_available` when `should_queue_adopted_update(disposition)` is true. Preserve the existing messages, but make the `DepMode::Adopt` message explicit:

```rust
"  {} {} -> {} (adopted; native PM remains authoritative; native update delegation is not implemented yet)"
```

Do not write package files for `NativeAuthority` or `TrackOnly`. For skipped adopted packages, derive the printed native command from `SystemPackageManager::from_version_scheme(trove.version_scheme.as_deref())` before falling back to live detection.

If all candidates were skipped because they are adopted/native-authoritative, do not print `All packages are up to date`. Print a truthful summary such as:

```text
No Conary-owned packages were updated. Adopted packages remain native-authoritative; use the native package manager or explicit --dep-mode takeover.
```

Add an output-focused test, or a small extracted summary helper test, proving skipped adopted updates do not end with generic up-to-date wording.

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p conary adopted_update_tests -- --nocapture
cargo test -p conary-core native_update_commands_cover_supported_package_managers -- --nocapture
cargo test -p conary-core native_manager_identity_comes_from_recorded_version_scheme -- --nocapture
```

Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/update.rs crates/conary-core/src/packages/mod.rs
git commit -m "fix(update): keep adopted packages native-authoritative"
```

## Task 5: Guard Force Install Takeover Boundary

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Test: `apps/conary/src/commands/install/mod.rs`

- [ ] **Step 1: Add adopted-force boundary tests**

Add tests around the existing adopted-package install validation so `--force` cannot silently turn adopted packages into Conary-owned packages.

Choose one behavior and encode it explicitly:

- preferred first slice: reject `--force` over adopted packages and tell the user to use `--dep-mode takeover` or `system takeover`
- acceptable alternate: treat `--force` over adopted packages as explicit takeover, but only with takeover wording, blocklist handling, and tests matching `--dep-mode takeover`

For the preferred first slice, add tests like:

```rust
#[test]
fn force_install_over_adopted_package_is_not_silent_takeover() {
    let (_tmp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = conary_core::db::models::Trove::new_with_source(
        "curl".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::db::models::InstallSource::AdoptedFull,
    );
    trove.insert(&conn).unwrap();

    let err = validate_install_over_existing_adopted_for_tests(&conn, "curl", true).unwrap_err();

    assert!(err.to_string().contains("--dep-mode takeover"));
    assert!(err.to_string().contains("adopted"));
}
```

The exact helper name can follow the local install module patterns; the important part is that the test covers the adopted existing-trove path, not only CLI parsing.

- [ ] **Step 2: Implement the boundary**

Update the existing `force` handling so it no longer acts as silent takeover for adopted packages. If keeping `--force` as a takeover affordance, it must reuse the critical-package blocklist and print takeover-specific language before any old trove is deleted.

- [ ] **Step 3: Run focused tests**

```bash
cargo test -p conary force_install_over_adopted -- --nocapture
```

Expected: tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/conary/src/commands/install/mod.rs
git commit -m "fix(install): guard adopted force takeover"
```

## Task 6: Add RPM/DEB/Arch Integration Proof

**Files:**
- Modify: `apps/conary/tests/integration/remi/manifests/phase1-advanced.toml`
- Modify: `docs/INTEGRATION-TESTING.md`

- [ ] **Step 1: Add unadopt integration steps after T21**

In `phase1-advanced.toml`, add tests after `T21`:

```toml
# --------------------------------------------------------------------------
# T21a: Unadopt Dry Run
# --------------------------------------------------------------------------

[[test]]
id = "T21a"
name = "unadopt_single_package_dry_run"
description = "Preview unadopting curl without mutating native package files"
timeout = 30
depends_on = ["T21"]

[[test.step]]
conary = "system unadopt curl --dry-run"

[test.step.assert]
exit_code = 0
stdout_contains_all = ["Would unadopt", "curl", "Native package files will not be deleted"]

# --------------------------------------------------------------------------
# T21b: Unadopt Single Package
# --------------------------------------------------------------------------

[[test]]
id = "T21b"
name = "unadopt_single_package"
description = "Unadopt curl from Conary tracking while leaving the native package installed"
timeout = 60
depends_on = ["T21a"]

[[test.step]]
conary = "system unadopt curl --allow-live-system-mutation"

[test.step.assert]
exit_code = 0
stdout_contains_all = ["Unadopted", "curl", "Native package files were not deleted"]

[[test.step]]
run = "curl --version >/dev/null"

[test.step.assert]
exit_code = 0

[[test.step]]
conary = "list curl"

[test.step.assert]
exit_code = 0
stdout_not_contains = "curl"

# --------------------------------------------------------------------------
# T21c: Unadopt All
# --------------------------------------------------------------------------

[[test]]
id = "T21c"
name = "unadopt_all_packages"
description = "Unadopt all adopted packages while leaving native package files installed"
timeout = 90
depends_on = ["T21b"]

[[test.step]]
conary = "system adopt curl"

[test.step.assert]
exit_code = 0

[[test.step]]
conary = "system unadopt --all --dry-run"

[test.step.assert]
exit_code = 0
stdout_contains_all = ["Would unadopt", "curl"]

[[test.step]]
conary = "system unadopt --all --allow-live-system-mutation"

[test.step.assert]
exit_code = 0
stdout_contains_all = ["Unadopted", "curl", "Native package files were not deleted"]

[[test.step]]
run = "curl --version >/dev/null"

[test.step.assert]
exit_code = 0

[[test.step]]
conary = "list curl"

[test.step.assert]
exit_code = 0
stdout_not_contains = "curl"
```

Adjust exact assertion strings to match the implementation, but keep the test meaning unchanged.

- [ ] **Step 2: Run manifest inventory**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: command passes and the Phase 1 advanced suite includes `T21a`, `T21b`, and `T21c`.

- [ ] **Step 3: Run all three package-manager proofs**

Run:

```bash
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro ubuntu-26.04 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro arch --phase 1
```

Expected: all three runs pass. These are the required real tests for RPM, DEB, and Arch adoption escape behavior.

- [ ] **Step 4: Update integration docs**

In `docs/INTEGRATION-TESTING.md`, add a short note under Phase 1 that `T21a`/`T21b`/`T21c` prove non-destructive single-package and all-package unadoption across the supported distro matrix when run on `fedora44`, `ubuntu-26.04`, and `arch`.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/tests/integration/remi/manifests/phase1-advanced.toml docs/INTEGRATION-TESTING.md
git commit -m "test(adopt): prove unadopt across supported distros"
```

## Task 7: Refresh User-Facing Docs

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update README preview framing**

In `README.md`, update release-status, quick-start, `System Takeover`, `Project Status`, and `What's Next` copy so it says:

- the limited preview is adoption-led
- native package managers remain authoritative for adopted packages
- `conary --allow-live-system-mutation system unadopt --all` is the non-destructive apply-mode escape hatch on hosts without a selected Conary generation
- active-generation handoff remains a fail-closed limitation until that follow-up lands
- takeover is explicit and not the risk-free trial path
- ISO generation export is follow-up, not a near-term release blocker

- [ ] **Step 2: Ensure roadmap matches the implemented priority**

Verify `ROADMAP.md` has `Adopt Without Regret` as the first current-focus section and `system unadopt` as the first near-term priority.

- [ ] **Step 3: Update doc audit metadata**

Run:

```bash
bash scripts/docs-audit-inventory.sh
```

Then update `docs/superpowers/documentation-accuracy-audit-ledger.tsv` for any new or changed docs so the ledger remains complete. The README ledger row must mention adoption-led preview, unadoption, explicit takeover, active-generation handoff limits, and ISO export follow-up.

- [ ] **Step 4: Run docs checks**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
rg -n "replace dnf|replace apt|replace pacman|risk-free|unadopt|takeover" README.md ROADMAP.md docs
```

Expected: ledger passes. Search output should show adoption/unadoption language and should not leave limited-preview copy implying silent package-manager replacement.

- [ ] **Step 5: Commit**

```bash
git add README.md ROADMAP.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: frame preview around adopt without regret"
```

## Task 8: Final Verification

**Files:**
- No direct edits unless verification exposes a defect.

- [ ] **Step 1: Run focused Rust checks**

```bash
cargo test -p conary system_unadopt -- --nocapture
cargo test -p conary unadopt_ -- --nocapture
cargo test -p conary adopted_update_tests -- --nocapture
cargo test -p conary force_install_over_adopted -- --nocapture
cargo test -p conary-core native_update_commands_cover_supported_package_managers -- --nocapture
cargo test -p conary-core native_manager_identity_comes_from_recorded_version_scheme -- --nocapture
```

Expected: pass.

- [ ] **Step 2: Run package-manager integration proof**

```bash
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro ubuntu-26.04 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro arch --phase 1
```

Expected: pass on all three supported package-manager families.

- [ ] **Step 3: Run workspace gates**

```bash
cargo fmt --check
cargo run -p conary-test -- list
cargo test -p conary-test
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: pass.

- [ ] **Step 4: Record any skipped validation**

If any distro integration run cannot execute because of local container, network, or host constraints, record the exact command, failure reason, and the replacement evidence in the goal summary. Do not mark the `/goal` complete without either passing the command or documenting the blocker.

- [ ] **Step 5: Final commit if verification required fixes**

```bash
git status --short
git add <only-files-fixed-during-verification>
git commit -m "fix: complete adopt without regret verification"
```

Expected: no commit is needed unless verification found a focused fix.

## Completion Criteria

- `conary system unadopt` exists and is documented.
- Unadoption removes Conary tracking for adopted packages and leaves native package files intact.
- Apply-mode unadoption refuses before DB mutation when a Conary generation is selected.
- `--all` is the one-command escape hatch on hosts without a selected Conary generation and removes sync hooks unless `--keep-hooks` is passed.
- Native package-manager identity is recorded or derived per adopted package instead of relying only on live binary detection.
- Update tests prove adopted packages remain native-authoritative unless explicit takeover is selected and do not print misleading "up to date" output when skipped.
- `install --force` cannot silently replace an adopted package with a Conary-owned package.
- Fedora 44, Ubuntu 26.04 LTS, and Arch integration runs prove the same unadopt behavior for RPM, DEB, and Arch.
- Roadmap and README present adoption as the limited-preview path and takeover as explicit opt-in.
