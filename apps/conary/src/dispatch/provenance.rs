// apps/conary/src/dispatch/provenance.rs

use anyhow::Result;

use crate::cli;
use crate::commands;

pub(super) async fn dispatch_provenance_command(cmd: cli::ProvenanceCommands) -> Result<()> {
    match cmd {
        cli::ProvenanceCommands::Show {
            package,
            db,
            section,
            recursive,
            format,
        } => {
            commands::cmd_provenance_show(&db.db_path, &package, &section, recursive, &format).await
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
            commands::cmd_provenance_audit(&db.db_path, missing.as_deref(), include_converted).await
        }
    }
}
