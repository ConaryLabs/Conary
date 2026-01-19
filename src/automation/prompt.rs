// src/automation/prompt.rs

//! User interaction prompts for automation decisions.
//!
//! Implements the "suggest + confirm" pattern where users are presented with
//! clear information about pending actions and can approve, reject, or defer.

use super::{ActionDecision, AutomationSummary, PendingAction};
use crate::error::Result;
use crate::model::AutomationCategory;
use std::io::{self, BufRead, IsTerminal, Write};

/// Style of prompt interaction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptStyle {
    /// Interactive TTY with colors and formatting
    Interactive,
    /// Simple text for non-TTY or scripting
    Simple,
    /// JSON output for programmatic consumption
    Json,
}

/// A prompt for user interaction
pub struct AutomationPrompt {
    style: PromptStyle,
}

impl AutomationPrompt {
    /// Create a new prompt with the given style
    pub fn new(style: PromptStyle) -> Self {
        Self { style }
    }

    /// Detect the appropriate prompt style based on environment
    pub fn detect() -> Self {
        let style = if std::io::stdout().is_terminal() {
            PromptStyle::Interactive
        } else {
            PromptStyle::Simple
        };
        Self { style }
    }

    /// Display the automation summary and ask what to do
    pub fn show_summary(&self, summary: &AutomationSummary) -> Result<SummaryResponse> {
        match self.style {
            PromptStyle::Interactive => self.show_summary_interactive(summary),
            PromptStyle::Simple => self.show_summary_simple(summary),
            PromptStyle::Json => self.show_summary_json(summary),
        }
    }

    fn show_summary_interactive(&self, summary: &AutomationSummary) -> Result<SummaryResponse> {
        let mut stdout = io::stdout();

        writeln!(stdout)?;
        writeln!(stdout, "=== Conary Automation Status ===")?;
        writeln!(stdout)?;

        if summary.total == 0 {
            writeln!(stdout, "  System is up to date. No actions pending.")?;
            writeln!(stdout)?;
            return Ok(SummaryResponse::Exit);
        }

        // Show categorized summary
        if summary.security_updates > 0 {
            writeln!(
                stdout,
                "  [SECURITY] {} security update(s) available",
                summary.security_updates
            )?;
        }
        if summary.available_updates > 0 {
            writeln!(
                stdout,
                "  [UPDATES]  {} package update(s) available",
                summary.available_updates
            )?;
        }
        if summary.orphaned_packages > 0 {
            writeln!(
                stdout,
                "  [CLEANUP]  {} orphaned package(s) can be removed",
                summary.orphaned_packages
            )?;
        }
        if summary.major_upgrades > 0 {
            writeln!(
                stdout,
                "  [MAJOR]    {} major upgrade(s) available",
                summary.major_upgrades
            )?;
        }
        if summary.integrity_issues > 0 {
            writeln!(
                stdout,
                "  [REPAIR]   {} integrity issue(s) detected",
                summary.integrity_issues
            )?;
        }

        writeln!(stdout)?;
        writeln!(stdout, "What would you like to do?")?;
        writeln!(stdout, "  [a] Apply all suggested changes")?;
        writeln!(stdout, "  [s] Review security updates only")?;
        writeln!(stdout, "  [d] Show details for all pending actions")?;
        writeln!(stdout, "  [c] Configure automation settings")?;
        writeln!(stdout, "  [n] Do nothing (exit)")?;
        writeln!(stdout)?;
        write!(stdout, "Choice [a/s/d/c/n]: ")?;
        stdout.flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;

        match input.trim().to_lowercase().as_str() {
            "a" | "apply" | "yes" | "y" => Ok(SummaryResponse::ApplyAll),
            "s" | "security" => Ok(SummaryResponse::ReviewCategory(AutomationCategory::Security)),
            "d" | "details" => Ok(SummaryResponse::ShowDetails),
            "c" | "config" | "configure" => Ok(SummaryResponse::Configure),
            "n" | "no" | "exit" | "q" | "" => Ok(SummaryResponse::Exit),
            _ => {
                writeln!(stdout, "Unknown option. Please try again.")?;
                self.show_summary_interactive(summary)
            }
        }
    }

    fn show_summary_simple(&self, summary: &AutomationSummary) -> Result<SummaryResponse> {
        println!("{}", summary.status_line());
        Ok(SummaryResponse::Exit)
    }

    fn show_summary_json(&self, summary: &AutomationSummary) -> Result<SummaryResponse> {
        let json = serde_json::json!({
            "total": summary.total,
            "security_updates": summary.security_updates,
            "available_updates": summary.available_updates,
            "orphaned_packages": summary.orphaned_packages,
            "major_upgrades": summary.major_upgrades,
            "integrity_issues": summary.integrity_issues,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
        Ok(SummaryResponse::Exit)
    }

    /// Prompt for a single action decision
    pub fn prompt_action(&self, action: &PendingAction) -> Result<ActionDecision> {
        match self.style {
            PromptStyle::Interactive => self.prompt_action_interactive(action),
            PromptStyle::Simple => self.prompt_action_simple(action),
            PromptStyle::Json => self.prompt_action_json(action),
        }
    }

    fn prompt_action_interactive(&self, action: &PendingAction) -> Result<ActionDecision> {
        let mut stdout = io::stdout();

        writeln!(stdout)?;
        writeln!(stdout, "--- {} ---", action.category.display_name())?;
        writeln!(stdout)?;
        writeln!(stdout, "  {}", action.summary)?;
        writeln!(stdout)?;

        // Show details
        if !action.details.is_empty() {
            writeln!(stdout, "  Details:")?;
            for detail in &action.details {
                writeln!(stdout, "    - {}", detail)?;
            }
            writeln!(stdout)?;
        }

        // Show affected packages
        if !action.packages.is_empty() {
            writeln!(stdout, "  Packages affected: {}", action.packages.join(", "))?;
        }

        // Show risk level
        let risk_text = if action.risk_level < 0.3 {
            "Low"
        } else if action.risk_level < 0.6 {
            "Medium"
        } else {
            "High"
        };
        writeln!(stdout, "  Risk level: {}", risk_text)?;

        // Show reboot requirement
        if action.requires_reboot {
            writeln!(stdout, "  Note: This action requires a reboot")?;
        }

        // Show reversibility
        if action.reversible {
            writeln!(stdout, "  Note: This action can be rolled back")?;
        }

        // Show deadline if present
        if let Some(deadline) = action.deadline {
            writeln!(stdout, "  Deadline: {}", deadline.format("%Y-%m-%d %H:%M"))?;
        }

        writeln!(stdout)?;
        writeln!(stdout, "  [Y] Yes, apply this change")?;
        writeln!(stdout, "  [n] No, skip this change")?;
        writeln!(stdout, "  [d] Defer until later")?;
        writeln!(stdout, "  [?] Show more details")?;
        writeln!(stdout)?;
        write!(stdout, "Apply? [Y/n/d/?]: ")?;
        stdout.flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;

        match input.trim().to_lowercase().as_str() {
            "y" | "yes" | "" => Ok(ActionDecision::Approved),
            "n" | "no" => Ok(ActionDecision::Rejected),
            "d" | "defer" | "later" => Ok(ActionDecision::Deferred { until: None }),
            "?" | "help" | "details" => Ok(ActionDecision::NeedsDetails),
            _ => {
                writeln!(stdout, "Unknown option. Please enter Y, n, d, or ?")?;
                self.prompt_action_interactive(action)
            }
        }
    }

    fn prompt_action_simple(&self, action: &PendingAction) -> Result<ActionDecision> {
        // In simple mode, just print the action info and skip
        println!("{}: {}", action.category.display_name(), action.summary);
        Ok(ActionDecision::Rejected)
    }

    fn prompt_action_json(&self, action: &PendingAction) -> Result<ActionDecision> {
        let json = serde_json::json!({
            "id": action.id,
            "category": format!("{:?}", action.category),
            "summary": action.summary,
            "details": action.details,
            "packages": action.packages,
            "risk_level": action.risk_level,
            "requires_reboot": action.requires_reboot,
            "reversible": action.reversible,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
        Ok(ActionDecision::Rejected)
    }

    /// Prompt for batch approval of multiple actions
    pub fn prompt_batch(&self, actions: &[&PendingAction]) -> Result<BatchDecision> {
        if actions.is_empty() {
            return Ok(BatchDecision::Skip);
        }

        match self.style {
            PromptStyle::Interactive => self.prompt_batch_interactive(actions),
            PromptStyle::Simple | PromptStyle::Json => Ok(BatchDecision::Skip),
        }
    }

    fn prompt_batch_interactive(&self, actions: &[&PendingAction]) -> Result<BatchDecision> {
        let mut stdout = io::stdout();

        writeln!(stdout)?;
        writeln!(stdout, "=== {} Action(s) Ready ===", actions.len())?;
        writeln!(stdout)?;

        for (i, action) in actions.iter().enumerate() {
            writeln!(
                stdout,
                "  {}. [{}] {}",
                i + 1,
                action.category.display_name(),
                action.summary
            )?;
        }

        writeln!(stdout)?;
        writeln!(stdout, "  [a] Apply all")?;
        writeln!(stdout, "  [r] Review each individually")?;
        writeln!(stdout, "  [s] Skip all")?;
        writeln!(stdout)?;
        write!(stdout, "Choice [a/r/s]: ")?;
        stdout.flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;

        match input.trim().to_lowercase().as_str() {
            "a" | "all" | "yes" | "y" => Ok(BatchDecision::ApplyAll),
            "r" | "review" => Ok(BatchDecision::ReviewEach),
            "s" | "skip" | "n" | "no" | "" => Ok(BatchDecision::Skip),
            _ => {
                writeln!(stdout, "Unknown option. Please try again.")?;
                self.prompt_batch_interactive(actions)
            }
        }
    }

    /// Display a confirmation message after actions are applied
    pub fn show_completion(&self, applied: usize, skipped: usize, failed: usize) -> Result<()> {
        match self.style {
            PromptStyle::Interactive | PromptStyle::Simple => {
                println!();
                if failed == 0 {
                    println!(
                        "Automation complete: {} applied, {} skipped",
                        applied, skipped
                    );
                } else {
                    println!(
                        "Automation complete: {} applied, {} skipped, {} failed",
                        applied, skipped, failed
                    );
                }
            }
            PromptStyle::Json => {
                let json = serde_json::json!({
                    "applied": applied,
                    "skipped": skipped,
                    "failed": failed,
                });
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
        }
        Ok(())
    }
}

/// Response to the summary prompt
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryResponse {
    /// Apply all pending actions
    ApplyAll,
    /// Review a specific category
    ReviewCategory(AutomationCategory),
    /// Show details for all actions
    ShowDetails,
    /// Open configuration
    Configure,
    /// Exit without changes
    Exit,
}

/// Response to batch prompt
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchDecision {
    /// Apply all actions in the batch
    ApplyAll,
    /// Review each action individually
    ReviewEach,
    /// Skip all actions
    Skip,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_style_detection() {
        // Can't really test atty in unit tests, but we can test construction
        let prompt = AutomationPrompt::new(PromptStyle::Simple);
        assert_eq!(prompt.style, PromptStyle::Simple);
    }

    #[test]
    fn test_summary_response_variants() {
        // Just ensure the enum variants exist and can be compared
        assert_ne!(SummaryResponse::ApplyAll, SummaryResponse::Exit);
        assert_eq!(
            SummaryResponse::ReviewCategory(AutomationCategory::Security),
            SummaryResponse::ReviewCategory(AutomationCategory::Security)
        );
    }
}
