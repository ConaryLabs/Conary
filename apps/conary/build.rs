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
#[path = "src/commands/install/ownership_mode.rs"]
mod ownership_mode;

mod commands {
    pub use super::ccs_init_template::CcsInitTemplate;
    pub use super::ownership_mode::OwnershipMode;
}

#[allow(dead_code, unused_imports)]
#[path = "src/cli/mod.rs"]
mod cli;

fn trim_roff_line_endings(rendered: Vec<u8>) -> Vec<u8> {
    let mut normalized = Vec::with_capacity(rendered.len());

    for line in rendered.split_inclusive(|byte| *byte == b'\n') {
        let has_newline = line.last() == Some(&b'\n');
        let body_end = line.len() - usize::from(has_newline);
        let mut trimmed_end = body_end;

        while trimmed_end > 0 && matches!(line[trimmed_end - 1], b' ' | b'\t') {
            trimmed_end -= 1;
        }

        normalized.extend_from_slice(&line[..trimmed_end]);
        if has_newline {
            normalized.push(b'\n');
        }
    }

    normalized
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cli");
    println!("cargo:rerun-if-changed=src/commands/ccs/init_template.rs");
    println!("cargo:rerun-if-changed=src/commands/install/ownership_mode.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let man_dir = manifest_dir.join("man");

    fs::create_dir_all(&man_dir)?;

    let command = cli::Cli::command();
    let man = Man::new(command);
    let mut buffer = Vec::new();

    man.render(&mut buffer)?;

    let man_path = man_dir.join("conary.1");
    fs::write(&man_path, trim_roff_line_endings(buffer))?;

    Ok(())
}
