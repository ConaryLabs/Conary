// src/capability/inference/confidence.rs
//! Confidence scoring for capability inference
//!
//! This module provides types for expressing how confident we are in
//! inferred capabilities. Confidence matters because:
//! - High confidence results can be used for enforcement
//! - Low confidence results should be reviewed by humans
//! - Confidence helps prioritize where to focus analysis effort

use std::fmt;

/// Confidence level in an inference result
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum Confidence {
    /// Very uncertain - needs human review
    #[default]
    Low,
    /// Reasonably confident - likely correct
    Medium,
    /// Highly confident - can be used for enforcement
    High,
    /// Definitive - from authoritative source (e.g., package metadata)
    Definitive,
}

impl Confidence {
    /// Convert to a numeric score (0.0 - 1.0)
    pub fn as_score(&self) -> f64 {
        match self {
            Self::Low => 0.25,
            Self::Medium => 0.50,
            Self::High => 0.75,
            Self::Definitive => 1.0,
        }
    }

    /// Create from a numeric score
    pub fn from_score(score: f64) -> Self {
        if score >= 0.9 {
            Self::Definitive
        } else if score >= 0.7 {
            Self::High
        } else if score >= 0.4 {
            Self::Medium
        } else {
            Self::Low
        }
    }

    /// Combine two confidence levels (takes minimum)
    pub fn combine(self, other: Self) -> Self {
        if self < other {
            self
        } else {
            other
        }
    }

    /// Check if confidence is sufficient for enforcement
    pub fn is_enforceable(&self) -> bool {
        matches!(self, Self::High | Self::Definitive)
    }
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Definitive => write!(f, "definitive"),
        }
    }
}

/// Detailed confidence scoring with breakdown by category
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfidenceScore {
    /// Overall/primary confidence level
    pub primary: Confidence,

    /// Confidence in network capability inference
    pub network: Confidence,

    /// Confidence in filesystem capability inference
    pub filesystem: Confidence,

    /// Confidence in syscall profile inference
    pub syscalls: Confidence,

    /// Number of evidence points supporting this inference
    pub evidence_count: u32,

    /// Brief explanation of confidence factors
    pub factors: Vec<String>,
}

impl Default for ConfidenceScore {
    fn default() -> Self {
        Self::new(Confidence::Low)
    }
}

impl ConfidenceScore {
    /// Create a new confidence score with uniform confidence
    pub fn new(confidence: Confidence) -> Self {
        Self {
            primary: confidence,
            network: confidence,
            filesystem: confidence,
            syscalls: confidence,
            evidence_count: 0,
            factors: Vec::new(),
        }
    }

    /// Create with different confidence per category
    pub fn detailed(network: Confidence, filesystem: Confidence, syscalls: Confidence) -> Self {
        // Primary confidence is the minimum of the three
        let primary = network.combine(filesystem).combine(syscalls);
        Self {
            primary,
            network,
            filesystem,
            syscalls,
            evidence_count: 0,
            factors: Vec::new(),
        }
    }

    /// Add an evidence factor
    pub fn add_factor(&mut self, factor: impl Into<String>) {
        self.factors.push(factor.into());
        self.evidence_count += 1;
    }

    /// Increase confidence based on additional evidence
    pub fn boost_with_evidence(&mut self, factor: impl Into<String>) {
        self.add_factor(factor);

        // Boost confidence based on evidence count
        if self.evidence_count >= 5 && self.primary < Confidence::High {
            self.primary = Confidence::High;
        } else if self.evidence_count >= 2 && self.primary < Confidence::Medium {
            self.primary = Confidence::Medium;
        }
    }

    /// Check if the inference is reliable enough to use
    pub fn is_reliable(&self) -> bool {
        self.primary >= Confidence::Medium
    }

    /// Get a summary of confidence factors
    pub fn summary(&self) -> String {
        if self.factors.is_empty() {
            format!("Confidence: {} (no specific factors)", self.primary)
        } else {
            format!(
                "Confidence: {} based on: {}",
                self.primary,
                self.factors.join(", ")
            )
        }
    }
}

/// Builder for incrementally constructing confidence scores
#[derive(Debug, Default)]
pub struct ConfidenceBuilder {
    network_evidence: Vec<(String, Confidence)>,
    filesystem_evidence: Vec<(String, Confidence)>,
    syscall_evidence: Vec<(String, Confidence)>,
}

impl ConfidenceBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add network-related evidence
    pub fn add_network_evidence(&mut self, reason: impl Into<String>, confidence: Confidence) {
        self.network_evidence.push((reason.into(), confidence));
    }

    /// Add filesystem-related evidence
    pub fn add_filesystem_evidence(&mut self, reason: impl Into<String>, confidence: Confidence) {
        self.filesystem_evidence.push((reason.into(), confidence));
    }

    /// Add syscall-related evidence
    pub fn add_syscall_evidence(&mut self, reason: impl Into<String>, confidence: Confidence) {
        self.syscall_evidence.push((reason.into(), confidence));
    }

    /// Build the final confidence score
    pub fn build(self) -> ConfidenceScore {
        let network = aggregate_confidence(&self.network_evidence);
        let filesystem = aggregate_confidence(&self.filesystem_evidence);
        let syscalls = aggregate_confidence(&self.syscall_evidence);

        let mut score = ConfidenceScore::detailed(network, filesystem, syscalls);

        // Collect all factors
        for (factor, _) in self.network_evidence {
            score.factors.push(format!("[network] {}", factor));
        }
        for (factor, _) in self.filesystem_evidence {
            score.factors.push(format!("[filesystem] {}", factor));
        }
        for (factor, _) in self.syscall_evidence {
            score.factors.push(format!("[syscall] {}", factor));
        }

        score.evidence_count = score.factors.len() as u32;
        score
    }
}

/// Aggregate multiple evidence points into a single confidence level
fn aggregate_confidence(evidence: &[(String, Confidence)]) -> Confidence {
    if evidence.is_empty() {
        return Confidence::Low;
    }

    // If any evidence is definitive, result is high (not definitive, since we're combining)
    if evidence.iter().any(|(_, c)| *c == Confidence::Definitive) {
        return Confidence::High;
    }

    // If we have 3+ high confidence items, overall is high
    let high_count = evidence.iter().filter(|(_, c)| *c >= Confidence::High).count();
    if high_count >= 3 {
        return Confidence::High;
    }

    // If we have any high confidence, overall is at least medium
    if high_count >= 1 {
        return Confidence::Medium;
    }

    // If we have 2+ medium confidence items, overall is medium
    let medium_count = evidence.iter().filter(|(_, c)| *c >= Confidence::Medium).count();
    if medium_count >= 2 {
        return Confidence::Medium;
    }

    Confidence::Low
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confidence_ordering() {
        assert!(Confidence::Low < Confidence::Medium);
        assert!(Confidence::Medium < Confidence::High);
        assert!(Confidence::High < Confidence::Definitive);
    }

    #[test]
    fn test_confidence_combine() {
        assert_eq!(Confidence::High.combine(Confidence::Low), Confidence::Low);
        assert_eq!(
            Confidence::Medium.combine(Confidence::Medium),
            Confidence::Medium
        );
    }

    #[test]
    fn test_confidence_score() {
        let mut score = ConfidenceScore::new(Confidence::Low);
        assert!(!score.is_reliable());

        score.boost_with_evidence("Found systemd service file");
        score.boost_with_evidence("Package name matches known pattern");
        assert!(score.is_reliable());
        assert_eq!(score.evidence_count, 2);
    }

    #[test]
    fn test_confidence_builder() {
        let mut builder = ConfidenceBuilder::new();
        builder.add_network_evidence("Listens on port 80", Confidence::High);
        builder.add_network_evidence("Has nginx in name", Confidence::Medium);
        builder.add_filesystem_evidence("Writes to /var/log", Confidence::Medium);

        let score = builder.build();
        assert!(score.network >= Confidence::Medium);
        assert_eq!(score.evidence_count, 3);
    }

    #[test]
    fn test_confidence_from_score() {
        assert_eq!(Confidence::from_score(0.1), Confidence::Low);
        assert_eq!(Confidence::from_score(0.5), Confidence::Medium);
        assert_eq!(Confidence::from_score(0.8), Confidence::High);
        assert_eq!(Confidence::from_score(1.0), Confidence::Definitive);
    }
}
