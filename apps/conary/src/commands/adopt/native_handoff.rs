// apps/conary/src/commands/adopt/native_handoff.rs

//! Return a selected Conary generation to native package-manager authority.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::db;
use conary_core::db::models::{Changeset, ChangesetStatus, Trove};
use conary_core::generation::mount::current_generation;
use conary_core::packages::SystemPackageManager;
use conary_core::runtime_root::ConaryRuntimeRoot;
use serde::{Deserialize, Serialize};

use super::hooks::remove_detected_sync_hooks;
use crate::commands::create_state_snapshot;

const RECORD_VERSION: u32 = 1;
const RECORD_FILE_NAME: &str = "native-authority-handoff.json";
const CURRENT_BACKUP_FILE_NAME: &str = "current.native-handoff-backup";

#[derive(Debug, Clone)]
pub struct NativeHandoffOptions {
    pub dry_run: bool,
    pub yes: bool,
    pub recover: bool,
    pub keep_hooks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeHandoffOutcome {
    DryRun,
    Applied,
    Recovered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeHandoffSummary {
    pub outcome: NativeHandoffOutcome,
    pub selected_generation: Option<i64>,
    pub unadopted: Vec<String>,
    pub skipped_conary_owned: Vec<String>,
    pub changeset_id: Option<i64>,
    pub current_link_cleared: bool,
    pub tracking_removed: bool,
    pub hooks_removed: bool,
    pub record_path: PathBuf,
}

#[derive(Debug, Clone)]
struct NativeHandoffEnvironment {
    package_manager: SystemPackageManager,
    fail_after_current_cleared: bool,
}

impl NativeHandoffEnvironment {
    fn detect() -> Self {
        let fail_after_current_cleared = std::env::var("CONARY_TEST_NATIVE_HANDOFF_FAIL_AFTER")
            .map(|value| value == "current-cleared")
            .unwrap_or(false);

        Self {
            package_manager: SystemPackageManager::detect(),
            fail_after_current_cleared,
        }
    }
}

#[derive(Debug)]
struct NativeHandoffPlan {
    selected_generation: i64,
    adopted: Vec<Trove>,
    skipped_conary_owned: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NativeHandoffRecord {
    version: u32,
    selected_generation: i64,
    packages: Vec<NativeHandoffPackageRecord>,
    skipped_conary_owned: Vec<String>,
    current_backup_path: PathBuf,
    current_link_cleared: bool,
    tracking_removed: bool,
    hook_stage_complete: bool,
    hooks_removed: bool,
    keep_hooks: bool,
    changeset_id: Option<i64>,
    started_at: String,
    updated_at: String,
    completed_at: Option<String>,
    recovery_instructions: String,
}

impl NativeHandoffRecord {
    fn from_plan(
        runtime_root: &ConaryRuntimeRoot,
        plan: NativeHandoffPlan,
        keep_hooks: bool,
    ) -> Self {
        let now = timestamp();
        Self {
            version: RECORD_VERSION,
            selected_generation: plan.selected_generation,
            packages: plan
                .adopted
                .into_iter()
                .filter_map(|trove| NativeHandoffPackageRecord::from_trove(&trove))
                .collect(),
            skipped_conary_owned: plan.skipped_conary_owned,
            current_backup_path: runtime_root.root().join(CURRENT_BACKUP_FILE_NAME),
            current_link_cleared: false,
            tracking_removed: false,
            hook_stage_complete: false,
            hooks_removed: false,
            keep_hooks,
            changeset_id: None,
            started_at: now.clone(),
            updated_at: now,
            completed_at: None,
            recovery_instructions:
                "If interrupted, rerun conary system native-handoff --recover --yes".to_string(),
        }
    }

    fn is_complete(&self) -> bool {
        self.completed_at.is_some()
    }

    fn mark_updated(&mut self) {
        self.updated_at = timestamp();
    }

    fn package_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .packages
            .iter()
            .map(|package| package.name.clone())
            .collect();
        names.sort();
        names
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NativeHandoffPackageRecord {
    id: i64,
    name: String,
    version: String,
    install_source: String,
}

impl NativeHandoffPackageRecord {
    fn from_trove(trove: &Trove) -> Option<Self> {
        Some(Self {
            id: trove.id?,
            name: trove.name.clone(),
            version: trove.version.clone(),
            install_source: trove.install_source.as_str().to_string(),
        })
    }
}

pub async fn cmd_native_handoff(
    options: NativeHandoffOptions,
    db_path: &str,
) -> Result<NativeHandoffSummary> {
    cmd_native_handoff_with_environment(
        options,
        db_path,
        NativeHandoffEnvironment::detect(),
        remove_detected_sync_hooks,
    )
}

fn cmd_native_handoff_with_environment<F>(
    options: NativeHandoffOptions,
    db_path: &str,
    environment: NativeHandoffEnvironment,
    hook_remover: F,
) -> Result<NativeHandoffSummary>
where
    F: FnOnce() -> Result<bool>,
{
    if options.dry_run && options.recover {
        bail!("conary system native-handoff --recover cannot be combined with --dry-run");
    }

    ensure_native_package_manager_available(environment.package_manager)?;

    let runtime_root = ConaryRuntimeRoot::from_db_path(Path::new(db_path));
    let record_path = handoff_record_path(&runtime_root);

    if options.recover {
        if !options.yes {
            bail!("conary system native-handoff --recover requires --yes before replaying state");
        }
        let record = load_incomplete_record(&record_path)?;
        return apply_recorded_handoff(
            record,
            &record_path,
            &runtime_root,
            db_path,
            environment,
            hook_remover,
            NativeHandoffOutcome::Recovered,
        );
    }

    if let Some(record) = load_record_if_exists(&record_path)?
        && !record.is_complete()
    {
        bail!(
            "Native authority handoff is already in progress. Rerun conary system native-handoff --recover --yes."
        );
    }

    let conn = db::open(db_path)?;
    let plan = build_handoff_plan(&conn, &runtime_root)?;
    let adopted_names = sorted_trove_names(&plan.adopted);
    let skipped_conary_owned = plan.skipped_conary_owned.clone();

    if options.dry_run {
        print_dry_run_summary(
            plan.selected_generation,
            &adopted_names,
            &skipped_conary_owned,
            &record_path,
            environment.package_manager,
        );
        return Ok(NativeHandoffSummary {
            outcome: NativeHandoffOutcome::DryRun,
            selected_generation: Some(plan.selected_generation),
            unadopted: adopted_names,
            skipped_conary_owned,
            changeset_id: None,
            current_link_cleared: false,
            tracking_removed: false,
            hooks_removed: false,
            record_path,
        });
    }

    if !options.yes {
        bail!(
            "conary system native-handoff requires --yes before clearing the selected generation and removing adopted tracking. Run with --dry-run first to inspect the plan."
        );
    }

    let record = NativeHandoffRecord::from_plan(&runtime_root, plan, options.keep_hooks);
    save_record(&record_path, &record)?;
    apply_recorded_handoff(
        record,
        &record_path,
        &runtime_root,
        db_path,
        environment,
        hook_remover,
        NativeHandoffOutcome::Applied,
    )
}

fn ensure_native_package_manager_available(pkg_mgr: SystemPackageManager) -> Result<()> {
    if pkg_mgr.is_available() {
        return Ok(());
    }

    bail!(
        "No supported native package manager found. Native authority handoff requires RPM, dpkg, or pacman so Conary can leave native authority intact."
    )
}

fn build_handoff_plan(
    conn: &rusqlite::Connection,
    runtime_root: &ConaryRuntimeRoot,
) -> Result<NativeHandoffPlan> {
    let selected_generation = current_generation(runtime_root.root())?.ok_or_else(|| {
        anyhow::anyhow!(
            "No Conary generation is selected. Use conary system unadopt --all for pre-generation native unadoption."
        )
    })?;

    let mut adopted = Vec::new();
    let mut skipped_conary_owned = Vec::new();

    for trove in Trove::list_all(conn)? {
        if trove.install_source.is_adopted() {
            adopted.push(trove);
        } else if trove.install_source.is_conary_owned() {
            skipped_conary_owned.push(trove.name);
        }
    }

    if adopted.is_empty() {
        bail!(
            "No adopted native packages are tracked by Conary. Nothing can be handed back to native authority."
        );
    }

    adopted.sort_by(|left, right| left.name.cmp(&right.name));
    skipped_conary_owned.sort();
    Ok(NativeHandoffPlan {
        selected_generation,
        adopted,
        skipped_conary_owned,
    })
}

fn apply_recorded_handoff<F>(
    mut record: NativeHandoffRecord,
    record_path: &Path,
    runtime_root: &ConaryRuntimeRoot,
    db_path: &str,
    environment: NativeHandoffEnvironment,
    hook_remover: F,
    outcome: NativeHandoffOutcome,
) -> Result<NativeHandoffSummary>
where
    F: FnOnce() -> Result<bool>,
{
    if !record.current_link_cleared {
        clear_current_link(runtime_root, &record)?;
        record.current_link_cleared = true;
        record.mark_updated();
        save_record(record_path, &record)?;
    }

    if environment.fail_after_current_cleared {
        bail!("Simulated interruption after current-cleared stage");
    }

    if !record.tracking_removed {
        record.changeset_id = remove_recorded_adopted_tracking(db_path, &record)?;
        record.tracking_removed = true;
        record.mark_updated();
        save_record(record_path, &record)?;
    }

    if !record.hook_stage_complete {
        record.hooks_removed = if record.keep_hooks {
            false
        } else {
            hook_remover()?
        };
        record.hook_stage_complete = true;
        record.mark_updated();
        save_record(record_path, &record)?;
    }

    record.completed_at = Some(timestamp());
    record.mark_updated();
    save_record(record_path, &record)?;

    let summary = summary_from_record(&record, record_path, outcome);
    print_apply_summary(&summary);
    Ok(summary)
}

fn clear_current_link(
    runtime_root: &ConaryRuntimeRoot,
    record: &NativeHandoffRecord,
) -> Result<()> {
    let current_link = runtime_root.current_link();
    match std::fs::symlink_metadata(&current_link) {
        Ok(_) => {
            if std::fs::symlink_metadata(&record.current_backup_path).is_ok() {
                bail!(
                    "Native handoff backup path already exists at {}. Remove or recover it before retrying.",
                    record.current_backup_path.display()
                );
            }
            std::fs::rename(&current_link, &record.current_backup_path).with_context(|| {
                format!(
                    "failed to move selected generation link {} to {}",
                    current_link.display(),
                    record.current_backup_path.display()
                )
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::symlink_metadata(&record.current_backup_path).is_err() {
                bail!(
                    "Selected generation link {} is missing and no native handoff backup exists; cannot prove current-link clearing state.",
                    current_link.display()
                );
            }
        }
        Err(error) => return Err(error).context("failed to inspect selected generation link"),
    }

    Ok(())
}

fn remove_recorded_adopted_tracking(
    db_path: &str,
    record: &NativeHandoffRecord,
) -> Result<Option<i64>> {
    let mut conn = db::open(db_path)?;
    let mut target_ids = Vec::new();
    let mut target_names = Vec::new();

    for package in &record.packages {
        let Some(trove) = Trove::find_by_id(&conn, package.id)? else {
            continue;
        };
        if trove.install_source.is_adopted() {
            target_ids.push(package.id);
            target_names.push(trove.name);
        }
    }

    target_names.sort();
    if target_ids.is_empty() {
        return Ok(record.changeset_id);
    }

    let changeset_id = db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new(changeset_description(&target_names));
        let changeset_id = changeset.insert(tx)?;
        for trove_id in &target_ids {
            Trove::delete(tx, *trove_id)?;
        }
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    create_state_snapshot(&conn, changeset_id, "Native authority handoff")?;
    Ok(Some(changeset_id))
}

fn summary_from_record(
    record: &NativeHandoffRecord,
    record_path: &Path,
    outcome: NativeHandoffOutcome,
) -> NativeHandoffSummary {
    let mut skipped_conary_owned = record.skipped_conary_owned.clone();
    skipped_conary_owned.sort();
    NativeHandoffSummary {
        outcome,
        selected_generation: Some(record.selected_generation),
        unadopted: record.package_names(),
        skipped_conary_owned,
        changeset_id: record.changeset_id,
        current_link_cleared: record.current_link_cleared,
        tracking_removed: record.tracking_removed,
        hooks_removed: record.hooks_removed,
        record_path: record_path.to_path_buf(),
    }
}

fn sorted_trove_names(troves: &[Trove]) -> Vec<String> {
    let mut names: Vec<String> = troves.iter().map(|trove| trove.name.clone()).collect();
    names.sort();
    names
}

fn changeset_description(names: &[String]) -> String {
    if names.len() == 1 {
        return format!("Native authority handoff for {}", names[0]);
    }
    format!(
        "Native authority handoff for {} adopted packages",
        names.len()
    )
}

fn handoff_record_path(runtime_root: &ConaryRuntimeRoot) -> PathBuf {
    runtime_root.root().join(RECORD_FILE_NAME)
}

fn save_record(path: &Path, record: &NativeHandoffRecord) -> Result<()> {
    crate::commands::operation_records::write_json_record(path, record)
}

fn load_record_if_exists(path: &Path) -> Result<Option<NativeHandoffRecord>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(crate::commands::operation_records::load_json_record(
        path,
    )?))
}

fn load_incomplete_record(path: &Path) -> Result<NativeHandoffRecord> {
    let record = load_record_if_exists(path)?.ok_or_else(|| {
        anyhow::anyhow!(
            "No native authority handoff record exists. Start with conary system native-handoff --dry-run."
        )
    })?;
    if record.is_complete() {
        bail!("Native authority handoff record is already completed.");
    }
    Ok(record)
}

fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn print_dry_run_summary(
    selected_generation: i64,
    unadopted: &[String],
    skipped_conary_owned: &[String],
    record_path: &Path,
    pkg_mgr: SystemPackageManager,
) {
    println!("Native authority handoff dry run");
    println!("Selected Conary generation: {selected_generation}");
    println!(
        "Detected native package manager: {}",
        pkg_mgr.display_name()
    );
    println!("Operation record: {}", record_path.display());
    println!("Would clear /conary/current before removing adopted tracking.");
    println!("Would unadopt {} adopted package(s):", unadopted.len());
    for name in unadopted {
        println!("  {name}");
    }
    if !skipped_conary_owned.is_empty() {
        println!(
            "Would leave {} Conary-owned package(s) tracked: {}",
            skipped_conary_owned.len(),
            skipped_conary_owned.join(", ")
        );
    }
    println!("Native package files and native package-manager databases would not be modified.");
}

fn print_apply_summary(summary: &NativeHandoffSummary) {
    let verb = match summary.outcome {
        NativeHandoffOutcome::DryRun => "Prepared",
        NativeHandoffOutcome::Applied => "Completed",
        NativeHandoffOutcome::Recovered => "Recovered",
    };
    println!(
        "{verb} native authority handoff for Conary generation {}.",
        summary.selected_generation.unwrap_or_default()
    );
    println!("Unadopted {} adopted package(s):", summary.unadopted.len());
    for name in &summary.unadopted {
        println!("  {name}");
    }
    if !summary.skipped_conary_owned.is_empty() {
        println!(
            "Left {} Conary-owned package(s) tracked: {}",
            summary.skipped_conary_owned.len(),
            summary.skipped_conary_owned.join(", ")
        );
    }
    println!("Native package files and native package-manager databases were not modified.");
    println!("Recovery record: {}", summary.record_path.display());
}

#[cfg(test)]
mod tests {
    use std::fs;

    use conary_core::db;
    use conary_core::db::models::{Changeset, ChangesetStatus, InstallSource, Trove, TroveType};
    use conary_core::generation::mount::current_generation;
    use conary_core::packages::SystemPackageManager;
    use conary_core::runtime_root::ConaryRuntimeRoot;

    use super::*;
    use crate::commands::test_helpers::create_test_db;

    fn handoff_options(dry_run: bool) -> NativeHandoffOptions {
        NativeHandoffOptions {
            dry_run,
            yes: !dry_run,
            recover: false,
            keep_hooks: true,
        }
    }

    fn supported_env() -> NativeHandoffEnvironment {
        NativeHandoffEnvironment {
            package_manager: SystemPackageManager::Rpm,
            fail_after_current_cleared: false,
        }
    }

    fn unsupported_env() -> NativeHandoffEnvironment {
        NativeHandoffEnvironment {
            package_manager: SystemPackageManager::Unknown,
            fail_after_current_cleared: false,
        }
    }

    fn seed_package(db_path: &str, name: &str, source: InstallSource) -> i64 {
        let mut conn = db::open(db_path).unwrap();
        db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new(format!("Seed {name}"));
            let changeset_id = changeset.insert(tx)?;
            let mut trove = Trove::new_with_source(
                name.to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                source,
            );
            trove.installed_by_changeset_id = Some(changeset_id);
            let trove_id = trove.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(trove_id)
        })
        .unwrap()
    }

    fn package_names(db_path: &str, name: &str) -> Vec<String> {
        let conn = db::open(db_path).unwrap();
        Trove::find_by_name(&conn, name)
            .unwrap()
            .into_iter()
            .map(|trove| trove.name)
            .collect()
    }

    fn state_snapshot_count(db_path: &str) -> i64 {
        let conn = db::open(db_path).unwrap();
        conn.query_row("SELECT COUNT(*) FROM system_states", [], |row| row.get(0))
            .unwrap()
    }

    #[cfg(unix)]
    fn select_generation(db_path: &str, generation: u32) {
        let runtime_root = ConaryRuntimeRoot::from_db_path(std::path::Path::new(db_path));
        let generation_path = runtime_root.generation_path(i64::from(generation));
        fs::create_dir_all(&generation_path).unwrap();
        std::os::unix::fs::symlink(&generation_path, runtime_root.current_link()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_handoff_dry_run_reports_selected_generation_without_mutation() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedFull);
        seed_package(&db_path, "bash", InstallSource::Repository);
        select_generation(&db_path, 7);

        let summary = cmd_native_handoff_with_environment(
            handoff_options(true),
            &db_path,
            supported_env(),
            || Ok(false),
        )
        .unwrap();

        let runtime_root = ConaryRuntimeRoot::from_db_path(std::path::Path::new(&db_path));
        assert_eq!(summary.outcome, NativeHandoffOutcome::DryRun);
        assert_eq!(summary.selected_generation, Some(7));
        assert_eq!(summary.unadopted, vec!["curl"]);
        assert_eq!(summary.skipped_conary_owned, vec!["bash"]);
        assert_eq!(summary.changeset_id, None);
        assert_eq!(package_names(&db_path, "curl"), vec!["curl"]);
        assert_eq!(
            current_generation(runtime_root.root()).unwrap(),
            Some(7),
            "dry-run must leave the selected generation in place"
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_handoff_apply_clears_current_and_removes_adopted_tracking() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedTrack);
        seed_package(&db_path, "bash", InstallSource::File);
        select_generation(&db_path, 9);

        let summary = cmd_native_handoff_with_environment(
            handoff_options(false),
            &db_path,
            supported_env(),
            || Ok(false),
        )
        .unwrap();

        let runtime_root = ConaryRuntimeRoot::from_db_path(std::path::Path::new(&db_path));
        assert_eq!(summary.outcome, NativeHandoffOutcome::Applied);
        assert_eq!(summary.selected_generation, Some(9));
        assert_eq!(summary.unadopted, vec!["curl"]);
        assert_eq!(summary.skipped_conary_owned, vec!["bash"]);
        assert!(summary.current_link_cleared);
        assert!(summary.tracking_removed);
        assert!(summary.changeset_id.is_some());
        assert_eq!(current_generation(runtime_root.root()).unwrap(), None);
        assert!(
            runtime_root
                .root()
                .join("current.native-handoff-backup")
                .exists()
        );
        assert!(package_names(&db_path, "curl").is_empty());
        assert_eq!(package_names(&db_path, "bash"), vec!["bash"]);
        assert_eq!(state_snapshot_count(&db_path), 1);
    }

    #[cfg(unix)]
    #[test]
    fn native_handoff_apply_requires_explicit_yes_before_mutation() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedTrack);
        select_generation(&db_path, 4);
        let mut options = handoff_options(false);
        options.yes = false;

        let err =
            cmd_native_handoff_with_environment(options, &db_path, supported_env(), || Ok(false))
                .unwrap_err();

        let runtime_root = ConaryRuntimeRoot::from_db_path(std::path::Path::new(&db_path));
        assert!(err.to_string().contains("--yes"));
        assert_eq!(package_names(&db_path, "curl"), vec!["curl"]);
        assert_eq!(current_generation(runtime_root.root()).unwrap(), Some(4));
    }

    #[test]
    fn native_handoff_refuses_without_supported_native_package_manager() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedTrack);

        let err = cmd_native_handoff_with_environment(
            handoff_options(true),
            &db_path,
            unsupported_env(),
            || Ok(false),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("No supported native package manager")
        );
        assert_eq!(package_names(&db_path, "curl"), vec!["curl"]);
    }

    #[cfg(unix)]
    #[test]
    fn native_handoff_routes_unselected_hosts_to_unadopt_all() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedTrack);

        let err = cmd_native_handoff_with_environment(
            handoff_options(false),
            &db_path,
            supported_env(),
            || Ok(false),
        )
        .unwrap_err();

        assert!(err.to_string().contains("conary system unadopt --all"));
        assert_eq!(package_names(&db_path, "curl"), vec!["curl"]);
    }

    #[cfg(unix)]
    #[test]
    fn native_handoff_recover_resumes_after_current_link_was_cleared() {
        let (_temp_dir, db_path) = create_test_db();
        seed_package(&db_path, "curl", InstallSource::AdoptedFull);
        select_generation(&db_path, 11);
        let mut interrupted_env = supported_env();
        interrupted_env.fail_after_current_cleared = true;

        let err = cmd_native_handoff_with_environment(
            handoff_options(false),
            &db_path,
            interrupted_env,
            || Ok(false),
        )
        .unwrap_err();

        let runtime_root = ConaryRuntimeRoot::from_db_path(std::path::Path::new(&db_path));
        assert!(err.to_string().contains("current-cleared"));
        assert_eq!(current_generation(runtime_root.root()).unwrap(), None);
        assert_eq!(package_names(&db_path, "curl"), vec!["curl"]);

        let mut recover_options = handoff_options(false);
        recover_options.recover = true;
        let summary =
            cmd_native_handoff_with_environment(recover_options, &db_path, supported_env(), || {
                Ok(false)
            })
            .unwrap();

        assert_eq!(summary.outcome, NativeHandoffOutcome::Recovered);
        assert_eq!(summary.selected_generation, Some(11));
        assert!(summary.current_link_cleared);
        assert!(summary.tracking_removed);
        assert!(package_names(&db_path, "curl").is_empty());
    }
}
