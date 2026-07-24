// apps/conary/src/cli/tests.rs

use super::{
    CcsCommands, Cli, CliSandboxMode, Commands, GenerationCommands, McpCommands, QueryCommands,
    RepoCommands, SystemCommands,
};
use clap::{CommandFactory, Parser};

fn parse_cli<const N: usize>(args: [&str; N]) -> Result<Cli, clap::Error> {
    let args = args.into_iter().map(str::to_string).collect::<Vec<_>>();

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || <Cli as Parser>::try_parse_from(args))
        .expect("parser thread should spawn")
        .join()
        .expect("parser thread should not panic")
}

fn render_help_with_stack<F>(render: F) -> String
where
    F: FnOnce() -> String + Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(render)
        .expect("help-render thread should spawn")
        .join()
        .expect("help-render thread should not panic")
}

#[test]
fn cli_accepts_seccomp_warn_flag() {
    parse_cli(["conary", "--seccomp-warn", "list"])
        .expect("--seccomp-warn should parse as a global CLI flag");
}

fn root_help() -> String {
    render_help_with_stack(|| Cli::command().render_long_help().to_string())
}

fn subcommand_help(name: &str) -> String {
    let name = name.to_string();
    render_help_with_stack(move || {
        let mut command = Cli::command();
        command
            .find_subcommand_mut(&name)
            .unwrap_or_else(|| panic!("{name} subcommand should exist"))
            .render_long_help()
            .to_string()
    })
}

fn nested_subcommand_help(parent: &str, child: &str) -> String {
    let parent = parent.to_string();
    let child = child.to_string();
    render_help_with_stack(move || {
        let mut command = Cli::command();
        command
            .find_subcommand_mut(&parent)
            .unwrap_or_else(|| panic!("{parent} subcommand should exist"))
            .find_subcommand_mut(&child)
            .unwrap_or_else(|| panic!("{parent} {child} subcommand should exist"))
            .render_long_help()
            .to_string()
    })
}

#[test]
fn hidden_authoring_surfaces_keep_command_help() {
    let root = root_help();
    assert!(root.contains("try"));
    assert!(!root.contains("\n  cook "));
    assert!(!root.contains("\n  new "));
    assert!(!root.contains("Create or infer a package recipe"));
    assert!(root.contains("Try a package artifact"));

    let cook = subcommand_help("cook");
    assert!(cook.contains("--explain"));
    assert!(!cook.contains("M1a"));

    let new = subcommand_help("new");
    assert!(new.contains("--from"));
    assert!(new.contains("--explain"));

    let try_help = subcommand_help("try");
    assert!(try_help.contains("--activate"));
    assert!(try_help.contains("--allow-irreversible"));
    assert!(try_help.contains("status"));
    assert!(try_help.contains("rollback"));
    assert!(try_help.contains("keep"));
}

#[test]
fn publish_help_exposes_attested_artifact_form() {
    let cook = subcommand_help("cook");
    assert!(!cook.contains("--hermetic"));
    assert!(!cook.contains("foreign"));

    let publish = subcommand_help("publish");
    assert!(publish.contains("[TARGET]"), "{publish}");
    assert!(publish.contains("attested CCS artifact"), "{publish}");
    assert!(
        publish.contains("Artifact-form destination target"),
        "{publish}"
    );

    let try_help = subcommand_help("try");
    assert!(try_help.contains("--watch"));
    assert!(try_help.contains("--recipe"));
    assert!(try_help.contains("--json"));
    assert!(!try_help.contains("--record"));
}

#[test]
fn cook_accepts_optional_target_and_recipe_flag() {
    assert!(parse_cli(["conary", "cook"]).is_ok());
    assert!(parse_cli(["conary", "cook", "--recipe", "recipe.toml"]).is_ok());
    assert!(parse_cli(["conary", "cook", "recipe.toml", "--isolated"]).is_ok());
}

#[test]
fn cook_accepts_hidden_m1a_compatibility_flags() {
    assert!(parse_cli(["conary", "cook", "--hermetic", "recipe.toml"]).is_ok());
    assert!(parse_cli(["conary", "cook", "--no-isolation", "recipe.toml"]).is_ok());
}

#[test]
fn cook_record_hidden_flags_parse_after_separator() {
    let cli = parse_cli([
        "conary",
        "cook",
        "--record",
        "demo-source",
        "--record-output",
        "recorded/demo",
        "--record-backend",
        "inotify",
        "--record-validate",
        "--",
        "make",
        "install",
        "DESTDIR=$CONARY_DESTDIR",
    ])
    .unwrap();

    match cli.command {
        Some(Commands::Cook {
            target,
            record,
            record_output,
            record_backend,
            record_validate,
            keep_raw_trace,
            record_unsafe_host,
            record_allow_network,
            record_command,
            ..
        }) => {
            assert_eq!(target.as_deref(), Some("demo-source"));
            assert!(record);
            assert_eq!(record_output.as_deref(), Some("recorded/demo"));
            assert_eq!(record_backend.as_deref(), Some("inotify"));
            assert!(record_validate);
            assert!(!keep_raw_trace);
            assert!(!record_unsafe_host);
            assert!(!record_allow_network);
            assert_eq!(
                record_command,
                ["make", "install", "DESTDIR=$CONARY_DESTDIR"]
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            );
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn public_cook_help_hides_record_mode_flags() {
    let help = subcommand_help("cook");
    assert!(!help.contains("--record"));
    assert!(!help.contains("--record-output"));
    assert!(!help.contains("--keep-raw-trace"));
}

#[test]
fn cook_accepts_explain() {
    let cli = parse_cli(["conary", "cook", ".", "--explain"]).unwrap();
    match cli.command {
        Some(Commands::Cook { explain, .. }) => assert!(explain),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn cook_publish_and_watch_accept_json_flags() {
    let cook = parse_cli(["conary", "cook", ".", "--json"]).unwrap();
    match cook.command {
        Some(Commands::Cook { json, .. }) => assert!(json),
        other => panic!("unexpected command: {other:?}"),
    }

    let publish = parse_cli(["conary", "publish", "dist/pkg.ccs", "./repo", "--json"]).unwrap();
    match publish.command {
        Some(Commands::Publish { json, .. }) => assert!(json),
        other => panic!("unexpected command: {other:?}"),
    }

    let watch = parse_cli(["conary", "try", "--watch", "--json"]).unwrap();
    match watch.command {
        Some(Commands::Try { watch, json, .. }) => {
            assert!(watch);
            assert!(json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn new_from_current_dir_parses_with_explain() {
    let cli = parse_cli(["conary", "new", "--from", ".", "--explain"]).unwrap();
    match cli.command {
        Some(Commands::New { from, explain, .. }) => {
            assert_eq!(from.as_deref(), Some("."));
            assert!(explain);
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn publish_project_form_parses() {
    let cli = parse_cli(["conary", "publish", "./repo", "--recipe", "recipe.toml"]).unwrap();
    let Some(Commands::Publish {
        what,
        target,
        recipe,
        refresh,
        ..
    }) = cli.command
    else {
        panic!("expected publish command");
    };

    assert_eq!(what, "./repo");
    assert_eq!(target, None);
    assert_eq!(recipe.as_deref(), Some("recipe.toml"));
    assert!(!refresh);
}

#[test]
fn publish_artifact_form_parses() {
    let cli = parse_cli(["conary", "publish", "dist/pkg.ccs", "./repo"]).unwrap();
    let Some(Commands::Publish {
        what,
        target,
        recipe,
        ..
    }) = cli.command
    else {
        panic!("expected publish command");
    };

    assert_eq!(what, "dist/pkg.ccs");
    assert_eq!(target.as_deref(), Some("./repo"));
    assert_eq!(recipe, None);
}

#[test]
fn parses_hidden_mcp_packaging_command() {
    let cli = parse_cli(["conary", "mcp", "packaging"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Mcp(McpCommands::Packaging))
    ));
}

#[test]
fn repo_add_rejects_fingerprint_with_gpg_flags_at_parse_time() {
    assert!(
        parse_cli([
            "conary",
            "repo",
            "add",
            "acme",
            "file:///tmp/repo",
            "--fingerprint",
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "--no-gpg-check",
        ])
        .is_err()
    );
}

#[test]
fn repo_reset_trust_parses() {
    let cli = parse_cli(["conary", "repo", "reset-trust", "acme"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Repo(RepoCommands::ResetTrust { .. }))
    ));
}

#[test]
fn repo_add_replace_parses_for_static_repin() {
    let cli = parse_cli([
        "conary",
        "repo",
        "add",
        "acme",
        "file:///tmp/repo",
        "--fingerprint",
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        "--replace",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Repo(RepoCommands::Add { .. }))
    ));
}

#[test]
fn repo_add_rejects_static_default_strategy_at_parse_time() {
    assert!(
        parse_cli([
            "conary",
            "repo",
            "add",
            "native",
            "https://example.invalid/repo",
            "--default-strategy",
            "static",
        ])
        .is_err()
    );
}

#[test]
fn repo_add_accepts_exact_public_remi_distro() {
    assert!(
        parse_cli([
            "conary",
            "repo",
            "add",
            "remi-fedora",
            "https://remi.example.invalid",
            "--default-strategy",
            "remi",
            "--remi-endpoint",
            "https://remi.example.invalid",
            "--remi-distro",
            "fedora-44",
        ])
        .is_ok()
    );
}

#[test]
fn repo_add_rejects_internal_remi_route_slug_at_parse_time() {
    assert!(
        parse_cli([
            "conary",
            "repo",
            "add",
            "remi-fedora",
            "https://remi.example.invalid",
            "--default-strategy",
            "remi",
            "--remi-endpoint",
            "https://remi.example.invalid",
            "--remi-distro",
            "fedora",
        ])
        .is_err()
    );
}

#[test]
fn system_init_accepts_only_exact_public_profiles() {
    for expected in ["fedora-44", "ubuntu-26.04", "arch"] {
        let cli = parse_cli(["conary", "system", "init", "--profile", expected]).unwrap();
        match cli.command {
            Some(Commands::System(SystemCommands::Init { profile, .. })) => {
                assert_eq!(profile, expected);
            }
            _ => panic!("expected system init command"),
        }
    }

    for unsupported in ["fedora", "ubuntu", "debian-13"] {
        assert!(
            parse_cli(["conary", "system", "init", "--profile", unsupported]).is_err(),
            "{unsupported} must not parse as a public profile"
        );
    }
    assert!(parse_cli(["conary", "system", "init"]).is_err());
}

#[test]
fn install_defaults_to_always_sandbox() {
    let cli = parse_cli(["conary", "install", "bash"]).unwrap();
    match cli.command {
        Some(Commands::Install { sandbox, .. }) => {
            assert_eq!(sandbox, CliSandboxMode::Always);
        }
        _ => panic!("expected install command"),
    }
}

#[test]
fn install_accepts_legacy_replay_flags_defaulting_false() {
    let cli = parse_cli(["conary", "install", "bash"]).unwrap();
    match cli.command {
        Some(Commands::Install {
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        }) => {
            assert!(!allow_legacy_replay);
            assert!(!allow_foreign_legacy_replay);
        }
        _ => panic!("expected install command"),
    }

    let cli = parse_cli([
        "conary",
        "install",
        "bash",
        "--allow-legacy-replay",
        "--allow-foreign-legacy-replay",
    ])
    .unwrap();
    match cli.command {
        Some(Commands::Install {
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        }) => {
            assert!(allow_legacy_replay);
            assert!(allow_foreign_legacy_replay);
        }
        _ => panic!("expected install command"),
    }
}

#[test]
fn install_accepts_capability_approval_flag() {
    let cli = parse_cli(["conary", "install", "htop", "--allow-capabilities"]).unwrap();
    match cli.command {
        Some(Commands::Install {
            allow_capabilities, ..
        }) => {
            assert!(allow_capabilities);
        }
        _ => panic!("expected install command"),
    }
}

#[test]
fn update_defaults_to_always_sandbox() {
    let cli = parse_cli(["conary", "update"]).unwrap();
    match cli.command {
        Some(Commands::Update { sandbox, .. }) => {
            assert_eq!(sandbox, CliSandboxMode::Always);
        }
        _ => panic!("expected update command"),
    }
}

#[test]
fn update_accepts_legacy_replay_flags_and_no_scripts_defaulting_false() {
    let cli = parse_cli(["conary", "update"]).unwrap();
    match cli.command {
        Some(Commands::Update {
            no_scripts,
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        }) => {
            assert!(!no_scripts);
            assert!(!allow_legacy_replay);
            assert!(!allow_foreign_legacy_replay);
        }
        _ => panic!("expected update command"),
    }

    let cli = parse_cli([
        "conary",
        "update",
        "bash",
        "--no-scripts",
        "--allow-legacy-replay",
        "--allow-foreign-legacy-replay",
    ])
    .unwrap();
    match cli.command {
        Some(Commands::Update {
            no_scripts,
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        }) => {
            assert!(no_scripts);
            assert!(allow_legacy_replay);
            assert!(allow_foreign_legacy_replay);
        }
        _ => panic!("expected update command"),
    }
}

#[test]
fn remove_accepts_legacy_replay_flags_defaulting_false() {
    let cli = parse_cli(["conary", "remove", "bash"]).unwrap();
    match cli.command {
        Some(Commands::Remove {
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        }) => {
            assert!(!allow_legacy_replay);
            assert!(!allow_foreign_legacy_replay);
        }
        _ => panic!("expected remove command"),
    }

    let cli = parse_cli([
        "conary",
        "remove",
        "bash",
        "--allow-legacy-replay",
        "--allow-foreign-legacy-replay",
    ])
    .unwrap();
    match cli.command {
        Some(Commands::Remove {
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        }) => {
            assert!(allow_legacy_replay);
            assert!(allow_foreign_legacy_replay);
        }
        _ => panic!("expected remove command"),
    }
}

#[test]
fn autoremove_accepts_legacy_replay_flags_defaulting_false() {
    let cli = parse_cli(["conary", "autoremove"]).unwrap();
    match cli.command {
        Some(Commands::Autoremove {
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        }) => {
            assert!(!allow_legacy_replay);
            assert!(!allow_foreign_legacy_replay);
        }
        _ => panic!("expected autoremove command"),
    }

    let cli = parse_cli([
        "conary",
        "autoremove",
        "--allow-legacy-replay",
        "--allow-foreign-legacy-replay",
    ])
    .unwrap();
    match cli.command {
        Some(Commands::Autoremove {
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        }) => {
            assert!(allow_legacy_replay);
            assert!(allow_foreign_legacy_replay);
        }
        _ => panic!("expected autoremove command"),
    }
}

#[test]
fn ccs_install_accepts_legacy_replay_flags_and_no_scripts_defaulting_false() {
    let cli = parse_cli(["conary", "ccs", "install", "fixture.ccs"]).unwrap();
    match cli.command {
        Some(Commands::Ccs(CcsCommands::Install {
            no_scripts,
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        })) => {
            assert!(!no_scripts);
            assert!(!allow_legacy_replay);
            assert!(!allow_foreign_legacy_replay);
        }
        _ => panic!("expected ccs install command"),
    }

    let cli = parse_cli([
        "conary",
        "ccs",
        "install",
        "fixture.ccs",
        "--no-scripts",
        "--allow-legacy-replay",
        "--allow-foreign-legacy-replay",
    ])
    .unwrap();
    match cli.command {
        Some(Commands::Ccs(CcsCommands::Install {
            no_scripts,
            allow_legacy_replay,
            allow_foreign_legacy_replay,
            ..
        })) => {
            assert!(no_scripts);
            assert!(allow_legacy_replay);
            assert!(allow_foreign_legacy_replay);
        }
        _ => panic!("expected ccs install command"),
    }
}

#[test]
fn update_dep_mode_omission_is_model_derived() {
    let cli = parse_cli(["conary", "update"]).unwrap();
    match cli.command {
        Some(Commands::Update { dep_mode, .. }) => {
            assert_eq!(dep_mode, None);
        }
        _ => panic!("expected update command"),
    }
}

#[test]
fn update_dep_mode_help_is_model_derived() {
    let help = subcommand_help("update");
    let hard_coded_default = ["[default: ", "satisfy]"].concat();

    assert!(
        !help.contains(&hard_coded_default),
        "update dep-mode must not hard-code satisfy as its CLI default:\n{help}"
    );
}

mod installed;
