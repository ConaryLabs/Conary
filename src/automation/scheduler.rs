// src/automation/scheduler.rs

//! Scheduler for periodic automation checks.
//!
//! Manages timing of automation checks based on configuration:
//! - Global check interval
//! - Per-category check frequencies
//! - Maintenance windows

use crate::error::Result;
use crate::model::AutomationConfig;
use chrono::{DateTime, Local, NaiveTime, Timelike, Utc};
use std::time::Duration;

/// State of the automation scheduler
#[derive(Debug, Clone)]
pub struct SchedulerState {
    /// Last time automation check was run
    pub last_check: Option<DateTime<Utc>>,

    /// Next scheduled check time
    pub next_check: Option<DateTime<Utc>>,

    /// Whether scheduler is enabled
    pub enabled: bool,

    /// Reason if scheduler is paused
    pub pause_reason: Option<String>,
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self {
            last_check: None,
            next_check: None,
            enabled: true,
            pause_reason: None,
        }
    }
}

/// Scheduler for automation checks
pub struct AutomationScheduler {
    config: AutomationConfig,
    state: SchedulerState,
}

impl AutomationScheduler {
    /// Create a new scheduler with the given configuration
    pub fn new(config: AutomationConfig) -> Self {
        let mut scheduler = Self {
            config,
            state: SchedulerState::default(),
        };
        scheduler.calculate_next_check();
        scheduler
    }

    /// Check if it's time to run automation checks
    pub fn should_run(&self) -> bool {
        if !self.state.enabled {
            return false;
        }

        match self.state.next_check {
            Some(next) => Utc::now() >= next,
            None => true, // No next check scheduled, run now
        }
    }

    /// Check if we're within the configured maintenance window
    pub fn within_window(&self) -> bool {
        let window = match &self.config.updates.window {
            Some(w) => w,
            None => return true, // No window configured, always allowed
        };

        let (start, end) = match parse_time_window(window) {
            Some(times) => times,
            None => return true, // Invalid window, allow
        };

        let now = Local::now().time();
        if start <= end {
            now >= start && now <= end
        } else {
            // Window spans midnight (e.g., 22:00-06:00)
            now >= start || now <= end
        }
    }

    /// Record that a check was performed
    pub fn record_check(&mut self) {
        self.state.last_check = Some(Utc::now());
        self.calculate_next_check();
    }

    /// Calculate the next check time
    fn calculate_next_check(&mut self) {
        let interval = match super::parse_duration(&self.config.check_interval) {
            Ok(d) => d,
            Err(_) => Duration::from_secs(6 * 3600), // Default 6 hours
        };

        let base = self.state.last_check.unwrap_or_else(Utc::now);
        let next = base + chrono::Duration::from_std(interval).unwrap_or_default();
        self.state.next_check = Some(next);
    }

    /// Pause the scheduler with a reason
    pub fn pause(&mut self, reason: impl Into<String>) {
        self.state.enabled = false;
        self.state.pause_reason = Some(reason.into());
    }

    /// Resume the scheduler
    pub fn resume(&mut self) {
        self.state.enabled = true;
        self.state.pause_reason = None;
    }

    /// Get current scheduler state
    pub fn state(&self) -> &SchedulerState {
        &self.state
    }

    /// Get time until next scheduled check
    pub fn time_until_next(&self) -> Option<Duration> {
        self.state.next_check.map(|next| {
            let now = Utc::now();
            if next > now {
                (next - now).to_std().unwrap_or(Duration::ZERO)
            } else {
                Duration::ZERO
            }
        })
    }

    /// Format status for display
    pub fn status_line(&self) -> String {
        if !self.state.enabled {
            return format!(
                "Scheduler paused: {}",
                self.state.pause_reason.as_deref().unwrap_or("unknown reason")
            );
        }

        match self.state.next_check {
            Some(next) => {
                let now = Utc::now();
                if next <= now {
                    "Check due now".to_string()
                } else {
                    let duration = next - now;
                    format_duration(duration)
                }
            }
            None => "Not scheduled".to_string(),
        }
    }
}

/// Parse a time window string like "02:00-06:00" into start/end times
fn parse_time_window(window: &str) -> Option<(NaiveTime, NaiveTime)> {
    let parts: Vec<&str> = window.split('-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start = NaiveTime::parse_from_str(parts[0].trim(), "%H:%M").ok()?;
    let end = NaiveTime::parse_from_str(parts[1].trim(), "%H:%M").ok()?;

    Some((start, end))
}

/// Format a chrono Duration for display
fn format_duration(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds();
    if total_secs < 0 {
        return "overdue".to_string();
    }

    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;

    if hours > 24 {
        let days = hours / 24;
        format!("Next check in {} day(s)", days)
    } else if hours > 0 {
        format!("Next check in {}h {}m", hours, minutes)
    } else if minutes > 0 {
        format!("Next check in {} minute(s)", minutes)
    } else {
        "Check due soon".to_string()
    }
}

/// Daemon mode runner for automation
pub struct AutomationDaemon {
    scheduler: AutomationScheduler,
    running: bool,
}

impl AutomationDaemon {
    /// Create a new daemon with the given configuration
    pub fn new(config: AutomationConfig) -> Self {
        Self {
            scheduler: AutomationScheduler::new(config),
            running: false,
        }
    }

    /// Start the daemon (blocking)
    pub fn run(&mut self) -> Result<()> {
        self.running = true;
        tracing::info!("Automation daemon started");

        while self.running {
            if self.scheduler.should_run() && self.scheduler.within_window() {
                tracing::info!("Running scheduled automation check");
                // Would call AutomationChecker here
                self.scheduler.record_check();
            }

            // Sleep until next check or 1 minute, whichever is shorter
            let sleep_duration = self
                .scheduler
                .time_until_next()
                .map(|d| d.min(Duration::from_secs(60)))
                .unwrap_or(Duration::from_secs(60));

            std::thread::sleep(sleep_duration);
        }

        Ok(())
    }

    /// Signal the daemon to stop
    pub fn stop(&mut self) {
        self.running = false;
        tracing::info!("Automation daemon stopping");
    }

    /// Get the scheduler
    pub fn scheduler(&self) -> &AutomationScheduler {
        &self.scheduler
    }

    /// Record that a check was performed
    pub fn record_check(&mut self) {
        self.scheduler.record_check();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time_window() {
        let (start, end) = parse_time_window("02:00-06:00").unwrap();
        assert_eq!(start.hour(), 2);
        assert_eq!(end.hour(), 6);

        let (start, end) = parse_time_window("22:00-04:00").unwrap();
        assert_eq!(start.hour(), 22);
        assert_eq!(end.hour(), 4);
    }

    #[test]
    fn test_scheduler_creation() {
        let config = AutomationConfig::default();
        let scheduler = AutomationScheduler::new(config);

        assert!(scheduler.state.enabled);
        assert!(scheduler.state.next_check.is_some());
    }

    #[test]
    fn test_scheduler_pause_resume() {
        let config = AutomationConfig::default();
        let mut scheduler = AutomationScheduler::new(config);

        scheduler.pause("Manual pause");
        assert!(!scheduler.state.enabled);
        assert_eq!(scheduler.state.pause_reason, Some("Manual pause".to_string()));

        scheduler.resume();
        assert!(scheduler.state.enabled);
        assert!(scheduler.state.pause_reason.is_none());
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(
            format_duration(chrono::Duration::hours(2)),
            "Next check in 2h 0m"
        );
        assert_eq!(
            format_duration(chrono::Duration::minutes(45)),
            "Next check in 45 minute(s)"
        );
        assert_eq!(
            format_duration(chrono::Duration::days(2)),
            "Next check in 2 day(s)"
        );
    }
}
