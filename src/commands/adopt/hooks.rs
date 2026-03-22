// src/commands/adopt/hooks.rs

//! System package manager hooks for automatic Conary refresh
//!
//! Installs or removes hooks for dpkg/rpm/pacman that automatically run
//! `conary system adopt --refresh --quiet` after the system PM modifies packages.
//! This keeps Conary's tracking database in sync during the hybrid transition period.

use anyhow::Result;
use conary_core::packages::SystemPackageManager;
use std::fs;
use std::path::Path;
use tracing::info;

/// RPM file trigger filter: match all files
const RPM_FILTER_CONTENT: &str = ".*\n";

/// RPM file trigger script
const RPM_SCRIPT_CONTENT: &str = r#"#!/bin/sh
# Conary sync hook: refresh adopted package tracking after RPM transactions
/usr/bin/conary system adopt --refresh --quiet 2>/dev/null || true
"#;

/// APT post-invoke hook configuration
const APT_HOOK_CONTENT: &str = r#"// Conary sync hook: refresh adopted package tracking after APT transactions
DPkg::Post-Invoke { "/usr/bin/conary system adopt --refresh --quiet 2>/dev/null || true"; };
"#;

/// Pacman alpm-hook
const PACMAN_HOOK_CONTENT: &str = r#"[Trigger]
Operation = Install
Operation = Upgrade
Operation = Remove
Type = Package
Target = *

[Action]
Description = Refreshing Conary adopted package tracking...
When = PostTransaction
Exec = /usr/bin/conary system adopt --refresh --quiet
"#;

/// Hook file paths for each package manager
struct HookPaths {
    filter: Option<&'static str>,
    script: &'static str,
}

fn hook_paths(pkg_mgr: SystemPackageManager) -> Option<HookPaths> {
    match pkg_mgr {
        SystemPackageManager::Rpm => Some(HookPaths {
            filter: Some("/usr/lib/rpm/filetriggers/conary-sync.filter"),
            script: "/usr/lib/rpm/filetriggers/conary-sync.script",
        }),
        SystemPackageManager::Dpkg => Some(HookPaths {
            filter: None,
            script: "/etc/apt/apt.conf.d/99conary",
        }),
        SystemPackageManager::Pacman => Some(HookPaths {
            filter: None,
            script: "/etc/pacman.d/hooks/conary-sync.hook",
        }),
        SystemPackageManager::Unknown => None,
    }
}

/// Install or remove system PM sync hooks.
///
/// When `remove` is false, installs hooks so that the system PM automatically
/// calls `conary system adopt --refresh --quiet` after package transactions.
///
/// When `remove` is true, removes the previously installed hooks.
pub async fn cmd_sync_hook_install(remove: bool) -> Result<()> {
    let pkg_mgr = SystemPackageManager::detect();
    if !pkg_mgr.is_available() {
        return Err(anyhow::anyhow!(
            "No supported package manager found. Conary supports RPM, dpkg, and pacman."
        ));
    }

    let paths = hook_paths(pkg_mgr).ok_or_else(|| {
        anyhow::anyhow!(
            "No hook configuration for package manager: {}",
            pkg_mgr.display_name()
        )
    })?;

    if remove {
        // Remove hooks
        remove_file_if_exists(paths.script)?;
        if let Some(filter) = paths.filter {
            remove_file_if_exists(filter)?;
        }
        println!("Removed Conary sync hook for {}.", pkg_mgr.display_name());
    } else {
        // Install hooks
        match pkg_mgr {
            SystemPackageManager::Rpm => {
                let filter_path = paths
                    .filter
                    .ok_or_else(|| anyhow::anyhow!("RPM hook paths missing filter path"))?;
                ensure_parent_dir(filter_path)?;
                fs::write(filter_path, RPM_FILTER_CONTENT)?;
                ensure_parent_dir(paths.script)?;
                fs::write(paths.script, RPM_SCRIPT_CONTENT)?;
                // Make script executable
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(paths.script, fs::Permissions::from_mode(0o755))?;
                }
                info!(
                    "Installed RPM file trigger at {} and {}",
                    filter_path, paths.script
                );
            }
            SystemPackageManager::Dpkg => {
                ensure_parent_dir(paths.script)?;
                fs::write(paths.script, APT_HOOK_CONTENT)?;
                info!("Installed APT post-invoke hook at {}", paths.script);
            }
            SystemPackageManager::Pacman => {
                ensure_parent_dir(paths.script)?;
                fs::write(paths.script, PACMAN_HOOK_CONTENT)?;
                info!("Installed pacman hook at {}", paths.script);
            }
            _ => unreachable!(),
        }

        println!("Installed Conary sync hook for {}.", pkg_mgr.display_name());
        println!("The system PM will now auto-refresh Conary tracking after package operations.");
    }

    Ok(())
}

/// Remove a file if it exists, printing the path.
fn remove_file_if_exists(path: &str) -> Result<()> {
    if Path::new(path).exists() {
        fs::remove_file(path)?;
        println!("  Removed: {}", path);
    }
    Ok(())
}

/// Ensure the parent directory of a path exists.
fn ensure_parent_dir(path: &str) -> Result<()> {
    if let Some(parent) = Path::new(path).parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpm_hook_content_format() {
        assert!(RPM_SCRIPT_CONTENT.contains("conary system adopt --refresh --quiet"));
        assert!(RPM_SCRIPT_CONTENT.starts_with("#!/bin/sh"));
    }

    #[test]
    fn test_apt_hook_content_format() {
        assert!(APT_HOOK_CONTENT.contains("DPkg::Post-Invoke"));
        assert!(APT_HOOK_CONTENT.contains("conary system adopt --refresh --quiet"));
    }

    #[test]
    fn test_pacman_hook_content_format() {
        assert!(PACMAN_HOOK_CONTENT.contains("[Trigger]"));
        assert!(PACMAN_HOOK_CONTENT.contains("[Action]"));
        assert!(PACMAN_HOOK_CONTENT.contains("PostTransaction"));
        assert!(PACMAN_HOOK_CONTENT.contains("conary system adopt --refresh --quiet"));
    }

    #[test]
    fn test_hook_paths_rpm() {
        let paths = hook_paths(SystemPackageManager::Rpm);
        assert!(paths.is_some());
        let paths = paths.unwrap();
        assert!(paths.filter.is_some());
        assert!(paths.script.contains("conary-sync"));
    }

    #[test]
    fn test_hook_paths_dpkg() {
        let paths = hook_paths(SystemPackageManager::Dpkg);
        assert!(paths.is_some());
        let paths = paths.unwrap();
        assert!(paths.filter.is_none());
        assert!(paths.script.contains("99conary"));
    }

    #[test]
    fn test_hook_paths_pacman() {
        let paths = hook_paths(SystemPackageManager::Pacman);
        assert!(paths.is_some());
        let paths = paths.unwrap();
        assert!(paths.filter.is_none());
        assert!(paths.script.contains("conary-sync"));
    }

    #[test]
    fn test_hook_paths_unknown() {
        let paths = hook_paths(SystemPackageManager::Unknown);
        assert!(paths.is_none());
    }
}
