# Native Package Manager Parity Slice C Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Tier 1 daily-driver commands share the same package selector, authority, pin, dependency, and ambiguity contracts as Conary-owned install/remove/update.

**Architecture:** Add one shared installed-package selector for commands that act on an installed package row, then wire the existing CLI surface through it instead of adding parallel command names. Keep `query whatprovides` and `query whatbreaks` under `conary query` for this slice. Tighten `autoremove` by classifying Conary-owned orphan candidates separately from adopted, pinned, and critical packages before any mutation.

**Tech Stack:** Rust, clap, rusqlite, existing Conary command modules, CLI integration tests under `apps/conary/tests`, and the existing documentation audit ledger.

---

## Codex Goal

Use this as the `/goal` text for the implementation run:

```text
Slice C: Implement Tier 1 daily-driver command parity. Make search/list/info/files/path/pin/unpin/pinned/autoremove/system history/query whatprovides/query whatbreaks/repo list/repo sync share the same package selector, authority, pin, dependency, and ambiguity contracts as mutation commands. Prove with CLI and integration tests while keeping top-level whatprovides/whatbreaks aliases out of scope.
```

## Source Documents

- Design spec: `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`
- Slice A plan: `docs/superpowers/plans/2026-05-14-native-package-manager-parity-slice-a-plan.md`
- Slice B plan: `docs/superpowers/plans/2026-05-15-native-package-manager-parity-slice-b-plan.md`
- Roadmap focus: `ROADMAP.md`, section "No Step Down Package Flows"

## Scope

In scope:

- Shared installed-package selector for name + optional version + optional architecture.
- `conary remove --arch <arch>` dispatch wiring, using the already-supported `cmd_remove` architecture argument.
- `conary update <pkg> --version <installed-version> --arch <arch>` as an installed-variant selector.
- `conary pin`, `conary unpin`, `conary list --info`, and `conary list --files` using the same selector and ambiguity wording.
- `conary list --pinned` output that includes enough identity to disambiguate variants.
- `conary query whatprovides` reporting installed and synced repository providers where metadata exists.
- `conary query whatbreaks` reporting the same preflight blockers that `remove` would enforce.
- `conary autoremove --dry-run` and `conary autoremove` classifying Conary-owned orphan candidates and skipped packages truthfully.
- CLI and unit tests proving selector ambiguity, pin/update/remove consistency, query diagnostics, and autoremove behavior.

Out of scope:

- Top-level `conary whatprovides` or `conary whatbreaks` aliases.
- Full Fedora/Ubuntu/Arch conary-test matrix. That remains Slice D.
- Native package-manager transaction history import/export.
- Distro-specific native hold policy beyond Conary pinning.
- Repository priority redesign beyond existing source-policy behavior.

## File Structure

- Create `apps/conary/src/commands/package_target.rs`: shared installed-package selector, variant formatting, authority labels, and tests.
- Modify `apps/conary/src/commands/mod.rs`: register and re-export the selector helpers for command modules.
- Modify `apps/conary/src/cli/mod.rs`: add installed-variant selector flags to commands that need them.
- Modify `apps/conary/src/dispatch.rs`: pass selector fields into command handlers.
- Modify `apps/conary/src/commands/query/package.rs`: use the shared selector for detailed package queries and improve source/authority output.
- Modify `apps/conary/src/commands/query/dependency.rs`: use the shared selector for `whatbreaks` and add repository-provider output to `whatprovides`.
- Modify `apps/conary/src/commands/update.rs`: use the shared selector for `pin`, `unpin`, and package-specific update selection.
- Modify `apps/conary/src/commands/remove.rs`: use the shared selector and classify autoremove candidates before mutation.
- Modify `apps/conary/tests/common/mod.rs`: add helper fixtures for ambiguous variants, pinned packages, orphans, adopted packages, and repository provides.
- Create `apps/conary/tests/native_pm_daily_driver.rs`: CLI proof for Tier 1 daily-driver flows.
- Modify docs and audit files after implementation: `README.md`, `docs/conaryopedia-v2.md`, `docs/INTEGRATION-TESTING.md`, and `docs/superpowers/documentation-accuracy-audit-*`.

---

## Task 1: Add A Shared Installed-Package Selector

**Files:**

- Create: `apps/conary/src/commands/package_target.rs`
- Modify: `apps/conary/src/commands/mod.rs`

- [ ] **Step 1: Add failing selector tests**

Create `apps/conary/src/commands/package_target.rs` with the module path comment and these tests first:

```rust
// apps/conary/src/commands/package_target.rs

use anyhow::Result;
use conary_core::db::models::Trove;

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{InstallSource, Trove, TroveType};

    fn db_with_variants() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();

        let mut x86 = Trove::new_with_source(
            "demo".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        x86.architecture = Some("x86_64".to_string());
        x86.insert(&conn).unwrap();

        let mut arm = Trove::new_with_source(
            "demo".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        arm.architecture = Some("aarch64".to_string());
        arm.insert(&conn).unwrap();

        conn
    }

    #[test]
    fn selector_refuses_ambiguous_package_without_variant_fields() {
        let conn = db_with_variants();
        let selector = InstalledPackageSelector::new("demo".to_string(), None, None);

        let err = resolve_installed_package(&conn, &selector)
            .unwrap_err()
            .to_string();

        assert!(err.contains("Multiple installed variants of 'demo' match"));
        assert!(err.contains("version 1.0.0 [x86_64]"));
        assert!(err.contains("version 1.0.0 [aarch64]"));
        assert!(err.contains("--version"));
        assert!(err.contains("--arch"));
    }

    #[test]
    fn selector_resolves_version_and_architecture() {
        let conn = db_with_variants();
        let selector = InstalledPackageSelector::new(
            "demo".to_string(),
            Some("1.0.0".to_string()),
            Some("aarch64".to_string()),
        );

        let resolved = resolve_installed_package(&conn, &selector).unwrap();

        assert_eq!(resolved.trove.name, "demo");
        assert_eq!(resolved.trove.version, "1.0.0");
        assert_eq!(resolved.trove.architecture.as_deref(), Some("aarch64"));
    }

    #[test]
    fn selector_reports_available_variants_when_filter_matches_none() {
        let conn = db_with_variants();
        let selector = InstalledPackageSelector::new(
            "demo".to_string(),
            Some("2.0.0".to_string()),
            Some("x86_64".to_string()),
        );

        let err = resolve_installed_package(&conn, &selector)
            .unwrap_err()
            .to_string();

        assert!(err.contains("Package 'demo' with selector"));
        assert!(err.contains("Installed variants:"));
        assert!(err.contains("1.0.0 [x86_64]"));
    }
}
```

- [ ] **Step 2: Run the new tests and verify they fail**

Run:

```bash
cargo test -p conary package_target -- --nocapture
```

Expected: compile failure because `InstalledPackageSelector`, `ResolvedInstalledPackage`, and `resolve_installed_package` do not exist yet.

- [ ] **Step 3: Implement the selector**

Replace the top of `package_target.rs` with the implementation below, keeping the tests from Step 1 at the bottom:

```rust
// apps/conary/src/commands/package_target.rs

//! Shared installed-package selector and rendering helpers.

use anyhow::Result;
use conary_core::db::models::{InstallSource, Trove};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledPackageSelector {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) architecture: Option<String>,
}

impl InstalledPackageSelector {
    pub(crate) fn new(
        name: String,
        version: Option<String>,
        architecture: Option<String>,
    ) -> Self {
        Self {
            name,
            version,
            architecture,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedInstalledPackage {
    pub(crate) trove: Trove,
    pub(crate) trove_id: i64,
}

pub(crate) fn resolve_installed_package(
    conn: &rusqlite::Connection,
    selector: &InstalledPackageSelector,
) -> Result<ResolvedInstalledPackage> {
    let troves = Trove::find_by_name(conn, &selector.name)?;

    if troves.is_empty() {
        anyhow::bail!("Package '{}' is not installed", selector.name);
    }

    let matches = matching_installed_packages(&troves, selector);
    match matches.as_slice() {
        [] => anyhow::bail!(
            "Package '{}' with selector version={:?} architecture={:?} is not installed. Installed variants: {}",
            selector.name,
            selector.version,
            selector.architecture,
            format_installed_variants(&troves)
        ),
        [trove] => {
            let trove_id = trove
                .id
                .ok_or_else(|| anyhow::anyhow!("Package '{}' has no database ID", selector.name))?;
            Ok(ResolvedInstalledPackage {
                trove: (*trove).clone(),
                trove_id,
            })
        }
        _ => {
            let variants = matches
                .iter()
                .map(|trove| format!("  - {}", format_installed_variant(trove)))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "Multiple installed variants of '{}' match the selector:\n{}\nUse --version and/or --arch to choose one.",
                selector.name,
                variants
            )
        }
    }
}

pub(crate) fn matching_installed_packages<'a>(
    troves: &'a [Trove],
    selector: &InstalledPackageSelector,
) -> Vec<&'a Trove> {
    troves
        .iter()
        .filter(|trove| {
            selector
                .version
                .as_deref()
                .is_none_or(|version| trove.version == version)
                && selector
                    .architecture
                    .as_deref()
                    .is_none_or(|arch| trove.architecture.as_deref() == Some(arch))
        })
        .collect()
}

pub(crate) fn format_installed_variant(trove: &Trove) -> String {
    format!(
        "version {} [{}] ({}, {})",
        trove.version,
        trove.architecture.as_deref().unwrap_or("none"),
        package_authority_label(trove.install_source.clone()),
        trove.version_scheme.as_deref().unwrap_or("unknown-scheme")
    )
}

pub(crate) fn format_installed_variants(troves: &[Trove]) -> String {
    troves
        .iter()
        .map(format_installed_variant)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn package_authority_label(source: InstallSource) -> &'static str {
    if source.is_adopted() {
        "native-authority"
    } else {
        "conary-owned"
    }
}
```

- [ ] **Step 4: Export the selector module**

In `apps/conary/src/commands/mod.rs`, add the module:

```rust
mod package_target;
```

Add a crate-visible re-export near the other `pub(crate) use` blocks:

```rust
pub(crate) use package_target::{
    InstalledPackageSelector, ResolvedInstalledPackage, format_installed_variant,
    format_installed_variants, package_authority_label, resolve_installed_package,
};
```

- [ ] **Step 5: Run the selector tests**

Run:

```bash
cargo test -p conary package_target -- --nocapture
```

Expected: all selector tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/package_target.rs apps/conary/src/commands/mod.rs
git commit -m "feat(cli): add shared installed package selector"
```

---

## Task 2: Add Installed-Variant Selector Flags To The CLI

**Files:**

- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch.rs`

- [ ] **Step 1: Add failing CLI help assertions**

Add tests to the `#[cfg(test)]` section of `apps/conary/src/cli/mod.rs`. If no CLI parser tests exist, add this module at the bottom:

```rust
#[cfg(test)]
mod selector_flag_tests {
    use super::*;
    use clap::CommandFactory;

    fn help_for(args: &[&str]) -> String {
        let mut command = Cli::command();
        let subcommand = command.find_subcommand_mut(args[0]).unwrap();
        let mut help = Vec::new();
        subcommand.write_long_help(&mut help).unwrap();
        String::from_utf8(help).unwrap()
    }

    #[test]
    fn remove_update_pin_unpin_and_list_expose_arch_selector() {
        for command in ["remove", "update", "pin", "unpin", "list"] {
            let help = help_for(&[command]);
            assert!(help.contains("--arch"), "{command} help:\n{help}");
        }
    }

    #[test]
    fn update_pin_unpin_and_list_expose_version_selector() {
        for command in ["update", "pin", "unpin", "list"] {
            let help = help_for(&[command]);
            assert!(help.contains("--version"), "{command} help:\n{help}");
        }
    }
}
```

Run:

```bash
cargo test -p conary selector_flag_tests -- --nocapture
```

Expected: fails until the flags exist.

- [ ] **Step 2: Add selector fields to CLI structs**

In `apps/conary/src/cli/mod.rs`, add:

```rust
        /// Specific architecture to remove when multiple variants are installed
        #[arg(long = "arch")]
        architecture: Option<String>,
```

to `Commands::Remove`.

Add this pair to `Commands::Update`, `Commands::Pin`, `Commands::Unpin`, and `Commands::List`:

```rust
        /// Installed package version to select when multiple variants are installed
        #[arg(short, long)]
        version: Option<String>,

        /// Installed package architecture to select when multiple variants are installed
        #[arg(long = "arch")]
        architecture: Option<String>,
```

For `Commands::List`, keep the fields after `pattern` and before `db` so the selector fields are visually grouped with the package pattern.

- [ ] **Step 3: Pass selector fields through dispatch**

In `apps/conary/src/dispatch.rs`, update the `Remove` match arm to bind `architecture` and pass it into `cmd_remove` instead of `None`.

Update the `Update`, `List`, `Pin`, and `Unpin` match arms to bind `version` and `architecture`. For `List`, extend `QueryOptions`:

```rust
let options = commands::QueryOptions {
    info,
    lsl,
    path,
    files,
    version,
    architecture,
};
```

For `Pin` and `Unpin`, construct selectors:

```rust
let selector = commands::InstalledPackageSelector::new(package_name, version, architecture);
commands::cmd_pin(selector, &db.db_path).await
```

and:

```rust
let selector = commands::InstalledPackageSelector::new(package_name, version, architecture);
commands::cmd_unpin(selector, &db.db_path).await
```

For `Update`, keep the function call compiling by temporarily passing the new fields after `yes`:

```rust
commands::cmd_update(
    package,
    &common.db.db_path,
    &common.root,
    security,
    dry_run,
    sandbox.into(),
    dep_mode,
    yes,
    version,
    architecture,
)
.await
```

Task 4 updates `cmd_update` itself.

- [ ] **Step 4: Extend `QueryOptions`**

In `apps/conary/src/commands/query/mod.rs`, add:

```rust
    /// Installed package version selector for detailed package operations
    pub version: Option<String>,
    /// Installed package architecture selector for detailed package operations
    pub architecture: Option<String>,
```

- [ ] **Step 5: Run parser and formatting checks**

Run:

```bash
cargo fmt
cargo test -p conary selector_flag_tests -- --nocapture
```

Expected: parser tests pass; compilation may still fail where command signatures have not been updated. Continue to Task 3 and Task 4 before committing this task.

---

## Task 3: Wire Selector Into List, Info, Files, Pin, And Unpin

**Files:**

- Modify: `apps/conary/src/commands/query/package.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `apps/conary/tests/query.rs`

- [ ] **Step 1: Add failing command-level tests for ambiguity and output**

In `apps/conary/tests/query.rs`, add tests using direct model assertions and binary CLI assertions:

```rust
fn run_conary(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn output_text(output: &std::process::Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn list_info_refuses_ambiguous_variants_until_selector_is_given() {
    use conary_core::db::models::{Trove, TroveType};

    let (_tmp, db_path, conn) = common::create_test_db();
    for arch in ["x86_64", "aarch64"] {
        let mut trove = Trove::new("variant-demo".to_string(), "1.0.0".to_string(), TroveType::Package);
        trove.architecture = Some(arch.to_string());
        trove.insert(&conn).unwrap();
    }

    let ambiguous = run_conary(&["list", "variant-demo", "--info", "--db-path", &db_path]);
    assert!(!ambiguous.status.success(), "{}", output_text(&ambiguous));
    let text = output_text(&ambiguous);
    assert!(text.contains("Multiple installed variants of 'variant-demo' match"), "{text}");
    assert!(text.contains("--arch"), "{text}");

    let selected = run_conary(&[
        "list",
        "variant-demo",
        "--info",
        "--version",
        "1.0.0",
        "--arch",
        "aarch64",
        "--db-path",
        &db_path,
    ]);
    assert!(selected.status.success(), "{}", output_text(&selected));
    let stdout = String::from_utf8_lossy(&selected.stdout);
    assert!(stdout.contains("Architecture: aarch64"), "{stdout}");
}

#[test]
fn pin_and_unpin_use_same_variant_selector() {
    use conary_core::db::models::{Trove, TroveType};

    let (_tmp, db_path, conn) = common::create_test_db();
    for arch in ["x86_64", "aarch64"] {
        let mut trove = Trove::new("pin-demo".to_string(), "1.0.0".to_string(), TroveType::Package);
        trove.architecture = Some(arch.to_string());
        trove.insert(&conn).unwrap();
    }

    let ambiguous = run_conary(&["pin", "pin-demo", "--db-path", &db_path]);
    assert!(!ambiguous.status.success(), "{}", output_text(&ambiguous));

    let pin = run_conary(&[
        "pin",
        "pin-demo",
        "--version",
        "1.0.0",
        "--arch",
        "x86_64",
        "--db-path",
        &db_path,
    ]);
    assert!(pin.status.success(), "{}", output_text(&pin));

    let pinned = run_conary(&["list", "--pinned", "--db-path", &db_path]);
    assert!(pinned.status.success(), "{}", output_text(&pinned));
    let stdout = String::from_utf8_lossy(&pinned.stdout);
    assert!(stdout.contains("pin-demo 1.0.0 [x86_64]"), "{stdout}");

    let unpin = run_conary(&[
        "unpin",
        "pin-demo",
        "--version",
        "1.0.0",
        "--arch",
        "x86_64",
        "--db-path",
        &db_path,
    ]);
    assert!(unpin.status.success(), "{}", output_text(&unpin));
}
```

Run:

```bash
cargo test -p conary list_info_refuses_ambiguous_variants_until_selector_is_given pin_and_unpin_use_same_variant_selector -- --nocapture
```

Expected: fails until selector wiring is complete.

- [ ] **Step 2: Use selector in `cmd_query` for detailed package actions**

In `apps/conary/src/commands/query/package.rs`, import:

```rust
use crate::commands::{
    InstalledPackageSelector, package_authority_label, resolve_installed_package,
};
```

Change the detailed branches in `cmd_query` so `--info`, `--files`, and `--lsl` resolve exactly one package:

```rust
    if options.info || options.files || options.lsl {
        let package_name = pattern.ok_or_else(|| {
            anyhow::anyhow!("A package name is required with --info, --files, or --lsl")
        })?;
        let selector = InstalledPackageSelector::new(
            package_name.to_string(),
            options.version.clone(),
            options.architecture.clone(),
        );
        let resolved = resolve_installed_package(&conn, &selector)?;

        if options.info {
            return show_package_info(&conn, &resolved.trove, &options);
        }
        return list_package_files(&conn, &resolved.trove, options.lsl);
    }
```

Leave plain `conary list [pattern]` as a multi-row listing.

- [ ] **Step 3: Show authority/source details in package info**

In `show_package_info`, after `Type`, print:

```rust
    println!(
        "Authority   : {}",
        package_authority_label(trove.install_source.clone())
    );
    println!("Source      : {}", trove.install_source.as_str());
    if let Some(source_distro) = &trove.source_distro {
        println!("Distro      : {}", source_distro);
    }
    if let Some(version_scheme) = &trove.version_scheme {
        println!("Versioning  : {}", version_scheme);
    }
    if let Some(repository_id) = trove.installed_from_repository_id {
        println!("Repository  : {}", repository_id);
    }
```

- [ ] **Step 4: Use selector in pin and unpin**

In `apps/conary/src/commands/update.rs`, replace `cmd_pin` and `cmd_unpin` signatures:

```rust
pub async fn cmd_pin(selector: InstalledPackageSelector, db_path: &str) -> Result<()>
pub async fn cmd_unpin(selector: InstalledPackageSelector, db_path: &str) -> Result<()>
```

Import:

```rust
use super::{InstalledPackageSelector, resolve_installed_package};
```

Resolve the row:

```rust
let resolved = resolve_installed_package(&conn, &selector)?;
let trove = resolved.trove;
let trove_id = resolved.trove_id;
```

Use `trove.name` in user-facing messages so the exact selected row is reported.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p conary list_info_refuses_ambiguous_variants_until_selector_is_given pin_and_unpin_use_same_variant_selector -- --nocapture
cargo test -p conary query::package update::tests -- --nocapture
```

Expected: tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/dispatch.rs apps/conary/src/commands/query/mod.rs apps/conary/src/commands/query/package.rs apps/conary/src/commands/update.rs apps/conary/tests/query.rs
git commit -m "feat(cli): share selector across list and pin commands"
```

---

## Task 4: Wire Selector Into Remove And Package-Specific Update

**Files:**

- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `apps/conary/tests/native_pm_live_root.rs`

- [ ] **Step 1: Add failing tests for selector consistency**

In `apps/conary/tests/native_pm_live_root.rs`, add tests that seed ambiguous package rows without needing package downloads:

```rust
#[test]
fn remove_arch_selector_targets_one_variant() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    for arch in ["x86_64", "aarch64"] {
        let payload = root.path().join(format!("usr/bin/remove-demo-{arch}"));
        fs::create_dir_all(payload.parent().unwrap()).unwrap();
        fs::write(&payload, arch).unwrap();

        let mut trove = conary_core::db::models::Trove::new_with_source(
            "remove-demo".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
            conary_core::db::models::InstallSource::Repository,
        );
        trove.architecture = Some(arch.to_string());
        let trove_id = trove.insert(&conn).unwrap();
        conary_core::db::models::FileEntry::new(
            format!("/usr/bin/remove-demo-{arch}"),
            "0".repeat(64),
            arch.len() as i64,
            0o100755,
            trove_id,
        )
        .insert(&conn)
        .unwrap();
    }
    drop(conn);

    let ambiguous = run_conary(&[
        "--allow-live-system-mutation",
        "remove",
        "remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert!(!ambiguous.status.success(), "{}", output_text(&ambiguous));

    let selected = run_conary(&[
        "--allow-live-system-mutation",
        "remove",
        "remove-demo",
        "--version",
        "1.0.0",
        "--arch",
        "aarch64",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert_success(&selected);
    assert!(root.path().join("usr/bin/remove-demo-x86_64").exists());
    assert!(!root.path().join("usr/bin/remove-demo-aarch64").exists());
}
```

Run:

```bash
cargo test -p conary remove_arch_selector_targets_one_variant -- --nocapture
```

Expected: fails until `--arch` dispatch is wired and remove uses the shared selector.

- [ ] **Step 2: Replace remove's local selector with the shared selector**

In `apps/conary/src/commands/remove.rs`, import:

```rust
use super::{InstalledPackageSelector, resolve_installed_package};
```

Replace the manual `troves` / `matches` block at the start of `cmd_remove` with:

```rust
let selector = InstalledPackageSelector::new(
    package_name.to_string(),
    version.clone(),
    architecture.clone(),
);
let resolved = resolve_installed_package(&conn, &selector)
    .with_context(|| format!("Failed to select package '{}'", package_name))?;
let trove = resolved.trove;
```

Keep the pin, critical-package, adopted-authority, dependency-breakage, and execution-path checks after that point.

- [ ] **Step 3: Add package-specific update selector tests**

In `apps/conary/src/commands/update.rs`, add a unit test near update selection tests:

```rust
#[test]
fn package_specific_update_requires_selector_for_ambiguous_variants() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();

    for arch in ["x86_64", "aarch64"] {
        let mut trove = Trove::new("demo".to_string(), "1.0.0".to_string(), conary_core::db::models::TroveType::Package);
        trove.architecture = Some(arch.to_string());
        trove.insert(&conn).unwrap();
    }

    let selector = InstalledPackageSelector::new("demo".to_string(), None, None);
    let err = resolve_installed_package(&conn, &selector)
        .unwrap_err()
        .to_string();
    assert!(err.contains("Multiple installed variants"));
}
```

This is a small guard around the shared helper. The CLI-level behavior is covered by Task 7.

- [ ] **Step 4: Use selector for package-specific update**

Update `cmd_update` to accept:

```rust
    package_version: Option<String>,
    architecture: Option<String>,
```

after `yes`.

Replace:

```rust
let installed_troves = if let Some(pkg_name) = package {
    Trove::find_by_name(&conn, &pkg_name)?
} else {
    Trove::list_all(&conn)?
};
```

with:

```rust
let installed_troves = if let Some(pkg_name) = package {
    let selector = InstalledPackageSelector::new(pkg_name, package_version, architecture);
    vec![resolve_installed_package(&conn, &selector)?.trove]
} else {
    Trove::list_all(&conn)?
};
```

- [ ] **Step 5: Run focused update/remove tests**

Run:

```bash
cargo test -p conary remove_arch_selector_targets_one_variant -- --nocapture
cargo test -p conary package_specific_update_requires_selector_for_ambiguous_variants -- --nocapture
cargo test -p conary security_update -- --nocapture
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/remove.rs apps/conary/src/commands/update.rs apps/conary/tests/native_pm_live_root.rs
git commit -m "feat(cli): align remove and update selectors"
```

---

## Task 5: Make Whatprovides And Whatbreaks Match Real Preflight

**Files:**

- Modify: `apps/conary/src/commands/query/dependency.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/tests/query.rs`

- [ ] **Step 1: Add failing tests for provider and breakage diagnostics**

In `apps/conary/tests/query.rs`, add:

```rust
#[test]
fn whatprovides_reports_installed_and_repository_providers() {
    use conary_core::db::models::{Repository, RepositoryPackage, RepositoryProvide};

    let (_tmp, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    let mut repo = Repository::new("daily-driver".to_string(), "https://example.test/repo".to_string());
    repo.gpg_check = false;
    repo.gpg_strict = false;
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "openssl-libs".to_string(),
        "3.1.0".to_string(),
        "0".repeat(64),
        10,
        "https://example.test/openssl-libs.ccs".to_string(),
    );
    pkg.architecture = Some("x86_64".to_string());
    let repo_pkg_id = pkg.insert(&conn).unwrap();

    RepositoryProvide::new(
        repo_pkg_id,
        "soname(libssl.so.3)".to_string(),
        None,
        "soname".to_string(),
        None,
    )
    .insert(&conn)
    .unwrap();

    let output = run_conary(&[
        "query",
        "whatprovides",
        "soname(libssl.so.3)",
        "--db-path",
        &db_path,
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Installed providers:"), "{stdout}");
    assert!(stdout.contains("openssl 3.0.0"), "{stdout}");
    assert!(stdout.contains("Repository providers:"), "{stdout}");
    assert!(stdout.contains("openssl-libs 3.1.0 [x86_64] @daily-driver"), "{stdout}");
}

#[test]
fn whatbreaks_reports_same_dependency_blocker_as_remove() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let whatbreaks = run_conary(&["query", "whatbreaks", "openssl", "--db-path", &db_path]);
    assert!(whatbreaks.status.success(), "{}", output_text(&whatbreaks));
    let stdout = String::from_utf8_lossy(&whatbreaks.stdout);
    assert!(stdout.contains("Removing 'openssl' would break"), "{stdout}");
    assert!(stdout.contains("nginx"), "{stdout}");

    let remove = run_conary(&[
        "--allow-live-system-mutation",
        "remove",
        "openssl",
        "--db-path",
        &db_path,
        "--root",
        _tmp.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert!(!remove.status.success(), "{}", output_text(&remove));
    assert!(output_text(&remove).contains("nginx"));
}
```

Run:

```bash
cargo test -p conary whatprovides_reports_installed_and_repository_providers whatbreaks_reports_same_dependency_blocker_as_remove -- --nocapture
```

Expected: `whatprovides` fails until repository providers are printed.

- [ ] **Step 2: Fix remove's hint**

In `apps/conary/src/commands/remove.rs`, replace the old hint:

```rust
"Use 'conary whatbreaks {}' for more information."
```

with:

```rust
"Use 'conary query whatbreaks {}' for more information."
```

- [ ] **Step 3: Add repository-provider output to `cmd_whatprovides`**

In `apps/conary/src/commands/query/dependency.rs`, import:

```rust
use conary_core::db::models::{Repository, RepositoryPackage, RepositoryProvide};
```

After installed-provider lookup, add repository lookup:

```rust
let repo_providers = RepositoryProvide::find_by_capability(&conn, capability)?;
```

Print installed and repository providers in separate sections. For each repository provider, load the package and repository:

```rust
if !repo_providers.is_empty() {
    println!("Repository providers:");
    for provide in &repo_providers {
        let Some(pkg) = RepositoryPackage::find_by_id(&conn, provide.repository_package_id)? else {
            continue;
        };
        let repo_name = Repository::find_by_id(&conn, pkg.repository_id)?
            .map(|repo| repo.name)
            .unwrap_or_else(|| "unknown-repo".to_string());
        print!("  {} {}", pkg.name, pkg.version);
        if let Some(arch) = &pkg.architecture {
            print!(" [{}]", arch);
        }
        print!(" @{}", repo_name);
        if let Some(version) = &provide.version {
            print!(" (provides version: {})", version);
        }
        println!();
    }
}
```

If both sections are empty, keep the existing no-provider message.

- [ ] **Step 4: Use shared selector and preflight language in `cmd_whatbreaks`**

Change `cmd_whatbreaks` to accept only the package name for now, but resolve through:

```rust
let selector = InstalledPackageSelector::new(package_name.to_string(), None, None);
let resolved = resolve_installed_package(&conn, &selector)?;
let trove = resolved.trove;
```

Then report preflight blockers without mutating:

```rust
if trove.pinned {
    println!(
        "Package '{}' is pinned and remove would be refused before mutation.",
        trove.name
    );
}
if crate::commands::install::is_package_blocked(&trove.name) {
    println!(
        "Package '{}' is critical and remove would be refused before mutation.",
        trove.name
    );
}
if trove.install_source.is_adopted() {
    println!(
        "Package '{}' is adopted; native package-manager authority is preserved.",
        trove.name
    );
}
```

Keep the `solve_removal` output so dependency diagnostics match remove behavior.

- [ ] **Step 5: Run query tests**

Run:

```bash
cargo test -p conary whatprovides_reports_installed_and_repository_providers whatbreaks_reports_same_dependency_blocker_as_remove -- --nocapture
cargo test -p conary test_whatprovides_query test_what_breaks_analysis -- --nocapture
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/query/dependency.rs apps/conary/src/commands/remove.rs apps/conary/tests/query.rs
git commit -m "feat(query): align provider and breakage diagnostics"
```

---

## Task 6: Tighten Autoremove Classification And Outcomes

**Files:**

- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/tests/native_pm_daily_driver.rs`

- [ ] **Step 1: Add failing CLI tests for autoremove**

Create `apps/conary/tests/native_pm_daily_driver.rs` with:

```rust
// apps/conary/tests/native_pm_daily_driver.rs

mod common;

use conary_core::db;
use conary_core::db::models::{FileEntry, InstallReason, InstallSource, Trove, TroveType};
use std::fs;
use std::process::{Command, Output};

fn run_conary(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn seed_orphan(
    conn: &rusqlite::Connection,
    root: &std::path::Path,
    name: &str,
    source: InstallSource,
) {
    let payload = root.join(format!("usr/share/{name}/payload.txt"));
    fs::create_dir_all(payload.parent().unwrap()).unwrap();
    fs::write(&payload, name).unwrap();

    let mut trove = Trove::new_with_source(
        name.to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        source,
    );
    trove.install_reason = InstallReason::Dependency;
    trove.selection_reason = Some("Required by removed-parent".to_string());
    let trove_id = trove.insert(conn).unwrap();
    FileEntry::new(
        format!("/usr/share/{name}/payload.txt"),
        "0".repeat(64),
        name.len() as i64,
        0o100644,
        trove_id,
    )
    .insert(conn)
    .unwrap();
}

#[test]
fn autoremove_dry_run_lists_conary_owned_orphans_and_skips_adopted() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_orphan(&conn, root.path(), "owned-orphan", InstallSource::Repository);
    seed_orphan(&conn, root.path(), "adopted-orphan", InstallSource::AdoptedTrack);
    drop(conn);

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "autoremove",
        "--dry-run",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("owned-orphan 1.0.0"), "{stdout}");
    assert!(stdout.contains("Skipping adopted orphaned package(s)"), "{stdout}");
    assert!(stdout.contains("adopted-orphan"), "{stdout}");
}

#[test]
fn autoremove_apply_removes_owned_orphan_without_deleting_adopted_orphan() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_orphan(&conn, root.path(), "owned-orphan", InstallSource::Repository);
    seed_orphan(&conn, root.path(), "adopted-orphan", InstallSource::AdoptedTrack);
    drop(conn);

    let output = run_conary(&[
        "--allow-live-system-mutation",
        "autoremove",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(!root.path().join("usr/share/owned-orphan/payload.txt").exists());
    assert!(root.path().join("usr/share/adopted-orphan/payload.txt").exists());

    let conn = db::open(&db_path).unwrap();
    assert!(Trove::find_by_name(&conn, "owned-orphan").unwrap().is_empty());
    assert_eq!(Trove::find_by_name(&conn, "adopted-orphan").unwrap().len(), 1);
}
```

Run:

```bash
cargo test -p conary autoremove_dry_run_lists_conary_owned_orphans_and_skips_adopted autoremove_apply_removes_owned_orphan_without_deleting_adopted_orphan -- --nocapture
```

Expected: fails until autoremove classifies adopted orphans instead of passing them into `cmd_remove`.

- [ ] **Step 2: Add autoremove classification helpers**

In `apps/conary/src/commands/remove.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum AutoremoveSkipReason {
    AdoptedNativeAuthority,
    Pinned,
    Critical,
}

#[derive(Debug, Clone)]
struct AutoremovePlan {
    removable: Vec<Trove>,
    skipped: Vec<(Trove, AutoremoveSkipReason)>,
}

fn plan_autoremove(orphaned: Vec<Trove>) -> AutoremovePlan {
    let mut removable = Vec::new();
    let mut skipped = Vec::new();

    for trove in orphaned {
        if trove.install_source.is_adopted() {
            skipped.push((trove, AutoremoveSkipReason::AdoptedNativeAuthority));
        } else if trove.pinned {
            skipped.push((trove, AutoremoveSkipReason::Pinned));
        } else if crate::commands::install::is_package_blocked(&trove.name) {
            skipped.push((trove, AutoremoveSkipReason::Critical));
        } else {
            removable.push(trove);
        }
    }

    AutoremovePlan { removable, skipped }
}
```

Add a small formatter for skipped sections:

```rust
fn print_autoremove_skips(skipped: &[(Trove, AutoremoveSkipReason)]) {
    if skipped.is_empty() {
        return;
    }

    let adopted = skipped
        .iter()
        .filter(|(_, reason)| *reason == AutoremoveSkipReason::AdoptedNativeAuthority)
        .collect::<Vec<_>>();
    if !adopted.is_empty() {
        println!("Skipping adopted orphaned package(s); native package-manager authority is preserved:");
        for (trove, _) in adopted {
            println!("  {} {}", trove.name, trove.version);
        }
    }

    let blocked = skipped
        .iter()
        .filter(|(_, reason)| **reason != AutoremoveSkipReason::AdoptedNativeAuthority)
        .collect::<Vec<_>>();
    if !blocked.is_empty() {
        println!("Skipping blocked orphaned package(s):");
        for (trove, reason) in blocked {
            println!("  {} {} ({:?})", trove.name, trove.version, reason);
        }
    }
}
```

- [ ] **Step 3: Use the plan in `cmd_autoremove`**

After `find_orphans`, build:

```rust
let plan = plan_autoremove(orphans);
```

Print `plan.removable` as the "Found orphaned package(s)" set and call `print_autoremove_skips(&plan.skipped)`.

If `plan.removable` is empty:

```rust
println!("No Conary-owned orphaned packages can be autoremoved.");
print_autoremove_skips(&plan.skipped);
return Ok(());
```

Use `plan.removable` for dry-run and apply. After each fixed-point re-query, rebuild the plan so adopted/pinned/critical orphans remain skipped.

At the end, if `total_failed > 0`, return:

```rust
anyhow::bail!("Autoremove failed for {} package(s); see summary above", total_failed);
```

after printing the summary.

- [ ] **Step 4: Run autoremove tests**

Run:

```bash
cargo test -p conary autoremove -- --nocapture
```

Expected: all autoremove-focused tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/remove.rs apps/conary/tests/native_pm_daily_driver.rs
git commit -m "feat(remove): classify autoremove authority"
```

---

## Task 7: Add Daily-Driver CLI Proof

**Files:**

- Modify: `apps/conary/tests/native_pm_daily_driver.rs`
- Modify: `apps/conary/tests/common/mod.rs` if shared helpers reduce duplication.

- [ ] **Step 1: Add a list/info/files/path proof**

Append to `apps/conary/tests/native_pm_daily_driver.rs`:

```rust
#[test]
fn list_info_files_and_path_show_installed_package_identity() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let info = run_conary(&["list", "nginx", "--info", "--db-path", &db_path]);
    assert!(info.status.success(), "{}", output_text(&info));
    let info_stdout = String::from_utf8_lossy(&info.stdout);
    assert!(info_stdout.contains("Name        : nginx"), "{info_stdout}");
    assert!(info_stdout.contains("Authority   : conary-owned"), "{info_stdout}");
    assert!(info_stdout.contains("Pinned      : no"), "{info_stdout}");

    let files = run_conary(&["list", "nginx", "--files", "--db-path", &db_path]);
    assert!(files.status.success(), "{}", output_text(&files));
    let files_stdout = String::from_utf8_lossy(&files.stdout);
    assert!(files_stdout.contains("/usr/sbin/nginx"), "{files_stdout}");
    assert!(files_stdout.contains("/etc/nginx/nginx.conf"), "{files_stdout}");

    let path = run_conary(&["list", "--path", "/usr/sbin/nginx", "--db-path", &db_path]);
    assert!(path.status.success(), "{}", output_text(&path));
    let path_stdout = String::from_utf8_lossy(&path.stdout);
    assert!(path_stdout.contains("nginx 1.24.0 provides"), "{path_stdout}");
}
```

- [ ] **Step 2: Add pin blocks update/remove proof**

Append:

```rust
#[test]
fn pin_blocks_remove_and_unpin_allows_remove() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("conary.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    seed_orphan(&conn, root.path(), "pin-remove-demo", InstallSource::Repository);
    conn.execute(
        "UPDATE troves SET install_reason = 'explicit', selection_reason = 'Explicitly installed' WHERE name = 'pin-remove-demo'",
        [],
    )
    .unwrap();
    drop(conn);

    let pin = run_conary(&["pin", "pin-remove-demo", "--db-path", db_path.to_str().unwrap()]);
    assert!(pin.status.success(), "{}", output_text(&pin));

    let blocked = run_conary(&[
        "--allow-live-system-mutation",
        "remove",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert!(!blocked.status.success(), "{}", output_text(&blocked));
    assert!(output_text(&blocked).contains("is pinned"));

    let unpin = run_conary(&["unpin", "pin-remove-demo", "--db-path", db_path.to_str().unwrap()]);
    assert!(unpin.status.success(), "{}", output_text(&unpin));

    let removed = run_conary(&[
        "--allow-live-system-mutation",
        "remove",
        "pin-remove-demo",
        "--db-path",
        db_path.to_str().unwrap(),
        "--root",
        root.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
    ]);
    assert!(removed.status.success(), "{}", output_text(&removed));
}
```

- [ ] **Step 3: Add whatprovides/whatbreaks proof to the same file or keep it in `query.rs`**

If Task 5 already added strong `query.rs` proof, do not duplicate it. Add this checklist assertion to the file as a comment only:

```rust
// Provider and breakage parity are covered by apps/conary/tests/query.rs:
// - whatprovides_reports_installed_and_repository_providers
// - whatbreaks_reports_same_dependency_blocker_as_remove
```

- [ ] **Step 4: Run the daily-driver CLI proof**

Run:

```bash
cargo test -p conary --test native_pm_daily_driver -- --nocapture
```

Expected: all daily-driver CLI tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/tests/native_pm_daily_driver.rs
git commit -m "test(cli): prove daily driver package commands"
```

---

## Task 8: Documentation, Audit Ledger, And Final Verification

**Files:**

- Modify: `README.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Update user-facing command examples**

In `README.md` and `docs/conaryopedia-v2.md`, add or update examples for:

```bash
conary list nginx --info
conary list nginx --files
conary list --path /usr/sbin/nginx
conary pin nginx
conary unpin nginx
conary list --pinned
conary autoremove --dry-run
conary query whatprovides 'soname(libssl.so.3)'
conary query whatbreaks openssl
```

Include one sentence that ambiguous installed variants require `--version` and/or `--arch`.

- [ ] **Step 2: Update integration testing docs**

In `docs/INTEGRATION-TESTING.md`, add the focused CLI proof:

```markdown
- `apps/conary/tests/native_pm_daily_driver.rs` proves Tier 1 list/info/files/path,
  pin/unpin, autoremove, and query diagnostics for Conary-owned packages before
  the broader Slice D distro matrix.
```

- [ ] **Step 3: Update the parity spec with the Slice C plan link**

In `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`, replace the single Slice A plan line with:

```markdown
Slice A plan: `docs/superpowers/plans/2026-05-14-native-package-manager-parity-slice-a-plan.md`.
Slice B plan: `docs/superpowers/plans/2026-05-15-native-package-manager-parity-slice-b-plan.md`.
Slice C plan: `docs/superpowers/plans/2026-05-15-native-package-manager-parity-slice-c-plan.md`.
```

- [ ] **Step 4: Update audit inventory and ledger**

Add this row to `docs/superpowers/documentation-accuracy-audit-inventory.tsv` after the Slice B plan row:

```tsv
docs/superpowers/plans/2026-05-15-native-package-manager-parity-slice-c-plan.md	planning	maintainer
```

Add this row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv` after the Slice B plan row:

```tsv
docs/superpowers/plans/2026-05-15-native-package-manager-parity-slice-c-plan.md	docs/superpowers/plans/2026-05-15-native-package-manager-parity-slice-c-plan.md	planning	maintainer	native-package-manager-parity; implementation-plan; daily-driver-commands; public-preview	docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md; apps/conary/src/commands/query/package.rs; apps/conary/src/commands/query/dependency.rs; apps/conary/src/commands/remove.rs; apps/conary/src/commands/update.rs	verified	corrected	Added the active Slice C implementation plan for Tier 1 daily-driver command parity across selectors, pinning, autoremove, history, provider, and breakage diagnostics.
```

Update `docs/superpowers/documentation-accuracy-audit-summary.md` counts:

```markdown
- Total tracked doc-like files audited: 79
- `corrected`: 33
```

Keep the other counts unchanged unless additional docs are corrected during implementation.

- [ ] **Step 5: Run final verification**

Run:

```bash
cargo fmt --check
git diff --check
cargo test -p conary package_target -- --nocapture
cargo test -p conary --test query -- --nocapture
cargo test -p conary --test native_pm_live_root -- --nocapture
cargo test -p conary --test native_pm_daily_driver -- --nocapture
cargo run -p conary -- list --help
cargo run -p conary -- update --help
cargo run -p conary -- query whatprovides --help
cargo run -p conary-test -- list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
cargo clippy --workspace --all-targets -- -D warnings
```

Expected:

- All tests pass.
- `list --help`, `update --help`, and `query whatprovides --help` show the intended command shapes.
- `conary-test list` still parses all manifests.
- The documentation ledger check passes with zero pending rows.
- Clippy exits with no warnings.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/conaryopedia-v2.md docs/INTEGRATION-TESTING.md docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: record daily driver parity proof"
```

---

## Plan Self-Review

- The plan keeps top-level `whatprovides` and `whatbreaks` aliases out of scope, matching the design spec's product decision boundary.
- The first implementation task creates a shared selector before wiring commands, so later command changes do not duplicate ambiguity logic.
- `autoremove` is explicitly constrained to Conary-owned removable candidates and truthful skipped-package reporting.
- Slice D distro matrix work remains separate; this slice adds focused CLI proof only.
- The final verification set includes formatting, focused tests, help text, conary-test inventory parsing, audit-ledger integrity, and workspace clippy.
