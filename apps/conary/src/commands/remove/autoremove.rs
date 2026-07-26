// apps/conary/src/commands/remove/autoremove.rs

use anyhow::Result;
use conary_core::db::models::{PackagePayloadOwnership, Trove};
use conary_core::scriptlet::ExecutionMode;
use std::collections::HashSet;
use tracing::info;

use super::types::RemoveLifecycleOptions;
use crate::commands::{SandboxMode, open_db};

#[derive(Debug, Clone, PartialEq, Eq)]
enum AutoremoveSkipReason {
    AdoptedNativeAuthority,
    Pinned,
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
pub async fn cmd_autoremove(db_path: &str, dry_run: bool, sandbox_mode: SandboxMode) -> Result<()> {
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
            db_path,
            RemoveLifecycleOptions::new(sandbox_mode),
        )?;

        let mut round_removed = 0;
        for trove in &current_plan.removable {
            println!("\nRemoving {} {}...", trove.name, trove.version);
            match super::cmd_remove(
                &trove.name,
                db_path,
                Some(trove.version.clone()),
                trove.architecture.clone(),
                sandbox_mode,
                false,
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
    db_path: &str,
    lifecycle_options: RemoveLifecycleOptions,
) -> Result<()> {
    for trove in troves {
        let Some(trove_id) = trove.id else {
            anyhow::bail!(
                "autoremove lifecycle execution preflight failed for {} {}: trove has no id",
                trove.name,
                trove.version
            );
        };
        let paths = PackagePayloadOwnership::load(conn, trove_id)?
            .lifecycle_paths()
            .to_vec();
        let native_transaction =
            crate::commands::install::native_events::PreparedNativeTransaction::prepare_remove(
                conn,
                trove_id,
                &trove.name,
                &trove.version,
                paths,
                false,
            );
        let native_transaction = match native_transaction {
            Ok(transaction) => transaction,
            Err(error) => {
                anyhow::bail!(
                    "autoremove lifecycle execution preflight failed for {} {}: {error}",
                    trove.name,
                    trove.version
                );
            }
        };
        let selected = crate::commands::generation::selected_root::SelectedRootSession::begin(
            conn,
            db_path,
            format!("Autoremove preflight {}-{}", trove.name, trove.version),
        )?;
        if let Err(error) =
            native_transaction.preflight(selected.selected_root(), &ExecutionMode::Remove)
        {
            anyhow::bail!(
                "autoremove lifecycle execution preflight failed for {} {}: {error}",
                trove.name,
                trove.version
            );
        }
        let selected_root = selected
            .selected_root()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("autoremove selected root is not valid UTF-8"))?;
        super::preflight_ccs_remove_hook(
            conn,
            trove,
            selected_root,
            lifecycle_options.sandbox_mode,
        )?;
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

    let protected = skipped
        .iter()
        .filter(|(_, reason)| *reason != AutoremoveSkipReason::AdoptedNativeAuthority)
        .collect::<Vec<_>>();
    if !protected.is_empty() {
        println!("Skipping protected orphaned package(s):");
        for (trove, reason) in protected {
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
    use conary_core::ccs::native_lifecycle::{
        LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation, NativeLifecycleBundle,
        NativeLifecycleEntry, NativeLifecycleEntryKind, ScriptletFidelity, SourceFormat,
        TransactionOrder, VersionScheme,
    };
    use conary_core::db::models::{
        InstallReason, InstallSource, InstalledNativeLifecycleBundle, TroveType,
    };
    use tempfile::TempDir;

    #[test]
    fn autoremove_plan_uses_recorded_ownership_and_pin_state() {
        let owned = Trove::new_with_source(
            "owned-orphan".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let adopted = Trove::new_with_source(
            "adopted-orphan".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let mut pinned = Trove::new_with_source(
            "pinned-orphan".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        pinned.pinned = true;
        let ordinary_named_bash = Trove::new_with_source(
            "bash".to_string(),
            "5.2.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Conary,
        );

        let plan = plan_autoremove(vec![owned, adopted, pinned, ordinary_named_bash]);

        assert_eq!(
            plan.removable
                .iter()
                .map(|trove| trove.name.as_str())
                .collect::<Vec<_>>(),
            vec!["owned-orphan", "bash"]
        );
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
            ]
        );
    }

    #[tokio::test]
    async fn autoremove_automatically_replays_native_remove_lifecycle() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

        let conn = conary_core::db::open(&db_path).unwrap();
        conary_core::db::models::DistroPin::set(
            &conn,
            "fedora-44",
            conary_core::repository::resolution_policy::DependencyMixingPolicy::Strict,
        )
        .unwrap();
        seed_dependency_trove(
            &conn,
            "aa-plain-orphan",
            "1.0.0",
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let native_trove_id = seed_dependency_trove(
            &conn,
            "zz-native-orphan",
            "1.0.0-1.fc44",
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        seed_installed_native_lifecycle_bundle(&conn, native_trove_id, "zz-native-orphan");
        drop(conn);

        cmd_autoremove(
            db_path.to_string_lossy().as_ref(),
            false,
            SandboxMode::Always,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(&db_path).unwrap();
        assert_eq!(table_count(&conn, "troves"), 1);
        assert!(
            Trove::find_one_by_name(&conn, "test-runtime-base")
                .unwrap()
                .is_some()
        );
        assert_eq!(table_count(&conn, "installed_native_lifecycle_bundles"), 0);
        assert_eq!(table_count(&conn, "changesets"), 2);
        let metadata =
            changeset_metadata_by_description(&conn, "Remove zz-native-orphan-1.0.0-1.fc44");
        assert!(
            metadata["removed_troves"][0]["native_lifecycle"]["bundle_toml"]
                .as_str()
                .unwrap()
                .contains("rpm:%postun")
        );

        let runtime_root =
            conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.clone());
        let generation = conary_core::generation::mount::current_generation(runtime_root.root())
            .unwrap()
            .expect("autoremove should publish a generation");
        let artifact = conary_core::generation::artifact::load_generation_artifact(
            &runtime_root.generation_path(generation),
        )
        .unwrap();
        let marker = artifact
            .generation_root
            .entries
            .iter()
            .find(|entry| entry.path == "/autoremove-native-lifecycle-ran")
            .expect("RPM remove lifecycle marker in published root");
        let content = marker.content.as_ref().expect("marker content authority");
        assert_eq!(
            conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
                .unwrap()
                .retrieve(&content.sha256)
                .unwrap(),
            b"zz-native-orphan"
        );
    }

    fn seed_dependency_trove(
        conn: &rusqlite::Connection,
        name: &str,
        version: &str,
        version_scheme: conary_core::repository::versioning::VersionScheme,
    ) -> i64 {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            InstallSource::Repository,
            version_scheme,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.install_reason = InstallReason::Dependency;
        trove.selection_reason = Some("Required by fixture-root".to_string());
        trove.insert(conn).unwrap()
    }

    fn seed_installed_native_lifecycle_bundle(
        conn: &rusqlite::Connection,
        trove_id: i64,
        package: &str,
    ) {
        let bundle = native_post_remove_bundle(package);
        let mut installed = InstalledNativeLifecycleBundle::new(trove_id, None, &bundle).unwrap();
        installed.insert_or_replace(conn).unwrap();
    }

    fn native_post_remove_bundle(package: &str) -> NativeLifecycleBundle {
        let entry = native_post_remove_entry();
        NativeLifecycleBundle {
            schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
            schema_revision: conary_core::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_profile: Some("fedora-44".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: package.to_string(),
            source_version: "1.0.0-1.fc44".to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "test".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "goal6-autoremove-test".to_string(),
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                format!("{package}-native-remove-evidence").as_bytes(),
            )),
            scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
            entries: vec![entry],
        }
    }

    fn native_post_remove_entry() -> NativeLifecycleEntry {
        let body = r#"
local marker = assert(io.open("/autoremove-native-lifecycle-ran", "w"))
marker:write("zz-native-orphan")
marker:close()
"#;
        NativeLifecycleEntry {
            id: "rpm:%postun".to_string(),
            native_slot: "%postun".to_string(),
            kind: NativeLifecycleEntryKind::Executable,
            phase: LifecyclePath::PostRemove,
            lifecycle_paths: vec!["remove:post".to_string()],
            interpreter: "<lua>".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation {
                args: Vec::new(),
                environment: Vec::new(),
                stdin: None,
                chroot: None,
            },
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                before: Vec::new(),
                after: Vec::new(),
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                b"rpm:%postun:print replay-post-remove",
            )),
            source_evidence_refs: vec!["capture:rpm:%postun".to_string()],
            rpm_trigger: None,
            rpm_runtime: Some(conary_core::ccs::native_lifecycle::RpmRuntimeMetadata {
                program: conary_core::ccs::native_lifecycle::RpmProgram::EmbeddedLua,
                body_transforms: Vec::new(),
                critical: false,
                criticality: conary_core::ccs::native_lifecycle::RpmCriticality::WarningOnly,
                raw_flags: 0,
                unknown_flags: 0,
                install_prefixes: Vec::new(),
                macro_context: Default::default(),
                header_context: Default::default(),
                package_rpm_version: None,
            }),
            rpm_sysusers: None,
            deb_maintainer: None,
            arch_install: None,
            arch_hook: None,
            residual_lifecycle: None,
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
