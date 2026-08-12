// apps/conary/src/dispatch/root/try_preflight.rs

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cli::{self, Commands};
use crate::command_risk::{self, CommandRisk};
use crate::commands;
use crate::commands::try_session::{
    activated_try_session_is_live, current_boot_id, namespace_try_session_is_decision_pending,
};
use conary_core::db::models::{TrySession, TrySessionMode};
use conary_core::runtime_root::ConaryRuntimeRoot;

pub(super) const DEFAULT_DB_PATH: &str = "/var/lib/conary/conary.db";

#[derive(Debug)]
pub(super) struct TryWatchDispatch {
    pub(super) target: String,
    pub(super) recipe: Option<String>,
    pub(super) signing_key_path: std::path::PathBuf,
    pub(super) isolated: bool,
    pub(super) json: bool,
}

#[derive(Debug)]
pub(super) enum TryDispatchAction {
    Package {
        package: String,
        trust_policy_path: std::path::PathBuf,
    },
    Watch(TryWatchDispatch),
    Status,
    Rollback,
    Keep,
}

pub(super) struct TryDispatchInput<'a> {
    pub(super) target: Option<String>,
    pub(super) activate: bool,
    pub(super) policy: Option<String>,
    pub(super) isolated: bool,
    pub(super) run: &'a [String],
    pub(super) watch: bool,
    pub(super) recipe: Option<String>,
    pub(super) key: Option<String>,
    pub(super) json: bool,
}

pub(super) fn try_dispatch_action(input: TryDispatchInput<'_>) -> Result<TryDispatchAction> {
    if input.watch {
        if input.activate {
            bail!("conary try --watch cannot be combined with --activate");
        }
        if !input.run.is_empty() {
            bail!("conary try --watch cannot run a command");
        }
        if input.policy.is_some() {
            bail!("conary try --watch derives trust from --key and cannot use --policy");
        }
        let target = input
            .target
            .or_else(|| input.recipe.clone())
            .context(
                "conary try --watch requires an explicit recipe file or project directory containing recipe.toml",
            )?;
        if is_reserved_try_action(&target) {
            bail!("conary try --watch cannot be combined with try action '{target}'");
        }
        let signing_key_path = input
            .key
            .map(std::path::PathBuf::from)
            .context("conary try --watch requires --key <private-key>")?;
        return Ok(TryDispatchAction::Watch(TryWatchDispatch {
            target,
            recipe: input.recipe,
            signing_key_path,
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
                && input.policy.is_none()
                && input.run.is_empty() =>
        {
            Ok(match target.as_str() {
                "status" => TryDispatchAction::Status,
                "rollback" => TryDispatchAction::Rollback,
                "keep" => TryDispatchAction::Keep,
                _ => unreachable!("reserved try action checked above"),
            })
        }
        Some(target) => {
            let trust_policy_path = input
                .policy
                .map(std::path::PathBuf::from)
                .context("conary try <package> requires --policy <PATH>")?;
            Ok(TryDispatchAction::Package {
                package: target,
                trust_policy_path,
            })
        }
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
            policy: None,
            run,
            ..
        } if run.is_empty() && is_reserved_try_action(target)
    )
}

pub(super) fn command_uses_try_session_preflight_db(command: &Commands) -> bool {
    match command {
        Commands::Cook { .. }
        | Commands::New { .. }
        | Commands::Publish { .. }
        | Commands::Mcp(_)
        | Commands::Bootstrap(
            cli::BootstrapCommands::VerifyConvergence { .. }
            | cli::BootstrapCommands::DiffSeeds { .. },
        )
        | Commands::System(
            cli::SystemCommands::Completions { .. } | cli::SystemCommands::RebuildDatabase { .. },
        )
        | Commands::Ccs(
            cli::CcsCommands::Init { .. }
            | cli::CcsCommands::Build { .. }
            | cli::CcsCommands::Lint { .. }
            | cli::CcsCommands::Inspect { .. }
            | cli::CcsCommands::Verify { .. }
            | cli::CcsCommands::Test { .. }
            | cli::CcsCommands::Sign { .. }
            | cli::CcsCommands::Keygen { .. },
        )
        | Commands::Capability(cli::CapabilityCommands::Validate { .. })
        | Commands::Trust(cli::TrustCommands::KeyGen { .. }) => false,
        Commands::Query(cli::QueryCommands::Scripts { package_path, .. }) => {
            !query_scripts_target_uses_package_file(package_path)
        }
        _ => true,
    }
}

fn query_scripts_target_uses_package_file(package_path: &str) -> bool {
    Path::new(package_path).is_file()
}

pub(in crate::dispatch) fn run_try_session_preflight(cli: &crate::cli::Cli) -> Result<()> {
    run_try_session_preflight_inner(cli, std::io::stdin().is_terminal())
}

#[cfg(test)]
pub(super) fn run_try_session_preflight_for_test(
    cli: &crate::cli::Cli,
    interactive: bool,
) -> Result<()> {
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

pub(super) fn selected_db_path(command: &Commands) -> &str {
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
        cli::RepoCommands::Add { args } => &args.db.db_path,
        cli::RepoCommands::List { db, .. }
        | cli::RepoCommands::Remove { db, .. }
        | cli::RepoCommands::ResetTrust { db, .. }
        | cli::RepoCommands::Enable { db, .. }
        | cli::RepoCommands::Disable { db, .. }
        | cli::RepoCommands::Sync { db, .. } => &db.db_path,
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
        cli::DistroCommands::List { db, .. } | cli::DistroCommands::Info { db, .. } => &db.db_path,
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
        cli::CcsCommands::Enhance { db, .. } => &db.db_path,
        cli::CcsCommands::Init { .. }
        | cli::CcsCommands::Build { .. }
        | cli::CcsCommands::Lint { .. }
        | cli::CcsCommands::Inspect { .. }
        | cli::CcsCommands::Verify { .. }
        | cli::CcsCommands::Test { .. }
        | cli::CcsCommands::Sign { .. }
        | cli::CcsCommands::Export { .. }
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
        cli::CapabilityCommands::Validate { .. } => DEFAULT_DB_PATH,
    }
}

fn selected_trust_db_path(command: &cli::TrustCommands) -> &str {
    match command {
        cli::TrustCommands::Init { db, .. }
        | cli::TrustCommands::Enable { db, .. }
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
        cli::VerifyCommands::Chain { db, .. } | cli::VerifyCommands::Diverse { db, .. } => {
            &db.db_path
        }
    }
}

fn selected_system_db_path(command: &cli::SystemCommands) -> &str {
    match command {
        cli::SystemCommands::Init { db, .. }
        | cli::SystemCommands::RebuildDatabase { db, .. }
        | cli::SystemCommands::History { db, .. }
        | cli::SystemCommands::Adopt { db, .. }
        | cli::SystemCommands::Unadopt { db, .. }
        | cli::SystemCommands::NativeHandoff { db, .. }
        | cli::SystemCommands::Sbom { db, .. }
        | cli::SystemCommands::Takeover { db, .. } => &db.db_path,
        cli::SystemCommands::Verify { common, .. }
        | cli::SystemCommands::Restore { common, .. }
        | cli::SystemCommands::RepositoryTakeover { common, .. } => &common.db.db_path,
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
        | cli::GenerationCommands::Activate { db, .. }
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
