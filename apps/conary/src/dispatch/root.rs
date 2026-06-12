// apps/conary/src/dispatch/root.rs

use std::borrow::Cow;
use std::path::Path;

use anyhow::Result;

use super::automation::dispatch_automation_command;
use super::bootstrap::dispatch_bootstrap_command;
use super::cache::dispatch_cache_command;
use super::capability::dispatch_capability_command;
use super::catalog::{
    dispatch_canonical_command, dispatch_distro_command, dispatch_groups_command,
    dispatch_registry_command,
};
use super::ccs::dispatch_ccs_command;
use super::collection::dispatch_collection_command;
use super::config::dispatch_config_command;
use super::context::{legacy_replay_options, require_live_mutation};
use super::derivation::dispatch_derivation_command;
use super::derive::dispatch_derive_command;
use super::federation::dispatch_federation_command;
use super::model::dispatch_model_command;
use super::profile::dispatch_profile_command;
use super::provenance::dispatch_provenance_command;
use super::query::dispatch_query_command;
use super::repo::dispatch_repo_command;
use super::system::dispatch_system_command;
use super::trust::dispatch_trust_command;
use super::verify_derivation::dispatch_verify_derivation_command;
use crate::cli::{self, Commands};
use crate::commands;
use crate::live_host_safety::{LiveMutationClass, MutationIntent};

pub(super) async fn dispatch_command(
    command: Option<Commands>,
    allow_live_system_mutation: bool,
) -> Result<()> {
    match command {
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
            allow_legacy_replay,
            allow_foreign_legacy_replay,
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
            let legacy_replay =
                legacy_replay_options(allow_legacy_replay, allow_foreign_legacy_replay);

            // Smart dispatch: @name installs a collection
            if package.starts_with('@') {
                require_live_mutation(
                    MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                    Cow::Borrowed("conary install @collection"),
                    LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                    dry_run,
                )?;
                let name = package.trim_start_matches('@');
                commands::cmd_collection_install(
                    name,
                    &common.db.db_path,
                    &common.root,
                    dry_run,
                    skip_optional,
                    sandbox_mode,
                    no_scripts,
                    legacy_replay,
                )
                .await
            } else {
                require_live_mutation(
                    MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                    Cow::Borrowed("conary install"),
                    LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                    dry_run,
                )?;
                commands::cmd_install(
                    &package,
                    commands::InstallOptions {
                        db_path: &common.db.db_path,
                        root: &common.root,
                        version,
                        repo,
                        architecture: None,
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
                        repository_provenance: None,
                        legacy_replay,
                    },
                )
                .await
            }
        }

        Some(Commands::Remove {
            package_name,
            common,
            version,
            architecture,
            no_scripts,
            yes,
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            sandbox,
            purge_files,
        }) => {
            let legacy_replay =
                legacy_replay_options(allow_legacy_replay, allow_foreign_legacy_replay);
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary remove"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                false,
            )?;
            commands::cmd_remove(
                &package_name,
                &common.db.db_path,
                &common.root,
                version,
                architecture,
                no_scripts,
                sandbox.into(),
                purge_files,
                legacy_replay,
            )
            .await
        }

        Some(Commands::Update {
            package,
            common,
            version,
            architecture,
            security,
            dry_run,
            no_scripts,
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            sandbox,
            dep_mode,
            yes,
        }) => {
            let sandbox_mode = sandbox.into();
            let legacy_replay =
                legacy_replay_options(allow_legacy_replay, allow_foreign_legacy_replay);
            // Smart dispatch: @name updates a collection/group
            if let Some(ref pkg) = package
                && pkg.starts_with('@')
            {
                if version.is_some() || architecture.is_some() {
                    anyhow::bail!(
                        "Installed package selectors --version/--arch cannot be used with collection updates"
                    );
                }
                require_live_mutation(
                    MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                    Cow::Borrowed("conary update @collection"),
                    LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                    dry_run,
                )?;
                let name = pkg.trim_start_matches('@');
                return commands::cmd_update_group(
                    name,
                    &common.db.db_path,
                    &common.root,
                    security,
                    dry_run,
                    no_scripts,
                    sandbox_mode,
                    dep_mode,
                    yes,
                    legacy_replay,
                )
                .await;
            }
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary update"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::cmd_update(
                package,
                &common.db.db_path,
                &common.root,
                security,
                dry_run,
                no_scripts,
                sandbox_mode,
                dep_mode,
                yes,
                version,
                architecture,
                legacy_replay,
            )
            .await
        }

        Some(Commands::Search { pattern, db }) => commands::cmd_search(&pattern, &db.db_path).await,

        Some(Commands::List {
            pattern,
            version,
            architecture,
            db,
            path,
            info,
            files,
            lsl,
            pinned,
        }) => {
            if pinned {
                if version.is_some() || architecture.is_some() {
                    anyhow::bail!(
                        "Installed package selectors --version/--arch cannot be used with --pinned"
                    );
                }
                commands::cmd_list_pinned(&db.db_path).await
            } else {
                let options = commands::QueryOptions {
                    info,
                    lsl,
                    path,
                    files,
                    version,
                    architecture,
                };
                commands::cmd_query(pattern.as_deref(), &db.db_path, options).await
            }
        }

        Some(Commands::Autoremove {
            common,
            dry_run,
            no_scripts,
            yes,
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            sandbox,
        }) => {
            let legacy_replay =
                legacy_replay_options(allow_legacy_replay, allow_foreign_legacy_replay);
            require_live_mutation(
                MutationIntent::from_apply_intent(yes, allow_live_system_mutation),
                Cow::Borrowed("conary autoremove"),
                LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
                dry_run,
            )?;
            commands::cmd_autoremove(
                &common.db.db_path,
                &common.root,
                dry_run,
                no_scripts,
                sandbox.into(),
                legacy_replay,
            )
            .await
        }

        Some(Commands::Pin {
            package_name,
            version,
            architecture,
            db,
        }) => {
            let selector =
                commands::InstalledPackageSelector::new(package_name, version, architecture);
            commands::cmd_pin(selector, &db.db_path).await
        }

        Some(Commands::Unpin {
            package_name,
            version,
            architecture,
            db,
        }) => {
            let selector =
                commands::InstalledPackageSelector::new(package_name, version, architecture);
            commands::cmd_unpin(selector, &db.db_path).await
        }

        Some(Commands::Cook {
            target,
            recipe,
            output,
            source_cache,
            jobs,
            keep_builddir,
            validate_only,
            fetch_only,
            isolated,
            no_isolation,
            hermetic,
        }) => {
            commands::cmd_cook(
                target.as_deref(),
                recipe.as_deref(),
                &output,
                &source_cache,
                jobs,
                keep_builddir,
                validate_only,
                fetch_only,
                isolated,
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

        Some(Commands::Cache(cmd)) => dispatch_cache_command(cmd).await,

        // =====================================================================
        // System Commands
        // =====================================================================
        Some(Commands::System(sys_cmd)) => {
            dispatch_system_command(sys_cmd, allow_live_system_mutation).await
        }

        // =====================================================================
        // Repository Commands
        // =====================================================================
        Some(Commands::Repo(repo_cmd)) => dispatch_repo_command(repo_cmd).await,

        // =====================================================================
        // Config Commands
        // =====================================================================
        Some(Commands::Config(config_cmd)) => dispatch_config_command(config_cmd).await,

        // =====================================================================
        // Query Commands
        // =====================================================================
        Some(Commands::Query(query_cmd)) => dispatch_query_command(query_cmd).await,

        // =====================================================================
        // Collection Commands
        // =====================================================================
        Some(Commands::Collection(coll_cmd)) => dispatch_collection_command(coll_cmd).await,

        // =====================================================================
        // CCS Commands
        // =====================================================================
        Some(Commands::Ccs(ccs_cmd)) => {
            dispatch_ccs_command(ccs_cmd, allow_live_system_mutation).await
        }

        // =====================================================================
        // Derive Commands
        // =====================================================================
        Some(Commands::Derive(derive_cmd)) => dispatch_derive_command(derive_cmd).await,

        // =====================================================================
        // Model Commands
        // =====================================================================
        Some(Commands::Model(model_cmd)) => {
            dispatch_model_command(model_cmd, allow_live_system_mutation).await
        }

        // =====================================================================
        // Automation Commands
        // =====================================================================
        Some(Commands::Automation(auto_cmd)) => {
            dispatch_automation_command(auto_cmd, allow_live_system_mutation).await
        }

        // =====================================================================
        // Bootstrap Commands
        // =====================================================================
        Some(Commands::Bootstrap(bootstrap_cmd)) => dispatch_bootstrap_command(bootstrap_cmd).await,

        Some(cli::Commands::Provenance(cmd)) => dispatch_provenance_command(cmd).await,

        // =====================================================================
        // Capability Commands
        // =====================================================================
        Some(cli::Commands::Capability(cmd)) => dispatch_capability_command(cmd).await,

        // =====================================================================
        // Federation Commands
        // =====================================================================
        // =====================================================================
        // Trust Commands
        // =====================================================================
        Some(cli::Commands::Trust(cmd)) => dispatch_trust_command(cmd).await,

        Some(cli::Commands::Federation(cmd)) => dispatch_federation_command(cmd).await,

        // =====================================================================
        // Distro Commands
        // =====================================================================
        Some(Commands::Distro(distro_cmd)) => dispatch_distro_command(distro_cmd).await,

        // =====================================================================
        // Canonical Commands
        // =====================================================================
        Some(Commands::Canonical(can_cmd)) => dispatch_canonical_command(can_cmd).await,

        // =====================================================================
        // Groups Commands
        // =====================================================================
        Some(Commands::Groups(grp_cmd)) => dispatch_groups_command(grp_cmd).await,

        // =====================================================================
        // Registry Commands
        // =====================================================================
        Some(Commands::Registry(reg_cmd)) => dispatch_registry_command(reg_cmd).await,

        // =====================================================================
        // Export
        // =====================================================================
        Some(Commands::Export {
            generation,
            output,
            objects_dir,
        }) => commands::export_oci(generation, Path::new(&objects_dir), Path::new(&output)).await,

        // =====================================================================
        // Derivation Engine
        // =====================================================================
        Some(Commands::Derivation(derivation_cmd)) => {
            dispatch_derivation_command(derivation_cmd).await
        }

        Some(Commands::Profile(profile_cmd)) => dispatch_profile_command(profile_cmd).await,

        // =====================================================================
        // Self-Update
        // =====================================================================
        Some(Commands::SelfUpdate {
            db,
            check,
            force,
            version,
            no_verify,
            verify_sha256,
            verify_signature_file,
            trusted_keys,
            print_trusted_keys,
        }) => {
            commands::cmd_self_update(
                &db.db_path,
                commands::SelfUpdateOptions {
                    check,
                    force,
                    version,
                    no_verify,
                    verify_sha256,
                    verify_signature_file,
                    trusted_keys,
                    print_trusted_keys,
                },
            )
            .await
        }

        // =====================================================================
        // Derivation Verification
        // =====================================================================
        Some(Commands::VerifyDerivation(verify_cmd)) => {
            dispatch_verify_derivation_command(verify_cmd).await
        }

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
