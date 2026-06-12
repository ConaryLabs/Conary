// apps/conary/src/command_risk.rs
//! CLI command risk classification for live-host acknowledgement policy.

use crate::cli::{self, Cli, Commands};
use crate::live_host_safety::{
    LiveMutationClass, LiveMutationRequest, MutationIntent, require_mutation_intent,
};
use anyhow::Result;
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRisk {
    ReadOnly,
    LocalStateMutation,
    DryRunOnly,
    HookRefreshDbMutation,
    DbMutation,
    ActiveHostMutation,
    AlwaysLive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRiskPolicy {
    pub command_label: Cow<'static, str>,
    pub risk: CommandRisk,
    pub dry_run: bool,
    pub apply_intent: bool,
}

impl CommandRiskPolicy {
    pub fn requires_ack(&self) -> bool {
        self.requires_apply_intent()
    }

    pub fn requires_apply_intent(&self) -> bool {
        matches!(
            self.risk,
            CommandRisk::ActiveHostMutation | CommandRisk::AlwaysLive
        ) && !self.dry_run
    }

    fn mutation_class(&self) -> Option<LiveMutationClass> {
        match self.risk {
            CommandRisk::ReadOnly
            | CommandRisk::LocalStateMutation
            | CommandRisk::DryRunOnly
            | CommandRisk::HookRefreshDbMutation => None,
            CommandRisk::DbMutation => Some(LiveMutationClass::LiveConaryState),
            CommandRisk::ActiveHostMutation => {
                Some(LiveMutationClass::CurrentlyLiveEvenWithRootArguments)
            }
            CommandRisk::AlwaysLive => Some(LiveMutationClass::AlwaysLive),
        }
    }
}

pub fn enforce_cli_policy(allow_live_system_mutation: bool, cli: &Cli) -> Result<()> {
    let Some(policy) = classify_cli(cli) else {
        return Ok(());
    };

    if policy.risk == CommandRisk::HookRefreshDbMutation {
        return require_sync_hook_context(&policy);
    }

    if !policy.requires_apply_intent() {
        return Ok(());
    }

    let Some(class) = policy.mutation_class() else {
        return Ok(());
    };

    require_mutation_intent(&LiveMutationRequest {
        command_label: policy.command_label,
        class,
        dry_run: policy.dry_run,
        intent: MutationIntent::from_apply_intent(policy.apply_intent, allow_live_system_mutation),
    })
}

fn require_sync_hook_context(policy: &CommandRiskPolicy) -> Result<()> {
    #[cfg(unix)]
    {
        if !nix::unistd::Uid::effective().is_root() {
            anyhow::bail!(
                "command '{}' is reserved for installed native package-manager sync hooks; run `conary system adopt --refresh` for an interactive refresh",
                policy.command_label
            );
        }
    }

    #[cfg(not(unix))]
    {
        anyhow::bail!(
            "command '{}' is reserved for Unix native package-manager sync hooks",
            policy.command_label
        );
    }

    Ok(())
}

pub fn classify_cli(cli: &Cli) -> Option<CommandRiskPolicy> {
    match cli.command.as_ref()? {
        Commands::Install {
            package,
            dry_run,
            yes,
            ..
        } => Some(policy_with_intent(
            if package.starts_with('@') {
                "conary install @collection"
            } else {
                "conary install"
            },
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        )),
        Commands::Remove { yes, .. } => Some(policy_with_intent(
            "conary remove",
            CommandRisk::ActiveHostMutation,
            false,
            *yes,
        )),
        Commands::Update {
            package,
            dry_run,
            yes,
            ..
        } => Some(policy_with_intent(
            if package.as_deref().is_some_and(|pkg| pkg.starts_with('@')) {
                "conary update @collection"
            } else {
                "conary update"
            },
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        )),
        Commands::Autoremove { dry_run, yes, .. } => Some(policy_with_intent(
            "conary autoremove",
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        )),
        Commands::Pin { .. } => Some(local_state("conary pin")),
        Commands::Unpin { .. } => Some(local_state("conary unpin")),
        Commands::New { .. } => Some(local_state("conary new")),
        Commands::Search { .. }
        | Commands::List { .. }
        | Commands::Cook { .. }
        | Commands::ConvertPkgbuild { .. }
        | Commands::RecipeAudit { .. }
        | Commands::Canonical(_)
        | Commands::Groups(_)
        | Commands::Export { .. }
        | Commands::Derivation(_)
        | Commands::Profile(_)
        | Commands::Sbom { .. }
        | Commands::VerifyDerivation(_)
        | Commands::Capability(_) => Some(read_only("conary read-only or non-host command")),
        Commands::Publish { .. } => Some(local_state("conary publish")),
        Commands::System(command) => classify_system(command),
        Commands::Repo(command) => Some(classify_repo(command)),
        Commands::Config(command) => Some(classify_config(command)),
        Commands::Distro(command) => Some(classify_distro(command)),
        Commands::Registry(command) => Some(classify_registry(command)),
        Commands::Query(command) => Some(classify_query(command)),
        Commands::Ccs(command) => Some(classify_ccs(command)),
        Commands::Derive(command) => Some(classify_derive(command)),
        Commands::Model(command) => Some(classify_model(command)),
        Commands::Collection(command) => Some(classify_collection(command)),
        Commands::Automation(command) => Some(classify_automation(command)),
        Commands::Bootstrap(command) => Some(classify_bootstrap(command)),
        Commands::Cache(command) => Some(classify_cache(command)),
        Commands::SelfUpdate {
            check,
            force,
            verify_sha256,
            verify_signature_file,
            print_trusted_keys,
            ..
        } => Some(
            if *check
                || verify_sha256.is_some()
                || verify_signature_file.is_some()
                || *print_trusted_keys
            {
                read_only("conary self-update --check")
            } else {
                policy_with_intent(
                    "conary self-update",
                    CommandRisk::ActiveHostMutation,
                    false,
                    *force,
                )
            },
        ),
        Commands::Provenance(command) => Some(classify_provenance(command)),
        Commands::Trust(command) => Some(classify_trust(command)),
        Commands::Federation(command) => Some(classify_federation(command)),
    }
}

fn classify_system(command: &cli::SystemCommands) -> Option<CommandRiskPolicy> {
    match command {
        cli::SystemCommands::Init { .. } => Some(local_state("conary system init")),
        cli::SystemCommands::Completions { .. }
        | cli::SystemCommands::History { .. }
        | cli::SystemCommands::Verify { .. }
        | cli::SystemCommands::Sbom { .. } => Some(read_only("conary system read-only command")),
        cli::SystemCommands::Restore { dry_run, yes, .. } => Some(policy_with_intent(
            "conary system restore",
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        )),
        cli::SystemCommands::Adopt {
            system,
            status,
            dry_run,
            refresh,
            convert,
            sync_hook,
            quiet,
            from_sync_hook,
            ..
        } => classify_adopt(AdoptRiskInput {
            system: *system,
            status: *status,
            dry_run: *dry_run,
            refresh: *refresh,
            convert: *convert,
            sync_hook: *sync_hook,
            quiet: *quiet,
            from_sync_hook: *from_sync_hook,
        }),
        cli::SystemCommands::Unadopt { dry_run, yes, .. } => Some(policy_with_intent(
            "conary system unadopt",
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        )),
        cli::SystemCommands::NativeHandoff { dry_run, yes, .. } => Some(policy_with_intent(
            "conary system native-handoff",
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        )),
        cli::SystemCommands::Gc { .. } => Some(local_state("conary system gc")),
        cli::SystemCommands::DbBackup { command } => Some(classify_db_backup(command)),
        cli::SystemCommands::State(command) => Some(classify_state(command)),
        cli::SystemCommands::Generation(command) => Some(classify_generation(command)),
        cli::SystemCommands::Takeover { dry_run, yes, .. } => Some(policy_with_intent(
            "conary system takeover",
            CommandRisk::AlwaysLive,
            *dry_run,
            *yes,
        )),
        cli::SystemCommands::Trigger(command) => Some(classify_trigger(command)),
        cli::SystemCommands::Redirect(command) => Some(classify_redirect(command)),
        cli::SystemCommands::UpdateChannel { action } => Some(classify_update_channel(action)),
    }
}

fn classify_db_backup(command: &cli::DbBackupCommands) -> CommandRiskPolicy {
    match command {
        cli::DbBackupCommands::List { .. } | cli::DbBackupCommands::Verify { .. } => {
            read_only("conary system db-backup")
        }
        cli::DbBackupCommands::Recover { dry_run, .. } if *dry_run => {
            read_only("conary system db-backup recover --dry-run")
        }
        cli::DbBackupCommands::Recover { yes, .. } => policy_with_intent(
            "conary system db-backup recover",
            CommandRisk::ActiveHostMutation,
            false,
            *yes,
        ),
    }
}

#[derive(Clone, Copy)]
struct AdoptRiskInput {
    system: bool,
    status: bool,
    dry_run: bool,
    refresh: bool,
    convert: bool,
    sync_hook: bool,
    quiet: bool,
    from_sync_hook: bool,
}

fn classify_adopt(input: AdoptRiskInput) -> Option<CommandRiskPolicy> {
    if input.status {
        return Some(read_only("conary system adopt --status"));
    }

    if input.sync_hook {
        return Some(policy(
            "conary system adopt --sync-hook",
            CommandRisk::ActiveHostMutation,
            false,
        ));
    }

    if input.from_sync_hook && input.refresh && input.quiet {
        return Some(policy(
            "conary system adopt --refresh --quiet --from-sync-hook",
            CommandRisk::HookRefreshDbMutation,
            false,
        ));
    }

    if input.dry_run {
        let label = if input.system {
            "conary system adopt --system --dry-run"
        } else if input.refresh {
            "conary system adopt --refresh --dry-run"
        } else if input.convert {
            "conary system adopt --convert --dry-run"
        } else {
            "conary system adopt <pkg> --dry-run"
        };
        return Some(policy(label, CommandRisk::DryRunOnly, true));
    }

    let label = if input.system {
        "conary system adopt --system"
    } else if input.refresh {
        "conary system adopt --refresh"
    } else if input.convert {
        "conary system adopt --convert"
    } else {
        "conary system adopt <pkg>"
    };

    Some(policy(label, CommandRisk::DbMutation, false))
}

fn classify_state(command: &cli::StateCommands) -> CommandRiskPolicy {
    match command {
        cli::StateCommands::List { .. }
        | cli::StateCommands::Show { .. }
        | cli::StateCommands::Diff { .. } => read_only("conary system state read-only command"),
        cli::StateCommands::Revert { dry_run, yes, .. } => policy_with_intent(
            "conary system state revert",
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        ),
        cli::StateCommands::Prune { .. } => local_state("conary system state prune"),
        cli::StateCommands::Create { .. } => local_state("conary system state create"),
        cli::StateCommands::Rollback { yes, .. } => policy_with_intent(
            "conary system state rollback",
            CommandRisk::ActiveHostMutation,
            false,
            *yes,
        ),
    }
}

fn classify_generation(command: &cli::GenerationCommands) -> CommandRiskPolicy {
    match command {
        cli::GenerationCommands::List
        | cli::GenerationCommands::Export { .. }
        | cli::GenerationCommands::VerifyDbBackup { .. }
        | cli::GenerationCommands::Info { .. } => {
            read_only("conary system generation read-only command")
        }
        cli::GenerationCommands::Build { yes, .. } => policy_with_intent(
            "conary system generation build",
            CommandRisk::AlwaysLive,
            false,
            *yes,
        ),
        cli::GenerationCommands::Publish { yes, .. } => policy_with_intent(
            "conary system generation publish",
            CommandRisk::AlwaysLive,
            false,
            *yes,
        ),
        cli::GenerationCommands::Pending { .. } => read_only("conary system generation pending"),
        cli::GenerationCommands::RecoverDb { dry_run, .. } if *dry_run => {
            read_only("conary system generation recover-db --dry-run")
        }
        cli::GenerationCommands::RecoverDb { yes, .. } => policy_with_intent(
            "conary system generation recover-db",
            CommandRisk::AlwaysLive,
            false,
            *yes,
        ),
        cli::GenerationCommands::Switch { yes, .. } => policy_with_intent(
            "conary system generation switch",
            CommandRisk::AlwaysLive,
            false,
            *yes,
        ),
        cli::GenerationCommands::Rollback { yes, .. } => policy_with_intent(
            "conary system generation rollback",
            CommandRisk::AlwaysLive,
            false,
            *yes,
        ),
        cli::GenerationCommands::Gc { yes, .. } => policy_with_intent(
            "conary system generation gc",
            CommandRisk::AlwaysLive,
            false,
            *yes,
        ),
        cli::GenerationCommands::Recover { yes, .. } => policy_with_intent(
            "conary system generation recover",
            CommandRisk::AlwaysLive,
            false,
            *yes,
        ),
    }
}

fn classify_trigger(command: &cli::TriggerCommands) -> CommandRiskPolicy {
    match command {
        cli::TriggerCommands::List { .. } | cli::TriggerCommands::Show { .. } => {
            read_only("conary system trigger read-only command")
        }
        cli::TriggerCommands::Enable { .. }
        | cli::TriggerCommands::Disable { .. }
        | cli::TriggerCommands::Add { .. }
        | cli::TriggerCommands::Remove { .. } => local_state("conary system trigger"),
        cli::TriggerCommands::Run { .. } => local_state("conary system trigger run"),
    }
}

fn classify_redirect(command: &cli::RedirectCommands) -> CommandRiskPolicy {
    match command {
        cli::RedirectCommands::List { .. }
        | cli::RedirectCommands::Show { .. }
        | cli::RedirectCommands::Resolve { .. } => {
            read_only("conary system redirect read-only command")
        }
        cli::RedirectCommands::Add { .. } | cli::RedirectCommands::Remove { .. } => {
            local_state("conary system redirect")
        }
    }
}

fn classify_update_channel(action: &cli::UpdateChannelAction) -> CommandRiskPolicy {
    match action {
        cli::UpdateChannelAction::Get { .. } => read_only("conary system update-channel get"),
        cli::UpdateChannelAction::Set { .. } => local_state("conary system update-channel set"),
        cli::UpdateChannelAction::Reset { .. } => local_state("conary system update-channel reset"),
    }
}

fn classify_repo(command: &cli::RepoCommands) -> CommandRiskPolicy {
    match command {
        cli::RepoCommands::List { .. } | cli::RepoCommands::KeyList { .. } => {
            read_only("conary repo read-only command")
        }
        cli::RepoCommands::Add { .. }
        | cli::RepoCommands::Remove { .. }
        | cli::RepoCommands::ResetTrust { .. }
        | cli::RepoCommands::Enable { .. }
        | cli::RepoCommands::Disable { .. }
        | cli::RepoCommands::Sync { .. }
        | cli::RepoCommands::KeyImport { .. }
        | cli::RepoCommands::KeyRemove { .. } => local_state("conary repo"),
    }
}

fn classify_config(command: &cli::ConfigCommands) -> CommandRiskPolicy {
    match command {
        cli::ConfigCommands::List { .. }
        | cli::ConfigCommands::Diff { .. }
        | cli::ConfigCommands::Check { .. }
        | cli::ConfigCommands::Backups { .. } => read_only("conary config read-only command"),
        cli::ConfigCommands::Backup { .. } => local_state("conary config backup"),
        cli::ConfigCommands::Restore { .. } => local_state("conary config restore"),
    }
}

fn classify_distro(command: &cli::DistroCommands) -> CommandRiskPolicy {
    match command {
        cli::DistroCommands::List { .. } | cli::DistroCommands::Info { .. } => {
            read_only("conary distro read-only command")
        }
        cli::DistroCommands::Set { .. }
        | cli::DistroCommands::Remove { .. }
        | cli::DistroCommands::Mixing { .. }
        | cli::DistroCommands::SelectionMode { .. } => local_state("conary distro"),
    }
}

fn classify_registry(command: &cli::RegistryCommands) -> CommandRiskPolicy {
    match command {
        cli::RegistryCommands::Stats { .. } => read_only("conary registry stats"),
        cli::RegistryCommands::Update { .. } => local_state("conary registry update"),
    }
}

fn classify_query(command: &cli::QueryCommands) -> CommandRiskPolicy {
    match command {
        cli::QueryCommands::Label(command) => classify_label(command),
        cli::QueryCommands::Depends { .. }
        | cli::QueryCommands::Rdepends { .. }
        | cli::QueryCommands::Deptree { .. }
        | cli::QueryCommands::Whatprovides { .. }
        | cli::QueryCommands::Whatbreaks { .. }
        | cli::QueryCommands::Reason { .. }
        | cli::QueryCommands::Repquery { .. }
        | cli::QueryCommands::Component { .. }
        | cli::QueryCommands::Components { .. }
        | cli::QueryCommands::Scripts { .. }
        | cli::QueryCommands::DeltaStats { .. }
        | cli::QueryCommands::Conflicts { .. } => read_only("conary query"),
    }
}

fn classify_label(command: &cli::LabelCommands) -> CommandRiskPolicy {
    match command {
        cli::LabelCommands::List { .. }
        | cli::LabelCommands::Show { .. }
        | cli::LabelCommands::Query { .. } => read_only("conary query label read-only command"),
        cli::LabelCommands::Add { .. }
        | cli::LabelCommands::Remove { .. }
        | cli::LabelCommands::Path { .. }
        | cli::LabelCommands::Set { .. }
        | cli::LabelCommands::Link { .. }
        | cli::LabelCommands::Delegate { .. } => local_state("conary query label"),
    }
}

fn classify_ccs(command: &cli::CcsCommands) -> CommandRiskPolicy {
    match command {
        cli::CcsCommands::Install { dry_run, yes, .. } => policy_with_intent(
            "conary ccs install",
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        ),
        cli::CcsCommands::Enhance { dry_run, .. } => policy(
            "conary ccs enhance",
            CommandRisk::LocalStateMutation,
            *dry_run,
        ),
        cli::CcsCommands::Init { .. }
        | cli::CcsCommands::Build { .. }
        | cli::CcsCommands::Inspect { .. }
        | cli::CcsCommands::Verify { .. }
        | cli::CcsCommands::Sign { .. }
        | cli::CcsCommands::Keygen { .. }
        | cli::CcsCommands::Export { .. }
        | cli::CcsCommands::Shell { .. }
        | cli::CcsCommands::Run { .. } => read_only("conary ccs non-host command"),
    }
}

fn classify_derive(command: &cli::DeriveCommands) -> CommandRiskPolicy {
    match command {
        cli::DeriveCommands::List { .. }
        | cli::DeriveCommands::Show { .. }
        | cli::DeriveCommands::Build { .. }
        | cli::DeriveCommands::Stale { .. } => read_only("conary derive read-only command"),
        cli::DeriveCommands::Create { .. }
        | cli::DeriveCommands::Patch { .. }
        | cli::DeriveCommands::Override { .. }
        | cli::DeriveCommands::Delete { .. } => local_state("conary derive"),
    }
}

fn classify_model(command: &cli::ModelCommands) -> CommandRiskPolicy {
    match command {
        cli::ModelCommands::Apply { dry_run, yes, .. } => policy_with_intent(
            "conary model apply",
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        ),
        cli::ModelCommands::Diff { .. }
        | cli::ModelCommands::Check { .. }
        | cli::ModelCommands::RemoteDiff { .. }
        | cli::ModelCommands::Lock { .. } => read_only("conary model read-only command"),
        cli::ModelCommands::Snapshot { .. }
        | cli::ModelCommands::Update { .. }
        | cli::ModelCommands::Publish { .. } => local_state("conary model"),
    }
}

fn classify_collection(command: &cli::CollectionCommands) -> CommandRiskPolicy {
    match command {
        cli::CollectionCommands::List { .. } | cli::CollectionCommands::Show { .. } => {
            read_only("conary collection read-only command")
        }
        cli::CollectionCommands::Create { .. }
        | cli::CollectionCommands::Add { .. }
        | cli::CollectionCommands::Remove { .. }
        | cli::CollectionCommands::Delete { .. } => local_state("conary collection"),
    }
}

fn classify_automation(command: &cli::AutomationCommands) -> CommandRiskPolicy {
    match command {
        cli::AutomationCommands::Apply { dry_run, yes, .. } => policy_with_intent(
            "conary automation apply",
            CommandRisk::ActiveHostMutation,
            *dry_run,
            *yes,
        ),
        cli::AutomationCommands::Configure { .. } => local_state("conary automation configure"),
        cli::AutomationCommands::Status { .. }
        | cli::AutomationCommands::Check { .. }
        | cli::AutomationCommands::Daemon { .. }
        | cli::AutomationCommands::History { .. } => {
            read_only("conary automation read-only command")
        }
    }
}

fn classify_bootstrap(command: &cli::BootstrapCommands) -> CommandRiskPolicy {
    match command {
        cli::BootstrapCommands::Seed { from_adopted, .. } if *from_adopted => {
            local_state("conary bootstrap seed --from-adopted")
        }
        cli::BootstrapCommands::Init { .. }
        | cli::BootstrapCommands::Check { .. }
        | cli::BootstrapCommands::Image { .. }
        | cli::BootstrapCommands::Status { .. }
        | cli::BootstrapCommands::Resume { .. }
        | cli::BootstrapCommands::DryRun { .. }
        | cli::BootstrapCommands::Clean { .. }
        | cli::BootstrapCommands::CrossTools { .. }
        | cli::BootstrapCommands::TempTools { .. }
        | cli::BootstrapCommands::System { .. }
        | cli::BootstrapCommands::Config { .. }
        | cli::BootstrapCommands::Run { .. }
        | cli::BootstrapCommands::VerifyConvergence { .. }
        | cli::BootstrapCommands::DiffSeeds { .. }
        | cli::BootstrapCommands::Tier2 { .. }
        | cli::BootstrapCommands::GuestProfile { .. }
        | cli::BootstrapCommands::Seed { .. } => read_only("conary bootstrap non-host command"),
    }
}

fn classify_cache(command: &cli::CacheCommands) -> CommandRiskPolicy {
    match command {
        cli::CacheCommands::Status { .. } => read_only("conary cache status"),
        cli::CacheCommands::Populate { .. } => read_only("conary cache populate"),
    }
}

fn classify_provenance(command: &cli::ProvenanceCommands) -> CommandRiskPolicy {
    match command {
        cli::ProvenanceCommands::Register { .. } => local_state("conary provenance register"),
        cli::ProvenanceCommands::Show { .. }
        | cli::ProvenanceCommands::Verify { .. }
        | cli::ProvenanceCommands::Diff { .. }
        | cli::ProvenanceCommands::FindByDep { .. }
        | cli::ProvenanceCommands::Export { .. }
        | cli::ProvenanceCommands::Audit { .. } => read_only("conary provenance read-only command"),
    }
}

fn classify_trust(command: &cli::TrustCommands) -> CommandRiskPolicy {
    match command {
        cli::TrustCommands::KeyGen { .. }
        | cli::TrustCommands::Status { .. }
        | cli::TrustCommands::Verify { .. } => read_only("conary trust read-only command"),
        cli::TrustCommands::Init { .. }
        | cli::TrustCommands::Enable { .. }
        | cli::TrustCommands::Disable { .. } => local_state("conary trust"),
    }
}

fn classify_federation(command: &cli::FederationCommands) -> CommandRiskPolicy {
    match command {
        cli::FederationCommands::Status { .. }
        | cli::FederationCommands::Peers { .. }
        | cli::FederationCommands::Stats { .. }
        | cli::FederationCommands::Test { .. }
        | cli::FederationCommands::Scan { add: false, .. } => {
            read_only("conary federation read-only command")
        }
        cli::FederationCommands::AddPeer { .. }
        | cli::FederationCommands::RemovePeer { .. }
        | cli::FederationCommands::EnablePeer { .. }
        | cli::FederationCommands::DisablePeer { .. }
        | cli::FederationCommands::Scan { add: true, .. } => local_state("conary federation"),
    }
}

fn policy(command_label: &'static str, risk: CommandRisk, dry_run: bool) -> CommandRiskPolicy {
    policy_with_intent(command_label, risk, dry_run, false)
}

fn policy_with_intent(
    command_label: &'static str,
    risk: CommandRisk,
    dry_run: bool,
    apply_intent: bool,
) -> CommandRiskPolicy {
    CommandRiskPolicy {
        command_label: Cow::Borrowed(command_label),
        risk,
        dry_run,
        apply_intent,
    }
}

fn read_only(command_label: &'static str) -> CommandRiskPolicy {
    policy(command_label, CommandRisk::ReadOnly, false)
}

fn local_state(command_label: &'static str) -> CommandRiskPolicy {
    policy(command_label, CommandRisk::LocalStateMutation, false)
}

#[cfg(test)]
mod tests {
    use super::{CommandRisk, classify_cli};
    use crate::cli::Cli;
    use clap::Parser;

    fn policy(args: &[&str]) -> super::CommandRiskPolicy {
        let cli = Cli::try_parse_from(args).unwrap();
        classify_cli(&cli).expect("command should be classified")
    }

    #[test]
    fn classify_system_adopt_status_as_read_only() {
        let policy = policy(&["conary", "system", "adopt", "--status"]);
        assert_eq!(policy.risk, CommandRisk::ReadOnly);
        assert!(!policy.requires_ack());
    }

    #[test]
    fn classify_system_adopt_system_dry_run_as_dry_run_only() {
        let policy = policy(&["conary", "system", "adopt", "--system", "--dry-run"]);
        assert_eq!(policy.risk, CommandRisk::DryRunOnly);
        assert!(!policy.requires_ack());
    }

    #[test]
    fn classify_system_adopt_package_as_live_db_mutation() {
        let policy = policy(&["conary", "system", "adopt", "curl"]);
        assert_eq!(policy.risk, CommandRisk::DbMutation);
        assert!(!policy.requires_ack());
        assert_eq!(policy.command_label.as_ref(), "conary system adopt <pkg>");
    }

    #[test]
    fn classify_system_adopt_full_package_as_live_db_mutation() {
        let policy = policy(&["conary", "system", "adopt", "curl", "--full"]);
        assert_eq!(policy.risk, CommandRisk::DbMutation);
        assert!(!policy.requires_ack());
    }

    #[test]
    fn classify_system_adopt_convert_dry_run_as_dry_run_only() {
        let policy = policy(&["conary", "system", "adopt", "--convert", "--dry-run"]);
        assert_eq!(policy.risk, CommandRisk::DryRunOnly);
        assert!(!policy.requires_ack());
    }

    #[test]
    fn classify_installed_sync_hook_refresh_as_narrow_hook_refresh() {
        let policy = policy(&[
            "conary",
            "system",
            "adopt",
            "--refresh",
            "--quiet",
            "--from-sync-hook",
        ]);
        assert_eq!(policy.risk, CommandRisk::HookRefreshDbMutation);
        assert!(!policy.requires_ack());
        assert_eq!(
            policy.command_label.as_ref(),
            "conary system adopt --refresh --quiet --from-sync-hook"
        );
    }

    #[test]
    fn from_sync_hook_requires_quiet_refresh_and_rejects_full_capture() {
        assert!(
            Cli::try_parse_from(["conary", "system", "adopt", "--refresh", "--from-sync-hook"])
                .is_err()
        );

        assert!(
            Cli::try_parse_from([
                "conary",
                "system",
                "adopt",
                "--refresh",
                "--quiet",
                "--full",
                "--from-sync-hook",
            ])
            .is_err()
        );
    }

    #[test]
    fn classify_system_adopt_sync_hook_as_active_host_mutation() {
        let policy = policy(&["conary", "system", "adopt", "--sync-hook"]);
        assert_eq!(policy.risk, CommandRisk::ActiveHostMutation);
        assert!(policy.requires_ack());
    }

    #[test]
    fn classify_system_adopt_remove_hook_as_active_host_mutation() {
        let policy = policy(&["conary", "system", "adopt", "--sync-hook", "--remove-hook"]);
        assert_eq!(policy.risk, CommandRisk::ActiveHostMutation);
        assert!(policy.requires_ack());
    }

    #[test]
    fn classify_pin_and_unpin_as_local_state_mutations() {
        for args in [
            ["conary", "pin", "curl"].as_slice(),
            ["conary", "unpin", "curl"].as_slice(),
        ] {
            let policy = policy(args);
            assert_eq!(policy.risk, CommandRisk::LocalStateMutation);
            assert!(!policy.requires_ack());
        }
    }

    #[test]
    fn classify_system_init_and_repo_sync_as_local_state_mutations() {
        for args in [
            ["conary", "system", "init"].as_slice(),
            ["conary", "repo", "sync", "remi"].as_slice(),
            ["conary", "publish", "./repo"].as_slice(),
            ["conary", "new", "--from", ".", "--explain"].as_slice(),
        ] {
            let policy = policy(args);
            assert_eq!(policy.risk, CommandRisk::LocalStateMutation);
            assert!(!policy.requires_ack());
        }
    }

    #[test]
    fn classify_adoption_dry_runs_with_precise_labels() {
        let system = policy(&["conary", "system", "adopt", "--system", "--dry-run"]);
        assert_eq!(
            system.command_label.as_ref(),
            "conary system adopt --system --dry-run"
        );

        let refresh = policy(&["conary", "system", "adopt", "--refresh", "--dry-run"]);
        assert_eq!(
            refresh.command_label.as_ref(),
            "conary system adopt --refresh --dry-run"
        );

        let convert = policy(&["conary", "system", "adopt", "--convert", "--dry-run"]);
        assert_eq!(
            convert.command_label.as_ref(),
            "conary system adopt --convert --dry-run"
        );
    }

    #[test]
    fn classify_generation_publish_as_always_live() {
        let policy = policy(&["conary", "system", "generation", "publish"]);
        assert_eq!(policy.risk, CommandRisk::AlwaysLive);
        assert!(policy.requires_ack());
    }

    #[test]
    fn db_mutation_adopt_no_longer_requires_live_ack() {
        let policy = policy(&["conary", "system", "adopt", "curl"]);
        assert_eq!(policy.risk, CommandRisk::DbMutation);
        assert!(!policy.requires_apply_intent());
    }

    #[test]
    fn active_host_install_requires_apply_intent() {
        let policy = policy(&["conary", "install", "nginx"]);
        assert_eq!(policy.risk, CommandRisk::ActiveHostMutation);
        assert!(policy.requires_apply_intent());
    }

    #[test]
    fn classify_generation_pending_as_read_only() {
        let policy = policy(&["conary", "system", "generation", "pending"]);
        assert_eq!(policy.risk, CommandRisk::ReadOnly);
        assert!(!policy.requires_ack());
    }

    #[test]
    fn classify_generation_db_backup_verification_and_dry_run_recovery_as_read_only() {
        for args in [
            [
                "conary",
                "system",
                "generation",
                "verify-db-backup",
                "--current",
            ]
            .as_slice(),
            [
                "conary",
                "system",
                "generation",
                "recover-db",
                "--generation",
                "7",
                "--dry-run",
            ]
            .as_slice(),
        ] {
            let policy = policy(args);
            assert_eq!(policy.risk, CommandRisk::ReadOnly);
            assert!(!policy.requires_ack());
        }
    }

    #[test]
    fn classify_generation_db_backup_recover_apply_as_always_live() {
        let policy = policy(&[
            "conary",
            "system",
            "generation",
            "recover-db",
            "--generation",
            "7",
            "--yes",
        ]);

        assert_eq!(policy.risk, CommandRisk::AlwaysLive);
        assert!(policy.requires_ack());
        assert_eq!(
            policy.command_label.as_ref(),
            "conary system generation recover-db"
        );
    }

    #[test]
    fn classify_db_backup_inspection_as_read_only() {
        for args in [
            ["conary", "system", "db-backup", "list"].as_slice(),
            ["conary", "system", "db-backup", "verify", "--latest"].as_slice(),
            [
                "conary",
                "system",
                "db-backup",
                "recover",
                "--latest",
                "--dry-run",
            ]
            .as_slice(),
        ] {
            let policy = policy(args);
            assert_eq!(policy.risk, CommandRisk::ReadOnly);
            assert!(!policy.requires_ack());
        }
    }

    #[test]
    fn classify_db_backup_recover_apply_as_active_host_mutation() {
        let policy = policy(&[
            "conary",
            "system",
            "db-backup",
            "recover",
            "--latest",
            "--yes",
        ]);
        assert_eq!(policy.risk, CommandRisk::ActiveHostMutation);
        assert!(policy.requires_ack());
        assert_eq!(
            policy.command_label.as_ref(),
            "conary system db-backup recover"
        );
    }
}
