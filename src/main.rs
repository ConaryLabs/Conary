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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if let Err(err) = run().await {
        let msg = format!("{err:#}");
        if msg.contains("Database not found") {
            eprintln!("Error: Database not initialized.");
            eprintln!("Run 'conary system init' to set up the package database.");
            std::process::exit(1);
        }
        eprintln!("Error: {msg}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        // =====================================================================
        // Primary Commands (Hoisted to Root)
        // =====================================================================
        Some(Commands::Install {
            package,
            common,
            version,
            repo,
            dry_run,
            no_deps,
            no_scripts,
            sandbox,
            allow_downgrade,
            convert_to_ccs,
            no_capture,
            skip_optional,
            force,
            dep_mode,
            from,
            yes,
        }) => {
            let sandbox_mode = sandbox.into();

            // Smart dispatch: @name installs a collection
            if package.starts_with('@') {
                let name = package.trim_start_matches('@');
                commands::cmd_collection_install(
                    name,
                    &common.db.db_path,
                    &common.root,
                    dry_run,
                    skip_optional,
                    sandbox_mode,
                )
                .await
            } else {
                commands::cmd_install(
                    &package,
                    commands::InstallOptions {
                        db_path: &common.db.db_path,
                        root: &common.root,
                        version,
                        repo,
                        dry_run,
                        no_deps,
                        no_scripts,
                        selection_reason: None,
                        sandbox_mode,
                        allow_downgrade,
                        convert_to_ccs,
                        no_capture,
                        force,
                        dep_mode,
                        yes,
                        from_distro: from,
                    },
                )
                .await
            }
        }

        Some(Commands::Remove {
            package_name,
            common,
            version,
            no_scripts,
            sandbox,
            purge_files,
        }) => {
            commands::cmd_remove(
                &package_name,
                &common.db.db_path,
                &common.root,
                version,
                no_scripts,
                sandbox.into(),
                purge_files,
            )
            .await
        }

        Some(Commands::Update {
            package,
            common,
            security,
            sandbox,
            dep_mode,
            yes,
        }) => {
            let sandbox_mode = sandbox.into();
            // Smart dispatch: @name updates a collection/group
            if let Some(ref pkg) = package
                && pkg.starts_with('@')
            {
                let name = pkg.trim_start_matches('@');
                return commands::cmd_update_group(
                    name,
                    &common.db.db_path,
                    &common.root,
                    security,
                    sandbox_mode,
                    dep_mode,
                    yes,
                )
                .await;
            }
            commands::cmd_update(
                package,
                &common.db.db_path,
                &common.root,
                security,
                sandbox_mode,
                dep_mode,
                yes,
            )
            .await
        }

        Some(Commands::Search { pattern, db }) => commands::cmd_search(&pattern, &db.db_path).await,

        Some(Commands::List {
            pattern,
            db,
            path,
            info,
            files,
            lsl,
            pinned,
        }) => {
            if pinned {
                commands::cmd_list_pinned(&db.db_path).await
            } else {
                let options = commands::QueryOptions {
                    info,
                    lsl,
                    path,
                    files,
                };
                commands::cmd_query(pattern.as_deref(), &db.db_path, options).await
            }
        }

        Some(Commands::Autoremove {
            common,
            dry_run,
            no_scripts,
            sandbox,
        }) => {
            commands::cmd_autoremove(
                &common.db.db_path,
                &common.root,
                dry_run,
                no_scripts,
                sandbox.into(),
            )
            .await
        }

        Some(Commands::Pin { package_name, db }) => {
            commands::cmd_pin(&package_name, &db.db_path).await
        }

        Some(Commands::Unpin { package_name, db }) => {
            commands::cmd_unpin(&package_name, &db.db_path).await
        }

        Some(Commands::Cook {
            recipe,
            output,
            source_cache,
            jobs,
            keep_builddir,
            validate_only,
            fetch_only,
            no_isolation,
            hermetic,
        }) => {
            commands::cmd_cook(
                &recipe,
                &output,
                &source_cache,
                jobs,
                keep_builddir,
                validate_only,
                fetch_only,
                no_isolation,
                hermetic,
            )
            .await
        }

        Some(Commands::ConvertPkgbuild { pkgbuild, output }) => {
            commands::cmd_convert_pkgbuild(&pkgbuild, output.as_deref()).await
        }

        Some(Commands::RecipeAudit { recipe, all, trace }) => {
            commands::cmd_recipe_audit(recipe.as_deref(), all, trace).await
        }

        Some(Commands::Cache(cmd)) => match cmd {
            cli::CacheCommands::Populate {
                profile,
                sources_only,
                full,
                db,
            } => commands::cmd_cache_populate(&profile, sources_only, full, &db.db_path).await,
            cli::CacheCommands::Status { db } => commands::cmd_cache_status(&db.db_path).await,
        },

        // =====================================================================
        // System Commands
        // =====================================================================
        Some(Commands::System(sys_cmd)) => match sys_cmd {
            cli::SystemCommands::Init { db } => commands::cmd_init(&db.db_path).await,

            cli::SystemCommands::Completions { shell } => {
                let mut cmd = Cli::command();
                generate(shell, &mut cmd, "conary", &mut io::stdout());
                Ok(())
            }

            cli::SystemCommands::History { db } => commands::cmd_history(&db.db_path).await,

            cli::SystemCommands::Verify {
                package,
                common,
                rpm,
            } => commands::cmd_verify(package, &common.db.db_path, &common.root, rpm).await,

            cli::SystemCommands::Restore {
                package,
                common,
                force,
                dry_run,
            } => {
                if package == "all" {
                    commands::cmd_restore_all(&common.db.db_path, &common.root, dry_run).await
                } else {
                    commands::cmd_restore(
                        &package,
                        &common.db.db_path,
                        &common.root,
                        force,
                        dry_run,
                    )
                    .await
                }
            }

            cli::SystemCommands::Adopt {
                packages,
                db,
                full,
                system,
                status,
                dry_run,
                pattern,
                exclude,
                explicit_only,
                refresh,
                convert,
                jobs,
                no_chunking,
                sync_hook,
                remove_hook,
                quiet,
            } => {
                if sync_hook {
                    commands::cmd_sync_hook_install(remove_hook).await
                } else if convert {
                    commands::cmd_adopt_convert(&db.db_path, jobs, no_chunking, dry_run).await
                } else if status {
                    commands::cmd_adopt_status(&db.db_path).await
                } else if refresh {
                    commands::cmd_adopt_refresh(&db.db_path, full, dry_run, quiet).await
                } else if system {
                    commands::cmd_adopt_system(
                        &db.db_path,
                        full,
                        dry_run,
                        pattern.as_deref(),
                        exclude.as_deref(),
                        explicit_only,
                    )
                    .await
                } else {
                    commands::cmd_adopt(&packages, &db.db_path, full).await
                }
            }

            cli::SystemCommands::Gc {
                db,
                objects_dir,
                keep_days,
                dry_run,
                chunks,
            } => commands::cmd_gc(&db.db_path, &objects_dir, keep_days, dry_run, chunks).await,

            cli::SystemCommands::Sbom {
                package_name,
                db,
                format,
                output,
            } => commands::cmd_sbom(&package_name, &db.db_path, &format, output.as_deref()).await,

            #[cfg(feature = "server")]
            cli::SystemCommands::IndexGen {
                db,
                chunk_dir,
                output_dir,
                distro,
                sign_key,
            } => commands::cmd_index_gen(db.db_path, chunk_dir, output_dir, distro, sign_key),

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
            } => commands::cmd_prewarm(
                db.db_path,
                chunk_dir,
                cache_dir,
                distro,
                max_packages,
                popularity_file,
                pattern,
                dry_run,
            ),

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
                use anyhow::Context;
                use conary_server::server::{ServerConfig, run_server};
                use std::path::PathBuf;

                let config = ServerConfig {
                    bind_addr: bind.parse().context("Invalid bind address")?,
                    db_path: PathBuf::from(db.db_path),
                    chunk_dir: PathBuf::from(chunk_dir),
                    cache_dir: PathBuf::from(cache_dir),
                    max_concurrent_conversions: max_concurrent,
                    cache_max_bytes: max_cache_gb.saturating_mul(1024 * 1024 * 1024),
                    chunk_ttl_days,
                    // Use defaults for Phase 0 features
                    ..Default::default()
                };

                // Run the async server
                run_server(config).await
            }

            // Nested: system state
            cli::SystemCommands::State(state_cmd) => match state_cmd {
                cli::StateCommands::List { db, limit } => {
                    commands::cmd_state_list(&db.db_path, limit).await
                }

                cli::StateCommands::Show { state_number, db } => {
                    commands::cmd_state_show(&db.db_path, state_number).await
                }

                cli::StateCommands::Diff {
                    from_state,
                    to_state,
                    db,
                } => commands::cmd_state_diff(&db.db_path, from_state, to_state).await,

                cli::StateCommands::Revert {
                    state_number,
                    db,
                    dry_run,
                } => commands::cmd_state_restore(&db.db_path, state_number, dry_run).await,

                cli::StateCommands::Prune { keep, db, dry_run } => {
                    commands::cmd_state_prune(&db.db_path, keep, dry_run).await
                }

                cli::StateCommands::Create {
                    summary,
                    description,
                    db,
                } => {
                    commands::cmd_state_create(&db.db_path, &summary, description.as_deref()).await
                }

                cli::StateCommands::Rollback {
                    changeset_id,
                    common,
                } => commands::cmd_rollback(changeset_id, &common.db.db_path, &common.root).await,
            },

            // Nested: system generation
            cli::SystemCommands::Generation(gen_cmd) => match gen_cmd {
                cli::GenerationCommands::List => {
                    commands::generation::commands::cmd_generation_list().await
                }
                cli::GenerationCommands::Build { summary, db } => {
                    commands::generation::commands::cmd_generation_build(&db.db_path, &summary)
                }
                cli::GenerationCommands::Switch { number, reboot } => {
                    commands::generation::commands::cmd_generation_switch(number, reboot)
                }
                cli::GenerationCommands::Rollback => {
                    commands::generation::commands::cmd_generation_rollback()
                }
                cli::GenerationCommands::Gc { keep, db } => {
                    commands::generation::commands::cmd_generation_gc(keep, &db.db_path).await
                }
                cli::GenerationCommands::Info { number } => {
                    commands::generation::commands::cmd_generation_info(number).await
                }
                cli::GenerationCommands::Recover { db } => {
                    commands::generation::commands::cmd_generation_recover(&db.db_path)
                }
            },

            // System takeover
            cli::SystemCommands::Takeover {
                up_to,
                yes,
                dry_run,
                db,
            } => {
                commands::generation::takeover::cmd_system_takeover(
                    &db.db_path,
                    up_to,
                    yes,
                    dry_run,
                )
                .await
            }

            // Nested: system trigger
            cli::SystemCommands::Trigger(trigger_cmd) => match trigger_cmd {
                cli::TriggerCommands::List { db, all, builtin } => {
                    commands::cmd_trigger_list(&db.db_path, all, builtin).await
                }

                cli::TriggerCommands::Show { name, db } => {
                    commands::cmd_trigger_show(&name, &db.db_path).await
                }

                cli::TriggerCommands::Enable { name, db } => {
                    commands::cmd_trigger_enable(&name, &db.db_path).await
                }

                cli::TriggerCommands::Disable { name, db } => {
                    commands::cmd_trigger_disable(&name, &db.db_path).await
                }

                cli::TriggerCommands::Add {
                    name,
                    pattern,
                    handler,
                    description,
                    priority,
                    db,
                } => {
                    commands::cmd_trigger_add(
                        &name,
                        &pattern,
                        &handler,
                        description.as_deref(),
                        priority,
                        &db.db_path,
                    )
                    .await
                }

                cli::TriggerCommands::Remove { name, db } => {
                    commands::cmd_trigger_remove(&name, &db.db_path).await
                }

                cli::TriggerCommands::Run {
                    changeset_id,
                    db,
                    root,
                } => commands::cmd_trigger_run(changeset_id, &db.db_path, &root).await,
            },

            // Nested: system redirect
            cli::SystemCommands::Redirect(redirect_cmd) => match redirect_cmd {
                cli::RedirectCommands::List {
                    db,
                    r#type,
                    verbose,
                } => commands::cmd_redirect_list(&db.db_path, r#type.as_deref(), verbose).await,

                cli::RedirectCommands::Add {
                    source,
                    target,
                    db,
                    r#type,
                    source_version,
                    target_version,
                    message,
                } => {
                    commands::cmd_redirect_add(
                        &source,
                        &target,
                        &db.db_path,
                        &r#type,
                        source_version.as_deref(),
                        target_version.as_deref(),
                        message.as_deref(),
                    )
                    .await
                }

                cli::RedirectCommands::Show {
                    source,
                    db,
                    version,
                } => commands::cmd_redirect_show(&source, &db.db_path, version.as_deref()).await,

                cli::RedirectCommands::Remove { source, db } => {
                    commands::cmd_redirect_remove(&source, &db.db_path).await
                }

                cli::RedirectCommands::Resolve {
                    package,
                    db,
                    version,
                } => {
                    commands::cmd_redirect_resolve(&package, &db.db_path, version.as_deref()).await
                }
            },

            // Nested: system update-channel
            cli::SystemCommands::UpdateChannel { action } => match action {
                cli::UpdateChannelAction::Get { db } => {
                    commands::cmd_update_channel_get(&db.db_path).await
                }
                cli::UpdateChannelAction::Set { url, db } => {
                    commands::cmd_update_channel_set(&db.db_path, &url).await
                }
                cli::UpdateChannelAction::Reset { db } => {
                    commands::cmd_update_channel_reset(&db.db_path).await
                }
            },
        },

        // =====================================================================
        // Repository Commands
        // =====================================================================
        Some(Commands::Repo(repo_cmd)) => match repo_cmd {
            cli::RepoCommands::Add {
                name,
                url,
                db,
                content_url,
                priority,
                disabled,
                gpg_key,
                no_gpg_check,
                gpg_strict,
                default_strategy,
                remi_endpoint,
                remi_distro,
            } => {
                commands::cmd_repo_add(commands::RepoAddOptions {
                    name,
                    url,
                    db_path: db.db_path,
                    content_url,
                    priority,
                    disabled,
                    gpg_key,
                    no_gpg_check,
                    gpg_strict,
                    default_strategy,
                    remi_endpoint,
                    remi_distro,
                })
                .await
            }

            cli::RepoCommands::List { db, all } => commands::cmd_repo_list(&db.db_path, all).await,

            cli::RepoCommands::Remove { name, db } => {
                commands::cmd_repo_remove(&name, &db.db_path).await
            }

            cli::RepoCommands::Enable { name, db } => {
                commands::cmd_repo_enable(&name, &db.db_path).await
            }

            cli::RepoCommands::Disable { name, db } => {
                commands::cmd_repo_disable(&name, &db.db_path).await
            }

            cli::RepoCommands::Sync { name, db, force } => {
                commands::cmd_repo_sync(name, &db.db_path, force).await
            }

            cli::RepoCommands::KeyImport {
                repository,
                key,
                db,
            } => commands::cmd_key_import(&repository, &key, &db.db_path).await,

            cli::RepoCommands::KeyList { db } => commands::cmd_key_list(&db.db_path).await,

            cli::RepoCommands::KeyRemove { repository, db } => {
                commands::cmd_key_remove(&repository, &db.db_path).await
            }
        },

        // =====================================================================
        // Config Commands
        // =====================================================================
        Some(Commands::Config(config_cmd)) => match config_cmd {
            cli::ConfigCommands::List { package, db, all } => {
                commands::cmd_config_list(&db.db_path, package.as_deref(), all).await
            }

            cli::ConfigCommands::Diff { path, common } => {
                commands::cmd_config_diff(&common.db.db_path, &path, &common.root).await
            }

            cli::ConfigCommands::Backup { path, common } => {
                commands::cmd_config_backup(&common.db.db_path, &path, &common.root).await
            }

            cli::ConfigCommands::Restore {
                path,
                common,
                backup_id,
            } => {
                commands::cmd_config_restore(&common.db.db_path, &path, &common.root, backup_id)
                    .await
            }

            cli::ConfigCommands::Check { package, common } => {
                commands::cmd_config_check(&common.db.db_path, &common.root, package.as_deref())
                    .await
            }

            cli::ConfigCommands::Backups { path, db } => {
                commands::cmd_config_backups(&db.db_path, &path).await
            }
        },

        // =====================================================================
        // Query Commands
        // =====================================================================
        Some(Commands::Query(query_cmd)) => match query_cmd {
            cli::QueryCommands::Depends { package_name, db } => {
                commands::cmd_depends(&package_name, &db.db_path).await
            }

            cli::QueryCommands::Rdepends { package_name, db } => {
                commands::cmd_rdepends(&package_name, &db.db_path).await
            }

            cli::QueryCommands::Deptree {
                package_name,
                db,
                reverse,
                depth,
            } => commands::cmd_deptree(&package_name, &db.db_path, reverse, depth).await,

            cli::QueryCommands::Whatprovides { capability, db } => {
                commands::cmd_whatprovides(&capability, &db.db_path).await
            }

            cli::QueryCommands::Whatbreaks { package_name, db } => {
                commands::cmd_whatbreaks(&package_name, &db.db_path).await
            }

            cli::QueryCommands::Reason { pattern, db } => {
                commands::cmd_query_reason(pattern.as_deref(), &db.db_path).await
            }

            cli::QueryCommands::Repquery { pattern, db, info } => {
                commands::cmd_repquery(pattern.as_deref(), &db.db_path, info).await
            }

            cli::QueryCommands::Component { component_spec, db } => {
                commands::cmd_query_component(&component_spec, &db.db_path).await
            }

            cli::QueryCommands::Components { package_name, db } => {
                commands::cmd_list_components(&package_name, &db.db_path).await
            }

            cli::QueryCommands::Scripts { package_path } => {
                commands::cmd_scripts(&package_path).await
            }

            cli::QueryCommands::DeltaStats { db } => commands::cmd_delta_stats(&db.db_path).await,

            cli::QueryCommands::Conflicts { db, verbose } => {
                commands::cmd_conflicts(&db.db_path, verbose).await
            }

            // Nested: query label
            cli::QueryCommands::Label(label_cmd) => match label_cmd {
                cli::LabelCommands::List { db, verbose } => {
                    commands::cmd_label_list(&db.db_path, verbose).await
                }

                cli::LabelCommands::Add {
                    label,
                    description,
                    parent,
                    db,
                } => {
                    commands::cmd_label_add(
                        &label,
                        description.as_deref(),
                        parent.as_deref(),
                        &db.db_path,
                    )
                    .await
                }

                cli::LabelCommands::Remove { label, db, force } => {
                    commands::cmd_label_remove(&label, &db.db_path, force).await
                }

                cli::LabelCommands::Path {
                    db,
                    add,
                    remove,
                    priority,
                } => {
                    commands::cmd_label_path(
                        &db.db_path,
                        add.as_deref(),
                        remove.as_deref(),
                        priority,
                    )
                    .await
                }

                cli::LabelCommands::Show { package, db } => {
                    commands::cmd_label_show(&package, &db.db_path).await
                }

                cli::LabelCommands::Set { package, label, db } => {
                    commands::cmd_label_set(&package, &label, &db.db_path).await
                }

                cli::LabelCommands::Query { label, db } => {
                    commands::cmd_label_query(&label, &db.db_path).await
                }

                cli::LabelCommands::Link {
                    label,
                    repository,
                    unlink,
                    db,
                } => {
                    commands::cmd_label_link(&label, repository.as_deref(), unlink, &db.db_path)
                        .await
                }

                cli::LabelCommands::Delegate {
                    label,
                    target,
                    undelegate,
                    db,
                } => {
                    commands::cmd_label_delegate(&label, target.as_deref(), undelegate, &db.db_path)
                        .await
                }
            },
        },

        // =====================================================================
        // Collection Commands
        // =====================================================================
        Some(Commands::Collection(coll_cmd)) => match coll_cmd {
            cli::CollectionCommands::Create {
                name,
                description,
                members,
                db,
            } => {
                commands::cmd_collection_create(
                    &name,
                    description.as_deref(),
                    &members,
                    &db.db_path,
                )
                .await
            }

            cli::CollectionCommands::List { db } => {
                commands::cmd_collection_list(&db.db_path).await
            }

            cli::CollectionCommands::Show { name, db } => {
                commands::cmd_collection_show(&name, &db.db_path).await
            }

            cli::CollectionCommands::Add { name, members, db } => {
                commands::cmd_collection_add(&name, &members, &db.db_path).await
            }

            cli::CollectionCommands::Remove { name, members, db } => {
                commands::cmd_collection_remove_member(&name, &members, &db.db_path).await
            }

            cli::CollectionCommands::Delete { name, db } => {
                commands::cmd_collection_delete(&name, &db.db_path).await
            }
        },

        // =====================================================================
        // CCS Commands
        // =====================================================================
        Some(Commands::Ccs(ccs_cmd)) => match ccs_cmd {
            cli::CcsCommands::Init {
                path,
                name,
                version,
                force,
            } => commands::ccs::cmd_ccs_init(&path, name, &version, force).await,

            cli::CcsCommands::Build {
                path,
                output,
                target,
                source,
                no_classify,
                no_chunked,
                dry_run,
            } => {
                commands::ccs::cmd_ccs_build(
                    &path,
                    &output,
                    &target,
                    source,
                    no_classify,
                    !no_chunked,
                    dry_run,
                )
                .await
            }

            cli::CcsCommands::Inspect {
                package,
                files,
                hooks,
                deps,
                format,
            } => commands::ccs::cmd_ccs_inspect(&package, files, hooks, deps, &format).await,

            cli::CcsCommands::Verify {
                package,
                policy,
                allow_unsigned,
            } => commands::ccs::cmd_ccs_verify(&package, policy, allow_unsigned).await,

            cli::CcsCommands::Sign {
                package,
                key,
                output,
            } => commands::ccs::cmd_ccs_sign(&package, &key, output).await,

            cli::CcsCommands::Keygen {
                output,
                key_id,
                force,
            } => commands::ccs::cmd_ccs_keygen(&output, key_id, force).await,

            cli::CcsCommands::Install {
                package,
                common,
                dry_run,
                allow_unsigned,
                policy,
                components,
                sandbox,
                no_deps,
                reinstall,
                allow_capabilities,
                capability_policy,
            } => {
                commands::ccs::cmd_ccs_install(
                    &package,
                    &common.db.db_path,
                    &common.root,
                    dry_run,
                    allow_unsigned,
                    policy,
                    components,
                    sandbox.into(),
                    no_deps,
                    reinstall,
                    allow_capabilities,
                    capability_policy,
                )
                .await
            }

            cli::CcsCommands::Export {
                packages,
                output,
                format,
                db,
            } => commands::ccs::cmd_ccs_export(&packages, &output, &format, &db.db_path).await,

            cli::CcsCommands::Shell {
                packages,
                db,
                shell,
                env,
                keep,
            } => {
                commands::ccs::cmd_ccs_shell(&packages, &db.db_path, shell.as_deref(), &env, keep)
                    .await
            }

            cli::CcsCommands::Run {
                package,
                command,
                db,
                env,
            } => commands::ccs::cmd_ccs_run(&package, &command, &db.db_path, &env).await,

            cli::CcsCommands::Enhance {
                db,
                trove_id,
                all_pending,
                update_outdated,
                types,
                force,
                stats,
                dry_run,
                install_root,
            } => {
                commands::ccs::cmd_ccs_enhance(
                    &db.db_path,
                    trove_id,
                    all_pending,
                    update_outdated,
                    types,
                    force,
                    stats,
                    dry_run,
                    &install_root,
                )
                .await
            }
        },

        // =====================================================================
        // Derive Commands
        // =====================================================================
        Some(Commands::Derive(derive_cmd)) => match derive_cmd {
            cli::DeriveCommands::List { db, verbose } => {
                commands::cmd_derive_list(&db.db_path, verbose).await
            }

            cli::DeriveCommands::Show { name, db } => {
                commands::cmd_derive_show(&name, &db.db_path).await
            }

            cli::DeriveCommands::Create {
                name,
                from,
                version_suffix,
                description,
                db,
            } => {
                commands::cmd_derive_create(
                    &name,
                    &from,
                    version_suffix.as_deref(),
                    description.as_deref(),
                    &db.db_path,
                )
                .await
            }

            cli::DeriveCommands::Patch {
                name,
                patch_file,
                strip,
                db,
            } => commands::cmd_derive_patch(&name, &patch_file, strip, &db.db_path).await,

            cli::DeriveCommands::Override {
                name,
                target,
                source,
                mode,
                db,
            } => {
                commands::cmd_derive_override(&name, &target, source.as_deref(), mode, &db.db_path)
                    .await
            }

            cli::DeriveCommands::Build { name, db } => {
                commands::cmd_derive_build(&name, &db.db_path).await
            }

            cli::DeriveCommands::Delete { name, db } => {
                commands::cmd_derive_delete(&name, &db.db_path).await
            }

            cli::DeriveCommands::Stale { db } => commands::cmd_derive_stale(&db.db_path).await,
        },

        // =====================================================================
        // Model Commands
        // =====================================================================
        Some(Commands::Model(model_cmd)) => match model_cmd {
            cli::ModelCommands::Diff { model, offline, db } => {
                commands::cmd_model_diff(&model, &db.db_path, offline).await
            }

            cli::ModelCommands::Apply {
                model,
                common,
                dry_run,
                skip_optional,
                strict,
                no_autoremove,
                offline,
            } => {
                commands::cmd_model_apply(commands::ApplyOptions {
                    model_path: &model,
                    db_path: &common.db.db_path,
                    root: &common.root,
                    dry_run,
                    skip_optional,
                    strict,
                    autoremove: !no_autoremove,
                    offline,
                })
                .await
            }

            cli::ModelCommands::Check {
                model,
                db,
                verbose,
                offline,
            } => commands::cmd_model_check(&model, &db.db_path, verbose, offline).await,

            cli::ModelCommands::RemoteDiff { model, refresh, db } => {
                commands::cmd_model_remote_diff(&model, &db.db_path, refresh).await
            }

            cli::ModelCommands::Snapshot {
                output,
                db,
                description,
            } => commands::cmd_model_snapshot(&output, &db.db_path, description.as_deref()).await,

            cli::ModelCommands::Lock { model, output, db } => {
                commands::cmd_model_lock(&model, output.as_deref(), &db.db_path).await
            }

            cli::ModelCommands::Update { model, db } => {
                commands::cmd_model_update(&model, &db.db_path).await
            }

            cli::ModelCommands::Publish {
                model,
                name,
                version,
                repo,
                description,
                force,
                sign_key,
                db,
            } => {
                commands::cmd_model_publish(
                    &model,
                    &name,
                    &version,
                    &repo,
                    description.as_deref(),
                    &db.db_path,
                    force,
                    sign_key.as_deref(),
                )
                .await
            }
        },

        // =====================================================================
        // Automation Commands
        // =====================================================================
        Some(Commands::Automation(auto_cmd)) => match auto_cmd {
            cli::AutomationCommands::Status {
                db,
                format,
                verbose,
            } => commands::cmd_automation_status(&db.db_path, &format, verbose).await,

            cli::AutomationCommands::Check {
                common,
                categories,
                quiet,
            } => {
                commands::cmd_automation_check(&common.db.db_path, &common.root, categories, quiet)
                    .await
            }

            cli::AutomationCommands::Apply {
                common,
                yes,
                categories,
                dry_run,
                no_scripts,
            } => {
                commands::cmd_automation_apply(
                    &common.db.db_path,
                    &common.root,
                    yes,
                    categories,
                    dry_run,
                    no_scripts,
                )
                .await
            }

            cli::AutomationCommands::Configure {
                db,
                show,
                mode,
                enable,
                disable,
                interval,
                enable_ai,
                disable_ai,
            } => {
                commands::cmd_automation_configure(
                    &db.db_path,
                    show,
                    mode,
                    enable,
                    disable,
                    interval,
                    enable_ai,
                    disable_ai,
                )
                .await
            }

            cli::AutomationCommands::Daemon {
                common,
                foreground,
                pidfile,
            } => {
                commands::cmd_automation_daemon(
                    &common.db.db_path,
                    &common.root,
                    foreground,
                    &pidfile,
                )
                .await
            }

            cli::AutomationCommands::History {
                db,
                limit,
                category,
                status,
                since,
            } => {
                commands::cmd_automation_history(&db.db_path, limit, category, status, since).await
            }

            #[cfg(feature = "experimental")]
            cli::AutomationCommands::Ai(ai_cmd) => match ai_cmd {
                cli::AiCommands::Find {
                    intent,
                    db,
                    limit,
                    verbose,
                } => commands::cmd_ai_find(&db.db_path, &intent, limit, verbose).await,

                cli::AiCommands::Translate {
                    source,
                    format,
                    confidence,
                } => commands::cmd_ai_translate(&source, &format, confidence).await,

                cli::AiCommands::Query { question, db } => {
                    commands::cmd_ai_query(&db.db_path, &question).await
                }

                cli::AiCommands::Explain { command, db } => {
                    commands::cmd_ai_explain(&db.db_path, &command).await
                }
            },
        },

        // =====================================================================
        // Bootstrap Commands
        // =====================================================================
        Some(Commands::Bootstrap(bootstrap_cmd)) => match bootstrap_cmd {
            cli::BootstrapCommands::Init {
                work_dir,
                target,
                jobs,
            } => commands::cmd_bootstrap_init(&work_dir, &target, jobs).await,

            cli::BootstrapCommands::Check { verbose } => {
                commands::cmd_bootstrap_check(verbose).await
            }

            cli::BootstrapCommands::Image {
                work_dir,
                output,
                format,
                size,
                from_generation,
            } => {
                commands::cmd_bootstrap_image(
                    &work_dir,
                    &output,
                    &format,
                    &size,
                    from_generation.as_deref(),
                )
                .await
            }

            cli::BootstrapCommands::Status { work_dir, verbose } => {
                commands::cmd_bootstrap_status(&work_dir, verbose).await
            }

            cli::BootstrapCommands::Resume { work_dir, verbose } => {
                commands::cmd_bootstrap_resume(&work_dir, verbose).await
            }

            cli::BootstrapCommands::DryRun {
                work_dir,
                recipe_dir,
                verbose,
            } => commands::cmd_bootstrap_dry_run(&work_dir, &recipe_dir, verbose).await,

            cli::BootstrapCommands::Clean {
                work_dir,
                stage,
                sources,
            } => commands::cmd_bootstrap_clean(&work_dir, stage, sources).await,

            cli::BootstrapCommands::CrossTools {
                work_dir,
                lfs_root,
                jobs,
                verbose,
                skip_verify,
            } => {
                commands::cmd_bootstrap_cross_tools(
                    &work_dir,
                    jobs,
                    verbose,
                    skip_verify,
                    lfs_root.as_deref(),
                )
                .await
            }

            cli::BootstrapCommands::TempTools {
                work_dir,
                lfs_root,
                jobs,
                verbose,
                skip_verify,
            } => {
                commands::cmd_bootstrap_temp_tools(
                    &work_dir,
                    jobs,
                    verbose,
                    skip_verify,
                    lfs_root.as_deref(),
                )
                .await
            }

            cli::BootstrapCommands::System {
                work_dir,
                lfs_root,
                jobs,
                verbose,
                skip_verify,
            } => {
                commands::cmd_bootstrap_system(
                    &work_dir,
                    jobs,
                    verbose,
                    skip_verify,
                    lfs_root.as_deref(),
                )
                .await
            }

            cli::BootstrapCommands::Config {
                work_dir,
                lfs_root,
                verbose,
            } => commands::cmd_bootstrap_config(&work_dir, verbose, lfs_root.as_deref()).await,

            cli::BootstrapCommands::Tier2 {
                work_dir,
                lfs_root,
                jobs,
                verbose,
                skip_verify,
            } => {
                commands::cmd_bootstrap_tier2(
                    &work_dir,
                    jobs,
                    verbose,
                    skip_verify,
                    lfs_root.as_deref(),
                )
                .await
            }

            cli::BootstrapCommands::Seed {
                from,
                from_adopted,
                distro,
                distro_version,
                output,
                target,
            } => {
                if from_adopted {
                    commands::cmd_bootstrap_seed_adopted(
                        &output,
                        distro.as_deref(),
                        distro_version.as_deref(),
                    )
                    .await?;
                } else {
                    let from_path = from.ok_or_else(|| {
                        anyhow::anyhow!("--from is required when not using --from-adopted")
                    })?;
                    commands::cmd_bootstrap_seed(&from_path, &output, &target).await?;
                }
                Ok(())
            }

            cli::BootstrapCommands::VerifyConvergence {
                seed_a,
                seed_b,
                diff,
            } => commands::cmd_bootstrap_verify_convergence(&seed_a, &seed_b, diff).await,

            cli::BootstrapCommands::DiffSeeds { path_a, path_b } => {
                commands::cmd_bootstrap_diff_seeds(&path_a, &path_b).await
            }

            cli::BootstrapCommands::Run {
                manifest,
                work_dir,
                seed,
                recipe_dir,
                up_to,
                only,
                cascade,
                keep_logs,
                shell_on_failure,
                verbose,
                no_substituters,
                publish,
            } => {
                commands::cmd_bootstrap_run(commands::BootstrapRunOptions {
                    manifest: &manifest,
                    work_dir: &work_dir,
                    seed: &seed,
                    recipe_dir: &recipe_dir,
                    up_to: up_to.as_deref(),
                    only: only.as_deref(),
                    cascade,
                    keep_logs,
                    shell_on_failure,
                    verbose,
                    no_substituters,
                    publish,
                })
                .await
            }
        },

        Some(cli::Commands::Provenance(cmd)) => match cmd {
            cli::ProvenanceCommands::Show {
                package,
                db,
                section,
                recursive,
                format,
            } => {
                commands::cmd_provenance_show(&db.db_path, &package, &section, recursive, &format)
                    .await
            }
            cli::ProvenanceCommands::Verify {
                package,
                db,
                all_signatures,
            } => commands::cmd_provenance_verify(&db.db_path, &package, all_signatures).await,
            cli::ProvenanceCommands::Diff {
                package1,
                package2,
                db,
                format,
            } => commands::cmd_provenance_diff(&db.db_path, &package1, &package2, &format).await,
            cli::ProvenanceCommands::FindByDep {
                dep_name,
                version,
                dna,
                db,
            } => {
                commands::cmd_provenance_find_by_dep(
                    &db.db_path,
                    &dep_name,
                    version.as_deref(),
                    dna.as_deref(),
                )
                .await
            }
            cli::ProvenanceCommands::Export {
                package,
                db,
                format,
                output,
                recursive,
            } => {
                commands::cmd_provenance_export(
                    &db.db_path,
                    &package,
                    &format,
                    output.as_deref(),
                    recursive,
                )
                .await
            }
            cli::ProvenanceCommands::Register {
                package,
                db,
                key,
                keyless,
                dry_run,
            } => {
                commands::cmd_provenance_register(
                    &db.db_path,
                    &package,
                    key.as_deref(),
                    keyless,
                    dry_run,
                )
                .await
            }
            cli::ProvenanceCommands::Audit {
                db,
                missing,
                include_converted,
            } => {
                commands::cmd_provenance_audit(&db.db_path, missing.as_deref(), include_converted)
                    .await
            }
        },

        // =====================================================================
        // Capability Commands
        // =====================================================================
        Some(cli::Commands::Capability(cmd)) => match cmd {
            cli::CapabilityCommands::Show {
                package,
                db,
                format,
            } => commands::cmd_capability_show(&db.db_path, &package, &format).await,
            cli::CapabilityCommands::Validate { path, verbose } => {
                commands::cmd_capability_validate(&path, verbose).await
            }
            cli::CapabilityCommands::List {
                db,
                missing,
                format,
            } => commands::cmd_capability_list(&db.db_path, missing, &format).await,
            cli::CapabilityCommands::Generate {
                binary,
                args,
                output,
                timeout,
            } => {
                commands::cmd_capability_generate(&binary, &args, output.as_deref(), timeout).await
            }
            cli::CapabilityCommands::Audit {
                package,
                db,
                command,
                timeout,
            } => {
                commands::cmd_capability_audit(&db.db_path, &package, command.as_deref(), timeout)
                    .await
            }
            cli::CapabilityCommands::Run {
                package,
                command,
                db,
                permissive,
            } => commands::cmd_capability_run(&db.db_path, &package, &command, permissive).await,
        },

        // =====================================================================
        // Federation Commands
        // =====================================================================
        // =====================================================================
        // Trust Commands
        // =====================================================================
        Some(cli::Commands::Trust(cmd)) => match cmd {
            cli::TrustCommands::KeyGen { role, output } => {
                commands::cmd_trust_key_gen(&role, &output).await
            }
            cli::TrustCommands::Init { repo, root, db } => {
                commands::cmd_trust_init(&repo, &root, &db.db_path).await
            }
            cli::TrustCommands::Enable { repo, tuf_url, db } => {
                commands::cmd_trust_enable(&repo, tuf_url.as_deref(), &db.db_path).await
            }
            cli::TrustCommands::Disable { repo, force, db } => {
                commands::cmd_trust_disable(&repo, force, &db.db_path).await
            }
            cli::TrustCommands::Status { repo, db } => {
                commands::cmd_trust_status(&repo, &db.db_path).await
            }
            cli::TrustCommands::Verify { repo, db } => {
                commands::cmd_trust_verify(&repo, &db.db_path).await
            }
            #[cfg(feature = "server")]
            cli::TrustCommands::SignTargets { repo, key, db } => {
                commands::cmd_trust_sign_targets(&repo, &key, &db.db_path).await
            }
            #[cfg(feature = "server")]
            cli::TrustCommands::RotateKey {
                role,
                old_key,
                new_key,
                root_key,
                repo,
                db,
            } => {
                commands::cmd_trust_rotate_key(
                    &role,
                    &old_key,
                    &new_key,
                    &root_key,
                    &repo,
                    &db.db_path,
                )
                .await
            }
        },

        Some(cli::Commands::Federation(cmd)) => match cmd {
            cli::FederationCommands::Status { db, verbose } => {
                commands::cmd_federation_status(&db.db_path, verbose).await
            }
            cli::FederationCommands::Peers {
                db,
                tier,
                enabled_only,
            } => commands::cmd_federation_peers(&db.db_path, tier.as_deref(), enabled_only).await,
            cli::FederationCommands::AddPeer {
                url,
                db,
                tier,
                name,
            } => commands::cmd_federation_add_peer(&url, &db.db_path, &tier, name.as_deref()).await,
            cli::FederationCommands::RemovePeer { peer, db } => {
                commands::cmd_federation_remove_peer(&peer, &db.db_path).await
            }
            cli::FederationCommands::Stats { db, days } => {
                commands::cmd_federation_stats(&db.db_path, days).await
            }
            cli::FederationCommands::EnablePeer { peer, db, enable } => {
                commands::cmd_federation_enable_peer(&peer, &db.db_path, enable).await
            }
            cli::FederationCommands::Test { db, peer, timeout } => {
                commands::cmd_federation_test(&db.db_path, peer.as_deref(), timeout).await
            }
            #[cfg(feature = "server")]
            cli::FederationCommands::Scan { db, duration, add } => {
                commands::cmd_federation_scan(&db.db_path, duration, add).await
            }
        },

        // =====================================================================
        // Daemon Command
        // =====================================================================
        #[cfg(feature = "server")]
        Some(cli::Commands::Daemon { db, socket, tcp }) => {
            use conary_server::daemon::{DaemonConfig, run_daemon};
            use std::path::PathBuf;

            let config = DaemonConfig {
                db_path: PathBuf::from(db.db_path),
                socket_path: PathBuf::from(socket),
                enable_tcp: tcp.is_some(),
                tcp_bind: tcp,
                ..Default::default()
            };

            // Run the async daemon
            run_daemon(config)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
        }

        // =====================================================================
        // Remi Lite Proxy Command
        // =====================================================================
        #[cfg(feature = "server")]
        Some(cli::Commands::RemiProxy {
            port,
            upstream,
            no_mdns,
            cache_dir,
            offline,
            no_advertise,
        }) => {
            use conary_server::server::{ProxyConfig, run_proxy};
            use std::path::PathBuf;

            let config = ProxyConfig {
                port,
                upstream_url: upstream,
                cache_dir: PathBuf::from(cache_dir),
                mdns_enabled: !no_mdns,
                mdns_scan_secs: 3,
                offline,
                advertise: !no_advertise,
            };

            // Ensure cache directory exists
            if let Some(parent) = config.cache_dir.parent()
                && !parent.exists()
            {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::create_dir_all(&config.cache_dir)?;

            run_proxy(config).await
        }

        // =====================================================================
        // Remi Server Command
        // =====================================================================
        #[cfg(feature = "server")]
        Some(cli::Commands::Remi {
            config,
            bind,
            admin_bind,
            storage,
            init,
            validate,
        }) => {
            use conary_server::server::{RemiConfig, run_server_from_config};
            use std::path::PathBuf;

            // Check if only --init was provided (before moving values)
            let only_init = init
                && bind.is_none()
                && admin_bind.is_none()
                && storage.is_none()
                && config.is_none();

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
            remi_config
                .validate()
                .map_err(|e| anyhow::anyhow!("Configuration error: {}", e))?;

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
            run_server_from_config(&remi_config).await
        }

        // =====================================================================
        // Distro Commands
        // =====================================================================
        Some(Commands::Distro(distro_cmd)) => match distro_cmd {
            cli::DistroCommands::Set { distro, mixing, db } => {
                commands::distro::cmd_distro_set(&db.db_path, &distro, &mixing).await
            }
            cli::DistroCommands::Remove { db } => {
                commands::distro::cmd_distro_remove(&db.db_path).await
            }
            cli::DistroCommands::List => commands::distro::cmd_distro_list().await,
            cli::DistroCommands::Info { db } => {
                commands::distro::cmd_distro_info(&db.db_path).await
            }
            cli::DistroCommands::Mixing { policy, db } => {
                commands::distro::cmd_distro_mixing(&db.db_path, &policy).await
            }
        },

        // =====================================================================
        // Canonical Commands
        // =====================================================================
        Some(Commands::Canonical(can_cmd)) => match can_cmd {
            cli::CanonicalCommands::Show { name, db } => {
                commands::canonical::cmd_canonical_show(&db.db_path, &name).await
            }
            cli::CanonicalCommands::Search { query, db } => {
                commands::canonical::cmd_canonical_search(&db.db_path, &query).await
            }
            cli::CanonicalCommands::Unmapped { db } => {
                commands::canonical::cmd_canonical_unmapped(&db.db_path).await
            }
        },

        // =====================================================================
        // Groups Commands
        // =====================================================================
        Some(Commands::Groups(grp_cmd)) => match grp_cmd {
            cli::GroupsCommands::List { db } => {
                commands::groups::cmd_groups_list(&db.db_path).await
            }
            cli::GroupsCommands::Show { name, distro, db } => {
                commands::groups::cmd_groups_show(&db.db_path, &name, distro.as_deref()).await
            }
        },

        // =====================================================================
        // Registry Commands
        // =====================================================================
        Some(Commands::Registry(reg_cmd)) => match reg_cmd {
            cli::RegistryCommands::Update { db } => {
                commands::registry::cmd_registry_update(&db.db_path).await
            }
            cli::RegistryCommands::Stats { db } => {
                commands::registry::cmd_registry_stats(&db.db_path).await
            }
        },

        // =====================================================================
        // Export
        // =====================================================================
        Some(Commands::Export {
            generation,
            output,
            objects_dir,
            db,
        }) => {
            commands::export_oci(
                generation,
                std::path::Path::new(&objects_dir),
                std::path::Path::new(&output),
                &db,
            )
            .await
        }

        // =====================================================================
        // Derivation Engine
        // =====================================================================
        Some(Commands::Derivation(derivation_cmd)) => match derivation_cmd {
            cli::DerivationCommands::Build {
                recipe,
                env,
                cas_dir,
                db_path,
            } => commands::cmd_derivation_build(&recipe, &env, &cas_dir, db_path.as_deref()).await,
            cli::DerivationCommands::Show { recipe, env_hash } => {
                commands::cmd_derivation_show(&recipe, &env_hash).await
            }
        },

        Some(Commands::Profile(profile_cmd)) => match profile_cmd {
            cli::ProfileCommands::Generate { manifest, output } => {
                commands::cmd_profile_generate(&manifest, output.as_deref()).await
            }
            cli::ProfileCommands::Show { path } => commands::cmd_profile_show(&path).await,
            cli::ProfileCommands::Diff { old, new } => commands::cmd_profile_diff(&old, &new).await,
            cli::ProfileCommands::Publish {
                profile,
                endpoint,
                token,
            } => {
                commands::cmd_profile_publish(&profile, endpoint.as_deref(), token.as_deref()).await
            }
        },

        // =====================================================================
        // Self-Update
        // =====================================================================
        Some(Commands::SelfUpdate {
            db,
            check,
            force,
            version,
        }) => commands::cmd_self_update(&db.db_path, check, force, version).await,

        // =====================================================================
        // Derivation Verification
        // =====================================================================
        Some(Commands::VerifyDerivation(verify_cmd)) => match verify_cmd {
            cli::VerifyCommands::Chain {
                profile,
                verbose,
                json,
                db,
            } => commands::verify::cmd_verify_chain(&profile, verbose, json, &db.db_path).await,
            cli::VerifyCommands::Rebuild {
                derivation,
                work_dir,
                db,
            } => commands::verify::cmd_verify_rebuild(&derivation, &work_dir, &db.db_path).await,
            cli::VerifyCommands::Diverse {
                profile_a,
                profile_b,
                db,
            } => commands::verify::cmd_verify_diverse(&profile_a, &profile_b, &db.db_path).await,
        },

        Some(Commands::Sbom {
            profile,
            derivation,
            output,
            db,
        }) => {
            commands::cmd_derivation_sbom(
                profile.as_deref(),
                derivation.as_deref(),
                output.as_deref(),
                &db.db_path,
            )
            .await
        }

        None => {
            println!("Conary Package Manager v{}", env!("CARGO_PKG_VERSION"));
            println!("Run 'conary --help' for usage information");
            Ok(())
        }
    }
}
