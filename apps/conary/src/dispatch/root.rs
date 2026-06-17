// apps/conary/src/dispatch/root.rs

use std::borrow::Cow;
use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result, bail};

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
use crate::command_risk::{self, CommandRisk};
use crate::commands;
use crate::commands::try_session::{
    activated_try_session_is_live, current_boot_id, namespace_try_session_is_decision_pending,
};
use crate::live_host_safety::{LiveMutationClass, MutationIntent};
use conary_core::db::models::{TrySession, TrySessionMode};
use conary_core::runtime_root::ConaryRuntimeRoot;

const DEFAULT_DB_PATH: &str = "/var/lib/conary/conary.db";

#[derive(Debug)]
struct TryWatchDispatch {
    target: String,
    recipe: Option<String>,
    isolated: bool,
    json: bool,
}

#[derive(Debug)]
enum TryDispatchAction {
    Package(String),
    Watch(TryWatchDispatch),
    Status,
    Rollback,
    Keep,
}

struct TryDispatchInput<'a> {
    target: Option<String>,
    activate: bool,
    allow_irreversible: bool,
    isolated: bool,
    run: &'a [String],
    watch: bool,
    recipe: Option<String>,
    json: bool,
}

fn try_dispatch_action(input: TryDispatchInput<'_>) -> Result<TryDispatchAction> {
    if input.watch {
        if input.activate {
            bail!("conary try --watch cannot be combined with --activate");
        }
        if input.allow_irreversible {
            bail!("conary try --watch cannot be combined with --allow-irreversible");
        }
        if !input.run.is_empty() {
            bail!("conary try --watch cannot run a command");
        }
        let target = input.target.unwrap_or_else(|| ".".to_string());
        if is_reserved_try_action(&target) {
            bail!("conary try --watch cannot be combined with try action '{target}'");
        }
        if target.ends_with(".ccs") {
            bail!("conary try --watch does not accept prebuilt .ccs artifacts");
        }
        return Ok(TryDispatchAction::Watch(TryWatchDispatch {
            target,
            recipe: input.recipe,
            isolated: input.isolated,
            json: input.json,
        }));
    }

    if input.isolated {
        bail!("conary try --isolated requires --watch");
    }

    match input.target {
        Some(target)
            if is_reserved_try_action(&target)
                && !input.activate
                && !input.allow_irreversible
                && input.run.is_empty() =>
        {
            Ok(match target.as_str() {
                "status" => TryDispatchAction::Status,
                "rollback" => TryDispatchAction::Rollback,
                "keep" => TryDispatchAction::Keep,
                _ => unreachable!("reserved try action checked above"),
            })
        }
        Some(target) => Ok(TryDispatchAction::Package(target)),
        None => bail!("conary try requires a package artifact or one of: status, rollback, keep"),
    }
}

fn is_reserved_try_action(target: &str) -> bool {
    matches!(target, "status" | "rollback" | "keep")
}

fn is_try_management_action(command: &Commands) -> bool {
    matches!(
        command,
        Commands::Try {
            target: Some(target),
            activate: false,
            allow_irreversible: false,
            run,
            ..
        } if run.is_empty() && is_reserved_try_action(target)
    )
}

fn command_uses_try_session_preflight_db(command: &Commands) -> bool {
    match command {
        Commands::Cook { .. }
        | Commands::New { .. }
        | Commands::Publish { .. }
        | Commands::Mcp(_)
        | Commands::Bootstrap(
            cli::BootstrapCommands::VerifyConvergence { .. }
            | cli::BootstrapCommands::DiffSeeds { .. },
        )
        | Commands::System(cli::SystemCommands::Completions { .. })
        | Commands::Ccs(
            cli::CcsCommands::Init { .. }
            | cli::CcsCommands::Build { .. }
            | cli::CcsCommands::Inspect { .. }
            | cli::CcsCommands::Verify { .. }
            | cli::CcsCommands::Sign { .. }
            | cli::CcsCommands::Keygen { .. },
        )
        | Commands::Capability(
            cli::CapabilityCommands::Validate { .. } | cli::CapabilityCommands::Generate { .. },
        )
        | Commands::Trust(cli::TrustCommands::KeyGen { .. }) => false,
        Commands::Query(cli::QueryCommands::Scripts { package_path, .. }) => {
            !query_scripts_target_uses_package_file(package_path)
        }
        _ => true,
    }
}

fn query_scripts_target_uses_package_file(package_path: &str) -> bool {
    let lower = package_path.to_ascii_lowercase();
    Path::new(package_path).exists()
        || lower.ends_with(".ccs")
        || lower.ends_with(".rpm")
        || lower.ends_with(".deb")
        || lower.contains(".pkg.tar")
}

pub(super) fn run_try_session_preflight(cli: &crate::cli::Cli) -> Result<()> {
    run_try_session_preflight_inner(cli, std::io::stdin().is_terminal())
}

#[cfg(test)]
fn run_try_session_preflight_for_test(cli: &crate::cli::Cli, interactive: bool) -> Result<()> {
    run_try_session_preflight_inner(cli, interactive)
}

fn run_try_session_preflight_inner(cli: &crate::cli::Cli, interactive: bool) -> Result<()> {
    let Some(command) = cli.command.as_ref() else {
        return Ok(());
    };
    if is_try_management_action(command) {
        return Ok(());
    }
    if !command_uses_try_session_preflight_db(command) {
        return Ok(());
    }

    let db_path = selected_db_path(command);
    let live_conn = match conary_core::db::open(db_path) {
        Ok(conn) => conn,
        Err(conary_core::Error::DatabaseNotFound(_)) => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to open Conary DB {db_path}"));
        }
    };
    let Some(session) = TrySession::find_active_or_orphaned(&live_conn)? else {
        return Ok(());
    };

    let policy = command_risk::classify_cli(cli);
    let allows_live_try_session = policy.as_ref().is_some_and(|policy| {
        policy.dry_run || matches!(policy.risk, CommandRisk::ReadOnly | CommandRisk::DryRunOnly)
    });
    let current_boot_id = current_boot_id();
    let interactive = interactive && !env_forces_non_interactive();

    match session.mode {
        TrySessionMode::Namespace => {
            if namespace_try_session_is_decision_pending(&session, &current_boot_id) {
                if allows_live_try_session {
                    return Ok(());
                }
                bail!(
                    "another try session is active ({}); run `conary try status`, `conary try rollback`, or `conary try keep` before mutating Conary state",
                    session.id
                );
            }
            session.mark_orphaned(&live_conn)?;
            bail!(
                "orphaned try session {} requires cleanup; run `conary try status`, `conary try rollback`, or `conary try keep`",
                session.id
            );
        }
        TrySessionMode::Activated => {
            let runtime_root = ConaryRuntimeRoot::from_db_path(db_path);
            let current_generation =
                conary_core::generation::mount::current_generation(runtime_root.root())?;
            if activated_try_session_is_live(&session, &current_boot_id, current_generation) {
                if allows_live_try_session {
                    return Ok(());
                }
                bail!(
                    "activated try session {} is active; run `conary try keep` or `conary try rollback` before mutating Conary state",
                    session.id
                );
            }

            session.mark_orphaned(&live_conn)?;
            drop(live_conn);

            if interactive {
                bail!(
                    "orphaned activated try session {} requires a decision; run `conary try keep` or `conary try rollback`",
                    session.id
                );
            }

            commands::rollback_active_try_session(db_path)
                .context("automatic rollback of orphaned activated try session failed")?;
            Ok(())
        }
    }
}

fn selected_db_path(command: &Commands) -> &str {
    match command {
        Commands::Install { common, .. }
        | Commands::Remove { common, .. }
        | Commands::Update { common, .. }
        | Commands::Autoremove { common, .. } => &common.db.db_path,
        Commands::Search { db, .. }
        | Commands::List { db, .. }
        | Commands::Pin { db, .. }
        | Commands::Unpin { db, .. }
        | Commands::Try { db, .. }
        | Commands::SelfUpdate { db, .. }
        | Commands::Sbom { db, .. } => &db.db_path,
        Commands::Repo(command) => selected_repo_db_path(command),
        Commands::Config(command) => selected_config_db_path(command),
        Commands::Distro(command) => selected_distro_db_path(command),
        Commands::Canonical(command) => selected_canonical_db_path(command),
        Commands::Groups(command) => selected_groups_db_path(command),
        Commands::Registry(command) => selected_registry_db_path(command),
        Commands::Query(command) => selected_query_db_path(command),
        Commands::Ccs(command) => selected_ccs_db_path(command),
        Commands::Derive(command) => selected_derive_db_path(command),
        Commands::Model(command) => selected_model_db_path(command),
        Commands::Collection(command) => selected_collection_db_path(command),
        Commands::Automation(command) => selected_automation_db_path(command),
        Commands::Cache(command) => selected_cache_db_path(command),
        Commands::Provenance(command) => selected_provenance_db_path(command),
        Commands::Capability(command) => selected_capability_db_path(command),
        Commands::Trust(command) => selected_trust_db_path(command),
        Commands::Federation(command) => selected_federation_db_path(command),
        Commands::VerifyDerivation(command) => selected_verify_db_path(command),
        Commands::System(command) => selected_system_db_path(command),
        _ => DEFAULT_DB_PATH,
    }
}

fn selected_repo_db_path(command: &cli::RepoCommands) -> &str {
    match command {
        cli::RepoCommands::Add { db, .. }
        | cli::RepoCommands::List { db, .. }
        | cli::RepoCommands::Remove { db, .. }
        | cli::RepoCommands::ResetTrust { db, .. }
        | cli::RepoCommands::Enable { db, .. }
        | cli::RepoCommands::Disable { db, .. }
        | cli::RepoCommands::Sync { db, .. }
        | cli::RepoCommands::KeyImport { db, .. }
        | cli::RepoCommands::KeyList { db, .. }
        | cli::RepoCommands::KeyRemove { db, .. } => &db.db_path,
    }
}

fn selected_config_db_path(command: &cli::ConfigCommands) -> &str {
    match command {
        cli::ConfigCommands::List { db, .. } | cli::ConfigCommands::Backups { db, .. } => {
            &db.db_path
        }
        cli::ConfigCommands::Diff { common, .. }
        | cli::ConfigCommands::Backup { common, .. }
        | cli::ConfigCommands::Restore { common, .. }
        | cli::ConfigCommands::Check { common, .. } => &common.db.db_path,
    }
}

fn selected_distro_db_path(command: &cli::DistroCommands) -> &str {
    match command {
        cli::DistroCommands::Set { db, .. }
        | cli::DistroCommands::Remove { db, .. }
        | cli::DistroCommands::List { db, .. }
        | cli::DistroCommands::Info { db, .. }
        | cli::DistroCommands::Mixing { db, .. }
        | cli::DistroCommands::SelectionMode { db, .. } => &db.db_path,
    }
}

fn selected_canonical_db_path(command: &cli::CanonicalCommands) -> &str {
    match command {
        cli::CanonicalCommands::Show { db, .. }
        | cli::CanonicalCommands::Search { db, .. }
        | cli::CanonicalCommands::Unmapped { db, .. } => &db.db_path,
    }
}

fn selected_groups_db_path(command: &cli::GroupsCommands) -> &str {
    match command {
        cli::GroupsCommands::List { db, .. } | cli::GroupsCommands::Show { db, .. } => &db.db_path,
    }
}

fn selected_registry_db_path(command: &cli::RegistryCommands) -> &str {
    match command {
        cli::RegistryCommands::Update { db, .. } | cli::RegistryCommands::Stats { db, .. } => {
            &db.db_path
        }
    }
}

fn selected_query_db_path(command: &cli::QueryCommands) -> &str {
    match command {
        cli::QueryCommands::Depends { db, .. }
        | cli::QueryCommands::Rdepends { db, .. }
        | cli::QueryCommands::Deptree { db, .. }
        | cli::QueryCommands::Whatprovides { db, .. }
        | cli::QueryCommands::Whatbreaks { db, .. }
        | cli::QueryCommands::Reason { db, .. }
        | cli::QueryCommands::Repquery { db, .. }
        | cli::QueryCommands::Component { db, .. }
        | cli::QueryCommands::Components { db, .. }
        | cli::QueryCommands::Scripts { db, .. }
        | cli::QueryCommands::DeltaStats { db, .. }
        | cli::QueryCommands::Conflicts { db, .. } => &db.db_path,
        cli::QueryCommands::Label(command) => selected_label_db_path(command),
    }
}

fn selected_label_db_path(command: &cli::LabelCommands) -> &str {
    match command {
        cli::LabelCommands::List { db, .. }
        | cli::LabelCommands::Add { db, .. }
        | cli::LabelCommands::Remove { db, .. }
        | cli::LabelCommands::Path { db, .. }
        | cli::LabelCommands::Show { db, .. }
        | cli::LabelCommands::Set { db, .. }
        | cli::LabelCommands::Query { db, .. }
        | cli::LabelCommands::Link { db, .. }
        | cli::LabelCommands::Delegate { db, .. } => &db.db_path,
    }
}

fn selected_ccs_db_path(command: &cli::CcsCommands) -> &str {
    match command {
        cli::CcsCommands::Install { common, .. } => &common.db.db_path,
        cli::CcsCommands::Export { db, .. }
        | cli::CcsCommands::Shell { db, .. }
        | cli::CcsCommands::Run { db, .. }
        | cli::CcsCommands::Enhance { db, .. } => &db.db_path,
        cli::CcsCommands::Init { .. }
        | cli::CcsCommands::Build { .. }
        | cli::CcsCommands::Inspect { .. }
        | cli::CcsCommands::Verify { .. }
        | cli::CcsCommands::Sign { .. }
        | cli::CcsCommands::Keygen { .. } => DEFAULT_DB_PATH,
    }
}

fn selected_derive_db_path(command: &cli::DeriveCommands) -> &str {
    match command {
        cli::DeriveCommands::List { db, .. }
        | cli::DeriveCommands::Show { db, .. }
        | cli::DeriveCommands::Create { db, .. }
        | cli::DeriveCommands::Patch { db, .. }
        | cli::DeriveCommands::Override { db, .. }
        | cli::DeriveCommands::Build { db, .. }
        | cli::DeriveCommands::Delete { db, .. }
        | cli::DeriveCommands::Stale { db, .. } => &db.db_path,
    }
}

fn selected_model_db_path(command: &cli::ModelCommands) -> &str {
    match command {
        cli::ModelCommands::Diff { db, .. }
        | cli::ModelCommands::Check { db, .. }
        | cli::ModelCommands::Snapshot { db, .. }
        | cli::ModelCommands::Lock { db, .. }
        | cli::ModelCommands::Update { db, .. }
        | cli::ModelCommands::RemoteDiff { db, .. }
        | cli::ModelCommands::Publish { db, .. } => &db.db_path,
        cli::ModelCommands::Apply { common, .. } => &common.db.db_path,
    }
}

fn selected_collection_db_path(command: &cli::CollectionCommands) -> &str {
    match command {
        cli::CollectionCommands::Create { db, .. }
        | cli::CollectionCommands::List { db, .. }
        | cli::CollectionCommands::Show { db, .. }
        | cli::CollectionCommands::Add { db, .. }
        | cli::CollectionCommands::Remove { db, .. }
        | cli::CollectionCommands::Delete { db, .. } => &db.db_path,
    }
}

fn selected_automation_db_path(command: &cli::AutomationCommands) -> &str {
    match command {
        cli::AutomationCommands::Status { db, .. }
        | cli::AutomationCommands::Configure { db, .. }
        | cli::AutomationCommands::History { db, .. } => &db.db_path,
        cli::AutomationCommands::Check { common, .. }
        | cli::AutomationCommands::Apply { common, .. }
        | cli::AutomationCommands::Daemon { common, .. } => &common.db.db_path,
    }
}

fn selected_cache_db_path(command: &cli::CacheCommands) -> &str {
    match command {
        cli::CacheCommands::Populate { db, .. } | cli::CacheCommands::Status { db, .. } => {
            &db.db_path
        }
    }
}

fn selected_provenance_db_path(command: &cli::ProvenanceCommands) -> &str {
    match command {
        cli::ProvenanceCommands::Show { db, .. }
        | cli::ProvenanceCommands::Verify { db, .. }
        | cli::ProvenanceCommands::Diff { db, .. }
        | cli::ProvenanceCommands::FindByDep { db, .. }
        | cli::ProvenanceCommands::Export { db, .. }
        | cli::ProvenanceCommands::Register { db, .. }
        | cli::ProvenanceCommands::Audit { db, .. } => &db.db_path,
    }
}

fn selected_capability_db_path(command: &cli::CapabilityCommands) -> &str {
    match command {
        cli::CapabilityCommands::Show { db, .. }
        | cli::CapabilityCommands::List { db, .. }
        | cli::CapabilityCommands::Audit { db, .. }
        | cli::CapabilityCommands::Run { db, .. } => &db.db_path,
        cli::CapabilityCommands::Validate { .. } | cli::CapabilityCommands::Generate { .. } => {
            DEFAULT_DB_PATH
        }
    }
}

fn selected_trust_db_path(command: &cli::TrustCommands) -> &str {
    match command {
        cli::TrustCommands::Init { db, .. }
        | cli::TrustCommands::Enable { db, .. }
        | cli::TrustCommands::Disable { db, .. }
        | cli::TrustCommands::Status { db, .. }
        | cli::TrustCommands::Verify { db, .. } => &db.db_path,
        cli::TrustCommands::KeyGen { .. } => DEFAULT_DB_PATH,
    }
}

fn selected_federation_db_path(command: &cli::FederationCommands) -> &str {
    match command {
        cli::FederationCommands::Status { db, .. }
        | cli::FederationCommands::Peers { db, .. }
        | cli::FederationCommands::AddPeer { db, .. }
        | cli::FederationCommands::RemovePeer { db, .. }
        | cli::FederationCommands::Stats { db, .. }
        | cli::FederationCommands::EnablePeer { db, .. }
        | cli::FederationCommands::DisablePeer { db, .. }
        | cli::FederationCommands::Test { db, .. }
        | cli::FederationCommands::Scan { db, .. } => &db.db_path,
    }
}

fn selected_verify_db_path(command: &cli::VerifyCommands) -> &str {
    match command {
        cli::VerifyCommands::Chain { db, .. }
        | cli::VerifyCommands::Rebuild { db, .. }
        | cli::VerifyCommands::Diverse { db, .. } => &db.db_path,
    }
}

fn selected_system_db_path(command: &cli::SystemCommands) -> &str {
    match command {
        cli::SystemCommands::Init { db, .. }
        | cli::SystemCommands::History { db, .. }
        | cli::SystemCommands::Adopt { db, .. }
        | cli::SystemCommands::Unadopt { db, .. }
        | cli::SystemCommands::NativeHandoff { db, .. }
        | cli::SystemCommands::Gc { db, .. }
        | cli::SystemCommands::Sbom { db, .. }
        | cli::SystemCommands::Takeover { db, .. } => &db.db_path,
        cli::SystemCommands::Verify { common, .. }
        | cli::SystemCommands::Restore { common, .. } => &common.db.db_path,
        cli::SystemCommands::DbBackup { command } => selected_db_backup_db_path(command),
        cli::SystemCommands::State(command) => selected_state_db_path(command),
        cli::SystemCommands::Generation(command) => selected_generation_db_path(command),
        cli::SystemCommands::Trigger(command) => selected_trigger_db_path(command),
        cli::SystemCommands::Redirect(command) => selected_redirect_db_path(command),
        cli::SystemCommands::UpdateChannel { action } => selected_update_channel_db_path(action),
        cli::SystemCommands::Completions { .. } => DEFAULT_DB_PATH,
    }
}

fn selected_db_backup_db_path(command: &cli::DbBackupCommands) -> &str {
    match command {
        cli::DbBackupCommands::List { db, .. }
        | cli::DbBackupCommands::Verify { db, .. }
        | cli::DbBackupCommands::Recover { db, .. } => &db.db_path,
    }
}

fn selected_state_db_path(command: &cli::StateCommands) -> &str {
    match command {
        cli::StateCommands::List { db, .. }
        | cli::StateCommands::Show { db, .. }
        | cli::StateCommands::Diff { db, .. }
        | cli::StateCommands::Revert { db, .. }
        | cli::StateCommands::Prune { db, .. }
        | cli::StateCommands::Create { db, .. } => &db.db_path,
        cli::StateCommands::Rollback { common, .. } => &common.db.db_path,
    }
}

fn selected_generation_db_path(command: &cli::GenerationCommands) -> &str {
    match command {
        cli::GenerationCommands::Build { db, .. }
        | cli::GenerationCommands::Publish { db, .. }
        | cli::GenerationCommands::Pending { db, .. }
        | cli::GenerationCommands::VerifyDbBackup { db, .. }
        | cli::GenerationCommands::RecoverDb { db, .. }
        | cli::GenerationCommands::Gc { db, .. }
        | cli::GenerationCommands::Recover { db, .. } => &db.db_path,
        cli::GenerationCommands::List
        | cli::GenerationCommands::Export { .. }
        | cli::GenerationCommands::Switch { .. }
        | cli::GenerationCommands::Rollback { .. }
        | cli::GenerationCommands::Info { .. } => DEFAULT_DB_PATH,
    }
}

fn selected_trigger_db_path(command: &cli::TriggerCommands) -> &str {
    match command {
        cli::TriggerCommands::List { db, .. }
        | cli::TriggerCommands::Show { db, .. }
        | cli::TriggerCommands::Enable { db, .. }
        | cli::TriggerCommands::Disable { db, .. }
        | cli::TriggerCommands::Add { db, .. }
        | cli::TriggerCommands::Remove { db, .. }
        | cli::TriggerCommands::Run { db, .. } => &db.db_path,
    }
}

fn selected_redirect_db_path(command: &cli::RedirectCommands) -> &str {
    match command {
        cli::RedirectCommands::List { db, .. }
        | cli::RedirectCommands::Add { db, .. }
        | cli::RedirectCommands::Show { db, .. }
        | cli::RedirectCommands::Remove { db, .. }
        | cli::RedirectCommands::Resolve { db, .. } => &db.db_path,
    }
}

fn selected_update_channel_db_path(command: &cli::UpdateChannelAction) -> &str {
    match command {
        cli::UpdateChannelAction::Get { db, .. }
        | cli::UpdateChannelAction::Set { db, .. }
        | cli::UpdateChannelAction::Reset { db, .. } => &db.db_path,
    }
}

fn env_forces_non_interactive() -> bool {
    std::env::var("CONARY_NON_INTERACTIVE").as_deref() == Ok("1")
}

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
            explain,
            isolated,
            no_isolation,
            hermetic,
            json,
            record,
            record_output,
            record_backend,
            record_validate,
            keep_raw_trace,
            record_unsafe_host,
            record_allow_network,
            record_command,
        }) => {
            if record {
                let source = target
                    .as_deref()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let output_dir = record_output
                    .as_deref()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| commands::record_mode::default_record_output_dir(&source));
                return commands::cmd_cook_record(commands::record_mode::RecordCliRequest {
                    source,
                    output_dir,
                    backend: commands::record_mode::RequestedRecordBackend::parse(
                        record_backend.as_deref(),
                    )?,
                    validate: record_validate,
                    keep_raw_trace,
                    unsafe_host: record_unsafe_host,
                    allow_network: record_allow_network,
                    json,
                    command: record_command,
                })
                .await;
            }

            commands::cmd_cook(
                target.as_deref(),
                recipe.as_deref(),
                &output,
                &source_cache,
                jobs,
                keep_builddir,
                validate_only,
                fetch_only,
                explain,
                isolated,
                no_isolation,
                hermetic,
                json,
            )
            .await
        }

        Some(Commands::New {
            name,
            from,
            output,
            force,
            explain,
        }) => {
            commands::cmd_new(
                name.as_deref(),
                from.as_deref(),
                output.as_deref(),
                force,
                explain,
            )
            .await
        }

        Some(Commands::Try {
            target,
            activate,
            allow_irreversible,
            watch,
            isolated,
            recipe,
            json,
            run,
            db,
        }) => match try_dispatch_action(TryDispatchInput {
            target,
            activate,
            allow_irreversible,
            isolated,
            run: &run,
            watch,
            recipe,
            json,
        })? {
            TryDispatchAction::Package(package) => {
                commands::cmd_try_package(
                    &db.db_path,
                    Path::new(&package),
                    activate,
                    allow_irreversible,
                    &run,
                )
                .await
            }
            TryDispatchAction::Watch(watch) => {
                commands::cmd_try_watch(
                    &db.db_path,
                    &watch.target,
                    watch.recipe.as_deref(),
                    watch.isolated,
                    watch.json,
                )
                .await
            }
            TryDispatchAction::Status => commands::cmd_try_status(&db.db_path).await,
            TryDispatchAction::Rollback => commands::cmd_try_rollback(&db.db_path).await,
            TryDispatchAction::Keep => commands::cmd_try_keep(&db.db_path).await,
        },

        Some(Commands::Publish {
            what,
            target,
            recipe,
            key_dir,
            state_file,
            refresh,
            force_reinit,
            accept_destination_state,
            rotate_publish_key,
            rotate_root_key,
            yes,
            json,
        }) => {
            commands::cmd_publish(commands::PublishOptions {
                what,
                target,
                recipe,
                key_dir,
                state_file,
                refresh,
                force_reinit,
                accept_destination_state,
                rotate_publish_key,
                rotate_root_key,
                yes,
                json,
            })
            .await
        }

        Some(Commands::ConvertPkgbuild { pkgbuild, output }) => {
            commands::cmd_convert_pkgbuild(&pkgbuild, output.as_deref()).await
        }

        Some(Commands::RecipeAudit { recipe, all, trace }) => {
            commands::cmd_recipe_audit(recipe.as_deref(), all, trace).await
        }

        Some(Commands::Mcp(cli::McpCommands::Packaging)) => commands::cmd_mcp_packaging().await,

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

#[cfg(test)]
mod tests {
    use super::run_try_session_preflight_for_test;
    use crate::cli::Cli;
    use clap::Parser;
    use conary_core::db::models::{CreateTrySession, TrySession, TrySessionMode, TrySessionStatus};
    use std::ffi::OsString;

    struct TryPreflightFixture {
        _temp: tempfile::TempDir,
        db_path: std::path::PathBuf,
        db_path_string: String,
    }

    impl TryPreflightFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let db_path = temp.path().join("conary.db");
            conary_core::db::init(&db_path).unwrap();
            let db_path_string = db_path.to_string_lossy().into_owned();
            Self {
                _temp: temp,
                db_path,
                db_path_string,
            }
        }

        fn open(&self) -> rusqlite::Connection {
            conary_core::db::open(&self.db_path).unwrap()
        }

        fn parse_with_db(&self, args: &[&str]) -> Cli {
            let mut full = args.to_vec();
            full.extend(["--db-path", &self.db_path_string]);
            Cli::try_parse_from(full).unwrap()
        }

        fn create_session(&self, id: &str, mode: TrySessionMode) -> TrySession {
            TrySession::create_active(
                &self.open(),
                CreateTrySession {
                    id,
                    package_path: &self
                        ._temp
                        .path()
                        .join(format!("{id}.ccs"))
                        .to_string_lossy(),
                    package_name: Some("demo"),
                    package_version: Some("1.0.0"),
                    previous_generation_id: Some(1),
                    mode,
                    work_dir: &self._temp.path().join("try").join(id).to_string_lossy(),
                },
            )
            .unwrap()
        }

        fn stored_session(&self, id: &str) -> TrySession {
            TrySession::find_by_id(&self.open(), id)
                .unwrap()
                .expect("stored try session")
        }

        fn set_current_generation(&self, generation: i64) {
            std::fs::create_dir_all(self._temp.path().join(format!("generations/{generation}")))
                .unwrap();
            conary_core::generation::mount::update_current_symlink(self._temp.path(), generation)
                .unwrap();
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read_only_cli(fixture: &TryPreflightFixture) -> Cli {
        fixture.parse_with_db(&["conary", "list"])
    }

    fn mutating_cli(fixture: &TryPreflightFixture) -> Cli {
        fixture.parse_with_db(&["conary", "pin", "demo"])
    }

    fn dry_run_cli(fixture: &TryPreflightFixture) -> Cli {
        fixture.parse_with_db(&["conary", "install", "demo", "--dry-run"])
    }

    fn set_launcher(session: &TrySession, fixture: &TryPreflightFixture, pid: i64, boot_id: &str) {
        session.set_launcher(&fixture.open(), pid, boot_id).unwrap();
    }

    fn set_try_generation(session: &TrySession, fixture: &TryPreflightFixture, generation: i64) {
        session
            .set_try_generation(&fixture.open(), generation)
            .unwrap();
    }

    fn assert_message_mentions_try_actions(message: &str) {
        assert!(message.contains("try status"), "{message}");
        assert!(message.contains("try rollback"), "{message}");
        assert!(message.contains("try keep"), "{message}");
    }

    #[test]
    fn try_dispatch_watch_defaults_to_current_dir() {
        match super::try_dispatch_action(super::TryDispatchInput {
            target: None,
            activate: false,
            allow_irreversible: false,
            isolated: false,
            run: &[],
            watch: true,
            recipe: None,
            json: false,
        })
        .unwrap()
        {
            super::TryDispatchAction::Watch(watch) => {
                assert_eq!(watch.target, ".");
                assert_eq!(watch.recipe, None);
                assert!(!watch.json);
                assert!(!watch.isolated);
            }
            other => panic!("unexpected try dispatch action: {other:?}"),
        }
    }

    #[test]
    fn try_dispatch_watch_accepts_isolated() {
        match super::try_dispatch_action(super::TryDispatchInput {
            target: Some(".".to_string()),
            activate: false,
            allow_irreversible: false,
            isolated: true,
            run: &[],
            watch: true,
            recipe: None,
            json: true,
        })
        .unwrap()
        {
            super::TryDispatchAction::Watch(watch) => {
                assert_eq!(watch.target, ".");
                assert!(watch.isolated);
                assert!(watch.json);
            }
            other => panic!("unexpected try dispatch action: {other:?}"),
        }
    }

    #[test]
    fn try_dispatch_rejects_isolated_without_watch() {
        let err = super::try_dispatch_action(super::TryDispatchInput {
            target: Some("pkg.ccs".to_string()),
            activate: false,
            allow_irreversible: false,
            isolated: true,
            run: &[],
            watch: false,
            recipe: None,
            json: false,
        })
        .expect_err("isolated without watch should fail");
        assert!(
            err.to_string().contains("--isolated requires --watch"),
            "{err:#}"
        );
    }

    #[test]
    fn try_dispatch_watch_rejects_artifacts_actions_activation_and_run_commands() {
        for (target, activate, allow_irreversible, run, message) in [
            (
                Some("pkg.ccs".to_string()),
                false,
                false,
                vec![],
                "does not accept prebuilt .ccs artifacts",
            ),
            (
                Some("status".to_string()),
                false,
                false,
                vec![],
                "cannot be combined with try action",
            ),
            (
                Some("rollback".to_string()),
                false,
                false,
                vec![],
                "cannot be combined with try action",
            ),
            (
                Some("keep".to_string()),
                false,
                false,
                vec![],
                "cannot be combined with try action",
            ),
            (
                None,
                true,
                false,
                vec![],
                "cannot be combined with --activate",
            ),
            (
                None,
                false,
                true,
                vec![],
                "cannot be combined with --allow-irreversible",
            ),
            (
                None,
                false,
                false,
                vec!["/bin/true".to_string()],
                "cannot run a command",
            ),
        ] {
            let err = super::try_dispatch_action(super::TryDispatchInput {
                target,
                activate,
                allow_irreversible,
                isolated: false,
                run: &run,
                watch: true,
                recipe: None,
                json: false,
            })
            .expect_err("watch conflict should fail");
            assert!(err.to_string().contains(message), "{err:#}");
        }
    }

    #[test]
    fn live_namespace_read_only_preflight_allows_command() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        let session = fixture.create_session("try-live-ns", TrySessionMode::Namespace);
        set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");

        run_try_session_preflight_for_test(&read_only_cli(&fixture), true).unwrap();

        assert_eq!(
            fixture.stored_session("try-live-ns").status,
            TrySessionStatus::Active
        );
    }

    #[test]
    fn live_namespace_mutating_preflight_blocks_command() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        let session = fixture.create_session("try-live-ns", TrySessionMode::Namespace);
        set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");

        let err = run_try_session_preflight_for_test(&mutating_cli(&fixture), true)
            .expect_err("live namespace try session should block mutating commands");

        let message = err.to_string();
        assert!(
            message.contains("another try session is active"),
            "{message}"
        );
        assert_message_mentions_try_actions(&message);
        assert_eq!(
            fixture.stored_session("try-live-ns").status,
            TrySessionStatus::Active
        );
    }

    #[test]
    fn live_namespace_dry_run_preflight_allows_command() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        let session = fixture.create_session("try-live-ns", TrySessionMode::Namespace);
        set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");

        run_try_session_preflight_for_test(&dry_run_cli(&fixture), true).unwrap();

        assert_eq!(
            fixture.stored_session("try-live-ns").status,
            TrySessionStatus::Active
        );
    }

    #[test]
    fn completed_namespace_preflight_stays_active_and_blocks_mutation() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        fixture.create_session("try-complete-ns", TrySessionMode::Namespace);

        let err = run_try_session_preflight_for_test(&mutating_cli(&fixture), true)
            .expect_err("decision-pending namespace try session should block mutating commands");

        let message = err.to_string();
        assert!(
            message.contains("another try session is active"),
            "{message}"
        );
        assert_message_mentions_try_actions(&message);
        assert_eq!(
            fixture.stored_session("try-complete-ns").status,
            TrySessionStatus::Active
        );
    }

    #[test]
    fn orphaned_namespace_preflight_marks_orphaned_and_blocks_command() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        let session = fixture.create_session("try-orphan-ns", TrySessionMode::Namespace);
        set_launcher(&session, &fixture, 9_999_999, "boot-a");

        let err = run_try_session_preflight_for_test(&read_only_cli(&fixture), false)
            .expect_err("orphaned namespace try session should block ordinary commands");

        let message = err.to_string();
        assert!(message.contains("orphaned try session"), "{message}");
        assert_message_mentions_try_actions(&message);
        assert_eq!(
            fixture.stored_session("try-orphan-ns").status,
            TrySessionStatus::Orphaned
        );
    }

    #[test]
    fn live_activated_read_only_preflight_allows_command() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        fixture.set_current_generation(7);
        let session = fixture.create_session("try-live-activated", TrySessionMode::Activated);
        set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
        set_try_generation(&session, &fixture, 7);

        run_try_session_preflight_for_test(&read_only_cli(&fixture), true).unwrap();

        assert_eq!(
            fixture.stored_session("try-live-activated").status,
            TrySessionStatus::Active
        );
    }

    #[test]
    fn live_activated_dry_run_preflight_allows_command() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        fixture.set_current_generation(7);
        let session = fixture.create_session("try-live-activated", TrySessionMode::Activated);
        set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
        set_try_generation(&session, &fixture, 7);

        run_try_session_preflight_for_test(&dry_run_cli(&fixture), true).unwrap();

        assert_eq!(
            fixture.stored_session("try-live-activated").status,
            TrySessionStatus::Active
        );
    }

    #[test]
    fn live_activated_mutating_preflight_blocks_command() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        fixture.set_current_generation(7);
        let session = fixture.create_session("try-live-activated", TrySessionMode::Activated);
        set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
        set_try_generation(&session, &fixture, 7);

        let err = run_try_session_preflight_for_test(&mutating_cli(&fixture), true)
            .expect_err("live activated try session should block mutating commands");

        let message = err.to_string();
        assert!(message.contains("activated try session"), "{message}");
        assert!(message.contains("is active"), "{message}");
        assert!(message.contains("try rollback"), "{message}");
        assert!(message.contains("try keep"), "{message}");
        assert_eq!(
            fixture.stored_session("try-live-activated").status,
            TrySessionStatus::Active
        );
    }

    #[test]
    fn orphaned_activated_interactive_preflight_marks_orphaned_and_blocks_command() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        fixture.set_current_generation(8);
        let session = fixture.create_session("try-orphan-activated", TrySessionMode::Activated);
        set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
        set_try_generation(&session, &fixture, 7);

        let err = run_try_session_preflight_for_test(&read_only_cli(&fixture), true)
            .expect_err("orphaned activated interactive preflight should block command");

        let message = err.to_string();
        assert!(
            message.contains("orphaned activated try session"),
            "{message}"
        );
        assert!(message.contains("try rollback"), "{message}");
        assert!(message.contains("try keep"), "{message}");
        assert_eq!(
            fixture.stored_session("try-orphan-activated").status,
            TrySessionStatus::Orphaned
        );
    }

    #[test]
    fn orphaned_activated_env_forced_non_interactive_attempts_rollback() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let _non_interactive_guard = EnvVarGuard::set("CONARY_NON_INTERACTIVE", "1");
        let fixture = TryPreflightFixture::new();
        fixture.set_current_generation(8);
        let session = fixture.create_session("try-orphan-activated", TrySessionMode::Activated);
        set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
        set_try_generation(&session, &fixture, 7);

        let err = run_try_session_preflight_for_test(&read_only_cli(&fixture), true)
            .expect_err("CONARY_NON_INTERACTIVE=1 should force automatic rollback");

        let message = err.to_string();
        assert!(message.contains("automatic rollback"), "{message}");
        assert_eq!(
            fixture.stored_session("try-orphan-activated").status,
            TrySessionStatus::Orphaned
        );
    }

    #[test]
    fn orphaned_activated_non_interactive_preflight_attempts_rollback() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        fixture.set_current_generation(8);
        let session = fixture.create_session("try-orphan-activated", TrySessionMode::Activated);
        set_launcher(&session, &fixture, i64::from(std::process::id()), "boot-a");
        set_try_generation(&session, &fixture, 7);

        let err = run_try_session_preflight_for_test(&read_only_cli(&fixture), false)
            .expect_err("rollback attempt should surface rollback error for invalid test package");

        let message = err.to_string();
        assert!(message.contains("automatic rollback"), "{message}");
        assert_eq!(
            fixture.stored_session("try-orphan-activated").status,
            TrySessionStatus::Orphaned
        );
    }

    #[test]
    fn preflight_uses_default_db_path_for_commands_without_db_args() {
        let cli = Cli::try_parse_from(["conary", "cook", "."]).unwrap();

        let command = cli.command.as_ref().expect("parsed command");
        assert_eq!(super::selected_db_path(command), super::DEFAULT_DB_PATH);
    }

    #[test]
    fn commands_without_db_args_do_not_use_try_session_preflight_scope() {
        for args in [
            ["conary", "cook", "."].as_slice(),
            ["conary", "new", "hello-m1b"].as_slice(),
            ["conary", "publish", "./repo", "--recipe", "recipe.toml"].as_slice(),
            ["conary", "publish", "dist/pkg.ccs", "./repo"].as_slice(),
            [
                "conary",
                "bootstrap",
                "verify-convergence",
                "--run-a",
                "/tmp/run-a",
                "--run-b",
                "/tmp/run-b",
            ]
            .as_slice(),
            [
                "conary",
                "bootstrap",
                "diff-seeds",
                "/tmp/seed-a",
                "/tmp/seed-b",
            ]
            .as_slice(),
            ["conary", "system", "completions", "bash"].as_slice(),
            [
                "conary",
                "ccs",
                "init",
                "/tmp/ccs-demo",
                "--name",
                "ccs-demo",
            ]
            .as_slice(),
            ["conary", "ccs", "build", "/tmp/ccs-demo"].as_slice(),
            ["conary", "ccs", "inspect", "/tmp/pkg.ccs"].as_slice(),
            ["conary", "ccs", "verify", "/tmp/pkg.ccs"].as_slice(),
            ["conary", "ccs", "sign", "/tmp/pkg.ccs", "--key", "/tmp/key"].as_slice(),
            ["conary", "ccs", "keygen", "--output", "/tmp/key"].as_slice(),
            ["conary", "capability", "validate", "/tmp/ccs.toml"].as_slice(),
            ["conary", "trust", "key-gen", "root", "--output", "/tmp"].as_slice(),
            ["conary", "query", "scripts", "/tmp/pkg.ccs"].as_slice(),
            ["conary", "query", "scripts", "/tmp/pkg.rpm"].as_slice(),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let command = cli.command.as_ref().expect("parsed command");
            assert!(!super::command_uses_try_session_preflight_db(command));
        }

        let cli = Cli::try_parse_from(["conary", "pin", "demo"]).unwrap();
        let command = cli.command.as_ref().expect("parsed command");
        assert!(super::command_uses_try_session_preflight_db(command));

        let cli = Cli::try_parse_from(["conary", "query", "scripts", "bash"]).unwrap();
        let command = cli.command.as_ref().expect("parsed command");
        assert!(super::command_uses_try_session_preflight_db(command));
    }

    #[tokio::test]
    async fn artifact_form_publish_reaches_artifact_reader_without_preflight_db() {
        let cli = Cli::try_parse_from(["conary", "publish", "dist/pkg.ccs", "./repo"]).unwrap();

        let err = crate::dispatch::dispatch(cli)
            .await
            .expect_err("artifact-form publish should reach artifact handling");

        assert!(
            err.to_string()
                .contains("Failed to open package: dist/pkg.ccs")
        );
    }

    #[test]
    fn nested_db_command_preflights_selected_db_path() {
        let _env_lock = lock_env();
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
        let fixture = TryPreflightFixture::new();
        let session = fixture.create_session("try-nested-db", TrySessionMode::Namespace);
        set_launcher(&session, &fixture, 9_999_999, "boot-a");

        let cli = fixture.parse_with_db(&["conary", "repo", "list"]);
        let err = run_try_session_preflight_for_test(&cli, true)
            .expect_err("nested repo command should inspect the selected active-session DB");

        let message = err.to_string();
        assert!(message.contains("orphaned try session"), "{message}");
        assert_eq!(
            fixture.stored_session("try-nested-db").status,
            TrySessionStatus::Orphaned
        );
    }

    #[test]
    fn try_action_commands_skip_orphan_preflight() {
        let _env_lock = lock_env();
        let fixture = TryPreflightFixture::new();
        let session = fixture.create_session("try-action", TrySessionMode::Namespace);
        set_launcher(&session, &fixture, 9_999_999, "old-boot");

        for action in ["status", "rollback", "keep"] {
            let cli = fixture.parse_with_db(&["conary", "try", action]);
            run_try_session_preflight_for_test(&cli, false).unwrap();
        }

        assert_eq!(
            fixture.stored_session("try-action").status,
            TrySessionStatus::Active
        );
    }

    #[test]
    fn database_not_found_is_no_active_try_session() {
        let missing_db = tempfile::tempdir()
            .unwrap()
            .path()
            .join("missing")
            .join("conary.db");
        let db_path = missing_db.to_string_lossy();
        let cli = Cli::try_parse_from(["conary", "list", "--db-path", &db_path]).unwrap();

        run_try_session_preflight_for_test(&cli, true).unwrap();
    }

    #[test]
    fn package_named_try_action_requires_explicit_path_prefix() {
        let fixture = TryPreflightFixture::new();
        for package in ["./status", "./rollback", "./keep"] {
            let cli = fixture.parse_with_db(&["conary", "try", package]);
            run_try_session_preflight_for_test(&cli, true).unwrap();
        }
    }

    #[test]
    fn activated_liveness_rejects_recorded_dead_pid_but_allows_absent_pid() {
        let session = TrySession {
            id: "try-liveness".to_string(),
            package_path: "/tmp/demo.ccs".to_string(),
            package_name: None,
            package_version: None,
            previous_generation_id: Some(1),
            try_generation_id: Some(7),
            launcher_pid: Some(9_999_999),
            launcher_boot_id: Some("boot-a".to_string()),
            status: TrySessionStatus::Active,
            mode: TrySessionMode::Activated,
            work_dir: "/tmp/try-liveness".to_string(),
            last_error: None,
            started_at: None,
            updated_at: None,
            completed_at: None,
        };

        assert!(!super::activated_try_session_is_live(
            &session,
            "boot-a",
            Some(7)
        ));

        let no_pid = TrySession {
            launcher_pid: None,
            ..session
        };
        assert!(super::activated_try_session_is_live(
            &no_pid,
            "boot-a",
            Some(7)
        ));
    }
}
