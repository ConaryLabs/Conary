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
        // =====================================================================
        // Package Commands
        // =====================================================================
        Some(Commands::Package(pkg_cmd)) => match pkg_cmd {
            cli::PackageCommands::Install {
                package, db_path, root, version, repo, dry_run, no_deps,
                no_scripts, sandbox, allow_downgrade, convert_to_ccs, refinery, distro
            } => {
                let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                    .expect("Invalid sandbox mode. Use: auto, always, never");
                commands::cmd_install(&package, &db_path, &root, version, repo, dry_run, no_deps, no_scripts, None, sandbox_mode, allow_downgrade, convert_to_ccs, refinery, distro)
            }

            cli::PackageCommands::Remove { package_name, db_path, root, version, no_scripts, sandbox } => {
                let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                    .expect("Invalid sandbox mode. Use: auto, always, never");
                commands::cmd_remove(&package_name, &db_path, &root, version, no_scripts, sandbox_mode)
            }

            cli::PackageCommands::Autoremove { db_path, root, dry_run, no_scripts, sandbox } => {
                let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                    .expect("Invalid sandbox mode. Use: auto, always, never");
                commands::cmd_autoremove(&db_path, &root, dry_run, no_scripts, sandbox_mode)
            }

            cli::PackageCommands::Update { package, db_path, root, security } => {
                commands::cmd_update(package, &db_path, &root, security)
            }

            cli::PackageCommands::UpdateGroup { name, db_path, root, security } => {
                commands::cmd_update_group(&name, &db_path, &root, security)
            }

            cli::PackageCommands::Pin { package_name, db_path } => {
                commands::cmd_pin(&package_name, &db_path)
            }

            cli::PackageCommands::Unpin { package_name, db_path } => {
                commands::cmd_unpin(&package_name, &db_path)
            }

            cli::PackageCommands::ListPinned { db_path } => {
                commands::cmd_list_pinned(&db_path)
            }

            cli::PackageCommands::AdoptSystem { db_path, full, dry_run } => {
                commands::cmd_adopt_system(&db_path, full, dry_run)
            }

            cli::PackageCommands::Adopt { packages, db_path, full } => {
                commands::cmd_adopt(&packages, &db_path, full)
            }

            cli::PackageCommands::AdoptStatus { db_path } => {
                commands::cmd_adopt_status(&db_path)
            }

            cli::PackageCommands::Conflicts { db_path, verbose } => {
                commands::cmd_conflicts(&db_path, verbose)
            }

            cli::PackageCommands::Verify { package, db_path, root, rpm } => {
                commands::cmd_verify(package, &db_path, &root, rpm)
            }

            cli::PackageCommands::Restore { package, db_path, root, force, dry_run } => {
                if package == "all" {
                    commands::cmd_restore_all(&db_path, &root, dry_run)
                } else {
                    commands::cmd_restore(&package, &db_path, &root, force, dry_run)
                }
            }

            cli::PackageCommands::Scripts { package_path } => {
                commands::cmd_scripts(&package_path)
            }

            cli::PackageCommands::DeltaStats { db_path } => {
                commands::cmd_delta_stats(&db_path)
            }
        }

        // =====================================================================
        // Query Commands
        // =====================================================================
        Some(Commands::Query(query_cmd)) => match query_cmd {
            cli::QueryCommands::List { pattern, db_path, path, info, files, lsl } => {
                let options = commands::QueryOptions {
                    info,
                    lsl,
                    path,
                    files,
                };
                commands::cmd_query(pattern.as_deref(), &db_path, options)
            }

            cli::QueryCommands::Repquery { pattern, db_path, info } => {
                commands::cmd_repquery(pattern.as_deref(), &db_path, info)
            }

            cli::QueryCommands::Reason { pattern, db_path } => {
                commands::cmd_query_reason(pattern.as_deref(), &db_path)
            }

            cli::QueryCommands::Depends { package_name, db_path } => {
                commands::cmd_depends(&package_name, &db_path)
            }

            cli::QueryCommands::Rdepends { package_name, db_path } => {
                commands::cmd_rdepends(&package_name, &db_path)
            }

            cli::QueryCommands::Deptree { package_name, db_path, reverse, depth } => {
                commands::cmd_deptree(&package_name, &db_path, reverse, depth)
            }

            cli::QueryCommands::Whatbreaks { package_name, db_path } => {
                commands::cmd_whatbreaks(&package_name, &db_path)
            }

            cli::QueryCommands::Whatprovides { capability, db_path } => {
                commands::cmd_whatprovides(&capability, &db_path)
            }

            cli::QueryCommands::ListComponents { package_name, db_path } => {
                commands::cmd_list_components(&package_name, &db_path)
            }

            cli::QueryCommands::Component { component_spec, db_path } => {
                commands::cmd_query_component(&component_spec, &db_path)
            }

            cli::QueryCommands::Search { pattern, db_path } => {
                commands::cmd_search(&pattern, &db_path)
            }

            cli::QueryCommands::History { db_path } => {
                commands::cmd_history(&db_path)
            }

            cli::QueryCommands::Sbom { package_name, db_path, format, output } => {
                commands::cmd_sbom(&package_name, &db_path, &format, output.as_deref())
            }
        }

        // =====================================================================
        // Repository Commands
        // =====================================================================
        Some(Commands::Repo(repo_cmd)) => match repo_cmd {
            cli::RepoCommands::Add { name, url, db_path, content_url, priority, disabled, gpg_key, no_gpg_check, gpg_strict } => {
                commands::cmd_repo_add(&name, &url, &db_path, content_url, priority, disabled, gpg_key, no_gpg_check, gpg_strict)
            }

            cli::RepoCommands::List { db_path, all } => {
                commands::cmd_repo_list(&db_path, all)
            }

            cli::RepoCommands::Remove { name, db_path } => {
                commands::cmd_repo_remove(&name, &db_path)
            }

            cli::RepoCommands::Enable { name, db_path } => {
                commands::cmd_repo_enable(&name, &db_path)
            }

            cli::RepoCommands::Disable { name, db_path } => {
                commands::cmd_repo_disable(&name, &db_path)
            }

            cli::RepoCommands::Sync { name, db_path, force } => {
                commands::cmd_repo_sync(name, &db_path, force)
            }

            cli::RepoCommands::KeyImport { repository, key, db_path } => {
                commands::cmd_key_import(&repository, &key, &db_path)
            }

            cli::RepoCommands::KeyList { db_path } => {
                commands::cmd_key_list(&db_path)
            }

            cli::RepoCommands::KeyRemove { repository, db_path } => {
                commands::cmd_key_remove(&repository, &db_path)
            }
        }

        // =====================================================================
        // Config Commands
        // =====================================================================
        Some(Commands::Config(config_cmd)) => match config_cmd {
            cli::ConfigCommands::List { package, db_path, all } => {
                commands::cmd_config_list(&db_path, package.as_deref(), all)
            }

            cli::ConfigCommands::Diff { path, db_path, root } => {
                commands::cmd_config_diff(&db_path, &path, &root)
            }

            cli::ConfigCommands::Backup { path, db_path, root } => {
                commands::cmd_config_backup(&db_path, &path, &root)
            }

            cli::ConfigCommands::Restore { path, db_path, root, backup_id } => {
                commands::cmd_config_restore(&db_path, &path, &root, backup_id)
            }

            cli::ConfigCommands::Check { package, db_path, root } => {
                commands::cmd_config_check(&db_path, &root, package.as_deref())
            }

            cli::ConfigCommands::Backups { path, db_path } => {
                commands::cmd_config_backups(&db_path, &path)
            }
        }

        // =====================================================================
        // State Commands
        // =====================================================================
        Some(Commands::State(state_cmd)) => match state_cmd {
            cli::StateCommands::List { db_path, limit } => {
                commands::cmd_state_list(&db_path, limit)
            }

            cli::StateCommands::Show { state_number, db_path } => {
                commands::cmd_state_show(&db_path, state_number)
            }

            cli::StateCommands::Diff { from_state, to_state, db_path } => {
                commands::cmd_state_diff(&db_path, from_state, to_state)
            }

            cli::StateCommands::Restore { state_number, db_path, dry_run } => {
                commands::cmd_state_restore(&db_path, state_number, dry_run)
            }

            cli::StateCommands::Prune { keep, db_path, dry_run } => {
                commands::cmd_state_prune(&db_path, keep, dry_run)
            }

            cli::StateCommands::Create { summary, description, db_path } => {
                commands::cmd_state_create(&db_path, &summary, description.as_deref())
            }

            cli::StateCommands::Rollback { changeset_id, db_path, root } => {
                commands::cmd_rollback(changeset_id, &db_path, &root)
            }
        }

        // =====================================================================
        // Trigger Commands
        // =====================================================================
        Some(Commands::Trigger(trigger_cmd)) => match trigger_cmd {
            cli::TriggerCommands::List { db_path, all, builtin } => {
                commands::cmd_trigger_list(&db_path, all, builtin)
            }

            cli::TriggerCommands::Show { name, db_path } => {
                commands::cmd_trigger_show(&name, &db_path)
            }

            cli::TriggerCommands::Enable { name, db_path } => {
                commands::cmd_trigger_enable(&name, &db_path)
            }

            cli::TriggerCommands::Disable { name, db_path } => {
                commands::cmd_trigger_disable(&name, &db_path)
            }

            cli::TriggerCommands::Add { name, pattern, handler, description, priority, db_path } => {
                commands::cmd_trigger_add(&name, &pattern, &handler, description.as_deref(), priority, &db_path)
            }

            cli::TriggerCommands::Remove { name, db_path } => {
                commands::cmd_trigger_remove(&name, &db_path)
            }

            cli::TriggerCommands::Run { changeset_id, db_path, root } => {
                commands::cmd_trigger_run(changeset_id, &db_path, &root)
            }
        }

        // =====================================================================
        // Label Commands
        // =====================================================================
        Some(Commands::Label(label_cmd)) => match label_cmd {
            cli::LabelCommands::List { db_path, verbose } => {
                commands::cmd_label_list(&db_path, verbose)
            }

            cli::LabelCommands::Add { label, description, parent, db_path } => {
                commands::cmd_label_add(&label, description.as_deref(), parent.as_deref(), &db_path)
            }

            cli::LabelCommands::Remove { label, db_path, force } => {
                commands::cmd_label_remove(&label, &db_path, force)
            }

            cli::LabelCommands::Path { db_path, add, remove, priority } => {
                commands::cmd_label_path(&db_path, add.as_deref(), remove.as_deref(), priority)
            }

            cli::LabelCommands::Show { package, db_path } => {
                commands::cmd_label_show(&package, &db_path)
            }

            cli::LabelCommands::Set { package, label, db_path } => {
                commands::cmd_label_set(&package, &label, &db_path)
            }

            cli::LabelCommands::Query { label, db_path } => {
                commands::cmd_label_query(&label, &db_path)
            }
        }

        // =====================================================================
        // Collection Commands
        // =====================================================================
        Some(Commands::Collection(coll_cmd)) => match coll_cmd {
            cli::CollectionCommands::Create { name, description, members, db_path } => {
                commands::cmd_collection_create(&name, description.as_deref(), &members, &db_path)
            }

            cli::CollectionCommands::List { db_path } => {
                commands::cmd_collection_list(&db_path)
            }

            cli::CollectionCommands::Show { name, db_path } => {
                commands::cmd_collection_show(&name, &db_path)
            }

            cli::CollectionCommands::Add { name, members, db_path } => {
                commands::cmd_collection_add(&name, &members, &db_path)
            }

            cli::CollectionCommands::Remove { name, members, db_path } => {
                commands::cmd_collection_remove_member(&name, &members, &db_path)
            }

            cli::CollectionCommands::Delete { name, db_path } => {
                commands::cmd_collection_delete(&name, &db_path)
            }

            cli::CollectionCommands::Install { name, db_path, root, dry_run, skip_optional, sandbox } => {
                let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                    .expect("Invalid sandbox mode. Use: auto, always, never");
                commands::cmd_collection_install(&name, &db_path, &root, dry_run, skip_optional, sandbox_mode)
            }
        }

        // =====================================================================
        // CCS Commands
        // =====================================================================
        Some(Commands::Ccs(ccs_cmd)) => match ccs_cmd {
            cli::CcsCommands::Init { path, name, version, force } => {
                commands::ccs::cmd_ccs_init(&path, name, &version, force)
            }

            cli::CcsCommands::Build { path, output, target, source, no_classify, no_chunked, dry_run } => {
                commands::ccs::cmd_ccs_build(&path, &output, &target, source, no_classify, !no_chunked, dry_run)
            }

            cli::CcsCommands::Inspect { package, files, hooks, deps, format } => {
                commands::ccs::cmd_ccs_inspect(&package, files, hooks, deps, &format)
            }

            cli::CcsCommands::Verify { package, policy, allow_unsigned } => {
                commands::ccs::cmd_ccs_verify(&package, policy, allow_unsigned)
            }

            cli::CcsCommands::Sign { package, key, output } => {
                commands::ccs::cmd_ccs_sign(&package, &key, output)
            }

            cli::CcsCommands::Keygen { output, key_id, force } => {
                commands::ccs::cmd_ccs_keygen(&output, key_id, force)
            }

            cli::CcsCommands::Install { package, db_path, root, dry_run, allow_unsigned, policy, components, sandbox, no_deps } => {
                let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                    .expect("Invalid sandbox mode. Use: auto, always, never");
                commands::ccs::cmd_ccs_install(&package, &db_path, &root, dry_run, allow_unsigned, policy, components, sandbox_mode, no_deps)
            }

            cli::CcsCommands::Export { packages, output, format, db_path } => {
                commands::ccs::cmd_ccs_export(&packages, &output, &format, &db_path)
            }

            cli::CcsCommands::Shell { packages, db_path, shell, env, keep } => {
                commands::ccs::cmd_ccs_shell(&packages, &db_path, shell.as_deref(), &env, keep)
            }

            cli::CcsCommands::Run { package, command, db_path, env } => {
                commands::ccs::cmd_ccs_run(&package, &command, &db_path, &env)
            }
        }

        // =====================================================================
        // Derive Commands
        // =====================================================================
        Some(Commands::Derive(derive_cmd)) => match derive_cmd {
            cli::DeriveCommands::List { db_path, verbose } => {
                commands::cmd_derive_list(&db_path, verbose)
            }

            cli::DeriveCommands::Show { name, db_path } => {
                commands::cmd_derive_show(&name, &db_path)
            }

            cli::DeriveCommands::Create { name, from, version_suffix, description, db_path } => {
                commands::cmd_derive_create(&name, &from, version_suffix.as_deref(), description.as_deref(), &db_path)
            }

            cli::DeriveCommands::Patch { name, patch_file, strip, db_path } => {
                commands::cmd_derive_patch(&name, &patch_file, strip, &db_path)
            }

            cli::DeriveCommands::Override { name, target, source, mode, db_path } => {
                commands::cmd_derive_override(&name, &target, source.as_deref(), mode, &db_path)
            }

            cli::DeriveCommands::Build { name, db_path } => {
                commands::cmd_derive_build(&name, &db_path)
            }

            cli::DeriveCommands::Delete { name, db_path } => {
                commands::cmd_derive_delete(&name, &db_path)
            }

            cli::DeriveCommands::Stale { db_path } => {
                commands::cmd_derive_stale(&db_path)
            }
        }

        // =====================================================================
        // Model Commands
        // =====================================================================
        Some(Commands::Model(model_cmd)) => match model_cmd {
            cli::ModelCommands::Diff { model, db_path } => {
                commands::cmd_model_diff(&model, &db_path)
            }

            cli::ModelCommands::Apply { model, db_path, root, dry_run, skip_optional, strict, no_autoremove } => {
                commands::cmd_model_apply(&model, &db_path, &root, dry_run, skip_optional, strict, !no_autoremove)
            }

            cli::ModelCommands::Check { model, db_path, verbose } => {
                commands::cmd_model_check(&model, &db_path, verbose)
            }

            cli::ModelCommands::Snapshot { output, db_path, description } => {
                commands::cmd_model_snapshot(&output, &db_path, description.as_deref())
            }
        }

        // =====================================================================
        // Redirect Commands
        // =====================================================================
        Some(Commands::Redirect(redirect_cmd)) => match redirect_cmd {
            cli::RedirectCommands::List { db_path, r#type, verbose } => {
                commands::cmd_redirect_list(&db_path, r#type.as_deref(), verbose)
            }

            cli::RedirectCommands::Add { source, target, db_path, r#type, source_version, target_version, message } => {
                commands::cmd_redirect_add(&source, &target, &db_path, &r#type, source_version.as_deref(), target_version.as_deref(), message.as_deref())
            }

            cli::RedirectCommands::Show { source, db_path, version } => {
                commands::cmd_redirect_show(&source, &db_path, version.as_deref())
            }

            cli::RedirectCommands::Remove { source, db_path } => {
                commands::cmd_redirect_remove(&source, &db_path)
            }

            cli::RedirectCommands::Resolve { package, db_path, version } => {
                commands::cmd_redirect_resolve(&package, &db_path, version.as_deref())
            }
        }

        // =====================================================================
        // System Commands
        // =====================================================================
        Some(Commands::System(sys_cmd)) => match sys_cmd {
            cli::SystemCommands::Init { db_path } => {
                commands::cmd_init(&db_path)
            }

            cli::SystemCommands::Completions { shell } => {
                let mut cmd = Cli::command();
                generate(shell, &mut cmd, "conary", &mut io::stdout());
                Ok(())
            }

            #[cfg(feature = "server")]
            cli::SystemCommands::IndexGen {
                db_path,
                chunk_dir,
                output_dir,
                distro,
                sign_key,
            } => {
                use conary::server::{generate_indices, IndexGenConfig};

                let config = IndexGenConfig {
                    db_path,
                    chunk_dir,
                    output_dir,
                    distro,
                    sign_key,
                };

                match generate_indices(&config) {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No indices generated.");
                        } else {
                            for result in results {
                                println!(
                                    "{}: {} packages ({} versions) -> {}{}",
                                    result.distro,
                                    result.package_count,
                                    result.version_count,
                                    result.index_path,
                                    if result.signed { " [signed]" } else { "" }
                                );
                            }
                        }
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }

            #[cfg(feature = "server")]
            cli::SystemCommands::Prewarm {
                db_path,
                chunk_dir,
                cache_dir,
                distro,
                max_packages,
                popularity_file,
                pattern,
                dry_run,
            } => {
                use conary::server::{run_prewarm, PrewarmConfig};

                let config = PrewarmConfig {
                    db_path,
                    chunk_dir,
                    cache_dir,
                    distro,
                    max_packages,
                    popularity_file,
                    pattern,
                    dry_run,
                };

                match run_prewarm(&config) {
                    Ok(result) => {
                        println!("Pre-warm complete:");
                        println!("  Processed:  {}", result.packages_processed);
                        println!("  Converted:  {}", result.packages_converted);
                        println!("  Skipped:    {}", result.packages_skipped);
                        println!("  Failed:     {}", result.packages_failed);
                        println!("  Total size: {} bytes", result.total_bytes);

                        if !result.converted.is_empty() {
                            println!("\nConverted packages:");
                            for pkg in &result.converted {
                                println!("  {}", pkg);
                            }
                        }

                        if !result.failed.is_empty() {
                            println!("\nFailed packages:");
                            for (pkg, err) in &result.failed {
                                println!("  {}: {}", pkg, err);
                            }
                        }

                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }

            #[cfg(feature = "server")]
            cli::SystemCommands::Server {
                bind,
                db_path,
                chunk_dir,
                cache_dir,
                max_concurrent,
                max_cache_gb,
                chunk_ttl_days,
            } => {
                use conary::server::{run_server, ServerConfig};
                use std::path::PathBuf;

                let config = ServerConfig {
                    bind_addr: bind.parse().expect("Invalid bind address"),
                    db_path: PathBuf::from(db_path),
                    chunk_dir: PathBuf::from(chunk_dir),
                    cache_dir: PathBuf::from(cache_dir),
                    max_concurrent_conversions: max_concurrent,
                    cache_max_bytes: max_cache_gb * 1024 * 1024 * 1024,
                    chunk_ttl_days,
                };

                // Run the async server
                tokio::runtime::Runtime::new()
                    .expect("Failed to create Tokio runtime")
                    .block_on(run_server(config))
            }

            cli::SystemCommands::Gc { db_path, objects_dir, keep_days, dry_run } => {
                commands::cmd_gc(&db_path, &objects_dir, keep_days, dry_run)
            }
        }

        None => {
            println!("Conary Package Manager v{}", env!("CARGO_PKG_VERSION"));
            println!("Run 'conary --help' for usage information");
            Ok(())
        }
    }
}
