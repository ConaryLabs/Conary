// apps/conary/src/commands/remove/autoremove.rs

use std::collections::HashSet;

use anyhow::Result;
use conary_core::db::models::Trove;
use tracing::info;

use super::legacy_replay::load_installed_legacy_remove_plan;
use super::types::RemoveScriptletOptions;
use crate::commands::{LegacyReplayOptions, SandboxMode, open_db};

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

/// Remove orphaned packages (installed as dependencies but no longer needed)
///
/// Finds packages that were installed as dependencies of other packages,
/// but are no longer required by any installed package.
pub async fn cmd_autoremove(
    db_path: &str,
    root: &str,
    dry_run: bool,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    legacy_replay: LegacyReplayOptions,
) -> Result<()> {
    info!("Finding orphaned packages...");

    let conn = open_db(db_path)?;

    let orphans = conary_core::db::models::Trove::find_orphans(&conn)?;
    if orphans.is_empty() {
        println!("No orphaned packages found.");
        return Ok(());
    }

    let plan = plan_autoremove(orphans);
    if plan.removable.is_empty() {
        println!("No Conary-owned orphaned packages can be autoremoved.");
        print_autoremove_skips(&plan.skipped);
        return Ok(());
    }
    print_autoremove_candidates("Found", &plan.removable);
    print_autoremove_skips(&plan.skipped);

    if dry_run {
        println!("\nDry run - no packages will be removed.");
        println!("Run without --dry-run to remove these packages.");
        return Ok(());
    }

    // Fixed-point iteration: removing orphans may expose new orphans (transitive chains).
    // Re-query after each round until no more orphans are found.
    const MAX_ITERATIONS: usize = 100;
    let mut total_removed = 0;
    let mut total_failed = 0;
    let mut current_plan = plan;
    let mut failed_orphans = HashSet::new();

    for iteration in 0..MAX_ITERATIONS {
        if iteration > 0 {
            // Re-query orphans after previous round of removals
            let conn = open_db(db_path)?;
            let current_orphans = conary_core::db::models::Trove::find_orphans(&conn)?;
            if current_orphans.is_empty() {
                break;
            }
            current_plan = plan_autoremove(current_orphans);
            current_plan
                .removable
                .retain(|trove| !failed_orphans.contains(&autoremove_identity(trove)));
            if current_plan.removable.is_empty() {
                println!("\nNo additional Conary-owned orphaned packages can be autoremoved.");
                print_autoremove_skips(&current_plan.skipped);
                break;
            }
            print_autoremove_candidates("Found additional", &current_plan.removable);
            print_autoremove_skips(&current_plan.skipped);
        } else {
            println!(
                "\nRemoving {} orphaned package(s)...",
                current_plan.removable.len()
            );
        }

        let conn = open_db(db_path)?;
        preflight_autoremove_round(
            &conn,
            &current_plan.removable,
            RemoveScriptletOptions::new(no_scripts, sandbox_mode, legacy_replay),
        )?;

        let mut round_removed = 0;
        for trove in &current_plan.removable {
            println!("\nRemoving {} {}...", trove.name, trove.version);
            match super::cmd_remove(
                &trove.name,
                db_path,
                root,
                Some(trove.version.clone()),
                trove.architecture.clone(),
                no_scripts,
                sandbox_mode,
                false,
                legacy_replay,
            )
            .await
            {
                Ok(()) => {
                    round_removed += 1;
                }
                Err(e) => {
                    eprintln!("  Failed to remove {}: {}", trove.name, e);
                    failed_orphans.insert(autoremove_identity(trove));
                    total_failed += 1;
                }
            }
        }

        total_removed += round_removed;

        // If nothing was removed this round, no point continuing
        if round_removed == 0 {
            break;
        }
    }

    println!("\nAutoremove complete:");
    println!("  Removed: {} package(s)", total_removed);
    if total_failed > 0 {
        println!("  Failed: {} package(s)", total_failed);
        anyhow::bail!(
            "Autoremove failed for {} package(s); see summary above",
            total_failed
        );
    }

    Ok(())
}

fn preflight_autoremove_round(
    conn: &rusqlite::Connection,
    troves: &[Trove],
    scriptlet_options: RemoveScriptletOptions,
) -> Result<()> {
    for trove in troves {
        let Some(trove_id) = trove.id else {
            anyhow::bail!(
                "autoremove legacy replay preflight failed for {} {}: trove has no id",
                trove.name,
                trove.version
            );
        };
        if let Err(error) = load_installed_legacy_remove_plan(conn, trove_id, scriptlet_options) {
            anyhow::bail!(
                "autoremove legacy replay preflight failed for {} {}: {error}",
                trove.name,
                trove.version
            );
        }
    }

    Ok(())
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

fn print_autoremove_candidates(prefix: &str, troves: &[Trove]) {
    println!("{prefix} {} orphaned package(s):", troves.len());
    for trove in troves {
        print_autoremove_trove(trove);
    }
}

fn print_autoremove_skips(skipped: &[(Trove, AutoremoveSkipReason)]) {
    if skipped.is_empty() {
        return;
    }

    let adopted = skipped
        .iter()
        .filter(|(_, reason)| *reason == AutoremoveSkipReason::AdoptedNativeAuthority)
        .collect::<Vec<_>>();
    if !adopted.is_empty() {
        println!(
            "Skipping adopted orphaned package(s); native package-manager authority is preserved:"
        );
        for (trove, _) in adopted {
            print_autoremove_trove(trove);
        }
    }

    let blocked = skipped
        .iter()
        .filter(|(_, reason)| *reason != AutoremoveSkipReason::AdoptedNativeAuthority)
        .collect::<Vec<_>>();
    if !blocked.is_empty() {
        println!("Skipping blocked orphaned package(s):");
        for (trove, reason) in blocked {
            print!("  {} {}", trove.name, trove.version);
            if let Some(arch) = &trove.architecture {
                print!(" [{}]", arch);
            }
            println!(" ({:?})", reason);
        }
    }
}

fn print_autoremove_trove(trove: &Trove) {
    print!("  {} {}", trove.name, trove.version);
    if let Some(arch) = &trove.architecture {
        print!(" [{}]", arch);
    }
    println!();
}

fn autoremove_identity(trove: &Trove) -> (String, String, Option<String>) {
    (
        trove.name.clone(),
        trove.version.clone(),
        trove.architecture.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
        LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy,
        PublicationStatus, ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility,
        TransactionOrder, VersionScheme,
    };
    use conary_core::db::models::{InstallSource, InstalledLegacyScriptletBundle, TroveType};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    #[test]
    fn autoremove_plan_classifies_authority_and_safety_skips() {
        let owned = Trove::new_with_source(
            "owned-orphan".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        let adopted = Trove::new_with_source(
            "adopted-orphan".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
        );
        let mut pinned = Trove::new_with_source(
            "pinned-orphan".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        pinned.pinned = true;
        let critical = Trove::new_with_source(
            "bash".to_string(),
            "5.2.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );

        let plan = plan_autoremove(vec![owned, adopted, pinned, critical]);

        assert_eq!(plan.removable.len(), 1);
        assert_eq!(plan.removable[0].name, "owned-orphan");
        assert_eq!(
            plan.skipped
                .iter()
                .map(|(trove, reason)| (trove.name.as_str(), reason))
                .collect::<Vec<_>>(),
            vec![
                (
                    "adopted-orphan",
                    &AutoremoveSkipReason::AdoptedNativeAuthority
                ),
                ("pinned-orphan", &AutoremoveSkipReason::Pinned),
                ("bash", &AutoremoveSkipReason::Critical),
            ]
        );
    }

    #[tokio::test]
    async fn autoremove_refuses_legacy_candidate_before_removing_any_package() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_clear_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        conary_core::db::models::DistroPin::set(&conn, "fedora-44", "strict").unwrap();
        seed_dependency_trove(&conn, "aa-plain-orphan");
        let legacy_trove_id = seed_dependency_trove(&conn, "zz-legacy-orphan");
        seed_installed_legacy_bundle(&conn, legacy_trove_id, "zz-legacy-orphan");
        drop(conn);

        let err = cmd_autoremove(
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            false,
            false,
            SandboxMode::None,
            LegacyReplayOptions::default(),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("LegacyReplayFeatureDisabled"), "{err}");
        let conn = conary_core::db::open(&db_path).unwrap();
        assert_eq!(
            Trove::find_by_name(&conn, "aa-plain-orphan").unwrap().len(),
            1,
            "autoremove must not remove earlier candidates before a later legacy refusal"
        );
        assert_eq!(
            Trove::find_by_name(&conn, "zz-legacy-orphan")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(table_count(&conn, "changesets"), 0);
        assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 1);
    }

    #[tokio::test]
    async fn autoremove_with_legacy_replay_flag_removes_all_candidates() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_clear_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        conary_core::db::models::DistroPin::set(&conn, "fedora-44", "strict").unwrap();
        seed_dependency_trove(&conn, "aa-plain-orphan");
        let legacy_trove_id = seed_dependency_trove(&conn, "zz-legacy-orphan");
        seed_installed_legacy_bundle(&conn, legacy_trove_id, "zz-legacy-orphan");
        drop(conn);

        cmd_autoremove(
            db_path.to_string_lossy().as_ref(),
            root.to_string_lossy().as_ref(),
            false,
            false,
            SandboxMode::None,
            LegacyReplayOptions {
                allow_legacy_replay: true,
                allow_foreign_legacy_replay: false,
            },
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        assert_eq!(table_count(&conn, "troves"), 0);
        assert_eq!(table_count(&conn, "installed_legacy_scriptlet_bundles"), 0);
        assert_eq!(table_count(&conn, "changesets"), 2);
        let metadata = changeset_metadata_by_description(&conn, "Remove zz-legacy-orphan-1.0.0");
        let planned_entries = metadata["legacy_scriptlet_replay"]["planned_entries"]
            .as_array()
            .expect("planned entries array");
        assert_eq!(planned_entries.len(), 1);
        assert_eq!(planned_entries[0]["entry_id"], "rpm:%postun");
        assert_eq!(planned_entries[0]["phase"], "post-remove");
        assert!(planned_entries[0].get("outcome").is_some());
    }

    fn seed_dependency_trove(conn: &rusqlite::Connection, name: &str) -> i64 {
        let mut trove = Trove::new_as_dependency(
            name.to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            "fixture-root",
        );
        trove.architecture = Some("x86_64".to_string());
        trove.insert(conn).unwrap()
    }

    fn seed_installed_legacy_bundle(conn: &rusqlite::Connection, trove_id: i64, package: &str) {
        let bundle = legacy_post_remove_bundle(package);
        let target_id = conary_core::repository::distro::source_target_from_bundle(&bundle).to_id();
        let mut installed = InstalledLegacyScriptletBundle::new(
            trove_id,
            None,
            target_id,
            "strict".to_string(),
            false,
            &bundle,
        )
        .unwrap();
        installed.insert_or_replace(conn).unwrap();
    }

    fn legacy_post_remove_bundle(package: &str) -> LegacyScriptletBundle {
        let entry = legacy_post_remove_entry();
        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: package.to_string(),
            source_version: "1.0.0-1.fc44".to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "test".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "goal6-autoremove-test".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                format!("{package}-legacy-remove-evidence").as_bytes(),
            )),
            target_compatibility: TargetCompatibility::SourceNative,
            allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::LocalOnly,
            publication_status: PublicationStatus::Public,
            scriptlet_fidelity: ScriptletFidelity::LegacyReplay,
            decision_counts: DecisionCounts {
                replaced: 0,
                legacy: 1,
                blocked: 0,
                review: 0,
                extra: BTreeMap::new(),
            },
            unsupported_class_counts: BTreeMap::new(),
            entries: vec![entry],
            extra: BTreeMap::new(),
        }
    }

    fn legacy_post_remove_entry() -> LegacyScriptletEntry {
        let body = "echo replay-post-remove\n";
        LegacyScriptletEntry {
            id: "rpm:%postun".to_string(),
            native_slot: "%postun".to_string(),
            phase: LifecyclePath::PostRemove,
            lifecycle_paths: vec!["remove:post".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: vec!["-e".to_string()],
            body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation {
                args: Vec::new(),
                environment: Vec::new(),
                stdin: None,
                chroot: None,
                extra: BTreeMap::new(),
            },
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                before: Vec::new(),
                after: Vec::new(),
                extra: BTreeMap::new(),
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            decision: ScriptletDecision::Legacy,
            reason_code: "legacy-replay-required".to_string(),
            human_reason: Some("test fixture".to_string()),
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                b"rpm:%postun:echo replay-post-remove",
            )),
            source_evidence_refs: vec!["capture:rpm:%postun".to_string()],
            effects: Vec::new(),
            unknown_commands: Vec::new(),
            blocked_classes: Vec::new(),
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn table_count(conn: &rusqlite::Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    fn changeset_metadata_by_description(
        conn: &rusqlite::Connection,
        description: &str,
    ) -> serde_json::Value {
        let raw: Option<String> = conn
            .query_row(
                "SELECT metadata FROM changesets WHERE description = ?1",
                [description],
                |row| row.get(0),
            )
            .expect("changeset metadata");
        serde_json::from_str(&raw.expect("changeset metadata should be present"))
            .expect("changeset metadata is JSON")
    }
}
