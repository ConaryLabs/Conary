// apps/conary/build.rs

use clap::CommandFactory;
use clap_mangen::Man;
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

#[allow(dead_code)]
#[path = "src/commands/ccs/init_template.rs"]
mod ccs_init_template;
#[allow(dead_code)]
#[path = "src/commands/install/dep_mode.rs"]
mod dep_mode;

mod commands {
    pub use super::ccs_init_template::CcsInitTemplate;
    pub use super::dep_mode::DepMode;
}

#[allow(dead_code, unused_imports)]
#[path = "src/cli/mod.rs"]
mod cli;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cli");
    println!("cargo:rerun-if-changed=src/commands/ccs/init_template.rs");
    println!("cargo:rerun-if-changed=src/commands/install/dep_mode.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let man_dir = manifest_dir.join("man");

    fs::create_dir_all(&man_dir)?;

    let command = cli::Cli::command();
    let man = Man::new(command);
    let mut buffer = Vec::new();

    man.render(&mut buffer)?;

    let man_path = man_dir.join("conary.1");
    fs::write(&man_path, buffer)?;

    Ok(())
}
