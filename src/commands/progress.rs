// src/commands/progress.rs
//! Progress tracking for package operations
//!
//! Provides visual feedback during package installation, removal, and updates
//! with overall progress bars and per-operation status displays.

// Allow dead code - this is a new API module with types that will be used incrementally
#![allow(dead_code)]

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::time::Duration;

/// Installation progress tracker for multi-package operations
///
/// Displays an overall progress bar at the top with a status line below
/// showing the current operation.
pub struct InstallProgress {
    multi: MultiProgress,
    overall: ProgressBar,
    status: ProgressBar,
    total_packages: u64,
    completed: u64,
}

impl InstallProgress {
    /// Create a new installation progress tracker
    ///
    /// # Arguments
    /// * `total_packages` - Total number of packages to install
    /// * `operation` - Description of the operation (e.g., "Installing", "Updating")
    pub fn new(total_packages: u64, operation: &str) -> Self {
        let multi = MultiProgress::new();

        // Overall progress bar
        let overall = ProgressBar::new(total_packages);
        overall.set_style(
            ProgressStyle::default_bar()
                .template("{msg} ({pos}/{len}) [{bar:40.green/dim}] {percent}%")
                .expect("Invalid progress bar template")
                .progress_chars("##-"),
        );
        overall.set_message(operation.to_string());

        // Status line below (spinner with message)
        let status = ProgressBar::new_spinner();
        status.set_style(
            ProgressStyle::default_spinner()
                .template("  {spinner:.cyan} {msg}")
                .expect("Invalid spinner template"),
        );
        status.enable_steady_tick(Duration::from_millis(100));

        let overall = multi.add(overall);
        let status = multi.add(status);

        Self {
            multi,
            overall,
            status,
            total_packages,
            completed: 0,
        }
    }

    /// Create a minimal progress tracker for single-package operations
    pub fn single(operation: &str) -> Self {
        let multi = MultiProgress::new();

        // Just a spinner for single package
        let overall = ProgressBar::new_spinner();
        overall.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .expect("Invalid spinner template"),
        );
        overall.set_message(operation.to_string());
        overall.enable_steady_tick(Duration::from_millis(100));

        let status = ProgressBar::hidden();

        let overall = multi.add(overall);
        let status = multi.add(status);

        Self {
            multi,
            overall,
            status,
            total_packages: 1,
            completed: 0,
        }
    }

    /// Update the status message for the current operation
    pub fn set_status(&self, message: &str) {
        self.status.set_message(message.to_string());
    }

    /// Update status with package name and phase
    pub fn set_phase(&self, package: &str, phase: InstallPhase) {
        let msg = match phase {
            InstallPhase::Downloading => format!("Downloading {}...", package),
            InstallPhase::Parsing => format!("Parsing {}...", package),
            InstallPhase::ResolvingDeps => format!("Resolving dependencies for {}...", package),
            InstallPhase::InstallingDeps => format!("Installing dependencies for {}...", package),
            InstallPhase::Extracting => format!("Extracting {}...", package),
            InstallPhase::PreScript => format!("Running pre-install script for {}...", package),
            InstallPhase::Deploying => format!("Deploying files for {}...", package),
            InstallPhase::PostScript => format!("Running post-install script for {}...", package),
            InstallPhase::Verifying => format!("Verifying {}...", package),
            InstallPhase::Complete => format!("{} [done]", package),
            InstallPhase::Failed(ref err) => format!("{} [FAILED: {}]", package, err),
        };
        self.status.set_message(msg);
    }

    /// Mark a package as complete and advance the overall progress
    pub fn complete_package(&mut self, package: &str) {
        self.completed += 1;
        self.overall.set_position(self.completed);
        self.set_phase(package, InstallPhase::Complete);
    }

    /// Mark a package as failed
    pub fn fail_package(&mut self, package: &str, error: &str) {
        self.set_phase(package, InstallPhase::Failed(error.to_string()));
    }

    /// Add a sub-progress bar for file deployment
    pub fn add_file_progress(&self, total_files: u64, package: &str) -> ProgressBar {
        let pb = ProgressBar::new(total_files);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("    {msg} [{bar:30.blue/dim}] {pos}/{len}")
                .expect("Invalid progress bar template")
                .progress_chars("=>-"),
        );
        pb.set_message(format!("Files for {}", package));
        self.multi.add(pb)
    }

    /// Finish the overall progress with a success message
    pub fn finish(&self, message: &str) {
        self.status.finish_and_clear();
        self.overall.finish_with_message(message.to_string());
    }

    /// Finish the overall progress with a failure message
    pub fn finish_with_error(&self, message: &str) {
        self.status.finish_and_clear();
        self.overall.abandon_with_message(message.to_string());
    }

    /// Get the MultiProgress handle for adding custom progress bars
    pub fn multi(&self) -> &MultiProgress {
        &self.multi
    }
}

/// Phases of package installation
#[derive(Debug, Clone)]
pub enum InstallPhase {
    Downloading,
    Parsing,
    ResolvingDeps,
    InstallingDeps,
    Extracting,
    PreScript,
    Deploying,
    PostScript,
    Verifying,
    Complete,
    Failed(String),
}

/// Progress tracker for package removal
pub struct RemoveProgress {
    multi: MultiProgress,
    overall: ProgressBar,
    status: ProgressBar,
}

impl RemoveProgress {
    /// Create a new removal progress tracker
    pub fn new(package: &str) -> Self {
        let multi = MultiProgress::new();

        let overall = ProgressBar::new_spinner();
        overall.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.red} Removing {msg}...")
                .expect("Invalid spinner template"),
        );
        overall.set_message(package.to_string());
        overall.enable_steady_tick(Duration::from_millis(100));

        let status = ProgressBar::new_spinner();
        status.set_style(
            ProgressStyle::default_spinner()
                .template("  {spinner:.cyan} {msg}")
                .expect("Invalid spinner template"),
        );
        status.enable_steady_tick(Duration::from_millis(100));

        let overall = multi.add(overall);
        let status = multi.add(status);

        Self {
            multi,
            overall,
            status,
        }
    }

    /// Set the current phase of removal
    pub fn set_phase(&self, phase: RemovePhase) {
        let msg = match phase {
            RemovePhase::PreScript => "Running pre-remove script...",
            RemovePhase::RemovingFiles => "Removing files...",
            RemovePhase::RemovingDirs => "Cleaning up directories...",
            RemovePhase::PostScript => "Running post-remove script...",
            RemovePhase::UpdatingDb => "Updating database...",
        };
        self.status.set_message(msg.to_string());
    }

    /// Add a file removal progress bar
    pub fn add_file_progress(&self, total_files: u64) -> ProgressBar {
        let pb = ProgressBar::new(total_files);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("    Removing files [{bar:30.red/dim}] {pos}/{len}")
                .expect("Invalid progress bar template")
                .progress_chars("=>-"),
        );
        self.multi.add(pb)
    }

    /// Finish with success
    pub fn finish(&self, message: &str) {
        self.status.finish_and_clear();
        self.overall.finish_with_message(message.to_string());
    }

    /// Finish with error
    pub fn finish_with_error(&self, message: &str) {
        self.status.finish_and_clear();
        self.overall.abandon_with_message(message.to_string());
    }
}

/// Phases of package removal
#[derive(Debug, Clone, Copy)]
pub enum RemovePhase {
    PreScript,
    RemovingFiles,
    RemovingDirs,
    PostScript,
    UpdatingDb,
}

/// Progress tracker for update operations
pub struct UpdateProgress {
    multi: MultiProgress,
    overall: ProgressBar,
    status: ProgressBar,
    total: u64,
    completed: u64,
}

impl UpdateProgress {
    /// Create a new update progress tracker
    pub fn new(total_packages: u64) -> Self {
        let multi = MultiProgress::new();

        let overall = ProgressBar::new(total_packages);
        overall.set_style(
            ProgressStyle::default_bar()
                .template("Updating packages ({pos}/{len}) [{bar:40.yellow/dim}] {percent}%")
                .expect("Invalid progress bar template")
                .progress_chars("##-"),
        );

        let status = ProgressBar::new_spinner();
        status.set_style(
            ProgressStyle::default_spinner()
                .template("  {spinner:.cyan} {msg}")
                .expect("Invalid spinner template"),
        );
        status.enable_steady_tick(Duration::from_millis(100));

        let overall = multi.add(overall);
        let status = multi.add(status);

        Self {
            multi,
            overall,
            status,
            total: total_packages,
            completed: 0,
        }
    }

    /// Set status message
    pub fn set_status(&self, message: &str) {
        self.status.set_message(message.to_string());
    }

    /// Update phase for a specific package
    pub fn set_phase(&self, package: &str, phase: UpdatePhase) {
        let msg = match phase {
            UpdatePhase::CheckingDelta => format!("Checking delta for {}...", package),
            UpdatePhase::DownloadingDelta => format!("Downloading delta for {}...", package),
            UpdatePhase::ApplyingDelta => format!("Applying delta for {}...", package),
            UpdatePhase::DownloadingFull => format!("Downloading {}...", package),
            UpdatePhase::Installing => format!("Installing {}...", package),
            UpdatePhase::Complete => format!("{} [done]", package),
            UpdatePhase::Failed(ref err) => format!("{} [FAILED: {}]", package, err),
        };
        self.status.set_message(msg);
    }

    /// Complete a package update
    pub fn complete_package(&mut self, package: &str) {
        self.completed += 1;
        self.overall.set_position(self.completed);
        self.set_phase(package, UpdatePhase::Complete);
    }

    /// Mark a package update as failed
    pub fn fail_package(&mut self, package: &str, error: &str) {
        self.set_phase(package, UpdatePhase::Failed(error.to_string()));
    }

    /// Get the MultiProgress handle
    pub fn multi(&self) -> &MultiProgress {
        &self.multi
    }

    /// Add a download progress bar
    pub fn add_download_progress(&self, name: &str, size: u64) -> ProgressBar {
        let pb = ProgressBar::new(size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("    {msg} [{bar:30.cyan/dim}] {bytes}/{total_bytes} ({bytes_per_sec})")
                .expect("Invalid progress bar template")
                .progress_chars("#>-"),
        );
        pb.set_message(name.to_string());
        self.multi.add(pb)
    }

    /// Finish with success
    pub fn finish(&self, message: &str) {
        self.status.finish_and_clear();
        self.overall.finish_with_message(message.to_string());
    }
}

/// Phases of package update
#[derive(Debug, Clone)]
pub enum UpdatePhase {
    CheckingDelta,
    DownloadingDelta,
    ApplyingDelta,
    DownloadingFull,
    Installing,
    Complete,
    Failed(String),
}
