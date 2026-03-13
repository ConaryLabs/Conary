// src/commands/install/dep_resolution.rs
//! Dependency resolution with dep-mode awareness
//!
//! This module implements the dep-mode-aware resolution logic that determines
//! how each missing dependency should be handled based on the user's chosen
//! `DepMode` (Satisfy, Adopt, Takeover).

use super::blocklist;
use super::dep_mode::DepMode;
use super::system_pm;
use conary_core::db::models::Trove;
use conary_core::resolver::MissingDependency;
use tracing::debug;

/// A dependency that needs to be installed from a repository
#[derive(Debug, Clone)]
pub struct ResolvedDep {
    pub name: String,
    #[allow(dead_code)]
    pub version: Option<String>,
    #[allow(dead_code)]
    pub required_by: Vec<String>,
}

/// Result of dep-mode-aware dependency resolution
#[derive(Debug, Default)]
pub struct DepResolutionPlan {
    /// Dependencies to download and install from Remi
    pub to_install: Vec<ResolvedDep>,
    /// Dependencies to auto-adopt from system PM (adopt mode)
    pub to_adopt: Vec<String>,
    /// Dependencies already satisfied (name, reason)
    pub satisfied: Vec<(String, String)>,
    /// Dependencies on the blocklist (treated as satisfied)
    pub blocked: Vec<String>,
    /// Dependencies that cannot be resolved
    pub unresolvable: Vec<MissingDependency>,
}

/// Resolve missing dependencies using the active source policy convergence intent
/// as the default dep mode.
///
/// When the user has not explicitly specified `--dep-mode`, this function reads
/// the convergence intent from the source policy and derives an appropriate mode:
/// - `TrackOnly` -> `Satisfy`
/// - `CasBacked` -> `Adopt`
/// - `FullOwnership` -> `Takeover`
///
/// If `explicit_mode` is `Some`, it takes precedence over the policy default.
#[allow(dead_code)] // Callers land in later tasks (Task 8+)
pub fn resolve_missing_deps_policy_aware(
    conn: &rusqlite::Connection,
    missing: &[MissingDependency],
    explicit_mode: Option<DepMode>,
    convergence: &conary_core::model::parser::ConvergenceIntent,
) -> DepResolutionPlan {
    let effective_mode = explicit_mode
        .unwrap_or_else(|| DepMode::from_convergence_intent(convergence));
    debug!(
        "Dep resolution: explicit_mode={:?}, convergence={}, effective={}",
        explicit_mode,
        convergence.display_name(),
        effective_mode
    );
    resolve_missing_deps(conn, missing, effective_mode)
}

/// Classify missing dependencies according to the chosen `DepMode`.
///
/// For each missing dependency, the function checks (in order):
/// 1. Blocklist -- always treated as satisfied (never touched)
/// 2. Already tracked in Conary DB -- satisfied or needs takeover
/// 3. System PM presence -- depends on dep mode
///
/// The caller is responsible for actually installing, adopting, or
/// reporting the results.
pub fn resolve_missing_deps(
    conn: &rusqlite::Connection,
    missing: &[MissingDependency],
    dep_mode: DepMode,
) -> DepResolutionPlan {
    let mut plan = DepResolutionPlan::default();

    for dep in missing {
        // 1. Check blocklist first -- these are never replaced, but must be present
        if blocklist::is_blocked(&dep.name) {
            // Verify the blocked package is actually installed on the system
            let is_tracked = Trove::find_by_name(conn, &dep.name)
                .map(|t| !t.is_empty())
                .unwrap_or(false);
            let is_on_system = is_tracked || system_pm::is_system_package_installed(&dep.name);

            if is_on_system {
                debug!("Dependency '{}' is blocked and present on system", dep.name);
                plan.blocked.push(dep.name.clone());
            } else {
                debug!(
                    "Dependency '{}' is blocked but NOT present on system",
                    dep.name
                );
                plan.unresolvable.push(dep.clone());
            }
            continue;
        }

        // 2. Check if already tracked in Conary's DB
        if let Ok(troves) = Trove::find_by_name(conn, &dep.name)
            && !troves.is_empty()
        {
            let trove = &troves[0];
            match dep_mode {
                DepMode::Satisfy | DepMode::Adopt => {
                    // Already tracked (any source) = satisfied
                    plan.satisfied.push((
                        dep.name.clone(),
                        format!("tracked ({})", trove.install_source),
                    ));
                    continue;
                }
                DepMode::Takeover => {
                    if trove.install_source.is_conary_owned() {
                        // Already Conary-owned = satisfied
                        plan.satisfied
                            .push((dep.name.clone(), "Conary-owned".into()));
                        continue;
                    }
                    // Adopted but not owned -- need to take over
                    debug!(
                        "Dependency '{}' is adopted but not owned, scheduling takeover",
                        dep.name
                    );
                    plan.to_install.push(ResolvedDep {
                        name: dep.name.clone(),
                        version: None,
                        required_by: dep.required_by.clone(),
                    });
                    continue;
                }
            }
        }

        // 3. Not in Conary DB -- check system PM
        match dep_mode {
            DepMode::Satisfy => {
                if system_pm::is_system_package_installed(&dep.name) {
                    plan.satisfied.push((dep.name.clone(), "system PM".into()));
                } else {
                    // Not on system either -- unresolvable in satisfy mode
                    debug!(
                        "Dependency '{}' not found in system PM (satisfy mode), marking unresolvable",
                        dep.name
                    );
                    plan.unresolvable.push(dep.clone());
                }
            }
            DepMode::Adopt => {
                if system_pm::is_system_package_installed(&dep.name) {
                    debug!(
                        "Dependency '{}' found in system PM, scheduling auto-adopt",
                        dep.name
                    );
                    plan.to_adopt.push(dep.name.clone());
                } else {
                    // Not on system either -- need to install from Remi
                    debug!(
                        "Dependency '{}' not found in system PM (adopt mode), scheduling install",
                        dep.name
                    );
                    plan.to_install.push(ResolvedDep {
                        name: dep.name.clone(),
                        version: None,
                        required_by: dep.required_by.clone(),
                    });
                }
            }
            DepMode::Takeover => {
                // Always install CCS version from Remi
                debug!(
                    "Dependency '{}' scheduled for Remi install (takeover mode)",
                    dep.name
                );
                plan.to_install.push(ResolvedDep {
                    name: dep.name.clone(),
                    version: None,
                    required_by: dep.required_by.clone(),
                });
            }
        }
    }

    debug!(
        "Dep resolution plan: {} to_install, {} to_adopt, {} satisfied, {} blocked, {} unresolvable",
        plan.to_install.len(),
        plan.to_adopt.len(),
        plan.satisfied.len(),
        plan.blocked.len(),
        plan.unresolvable.len()
    );

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::schema;
    use conary_core::version::VersionConstraint;

    /// Set up an in-memory database with the full Conary schema
    fn test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    fn make_dep(name: &str, required_by: &[&str]) -> MissingDependency {
        MissingDependency {
            name: name.to_string(),
            constraint: VersionConstraint::Any,
            required_by: required_by.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_blocked_deps_always_blocked() {
        let conn = test_db();

        let missing = vec![make_dep("glibc", &["nginx"])];

        let plan = resolve_missing_deps(&conn, &missing, DepMode::Takeover);
        assert_eq!(plan.blocked.len(), 1);
        assert_eq!(plan.blocked[0], "glibc");
        assert!(plan.to_install.is_empty());
        assert!(plan.to_adopt.is_empty());
        assert!(plan.unresolvable.is_empty());
    }

    #[test]
    fn test_blocked_in_all_modes() {
        let conn = test_db();

        for mode in [DepMode::Satisfy, DepMode::Adopt, DepMode::Takeover] {
            let missing = vec![make_dep("systemd", &["nginx"])];
            let plan = resolve_missing_deps(&conn, &missing, mode);
            assert_eq!(
                plan.blocked.len(),
                1,
                "systemd should be blocked in {} mode",
                mode
            );
        }
    }

    #[test]
    fn test_normal_dep_in_takeover_mode() {
        let conn = test_db();

        let missing = vec![make_dep("pcre2", &["nginx"])];

        let plan = resolve_missing_deps(&conn, &missing, DepMode::Takeover);
        // pcre2 is not blocked, not in DB, so should be in to_install
        assert_eq!(plan.to_install.len(), 1);
        assert_eq!(plan.to_install[0].name, "pcre2");
        assert!(plan.blocked.is_empty());
    }

    #[test]
    fn test_mixed_deps() {
        let conn = test_db();

        let missing = vec![
            make_dep("glibc", &["nginx"]),   // blocked
            make_dep("pcre2", &["nginx"]),   // not blocked, not installed
            make_dep("openssl", &["nginx"]), // blocked
        ];

        let plan = resolve_missing_deps(&conn, &missing, DepMode::Takeover);
        assert_eq!(plan.blocked.len(), 2, "glibc and openssl should be blocked");
        assert_eq!(plan.to_install.len(), 1, "pcre2 should be to_install");
        assert_eq!(plan.to_install[0].name, "pcre2");
    }

    #[test]
    fn test_empty_missing_list() {
        let conn = test_db();

        let plan = resolve_missing_deps(&conn, &[], DepMode::Satisfy);
        assert!(plan.to_install.is_empty());
        assert!(plan.to_adopt.is_empty());
        assert!(plan.satisfied.is_empty());
        assert!(plan.blocked.is_empty());
        assert!(plan.unresolvable.is_empty());
    }

    #[test]
    fn test_satisfy_mode_unknown_dep_is_unresolvable() {
        let conn = test_db();

        // In satisfy mode, a dep not in DB and not on system PM is unresolvable
        // (system PM check will fail in test env since no PM is available)
        let missing = vec![make_dep("some-obscure-lib", &["myapp"])];
        let plan = resolve_missing_deps(&conn, &missing, DepMode::Satisfy);

        // Since we're in a test environment without a real system PM,
        // the dep should end up as unresolvable
        assert_eq!(plan.unresolvable.len(), 1);
        assert_eq!(plan.unresolvable[0].name, "some-obscure-lib");
    }

    #[test]
    fn test_policy_aware_uses_convergence_when_no_explicit_mode() {
        use conary_core::model::parser::ConvergenceIntent;

        let conn = test_db();
        let missing = vec![make_dep("pcre2", &["nginx"])];

        // FullOwnership convergence intent -> Takeover mode -> to_install
        let plan = resolve_missing_deps_policy_aware(
            &conn,
            &missing,
            None,
            &ConvergenceIntent::FullOwnership,
        );
        assert_eq!(plan.to_install.len(), 1);
        assert_eq!(plan.to_install[0].name, "pcre2");
    }

    #[test]
    fn test_policy_aware_explicit_mode_overrides_convergence() {
        use conary_core::model::parser::ConvergenceIntent;

        let conn = test_db();
        let missing = vec![make_dep("some-obscure-lib", &["myapp"])];

        // Even though convergence is FullOwnership (-> Takeover), the explicit
        // mode Satisfy should win and produce unresolvable (no system PM in test)
        let plan = resolve_missing_deps_policy_aware(
            &conn,
            &missing,
            Some(DepMode::Satisfy),
            &ConvergenceIntent::FullOwnership,
        );
        assert_eq!(plan.unresolvable.len(), 1);
    }
}
