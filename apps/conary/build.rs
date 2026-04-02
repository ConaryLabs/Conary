// apps/conary/build.rs

use clap::CommandFactory;
use clap_mangen::Man;
use std::env;
use std::fs;
use std::path::PathBuf;

#[allow(dead_code)]
#[path = "src/commands/install/dep_mode.rs"]
mod dep_mode;

mod commands {
    pub use super::dep_mode::DepMode;
}

#[allow(dead_code, unused_imports)]
#[path = "src/cli/mod.rs"]
mod cli;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cli");
    println!("cargo:rerun-if-changed=src/commands/install/dep_mode.rs");

    let manifest_dir = match env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(error) => {
            println!("cargo:warning=CARGO_MANIFEST_DIR not set: {error}");
            return;
        }
    };
    let man_dir = manifest_dir.join("man");

    if let Err(error) = fs::create_dir_all(&man_dir) {
        println!("cargo:warning=Failed to create man directory: {error}");
        return;
    }

    let command = cli::Cli::command();
    let man = Man::new(command);
    let mut buffer = Vec::new();

    if let Err(error) = man.render(&mut buffer) {
        println!("cargo:warning=Failed to render man page: {error}");
        return;
    }

    let man_path = man_dir.join("conary.1");
    if let Err(error) = fs::write(&man_path, buffer) {
        println!("cargo:warning=Failed to write man page: {error}");
    }
}
