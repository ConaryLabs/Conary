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
        // Primary Commands (Hoisted to Root)
        // =====================================================================
        Some(Commands::Install {
            package, common, version, repo, dry_run, no_deps,
            no_scripts, sandbox, allow_downgrade, convert_to_ccs,
            no_capture, skip_optional,
        }) => {
            let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                .expect("Invalid sandbox mode. Use: auto, always, never");

            // Smart dispatch: @name installs a collection
            if package.starts_with('@') {
                let name = package.trim_start_matches('@');
                commands::cmd_collection_install(name, &common.db.db_path, &common.root, dry_run, skip_optional, sandbox_mode)
            } else {
                commands::cmd_install(&package, &common.db.db_path, &common.root, version, repo, dry_run, no_deps, no_scripts, None, sandbox_mode, allow_downgrade, convert_to_ccs, no_capture)
            }
        }

        Some(Commands::Remove { package_name, common, version, no_scripts, sandbox }) => {
            let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                .expect("Invalid sandbox mode. Use: auto, always, never");
            commands::cmd_remove(&package_name, &common.db.db_path, &common.root, version, no_scripts, sandbox_mode)
        }

        Some(Commands::Update { package, common, security }) => {
            // Smart dispatch: @name updates a collection/group
            if let Some(ref pkg) = package
                && pkg.starts_with('@')
            {
                let name = pkg.trim_start_matches('@');
                return commands::cmd_update_group(name, &common.db.db_path, &common.root, security);
            }
            commands::cmd_update(package, &common.db.db_path, &common.root, security)
        }

        Some(Commands::Search { pattern, db }) => {
            commands::cmd_search(&pattern, &db.db_path)
        }

        Some(Commands::List { pattern, db, path, info, files, lsl, pinned }) => {
            if pinned {
                commands::cmd_list_pinned(&db.db_path)
            } else {
                let options = commands::QueryOptions {
                    info,
                    lsl,
                    path,
                    files,
                };
                commands::cmd_query(pattern.as_deref(), &db.db_path, options)
            }
        }

        Some(Commands::Autoremove { common, dry_run, no_scripts, sandbox }) => {
            let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                .expect("Invalid sandbox mode. Use: auto, always, never");
            commands::cmd_autoremove(&common.db.db_path, &common.root, dry_run, no_scripts, sandbox_mode)
        }

        Some(Commands::Pin { package_name, db }) => {
            commands::cmd_pin(&package_name, &db.db_path)
        }

        Some(Commands::Unpin { package_name, db }) => {
            commands::cmd_unpin(&package_name, &db.db_path)
        }

        Some(Commands::Cook { recipe, output, source_cache, jobs, keep_builddir, validate_only, fetch_only, no_isolation, hermetic }) => {
            commands::cmd_cook(&recipe, &output, &source_cache, jobs, keep_builddir, validate_only, fetch_only, no_isolation, hermetic)
        }

        Some(Commands::ConvertPkgbuild { pkgbuild, output }) => {
            commands::cmd_convert_pkgbuild(&pkgbuild, output.as_deref())
        }

        // =====================================================================
        // System Commands
        // =====================================================================
        Some(Commands::System(sys_cmd)) => match sys_cmd {
            cli::SystemCommands::Init { db } => {
                commands::cmd_init(&db.db_path)
            }

            cli::SystemCommands::Completions { shell } => {
                let mut cmd = Cli::command();
                generate(shell, &mut cmd, "conary", &mut io::stdout());
                Ok(())
            }

            cli::SystemCommands::History { db } => {
                commands::cmd_history(&db.db_path)
            }

            cli::SystemCommands::Verify { package, common, rpm } => {
                commands::cmd_verify(package, &common.db.db_path, &common.root, rpm)
            }

            cli::SystemCommands::Restore { package, common, force, dry_run } => {
                if package == "all" {
                    commands::cmd_restore_all(&common.db.db_path, &common.root, dry_run)
                } else {
                    commands::cmd_restore(&package, &common.db.db_path, &common.root, force, dry_run)
                }
            }

            cli::SystemCommands::Adopt { packages, db, full, system, status, dry_run } => {
                if status {
                    commands::cmd_adopt_status(&db.db_path)
                } else if system {
                    commands::cmd_adopt_system(&db.db_path, full, dry_run)
                } else {
                    commands::cmd_adopt(&packages, &db.db_path, full)
                }
            }

            cli::SystemCommands::Gc { db, objects_dir, keep_days, dry_run } => {
                commands::cmd_gc(&db.db_path, &objects_dir, keep_days, dry_run)
            }

            cli::SystemCommands::Sbom { package_name, db, format, output } => {
                commands::cmd_sbom(&package_name, &db.db_path, &format, output.as_deref())
            }

            #[cfg(feature = "server")]
            cli::SystemCommands::IndexGen {
                db,
                chunk_dir,
                output_dir,
                distro,
                sign_key,
            } => {
                use conary::server::{generate_indices, IndexGenConfig};

                let config = IndexGenConfig {
                    db_path: db.db_path,
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
                db,
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
                    db_path: db.db_path,
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
                db,
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
                    db_path: PathBuf::from(db.db_path),
                    chunk_dir: PathBuf::from(chunk_dir),
                    cache_dir: PathBuf::from(cache_dir),
                    max_concurrent_conversions: max_concurrent,
                    cache_max_bytes: max_cache_gb * 1024 * 1024 * 1024,
                    chunk_ttl_days,
                    // Use defaults for Phase 0 features
                    ..Default::default()
                };

                // Run the async server
                tokio::runtime::Runtime::new()
                    .expect("Failed to create Tokio runtime")
                    .block_on(run_server(config))
            }

            // Nested: system state
            cli::SystemCommands::State(state_cmd) => match state_cmd {
                cli::StateCommands::List { db, limit } => {
                    commands::cmd_state_list(&db.db_path, limit)
                }

                cli::StateCommands::Show { state_number, db } => {
                    commands::cmd_state_show(&db.db_path, state_number)
                }

                cli::StateCommands::Diff { from_state, to_state, db } => {
                    commands::cmd_state_diff(&db.db_path, from_state, to_state)
                }

                cli::StateCommands::Revert { state_number, db, dry_run } => {
                    commands::cmd_state_restore(&db.db_path, state_number, dry_run)
                }

                cli::StateCommands::Prune { keep, db, dry_run } => {
                    commands::cmd_state_prune(&db.db_path, keep, dry_run)
                }

                cli::StateCommands::Create { summary, description, db } => {
                    commands::cmd_state_create(&db.db_path, &summary, description.as_deref())
                }

                cli::StateCommands::Rollback { changeset_id, common } => {
                    commands::cmd_rollback(changeset_id, &common.db.db_path, &common.root)
                }
            }

            // Nested: system trigger
            cli::SystemCommands::Trigger(trigger_cmd) => match trigger_cmd {
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

            // Nested: system redirect
            cli::SystemCommands::Redirect(redirect_cmd) => match redirect_cmd {
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
        }

        // =====================================================================
        // Repository Commands
        // =====================================================================
        Some(Commands::Repo(repo_cmd)) => match repo_cmd {
            cli::RepoCommands::Add { name, url, db, content_url, priority, disabled, gpg_key, no_gpg_check, gpg_strict, default_strategy, remi_endpoint, remi_distro } => {
                commands::cmd_repo_add(&name, &url, &db.db_path, content_url, priority, disabled, gpg_key, no_gpg_check, gpg_strict, default_strategy, remi_endpoint, remi_distro)
            }

            cli::RepoCommands::List { db, all } => {
                commands::cmd_repo_list(&db.db_path, all)
            }

            cli::RepoCommands::Remove { name, db } => {
                commands::cmd_repo_remove(&name, &db.db_path)
            }

            cli::RepoCommands::Enable { name, db } => {
                commands::cmd_repo_enable(&name, &db.db_path)
            }

            cli::RepoCommands::Disable { name, db } => {
                commands::cmd_repo_disable(&name, &db.db_path)
            }

            cli::RepoCommands::Sync { name, db, force } => {
                commands::cmd_repo_sync(name, &db.db_path, force)
            }

            cli::RepoCommands::KeyImport { repository, key, db } => {
                commands::cmd_key_import(&repository, &key, &db.db_path)
            }

            cli::RepoCommands::KeyList { db } => {
                commands::cmd_key_list(&db.db_path)
            }

            cli::RepoCommands::KeyRemove { repository, db } => {
                commands::cmd_key_remove(&repository, &db.db_path)
            }
        }

        // =====================================================================
        // Config Commands
        // =====================================================================
        Some(Commands::Config(config_cmd)) => match config_cmd {
            cli::ConfigCommands::List { package, db, all } => {
                commands::cmd_config_list(&db.db_path, package.as_deref(), all)
            }

            cli::ConfigCommands::Diff { path, common } => {
                commands::cmd_config_diff(&common.db.db_path, &path, &common.root)
            }

            cli::ConfigCommands::Backup { path, common } => {
                commands::cmd_config_backup(&common.db.db_path, &path, &common.root)
            }

            cli::ConfigCommands::Restore { path, common, backup_id } => {
                commands::cmd_config_restore(&common.db.db_path, &path, &common.root, backup_id)
            }

            cli::ConfigCommands::Check { package, common } => {
                commands::cmd_config_check(&common.db.db_path, &common.root, package.as_deref())
            }

            cli::ConfigCommands::Backups { path, db } => {
                commands::cmd_config_backups(&db.db_path, &path)
            }
        }

        // =====================================================================
        // Query Commands
        // =====================================================================
        Some(Commands::Query(query_cmd)) => match query_cmd {
            cli::QueryCommands::Depends { package_name, db } => {
                commands::cmd_depends(&package_name, &db.db_path)
            }

            cli::QueryCommands::Rdepends { package_name, db } => {
                commands::cmd_rdepends(&package_name, &db.db_path)
            }

            cli::QueryCommands::Deptree { package_name, db, reverse, depth } => {
                commands::cmd_deptree(&package_name, &db.db_path, reverse, depth)
            }

            cli::QueryCommands::Whatprovides { capability, db } => {
                commands::cmd_whatprovides(&capability, &db.db_path)
            }

            cli::QueryCommands::Whatbreaks { package_name, db } => {
                commands::cmd_whatbreaks(&package_name, &db.db_path)
            }

            cli::QueryCommands::Reason { pattern, db } => {
                commands::cmd_query_reason(pattern.as_deref(), &db.db_path)
            }

            cli::QueryCommands::Repquery { pattern, db, info } => {
                commands::cmd_repquery(pattern.as_deref(), &db.db_path, info)
            }

            cli::QueryCommands::Component { component_spec, db } => {
                commands::cmd_query_component(&component_spec, &db.db_path)
            }

            cli::QueryCommands::Components { package_name, db } => {
                commands::cmd_list_components(&package_name, &db.db_path)
            }

            cli::QueryCommands::Scripts { package_path } => {
                commands::cmd_scripts(&package_path)
            }

            cli::QueryCommands::DeltaStats { db } => {
                commands::cmd_delta_stats(&db.db_path)
            }

            cli::QueryCommands::Conflicts { db, verbose } => {
                commands::cmd_conflicts(&db.db_path, verbose)
            }

            // Nested: query label
            cli::QueryCommands::Label(label_cmd) => match label_cmd {
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

                cli::LabelCommands::Link { label, repository, unlink, db_path } => {
                    commands::cmd_label_link(&label, repository.as_deref(), unlink, &db_path)
                }

                cli::LabelCommands::Delegate { label, target, undelegate, db_path } => {
                    commands::cmd_label_delegate(&label, target.as_deref(), undelegate, &db_path)
                }
            }
        }

        // =====================================================================
        // Collection Commands
        // =====================================================================
        Some(Commands::Collection(coll_cmd)) => match coll_cmd {
            cli::CollectionCommands::Create { name, description, members, db } => {
                commands::cmd_collection_create(&name, description.as_deref(), &members, &db.db_path)
            }

            cli::CollectionCommands::List { db } => {
                commands::cmd_collection_list(&db.db_path)
            }

            cli::CollectionCommands::Show { name, db } => {
                commands::cmd_collection_show(&name, &db.db_path)
            }

            cli::CollectionCommands::Add { name, members, db } => {
                commands::cmd_collection_add(&name, &members, &db.db_path)
            }

            cli::CollectionCommands::Remove { name, members, db } => {
                commands::cmd_collection_remove_member(&name, &members, &db.db_path)
            }

            cli::CollectionCommands::Delete { name, db } => {
                commands::cmd_collection_delete(&name, &db.db_path)
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

            cli::CcsCommands::Install { package, common, dry_run, allow_unsigned, policy, components, sandbox, no_deps } => {
                let sandbox_mode = commands::SandboxMode::parse(&sandbox)
                    .expect("Invalid sandbox mode. Use: auto, always, never");
                commands::ccs::cmd_ccs_install(&package, &common.db.db_path, &common.root, dry_run, allow_unsigned, policy, components, sandbox_mode, no_deps)
            }

            cli::CcsCommands::Export { packages, output, format, db } => {
                commands::ccs::cmd_ccs_export(&packages, &output, &format, &db.db_path)
            }

            cli::CcsCommands::Shell { packages, db, shell, env, keep } => {
                commands::ccs::cmd_ccs_shell(&packages, &db.db_path, shell.as_deref(), &env, keep)
            }

            cli::CcsCommands::Run { package, command, db, env } => {
                commands::ccs::cmd_ccs_run(&package, &command, &db.db_path, &env)
            }

            cli::CcsCommands::Enhance { db, trove_id, all_pending, update_outdated, types, force, stats, dry_run, install_root } => {
                commands::ccs::cmd_ccs_enhance(&db.db_path, trove_id, all_pending, update_outdated, types, force, stats, dry_run, &install_root)
            }
        }

        // =====================================================================
        // Derive Commands
        // =====================================================================
        Some(Commands::Derive(derive_cmd)) => match derive_cmd {
            cli::DeriveCommands::List { db, verbose } => {
                commands::cmd_derive_list(&db.db_path, verbose)
            }

            cli::DeriveCommands::Show { name, db } => {
                commands::cmd_derive_show(&name, &db.db_path)
            }

            cli::DeriveCommands::Create { name, from, version_suffix, description, db } => {
                commands::cmd_derive_create(&name, &from, version_suffix.as_deref(), description.as_deref(), &db.db_path)
            }

            cli::DeriveCommands::Patch { name, patch_file, strip, db } => {
                commands::cmd_derive_patch(&name, &patch_file, strip, &db.db_path)
            }

            cli::DeriveCommands::Override { name, target, source, mode, db } => {
                commands::cmd_derive_override(&name, &target, source.as_deref(), mode, &db.db_path)
            }

            cli::DeriveCommands::Build { name, db } => {
                commands::cmd_derive_build(&name, &db.db_path)
            }

            cli::DeriveCommands::Delete { name, db } => {
                commands::cmd_derive_delete(&name, &db.db_path)
            }

            cli::DeriveCommands::Stale { db } => {
                commands::cmd_derive_stale(&db.db_path)
            }
        }

        // =====================================================================
        // Model Commands
        // =====================================================================
        Some(Commands::Model(model_cmd)) => match model_cmd {
            cli::ModelCommands::Diff { model, db } => {
                commands::cmd_model_diff(&model, &db.db_path)
            }

            cli::ModelCommands::Apply { model, common, dry_run, skip_optional, strict, no_autoremove } => {
                commands::cmd_model_apply(&model, &common.db.db_path, &common.root, dry_run, skip_optional, strict, !no_autoremove)
            }

            cli::ModelCommands::Check { model, db, verbose } => {
                commands::cmd_model_check(&model, &db.db_path, verbose)
            }

            cli::ModelCommands::Snapshot { output, db, description } => {
                commands::cmd_model_snapshot(&output, &db.db_path, description.as_deref())
            }

            cli::ModelCommands::Publish { model, name, version, repo, description, db } => {
                commands::cmd_model_publish(&model, &name, &version, &repo, description.as_deref(), &db.db_path)
            }
        }

        // =====================================================================
        // Automation Commands
        // =====================================================================
        Some(Commands::Automation(auto_cmd)) => match auto_cmd {
            cli::AutomationCommands::Status { db, format, verbose } => {
                commands::cmd_automation_status(&db.db_path, &format, verbose)
            }

            cli::AutomationCommands::Check { common, categories, quiet } => {
                commands::cmd_automation_check(&common.db.db_path, &common.root, categories, quiet)
            }

            cli::AutomationCommands::Apply { common, yes, categories, dry_run, no_scripts } => {
                commands::cmd_automation_apply(&common.db.db_path, &common.root, yes, categories, dry_run, no_scripts)
            }

            cli::AutomationCommands::Configure { db, show, mode, enable, disable, interval, enable_ai, disable_ai } => {
                commands::cmd_automation_configure(&db.db_path, show, mode, enable, disable, interval, enable_ai, disable_ai)
            }

            cli::AutomationCommands::Daemon { common, foreground, pidfile } => {
                commands::cmd_automation_daemon(&common.db.db_path, &common.root, foreground, &pidfile)
            }

            cli::AutomationCommands::History { db, limit, category, status, since } => {
                commands::cmd_automation_history(&db.db_path, limit, category, status, since)
            }

            #[cfg(feature = "experimental")]
            cli::AutomationCommands::Ai(ai_cmd) => match ai_cmd {
                cli::AiCommands::Find { intent, db, limit, verbose } => {
                    commands::cmd_ai_find(&db.db_path, &intent, limit, verbose)
                }

                cli::AiCommands::Translate { source, format, confidence } => {
                    commands::cmd_ai_translate(&source, &format, confidence)
                }

                cli::AiCommands::Query { question, db } => {
                    commands::cmd_ai_query(&db.db_path, &question)
                }

                cli::AiCommands::Explain { command, db } => {
                    commands::cmd_ai_explain(&db.db_path, &command)
                }
            }
        }

        // =====================================================================
        // Bootstrap Commands
        // =====================================================================
        Some(Commands::Bootstrap(bootstrap_cmd)) => match bootstrap_cmd {
            cli::BootstrapCommands::Init { work_dir, target, jobs } => {
                commands::cmd_bootstrap_init(&work_dir, &target, jobs)
            }

            cli::BootstrapCommands::Check { verbose } => {
                commands::cmd_bootstrap_check(verbose)
            }

            cli::BootstrapCommands::Stage0 { work_dir, config, jobs, verbose, download_only, clean } => {
                commands::cmd_bootstrap_stage0(&work_dir, config, jobs, verbose, download_only, clean)
            }

            cli::BootstrapCommands::Stage1 { work_dir, recipe_dir, jobs, verbose } => {
                commands::cmd_bootstrap_stage1(&work_dir, recipe_dir.as_deref(), jobs, verbose)
            }

            cli::BootstrapCommands::Base { work_dir, root, recipe_dir, verbose } => {
                commands::cmd_bootstrap_base(&work_dir, &root, recipe_dir.as_deref(), verbose)
            }

            cli::BootstrapCommands::Image { work_dir, output, format, size } => {
                commands::cmd_bootstrap_image(&work_dir, &output, &format, &size)
            }

            cli::BootstrapCommands::Status { work_dir, verbose } => {
                commands::cmd_bootstrap_status(&work_dir, verbose)
            }

            cli::BootstrapCommands::Resume { work_dir, verbose } => {
                commands::cmd_bootstrap_resume(&work_dir, verbose)
            }

            cli::BootstrapCommands::Clean { work_dir, stage, sources } => {
                commands::cmd_bootstrap_clean(&work_dir, stage, sources)
            }
        }

        Some(cli::Commands::Provenance(cmd)) => match cmd {
            cli::ProvenanceCommands::Show { package, db, section, recursive, format } => {
                commands::cmd_provenance_show(&db.db_path, &package, &section, recursive, &format)
            }
            cli::ProvenanceCommands::Verify { package, db, all_signatures } => {
                commands::cmd_provenance_verify(&db.db_path, &package, all_signatures)
            }
            cli::ProvenanceCommands::Diff { package1, package2, db, format } => {
                commands::cmd_provenance_diff(&db.db_path, &package1, &package2, &format)
            }
            cli::ProvenanceCommands::FindByDep { dep_name, version, dna, db } => {
                commands::cmd_provenance_find_by_dep(&db.db_path, &dep_name, version.as_deref(), dna.as_deref())
            }
            cli::ProvenanceCommands::Export { package, db, format, output, recursive } => {
                commands::cmd_provenance_export(&db.db_path, &package, &format, output.as_deref(), recursive)
            }
            cli::ProvenanceCommands::Register { package, db, key, keyless, dry_run } => {
                commands::cmd_provenance_register(
                    &db.db_path,
                    &package,
                    key.as_deref(),
                    keyless,
                    dry_run,
                )
            }
            cli::ProvenanceCommands::Audit { db, missing, include_converted } => {
                commands::cmd_provenance_audit(&db.db_path, missing.as_deref(), include_converted)
            }
        }

        // =====================================================================
        // Capability Commands
        // =====================================================================
        Some(cli::Commands::Capability(cmd)) => match cmd {
            cli::CapabilityCommands::Show { package, db, format } => {
                commands::cmd_capability_show(&db.db_path, &package, &format)
            }
            cli::CapabilityCommands::Validate { path, verbose } => {
                commands::cmd_capability_validate(&path, verbose)
            }
            cli::CapabilityCommands::List { db, missing, format } => {
                commands::cmd_capability_list(&db.db_path, missing, &format)
            }
            cli::CapabilityCommands::Generate { binary, args, output, timeout } => {
                commands::cmd_capability_generate(&binary, &args, output.as_deref(), timeout)
            }
            cli::CapabilityCommands::Audit { package, db, command, timeout } => {
                commands::cmd_capability_audit(&db.db_path, &package, command.as_deref(), timeout)
            }
            cli::CapabilityCommands::Run { package, command, db, permissive } => {
                commands::cmd_capability_run(&db.db_path, &package, &command, permissive)
            }
        }

        // =====================================================================
        // Federation Commands
        // =====================================================================
        Some(cli::Commands::Federation(cmd)) => match cmd {
            cli::FederationCommands::Status { db, verbose } => {
                commands::cmd_federation_status(&db.db_path, verbose)
            }
            cli::FederationCommands::Peers { db, tier, enabled_only } => {
                commands::cmd_federation_peers(&db.db_path, tier.as_deref(), enabled_only)
            }
            cli::FederationCommands::AddPeer { url, db, tier, name } => {
                commands::cmd_federation_add_peer(&url, &db.db_path, &tier, name.as_deref())
            }
            cli::FederationCommands::RemovePeer { peer, db } => {
                commands::cmd_federation_remove_peer(&peer, &db.db_path)
            }
            cli::FederationCommands::Stats { db, days } => {
                commands::cmd_federation_stats(&db.db_path, days)
            }
            cli::FederationCommands::EnablePeer { peer, db, enable } => {
                commands::cmd_federation_enable_peer(&peer, &db.db_path, enable)
            }
            cli::FederationCommands::Test { db, peer, timeout } => {
                commands::cmd_federation_test(&db.db_path, peer.as_deref(), timeout)
            }
            #[cfg(feature = "server")]
            cli::FederationCommands::Scan { db, duration, add } => {
                commands::cmd_federation_scan(&db.db_path, duration, add)
            }
        }

        // =====================================================================
        // Daemon Command
        // =====================================================================
        #[cfg(feature = "daemon")]
        Some(cli::Commands::Daemon { db, socket, tcp, foreground: _ }) => {
            use conary::daemon::{run_daemon, DaemonConfig};
            use std::path::PathBuf;

            let config = DaemonConfig {
                db_path: PathBuf::from(db.db_path),
                socket_path: PathBuf::from(socket),
                enable_tcp: tcp.is_some(),
                tcp_bind: tcp,
                ..Default::default()
            };

            // Run the async daemon
            tokio::runtime::Runtime::new()
                .expect("Failed to create Tokio runtime")
                .block_on(async {
                    run_daemon(config).await.map_err(|e| anyhow::anyhow!("{}", e))
                })
        }

        // =====================================================================
        // Remi Server Command
        // =====================================================================
        #[cfg(feature = "server")]
        Some(cli::Commands::Remi { config, bind, admin_bind, storage, init, validate }) => {
            use conary::server::{run_server_from_config, RemiConfig};
            use std::path::PathBuf;

            // Check if only --init was provided (before moving values)
            let only_init = init && bind.is_none() && admin_bind.is_none() && storage.is_none() && config.is_none();

            // Load or create configuration
            let mut remi_config = if let Some(config_path) = config {
                RemiConfig::load(&PathBuf::from(&config_path))?
            } else {
                // Look for default config locations
                let default_paths = [
                    PathBuf::from("/etc/conary/remi.toml"),
                    PathBuf::from("remi.toml"),
                ];

                let mut found_config = None;
                for path in &default_paths {
                    if path.exists() {
                        println!("Using config: {}", path.display());
                        found_config = Some(RemiConfig::load(path)?);
                        break;
                    }
                }
                found_config.unwrap_or_else(RemiConfig::new)
            };

            // Apply CLI overrides
            if let Some(bind_addr) = bind {
                remi_config.server.bind = bind_addr;
            }
            if let Some(admin_addr) = admin_bind {
                remi_config.server.admin_bind = admin_addr;
            }
            if let Some(storage_path) = storage {
                remi_config.storage.root = PathBuf::from(storage_path);
            }

            // Validate configuration
            if let Err(e) = remi_config.validate() {
                eprintln!("Configuration error: {}", e);
                std::process::exit(1);
            }

            if validate {
                println!("Configuration is valid.");
                println!("  Public API:   {}", remi_config.server.bind);
                println!("  Admin API:    {}", remi_config.server.admin_bind);
                println!("  Storage root: {}", remi_config.storage.root.display());
                return Ok(());
            }

            // Initialize directories if requested
            if init {
                println!("Initializing Remi storage directories...");
                for dir in remi_config.storage_dirs() {
                    if !dir.exists() {
                        println!("  Creating: {}", dir.display());
                        std::fs::create_dir_all(&dir)?;
                    }
                }
                println!("Storage directories initialized.");

                // If only init was requested, exit
                if only_init {
                    return Ok(());
                }
            }

            // Run the async server
            tokio::runtime::Runtime::new()
                .expect("Failed to create Tokio runtime")
                .block_on(run_server_from_config(&remi_config))
        }

        None => {
            println!("Conary Package Manager v{}", env!("CARGO_PKG_VERSION"));
            println!("Run 'conary --help' for usage information");
            Ok(())
        }
    }
}
