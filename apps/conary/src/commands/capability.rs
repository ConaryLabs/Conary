// src/commands/capability.rs
//! Command implementations for package capability declarations

use super::open_db;
use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;

use conary_core::capability::enforcement::{
    EnforcementMode, EnforcementPolicy, check_enforcement_support, landlock_enforce,
    seccomp_enforce,
};
use conary_core::capability::{
    CapabilityDeclaration, SyscallCapabilities, list_packages_with_capabilities,
    load_capabilities_by_name,
};
use conary_core::ccs::manifest::CcsManifest;
use conary_core::container::{ContainerConfig, Sandbox};

const CAPABILITY_RUN_LAUNCHER_SYSCALLS: &[&str] = &[
    "read",
    "write",
    "close",
    "mmap",
    "mprotect",
    "munmap",
    "brk",
    "pread64",
    "openat",
    "newfstatat",
    "access",
    "execve",
    "exit",
    "exit_group",
    "arch_prctl",
    "rt_sigaction",
    "rt_sigprocmask",
    "futex",
    "set_tid_address",
    "set_robust_list",
    "getcwd",
    "readlink",
    "prlimit64",
    "clock_gettime",
    "madvise",
    "getrandom",
    "rseq",
];

/// Show declared capabilities for a package
pub async fn cmd_capability_show(db_path: &str, package: &str, format: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let capabilities = load_capabilities_by_name(&conn, package)?;

    match capabilities {
        Some(caps) => {
            display_capabilities(&caps, package, format)?;
        }
        None => {
            // Check if package exists but has no capabilities
            let exists: Option<i64> = conn
                .query_row(
                    "SELECT id FROM troves WHERE name = ?1 AND type = 'package'",
                    [package],
                    |row| row.get(0),
                )
                .ok();

            if exists.is_some() {
                println!("Package '{}' has no capability declarations.", package);
                println!();
                println!(
                    "To add capabilities, include a [capabilities] section in the package's ccs.toml."
                );
            } else {
                anyhow::bail!("Package '{}' not found", package);
            }
        }
    }

    Ok(())
}

/// Display capabilities in the requested format
fn display_capabilities(caps: &CapabilityDeclaration, package: &str, format: &str) -> Result<()> {
    match format {
        "json" => {
            let json = serde_json::to_string_pretty(caps)?;
            println!("{}", json);
        }
        "toml" => {
            let toml = toml::to_string_pretty(caps)?;
            println!("[capabilities]");
            println!("{}", toml);
        }
        _ => {
            // Text format
            println!("Capability Declaration for: {}", package);
            println!("Schema Version: {}", caps.version);
            println!();

            if let Some(ref rationale) = caps.rationale {
                println!("Rationale: {}", rationale);
                println!();
            }

            // Network
            if !caps.network.is_empty() {
                println!("[Network]");
                if caps.network.none {
                    println!("  No network access required");
                } else {
                    if !caps.network.outbound.is_empty() {
                        println!("  Outbound: {}", caps.network.outbound.join(", "));
                    }
                    if !caps.network.listen.is_empty() {
                        println!("  Listen:   {}", caps.network.listen.join(", "));
                    }
                }
                println!();
            }

            // Filesystem
            if !caps.filesystem.is_empty() {
                println!("[Filesystem]");
                if !caps.filesystem.read.is_empty() {
                    println!("  Read:");
                    for path in &caps.filesystem.read {
                        println!("    - {}", path);
                    }
                }
                if !caps.filesystem.write.is_empty() {
                    println!("  Write:");
                    for path in &caps.filesystem.write {
                        println!("    - {}", path);
                    }
                }
                if !caps.filesystem.execute.is_empty() {
                    println!("  Execute:");
                    for path in &caps.filesystem.execute {
                        println!("    - {}", path);
                    }
                }
                if !caps.filesystem.deny.is_empty() {
                    println!("  Deny:");
                    for path in &caps.filesystem.deny {
                        println!("    - {}", path);
                    }
                }
                println!();
            }

            // Syscalls
            if !caps.syscalls.is_empty() {
                println!("[Syscalls]");
                if let Some(ref profile) = caps.syscalls.profile {
                    println!("  Profile: {}", profile);
                }
                if !caps.syscalls.allow.is_empty() {
                    println!("  Allow: {}", caps.syscalls.allow.join(", "));
                }
                if !caps.syscalls.deny.is_empty() {
                    println!("  Deny:  {}", caps.syscalls.deny.join(", "));
                }
                println!();
            }

            if caps.is_empty() {
                println!("(No specific capabilities declared)");
            }
        }
    }

    Ok(())
}

/// Validate capability syntax in a ccs.toml manifest
pub async fn cmd_capability_validate(path: &str, verbose: bool) -> Result<()> {
    let manifest_path = Path::new(path);

    if !manifest_path.exists() {
        anyhow::bail!("File not found: {}", path);
    }

    // First validate we can parse it
    let manifest = CcsManifest::from_file(manifest_path)
        .with_context(|| format!("Failed to parse manifest: {}", path))?;

    if verbose {
        println!(
            "Parsed manifest for: {} v{}",
            manifest.package.name, manifest.package.version
        );
    }

    // Check for capabilities section
    match &manifest.capabilities {
        Some(caps) => {
            // Validate the capabilities
            caps.validate()
                .map_err(|e| anyhow::anyhow!("Validation error: {}", e))?;

            if verbose {
                println!();
                println!("Capability declaration found:");
                println!("  Version:    {}", caps.version);
                println!(
                    "  Network:    {} rules",
                    caps.network.outbound.len()
                        + caps.network.listen.len()
                        + if caps.network.none { 1 } else { 0 }
                );
                println!(
                    "  Filesystem: {} rules",
                    caps.filesystem.read.len()
                        + caps.filesystem.write.len()
                        + caps.filesystem.execute.len()
                        + caps.filesystem.deny.len()
                );
                println!(
                    "  Syscalls:   {} rules (profile: {})",
                    caps.syscalls.allow.len() + caps.syscalls.deny.len(),
                    caps.syscalls.profile.as_deref().unwrap_or("none")
                );
            }

            println!("[VALID] Capability declaration in '{}' is valid.", path);
        }
        None => {
            println!("[INFO] No [capabilities] section found in '{}'.", path);
            if verbose {
                println!();
                println!("To add capability declarations, include a section like:");
                println!();
                println!("  [capabilities]");
                println!("  version = 1");
                println!("  rationale = \"Description of why these capabilities are needed\"");
                println!();
                println!("  [capabilities.network]");
                println!("  listen = [\"80\", \"443\"]");
                println!();
                println!("  [capabilities.filesystem]");
                println!("  read = [\"/etc/myapp\"]");
                println!("  write = [\"/var/log/myapp\"]");
            }
        }
    }

    Ok(())
}

/// List packages by capability status
pub async fn cmd_capability_list(db_path: &str, missing_only: bool, format: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let packages = list_packages_with_capabilities(&conn, missing_only)?;

    if packages.is_empty() {
        if missing_only {
            println!("All packages have capability declarations.");
        } else {
            println!("No packages installed.");
        }
        return Ok(());
    }

    match format {
        "json" => {
            let json_packages: Vec<_> = packages
                .iter()
                .map(|(name, version, has_caps)| {
                    serde_json::json!({
                        "name": name,
                        "version": version,
                        "has_capabilities": has_caps
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json_packages)?);
        }
        _ => {
            // Text format
            if missing_only {
                println!("Packages missing capability declarations:");
                println!();
            } else {
                println!("Package Capability Status:");
                println!();
            }

            let max_name_len = packages.iter().map(|(n, _, _)| n.len()).max().unwrap_or(20);

            for (name, version, has_caps) in &packages {
                let status = if *has_caps { "[DECLARED]" } else { "[MISSING]" };
                println!(
                    "  {:<width$} {:12} {}",
                    name,
                    version,
                    status,
                    width = max_name_len
                );
            }

            println!();

            let declared_count = packages.iter().filter(|(_, _, h)| *h).count();
            let missing_count = packages.len() - declared_count;

            println!(
                "Summary: {} declared, {} missing",
                declared_count, missing_count
            );
        }
    }

    Ok(())
}

/// Generate capability declarations by observing a binary (Phase 2 - Not yet implemented)
pub async fn cmd_capability_generate(
    _binary: &str,
    _args: &[String],
    _output: Option<&str>,
    _timeout: u32,
) -> Result<()> {
    println!("[NOT YET IMPLEMENTED] capability generate is planned but not yet available.");
    Ok(())
}

/// Audit a package's capabilities by showing what enforcement would be applied
///
/// In audit mode, the enforcement is logged but not blocking. This lets users
/// see what restrictions would be applied before enabling enforce mode.
pub async fn cmd_capability_audit(
    db_path: &str,
    package: &str,
    _command: Option<&str>,
    _timeout: u32,
) -> Result<()> {
    let conn = open_db(db_path)?;

    let capabilities = load_capabilities_by_name(&conn, package)?;

    let caps = match capabilities {
        Some(c) => c,
        None => {
            println!("Package '{}' has no capability declarations.", package);
            println!("Nothing to audit.");
            return Ok(());
        }
    };

    // Check kernel support
    let support = check_enforcement_support();

    println!("Capability Audit for: {}", package);
    println!();

    // Kernel support status
    println!("[Kernel Support]");
    println!(
        "  Landlock: {}",
        if support.landlock {
            "supported"
        } else {
            "NOT supported"
        }
    );
    println!(
        "  Seccomp:  {}",
        if support.seccomp {
            "supported"
        } else {
            "NOT supported"
        }
    );
    println!();

    // Filesystem enforcement report
    if !caps.filesystem.is_empty() {
        println!("[Filesystem Enforcement (Landlock)]");
        let info = landlock_enforce::build_landlock_ruleset(&caps.filesystem)?;
        println!("  Read rules:    {}", info.read_rules);
        println!("  Write rules:   {}", info.write_rules);
        println!("  Execute rules: {}", info.execute_rules);
        if info.deny_conflicts > 0 {
            println!(
                "  [WARNING] {} deny paths conflict with allowed parents",
                info.deny_conflicts
            );
        }
        if !info.skipped_paths.is_empty() {
            println!("  Skipped (non-existent):");
            for path in &info.skipped_paths {
                println!("    - {}", path);
            }
        }
        println!();
    }

    // Syscall enforcement report
    if !caps.syscalls.is_empty() {
        println!("[Syscall Enforcement (Seccomp)]");
        let info = seccomp_enforce::describe_seccomp_filter(&caps.syscalls, EnforcementMode::Audit);
        if let Some(ref profile) = info.profile {
            println!("  Profile:          {}", profile);
        }
        println!("  Allowed syscalls: {}", info.allowed_count);
        println!("  Explicit denies:  {}", info.denied_explicit);
        if !info.unmapped_names.is_empty() {
            println!(
                "  Unmapped names:   {} ({})",
                info.unmapped_names.len(),
                info.unmapped_names.join(", ")
            );
        }
        println!();
    }

    // Network enforcement report
    if !caps.network.is_empty() {
        println!("[Network Enforcement]");
        if caps.network.none {
            println!("  Mode: Full network isolation (CLONE_NEWNET)");
        } else {
            println!("  Mode: Network access allowed");
            if !caps.network.outbound.is_empty() {
                println!("  Outbound ports: {}", caps.network.outbound.join(", "));
            }
            if !caps.network.listen.is_empty() {
                println!("  Listen ports:   {}", caps.network.listen.join(", "));
            }
            println!(
                "  Note: Port-level filtering requires iptables/nftables (not yet implemented)"
            );
        }
        println!();
    }

    println!("[Summary]");
    println!(
        "  To enforce these capabilities: conary capability run {} -- <command>",
        package
    );
    println!(
        "  To run in audit mode:          conary capability run --audit {} -- <command>",
        package
    );

    Ok(())
}

/// Run a command with capability enforcement
///
/// Loads the package's declared capabilities, builds an enforcement policy,
/// creates a sandboxed environment, and executes the command with restrictions.
///
/// `audit` logs violations without blocking them. Otherwise enforce mode blocks.
pub async fn cmd_capability_run(
    db_path: &str,
    package: &str,
    command: &[String],
    audit: bool,
) -> Result<()> {
    if command.is_empty() {
        anyhow::bail!("No command specified. Usage: conary capability run <package> -- <command>");
    }

    let conn = open_db(db_path)?;

    let capabilities = load_capabilities_by_name(&conn, package)?;

    let caps = match capabilities {
        Some(c) => c,
        None => {
            anyhow::bail!(
                "Package '{}' has no capability declarations.\n\
                 Add a [capabilities] section to the package's ccs.toml first.",
                package
            );
        }
    };

    let mode = if audit {
        EnforcementMode::Audit
    } else {
        EnforcementMode::Enforce
    };

    let policy = build_enforcement_policy(&caps, mode);

    // Build container config with enforcement
    let mut config = ContainerConfig::default();
    config.timeout = Duration::from_secs(3600); // generous timeout for interactive use
    config.capability_policy = Some(policy);

    // Wire network isolation from capabilities
    if caps.network.none {
        config.deny_network();
    } else if !caps.network.is_empty() {
        // Package declares specific ports but not "none" — allow network
        config.allow_network();
    }

    println!(
        "Running with {} enforcement for package '{}'",
        mode, package
    );
    if mode == EnforcementMode::Enforce {
        println!("  Violations will be blocked at the kernel level.");
    } else {
        println!("  Violations will be logged but allowed (audit mode).");
    }
    println!();

    let mut sandbox = Sandbox::new(config);
    let (program, args) = command
        .split_first()
        .expect("empty command should be rejected earlier");
    let (exit_code, stdout, stderr) = sandbox.execute_command(program, args, &[])?;

    // Print output
    if !stdout.is_empty() {
        print!("{}", stdout);
    }
    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    if exit_code != 0 {
        anyhow::bail!("Command exited with code {}", exit_code);
    }

    Ok(())
}

fn build_enforcement_policy(
    caps: &CapabilityDeclaration,
    mode: EnforcementMode,
) -> EnforcementPolicy {
    EnforcementPolicy {
        mode,
        filesystem: if caps.filesystem.is_empty() {
            None
        } else {
            Some(caps.filesystem.clone())
        },
        syscalls: if caps.syscalls.is_empty() {
            None
        } else {
            Some(with_runtime_launcher_syscalls(&caps.syscalls))
        },
        network_isolation: caps.network.none,
    }
}

fn with_runtime_launcher_syscalls(syscalls: &SyscallCapabilities) -> SyscallCapabilities {
    let mut merged = syscalls.clone();
    for syscall in CAPABILITY_RUN_LAUNCHER_SYSCALLS {
        if !merged.allow.iter().any(|existing| existing == syscall) {
            merged.allow.push((*syscall).to_string());
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::capability::CapabilityDeclaration;

    #[test]
    fn test_display_capabilities_text() {
        let mut caps = CapabilityDeclaration::default();
        caps.network.listen.push("80".to_string());
        caps.filesystem.read.push("/etc".to_string());

        // Just verify it doesn't panic
        display_capabilities(&caps, "test-pkg", "text").unwrap();
    }

    #[test]
    fn test_display_capabilities_json() {
        let caps = CapabilityDeclaration::default();
        display_capabilities(&caps, "test-pkg", "json").unwrap();
    }

    #[test]
    fn test_display_capabilities_toml() {
        let caps = CapabilityDeclaration::default();
        display_capabilities(&caps, "test-pkg", "toml").unwrap();
    }

    #[test]
    fn test_build_enforcement_policy_uses_declared_restrictions() {
        let mut caps = CapabilityDeclaration::default();
        caps.network.none = true;
        caps.filesystem.read.push("/etc/test".to_string());
        caps.syscalls.profile = Some("scriptlet".to_string());

        let policy = build_enforcement_policy(&caps, EnforcementMode::Enforce);

        assert_eq!(policy.mode, EnforcementMode::Enforce);
        assert!(policy.network_isolation);
        assert_eq!(
            policy
                .filesystem
                .as_ref()
                .expect("filesystem policy should be present")
                .read,
            vec!["/etc/test".to_string()]
        );
        assert_eq!(
            policy
                .syscalls
                .as_ref()
                .expect("syscall policy should be present")
                .profile
                .as_deref(),
            Some("scriptlet")
        );
    }

    #[test]
    fn test_build_enforcement_policy_adds_runtime_launcher_baseline() {
        let mut caps = CapabilityDeclaration::default();
        caps.syscalls.allow.push("socket".to_string());

        let policy = build_enforcement_policy(&caps, EnforcementMode::Enforce);
        let syscalls = policy
            .syscalls
            .as_ref()
            .expect("syscall policy should be present");

        assert!(syscalls.allow.contains(&"socket".to_string()));
        assert!(syscalls.allow.contains(&"execve".to_string()));
        assert!(syscalls.allow.contains(&"prlimit64".to_string()));
        assert!(syscalls.allow.contains(&"clock_gettime".to_string()));
    }
}
