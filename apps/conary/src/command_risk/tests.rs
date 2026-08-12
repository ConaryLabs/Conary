// apps/conary/src/command_risk/tests.rs
use super::{CommandRisk, classify_cli};
use crate::cli::{self, Cli, Commands};
use clap::Parser;

fn parse_cli(args: &[&str]) -> Result<Cli, String> {
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || Cli::try_parse_from(args).map_err(|err| err.to_string()))
        .expect("parser thread should spawn")
        .join()
        .expect("parser thread should not panic")
}

fn policy(args: &[&str]) -> super::CommandRiskPolicy {
    let cli = parse_cli(args).unwrap();
    classify_cli(&cli).expect("command should be classified")
}

#[test]
fn classify_system_adopt_status_as_read_only() {
    let policy = policy(&["conary", "system", "adopt", "--status"]);
    assert_eq!(policy.risk, CommandRisk::ReadOnly);
    assert!(!policy.requires_ack());
}

#[test]
fn mcp_packaging_startup_is_read_only() {
    let cli = Cli {
        help_advanced: false,
        log_verbose: 0,
        quiet: false,
        command: Some(Commands::Mcp(cli::McpCommands::Packaging)),
    };

    let policy = classify_cli(&cli).expect("mcp packaging should have a risk policy");
    assert_eq!(policy.command_label, "conary mcp packaging");
    assert_eq!(policy.risk, CommandRisk::ReadOnly);
    assert!(!policy.dry_run);
    assert!(!policy.apply_intent);
}

#[test]
fn classify_self_update_default_as_apply_intent() {
    let policy = policy(&["conary", "self-update"]);
    assert_eq!(policy.command_label.as_ref(), "conary self-update");
    assert_eq!(policy.risk, CommandRisk::ActiveHostMutation);
    assert!(policy.requires_ack());
    assert!(policy.apply_intent);
}

#[test]
fn classify_self_update_check_and_offline_verification_as_read_only() {
    for args in [
        ["conary", "self-update", "--check"].as_slice(),
        [
            "conary",
            "self-update",
            "--verify-sha256",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "--verify-signature-file",
            "/tmp/conary.sig",
        ]
        .as_slice(),
        ["conary", "self-update", "--print-trusted-keys"].as_slice(),
    ] {
        let policy = policy(args);
        assert_eq!(policy.risk, CommandRisk::ReadOnly);
        assert!(!policy.requires_ack());
        assert!(!policy.apply_intent);
    }
}

#[test]
fn m4b_ccs_authoring_commands_are_not_active_host_mutations() {
    let init = policy(&["conary", "ccs", "init", "--template", "minimal-file"]);
    assert_eq!(init.risk, CommandRisk::LocalStateMutation);

    let lint = policy(&["conary", "ccs", "lint"]);
    assert_eq!(lint.risk, CommandRisk::ReadOnly);

    let build = policy(&["conary", "ccs", "build", "--local-dev"]);
    assert_eq!(build.risk, CommandRisk::LocalStateMutation);

    let test = policy(&["conary", "ccs", "test", "pkg.ccs", "--dry-run"]);
    assert_eq!(test.risk, CommandRisk::LocalStateMutation);
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
fn repository_takeover_preview_is_read_only_and_mutations_are_selected_root_scoped() {
    let preview = policy(&[
        "conary",
        "system",
        "repository-takeover",
        "--manifest",
        "takeover.json",
        "--dry-run",
    ]);
    assert_eq!(preview.risk, CommandRisk::SelectedRootMutation);
    assert!(preview.dry_run);
    assert!(!preview.requires_ack());

    let apply = policy(&[
        "conary",
        "system",
        "repository-takeover",
        "--manifest",
        "takeover.json",
        "--preview-sha256",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "--yes",
    ]);
    assert_eq!(apply.risk, CommandRisk::SelectedRootMutation);
    assert!(!apply.dry_run);
    assert!(apply.requires_ack());

    let rollback = policy(&[
        "conary",
        "system",
        "repository-takeover",
        "--rollback",
        "--yes",
    ]);
    assert_eq!(rollback.risk, CommandRisk::SelectedRootMutation);
    assert!(rollback.requires_ack());
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
    assert!(parse_cli(&["conary", "system", "adopt", "--refresh", "--from-sync-hook"]).is_err());

    assert!(
        parse_cli(&[
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
fn classify_try_commands_by_session_risk() {
    let namespace = policy(&["conary", "try", "pkg.ccs"]);
    assert_eq!(namespace.risk, CommandRisk::LocalStateMutation);
    assert!(!namespace.requires_ack());

    let watch = policy(&["conary", "try", "--watch"]);
    assert_eq!(watch.command_label.as_ref(), "conary try --watch");
    assert_eq!(watch.risk, CommandRisk::LocalStateMutation);
    assert!(!watch.requires_ack());

    let invalid_activated_watch = policy(&["conary", "try", "--watch", "--activate"]);
    assert_eq!(
        invalid_activated_watch.risk,
        CommandRisk::LocalStateMutation
    );
    assert!(!invalid_activated_watch.requires_ack());

    let activated = policy(&["conary", "try", "pkg.ccs", "--activate"]);
    assert_eq!(activated.risk, CommandRisk::ActiveHostMutation);
    assert!(activated.requires_ack());
    assert!(
        activated.apply_intent,
        "--activate is the explicit host-global try intent"
    );

    let status = policy(&["conary", "try", "status"]);
    assert_eq!(status.risk, CommandRisk::ReadOnly);
    assert!(!status.requires_ack());

    for args in [
        ["conary", "try", "rollback"].as_slice(),
        ["conary", "try", "keep"].as_slice(),
    ] {
        let policy = policy(args);
        assert_eq!(policy.risk, CommandRisk::ActiveHostMutation);
        assert!(policy.requires_ack());
        assert!(
            policy.apply_intent,
            "try management actions are explicit decisions"
        );
    }
}

#[test]
fn classify_cook_as_local_state_mutation_even_when_validate_only() {
    for args in [
        ["conary", "cook", "."].as_slice(),
        ["conary", "cook", ".", "--validate-only"].as_slice(),
    ] {
        let policy = policy(args);
        assert_eq!(policy.risk, CommandRisk::LocalStateMutation);
        assert!(!policy.requires_ack());
        assert_eq!(policy.command_label.as_ref(), "conary cook");
    }
}

#[test]
fn cook_record_is_local_state_mutation_like_cook() {
    let policy = policy(&["conary", "cook", "--record", ".", "--", "make"]);
    assert_eq!(policy.risk, CommandRisk::LocalStateMutation);
    assert!(!policy.requires_ack());
    assert_eq!(policy.command_label.as_ref(), "conary cook");
}

#[test]
fn classify_system_init_and_repo_sync_as_local_state_mutations() {
    for args in [
        ["conary", "system", "init"].as_slice(),
        ["conary", "repo", "sync", "remi"].as_slice(),
        ["conary", "publish", "./repo"].as_slice(),
        ["conary", "new", "demo"].as_slice(),
    ] {
        let policy = policy(args);
        assert_eq!(policy.risk, CommandRisk::LocalStateMutation);
        assert!(!policy.requires_ack());
    }
}

#[test]
fn retired_schema_rebuild_requires_explicit_discard_and_confirmation() {
    assert!(
        parse_cli(&["conary", "system", "rebuild-db", "--yes"]).is_err(),
        "the destructive semantic must be explicit"
    );

    let unconfirmed = policy(&["conary", "system", "rebuild-db", "--discard-state"]);
    assert_eq!(unconfirmed.risk, CommandRisk::DestructiveDbMutation);
    assert!(unconfirmed.requires_ack());
    assert!(!unconfirmed.apply_intent);

    let confirmed = policy(&["conary", "system", "rebuild-db", "--discard-state", "--yes"]);
    assert_eq!(confirmed.risk, CommandRisk::DestructiveDbMutation);
    assert!(confirmed.requires_ack());
    assert!(confirmed.apply_intent);
}

#[test]
fn classify_adoption_dry_runs_with_precise_labels() {
    let package = policy(&["conary", "system", "adopt", "curl", "--dry-run"]);
    assert_eq!(
        package.command_label.as_ref(),
        "conary system adopt <pkg> --dry-run"
    );
    assert_eq!(package.risk, CommandRisk::DryRunOnly);
    assert!(package.dry_run);
    assert!(!package.requires_ack());

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
