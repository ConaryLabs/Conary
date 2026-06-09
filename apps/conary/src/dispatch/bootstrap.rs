// apps/conary/src/dispatch/bootstrap.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_bootstrap_command(
    bootstrap_cmd: cli::BootstrapCommands,
) -> Result<()> {
    match bootstrap_cmd {
        cli::BootstrapCommands::Init {
            work_dir,
            target,
            jobs,
        } => commands::cmd_bootstrap_init(&work_dir, &target, jobs).await,

        cli::BootstrapCommands::Check { verbose } => commands::cmd_bootstrap_check(verbose).await,

        cli::BootstrapCommands::Image {
            work_dir,
            output,
            format,
            size,
        } => commands::cmd_bootstrap_image(&work_dir, &output, &format, &size).await,

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

        cli::BootstrapCommands::GuestProfile {
            work_dir,
            public_key,
            verbose,
            lfs_root,
        } => {
            commands::cmd_bootstrap_guest_profile(
                &work_dir,
                &public_key,
                verbose,
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
            run_a,
            run_b,
            seed_a,
            seed_b,
            diff,
        } => {
            commands::cmd_bootstrap_verify_convergence(
                &run_a,
                &run_b,
                seed_a.as_deref(),
                seed_b.as_deref(),
                diff,
            )
            .await
        }

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
    }
}
