// src/delta/metrics.rs

//! Delta generation metrics
//!
//! Tracks bandwidth savings and delta effectiveness.

/// Maximum delta size as percentage of full file (fallback if delta too large)
pub const MAX_DELTA_RATIO: f64 = 0.9;

/// Delta generation metrics
#[derive(Debug, Clone)]
pub struct DeltaMetrics {
    pub old_size: u64,
    pub new_size: u64,
    pub delta_size: u64,
    pub compression_ratio: f64,
    pub bandwidth_saved: i64,
}

impl DeltaMetrics {
    /// Calculate metrics from sizes
    pub fn new(old_size: u64, new_size: u64, delta_size: u64) -> Self {
        let compression_ratio = if new_size > 0 {
            delta_size as f64 / new_size as f64
        } else {
            1.0
        };

        let bandwidth_saved = new_size as i64 - delta_size as i64;

        Self {
            old_size,
            new_size,
            delta_size,
            compression_ratio,
            bandwidth_saved,
        }
    }

    /// Check if delta is worthwhile (smaller than threshold)
    pub fn is_worthwhile(&self) -> bool {
        self.compression_ratio < MAX_DELTA_RATIO
    }

    /// Get percentage of bandwidth saved
    pub fn savings_percentage(&self) -> f64 {
        if self.new_size > 0 {
            (self.bandwidth_saved as f64 / self.new_size as f64) * 100.0
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_metrics_calculation() {
        let metrics = DeltaMetrics::new(1000, 1200, 300);

        assert_eq!(metrics.old_size, 1000);
        assert_eq!(metrics.new_size, 1200);
        assert_eq!(metrics.delta_size, 300);
        assert_eq!(metrics.compression_ratio, 0.25); // 300/1200
        assert_eq!(metrics.bandwidth_saved, 900); // 1200 - 300
        assert!((metrics.savings_percentage() - 75.0).abs() < 0.1);
        assert!(metrics.is_worthwhile());
    }

    #[test]
    fn test_delta_metrics_not_worthwhile() {
        // Delta is 95% of original size - not worthwhile
        let metrics = DeltaMetrics::new(1000, 1000, 950);
        assert!(!metrics.is_worthwhile());
    }
}
