// src/progress.rs

//! Shared progress tracking trait and implementations
//!
//! This module provides a unified interface for progress reporting across
//! different operations (install, remove, update, download, etc.) and
//! different output modes (CLI progress bars, logging, silent).
//!
//! # Design
//!
//! The `ProgressTracker` trait defines the core interface. Implementations
//! include:
//! - `CliProgress`: Visual progress bars using indicatif
//! - `LogProgress`: Logs progress to tracing
//! - `SilentProgress`: No-op for scripted/quiet modes
//!
//! # Example
//!
//! ```ignore
//! use conary::progress::{ProgressTracker, CliProgress, ProgressStyle};
//!
//! let progress = CliProgress::new("Installing packages", 5, ProgressStyle::Bar);
//!
//! for package in packages {
//!     progress.set_message(&format!("Installing {}", package));
//!     // ... do work ...
//!     progress.increment(1);
//! }
//!
//! progress.finish_with_message("Installation complete");
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::info;

/// Progress reporting style
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressStyle {
    /// Progress bar with percentage (for known totals)
    #[default]
    Bar,
    /// Spinner (for unknown totals or indeterminate progress)
    Spinner,
    /// Bytes transfer (shows bytes/total and speed)
    Bytes,
}

/// Core trait for progress tracking
///
/// Implementations should be thread-safe (Send + Sync) to allow
/// progress updates from multiple threads.
pub trait ProgressTracker: Send + Sync {
    /// Set the current status message
    fn set_message(&self, message: &str);

    /// Increment progress by the given amount
    fn increment(&self, amount: u64);

    /// Set progress to a specific position
    fn set_position(&self, position: u64);

    /// Set the total (length) of the progress
    fn set_length(&self, length: u64);

    /// Get current position
    fn position(&self) -> u64;

    /// Get total length
    fn length(&self) -> u64;

    /// Finish progress successfully with a message
    fn finish_with_message(&self, message: &str);

    /// Finish progress with an error/abandonment message
    fn finish_with_error(&self, message: &str);

    /// Check if progress is finished
    fn is_finished(&self) -> bool;

    /// Create a child progress tracker (for nested operations)
    fn child(&self, message: &str, length: u64, style: ProgressStyle) -> Box<dyn ProgressTracker>;
}

/// Silent progress tracker (no-op)
///
/// Use this for quiet mode, scripted usage, or when progress output
/// is not desired.
#[derive(Debug, Default)]
pub struct SilentProgress {
    position: AtomicU64,
    length: AtomicU64,
    finished: std::sync::atomic::AtomicBool,
}

impl SilentProgress {
    /// Create a new silent progress tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with a known length
    pub fn with_length(length: u64) -> Self {
        Self {
            length: AtomicU64::new(length),
            ..Default::default()
        }
    }
}

impl ProgressTracker for SilentProgress {
    fn set_message(&self, _message: &str) {}

    fn increment(&self, amount: u64) {
        self.position.fetch_add(amount, Ordering::Relaxed);
    }

    fn set_position(&self, position: u64) {
        self.position.store(position, Ordering::Relaxed);
    }

    fn set_length(&self, length: u64) {
        self.length.store(length, Ordering::Relaxed);
    }

    fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    fn length(&self) -> u64 {
        self.length.load(Ordering::Relaxed)
    }

    fn finish_with_message(&self, _message: &str) {
        self.finished.store(true, Ordering::Relaxed);
    }

    fn finish_with_error(&self, _message: &str) {
        self.finished.store(true, Ordering::Relaxed);
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    fn child(&self, _message: &str, length: u64, _style: ProgressStyle) -> Box<dyn ProgressTracker> {
        Box::new(SilentProgress::with_length(length))
    }
}

/// Logging progress tracker
///
/// Logs progress updates to tracing at info level.
/// Useful for non-interactive environments or when you want
/// progress in logs.
#[derive(Debug)]
pub struct LogProgress {
    name: String,
    position: AtomicU64,
    length: AtomicU64,
    finished: std::sync::atomic::AtomicBool,
    /// Log interval - only log every N increments to avoid spam
    log_interval: u64,
}

impl LogProgress {
    /// Create a new logging progress tracker
    pub fn new(name: impl Into<String>, length: u64) -> Self {
        Self {
            name: name.into(),
            position: AtomicU64::new(0),
            length: AtomicU64::new(length),
            finished: std::sync::atomic::AtomicBool::new(false),
            log_interval: std::cmp::max(1, length / 10), // Log ~10 times
        }
    }

    /// Set the logging interval
    pub fn with_log_interval(mut self, interval: u64) -> Self {
        self.log_interval = interval;
        self
    }
}

impl ProgressTracker for LogProgress {
    fn set_message(&self, message: &str) {
        info!("{}: {}", self.name, message);
    }

    fn increment(&self, amount: u64) {
        let old_pos = self.position.fetch_add(amount, Ordering::Relaxed);
        let new_pos = old_pos + amount;
        let length = self.length.load(Ordering::Relaxed);

        // Log at intervals
        if length > 0 && self.log_interval > 0 {
            let old_interval = old_pos / self.log_interval;
            let new_interval = new_pos / self.log_interval;
            if new_interval > old_interval {
                let percent = (new_pos * 100) / length;
                info!("{}: {}% ({}/{})", self.name, percent, new_pos, length);
            }
        }
    }

    fn set_position(&self, position: u64) {
        self.position.store(position, Ordering::Relaxed);
    }

    fn set_length(&self, length: u64) {
        self.length.store(length, Ordering::Relaxed);
    }

    fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    fn length(&self) -> u64 {
        self.length.load(Ordering::Relaxed)
    }

    fn finish_with_message(&self, message: &str) {
        self.finished.store(true, Ordering::Relaxed);
        info!("{}: {}", self.name, message);
    }

    fn finish_with_error(&self, message: &str) {
        self.finished.store(true, Ordering::Relaxed);
        info!("{}: ERROR - {}", self.name, message);
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    fn child(&self, message: &str, length: u64, _style: ProgressStyle) -> Box<dyn ProgressTracker> {
        Box::new(LogProgress::new(format!("{}:{}", self.name, message), length))
    }
}

/// Callback-based progress tracker
///
/// Calls a user-provided function on progress updates.
/// Useful for custom progress handling or GUI integration.
pub struct CallbackProgress<F>
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    callback: F,
    position: AtomicU64,
    length: AtomicU64,
    finished: std::sync::atomic::AtomicBool,
}

/// Events emitted by callback progress tracker
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Message updated
    Message(String),
    /// Position changed
    Position { current: u64, total: u64 },
    /// Progress finished successfully
    Finished(String),
    /// Progress finished with error
    Error(String),
}

impl<F> CallbackProgress<F>
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    /// Create a new callback progress tracker
    pub fn new(length: u64, callback: F) -> Self {
        Self {
            callback,
            position: AtomicU64::new(0),
            length: AtomicU64::new(length),
            finished: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl<F> ProgressTracker for CallbackProgress<F>
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    fn set_message(&self, message: &str) {
        (self.callback)(ProgressEvent::Message(message.to_string()));
    }

    fn increment(&self, amount: u64) {
        let new_pos = self.position.fetch_add(amount, Ordering::Relaxed) + amount;
        let length = self.length.load(Ordering::Relaxed);
        (self.callback)(ProgressEvent::Position {
            current: new_pos,
            total: length,
        });
    }

    fn set_position(&self, position: u64) {
        self.position.store(position, Ordering::Relaxed);
        let length = self.length.load(Ordering::Relaxed);
        (self.callback)(ProgressEvent::Position {
            current: position,
            total: length,
        });
    }

    fn set_length(&self, length: u64) {
        self.length.store(length, Ordering::Relaxed);
    }

    fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    fn length(&self) -> u64 {
        self.length.load(Ordering::Relaxed)
    }

    fn finish_with_message(&self, message: &str) {
        self.finished.store(true, Ordering::Relaxed);
        (self.callback)(ProgressEvent::Finished(message.to_string()));
    }

    fn finish_with_error(&self, message: &str) {
        self.finished.store(true, Ordering::Relaxed);
        (self.callback)(ProgressEvent::Error(message.to_string()));
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    fn child(&self, _message: &str, length: u64, _style: ProgressStyle) -> Box<dyn ProgressTracker> {
        // For callback progress, children are silent to avoid callback complexity
        Box::new(SilentProgress::with_length(length))
    }
}

/// Multi-operation progress tracker
///
/// Tracks progress across multiple sub-operations, each with their own
/// progress. Useful for batch operations like installing multiple packages.
pub struct MultiProgress {
    /// Name of the overall operation
    name: String,
    /// Total number of sub-operations
    total: AtomicU64,
    /// Completed sub-operations
    completed: AtomicU64,
    /// Current sub-operation message
    current_message: std::sync::RwLock<String>,
    /// Whether the operation is finished
    finished: std::sync::atomic::AtomicBool,
    /// Child trackers
    children: std::sync::RwLock<Vec<Arc<dyn ProgressTracker>>>,
}

impl MultiProgress {
    /// Create a new multi-progress tracker
    pub fn new(name: impl Into<String>, total: u64) -> Self {
        Self {
            name: name.into(),
            total: AtomicU64::new(total),
            completed: AtomicU64::new(0),
            current_message: std::sync::RwLock::new(String::new()),
            finished: std::sync::atomic::AtomicBool::new(false),
            children: std::sync::RwLock::new(Vec::new()),
        }
    }

    /// Get the number of completed sub-operations
    pub fn completed(&self) -> u64 {
        self.completed.load(Ordering::Relaxed)
    }

    /// Mark a sub-operation as complete
    pub fn complete_one(&self) {
        self.completed.fetch_add(1, Ordering::Relaxed);
    }

    /// Add a tracked child operation
    pub fn add_child(&self, child: Arc<dyn ProgressTracker>) {
        self.children.write().unwrap().push(child);
    }
}

impl ProgressTracker for MultiProgress {
    fn set_message(&self, message: &str) {
        *self.current_message.write().unwrap() = message.to_string();
    }

    fn increment(&self, amount: u64) {
        self.completed.fetch_add(amount, Ordering::Relaxed);
    }

    fn set_position(&self, position: u64) {
        self.completed.store(position, Ordering::Relaxed);
    }

    fn set_length(&self, length: u64) {
        self.total.store(length, Ordering::Relaxed);
    }

    fn position(&self) -> u64 {
        self.completed.load(Ordering::Relaxed)
    }

    fn length(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    fn finish_with_message(&self, message: &str) {
        self.finished.store(true, Ordering::Relaxed);
        *self.current_message.write().unwrap() = message.to_string();
    }

    fn finish_with_error(&self, message: &str) {
        self.finished.store(true, Ordering::Relaxed);
        *self.current_message.write().unwrap() = format!("ERROR: {}", message);
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    fn child(&self, message: &str, length: u64, _style: ProgressStyle) -> Box<dyn ProgressTracker> {
        Box::new(LogProgress::new(format!("{}:{}", self.name, message), length))
    }
}

/// Extension trait for progress tracking with phases
pub trait PhaseProgress: ProgressTracker {
    /// Phase type for this progress tracker
    type Phase: std::fmt::Display;

    /// Set the current phase
    fn set_phase(&self, phase: Self::Phase) {
        self.set_message(&phase.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silent_progress() {
        let progress = SilentProgress::with_length(100);

        progress.set_message("test");
        progress.increment(10);
        assert_eq!(progress.position(), 10);

        progress.set_position(50);
        assert_eq!(progress.position(), 50);

        assert!(!progress.is_finished());
        progress.finish_with_message("done");
        assert!(progress.is_finished());
    }

    #[test]
    fn test_log_progress() {
        let progress = LogProgress::new("test", 100);

        progress.increment(25);
        assert_eq!(progress.position(), 25);

        progress.increment(25);
        assert_eq!(progress.position(), 50);

        progress.finish_with_message("complete");
        assert!(progress.is_finished());
    }

    #[test]
    fn test_callback_progress() {
        use std::sync::Mutex;

        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let progress = CallbackProgress::new(100, move |event| {
            events_clone.lock().unwrap().push(event);
        });

        progress.set_message("starting");
        progress.increment(50);
        progress.finish_with_message("done");

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 3);

        assert!(matches!(&captured[0], ProgressEvent::Message(m) if m == "starting"));
        assert!(matches!(&captured[1], ProgressEvent::Position { current: 50, total: 100 }));
        assert!(matches!(&captured[2], ProgressEvent::Finished(m) if m == "done"));
    }

    #[test]
    fn test_multi_progress() {
        let multi = MultiProgress::new("install", 3);

        assert_eq!(multi.position(), 0);
        assert_eq!(multi.length(), 3);

        multi.complete_one();
        assert_eq!(multi.position(), 1);

        multi.complete_one();
        multi.complete_one();
        assert_eq!(multi.position(), 3);
    }

    #[test]
    fn test_child_progress() {
        let parent = SilentProgress::with_length(10);
        let child = parent.child("sub-task", 100, ProgressStyle::Bar);

        child.increment(50);
        assert_eq!(child.position(), 50);

        // Parent is unaffected
        assert_eq!(parent.position(), 0);
    }
}
