// src/commands/replatform_rendering.rs

//! Shared rendering functions for replatform execution plans.
//!
//! Both `model.rs` and `update.rs` display replatform execution plans
//! and blocked-reason diagnostics. This module provides the canonical
//! implementations so both callers share a single code path.

use conary_core::model::{ReplatformBlockedReason, ReplatformExecutionPlan};

pub(crate) fn render_replatform_execution_plan(plan: &ReplatformExecutionPlan) -> String {
    let mut lines = vec![format!(
        "Planned replatform transactions ({}):",
        plan.transactions.len()
    )];

    for transaction in &plan.transactions {
        let current = transaction
            .current_distro
            .as_deref()
            .unwrap_or("unknown source");
        let status = if transaction.executable {
            "executable"
        } else {
            "blocked"
        };
        let mut line = format!(
            "  > [{}] remove {} {} from {}, install {} {}",
            status,
            transaction.package,
            transaction.current_version,
            current,
            transaction.target_distro,
            transaction.target_version
        );
        if let Some(arch) = &transaction.architecture {
            line.push_str(&format!(" [{}]", arch));
        }
        match (
            transaction.install_repository.as_deref(),
            transaction.install_repository_package_id,
        ) {
            (Some(repo), Some(pkg_id)) => {
                line.push_str(&format!(" via {} [repo-pkg:{}]", repo, pkg_id));
                if let Some(route) = &transaction.install_route {
                    line.push_str(&format!(" [route:{}]", route));
                }
                if let Some(reason) = transaction.blocked_reason.as_ref() {
                    line.push_str(&format!(" [{}]", render_replatform_blocked_reason(reason)));
                }
            }
            _ => {
                let reason = transaction
                    .blocked_reason
                    .as_ref()
                    .map(render_replatform_blocked_reason)
                    .unwrap_or("pending repo/package resolution");
                line.push_str(&format!(" [{}]", reason));
            }
        }
        if !transaction.unresolved_dependencies.is_empty() {
            line.push_str(&format!(
                " [deps:{}]",
                transaction.unresolved_dependencies.join(", ")
            ));
        }
        lines.push(line);
    }

    lines.join("\n")
}

pub(crate) fn render_replatform_blocked_reason(
    reason: &ReplatformBlockedReason,
) -> &'static str {
    match reason {
        ReplatformBlockedReason::MissingRepositoryMetadata => "missing repository metadata",
        ReplatformBlockedReason::MissingRepositoryPackageId => "missing repository package id",
        ReplatformBlockedReason::AnyVersionRouteOnly => "only any-version install route",
        ReplatformBlockedReason::MissingVersionedInstallRoute => {
            "missing versioned install route"
        }
        ReplatformBlockedReason::MissingInstallRoute => "missing install route",
        ReplatformBlockedReason::UnsatisfiedTargetDependencies => {
            "unsatisfied target dependencies"
        }
        ReplatformBlockedReason::ArchitectureMismatch => "architecture mismatch",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::model::{ReplatformBlockedReason, ReplatformExecutionPlan};

    #[test]
    fn test_render_replatform_execution_plan_lists_transactions() {
        let plan = ReplatformExecutionPlan {
            transactions: vec![
                conary_core::model::ReplatformExecutionTransaction {
                    package: "bash".to_string(),
                    current_distro: Some("fedora-43".to_string()),
                    target_distro: "arch".to_string(),
                    current_version: "5.1.0".to_string(),
                    current_architecture: Some("x86_64".to_string()),
                    target_version: "5.2.0".to_string(),
                    architecture: Some("x86_64".to_string()),
                    install_repository: Some("arch-core".to_string()),
                    install_repository_package_id: Some(11),
                    install_route: Some("default:legacy".to_string()),
                    unresolved_dependencies: Vec::new(),
                    executable: false,
                    blocked_reason: Some(
                        ReplatformBlockedReason::MissingVersionedInstallRoute,
                    ),
                },
                conary_core::model::ReplatformExecutionTransaction {
                    package: "vim".to_string(),
                    current_distro: Some("fedora-43".to_string()),
                    target_distro: "arch".to_string(),
                    current_version: "9.0.1".to_string(),
                    current_architecture: Some("x86_64".to_string()),
                    target_version: "9.1.0".to_string(),
                    architecture: Some("x86_64".to_string()),
                    install_repository: Some("arch-core".to_string()),
                    install_repository_package_id: Some(22),
                    install_route: Some("default:legacy".to_string()),
                    unresolved_dependencies: Vec::new(),
                    executable: false,
                    blocked_reason: Some(
                        ReplatformBlockedReason::MissingVersionedInstallRoute,
                    ),
                },
            ],
        };

        let rendered = render_replatform_execution_plan(&plan);

        assert!(rendered.contains("Planned replatform transactions (2):"));
        assert!(
            rendered.contains("[blocked] remove bash 5.1.0 from fedora-43, install arch 5.2.0")
        );
        assert!(rendered.contains(
            "via arch-core [repo-pkg:11] [route:default:legacy] [missing versioned install route]"
        ));
        assert!(rendered.contains("[blocked] remove vim 9.0.1 from fedora-43, install arch 9.1.0"));
        assert!(rendered.contains(
            "via arch-core [repo-pkg:22] [route:default:legacy] [missing versioned install route]"
        ));
    }

    #[test]
    fn test_render_blocked_reason_missing_metadata() {
        let rendered = render_replatform_blocked_reason(
            &ReplatformBlockedReason::MissingRepositoryMetadata,
        );
        assert_eq!(rendered, "missing repository metadata");
    }

    #[test]
    fn test_render_blocked_reason_any_version() {
        let rendered = render_replatform_blocked_reason(
            &ReplatformBlockedReason::AnyVersionRouteOnly,
        );
        assert_eq!(rendered, "only any-version install route");
    }

    #[test]
    fn test_render_executable_transaction() {
        let plan = ReplatformExecutionPlan {
            transactions: vec![conary_core::model::ReplatformExecutionTransaction {
                package: "vim".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                current_version: "9.0.1".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                install_repository: Some("arch-core".to_string()),
                install_repository_package_id: Some(22),
                install_route: Some("resolution:binary".to_string()),
                unresolved_dependencies: Vec::new(),
                executable: true,
                blocked_reason: None,
            }],
        };

        let rendered = render_replatform_execution_plan(&plan);

        assert!(
            rendered.contains("[executable] remove vim 9.0.1 from fedora-43, install arch 9.1.0")
        );
        assert!(rendered.contains("via arch-core [repo-pkg:22] [route:resolution:binary]"));
        assert!(!rendered.contains("missing install route"));
    }

    #[test]
    fn test_render_unresolved_dependencies() {
        let plan = ReplatformExecutionPlan {
            transactions: vec![conary_core::model::ReplatformExecutionTransaction {
                package: "vim".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                current_version: "9.0.1".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                install_repository: Some("arch-core".to_string()),
                install_repository_package_id: Some(22),
                install_route: Some("resolution:binary".to_string()),
                unresolved_dependencies: vec!["libmagic (>= 1.0)".to_string()],
                executable: false,
                blocked_reason: Some(
                    ReplatformBlockedReason::UnsatisfiedTargetDependencies,
                ),
            }],
        };

        let rendered = render_replatform_execution_plan(&plan);

        assert!(rendered.contains("[unsatisfied target dependencies]"));
        assert!(rendered.contains("[deps:libmagic (>= 1.0)]"));
    }
}
