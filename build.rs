// build.rs

use clap::{Arg, Command};
use clap_mangen::Man;
use std::env;
use std::fs;
use std::path::PathBuf;

/// Common argument: database path
fn db_path_arg() -> Arg {
    Arg::new("db_path")
        .short('d')
        .long("db-path")
        .value_name("PATH")
        .default_value("/var/lib/conary/conary.db")
        .help("Database path")
}

/// Common argument: install root directory
fn root_arg() -> Arg {
    Arg::new("root")
        .short('r')
        .long("root")
        .default_value("/")
        .help("Install root directory")
}

fn build_cli() -> Command {
    Command::new("conary")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Conary Contributors")
        .about("Modern package manager with atomic operations and rollback")
        .subcommand_required(false)
        .subcommand(
            Command::new("init")
                .about("Initialize the Conary database")
                .arg(db_path_arg()),
        )
        .subcommand(
            Command::new("install")
                .about("Install a package from file or repository")
                .arg(Arg::new("package").required(true).help("Package file path or package name"))
                .arg(db_path_arg())
                .arg(root_arg())
                .arg(Arg::new("version").long("version").help("Specific version to install"))
                .arg(Arg::new("repo").long("repo").help("Specific repository to use"))
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
                .arg(db_path_arg())
                .arg(root_arg()),
        )
        .subcommand(
            Command::new("query")
                .about("Query installed packages")
                .arg(Arg::new("pattern").help("Package name pattern (optional)"))
                .arg(db_path_arg()),
        )
        .subcommand(
            Command::new("history")
                .about("Show changeset history")
                .arg(db_path_arg()),
        )
        .subcommand(
            Command::new("rollback")
                .about("Rollback a changeset")
                .arg(Arg::new("changeset_id").required(true).help("Changeset ID to rollback"))
                .arg(db_path_arg())
                .arg(root_arg()),
        )
        .subcommand(
            Command::new("verify")
                .about("Verify installed files match their stored hashes")
                .arg(Arg::new("package").help("Package name to verify (optional)"))
                .arg(db_path_arg())
                .arg(root_arg()),
        )
        .subcommand(
            Command::new("depends")
                .about("Show dependencies of a package")
                .arg(Arg::new("package_name").required(true).help("Package name"))
                .arg(db_path_arg()),
        )
        .subcommand(
            Command::new("rdepends")
                .about("Show reverse dependencies (what depends on this package)")
                .arg(Arg::new("package_name").required(true).help("Package name"))
                .arg(db_path_arg()),
        )
        .subcommand(
            Command::new("whatbreaks")
                .about("Show what packages would break if this package is removed")
                .arg(Arg::new("package_name").required(true).help("Package name"))
                .arg(db_path_arg()),
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
                .arg(db_path_arg())
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
                .arg(db_path_arg())
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
                .arg(db_path_arg()),
        )
        .subcommand(
            Command::new("repo-enable")
                .about("Enable a repository")
                .arg(Arg::new("name").required(true).help("Repository name"))
                .arg(db_path_arg()),
        )
        .subcommand(
            Command::new("repo-disable")
                .about("Disable a repository")
                .arg(Arg::new("name").required(true).help("Repository name"))
                .arg(db_path_arg()),
        )
        .subcommand(
            Command::new("repo-sync")
                .about("Synchronize repository metadata")
                .arg(Arg::new("name").help("Repository name (syncs all if omitted)"))
                .arg(db_path_arg())
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
                .arg(db_path_arg()),
        )
        .subcommand(
            Command::new("update")
                .about("Update installed packages from repositories")
                .arg(Arg::new("package").help("Package name (updates all if omitted)"))
                .arg(db_path_arg())
                .arg(root_arg()),
        )
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Create man directory - use CARGO_MANIFEST_DIR which is always set by cargo
    let manifest_dir = match env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(e) => {
            println!("cargo:warning=CARGO_MANIFEST_DIR not set: {}", e);
            return;
        }
    };
    let man_dir = manifest_dir.join("man");

    if let Err(e) = fs::create_dir_all(&man_dir) {
        println!("cargo:warning=Failed to create man directory: {}", e);
        return;
    }

    // Generate main man page
    let cmd = build_cli();
    let man = Man::new(cmd);
    let mut buffer = Vec::new();

    if let Err(e) = man.render(&mut buffer) {
        println!("cargo:warning=Failed to render man page: {}", e);
        return;
    }

    let man_path = man_dir.join("conary.1");
    if let Err(e) = fs::write(&man_path, buffer) {
        println!("cargo:warning=Failed to write man page: {}", e);
        return;
    }

    println!("cargo:warning=Man page generated at {}", man_path.display());
}
