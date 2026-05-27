// apps/conary/tests/query_scripts.rs

use clap::Parser;
use conary::cli::{Cli, Commands, QueryCommands};

fn parse_query_scripts(args: &[&str]) -> QueryCommands {
    let cli = Cli::try_parse_from(args).expect("parse CLI");
    match cli.command.expect("command") {
        Commands::Query(command @ QueryCommands::Scripts { .. }) => command,
        _ => panic!("expected query scripts command"),
    }
}

#[test]
fn query_scripts_accepts_verbose_flag() {
    let command = parse_query_scripts(&["conary", "query", "scripts", "nginx.ccs", "--verbose"]);

    match command {
        QueryCommands::Scripts {
            package_path,
            verbose,
            entry,
            json,
        } => {
            assert_eq!(package_path, "nginx.ccs");
            assert!(verbose);
            assert_eq!(entry, None);
            assert!(!json);
        }
        _ => panic!("expected query scripts command"),
    }
}

#[test]
fn query_scripts_accepts_entry_filter() {
    let command = parse_query_scripts(&[
        "conary",
        "query",
        "scripts",
        "nginx.ccs",
        "--entry",
        "rpm:%post",
    ]);

    match command {
        QueryCommands::Scripts {
            package_path,
            verbose,
            entry,
            json,
        } => {
            assert_eq!(package_path, "nginx.ccs");
            assert!(!verbose);
            assert_eq!(entry.as_deref(), Some("rpm:%post"));
            assert!(!json);
        }
        _ => panic!("expected query scripts command"),
    }
}

#[test]
fn query_scripts_accepts_json_flag() {
    let command = parse_query_scripts(&["conary", "query", "scripts", "nginx.ccs", "--json"]);

    match command {
        QueryCommands::Scripts {
            package_path,
            verbose,
            entry,
            json,
        } => {
            assert_eq!(package_path, "nginx.ccs");
            assert!(!verbose);
            assert_eq!(entry, None);
            assert!(json);
        }
        _ => panic!("expected query scripts command"),
    }
}
