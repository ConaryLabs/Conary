// src/main.rs
//! Conary Package Manager - CLI Entry Point

use anyhow::Result;
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use std::io;

mod cli;
mod commands;

use cli::{Cli, Commands};

// =============================================================================
// Main Entry Point
// =============================================================================

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init { db_path }) => commands::cmd_init(&db_path),

        Some(Commands::Install { package, db_path, root, version, repo, dry_run, no_deps, no_scripts, sandbox, allow_downgrade }) => {
            let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                .expect("Invalid sandbox mode. Use: auto, always, never");
            commands::cmd_install(&package, &db_path, &root, version, repo, dry_run, no_deps, no_scripts, None, sandbox_mode, allow_downgrade)
        }

        Some(Commands::Remove { package_name, db_path, root, version, no_scripts, sandbox }) => {
            let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                .expect("Invalid sandbox mode. Use: auto, always, never");
            commands::cmd_remove(&package_name, &db_path, &root, version, no_scripts, sandbox_mode)
        }

        Some(Commands::Autoremove { db_path, root, dry_run, no_scripts, sandbox }) => {
            let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                .expect("Invalid sandbox mode. Use: auto, always, never");
            commands::cmd_autoremove(&db_path, &root, dry_run, no_scripts, sandbox_mode)
        }

        Some(Commands::AdoptSystem { db_path, full, dry_run }) => {
            commands::cmd_adopt_system(&db_path, full, dry_run)
        }

        Some(Commands::Adopt { packages, db_path, full }) => {
            commands::cmd_adopt(&packages, &db_path, full)
        }

        Some(Commands::AdoptStatus { db_path }) => {
            commands::cmd_adopt_status(&db_path)
        }

        Some(Commands::Conflicts { db_path, verbose }) => {
            commands::cmd_conflicts(&db_path, verbose)
        }

        Some(Commands::Query { pattern, db_path, path, info, files, lsl }) => {
            let options = commands::QueryOptions {
                info,
                lsl,
                path,
                files,
            };
            commands::cmd_query(pattern.as_deref(), &db_path, options)
        }

        Some(Commands::Repquery { pattern, db_path, info }) => {
            commands::cmd_repquery(pattern.as_deref(), &db_path, info)
        }

        Some(Commands::QueryReason { pattern, db_path }) => {
            commands::cmd_query_reason(pattern.as_deref(), &db_path)
        }

        Some(Commands::History { db_path }) => commands::cmd_history(&db_path),

        Some(Commands::Rollback { changeset_id, db_path, root }) => {
            commands::cmd_rollback(changeset_id, &db_path, &root)
        }

        Some(Commands::Verify { package, db_path, root, rpm }) => {
            commands::cmd_verify(package, &db_path, &root, rpm)
        }

        Some(Commands::Depends { package_name, db_path }) => {
            commands::cmd_depends(&package_name, &db_path)
        }

        Some(Commands::Rdepends { package_name, db_path }) => {
            commands::cmd_rdepends(&package_name, &db_path)
        }

        Some(Commands::Deptree { package_name, db_path, reverse, depth }) => {
            commands::cmd_deptree(&package_name, &db_path, reverse, depth)
        }

        Some(Commands::Whatbreaks { package_name, db_path }) => {
            commands::cmd_whatbreaks(&package_name, &db_path)
        }

        Some(Commands::Whatprovides { capability, db_path }) => {
            commands::cmd_whatprovides(&capability, &db_path)
        }

        Some(Commands::ListComponents { package_name, db_path }) => {
            commands::cmd_list_components(&package_name, &db_path)
        }

        Some(Commands::QueryComponent { component_spec, db_path }) => {
            commands::cmd_query_component(&component_spec, &db_path)
        }

        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "conary", &mut io::stdout());
            Ok(())
        }

        Some(Commands::RepoAdd { name, url, db_path, priority, disabled, gpg_key, no_gpg_check, gpg_strict }) => {
            commands::cmd_repo_add(&name, &url, &db_path, priority, disabled, gpg_key, no_gpg_check, gpg_strict)
        }

        Some(Commands::RepoList { db_path, all }) => commands::cmd_repo_list(&db_path, all),

        Some(Commands::RepoRemove { name, db_path }) => {
            commands::cmd_repo_remove(&name, &db_path)
        }

        Some(Commands::RepoEnable { name, db_path }) => {
            commands::cmd_repo_enable(&name, &db_path)
        }

        Some(Commands::RepoDisable { name, db_path }) => {
            commands::cmd_repo_disable(&name, &db_path)
        }

        Some(Commands::RepoSync { name, db_path, force }) => {
            commands::cmd_repo_sync(name, &db_path, force)
        }

        Some(Commands::KeyImport { repository, key, db_path }) => {
            commands::cmd_key_import(&repository, &key, &db_path)
        }

        Some(Commands::KeyList { db_path }) => commands::cmd_key_list(&db_path),

        Some(Commands::KeyRemove { repository, db_path }) => {
            commands::cmd_key_remove(&repository, &db_path)
        }

        Some(Commands::Search { pattern, db_path }) => {
            commands::cmd_search(&pattern, &db_path)
        }

        Some(Commands::Update { package, db_path, root, security }) => {
            commands::cmd_update(package, &db_path, &root, security)
        }

        Some(Commands::UpdateGroup { name, db_path, root, security }) => {
            commands::cmd_update_group(&name, &db_path, &root, security)
        }

        Some(Commands::Pin { package_name, db_path }) => {
            commands::cmd_pin(&package_name, &db_path)
        }

        Some(Commands::Unpin { package_name, db_path }) => {
            commands::cmd_unpin(&package_name, &db_path)
        }

        Some(Commands::ListPinned { db_path }) => commands::cmd_list_pinned(&db_path),

        Some(Commands::DeltaStats { db_path }) => commands::cmd_delta_stats(&db_path),

        Some(Commands::TriggerList { db_path, all, builtin }) => {
            commands::cmd_trigger_list(&db_path, all, builtin)
        }

        Some(Commands::TriggerShow { name, db_path }) => {
            commands::cmd_trigger_show(&name, &db_path)
        }

        Some(Commands::TriggerEnable { name, db_path }) => {
            commands::cmd_trigger_enable(&name, &db_path)
        }

        Some(Commands::TriggerDisable { name, db_path }) => {
            commands::cmd_trigger_disable(&name, &db_path)
        }

        Some(Commands::TriggerAdd { name, pattern, handler, description, priority, db_path }) => {
            commands::cmd_trigger_add(&name, &pattern, &handler, description.as_deref(), priority, &db_path)
        }

        Some(Commands::TriggerRemove { name, db_path }) => {
            commands::cmd_trigger_remove(&name, &db_path)
        }

        Some(Commands::TriggerRun { changeset_id, db_path, root }) => {
            commands::cmd_trigger_run(changeset_id, &db_path, &root)
        }

        Some(Commands::StateList { db_path, limit }) => {
            commands::cmd_state_list(&db_path, limit)
        }

        Some(Commands::StateShow { state_number, db_path }) => {
            commands::cmd_state_show(&db_path, state_number)
        }

        Some(Commands::StateDiff { from_state, to_state, db_path }) => {
            commands::cmd_state_diff(&db_path, from_state, to_state)
        }

        Some(Commands::StateRestore { state_number, db_path, dry_run }) => {
            commands::cmd_state_restore(&db_path, state_number, dry_run)
        }

        Some(Commands::StatePrune { keep, db_path, dry_run }) => {
            commands::cmd_state_prune(&db_path, keep, dry_run)
        }

        Some(Commands::StateCreate { summary, description, db_path }) => {
            commands::cmd_state_create(&db_path, &summary, description.as_deref())
        }

        Some(Commands::LabelList { db_path, verbose }) => {
            commands::cmd_label_list(&db_path, verbose)
        }

        Some(Commands::LabelAdd { label, description, parent, db_path }) => {
            commands::cmd_label_add(&label, description.as_deref(), parent.as_deref(), &db_path)
        }

        Some(Commands::LabelRemove { label, db_path, force }) => {
            commands::cmd_label_remove(&label, &db_path, force)
        }

        Some(Commands::LabelPath { db_path, add, remove, priority }) => {
            commands::cmd_label_path(&db_path, add.as_deref(), remove.as_deref(), priority)
        }

        Some(Commands::LabelShow { package, db_path }) => {
            commands::cmd_label_show(&package, &db_path)
        }

        Some(Commands::LabelSet { package, label, db_path }) => {
            commands::cmd_label_set(&package, &label, &db_path)
        }

        Some(Commands::LabelQuery { label, db_path }) => {
            commands::cmd_label_query(&label, &db_path)
        }

        Some(Commands::ConfigList { package, db_path, all }) => {
            commands::cmd_config_list(&db_path, package.as_deref(), all)
        }

        Some(Commands::ConfigDiff { path, db_path, root }) => {
            commands::cmd_config_diff(&db_path, &path, &root)
        }

        Some(Commands::ConfigBackup { path, db_path, root }) => {
            commands::cmd_config_backup(&db_path, &path, &root)
        }

        Some(Commands::ConfigRestore { path, db_path, root, backup_id }) => {
            commands::cmd_config_restore(&db_path, &path, &root, backup_id)
        }

        Some(Commands::ConfigCheck { package, db_path, root }) => {
            commands::cmd_config_check(&db_path, &root, package.as_deref())
        }

        Some(Commands::ConfigBackups { path, db_path }) => {
            commands::cmd_config_backups(&db_path, &path)
        }

        Some(Commands::Restore { package, db_path, root, force, dry_run }) => {
            if package == "all" {
                commands::cmd_restore_all(&db_path, &root, dry_run)
            } else {
                commands::cmd_restore(&package, &db_path, &root, force, dry_run)
            }
        }

        Some(Commands::Scripts { package_path }) => {
            commands::cmd_scripts(&package_path)
        }

        Some(Commands::CollectionCreate { name, description, members, db_path }) => {
            commands::cmd_collection_create(&name, description.as_deref(), &members, &db_path)
        }

        Some(Commands::CollectionList { db_path }) => {
            commands::cmd_collection_list(&db_path)
        }

        Some(Commands::CollectionShow { name, db_path }) => {
            commands::cmd_collection_show(&name, &db_path)
        }

        Some(Commands::CollectionAdd { name, members, db_path }) => {
            commands::cmd_collection_add(&name, &members, &db_path)
        }

        Some(Commands::CollectionRemove { name, members, db_path }) => {
            commands::cmd_collection_remove_member(&name, &members, &db_path)
        }

        Some(Commands::CollectionDelete { name, db_path }) => {
            commands::cmd_collection_delete(&name, &db_path)
        }

        Some(Commands::CollectionInstall { name, db_path, root, dry_run, skip_optional, sandbox }) => {
            let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                .expect("Invalid sandbox mode. Use: auto, always, never");
            commands::cmd_collection_install(&name, &db_path, &root, dry_run, skip_optional, sandbox_mode)
        }

        None => {
            println!("Conary Package Manager v{}", env!("CARGO_PKG_VERSION"));
            println!("Run 'conary --help' for usage information");
            Ok(())
        }
    }
}
