// conary-core/src/container/child_fork_safety.rs

//! The post-fork child path may not log or spawn.
//!
//! `fork()` gives the child the parent's lock state with none of its other
//! threads. glibc re-initializes its own primitives in the child, so allocation
//! survives; Rust-level locks do not. `tracing`'s dispatcher is a global
//! `RwLock`, so a child that logs while another thread held that lock at fork
//! time never returns, and the parent reports `Script timed out after ...`.
//!
//! That is not a hypothetical shape. `setup_mount_namespace` logged through
//! `debug!` every time a bind-mount source was absent, which is most sandbox
//! starts, and the CLI forks from a `#[tokio::main]` runtime with worker
//! threads logging alongside it.
//!
//! Reviewing for this is unreliable, because the offending call looks like
//! ordinary logging. So it is checked instead: the child-path sources are
//! compiled into the test binary and scanned.

/// Sources that run between `fork()` and `exec()`.
const CHILD_PATH_SOURCES: &[(&str, &str)] = &[
    (
        "container/execution/root_setup.rs",
        include_str!("execution/root_setup.rs"),
    ),
    ("container/child_safety.rs", include_str!("child_safety.rs")),
];

/// Constructs that take a Rust-level lock or start a process.
///
/// `Command::new` itself is allowed: the child's final act is `command.exec()`,
/// which is `execve` and is the point of the whole path. What is banned is
/// *running* a second process from the child, which drags the full
/// `std::process` machinery through inherited lock state.
const FORBIDDEN: &[(&str, &str)] = &[
    ("warn!(", "tracing macro"),
    ("debug!(", "tracing macro"),
    ("info!(", "tracing macro"),
    ("trace!(", "tracing macro"),
    ("error!(", "tracing macro"),
    ("println!(", "buffered stdio macro"),
    ("eprintln!(", "buffered stdio macro"),
    (".status()", "subprocess execution"),
    (".spawn()", "subprocess execution"),
    (".output()", "subprocess execution"),
];

#[test]
fn child_fork_safety_forbids_logging_and_spawning_after_fork() {
    let mut violations = Vec::new();
    for (name, source) in CHILD_PATH_SOURCES {
        for (line_number, line) in source.lines().enumerate() {
            let code = line.trim_start();
            // Doc comments and ordinary comments describe the rule; they do not
            // execute it.
            if code.starts_with("//") {
                continue;
            }
            for (needle, why) in FORBIDDEN {
                if code.contains(needle) {
                    violations.push(format!(
                        "{name}:{}: {needle} ({why}) is not fork-safe in the post-fork child",
                        line_number + 1
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "post-fork child path is not fork-safe:\n  {}\n\nUse container::child_safety::child_diag \
         for diagnostics, and a direct syscall instead of a subprocess.",
        violations.join("\n  ")
    );
}

#[test]
fn format_int_renders_without_allocating() {
    use super::child_safety::{format_int, int_buffer};

    let mut buffer = int_buffer();
    assert_eq!(format_int(&mut buffer, 0), b"0");
    let mut buffer = int_buffer();
    assert_eq!(format_int(&mut buffer, 7), b"7");
    let mut buffer = int_buffer();
    assert_eq!(format_int(&mut buffer, 12345), b"12345");
    let mut buffer = int_buffer();
    assert_eq!(format_int(&mut buffer, -13), b"-13");
    // errno values arrive as i32 but the widest input the buffer must survive
    // is i64::MIN, whose magnitude does not fit in i64.
    let mut buffer = int_buffer();
    assert_eq!(format_int(&mut buffer, i64::MIN), b"-9223372036854775808");
    let mut buffer = int_buffer();
    assert_eq!(format_int(&mut buffer, i64::MAX), b"9223372036854775807");
}
