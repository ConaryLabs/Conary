// apps/conary/src/dispatch/query.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_query_command(query_cmd: cli::QueryCommands) -> Result<()> {
    match query_cmd {
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

        cli::QueryCommands::Scripts {
            package_path,
            db,
            version,
            architecture,
            verbose,
            entry,
            json,
        } => {
            commands::cmd_scripts_with_options(
                &package_path,
                commands::ScriptQueryOptions {
                    db_path: Some(db.db_path),
                    version,
                    architecture,
                    verbose,
                    entry,
                    json,
                },
            )
            .await
        }

        cli::QueryCommands::DeltaStats { db } => commands::cmd_delta_stats(&db.db_path).await,

        cli::QueryCommands::Conflicts { db, verbose } => {
            commands::cmd_conflicts(&db.db_path, verbose).await
        }

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
                commands::cmd_label_path(&db.db_path, add.as_deref(), remove.as_deref(), priority)
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
            } => commands::cmd_label_link(&label, repository.as_deref(), unlink, &db.db_path).await,

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
    }
}
