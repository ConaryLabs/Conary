// src/cli/automation.rs

//! CLI commands for automation system.
//!
//! These commands manage the suggest+confirm automation flow,
//! allowing users to review and apply automated changes.

use clap::Subcommand;
use super::{DbArgs, CommonArgs};

#[derive(Subcommand)]
pub enum AutomationCommands {
    /// Show automation status and pending actions
    ///
    /// Displays a summary of pending automation actions including
    /// security updates, orphaned packages, available updates, and
    /// any integrity issues detected.
    Status {
        #[command(flatten)]
        db: DbArgs,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,

        /// Show detailed information for each pending action
        #[arg(short, long)]
        verbose: bool,
    },

    /// Check for automation actions without applying
    ///
    /// Scans the system for actionable items:
    /// - Security updates available in repositories
    /// - Orphaned packages that can be removed
    /// - Package updates available
    /// - Integrity issues (corrupted files)
    Check {
        #[command(flatten)]
        common: CommonArgs,

        /// Only check specific categories (security, orphans, updates, integrity)
        #[arg(long, value_delimiter = ',')]
        categories: Option<Vec<String>>,

        /// Save results without prompting
        #[arg(long)]
        quiet: bool,
    },

    /// Review and apply pending automation actions
    ///
    /// Interactive mode to review each pending action and decide
    /// whether to apply, skip, or defer it.
    Apply {
        #[command(flatten)]
        common: CommonArgs,

        /// Apply all pending actions without prompting
        #[arg(long)]
        yes: bool,

        /// Only apply specific categories
        #[arg(long, value_delimiter = ',')]
        categories: Option<Vec<String>>,

        /// Show what would be done without making changes
        #[arg(long)]
        dry_run: bool,

        /// Skip running package scriptlets
        #[arg(long)]
        no_scripts: bool,
    },

    /// Configure automation settings
    ///
    /// View or modify automation configuration for the system model.
    /// Changes are written to /etc/conary/system.toml.
    Configure {
        #[command(flatten)]
        db: DbArgs,

        /// Show current configuration
        #[arg(long)]
        show: bool,

        /// Set global automation mode: suggest, auto, disabled
        #[arg(long)]
        mode: Option<String>,

        /// Enable/disable specific category (e.g., --enable security)
        #[arg(long)]
        enable: Option<String>,

        /// Disable specific category
        #[arg(long)]
        disable: Option<String>,

        /// Set check interval (e.g., "6h", "1d")
        #[arg(long)]
        interval: Option<String>,

        /// Enable AI assistance
        #[arg(long)]
        enable_ai: bool,

        /// Disable AI assistance
        #[arg(long)]
        disable_ai: bool,
    },

    /// Run automation daemon in background
    ///
    /// Starts a background process that periodically checks for
    /// automation actions and either applies them automatically
    /// or queues them for review based on configuration.
    Daemon {
        #[command(flatten)]
        common: CommonArgs,

        /// Run in foreground (don't daemonize)
        #[arg(long)]
        foreground: bool,

        /// PID file location
        #[arg(long, default_value = "/run/conary/automation.pid")]
        pidfile: String,
    },

    /// Show automation history
    ///
    /// Displays a log of past automation actions including
    /// what was applied, when, and the outcome.
    History {
        #[command(flatten)]
        db: DbArgs,

        /// Number of entries to show
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,

        /// Filter by category
        #[arg(long)]
        category: Option<String>,

        /// Filter by status: applied, skipped, failed
        #[arg(long)]
        status: Option<String>,

        /// Show entries since date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,
    },

    /// AI-assisted operations
    ///
    /// Use AI assistance for package management tasks.
    /// Requires AI assistance to be enabled in configuration.
    ///
    /// Note: This is an experimental feature. Build with --features experimental to enable.
    #[cfg(feature = "experimental")]
    #[command(subcommand)]
    Ai(AiCommands),
}

#[cfg(feature = "experimental")]
#[derive(Subcommand)]
pub enum AiCommands {
    /// Find packages by intent (what you want to accomplish)
    ///
    /// Instead of specifying a package name, describe what you need:
    /// "web server with HTTP/2 and systemd integration"
    Find {
        /// Description of what you're looking for
        intent: String,

        #[command(flatten)]
        db: DbArgs,

        /// Maximum number of suggestions
        #[arg(short = 'n', long, default_value = "5")]
        limit: usize,

        /// Show detailed comparison of options
        #[arg(short, long)]
        verbose: bool,
    },

    /// Translate a scriptlet to declarative hooks
    ///
    /// Uses AI to analyze a bash scriptlet and suggest
    /// equivalent declarative CCS hooks.
    Translate {
        /// Path to scriptlet file or package name
        source: String,

        /// Output format: toml, json
        #[arg(long, default_value = "toml")]
        format: String,

        /// Minimum confidence threshold (0.0-1.0)
        #[arg(long, default_value = "0.8")]
        confidence: f64,
    },

    /// Query system state in natural language
    ///
    /// Ask questions about your system:
    /// "What changed since Tuesday?"
    /// "Which packages depend on openssl?"
    Query {
        /// Your question
        question: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Explain what an action would do
    ///
    /// Get a detailed explanation of what a package operation
    /// would do before running it.
    Explain {
        /// Command to explain (e.g., "install nginx")
        command: String,

        #[command(flatten)]
        db: DbArgs,
    },
}
