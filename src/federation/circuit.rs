// src/federation/circuit.rs
//! Circuit breaker implementation for federation peers
//!
//! Implements the circuit breaker pattern with:
//! - Configurable failure threshold before opening
//! - Jitter-based cooldown to prevent synchronized retry storms
//! - Half-open state for gradual recovery
//!
//! Based on recommendations from GPT 5.2 and Gemini 3 Pro experts.

use super::peer::PeerId;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tracing::debug;

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - requests allowed
    Closed,
    /// Circuit tripped - requests blocked
    Open,
    /// Testing recovery - limited requests allowed
    HalfOpen,
}

/// Per-peer circuit breaker
#[derive(Debug)]
pub struct CircuitBreaker {
    /// Current state
    state: CircuitState,
    /// Number of consecutive failures
    failure_count: u32,
    /// Threshold for opening circuit
    failure_threshold: u32,
    /// Base cooldown duration before half-open
    base_cooldown: Duration,
    /// Jitter factor (0.0 - 1.0)
    jitter_factor: f32,
    /// When the circuit was opened
    opened_at: Option<Instant>,
    /// Computed cooldown with jitter
    computed_cooldown: Duration,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    pub fn new(failure_threshold: u32, base_cooldown: Duration, jitter_factor: f32) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            failure_threshold,
            base_cooldown,
            jitter_factor: jitter_factor.clamp(0.0, 1.0),
            opened_at: None,
            computed_cooldown: base_cooldown,
        }
    }

    /// Check if the circuit is open (blocking requests)
    pub fn is_open(&self) -> bool {
        match self.state {
            CircuitState::Closed => false,
            CircuitState::Open => {
                // Check if cooldown has elapsed
                if let Some(opened_at) = self.opened_at
                    && opened_at.elapsed() >= self.computed_cooldown
                {
                    // Would transition to HalfOpen, but we need mut for that
                    return false;
                }
                true
            }
            CircuitState::HalfOpen => false, // Allow test requests
        }
    }

    /// Get current state, potentially transitioning Open -> HalfOpen
    pub fn get_state(&mut self) -> CircuitState {
        if self.state == CircuitState::Open
            && let Some(opened_at) = self.opened_at
            && opened_at.elapsed() >= self.computed_cooldown
        {
            debug!("Circuit breaker transitioning to half-open");
            self.state = CircuitState::HalfOpen;
        }
        self.state
    }

    /// Record a successful request
    pub fn record_success(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            CircuitState::HalfOpen => {
                // Recovery successful - close circuit
                debug!("Circuit breaker closing (recovery successful)");
                self.state = CircuitState::Closed;
                self.failure_count = 0;
                self.opened_at = None;
            }
            CircuitState::Open => {
                // Shouldn't happen (requests blocked), but handle gracefully
            }
        }
    }

    /// Record a failed request
    pub fn record_failure(&mut self) {
        self.failure_count += 1;

        match self.state {
            CircuitState::Closed => {
                if self.failure_count >= self.failure_threshold {
                    self.open();
                }
            }
            CircuitState::HalfOpen => {
                // Failed during recovery - re-open
                debug!("Circuit breaker re-opening (recovery failed)");
                self.open();
            }
            CircuitState::Open => {
                // Already open, just track failures
            }
        }
    }

    /// Open the circuit with jitter-based cooldown
    fn open(&mut self) {
        debug!(
            "Circuit breaker opening (failures: {})",
            self.failure_count
        );
        self.state = CircuitState::Open;
        self.opened_at = Some(Instant::now());

        // Compute cooldown with random jitter
        let jitter = rand::random::<f32>() * self.jitter_factor;
        self.computed_cooldown = self.base_cooldown.mul_f32(1.0 + jitter);

        debug!("Cooldown: {:?}", self.computed_cooldown);
    }

    /// Get failure count
    pub fn failure_count(&self) -> u32 {
        self.failure_count
    }
}

/// Registry of circuit breakers for all peers
pub struct CircuitBreakerRegistry {
    /// Per-peer circuit breakers
    breakers: DashMap<PeerId, CircuitBreaker>,
    /// Default failure threshold
    failure_threshold: u32,
    /// Default cooldown duration
    base_cooldown: Duration,
    /// Default jitter factor
    jitter_factor: f32,
    /// Count of successful requests (for stats)
    success_count: AtomicU32,
    /// Count of failed requests (for stats)
    failure_count: AtomicU32,
}

impl CircuitBreakerRegistry {
    /// Create a new registry with default parameters
    pub fn new(failure_threshold: u32, base_cooldown: Duration, jitter_factor: f32) -> Self {
        Self {
            breakers: DashMap::new(),
            failure_threshold,
            base_cooldown,
            jitter_factor,
            success_count: AtomicU32::new(0),
            failure_count: AtomicU32::new(0),
        }
    }

    /// Check if a peer's circuit is open
    pub fn is_open(&self, peer_id: &PeerId) -> bool {
        if let Some(breaker) = self.breakers.get(peer_id) {
            breaker.is_open()
        } else {
            false // No breaker = closed
        }
    }

    /// Get or create a circuit breaker for a peer
    fn get_or_create(&self, peer_id: &PeerId) -> dashmap::mapref::one::RefMut<'_, PeerId, CircuitBreaker> {
        self.breakers.entry(peer_id.clone()).or_insert_with(|| {
            CircuitBreaker::new(self.failure_threshold, self.base_cooldown, self.jitter_factor)
        })
    }

    /// Record a successful request for a peer
    pub fn record_success(&self, peer_id: &PeerId) {
        let mut breaker = self.get_or_create(peer_id);
        breaker.record_success();
        self.success_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failed request for a peer
    pub fn record_failure(&self, peer_id: &PeerId) {
        let mut breaker = self.get_or_create(peer_id);
        breaker.record_failure();
        self.failure_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the state of a peer's circuit breaker
    pub fn get_state(&self, peer_id: &PeerId) -> CircuitState {
        if let Some(mut breaker) = self.breakers.get_mut(peer_id) {
            breaker.get_state()
        } else {
            CircuitState::Closed
        }
    }

    /// Get the number of open circuits
    pub fn open_count(&self) -> usize {
        self.breakers.iter().filter(|e| e.value().is_open()).count()
    }

    /// Get total success count
    pub fn total_successes(&self) -> u32 {
        self.success_count.load(Ordering::Relaxed)
    }

    /// Get total failure count
    pub fn total_failures(&self) -> u32 {
        self.failure_count.load(Ordering::Relaxed)
    }

    /// Reset all circuit breakers
    pub fn reset_all(&self) {
        self.breakers.clear();
        self.success_count.store(0, Ordering::Relaxed);
        self.failure_count.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::new(5, Duration::from_secs(30), 0.0);
        assert!(!cb.is_open());
    }

    #[test]
    fn test_circuit_opens_after_threshold() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(30), 0.0);

        cb.record_failure();
        assert!(!cb.is_open());
        cb.record_failure();
        assert!(!cb.is_open());
        cb.record_failure();
        assert!(cb.is_open());
    }

    #[test]
    fn test_success_resets_failure_count() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(30), 0.0);

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.failure_count(), 2);

        cb.record_success();
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn test_circuit_cooldown() {
        let mut cb = CircuitBreaker::new(1, Duration::from_millis(100), 0.0);

        cb.record_failure();
        assert!(cb.is_open());

        // Wait for cooldown
        std::thread::sleep(Duration::from_millis(150));

        // Should transition to half-open
        assert_eq!(cb.get_state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_half_open_success_closes() {
        let mut cb = CircuitBreaker::new(1, Duration::from_millis(50), 0.0);

        cb.record_failure();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(cb.get_state(), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.get_state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_failure_reopens() {
        let mut cb = CircuitBreaker::new(1, Duration::from_millis(50), 0.0);

        cb.record_failure();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(cb.get_state(), CircuitState::HalfOpen);

        cb.record_failure();
        assert!(cb.is_open());
    }

    #[test]
    fn test_registry() {
        let registry = CircuitBreakerRegistry::new(2, Duration::from_secs(30), 0.0);
        let peer_id = "test_peer".to_string();

        assert!(!registry.is_open(&peer_id));

        registry.record_failure(&peer_id);
        assert!(!registry.is_open(&peer_id));

        registry.record_failure(&peer_id);
        assert!(registry.is_open(&peer_id));

        assert_eq!(registry.open_count(), 1);
    }

    #[test]
    fn test_jitter_varies_cooldown() {
        // With 50% jitter, cooldowns should vary
        let mut cooldowns = Vec::new();

        for _ in 0..10 {
            let mut cb = CircuitBreaker::new(1, Duration::from_secs(100), 0.5);
            cb.record_failure();
            cooldowns.push(cb.computed_cooldown);
        }

        // At least some should differ
        let first = cooldowns[0];
        let any_different = cooldowns.iter().any(|c| *c != first);
        assert!(any_different, "Jitter should produce varying cooldowns");

        // All should be between 100s and 150s
        for c in cooldowns {
            assert!(c >= Duration::from_secs(100));
            assert!(c <= Duration::from_secs(150));
        }
    }
}
