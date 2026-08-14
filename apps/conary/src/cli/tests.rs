// apps/conary/src/cli/tests.rs

use super::{
    CcsCommands, Cli, CliSandboxMode, Commands, GenerationCommands, McpCommands,
    NativePackageManager, QueryCommands, RepoCommands, SystemCommands,
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

#[test]
fn cli_rejects_removed_seccomp_warn_bypass() {
    let error = parse_cli(["conary", "--seccomp-warn", "list"])
        .err()
        .expect("scriptlet enforcement has no warning-mode bypass");
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn cli_rejects_removed_builtin_trigger_filter() {
    let error = parse_cli(["conary", "system", "trigger", "list", "--builtin"])
        .err()
        .expect("current trigger authority has no built-in classification");
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn ccs_build_parses_only_typed_install_prefixes() {
    let cli = parse_cli(["conary", "ccs", "build", "--install-prefix", "/usr/bin"]).unwrap();
    match cli.command {
        Some(Commands::Ccs(CcsCommands::Build { install_prefix, .. })) => {
            assert_eq!(install_prefix.as_path(), std::path::Path::new("/usr/bin"));
        }
        _ => panic!("expected ccs build command"),
    }

    for invalid in ["usr/bin", "/usr/../bin", "/usr//bin", "/usr/bin/"] {
        assert!(
            parse_cli(["conary", "ccs", "build", "--install-prefix", invalid]).is_err(),
            "invalid install prefix parsed: {invalid}"
        );
    }
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
    assert!(!root.contains("Create a named package recipe scaffold"));
    assert!(root.contains("Try a package artifact"));

    let cook = subcommand_help("cook");
    assert!(!cook.contains("--explain"));
    assert!(!cook.contains("M1a"));

    let new = subcommand_help("new");
    assert!(!new.contains("--from"));
    assert!(!new.contains("--explain"));

    let try_help = subcommand_help("try");
    assert!(try_help.contains("--activate"));
    assert!(try_help.contains("--policy"));
    assert!(!try_help.contains("--allow-irreversible"));
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
fn cook_rejects_removed_isolation_aliases() {
    for flag in ["--hermetic", "--no-isolation"] {
        assert!(
            parse_cli(["conary", "cook", flag, "recipe.toml"]).is_err(),
            "{flag} must not parse"
        );
    }
}

#[test]
fn bootstrap_rejects_removed_checksum_bypass() {
    for phase in ["cross-tools", "temp-tools", "system", "tier2"] {
        let error = match parse_cli(["conary", "bootstrap", phase, "--skip-verify"]) {
            Ok(_) => panic!("bootstrap checksum verification cannot be disabled"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
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
            record_command,
            ..
        }) => {
            assert_eq!(target.as_deref(), Some("demo-source"));
            assert!(record);
            assert_eq!(record_output.as_deref(), Some("recorded/demo"));
            assert_eq!(record_backend.as_deref(), Some("inotify"));
            assert!(record_validate);
            assert!(!keep_raw_trace);
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
fn cook_record_rejects_removed_containment_bypasses() {
    for flag in ["--record-unsafe-host", "--record-allow-network"] {
        let error = parse_cli(["conary", "cook", "--record", flag, "--", "true"])
            .err()
            .expect("record containment bypass must remain unavailable");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
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
fn cook_rejects_removed_explain_flag() {
    let error = parse_cli(["conary", "cook", ".", "--explain"])
        .err()
        .expect("recipe inference trace flag must remain removed");
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
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
fn new_requires_name_and_rejects_removed_inference_flags() {
    assert!(parse_cli(["conary", "new", "demo"]).is_ok());
    assert!(parse_cli(["conary", "new"]).is_err());
    let from_error = parse_cli(["conary", "new", "demo", "--from", "."])
        .err()
        .expect("--from must remain removed");
    assert_eq!(from_error.kind(), clap::error::ErrorKind::UnknownArgument);
    let explain_error = parse_cli(["conary", "new", "demo", "--explain"])
        .err()
        .expect("--explain must remain removed");
    assert_eq!(
        explain_error.kind(),
        clap::error::ErrorKind::UnknownArgument
    );
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
fn publish_rejects_removed_repository_trust_bypasses() {
    for flag in ["--force-reinit", "--accept-destination-state"] {
        let error = parse_cli(["conary", "publish", "./repo", flag])
            .err()
            .expect("repository trust bypass must remain unavailable");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}

#[test]
fn cli_rejects_removed_heuristic_pkgbuild_converter() {
    let error = parse_cli(["conary", "convert-pkgbuild", "PKGBUILD"])
        .err()
        .expect("heuristic PKGBUILD conversion must remain unavailable");
    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
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
fn retired_native_trust_bypass_is_rejected_at_parse_time() {
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
fn retired_tuf_disable_bypass_is_rejected_at_parse_time() {
    assert!(parse_cli(["conary", "trust", "disable", "acme", "--force",]).is_err());
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
fn repo_add_accepts_exact_public_source_profile() {
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
            "--source-profile",
            "fedora-44",
        ])
        .is_ok()
    );
}

#[test]
fn repo_add_parses_uncatalogued_native_follow_policy() {
    let cli = parse_cli([
        "conary",
        "repo",
        "add",
        "widgets",
        "https://packages.example.test/widgets",
        "--package-format",
        "rpm",
        "--architecture",
        "x86_64",
        "--source-id",
        "third-party:widgets",
        "--repository-id",
        "widgets:x86_64",
        "--stream-kind",
        "channel",
        "--stream-id",
        "stable",
        "--follow",
    ])
    .unwrap();
    let Some(Commands::Repo(RepoCommands::Add { args })) = cli.command else {
        panic!("expected repo add command");
    };
    assert_eq!(args.source_profile, None);
    assert_eq!(args.source_id.as_deref(), Some("third-party:widgets"));
    assert_eq!(args.repository_id.as_deref(), Some("widgets:x86_64"));
    assert_eq!(args.stream_id.as_deref(), Some("stable"));
    assert!(args.follow);
}

#[test]
fn repo_add_native_update_choice_is_closed_and_sha256_is_canonical() {
    let both = [
        "conary",
        "repo",
        "add",
        "widgets",
        "https://packages.example.test/widgets",
        "--follow",
        "--pin-snapshot-sha256",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ];
    assert!(parse_cli(both).is_err());

    assert!(
        parse_cli([
            "conary",
            "repo",
            "add",
            "widgets",
            "https://packages.example.test/widgets",
            "--pin-snapshot-sha256",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ])
        .is_err()
    );
}

#[test]
fn repo_add_accepts_repeatable_self_hosted_ccs_package_keys() {
    let cli = parse_cli([
        "conary",
        "repo",
        "add",
        "remi-fedora",
        "https://remi.example.invalid",
        "--default-strategy",
        "remi",
        "--remi-endpoint",
        "https://remi.example.invalid",
        "--ccs-package-key",
        "/keys/targets-current.public",
        "--ccs-package-key",
        "/keys/targets-next.public",
        "--source-profile",
        "fedora-44",
    ])
    .unwrap();
    let Some(Commands::Repo(RepoCommands::Add { args })) = cli.command else {
        panic!("expected repo add command");
    };
    assert_eq!(
        args.ccs_package_keys,
        [
            std::path::PathBuf::from("/keys/targets-current.public"),
            std::path::PathBuf::from("/keys/targets-next.public"),
        ]
    );
}

#[test]
fn repo_add_rejects_internal_source_route_slug_at_parse_time() {
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
            "--source-profile",
            "fedora",
        ])
        .is_err()
    );
}

#[test]
fn system_init_has_no_host_distro_selector() {
    let cli = parse_cli(["conary", "system", "init"]).unwrap();
    match cli.command {
        Some(Commands::System(SystemCommands::Init { .. })) => {}
        _ => panic!("expected system init command"),
    }

    assert!(parse_cli(["conary", "system", "init", "--profile", "fedora-44"]).is_err());
}

#[test]
fn repository_takeover_separates_preview_apply_and_rollback_intent() {
    let preview = parse_cli([
        "conary",
        "system",
        "repository-takeover",
        "--root",
        "/selected",
        "--manifest",
        "takeover.json",
        "--dry-run",
    ])
    .unwrap();
    match preview.command {
        Some(Commands::System(SystemCommands::RepositoryTakeover {
            common,
            manifest,
            dry_run,
            rollback,
            ..
        })) => {
            assert_eq!(common.root, "/selected");
            assert_eq!(manifest.unwrap(), std::path::PathBuf::from("takeover.json"));
            assert!(dry_run);
            assert!(!rollback);
        }
        _ => panic!("expected repository takeover command"),
    }

    assert!(
        parse_cli([
            "conary",
            "system",
            "repository-takeover",
            "--manifest",
            "takeover.json",
            "--yes",
        ])
        .is_err(),
        "apply requires the exact preview digest"
    );
    assert!(
        parse_cli([
            "conary",
            "system",
            "repository-takeover",
            "--manifest",
            "takeover.json",
            "--preview-sha256",
            "00",
        ])
        .is_err(),
        "apply requires explicit intent"
    );
    assert!(
        parse_cli([
            "conary",
            "system",
            "repository-takeover",
            "--rollback",
            "--yes",
        ])
        .is_ok()
    );
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
fn removed_native_replay_opt_in_flags_are_rejected() {
    for args in [
        vec!["conary", "install", "bash", "--allow-lifecycle-execution"],
        vec!["conary", "update", "--allow-foreign-lifecycle-execution"],
        vec!["conary", "remove", "bash", "--allow-lifecycle-execution"],
        vec!["conary", "autoremove", "--allow-lifecycle-execution"],
        vec![
            "conary",
            "ccs",
            "install",
            "fixture.ccs",
            "--allow-lifecycle-execution",
        ],
    ] {
        assert!(Cli::try_parse_from(args).is_err());
    }
}

#[test]
fn install_rejects_removed_force_alias() {
    assert!(parse_cli(["conary", "install", "bash", "--force"]).is_err());
}

#[test]
fn install_rejects_removed_capability_approval_flag() {
    assert!(parse_cli(["conary", "install", "htop", "--allow-capabilities"]).is_err());
    assert!(
        parse_cli([
            "conary",
            "ccs",
            "install",
            "fixture.ccs",
            "--capability-policy",
            "policy.toml",
        ])
        .is_err()
    );
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
fn lifecycle_bypass_flag_is_rejected_on_every_mutation_surface() {
    assert!(parse_cli(["conary", "install", "bash", "--no-scripts"]).is_err());
    assert!(parse_cli(["conary", "remove", "bash", "--no-scripts"]).is_err());
    assert!(parse_cli(["conary", "update", "bash", "--no-scripts"]).is_err());
    assert!(parse_cli(["conary", "autoremove", "--no-scripts"]).is_err());
    assert!(parse_cli(["conary", "ccs", "install", "fixture.ccs", "--no-scripts"]).is_err());
    assert!(parse_cli(["conary", "automation", "apply", "--no-scripts"]).is_err());
}

#[test]
fn unprotected_sandbox_modes_are_rejected_on_every_lifecycle_surface() {
    for mode in ["never", "auto"] {
        for args in [
            vec!["conary", "install", "bash", "--sandbox", mode],
            vec!["conary", "remove", "bash", "--sandbox", mode],
            vec!["conary", "update", "bash", "--sandbox", mode],
            vec!["conary", "autoremove", "--sandbox", mode],
            vec!["conary", "ccs", "install", "fixture.ccs", "--sandbox", mode],
        ] {
            assert!(
                Cli::try_parse_from(args).is_err(),
                "--sandbox {mode} must not parse on a lifecycle command"
            );
        }
    }
}

#[test]
fn update_ownership_omission_is_model_derived() {
    let cli = parse_cli(["conary", "update"]).unwrap();
    match cli.command {
        Some(Commands::Update { ownership, .. }) => {
            assert_eq!(ownership, None);
        }
        _ => panic!("expected update command"),
    }
}

#[test]
fn update_ownership_help_is_model_derived() {
    let help = subcommand_help("update");
    let hard_coded_default = ["[default: ", "preserve]"].concat();

    assert!(
        !help.contains(&hard_coded_default),
        "update ownership must not hard-code preserve as its CLI default:\n{help}"
    );
}

mod installed;
