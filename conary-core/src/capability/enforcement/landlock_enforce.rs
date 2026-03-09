// conary-core/src/capability/enforcement/landlock_enforce.rs
//! Landlock LSM filesystem enforcement
//!
//! Converts filesystem capability declarations into landlock rulesets that
//! restrict the process to only the declared paths. Landlock is deny-by-default:
//! once a ruleset is applied, any filesystem access not explicitly allowed is blocked.
//!
//! ## Limitations
//!
//! - Landlock cannot deny a specific path under an allowed parent (e.g., deny
//!   `/etc/shadow` when `/etc` is readable). Deny paths that conflict with
//!   allowed parents generate a warning.
//! - Non-existent paths are silently skipped (the package may not be fully
//!   installed yet when capabilities are checked).
//! - Requires Linux 5.13+ (kernel landlock support).

use super::{EnforcementError, EnforcementMode};
use crate::capability::FilesystemCapabilities;
use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus, path_beneath_rules,
};
use tracing::{debug, warn};

/// Apply landlock filesystem restrictions based on declared capabilities
///
/// Builds a landlock ruleset from the capability declaration and applies it
/// to the current process. After this call, filesystem access is restricted
/// to only the declared paths.
pub fn apply_landlock_rules(
    caps: &FilesystemCapabilities,
    mode: EnforcementMode,
) -> Result<(), EnforcementError> {
    if !check_landlock_support() {
        if mode == EnforcementMode::Enforce {
            return Err(EnforcementError::Unsupported {
                feature: "landlock".to_string(),
            });
        }
        warn!("Landlock not supported by kernel, skipping filesystem enforcement");
        return Ok(());
    }

    // Use V3 ABI (Linux 6.2+) with best-effort compatibility for older kernels
    let abi = ABI::V3;

    // Pre-filter paths to only existing ones (skip non-existent gracefully)
    let read_paths = filter_existing_paths(&caps.read, "read");
    let write_paths = filter_existing_paths(&caps.write, "write");
    let execute_paths = filter_existing_paths(&caps.execute, "execute");

    // Warn about deny paths that overlap with allowed parents
    check_deny_conflicts(caps);

    // Build and apply the ruleset in a fluent chain
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| EnforcementError::Landlock(format!("Failed to handle access: {e}")))?
        .create()
        .map_err(|e| EnforcementError::Landlock(format!("Failed to create ruleset: {e}")))?
        .set_compatibility(CompatLevel::BestEffort)
        .add_rules(path_beneath_rules(&read_paths, AccessFs::from_read(abi)))
        .map_err(|e| EnforcementError::Landlock(format!("Failed to add read rules: {e}")))?
        .add_rules(path_beneath_rules(&write_paths, AccessFs::from_all(abi)))
        .map_err(|e| EnforcementError::Landlock(format!("Failed to add write rules: {e}")))?
        .add_rules(path_beneath_rules(
            &execute_paths,
            AccessFs::from_read(abi) | AccessFs::Execute,
        ))
        .map_err(|e| EnforcementError::Landlock(format!("Failed to add execute rules: {e}")))?
        .restrict_self()
        .map_err(|e| EnforcementError::Landlock(format!("Failed to restrict self: {e}")))?;

    match status.ruleset {
        RulesetStatus::FullyEnforced => {
            debug!("Landlock fully enforced");
        }
        RulesetStatus::PartiallyEnforced => {
            warn!("Landlock partially enforced (kernel may not support all requested features)");
        }
        RulesetStatus::NotEnforced => {
            if mode == EnforcementMode::Enforce {
                return Err(EnforcementError::Landlock(
                    "Landlock ruleset was not enforced by kernel".to_string(),
                ));
            }
            warn!("Landlock not enforced by kernel");
        }
    }

    Ok(())
}

/// Filter a list of paths down to only those that exist on the filesystem
fn filter_existing_paths<'a>(paths: &'a [String], context: &str) -> Vec<&'a str> {
    paths
        .iter()
        .filter(|p| {
            let exists = std::path::Path::new(p.as_str()).exists();
            if !exists {
                debug!("Skipping non-existent {} path: {}", context, p);
            }
            exists
        })
        .map(String::as_str)
        .collect()
}

/// Count paths that exist, collecting non-existent ones into `skipped`
fn count_existing_paths(paths: &[String], skipped: &mut Vec<String>) -> usize {
    let mut count = 0;
    for path in paths {
        if std::path::Path::new(path).exists() {
            count += 1;
        } else {
            skipped.push(path.clone());
        }
    }
    count
}

/// Find deny paths that conflict with allowed parents.
///
/// Returns a list of `(deny_path, allowed_path, access_type)` tuples where
/// a deny path falls under an allowed parent, which landlock cannot enforce.
fn find_deny_conflicts(caps: &FilesystemCapabilities) -> Vec<(&str, &str, &'static str)> {
    let mut conflicts = Vec::new();
    for deny_path in &caps.deny {
        for read_path in &caps.read {
            if deny_path.starts_with(read_path) {
                conflicts.push((deny_path.as_str(), read_path.as_str(), "read"));
            }
        }
        for write_path in &caps.write {
            if deny_path.starts_with(write_path) {
                conflicts.push((deny_path.as_str(), write_path.as_str(), "write"));
            }
        }
    }
    conflicts
}

/// Warn about deny paths that cannot be enforced due to overlapping allow rules
fn check_deny_conflicts(caps: &FilesystemCapabilities) {
    for (deny_path, allowed_path, access_type) in find_deny_conflicts(caps) {
        warn!(
            "Deny path '{}' is under allowed {} path '{}' - \
             landlock cannot enforce sub-path denials (limitation)",
            deny_path, access_type, allowed_path
        );
    }
}

/// Check if the kernel supports landlock
pub fn check_landlock_support() -> bool {
    // Check /sys/kernel/security/lsm for landlock module
    if let Ok(lsm) = std::fs::read_to_string("/sys/kernel/security/lsm") {
        return lsm.contains("landlock");
    }

    // Fallback: try creating a minimal ruleset
    Ruleset::default()
        .handle_access(AccessFs::from_all(ABI::V1))
        .and_then(|r| r.create())
        .is_ok()
}

/// Build a ruleset from capabilities without applying it (for testing/validation)
pub fn build_landlock_ruleset(
    caps: &FilesystemCapabilities,
) -> Result<LandlockRulesetInfo, EnforcementError> {
    let mut skipped = Vec::new();
    let read_count = count_existing_paths(&caps.read, &mut skipped);
    let write_count = count_existing_paths(&caps.write, &mut skipped);
    let execute_count = count_existing_paths(&caps.execute, &mut skipped);

    Ok(LandlockRulesetInfo {
        read_rules: read_count,
        write_rules: write_count,
        execute_rules: execute_count,
        deny_conflicts: count_deny_conflicts(caps),
        skipped_paths: skipped,
    })
}

/// Count deny paths that conflict with allowed parents
fn count_deny_conflicts(caps: &FilesystemCapabilities) -> usize {
    find_deny_conflicts(caps).len()
}

/// Information about a built landlock ruleset (for reporting)
#[derive(Debug, Clone)]
pub struct LandlockRulesetInfo {
    /// Number of read access rules
    pub read_rules: usize,
    /// Number of write access rules
    pub write_rules: usize,
    /// Number of execute access rules
    pub execute_rules: usize,
    /// Number of deny paths that conflict with allowed parents
    pub deny_conflicts: usize,
    /// Paths that were skipped because they don't exist
    pub skipped_paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::FilesystemCapabilities;

    #[test]
    fn test_landlock_support_check() {
        // Should not panic regardless of kernel support
        let _ = check_landlock_support();
    }

    #[test]
    fn test_build_ruleset_from_capabilities() {
        let caps = FilesystemCapabilities {
            read: vec!["/usr".to_string(), "/etc".to_string()],
            write: vec!["/tmp".to_string()],
            execute: vec!["/usr/bin".to_string()],
            deny: vec!["/etc/shadow".to_string()],
        };

        let info = build_landlock_ruleset(&caps).unwrap();
        // Exact counts depend on which paths exist on the test system
        // but the function should not panic
        assert!(info.read_rules <= 2);
        assert!(info.write_rules <= 1);
        assert!(info.execute_rules <= 1);
    }

    #[test]
    fn test_empty_capabilities_ruleset() {
        let caps = FilesystemCapabilities {
            read: Vec::new(),
            write: Vec::new(),
            execute: Vec::new(),
            deny: Vec::new(),
        };

        let info = build_landlock_ruleset(&caps).unwrap();
        assert_eq!(info.read_rules, 0);
        assert_eq!(info.write_rules, 0);
        assert_eq!(info.execute_rules, 0);
        assert_eq!(info.deny_conflicts, 0);
        assert!(info.skipped_paths.is_empty());
    }

    #[test]
    fn test_deny_conflict_detection() {
        let caps = FilesystemCapabilities {
            read: vec!["/etc".to_string()],
            write: vec!["/var".to_string()],
            execute: Vec::new(),
            deny: vec![
                "/etc/shadow".to_string(),
                "/var/secret".to_string(),
                "/home/private".to_string(), // no conflict
            ],
        };

        let conflicts = count_deny_conflicts(&caps);
        assert_eq!(conflicts, 2); // /etc/shadow under /etc, /var/secret under /var
    }

    #[test]
    fn test_nonexistent_paths_skipped() {
        let caps = FilesystemCapabilities {
            read: vec!["/nonexistent/path/12345".to_string()],
            write: Vec::new(),
            execute: Vec::new(),
            deny: Vec::new(),
        };

        let info = build_landlock_ruleset(&caps).unwrap();
        assert_eq!(info.read_rules, 0);
        assert_eq!(info.skipped_paths.len(), 1);
        assert_eq!(info.skipped_paths[0], "/nonexistent/path/12345");
    }

    #[test]
    fn test_check_deny_conflicts_no_overlap() {
        let caps = FilesystemCapabilities {
            read: vec!["/usr".to_string()],
            write: Vec::new(),
            execute: Vec::new(),
            deny: vec!["/home/secret".to_string()], // not under /usr
        };

        let conflicts = count_deny_conflicts(&caps);
        assert_eq!(conflicts, 0);
    }
}
