// apps/conary/src/cli/tests/installed.rs

use super::*;

fn command_help(command_name: &str) -> String {
    subcommand_help(command_name)
}

#[test]
fn installed_package_commands_expose_arch_selector() {
    for command_name in ["remove", "update", "pin", "unpin", "list"] {
        let help = command_help(command_name);
        assert!(
            help.contains("--arch"),
            "{command_name} help should expose --arch:\n{help}"
        );
    }
}

#[test]
fn installed_package_commands_expose_version_selector() {
    for command_name in ["update", "pin", "unpin", "list"] {
        let help = command_help(command_name);
        assert!(
            help.contains("--version"),
            "{command_name} help should expose --version:\n{help}"
        );
    }
}

#[test]
fn system_adopt_full_help_only_names_consuming_modes() {
    let help = nested_subcommand_help("system", "adopt");

    assert!(
        help.contains("Used by: default (package adopt), --system"),
        "system adopt --full help should name its consuming modes:\n{help}"
    );
    assert!(
        !help.contains("Used by: default (package adopt), --system, --refresh"),
        "system adopt --full help must not claim --refresh consumes --full:\n{help}"
    );
}

#[test]
fn export_rejects_legacy_db_argument() {
    let err = match parse_cli(["conary", "export", "--output", "oci-out", "--db", "old.db"]) {
        Ok(_) => panic!("legacy export --db argument should be rejected"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("unexpected argument '--db'"),
        "legacy export --db argument should be rejected, got {err}"
    );
}

#[test]
fn cli_accepts_allow_live_system_mutation_as_global_flag() {
    let cli = parse_cli([
        "conary",
        "--allow-live-system-mutation",
        "system",
        "generation",
        "switch",
        "7",
    ])
    .expect("global live-mutation flag should parse before nested commands");

    assert!(cli.allow_live_system_mutation);
}

#[test]
fn global_log_verbose_does_not_collide_with_command_verbose_flags() {
    let local = parse_cli(["conary", "query", "scripts", "pkg.ccs", "--verbose"])
        .expect("query scripts --verbose should parse as command verbosity");
    assert_eq!(local.log_verbose, 0);
    match local.command {
        Some(Commands::Query(QueryCommands::Scripts { verbose, .. })) => {
            assert!(verbose);
        }
        _ => panic!("expected query scripts command"),
    }

    let global = parse_cli(["conary", "--verbose", "query", "scripts", "pkg.ccs"])
        .expect("top-level --verbose should parse as log verbosity");
    assert_eq!(global.log_verbose, 1);
    match global.command {
        Some(Commands::Query(QueryCommands::Scripts { verbose, .. })) => {
            assert!(!verbose);
        }
        _ => panic!("expected query scripts command"),
    }
}

#[test]
fn cli_accepts_yes_for_remove_autoremove_and_ccs_install() {
    let remove = parse_cli(["conary", "remove", "nginx", "--yes"]).unwrap();
    assert!(matches!(
        remove.command,
        Some(Commands::Remove { yes: true, .. })
    ));

    let autoremove = parse_cli(["conary", "autoremove", "--yes"]).unwrap();
    assert!(matches!(
        autoremove.command,
        Some(Commands::Autoremove { yes: true, .. })
    ));

    let ccs = parse_cli(["conary", "ccs", "install", "pkg.ccs", "--yes"]).unwrap();
    assert!(matches!(
        ccs.command,
        Some(Commands::Ccs(crate::cli::CcsCommands::Install {
            yes: true,
            ..
        }))
    ));
}

#[test]
fn cli_accepts_yes_for_state_and_generation_apply_commands() {
    let revert = parse_cli(["conary", "system", "state", "revert", "1", "--yes"]).unwrap();
    assert!(matches!(
        revert.command,
        Some(Commands::System(crate::cli::SystemCommands::State(
            crate::cli::StateCommands::Revert { yes: true, .. }
        )))
    ));

    let build = parse_cli([
        "conary",
        "system",
        "generation",
        "build",
        "--summary",
        "after install",
        "--yes",
    ])
    .unwrap();
    assert!(matches!(
        build.command,
        Some(Commands::System(crate::cli::SystemCommands::Generation(
            crate::cli::GenerationCommands::Build { yes: true, .. }
        )))
    ));
}

#[test]
fn parses_system_unadopt_all() {
    let cli = parse_cli(["conary", "system", "unadopt", "--all"])
        .expect("system unadopt --all should parse");

    match cli.command {
        Some(Commands::System(SystemCommands::Unadopt {
            packages,
            all,
            dry_run,
            keep_hooks,
            ..
        })) => {
            assert!(packages.is_empty());
            assert!(all);
            assert!(!dry_run);
            assert!(!keep_hooks);
        }
        _ => panic!("expected system unadopt command"),
    }
}

#[test]
fn parses_system_unadopt_package_dry_run() {
    let cli = parse_cli([
        "conary",
        "system",
        "unadopt",
        "curl",
        "--dry-run",
        "--keep-hooks",
    ])
    .expect("system unadopt curl --dry-run should parse");

    match cli.command {
        Some(Commands::System(SystemCommands::Unadopt {
            packages,
            all,
            dry_run,
            keep_hooks,
            ..
        })) => {
            assert_eq!(packages, vec!["curl"]);
            assert!(!all);
            assert!(dry_run);
            assert!(keep_hooks);
        }
        _ => panic!("expected system unadopt command"),
    }
}

#[test]
fn rejects_system_unadopt_without_scope() {
    let err = match parse_cli(["conary", "system", "unadopt"]) {
        Ok(_) => panic!("system unadopt must require --all or package names"),
        Err(err) => err,
    };

    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

#[test]
fn rejects_system_unadopt_all_with_packages() {
    let err = match parse_cli(["conary", "system", "unadopt", "--all", "curl"]) {
        Ok(_) => panic!("system unadopt --all must reject package names"),
        Err(err) => err,
    };

    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn parses_system_adopt_system_dry_run_filters() {
    let cli = parse_cli([
        "conary",
        "system",
        "adopt",
        "--system",
        "--dry-run",
        "--pattern",
        "lib*",
        "--exclude",
        "kernel*",
        "--explicit-only",
    ])
    .expect("system adopt --system dry-run filters should parse");

    match cli.command {
        Some(Commands::System(SystemCommands::Adopt {
            packages,
            full,
            system,
            status,
            dry_run,
            pattern,
            exclude,
            explicit_only,
            refresh,
            convert,
            sync_hook,
            ..
        })) => {
            assert!(packages.is_empty());
            assert!(!full);
            assert!(system);
            assert!(!status);
            assert!(dry_run);
            assert_eq!(pattern.as_deref(), Some("lib*"));
            assert_eq!(exclude.as_deref(), Some("kernel*"));
            assert!(explicit_only);
            assert!(!refresh);
            assert!(!convert);
            assert!(!sync_hook);
        }
        _ => panic!("expected system adopt command"),
    }
}

#[test]
fn parses_system_adopt_refresh_quiet_from_sync_hook() {
    let cli = parse_cli([
        "conary",
        "system",
        "adopt",
        "--refresh",
        "--quiet",
        "--from-sync-hook",
    ])
    .expect("installed sync hook refresh path should parse");

    match cli.command {
        Some(Commands::System(SystemCommands::Adopt {
            packages,
            full,
            system,
            status,
            dry_run,
            refresh,
            convert,
            sync_hook,
            quiet,
            from_sync_hook,
            ..
        })) => {
            assert!(packages.is_empty());
            assert!(!full);
            assert!(!system);
            assert!(!status);
            assert!(!dry_run);
            assert!(refresh);
            assert!(!convert);
            assert!(!sync_hook);
            assert!(quiet);
            assert!(from_sync_hook);
        }
        _ => panic!("expected system adopt command"),
    }
}

#[test]
fn rejects_system_adopt_from_sync_hook_with_full() {
    let err = match parse_cli([
        "conary",
        "system",
        "adopt",
        "--refresh",
        "--quiet",
        "--from-sync-hook",
        "--full",
    ]) {
        Ok(_) => panic!("--from-sync-hook must conflict with --full"),
        Err(err) => err,
    };

    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn parses_system_adopt_convert_dry_run_jobs() {
    let cli = parse_cli([
        "conary",
        "system",
        "adopt",
        "--convert",
        "--dry-run",
        "--jobs",
        "4",
        "--no-chunking",
    ])
    .expect("system adopt --convert dry-run jobs should parse");

    match cli.command {
        Some(Commands::System(SystemCommands::Adopt {
            packages,
            convert,
            dry_run,
            jobs,
            no_chunking,
            system,
            status,
            refresh,
            sync_hook,
            ..
        })) => {
            assert!(packages.is_empty());
            assert!(convert);
            assert!(dry_run);
            assert_eq!(jobs, Some(4));
            assert!(no_chunking);
            assert!(!system);
            assert!(!status);
            assert!(!refresh);
            assert!(!sync_hook);
        }
        _ => panic!("expected system adopt command"),
    }
}

#[test]
fn parses_system_adopt_sync_hook_remove_hook() {
    let cli = parse_cli(["conary", "system", "adopt", "--sync-hook", "--remove-hook"])
        .expect("system adopt --sync-hook --remove-hook should parse");

    match cli.command {
        Some(Commands::System(SystemCommands::Adopt {
            packages,
            sync_hook,
            remove_hook,
            system,
            status,
            refresh,
            convert,
            ..
        })) => {
            assert!(packages.is_empty());
            assert!(sync_hook);
            assert!(remove_hook);
            assert!(!system);
            assert!(!status);
            assert!(!refresh);
            assert!(!convert);
        }
        _ => panic!("expected system adopt command"),
    }
}

#[test]
fn parses_system_adopt_package_dry_run_preview_surface() {
    let cli = parse_cli(["conary", "system", "adopt", "curl", "--dry-run"])
        .expect("single-package dry-run preview should parse");

    match cli.command {
        Some(Commands::System(SystemCommands::Adopt {
            packages,
            full,
            system,
            status,
            dry_run,
            refresh,
            convert,
            sync_hook,
            ..
        })) => {
            assert_eq!(packages, vec!["curl".to_string()]);
            assert!(!full);
            assert!(!system);
            assert!(!status);
            assert!(dry_run);
            assert!(!refresh);
            assert!(!convert);
            assert!(!sync_hook);
        }
        _ => panic!("expected system adopt command"),
    }
}

#[test]
fn rejects_system_adopt_package_with_refresh_mode() {
    let err = match parse_cli(["conary", "system", "adopt", "curl", "--refresh"]) {
        Ok(_) => panic!("package adopt must conflict with --refresh mode"),
        Err(err) => err,
    };

    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn rejects_system_adopt_quiet_without_refresh() {
    let err = match parse_cli(["conary", "system", "adopt", "--quiet"]) {
        Ok(_) => panic!("--quiet must remain scoped to --refresh"),
        Err(err) => err,
    };

    let rendered = err.to_string();
    assert!(
        rendered.contains("--refresh"),
        "quiet error should point users back to --refresh: {rendered}"
    );
}

#[test]
fn cli_rejects_bootstrap_image_from_generation() {
    let err = match parse_cli([
        "conary",
        "bootstrap",
        "image",
        "--from-generation",
        "output/generations/1",
    ]) {
        Ok(_) => panic!("--from-generation must be removed from bootstrap image"),
        Err(err) => err,
    };

    assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn cli_accepts_generation_export_from_explicit_path() {
    let cli = parse_cli([
        "conary",
        "system",
        "generation",
        "export",
        "--path",
        "output/generations/1",
        "--format",
        "raw",
        "--output",
        "gen1.raw",
    ])
    .expect("generation export from path should parse");

    match cli.command {
        Some(Commands::System(SystemCommands::Generation(GenerationCommands::Export {
            generation,
            path,
            format,
            output,
            size,
        }))) => {
            assert_eq!(generation, None);
            assert_eq!(path.as_deref(), Some("output/generations/1"));
            assert_eq!(format, "raw");
            assert_eq!(output, "gen1.raw");
            assert_eq!(size, None);
        }
        _ => panic!("expected system generation export command"),
    }
}

#[test]
fn cli_accepts_generation_db_backup_verification_and_recovery() {
    let verify = parse_cli([
        "conary",
        "system",
        "generation",
        "verify-db-backup",
        "--current",
    ])
    .expect("generation DB backup verification should parse");
    match verify.command {
        Some(Commands::System(SystemCommands::Generation(
            GenerationCommands::VerifyDbBackup {
                current,
                generation,
                ..
            },
        ))) => {
            assert!(current);
            assert_eq!(generation, None);
        }
        _ => panic!("expected generation verify-db-backup command"),
    }

    let recover = parse_cli([
        "conary",
        "system",
        "generation",
        "recover-db",
        "--generation",
        "7",
        "--dry-run",
    ])
    .expect("generation DB recovery dry-run should parse");
    match recover.command {
        Some(Commands::System(SystemCommands::Generation(GenerationCommands::RecoverDb {
            generation,
            dry_run,
            yes,
            ..
        }))) => {
            assert_eq!(generation, 7);
            assert!(dry_run);
            assert!(!yes);
        }
        _ => panic!("expected generation recover-db command"),
    }
}

#[test]
fn cli_rejects_generation_export_path_and_number_together() {
    let err = match parse_cli([
        "conary",
        "system",
        "generation",
        "export",
        "7",
        "--path",
        "output/generations/1",
        "--format",
        "raw",
        "--output",
        "gen.raw",
    ]) {
        Ok(_) => panic!("--path must conflict with positional generation number"),
        Err(err) => err,
    };

    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn try_package_parses() {
    let cli = parse_cli(["conary", "try", "pkg.ccs"]).expect("try package command should parse");
    match cli.command {
        Some(Commands::Try {
            target,
            activate,
            allow_irreversible,
            run,
            ..
        }) => {
            assert_eq!(target.as_deref(), Some("pkg.ccs"));
            assert!(!activate);
            assert!(!allow_irreversible);
            assert!(run.is_empty());
        }
        _ => panic!("expected try package command"),
    }

    let with_run = parse_cli(["conary", "try", "pkg.ccs", "--", "/usr/bin/hello"])
        .expect("try package run command should parse");
    match with_run.command {
        Some(Commands::Try { target, run, .. }) => {
            assert_eq!(target.as_deref(), Some("pkg.ccs"));
            assert_eq!(run, vec!["/usr/bin/hello"]);
        }
        _ => panic!("expected try package command with runner"),
    }

    let activated = parse_cli(["conary", "try", "pkg.ccs", "--activate"])
        .expect("activated try package command should parse");
    match activated.command {
        Some(Commands::Try {
            target, activate, ..
        }) => {
            assert_eq!(target.as_deref(), Some("pkg.ccs"));
            assert!(activate);
        }
        _ => panic!("expected activated try package command"),
    }

    let irreversible = parse_cli([
        "conary",
        "try",
        "pkg.ccs",
        "--allow-irreversible",
        "--activate",
    ])
    .expect("activated irreversible try package command should parse");
    match irreversible.command {
        Some(Commands::Try {
            target,
            activate,
            allow_irreversible,
            ..
        }) => {
            assert_eq!(target.as_deref(), Some("pkg.ccs"));
            assert!(activate);
            assert!(allow_irreversible);
        }
        _ => panic!("expected activated irreversible try package command"),
    }
}

#[test]
fn try_watch_parses_project_recipe_and_json_forms() {
    let default_target = parse_cli(["conary", "try", "--watch"]).unwrap();
    match default_target.command {
        Some(Commands::Try {
            target,
            watch,
            recipe,
            json,
            isolated,
            activate,
            allow_irreversible,
            run,
            ..
        }) => {
            assert_eq!(target, None);
            assert!(watch);
            assert_eq!(recipe, None);
            assert!(!json);
            assert!(!isolated);
            assert!(!activate);
            assert!(!allow_irreversible);
            assert!(run.is_empty());
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let recipe = parse_cli([
        "conary",
        "try",
        "--watch",
        ".",
        "--recipe",
        "packaging/recipe.toml",
        "--json",
        "--isolated",
    ])
    .unwrap();
    match recipe.command {
        Some(Commands::Try {
            target,
            watch,
            recipe,
            json,
            isolated,
            ..
        }) => {
            assert_eq!(target.as_deref(), Some("."));
            assert!(watch);
            assert_eq!(recipe.as_deref(), Some("packaging/recipe.toml"));
            assert!(json);
            assert!(isolated);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn try_action_words_parse() {
    for action in ["status", "rollback", "keep"] {
        let cli = parse_cli(["conary", "try", action]).expect("try action word should parse");
        match cli.command {
            Some(Commands::Try {
                target,
                activate,
                allow_irreversible,
                run,
                ..
            }) => {
                assert_eq!(target.as_deref(), Some(action));
                assert!(!activate);
                assert!(!allow_irreversible);
                assert!(run.is_empty());
            }
            _ => panic!("expected try action command"),
        }
    }
}

#[test]
fn self_update_accepts_offline_signature_verification_flags() {
    let cli = parse_cli([
        "conary",
        "self-update",
        "--verify-sha256",
        "abc123def456",
        "--verify-signature-file",
        "/tmp/conary.sig",
        "--trusted-key",
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
    ])
    .expect("offline self-update verification flags should parse");

    match cli.command {
        Some(Commands::SelfUpdate { .. }) => {}
        _ => panic!("expected self-update command"),
    }
}
