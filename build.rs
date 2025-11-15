// build.rs

use clap::{Arg, Command};
use clap_mangen::Man;
use std::env;
use std::fs;
use std::path::PathBuf;

fn build_cli() -> Command {
    Command::new("conary")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Conary Contributors")
        .about("Modern package manager with atomic operations and rollback")
        .subcommand_required(false)
        .subcommand(
            Command::new("init")
                .about("Initialize the Conary database")
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .value_name("PATH")
                        .default_value("/var/lib/conary/conary.db")
                        .help("Database path"),
                ),
        )
        .subcommand(
            Command::new("install")
                .about("Install a package from file or repository")
                .arg(Arg::new("package").required(true).help("Package file path or package name"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db")
                        .help("Database path"),
                )
                .arg(
                    Arg::new("root")
                        .short('r')
                        .long("root")
                        .default_value("/")
                        .help("Install root directory"),
                )
                .arg(
                    Arg::new("version")
                        .long("version")
                        .help("Specific version to install"),
                )
                .arg(
                    Arg::new("repo")
                        .long("repo")
                        .help("Specific repository to use"),
                )
                .arg(
                    Arg::new("dry_run")
                        .long("dry-run")
                        .action(clap::ArgAction::SetTrue)
                        .help("Dry run - show what would be installed without installing"),
                ),
        )
        .subcommand(
            Command::new("remove")
                .about("Remove an installed package")
                .arg(Arg::new("package_name").required(true).help("Package name to remove"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("query")
                .about("Query installed packages")
                .arg(Arg::new("pattern").help("Package name pattern (optional)"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("history")
                .about("Show changeset history")
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("rollback")
                .about("Rollback a changeset")
                .arg(Arg::new("changeset_id").required(true).help("Changeset ID to rollback"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                )
                .arg(
                    Arg::new("root")
                        .short('r')
                        .long("root")
                        .default_value("/")
                        .help("Install root directory"),
                ),
        )
        .subcommand(
            Command::new("verify")
                .about("Verify installed files match their stored hashes")
                .arg(Arg::new("package").help("Package name to verify (optional)"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                )
                .arg(
                    Arg::new("root")
                        .short('r')
                        .long("root")
                        .default_value("/")
                        .help("Install root directory"),
                ),
        )
        .subcommand(
            Command::new("depends")
                .about("Show dependencies of a package")
                .arg(Arg::new("package_name").required(true).help("Package name"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("rdepends")
                .about("Show reverse dependencies (what depends on this package)")
                .arg(Arg::new("package_name").required(true).help("Package name"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("whatbreaks")
                .about("Show what packages would break if this package is removed")
                .arg(Arg::new("package_name").required(true).help("Package name"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("completions")
                .about("Generate shell completion scripts")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(["bash", "zsh", "fish", "powershell"])
                        .help("Shell type"),
                ),
        )
        .subcommand(
            Command::new("repo-add")
                .about("Add a new repository")
                .arg(Arg::new("name").required(true).help("Repository name"))
                .arg(Arg::new("url").required(true).help("Repository URL"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                )
                .arg(
                    Arg::new("priority")
                        .short('p')
                        .long("priority")
                        .default_value("0")
                        .help("Priority (higher = preferred)"),
                )
                .arg(
                    Arg::new("disabled")
                        .long("disabled")
                        .action(clap::ArgAction::SetTrue)
                        .help("Disable repository after adding"),
                ),
        )
        .subcommand(
            Command::new("repo-list")
                .about("List repositories")
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                )
                .arg(
                    Arg::new("all")
                        .short('a')
                        .long("all")
                        .action(clap::ArgAction::SetTrue)
                        .help("Show all repositories (including disabled)"),
                ),
        )
        .subcommand(
            Command::new("repo-remove")
                .about("Remove a repository")
                .arg(Arg::new("name").required(true).help("Repository name"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("repo-enable")
                .about("Enable a repository")
                .arg(Arg::new("name").required(true).help("Repository name"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("repo-disable")
                .about("Disable a repository")
                .arg(Arg::new("name").required(true).help("Repository name"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("repo-sync")
                .about("Synchronize repository metadata")
                .arg(Arg::new("name").help("Repository name (syncs all if omitted)"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                )
                .arg(
                    Arg::new("force")
                        .short('f')
                        .long("force")
                        .action(clap::ArgAction::SetTrue)
                        .help("Force sync even if metadata hasn't expired"),
                ),
        )
        .subcommand(
            Command::new("search")
                .about("Search for packages in repositories")
                .arg(Arg::new("pattern").required(true).help("Search pattern"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                ),
        )
        .subcommand(
            Command::new("update")
                .about("Update installed packages from repositories")
                .arg(Arg::new("package").help("Package name (updates all if omitted)"))
                .arg(
                    Arg::new("db_path")
                        .short('d')
                        .long("db-path")
                        .default_value("/var/lib/conary/conary.db"),
                )
                .arg(
                    Arg::new("root")
                        .short('r')
                        .long("root")
                        .default_value("/")
                        .help("Install root directory"),
                ),
        )
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Create man directory
    let out_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let man_dir = out_dir.join("man");
    fs::create_dir_all(&man_dir).expect("Failed to create man directory");

    // Generate main man page
    let cmd = build_cli();
    let man = Man::new(cmd);
    let mut buffer = Vec::new();
    man.render(&mut buffer)
        .expect("Failed to render man page");

    let man_path = man_dir.join("conary.1");
    fs::write(&man_path, buffer).expect("Failed to write man page");

    println!("cargo:warning=Man page generated at {}", man_path.display());
}
