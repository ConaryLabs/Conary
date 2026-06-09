// src/commands/model/presentation.rs

use super::super::replatform_rendering::render_replatform_execution_plan;
use anyhow::Result;
use conary_core::model::{
    DiffAction, ModelDiff, ModelDiffSummary, ReplatformEstimate, ReplatformStatus,
    VisibleRealignmentProposal, replatform_execution_plan,
};
use rusqlite::Connection;

pub(super) fn is_source_policy_action(action: &DiffAction) -> bool {
    matches!(
        action,
        DiffAction::SetSourcePin { .. }
            | DiffAction::ClearSourcePin
            | DiffAction::SetSelectionMode { .. }
            | DiffAction::ClearSelectionMode
            | DiffAction::SetAllowedDistros { .. }
            | DiffAction::ClearAllowedDistros
    )
}

pub(super) fn is_replatform_action(action: &DiffAction) -> bool {
    matches!(action, DiffAction::ReplatformReplace { .. })
}

pub(super) fn source_policy_summary(diff: &ModelDiff) -> Option<String> {
    if !diff.has_source_policy_changes() {
        return None;
    }

    Some(match diff.replatform_status() {
        Some(ReplatformStatus::PackageConvergencePlanned { .. }) => {
            "This is a source-policy transition with package convergence. Review the package plan carefully before applying."
                .to_string()
        }
        Some(ReplatformStatus::PendingWithEstimate(_)) | Some(ReplatformStatus::PolicyOnlyPending) => {
            "This is a source-policy-only transition. Applying it updates Conary's preferred package source policy now; package realignment remains limited to transactions that are already executable."
                .to_string()
        }
        None => return None,
    })
}

fn source_policy_replatform_estimate(
    estimate: Option<&ReplatformEstimate>,
    has_structural_changes: bool,
) -> Option<String> {
    if has_structural_changes {
        return None;
    }
    estimate.map(|estimate| {
        format!(
        "Estimated replatform scope: about {} installed package(s) already align with {}, and about {} package(s) may need source realignment.",
        estimate.aligned_packages, estimate.target_distro, estimate.packages_to_realign
    )
    })
}

pub(super) fn source_policy_replatform_note(diff: &ModelDiff) -> Option<String> {
    match diff.replatform_status() {
        Some(ReplatformStatus::PendingWithEstimate(estimate)) => {
            source_policy_replatform_estimate(Some(&estimate), false)
        }
        Some(ReplatformStatus::PolicyOnlyPending) => Some(
            "Replatform estimate unavailable: no source affinity data yet. Run a repo sync or refresh affinity data first."
                .to_string(),
        ),
        _ => None,
    }
}

pub(super) fn model_check_drift_headline(diff: &ModelDiff) -> String {
    match diff.replatform_status() {
        Some(ReplatformStatus::PendingWithEstimate(estimate)) => format!(
            "DRIFT: source policy pending replatform estimate for {} (about {} package(s) may need realignment)",
            estimate.target_distro, estimate.packages_to_realign
        ),
        Some(ReplatformStatus::PolicyOnlyPending) => {
            match diff.summary().visible_realignment_candidates {
                Some(candidates) => format!(
                    "DRIFT: source policy changed; replatform planning is still pending ({} visible package candidate(s))",
                    candidates
                ),
                None => {
                    "DRIFT: source policy changed; replatform planning is still pending".to_string()
                }
            }
        }
        Some(ReplatformStatus::PackageConvergencePlanned { structural_changes }) => format!(
            "DRIFT: source policy transition with {} planned package change(s)",
            structural_changes
        ),
        None => format!("DRIFT: {} difference(s) from model", diff.actions.len()),
    }
}

pub(super) fn render_replatform_summary(summary: &ModelDiffSummary) -> Option<String> {
    if let Some(packages) = summary.replatform_pending_packages {
        return Some(format!(
            "  Replatform pending estimate: {} package(s) may need realignment",
            packages
        ));
    }

    if let Some(changes) = summary.planned_package_convergence {
        return Some(format!(
            "  Planned package convergence changes: {}",
            changes
        ));
    }

    if let Some(candidates) = summary.visible_realignment_candidates {
        return Some(format!(
            "  Visible package-level realignment candidates: {}",
            candidates
        ));
    }

    None
}

fn render_realignment_proposal_preview(proposals: &[VisibleRealignmentProposal]) -> Option<String> {
    if proposals.is_empty() {
        return None;
    }

    let preview: Vec<String> = proposals
        .iter()
        .take(3)
        .map(|proposal| {
            let mut rendered = format!(
                "{} -> {} {}",
                proposal.package, proposal.target_distro, proposal.target_version
            );
            if let Some(arch) = &proposal.architecture {
                rendered.push_str(&format!(" [{}]", arch));
            }
            rendered
        })
        .collect();

    let mut line = format!("  Visible realignment proposals: {}", preview.join(", "));
    if proposals.len() > preview.len() {
        line.push_str(&format!(", +{} more", proposals.len() - preview.len()));
    }
    Some(line)
}

/// Print source policy summary, replatform estimate note, and replatform
/// plan (or realignment proposal preview).  Shared by `cmd_model_diff` and
/// `cmd_model_apply`.
pub(super) fn print_source_policy_and_replatform(
    conn: &Connection,
    diff: &ModelDiff,
) -> Result<()> {
    if let Some(summary) = source_policy_summary(diff) {
        println!("{}", summary);
        println!();
    }

    if let Some(estimate) = source_policy_replatform_note(diff) {
        println!("{}", estimate);
        println!();
    }

    if let Some(plan) = replatform_execution_plan(conn, &diff.actions)? {
        println!("{}", render_replatform_execution_plan(&plan));
    } else if let Some(proposals) = diff.visible_realignment_proposals.as_ref()
        && let Some(preview) = render_realignment_proposal_preview(proposals)
    {
        println!("{}", preview);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::model::{DiffAction, ModelDiff, ModelDiffSummary, ReplatformEstimate};

    #[test]
    fn test_source_policy_summary_for_policy_only_transition() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });

        let summary = source_policy_summary(&diff).unwrap();

        assert!(summary.contains("source-policy-only transition"));
        assert!(summary.contains("updates Conary's preferred package source policy now"));
    }

    #[test]
    fn test_source_policy_summary_for_transition_with_package_changes() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.actions.push(DiffAction::Install {
            package: "kernel".to_string(),
            pin: None,
            optional: false,
        });

        let summary = source_policy_summary(&diff).unwrap();

        assert!(summary.contains("source-policy transition with package convergence"));
    }

    #[test]
    fn test_source_policy_summary_policy_only_stays_conservative() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::ClearSourcePin);

        let summary = source_policy_summary(&diff).unwrap();

        assert!(summary.contains("transactions that are already executable"));
        assert!(!summary.contains("replaced"));
    }

    #[test]
    fn test_source_policy_replatform_note_falls_back_when_affinity_missing() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });

        let note = source_policy_replatform_note(&diff).unwrap();

        assert!(note.contains("no source affinity data yet"));
    }

    #[test]
    fn test_model_check_drift_headline_for_pending_estimate() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.replatform_estimate = Some(ReplatformEstimate {
            target_distro: "arch".to_string(),
            aligned_packages: 10,
            packages_to_realign: 30,
            total_packages: 40,
        });

        let headline = model_check_drift_headline(&diff);

        assert!(headline.contains("pending replatform estimate for arch"));
        assert!(headline.contains("30 package(s) may need realignment"));
    }

    #[test]
    fn test_model_check_drift_headline_for_policy_only_pending() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::ClearSourcePin);

        let headline = model_check_drift_headline(&diff);

        assert!(headline.contains("source policy changed"));
        assert!(headline.contains("replatform planning is still pending"));
    }

    #[test]
    fn test_model_check_drift_headline_mentions_visible_candidates_when_estimate_missing() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.visible_realignment_candidates =
            Some(conary_core::model::VisibleRealignmentCandidates {
                target_distro: "arch".to_string(),
                candidate_count: 5,
            });

        let headline = model_check_drift_headline(&diff);

        assert!(headline.contains("source policy changed"));
        assert!(headline.contains("5 visible package candidate(s)"));
    }

    #[test]
    fn test_model_check_drift_headline_for_package_convergence() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.actions.push(DiffAction::Install {
            package: "kernel".to_string(),
            pin: None,
            optional: false,
        });

        let headline = model_check_drift_headline(&diff);

        assert!(headline.contains("source policy transition"));
        assert!(headline.contains("1 planned package change(s)"));
    }

    #[test]
    fn test_render_replatform_summary_for_pending_estimate() {
        let summary = ModelDiffSummary {
            installs: 0,
            removes: 0,
            source_policy_changes: 1,
            other_changes: 0,
            warnings: 1,
            replatform_pending_packages: Some(30),
            planned_package_convergence: None,
            visible_realignment_candidates: None,
        };

        let rendered = render_replatform_summary(&summary).unwrap();

        assert!(rendered.contains("Replatform pending estimate"));
        assert!(rendered.contains("30 package(s) may need realignment"));
    }

    #[test]
    fn test_render_replatform_summary_for_visible_candidates() {
        let summary = ModelDiffSummary {
            installs: 0,
            removes: 0,
            source_policy_changes: 1,
            other_changes: 0,
            warnings: 0,
            replatform_pending_packages: None,
            planned_package_convergence: None,
            visible_realignment_candidates: Some(5),
        };

        let rendered = render_replatform_summary(&summary).unwrap();

        assert!(rendered.contains("Visible package-level realignment candidates"));
        assert!(rendered.contains("5"));
    }

    #[test]
    fn test_render_realignment_proposal_preview_lists_examples() {
        let proposals = vec![
            conary_core::model::VisibleRealignmentProposal {
                package: "bash".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                target_version: "5.2.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(11),
            },
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
                package: "zsh".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                target_version: "5.9.1".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(33),
            },
            conary_core::model::VisibleRealignmentProposal {
                package: "curl".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                target_version: "8.8.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(44),
            },
        ];

        let rendered = render_realignment_proposal_preview(&proposals).unwrap();

        assert!(rendered.contains("bash -> arch 5.2.0"));
        assert!(rendered.contains("vim -> arch 9.1.0"));
        assert!(rendered.contains("zsh -> arch 5.9.1"));
        assert!(rendered.contains("+1 more"));
    }
}
