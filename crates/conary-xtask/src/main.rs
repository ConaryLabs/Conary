// crates/conary-xtask/src/main.rs

mod line_cap;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("ERROR: missing command (expected: line-cap)");
        return ExitCode::from(2);
    };

    let result = match command.as_str() {
        "line-cap" => line_cap::run(args),
        "-h" | "--help" => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        _ => Err(format!("unknown command: {command}")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ERROR: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    println!("Usage: cargo run -q -p conary-xtask -- line-cap [options]");
}
