// apps/conary/src/commands/model/presentation.rs

use super::super::replatform_rendering::render_replatform_execution_plan;
use anyhow::Result;
use conary_core::model::{
    DiffAction, ModelDiff, ModelDiffSummary, VisibleRealignmentProposal, replatform_execution_plan,
};
use rusqlite::Connection;

pub(super) fn is_source_policy_action(_action: &DiffAction) -> bool {
    false
}

pub(super) fn is_replatform_action(action: &DiffAction) -> bool {
    matches!(action, DiffAction::ReplatformReplace { .. })
}

pub(super) fn source_policy_replatform_note(_diff: &ModelDiff) -> Option<String> {
    None
}

pub(super) fn model_check_drift_headline(diff: &ModelDiff) -> String {
    format!("DRIFT: {} difference(s) from model", diff.actions.len())
}

pub(super) fn render_replatform_summary(summary: &ModelDiffSummary) -> Option<String> {
    summary
        .visible_realignment_candidates
        .map(|candidates| format!("  Visible package-level realignment candidates: {candidates}"))
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
                proposal.package, proposal.target_source_identity, proposal.target_version
            );
            if let Some(arch) = &proposal.architecture {
                rendered.push_str(&format!(" [{arch}]"));
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

pub(super) fn print_source_policy_and_replatform(
    conn: &Connection,
    diff: &ModelDiff,
) -> Result<()> {
    if let Some(plan) = replatform_execution_plan(conn, &diff.actions)? {
        println!("{}", render_replatform_execution_plan(&plan));
    } else if let Some(proposals) = diff.visible_realignment_proposals.as_ref()
        && let Some(preview) = render_realignment_proposal_preview(proposals)
    {
        println!("{preview}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_headline_counts_model_actions_without_global_source_policy() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::Install {
            package: "kernel".to_string(),
            pin: None,
            optional: false,
        });
        assert_eq!(
            model_check_drift_headline(&diff),
            "DRIFT: 1 difference(s) from model"
        );
    }
}
