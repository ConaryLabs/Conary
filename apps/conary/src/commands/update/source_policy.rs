// src/commands/update/source_policy.rs

//! Source-policy and replatform preview helpers for update commands.

use super::super::replatform_rendering::render_replatform_execution_plan;
use anyhow::Result;
use conary_core::db::models::{DistroPin, SystemAffinity};
use conary_core::model::{
    DiffAction, capture_current_state, planned_replatform_actions, replatform_execution_plan,
    source_policy_replatform_snapshot,
};
use rusqlite::Connection;

pub(super) fn print_source_policy_update_preview(conn: &Connection) -> Result<()> {
    let current_pin = DistroPin::get_current(conn)?;
    let affinities = SystemAffinity::list(conn)?;
    let realignment_snapshot = current_pin
        .as_ref()
        .map(|pin| source_policy_replatform_snapshot(conn, &pin.distro))
        .transpose()?;
    let realignment_candidates = realignment_snapshot
        .as_ref()
        .map(|snapshot| snapshot.visible_realignment_candidates);
    if let Some(context) =
        source_policy_update_context(current_pin.as_ref(), &affinities, realignment_candidates)
    {
        println!("{}", context);
    }
    if let Some(snapshot) = realignment_snapshot.as_ref() {
        let state = capture_current_state(conn)?;
        let actions = planned_replatform_actions(snapshot, &state);
        if let Some(plan) = replatform_execution_plan(conn, &actions)? {
            println!("{}", render_replatform_execution_plan(&plan));
        } else if let Some(preview) = render_replatform_action_preview(&actions) {
            println!("{}", preview);
        }
    }

    Ok(())
}

fn source_policy_update_context(
    pin: Option<&DistroPin>,
    affinities: &[SystemAffinity],
    realignment_candidates: Option<usize>,
) -> Option<String> {
    let pin = pin?;
    let strength = pin.mixing_policy.as_str();

    if affinities.is_empty() {
        return Some(format!(
            "Active source policy pin: {} ({}). Replatform estimate unavailable: no source affinity data yet.{}",
            pin.distro,
            strength,
            match realignment_candidates {
                Some(count) => format!(
                    " Package-level realignment candidates currently visible: {}.",
                    count
                ),
                None => String::new(),
            }
        ));
    }

    let total_packages: i64 = affinities
        .iter()
        .map(|affinity| affinity.package_count)
        .sum();
    if total_packages == 0 {
        return Some(format!(
            "Active source policy pin: {} ({}). Replatform estimate unavailable: no installed packages are represented in current affinity data.{}",
            pin.distro,
            strength,
            match realignment_candidates {
                Some(count) => format!(
                    " Package-level realignment candidates currently visible: {}.",
                    count
                ),
                None => String::new(),
            }
        ));
    }

    let aligned_packages = affinities
        .iter()
        .find(|affinity| affinity.distro == pin.distro)
        .map(|affinity| affinity.package_count)
        .unwrap_or(0);
    let packages_to_realign = total_packages.saturating_sub(aligned_packages);

    Some(format!(
        "Active source policy pin: {} ({}). About {} installed package(s) already align, and about {} may need source realignment during future convergence.{}",
        pin.distro,
        strength,
        aligned_packages,
        packages_to_realign,
        match realignment_candidates {
            Some(count) => format!(
                " Package-level realignment candidates currently visible: {}.",
                count
            ),
            None => String::new(),
        }
    ))
}

fn render_replatform_action_preview(actions: &[DiffAction]) -> Option<String> {
    let replatforms: Vec<_> = actions
        .iter()
        .filter_map(|action| match action {
            DiffAction::ReplatformReplace { .. } => Some(action.description()),
            _ => None,
        })
        .collect();

    if replatforms.is_empty() {
        return None;
    }

    let preview: Vec<String> = replatforms.iter().take(3).cloned().collect();

    let mut line = format!("Planned replatform replacements: {}", preview.join(", "));
    if replatforms.len() > preview.len() {
        line.push_str(&format!(", +{} more", replatforms.len() - preview.len()));
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::{create_test_db, seed_mixed_replatform_fixture};
    use conary_core::db::models::DistroPin;
    use conary_core::model::ReplatformBlockedReason;

    #[test]
    fn test_source_policy_update_context_with_affinity() {
        let pin = DistroPin {
            id: Some(1),
            distro: "arch".to_string(),
            mixing_policy: "strict".to_string(),
            created_at: "2026-03-12".to_string(),
        };
        let affinities = vec![
            SystemAffinity {
                distro: "arch".to_string(),
                package_count: 10,
                percentage: 25.0,
            },
            SystemAffinity {
                distro: "fedora-44".to_string(),
                package_count: 30,
                percentage: 75.0,
            },
        ];

        let context = source_policy_update_context(Some(&pin), &affinities, Some(0)).unwrap();

        assert!(context.contains("Package-level realignment candidates"));
        assert!(context.contains("0."));
        assert!(context.contains("Active source policy pin: arch (strict)"));
        assert!(context.contains("10 installed package(s) already align"));
        assert!(context.contains("30 may need source realignment"));
    }

    #[test]
    fn test_source_policy_update_context_without_affinity_data() {
        let pin = DistroPin {
            id: Some(1),
            distro: "arch".to_string(),
            mixing_policy: "strict".to_string(),
            created_at: "2026-03-12".to_string(),
        };

        let context = source_policy_update_context(Some(&pin), &[], None).unwrap();

        assert!(context.contains("Replatform estimate unavailable"));
        assert!(context.contains("no source affinity data yet"));
    }

    #[test]
    fn test_update_replatform_planning_surfaces_mixed_execution_states() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        seed_mixed_replatform_fixture(&conn);
        DistroPin::set(&conn, "arch", "strict").unwrap();

        let pin = DistroPin::get_current(&conn)
            .unwrap()
            .expect("expected source pin");
        let snapshot = source_policy_replatform_snapshot(&conn, &pin.distro).unwrap();
        let state = capture_current_state(&conn).unwrap();
        let actions = planned_replatform_actions(&snapshot, &state);
        let plan = replatform_execution_plan(&conn, &actions)
            .unwrap()
            .expect("expected replatform execution plan");

        assert_eq!(plan.transactions.len(), 3);

        let bash = plan
            .transactions
            .iter()
            .find(|transaction| transaction.package == "bash")
            .expect("expected bash transaction");
        assert!(!bash.executable);
        assert_eq!(bash.install_route.as_deref(), Some("resolution:binary"));
        assert_eq!(
            bash.blocked_reason,
            Some(ReplatformBlockedReason::AnyVersionRouteOnly)
        );

        let vim = plan
            .transactions
            .iter()
            .find(|transaction| transaction.package == "vim")
            .expect("expected vim transaction");
        assert!(vim.executable);
        assert_eq!(vim.install_route.as_deref(), Some("resolution:binary"));
        assert_eq!(vim.blocked_reason, None);

        let zsh = plan
            .transactions
            .iter()
            .find(|transaction| transaction.package == "zsh")
            .expect("expected zsh transaction");
        assert!(!zsh.executable);
        assert_eq!(zsh.install_route.as_deref(), Some("default:legacy"));
        assert_eq!(
            zsh.blocked_reason,
            Some(ReplatformBlockedReason::MissingVersionedInstallRoute)
        );
    }

    #[test]
    fn test_render_replatform_action_preview_lists_examples() {
        let actions = vec![
            DiffAction::ReplatformReplace {
                package: "bash".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                current_version: "5.1.0".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "5.2.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(11),
            },
            DiffAction::ReplatformReplace {
                package: "vim".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                current_version: "9.0.1".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(22),
            },
            DiffAction::ReplatformReplace {
                package: "zsh".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                current_version: "5.8.0".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "5.9.1".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(33),
            },
            DiffAction::ReplatformReplace {
                package: "curl".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                current_version: "8.7.0".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "8.8.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(44),
            },
        ];

        let rendered = render_replatform_action_preview(&actions).unwrap();

        assert!(rendered.contains("Replatform bash"));
        assert!(rendered.contains("Replatform vim"));
        assert!(rendered.contains("Replatform zsh"));
        assert!(rendered.contains("+1 more"));
    }
}
