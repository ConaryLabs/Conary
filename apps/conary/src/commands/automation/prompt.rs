// apps/conary/src/commands/automation/prompt.rs
//! Terminal presentation and input for typed automation choices.

use anyhow::Result;
use conary_core::automation::{AutomationSummary, prompt::SummaryResponse};
use conary_core::model::AutomationCategory;
use std::io::{self, BufRead, IsTerminal, Write};

pub(super) fn show_summary(summary: &AutomationSummary) -> Result<SummaryResponse> {
    if !io::stdout().is_terminal() {
        crate::ui::row(crate::ui::Status::Info, &[&summary.status_line()]);
        return Ok(SummaryResponse::Exit);
    }
    let mut stdout = io::stdout();

    writeln!(stdout)?;
    writeln!(stdout, "=== Conary Automation Status ===")?;
    writeln!(stdout)?;

    if summary.total == 0 {
        writeln!(stdout, "  System is up to date. No actions pending.")?;
        writeln!(stdout)?;
        return Ok(SummaryResponse::Exit);
    }

    for (count, label) in [
        (summary.security_updates, "security update(s) available"),
        (summary.available_updates, "package update(s) available"),
        (
            summary.orphaned_packages,
            "orphaned package(s) can be removed",
        ),
        (summary.major_upgrades, "major upgrade(s) available"),
        (summary.integrity_issues, "integrity issue(s) detected"),
    ] {
        if count > 0 {
            writeln!(
                stdout,
                "{}",
                crate::ui::row_line(crate::ui::Status::Pending, &[&format!("{count} {label}")])
            )?;
        }
    }

    loop {
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
        if io::stdin().lock().read_line(&mut input)? == 0 {
            return Ok(SummaryResponse::Exit);
        }
        if let Some(decision) = parse_summary_choice(&input) {
            return Ok(decision);
        }
        writeln!(stdout, "Unknown option. Please try again.")?;
    }
}

fn parse_summary_choice(input: &str) -> Option<SummaryResponse> {
    match input.trim().to_lowercase().as_str() {
        "a" | "apply" | "yes" | "y" => Some(SummaryResponse::ApplyAll),
        "s" | "security" => Some(SummaryResponse::ReviewCategory(
            AutomationCategory::Security,
        )),
        "d" | "details" => Some(SummaryResponse::ShowDetails),
        "c" | "config" | "configure" => Some(SummaryResponse::Configure),
        "n" | "no" | "exit" | "q" | "" => Some(SummaryResponse::Exit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_input_requires_an_explicit_application_choice() {
        for input in ["", " ", "n", "NO", "q", "exit"] {
            assert_eq!(parse_summary_choice(input), Some(SummaryResponse::Exit));
        }
        assert_eq!(
            parse_summary_choice(" yes "),
            Some(SummaryResponse::ApplyAll)
        );
        assert_eq!(
            parse_summary_choice("s"),
            Some(SummaryResponse::ReviewCategory(
                AutomationCategory::Security
            ))
        );
        assert_eq!(parse_summary_choice("garbage"), None);
    }
}
