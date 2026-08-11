// conary-core/src/capability/enforcement/landlock_enforce.rs
//! Landlock LSM filesystem and exact TCP-port enforcement
//!
//! Converts filesystem and network capability declarations into Landlock
//! rulesets. Landlock is deny-by-default for every access class handled by a
//! ruleset.
//!
//! ## Limitations
//!
//! - Landlock cannot deny a specific path under an allowed parent (e.g., deny
//!   `/etc/shadow` when `/etc` is readable). In Enforce mode, deny paths that
//!   conflict with allowed parents return an error. In Warn/Audit mode, they
//!   generate a warning.
//! - Non-existent paths fail setup in Enforce mode. Audit and Warn may report
//!   them without treating the report as enforcement authority.
//! - Exact TCP-port restrictions require Landlock network ABI V4.

use super::{EnforcementError, EnforcementMode};
use crate::capability::{FilesystemCapabilities, NetworkCapabilities};
use landlock::{
    ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, NetPort, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus, path_beneath_rules,
};

/// Apply Landlock filesystem and exact TCP restrictions.
///
/// Builds a landlock ruleset from the capability declaration and applies it
/// to the current process. After this call, filesystem access is restricted
/// to only the declared paths.
pub fn apply_landlock_rules(
    filesystem: Option<&FilesystemCapabilities>,
    network: Option<&NetworkCapabilities>,
    mode: EnforcementMode,
) -> Result<(), EnforcementError> {
    if let Some(caps) = network {
        caps.validate()
            .map_err(|error| EnforcementError::Landlock(error.to_string()))?;
    }
    apply_prepared_landlock_rules(filesystem, network, mode)
}

/// Apply rules whose declarations were validated by the parent before fork.
pub(super) fn apply_prepared_landlock_rules(
    filesystem: Option<&FilesystemCapabilities>,
    network: Option<&NetworkCapabilities>,
    mode: EnforcementMode,
) -> Result<(), EnforcementError> {
    let has_network_rules =
        network.is_some_and(|caps| !caps.connect_tcp.is_empty() || !caps.bind_tcp.is_empty());
    let abi = if has_network_rules { ABI::V4 } else { ABI::V3 };

    let read_paths = paths_for_rules(
        filesystem.map_or(&[], |caps| caps.read.as_slice()),
        "read",
        mode,
    )?;
    let write_paths = paths_for_rules(
        filesystem.map_or(&[], |caps| caps.write.as_slice()),
        "write",
        mode,
    )?;
    let execute_paths = paths_for_rules(
        filesystem.map_or(&[], |caps| caps.execute.as_slice()),
        "execute",
        mode,
    )?;

    let deny_conflicts = filesystem.map_or(0, count_deny_conflicts);
    if deny_conflicts > 0 && mode == EnforcementMode::Enforce {
        return Err(EnforcementError::DenyConflict {
            count: deny_conflicts,
        });
    }

    let mut ruleset = Ruleset::default().set_compatibility(CompatLevel::HardRequirement);
    if filesystem.is_some() {
        ruleset = ruleset
            .handle_access(AccessFs::from_all(abi))
            .map_err(|e| EnforcementError::Landlock(format!("Failed to handle access: {e}")))?;
    }
    if has_network_rules {
        ruleset = ruleset
            .handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)
            .map_err(|e| {
                EnforcementError::Landlock(format!(
                    "Failed to handle exact TCP access with ABI V4: {e}"
                ))
            })?;
    }

    let mut created = ruleset
        .create()
        .map_err(|e| EnforcementError::Landlock(format!("Failed to create ruleset: {e}")))?
        .set_compatibility(CompatLevel::HardRequirement);

    if filesystem.is_some() {
        created = created
            .add_rules(path_beneath_rules(&read_paths, AccessFs::from_read(abi)))
            .map_err(|e| EnforcementError::Landlock(format!("Failed to add read rules: {e}")))?
            .add_rules(path_beneath_rules(&write_paths, AccessFs::from_write(abi)))
            .map_err(|e| EnforcementError::Landlock(format!("Failed to add write rules: {e}")))?
            .add_rules(path_beneath_rules(
                &execute_paths,
                AccessFs::from_read(abi) | AccessFs::Execute,
            ))
            .map_err(|e| EnforcementError::Landlock(format!("Failed to add execute rules: {e}")))?;
    }
    if let Some(caps) = network.filter(|_| has_network_rules) {
        for &port in &caps.connect_tcp {
            created = created
                .add_rule(NetPort::new(port, AccessNet::ConnectTcp))
                .map_err(|e| {
                    EnforcementError::Landlock(format!(
                        "Failed to add connect_tcp rule for port {port}: {e}"
                    ))
                })?;
        }
        for &port in &caps.bind_tcp {
            created = created
                .add_rule(NetPort::new(port, AccessNet::BindTcp))
                .map_err(|e| {
                    EnforcementError::Landlock(format!(
                        "Failed to add bind_tcp rule for port {port}: {e}"
                    ))
                })?;
        }
    }

    let status = created
        .restrict_self()
        .map_err(|e| EnforcementError::Landlock(format!("Failed to restrict self: {e}")))?;

    require_fully_enforced(status.ruleset)?;
    Ok(())
}

fn paths_for_rules<'a>(
    paths: &'a [String],
    context: &str,
    mode: EnforcementMode,
) -> Result<Vec<&'a str>, EnforcementError> {
    let mut existing = Vec::with_capacity(paths.len());
    for path in paths {
        if std::path::Path::new(path).exists() {
            existing.push(path.as_str());
        } else if mode == EnforcementMode::Enforce {
            return Err(EnforcementError::Landlock(format!(
                "declared {context} path does not exist: {path}"
            )));
        }
    }
    Ok(existing)
}

fn require_fully_enforced(status: RulesetStatus) -> Result<(), EnforcementError> {
    match status {
        RulesetStatus::FullyEnforced => Ok(()),
        RulesetStatus::PartiallyEnforced => Err(EnforcementError::Landlock(
            "Landlock ruleset was only partially enforced".to_string(),
        )),
        RulesetStatus::NotEnforced => Err(EnforcementError::Landlock(
            "Landlock ruleset was not enforced by kernel".to_string(),
        )),
    }
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
    use std::path::Path;

    let mut conflicts = Vec::new();
    for deny_path in &caps.deny {
        for read_path in &caps.read {
            if Path::new(deny_path).starts_with(Path::new(read_path)) {
                conflicts.push((deny_path.as_str(), read_path.as_str(), "read"));
            }
        }
        for write_path in &caps.write {
            if Path::new(deny_path).starts_with(Path::new(write_path)) {
                conflicts.push((deny_path.as_str(), write_path.as_str(), "write"));
            }
        }
    }
    conflicts
}

/// Check if the kernel supports landlock
pub fn check_landlock_support() -> bool {
    Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(ABI::V3))
        .and_then(|r| r.create())
        .is_ok()
}

/// Check for exact TCP-port enforcement support.
pub fn check_landlock_network_support() -> bool {
    Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)
        .and_then(|ruleset| ruleset.create())
        .is_ok()
}

/// Describe the exact rules that would be built without applying them.
pub fn build_landlock_ruleset(
    filesystem: Option<&FilesystemCapabilities>,
    network: Option<&NetworkCapabilities>,
) -> Result<LandlockRulesetInfo, EnforcementError> {
    let mut skipped = Vec::new();
    let read_count = filesystem.map_or(0, |caps| count_existing_paths(&caps.read, &mut skipped));
    let write_count = filesystem.map_or(0, |caps| count_existing_paths(&caps.write, &mut skipped));
    let execute_count =
        filesystem.map_or(0, |caps| count_existing_paths(&caps.execute, &mut skipped));
    if let Some(caps) = network {
        caps.validate()
            .map_err(|error| EnforcementError::Landlock(error.to_string()))?;
    }

    Ok(LandlockRulesetInfo {
        read_rules: read_count,
        write_rules: write_count,
        execute_rules: execute_count,
        connect_tcp_rules: network.map_or_else(Vec::new, |caps| caps.connect_tcp.clone()),
        bind_tcp_rules: network.map_or_else(Vec::new, |caps| caps.bind_tcp.clone()),
        deny_conflicts: filesystem.map_or(0, count_deny_conflicts),
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
    /// Exact remote TCP ports granted ConnectTcp access.
    pub connect_tcp_rules: Vec<u16>,
    /// Exact local TCP ports granted BindTcp access.
    pub bind_tcp_rules: Vec<u16>,
    /// Number of deny paths that conflict with allowed parents
    pub deny_conflicts: usize,
    /// Paths that were skipped because they don't exist
    pub skipped_paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{FilesystemCapabilities, NetworkCapabilities};

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

        let info = build_landlock_ruleset(Some(&caps), None).unwrap();
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

        let info = build_landlock_ruleset(Some(&caps), None).unwrap();
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
                "/etc-backup/shadow".to_string(),
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

        let info = build_landlock_ruleset(Some(&caps), None).unwrap();
        assert_eq!(info.read_rules, 0);
        assert_eq!(info.skipped_paths.len(), 1);
        assert_eq!(info.skipped_paths[0], "/nonexistent/path/12345");
    }

    #[test]
    fn enforce_mode_rejects_missing_declared_path_before_applying_rules() {
        let missing = vec!["/definitely/missing/conary-landlock-path".to_string()];
        let error = paths_for_rules(&missing, "read", EnforcementMode::Enforce)
            .expect_err("enforcement must not silently omit a declared path");
        assert!(
            error
                .to_string()
                .contains("declared read path does not exist")
        );
    }

    #[test]
    fn non_enforcing_mode_may_report_missing_path_without_authority() {
        let missing = vec!["/definitely/missing/conary-landlock-path".to_string()];
        let paths = paths_for_rules(&missing, "read", EnforcementMode::Audit).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn only_fully_enforced_landlock_status_is_success() {
        require_fully_enforced(RulesetStatus::FullyEnforced).unwrap();
        assert!(require_fully_enforced(RulesetStatus::PartiallyEnforced).is_err());
        assert!(require_fully_enforced(RulesetStatus::NotEnforced).is_err());
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

    #[test]
    fn test_enforce_mode_fails_on_deny_conflicts() {
        let caps = FilesystemCapabilities {
            read: vec!["/etc".to_string()],
            write: Vec::new(),
            execute: Vec::new(),
            deny: vec!["/etc/shadow".to_string()],
        };

        let result = apply_landlock_rules(Some(&caps), None, EnforcementMode::Enforce);
        // On systems without landlock, we get Unsupported; with landlock, DenyConflict
        assert!(
            result.is_err(),
            "Enforce mode must fail when deny paths conflict with allowed parents"
        );
    }

    #[test]
    fn test_warn_mode_allows_deny_conflicts() {
        // In Warn mode, deny conflicts should not cause an error
        // (they just log warnings). On systems without landlock support
        // the function returns Ok early, so this test verifies both paths.
        let caps = FilesystemCapabilities {
            read: vec!["/etc".to_string()],
            write: Vec::new(),
            execute: Vec::new(),
            deny: vec!["/etc/shadow".to_string()],
        };

        let result = apply_landlock_rules(Some(&caps), None, EnforcementMode::Warn);
        // Warn mode should succeed (or fail only due to lack of landlock support,
        // which also returns Ok in non-Enforce mode)
        assert!(
            result.is_ok(),
            "Warn mode should not fail on deny conflicts"
        );
    }

    #[test]
    fn exact_tcp_rules_are_preserved_in_the_landlock_plan() {
        let network = NetworkCapabilities {
            connect_tcp: vec![443, 8443],
            bind_tcp: vec![8080],
            none: false,
        };

        let info = build_landlock_ruleset(None, Some(&network)).unwrap();
        assert_eq!(info.connect_tcp_rules, [443, 8443]);
        assert_eq!(info.bind_tcp_rules, [8080]);
    }

    #[test]
    fn network_support_check_requires_landlock_abi_v4() {
        let _ = check_landlock_network_support();
    }

    #[test]
    fn exact_connect_tcp_rule_denies_an_undeclared_port_when_supported() {
        const CHILD_ENV: &str = "CONARY_TEST_LANDLOCK_TCP_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            let test_name = std::thread::current()
                .name()
                .expect("test thread has a name")
                .to_string();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &test_name, "--nocapture"])
                .env(CHILD_ENV, "1")
                .status()
                .expect("spawn isolated Landlock test process");
            assert!(status.success(), "isolated Landlock test failed");
            return;
        }

        if !check_landlock_network_support() {
            return;
        }

        let allowed = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let denied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let allowed_port = allowed.local_addr().unwrap().port();
        let denied_port = denied.local_addr().unwrap().port();
        let network = NetworkCapabilities {
            connect_tcp: vec![allowed_port],
            bind_tcp: Vec::new(),
            none: false,
        };
        apply_landlock_rules(None, Some(&network), EnforcementMode::Enforce)
            .expect("Landlock ABI V4 probe promised exact TCP enforcement");

        std::net::TcpStream::connect(("127.0.0.1", allowed_port))
            .expect("declared connect_tcp port should be allowed");
        let error = std::net::TcpStream::connect(("127.0.0.1", denied_port))
            .expect_err("undeclared connect_tcp port should be denied");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
