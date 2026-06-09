// apps/conary/src/dispatch/collection.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_collection_command(coll_cmd: cli::CollectionCommands) -> Result<()> {
    match coll_cmd {
        cli::CollectionCommands::Create {
            name,
            description,
            members,
            db,
        } => {
            commands::cmd_collection_create(&name, description.as_deref(), &members, &db.db_path)
                .await
        }

        cli::CollectionCommands::List { db } => commands::cmd_collection_list(&db.db_path).await,

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
    }
}
