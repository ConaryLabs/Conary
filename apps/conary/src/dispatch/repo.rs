// apps/conary/src/dispatch/repo.rs

use anyhow::Result;

use crate::cli;
use crate::commands;
use conary_core::db::models::SecurityAdvisorySupport;

pub(super) async fn dispatch_repo_command(repo_cmd: cli::RepoCommands) -> Result<()> {
    match repo_cmd {
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
            fingerprints,
            yes,
            replace,
            default_strategy,
            remi_endpoint,
            remi_distro,
            security_advisories,
        } => {
            let security_advisory_support = match security_advisories {
                cli::CliSecurityAdvisorySupport::Unknown => SecurityAdvisorySupport::Unknown,
                cli::CliSecurityAdvisorySupport::Unsupported => {
                    SecurityAdvisorySupport::Unsupported
                }
                cli::CliSecurityAdvisorySupport::Supported => SecurityAdvisorySupport::Supported,
            };
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
                fingerprints,
                yes,
                replace,
                default_strategy,
                remi_endpoint,
                remi_distro,
                security_advisory_support,
            })
            .await
        }

        cli::RepoCommands::List { db, all } => commands::cmd_repo_list(&db.db_path, all).await,

        cli::RepoCommands::Remove { name, db } => {
            commands::cmd_repo_remove(&name, &db.db_path).await
        }

        cli::RepoCommands::ResetTrust { name, db } => {
            commands::cmd_repo_reset_trust(&name, &db.db_path).await
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
    }
}
