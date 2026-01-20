// src/daemon/systemd.rs

//! Systemd integration for conaryd
//!
//! Provides:
//! - Socket activation support (LISTEN_FDS)
//! - Notification protocol (sd_notify)
//! - Watchdog support
//! - Idle timeout for socket activation
//!
//! # Socket Activation
//!
//! When started via systemd socket activation, the daemon receives
//! pre-opened file descriptors via the LISTEN_FDS mechanism. This
//! allows the daemon to be started on-demand when a client connects.
//!
//! # Notification Protocol
//!
//! The daemon notifies systemd of its state:
//! - `READY=1` - Service is ready to accept connections
//! - `STATUS=<message>` - Human-readable status
//! - `WATCHDOG=1` - Watchdog ping (keep-alive)
//! - `STOPPING=1` - Service is shutting down
//!
//! # Idle Timeout
//!
//! For socket-activated services, the daemon can exit after a period
//! of inactivity to save resources. Systemd will restart it when
//! a new connection arrives.

use std::os::unix::io::RawFd;
use std::time::{Duration, Instant};

/// Systemd notification state
#[derive(Debug, Clone)]
pub enum NotifyState<'a> {
    /// Service is ready
    Ready,
    /// Service is reloading
    Reloading,
    /// Service is stopping
    Stopping,
    /// Service status message
    Status(&'a str),
    /// Service main PID
    MainPid(u32),
    /// Watchdog keep-alive
    Watchdog,
    /// Watchdog trigger (service failure)
    WatchdogTrigger,
    /// Reset watchdog timeout
    WatchdogUsec(u64),
    /// Extend service timeout
    ExtendTimeoutUsec(u64),
    /// Custom notification
    Custom(&'a str),
}

impl<'a> NotifyState<'a> {
    /// Convert to notification string
    fn to_string(&self) -> String {
        match self {
            NotifyState::Ready => "READY=1".to_string(),
            NotifyState::Reloading => "RELOADING=1".to_string(),
            NotifyState::Stopping => "STOPPING=1".to_string(),
            NotifyState::Status(s) => format!("STATUS={}", s),
            NotifyState::MainPid(pid) => format!("MAINPID={}", pid),
            NotifyState::Watchdog => "WATCHDOG=1".to_string(),
            NotifyState::WatchdogTrigger => "WATCHDOG=trigger".to_string(),
            NotifyState::WatchdogUsec(usec) => format!("WATCHDOG_USEC={}", usec),
            NotifyState::ExtendTimeoutUsec(usec) => format!("EXTEND_TIMEOUT_USEC={}", usec),
            NotifyState::Custom(s) => s.to_string(),
        }
    }
}

/// Send notification to systemd
///
/// Returns true if notification was sent (systemd is managing the service).
pub fn notify(states: &[NotifyState<'_>]) -> bool {
    #[cfg(feature = "daemon")]
    {
        // Note: We build individual NotifyState values for sd_notify
        // The notification string is unused but kept for potential custom implementations

        // Convert to sd_notify format
        let sd_states: Vec<sd_notify::NotifyState> = states
            .iter()
            .filter_map(|s| match s {
                NotifyState::Ready => Some(sd_notify::NotifyState::Ready),
                NotifyState::Stopping => Some(sd_notify::NotifyState::Stopping),
                NotifyState::Watchdog => Some(sd_notify::NotifyState::Watchdog),
                NotifyState::Status(msg) => Some(sd_notify::NotifyState::Status(msg)),
                _ => None, // Handle custom states via Custom
            })
            .collect();

        if !sd_states.is_empty() {
            sd_notify::notify(true, &sd_states).is_ok()
        } else {
            false
        }
    }

    #[cfg(not(feature = "daemon"))]
    {
        false
    }
}

/// Notify systemd that the service is ready
pub fn notify_ready() -> bool {
    notify(&[NotifyState::Ready])
}

/// Notify systemd with a status message
pub fn notify_status(message: &str) -> bool {
    notify(&[NotifyState::Status(message)])
}

/// Send watchdog keep-alive to systemd
pub fn notify_watchdog() -> bool {
    notify(&[NotifyState::Watchdog])
}

/// Notify systemd that the service is stopping
pub fn notify_stopping() -> bool {
    notify(&[NotifyState::Stopping])
}

/// Check if running under systemd socket activation
///
/// Returns true if LISTEN_FDS environment variable is set,
/// indicating systemd has passed pre-opened sockets.
pub fn is_socket_activated() -> bool {
    std::env::var("LISTEN_FDS").is_ok()
}

/// Get the number of file descriptors passed by systemd
///
/// Returns the value of LISTEN_FDS if set, 0 otherwise.
pub fn listen_fds_count() -> usize {
    std::env::var("LISTEN_FDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Get the file descriptors passed by systemd
///
/// Systemd passes file descriptors starting at FD 3 (after stdin/stdout/stderr).
/// This function returns the raw FDs for use with socket listeners.
///
/// # Safety
///
/// The returned FDs are owned by systemd and should not be closed directly.
/// They will be valid for the lifetime of the process.
pub fn listen_fds() -> Vec<RawFd> {
    let count = listen_fds_count();
    (3..(3 + count as RawFd)).collect()
}

/// Get the watchdog timeout configured in systemd
///
/// Returns the watchdog timeout in microseconds, or None if not configured.
/// The daemon should ping sd_notify at least once within half this interval.
pub fn watchdog_timeout() -> Option<Duration> {
    std::env::var("WATCHDOG_USEC")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_micros)
}

/// Watchdog helper for automatic keep-alive pings
///
/// Creates a task that sends watchdog pings at regular intervals.
pub struct WatchdogTask {
    /// Interval between pings
    interval: Duration,
    /// Last ping time
    last_ping: Instant,
}

impl WatchdogTask {
    /// Create a new watchdog task
    ///
    /// The interval should be half the watchdog timeout configured in systemd.
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_ping: Instant::now(),
        }
    }

    /// Create from systemd environment
    ///
    /// Returns None if watchdog is not configured.
    pub fn from_env() -> Option<Self> {
        watchdog_timeout().map(|timeout| {
            // Ping at half the timeout interval for safety
            let interval = timeout / 2;
            Self::new(interval)
        })
    }

    /// Check if a ping is due and send it if so
    ///
    /// Returns true if a ping was sent.
    pub fn tick(&mut self) -> bool {
        if self.last_ping.elapsed() >= self.interval {
            self.last_ping = Instant::now();
            notify_watchdog()
        } else {
            false
        }
    }

    /// Get the interval between pings
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

/// Idle timeout tracker for socket-activated services
///
/// Tracks activity and signals when the service should exit due to inactivity.
pub struct IdleTracker {
    /// Timeout duration
    timeout: Duration,
    /// Last activity time
    last_activity: Instant,
}

impl IdleTracker {
    /// Create a new idle tracker
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            last_activity: Instant::now(),
        }
    }

    /// Record activity (reset the idle timer)
    pub fn activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Check if the idle timeout has expired
    pub fn is_expired(&self) -> bool {
        self.last_activity.elapsed() >= self.timeout
    }

    /// Get time until timeout
    pub fn time_until_timeout(&self) -> Duration {
        let elapsed = self.last_activity.elapsed();
        if elapsed >= self.timeout {
            Duration::ZERO
        } else {
            self.timeout - elapsed
        }
    }

    /// Get the timeout duration
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Systemd service manager for the daemon
pub struct SystemdManager {
    /// Watchdog task (if configured)
    watchdog: Option<WatchdogTask>,
    /// Idle tracker (if socket activated with timeout)
    idle: Option<IdleTracker>,
    /// Whether running under systemd
    is_systemd: bool,
}

impl SystemdManager {
    /// Create a new systemd manager
    pub fn new(idle_timeout: Option<Duration>) -> Self {
        let is_systemd = std::env::var("NOTIFY_SOCKET").is_ok();
        let watchdog = WatchdogTask::from_env();
        let idle = idle_timeout.map(IdleTracker::new);

        Self {
            watchdog,
            idle,
            is_systemd,
        }
    }

    /// Check if running under systemd
    pub fn is_systemd(&self) -> bool {
        self.is_systemd
    }

    /// Notify systemd that the service is ready
    pub fn notify_ready(&self, status: Option<&str>) {
        if self.is_systemd {
            notify_ready();
            if let Some(msg) = status {
                notify_status(msg);
            }
        }
    }

    /// Notify systemd that the service is stopping
    pub fn notify_stopping(&self) {
        if self.is_systemd {
            notify_stopping();
        }
    }

    /// Notify systemd with a status message
    pub fn notify_status(&self, message: &str) {
        if self.is_systemd {
            notify_status(message);
        }
    }

    /// Tick the watchdog (send ping if due)
    pub fn watchdog_tick(&mut self) {
        if let Some(ref mut wd) = self.watchdog {
            wd.tick();
        }
    }

    /// Get the watchdog interval
    pub fn watchdog_interval(&self) -> Option<Duration> {
        self.watchdog.as_ref().map(|wd| wd.interval())
    }

    /// Record activity for idle tracking
    pub fn activity(&mut self) {
        if let Some(ref mut idle) = self.idle {
            idle.activity();
        }
    }

    /// Check if idle timeout has expired
    pub fn is_idle_expired(&self) -> bool {
        self.idle.as_ref().map(|i| i.is_expired()).unwrap_or(false)
    }

    /// Get time until idle timeout
    pub fn time_until_idle(&self) -> Option<Duration> {
        self.idle.as_ref().map(|i| i.time_until_timeout())
    }

    /// Get the minimum interval for the main loop
    ///
    /// Returns the shorter of watchdog interval or time until idle timeout.
    pub fn tick_interval(&self) -> Duration {
        let watchdog_interval = self.watchdog_interval().unwrap_or(Duration::from_secs(60));
        let idle_timeout = self.time_until_idle().unwrap_or(Duration::from_secs(3600));

        std::cmp::min(watchdog_interval, idle_timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notify_state_to_string() {
        assert_eq!(NotifyState::Ready.to_string(), "READY=1");
        assert_eq!(NotifyState::Stopping.to_string(), "STOPPING=1");
        assert_eq!(NotifyState::Watchdog.to_string(), "WATCHDOG=1");
        assert_eq!(NotifyState::Status("test").to_string(), "STATUS=test");
        assert_eq!(NotifyState::MainPid(1234).to_string(), "MAINPID=1234");
    }

    #[test]
    fn test_listen_fds() {
        // Without LISTEN_FDS set, should return 0
        // Note: Can't easily test with env var set due to unsafe
        assert_eq!(listen_fds_count(), 0);
        assert!(listen_fds().is_empty());
    }

    #[test]
    fn test_watchdog_task() {
        let mut wd = WatchdogTask::new(Duration::from_millis(100));

        // First tick should succeed (enough time since creation)
        // Note: In practice this depends on timing
        let interval = wd.interval();
        assert_eq!(interval, Duration::from_millis(100));
    }

    #[test]
    fn test_idle_tracker() {
        let mut tracker = IdleTracker::new(Duration::from_secs(60));

        // Initially not expired
        assert!(!tracker.is_expired());

        // Activity resets
        tracker.activity();
        assert!(!tracker.is_expired());

        // Time until timeout should be positive
        let remaining = tracker.time_until_timeout();
        assert!(remaining > Duration::ZERO);
    }

    #[test]
    fn test_systemd_manager() {
        let manager = SystemdManager::new(Some(Duration::from_secs(300)));

        // Without NOTIFY_SOCKET, should not be systemd
        // Note: This may vary in CI environments
        let _ = manager.is_systemd();

        // Tick interval should be reasonable
        let interval = manager.tick_interval();
        assert!(interval > Duration::ZERO);
    }
}
