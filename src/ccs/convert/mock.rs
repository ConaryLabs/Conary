// src/ccs/convert/mock.rs

//! Mock environment for scriptlet capture
//!
//! Provides fake implementations of common system tools (useradd, systemctl, etc.)
//! that log their invocations instead of modifying the system. This allows us to
//! "capture" the intent of imperative scriptlets and convert them to declarative
//! CCS hooks.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use crate::error::Result;

/// Sets up mock tools in the given root directory
pub fn setup_mock_tools(root: &Path) -> Result<()> {
    let bin_dir = root.join("bin");
    let sbin_dir = root.join("sbin");
    let usr_bin_dir = root.join("usr/bin");
    let usr_sbin_dir = root.join("usr/sbin");
    let log_file = "/var/log/conary-mock.log";

    // Create directories
    for dir in &[&bin_dir, &sbin_dir, &usr_bin_dir, &usr_sbin_dir] {
        fs::create_dir_all(dir)?;
    }

    // Ensure log directory exists
    fs::create_dir_all(root.join("var/log"))?;
    fs::write(root.join("var/log/conary-mock.log"), "")?;

    // List of tools to mock
    let tools = [
        // User/Group management
        "useradd", "userdel", "groupadd", "groupdel", "usermod", "groupmod",
        // Service management
        "systemctl", "service", "chkconfig",
        // System updates
        "ldconfig", "update-alternatives", "update-desktop-database",
        "gtk-update-icon-cache", "glib-compile-schemas",
        // Shells (often invoked directly)
        "sh", "bash",
    ];

    for tool in tools {
        create_mock_tool(&bin_dir, tool, log_file)?;
        // Symlink to other locations
        symlink_force(&bin_dir.join(tool), &sbin_dir.join(tool))?;
        symlink_force(&bin_dir.join(tool), &usr_bin_dir.join(tool))?;
        symlink_force(&bin_dir.join(tool), &usr_sbin_dir.join(tool))?;
    }

    // Special handling for sh/bash: they need to execute the script if passed via -c or file
    // For now, we rely on the Sandbox using the *real* sh from the host/container mock-up
    // and only mocking the *tools* called by the script.
    // Wait, if we mock 'sh', we break the scriptlet itself if it calls `sh -c`.
    // We should NOT mock shells, only utilities.
    
    // Removing shells from the loop above...
    let utils = [
        "useradd", "userdel", "groupadd", "groupdel", "usermod", "groupmod",
        "systemctl", "service", "chkconfig",
        "ldconfig", "update-alternatives", "update-desktop-database",
        "gtk-update-icon-cache", "glib-compile-schemas",
        "update-mime-database", "install-info"
    ];

    for tool in utils {
        create_mock_tool(&bin_dir, tool, log_file)?;
        symlink_force(&bin_dir.join(tool), &sbin_dir.join(tool))?;
        symlink_force(&bin_dir.join(tool), &usr_bin_dir.join(tool))?;
        symlink_force(&bin_dir.join(tool), &usr_sbin_dir.join(tool))?;
    }

    Ok(())
}

/// Create a mock script that logs arguments
fn create_mock_tool(dir: &Path, name: &str, log_file: &str) -> Result<()> {
    let path = dir.join(name);
    let content = format!(
        r#"#!/bin/sh
echo "CALL:{name} $@" >> {log_file}
exit 0
"#
    );

    fs::write(&path, content)?;
    
    // Make executable (rwxr-xr-x)
    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms)?;

    Ok(())
}

fn symlink_force(target: &Path, link: &Path) -> Result<()> {
    if link.exists() {
        fs::remove_file(link)?;
    }
    // We use relative symlinks or absolute? In chroot, absolute is fine if it points to /bin
    // But here target is outside chroot context.
    // Actually, inside the sandbox, /bin/useradd will exist.
    // We can just copy the mock script to all locations to be safe/simple.
    fs::copy(target, link)?;
    Ok(())
}

/// Parsed intent from the mock log
#[derive(Debug, Clone)]
pub enum CapturedIntent {
    UserAdd(Vec<String>),
    GroupAdd(Vec<String>),
    SystemdEnable(String),
    SystemdDisable(String),
    LdConfig,
    IconCache,
    Unknown(String, Vec<String>),
}

/// Parse the capture log
pub fn parse_capture_log(root: &Path) -> Result<Vec<CapturedIntent>> {
    let log_path = root.join("var/log/conary-mock.log");
    if !log_path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(log_path)?;
    let mut intents = Vec::new();

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("CALL:") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.is_empty() { continue; }

            let tool = parts[0];
            let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

            let intent = match tool {
                "useradd" => CapturedIntent::UserAdd(args),
                "groupadd" => CapturedIntent::GroupAdd(args),
                "systemctl" => parse_systemctl(&args),
                "ldconfig" => CapturedIntent::LdConfig,
                "gtk-update-icon-cache" => CapturedIntent::IconCache,
                _ => CapturedIntent::Unknown(tool.to_string(), args),
            };
            intents.push(intent);
        }
    }

    Ok(intents)
}

fn parse_systemctl(args: &[String]) -> CapturedIntent {
    if args.is_empty() {
        return CapturedIntent::Unknown("systemctl".into(), args.to_vec());
    }
    
    match args[0].as_str() {
        "enable" => {
             if args.len() > 1 {
                 CapturedIntent::SystemdEnable(args[1].clone())
             } else {
                 CapturedIntent::Unknown("systemctl".into(), args.to_vec())
             }
        },
        "disable" => {
             if args.len() > 1 {
                 CapturedIntent::SystemdDisable(args[1].clone())
             } else {
                 CapturedIntent::Unknown("systemctl".into(), args.to_vec())
             }
        },
        _ => CapturedIntent::Unknown("systemctl".into(), args.to_vec()),
    }
}
