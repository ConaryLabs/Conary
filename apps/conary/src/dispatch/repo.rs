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
            package_format,
            distribution,
            component,
            architecture,
            database,
            db,
            content_url,
            priority,
            disabled,
            debian_release_keys,
            rpm_metadata_keys,
            rpm_metalink,
            rpm_package_keys,
            arch_keyring,
            arch_keyring_format,
            arch_master_keys,
            arch_packager_key_threshold,
            arch_database_signature,
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
                package_format: package_format.map(Into::into),
                distribution,
                component,
                architecture,
                database,
                db_path: db.db_path,
                content_url,
                priority,
                disabled,
                debian_release_keys,
                rpm_metadata_keys,
                rpm_metalink,
                rpm_package_keys,
                arch_keyring,
                arch_keyring_format: arch_keyring_format.map(|format| match format {
                    cli::CliArchKeyringFormat::OpenPgp => {
                        conary_core::repository::ArchKeyringFormat::OpenPgp
                    }
                    cli::CliArchKeyringFormat::AlpmPackageZstd => {
                        conary_core::repository::ArchKeyringFormat::AlpmPackageZstd
                    }
                }),
                arch_master_keys,
                arch_packager_key_threshold,
                arch_database_signature: arch_database_signature.map(
                    |requirement| match requirement {
                        cli::CliArchDatabaseSignature::Required => {
                            conary_core::repository::ArchSignatureRequirement::Required
                        }
                        cli::CliArchDatabaseSignature::Optional => {
                            conary_core::repository::ArchSignatureRequirement::Optional
                        }
                    },
                ),
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
    }
}
