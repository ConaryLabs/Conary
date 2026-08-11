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
//!
//! The remaining third-party calls were audited at their exact Cargo.lock
//! checksums: landlock 0.4.5's production path is direct ruleset/file/syscall
//! machinery with no lazy global, and seccompiler 0.5.0's post-fork operation
//! is only `apply_filter` (prctl + seccomp). A dependency change deliberately
//! fails the checksum test below and requires a new source audit.

/// Sources that run between `fork()` and `exec()`.
const CHILD_PATH_SOURCES: &[(&str, &str)] = &[
    (
        "container/execution/root_setup.rs",
        include_str!("execution/root_setup.rs"),
    ),
    ("container/child_safety.rs", include_str!("child_safety.rs")),
    ("container/namespaces.rs", include_str!("namespaces.rs")),
    (
        "container/execution/process_wait.rs",
        include_str!("execution/process_wait.rs"),
    ),
    (
        "capability/enforcement/mod.rs",
        include_str!("../capability/enforcement/mod.rs"),
    ),
    (
        "capability/enforcement/landlock_enforce.rs",
        include_str!("../capability/enforcement/landlock_enforce.rs"),
    ),
    (
        "capability/enforcement/seccomp_enforce.rs",
        include_str!("../capability/enforcement/seccomp_enforce.rs"),
    ),
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
    ("Mutex", "process-global lock"),
    ("RwLock", "process-global lock"),
    ("OnceLock", "lazy process-global lock"),
    ("LazyLock", "lazy process-global lock"),
    ("lazy_static!", "lazy process-global lock"),
    ("thread_local!", "thread-local initialization"),
];

#[test]
fn child_fork_safety_forbids_logging_and_spawning_after_fork() {
    let mut violations = Vec::new();
    for (name, source) in CHILD_PATH_SOURCES {
        let production_source = source
            .split_once("\n#[cfg(test)]\nmod tests")
            .map_or(*source, |(production, _)| production);
        for (line_number, line) in production_source.lines().enumerate() {
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
fn post_fork_enforcement_dependencies_match_the_audited_sources() {
    let lockfile = include_str!("../../../../Cargo.lock");
    for audited in [
        "name = \"landlock\"\nversion = \"0.4.5\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"635839550ae8b90d9fd2571460a6645dc0aec070225956ca7a2831ed31d2795d\"",
        "name = \"seccompiler\"\nversion = \"0.5.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"a4ae55de56877481d112a559bbc12667635fdaf5e005712fd4e2b2fa50ffc884\"",
    ] {
        assert!(
            lockfile.contains(audited),
            "a post-fork enforcement dependency changed; audit its production source for locks and lazy initialization before updating this pin"
        );
    }
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
