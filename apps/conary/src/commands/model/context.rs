// src/commands/model/context.rs

use std::path::Path;

use super::super::open_db;
use anyhow::{Result, anyhow};
use conary_core::db::models::SystemAffinity;
use conary_core::model::parser::SystemModel;
use conary_core::model::{
    DiffAction, ModelDiff, ReplatformEstimate, SystemState, capture_current_state, compute_diff,
    compute_diff_with_includes_offline, parse_model_file, planned_replatform_actions,
    replatform_estimate_from_affinities, source_policy_replatform_snapshot,
};
use rusqlite::Connection;

pub(super) fn load_model(model_path: &Path) -> Result<SystemModel> {
    if !model_path.exists() {
        return Err(anyhow!("Model file not found: {}", model_path.display()));
    }
    Ok(parse_model_file(model_path)?)
}

pub(super) async fn load_model_and_diff(
    model_path: &Path,
    db_path: &str,
    offline: bool,
    announce_includes: bool,
) -> Result<(SystemModel, Connection, ModelDiff)> {
    let model = load_model(model_path)?;
    let conn = open_db(db_path)?;
    let state = capture_current_state(&conn)?;
    let diff = compute_model_diff(&model, &state, &conn, offline, announce_includes).await?;
    Ok((model, conn, diff))
}

pub(super) async fn compute_model_diff(
    model: &SystemModel,
    state: &SystemState,
    conn: &Connection,
    offline: bool,
    announce: bool,
) -> Result<ModelDiff> {
    let mut diff = if model.has_includes() {
        if announce {
            let mode = if offline { " (offline mode)" } else { "" };
            println!(
                "Resolving {} remote include(s){}...",
                model.include.models.len(),
                mode
            );
        }
        compute_diff_with_includes_offline(model, state, conn, offline).await?
    } else {
        compute_diff(model, state)
    };
    diff.replatform_estimate = compute_replatform_estimate(&diff, &SystemAffinity::list(conn)?);
    if let Some(target_distro) = diff.actions.iter().find_map(|action| match action {
        DiffAction::SetSourcePin { distro, .. } => Some(distro.as_str()),
        _ => None,
    }) {
        let snapshot = source_policy_replatform_snapshot(conn, target_distro)?;
        diff.visible_realignment_candidates =
            Some(conary_core::model::VisibleRealignmentCandidates {
                target_distro: snapshot.target_distro.clone(),
                candidate_count: snapshot.visible_realignment_candidates,
            });
        diff.visible_realignment_proposals = Some(snapshot.visible_realignment_proposals);
        let planned_actions = planned_replatform_actions(
            &conary_core::model::SourcePolicyReplatformSnapshot {
                target_distro: target_distro.to_string(),
                estimate: diff.replatform_estimate.clone(),
                visible_realignment_candidates: diff
                    .visible_realignment_proposals
                    .as_ref()
                    .map(|items| items.len())
                    .unwrap_or(0),
                visible_realignment_proposals: diff
                    .visible_realignment_proposals
                    .clone()
                    .unwrap_or_default(),
            },
            state,
        );
        if diff.structural_change_count() == 0 {
            diff.actions.extend(planned_actions);
        }
    }
    Ok(diff)
}

fn compute_replatform_estimate(
    diff: &ModelDiff,
    affinities: &[SystemAffinity],
) -> Option<ReplatformEstimate> {
    if diff.structural_change_count() > 0 {
        return None;
    }

    let target_distro = diff.actions.iter().find_map(|action| match action {
        DiffAction::SetSourcePin { distro, .. } => Some(distro.clone()),
        _ => None,
    })?;

    replatform_estimate_from_affinities(affinities, &target_distro)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::{create_test_db, seed_mixed_replatform_fixture};
    use conary_core::model::{ReplatformBlockedReason, replatform_execution_plan};

    #[test]
    fn test_source_policy_replatform_estimate_uses_affinity_counts() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
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

        let estimate = compute_replatform_estimate(&diff, &affinities).unwrap();

        assert_eq!(estimate.target_distro, "arch");
        assert_eq!(estimate.aligned_packages, 10);
        assert_eq!(estimate.packages_to_realign, 30);
        assert_eq!(estimate.total_packages, 40);
    }

    #[test]
    fn test_source_policy_replatform_estimate_handles_missing_affinity_data() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });

        let estimate = compute_replatform_estimate(&diff, &[]);

        assert!(estimate.is_none());
    }

    #[tokio::test]
    async fn test_compute_model_diff_surfaces_mixed_replatform_execution_states() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        seed_mixed_replatform_fixture(&conn);

        let model: SystemModel = toml::from_str(
            r#"
[model]
version = 1

[system]
profile = "balanced/latest-anywhere"

[system.pin]
distro = "arch"
strength = "strict"
"#,
        )
        .unwrap();

        let state = capture_current_state(&conn).unwrap();
        let diff = compute_model_diff(&model, &state, &conn, false, false)
            .await
            .unwrap();

        assert_eq!(
            diff.actions
                .iter()
                .filter(|action| matches!(action, DiffAction::ReplatformReplace { .. }))
                .count(),
            3
        );

        let plan = replatform_execution_plan(&conn, &diff.actions)
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
    fn test_planned_replatform_actions_promote_proposals_into_actions() {
        let snapshot = conary_core::model::SourcePolicyReplatformSnapshot {
            target_distro: "arch".to_string(),
            estimate: Some(ReplatformEstimate {
                target_distro: "arch".to_string(),
                aligned_packages: 10,
                packages_to_realign: 2,
                total_packages: 12,
            }),
            visible_realignment_candidates: 2,
            visible_realignment_proposals: vec![
                conary_core::model::VisibleRealignmentProposal {
                    package: "vim".to_string(),
                    current_distro: Some("fedora-44".to_string()),
                    target_distro: "arch".to_string(),
                    target_version: "9.1.0".to_string(),
                    architecture: Some("x86_64".to_string()),
                    target_repository: Some("arch-core".to_string()),
                    target_repository_package_id: Some(22),
                },
                conary_core::model::VisibleRealignmentProposal {
                    package: "bash".to_string(),
                    current_distro: Some("fedora-44".to_string()),
                    target_distro: "arch".to_string(),
                    target_version: "5.2.0".to_string(),
                    architecture: Some("x86_64".to_string()),
                    target_repository: Some("arch-core".to_string()),
                    target_repository_package_id: Some(11),
                },
            ],
        };
        let mut state = SystemState::new();
        state.add_package(
            "vim".to_string(),
            conary_core::model::InstalledPackage {
                name: "vim".to_string(),
                version: "9.0.1".to_string(),
                architecture: Some("x86_64".to_string()),
                explicit: true,
                pinned: false,
                label: Some("fedora@f43:stable".to_string()),
            },
        );
        state.add_package(
            "bash".to_string(),
            conary_core::model::InstalledPackage {
                name: "bash".to_string(),
                version: "5.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                explicit: true,
                pinned: false,
                label: Some("fedora@f43:stable".to_string()),
            },
        );

        let actions = planned_replatform_actions(&snapshot, &state);

        assert!(actions.iter().any(|action| {
            matches!(
                action,
                DiffAction::ReplatformReplace {
                    package,
                    current_distro,
                    target_distro,
                    current_version,
                    current_architecture,
                    target_version,
                    target_repository,
                    target_repository_package_id,
                    ..
                } if package == "vim"
                    && current_distro.as_deref() == Some("fedora-44")
                    && target_distro == "arch"
                    && current_version == "9.0.1"
                    && current_architecture.as_deref() == Some("x86_64")
                    && target_version == "9.1.0"
                    && target_repository.as_deref() == Some("arch-core")
                    && *target_repository_package_id == Some(22)
            )
        }));
        assert_eq!(actions.len(), 2);
    }
}
