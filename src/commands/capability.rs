// src/commands/capability.rs
//! Command implementations for package capability declarations

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use conary::capability::{
    list_packages_with_capabilities, load_capabilities_by_name, CapabilityDeclaration,
};
use conary::ccs::manifest::CcsManifest;

/// Show declared capabilities for a package
pub fn cmd_capability_show(db_path: &str, package: &str, format: &str) -> Result<()> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open database: {}", db_path))?;

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
                println!("To add capabilities, include a [capabilities] section in the package's ccs.toml.");
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
pub fn cmd_capability_validate(path: &str, verbose: bool) -> Result<()> {
    let manifest_path = Path::new(path);

    if !manifest_path.exists() {
        anyhow::bail!("File not found: {}", path);
    }

    // First validate we can parse it
    let manifest = CcsManifest::from_file(manifest_path)
        .with_context(|| format!("Failed to parse manifest: {}", path))?;

    if verbose {
        println!("Parsed manifest for: {} v{}", manifest.package.name, manifest.package.version);
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
                println!("  Network:    {} rules",
                    caps.network.outbound.len() + caps.network.listen.len() + if caps.network.none { 1 } else { 0 });
                println!("  Filesystem: {} rules",
                    caps.filesystem.read.len() + caps.filesystem.write.len() +
                    caps.filesystem.execute.len() + caps.filesystem.deny.len());
                println!("  Syscalls:   {} rules (profile: {})",
                    caps.syscalls.allow.len() + caps.syscalls.deny.len(),
                    caps.syscalls.profile.as_deref().unwrap_or("none"));
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
pub fn cmd_capability_list(db_path: &str, missing_only: bool, format: &str) -> Result<()> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open database: {}", db_path))?;

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
                println!("  {:<width$} {:12} {}", name, version, status, width = max_name_len);
            }

            println!();

            let declared_count = packages.iter().filter(|(_, _, h)| *h).count();
            let missing_count = packages.len() - declared_count;

            println!("Summary: {} declared, {} missing", declared_count, missing_count);
        }
    }

    Ok(())
}

/// Generate capability declarations by observing a binary (Phase 2 - Not yet implemented)
pub fn cmd_capability_generate(
    _binary: &str,
    _args: &[String],
    _output: Option<&str>,
    _timeout: u32,
) -> Result<()> {
    anyhow::bail!(
        "The 'capability generate' command is not yet implemented.\n\
         This feature is planned for Phase 2 of the capability system."
    )
}

/// Audit a package against its declared capabilities (Phase 2 - Not yet implemented)
pub fn cmd_capability_audit(
    _db_path: &str,
    _package: &str,
    _command: Option<&str>,
    _timeout: u32,
) -> Result<()> {
    anyhow::bail!(
        "The 'capability audit' command is not yet implemented.\n\
         This feature is planned for Phase 2 of the capability system."
    )
}

/// Run a command with capability enforcement (Phase 3 - Not yet implemented)
pub fn cmd_capability_run(
    _db_path: &str,
    _package: &str,
    _command: &[String],
    _permissive: bool,
) -> Result<()> {
    anyhow::bail!(
        "The 'capability run' command is not yet implemented.\n\
         This feature is planned for Phase 3 of the capability system,\n\
         which will add enforcement via landlock and seccomp."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary::capability::CapabilityDeclaration;

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
}
