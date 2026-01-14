// src/flavor/mod.rs
//! Flavor specification parsing and matching
//!
//! Flavors represent build-time variations like architecture, features, and toolchain.
//! Syntax follows original Conary: `[ssl, !debug, ~vmware, is: x86_64]`

use crate::error::{Error, Result};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

/// Flavor operators from original Conary syntax
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlavorOp {
    /// Required: package built for systems with this feature (no prefix)
    Required,
    /// Not: package built for systems WITHOUT this feature (! prefix)
    Not,
    /// Prefers: soft preference, use if no !X exists (~ prefix)
    Prefers,
    /// PrefersNot: soft preference, use if no X exists (~! prefix)
    PrefersNot,
}

impl FlavorOp {
    /// Get the string prefix for this operator
    pub fn as_prefix(&self) -> &'static str {
        match self {
            Self::Required => "",
            Self::Not => "!",
            Self::Prefers => "~",
            Self::PrefersNot => "~!",
        }
    }

    /// Parse an operator and name from a string
    /// Returns (operator, remaining name)
    pub fn parse_with_name(s: &str) -> Result<(Self, &str)> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::ParseError("Empty flavor item".to_string()));
        }

        // Check longer operators first
        if let Some(rest) = s.strip_prefix("~!") {
            let name = rest.trim();
            if name.is_empty() {
                return Err(Error::ParseError(
                    "Missing name after ~! operator".to_string(),
                ));
            }
            Ok((Self::PrefersNot, name))
        } else if let Some(rest) = s.strip_prefix('~') {
            let name = rest.trim();
            if name.is_empty() {
                return Err(Error::ParseError(
                    "Missing name after ~ operator".to_string(),
                ));
            }
            Ok((Self::Prefers, name))
        } else if let Some(rest) = s.strip_prefix('!') {
            let name = rest.trim();
            if name.is_empty() {
                return Err(Error::ParseError(
                    "Missing name after ! operator".to_string(),
                ));
            }
            Ok((Self::Not, name))
        } else {
            Ok((Self::Required, s))
        }
    }
}

/// A single flavor item with operator
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlavorItem {
    pub op: FlavorOp,
    pub name: String,
}

impl FlavorItem {
    /// Create a new flavor item
    pub fn new(op: FlavorOp, name: impl Into<String>) -> Self {
        Self {
            op,
            name: name.into(),
        }
    }

    /// Parse a flavor item from a string like "ssl", "!debug", "~vmware", "~!xen"
    pub fn parse(s: &str) -> Result<Self> {
        let (op, name) = FlavorOp::parse_with_name(s)?;
        Ok(Self {
            op,
            name: name.to_string(),
        })
    }
}

impl fmt::Display for FlavorItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.op.as_prefix(), self.name)
    }
}

/// Architecture specification (is: x86 x86_64)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArchSpec {
    pub architectures: Vec<String>,
}

impl ArchSpec {
    /// Create a new architecture spec
    pub fn new(architectures: Vec<String>) -> Self {
        Self { architectures }
    }

    /// Check if this spec includes the given architecture
    pub fn contains(&self, arch: &str) -> bool {
        self.architectures.iter().any(|a| a == arch)
    }
}

impl fmt::Display for ArchSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "is: {}", self.architectures.join(" "))
    }
}

/// Complete flavor specification like [ssl, !debug, is: x86_64]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlavorSpec {
    pub items: Vec<FlavorItem>,
    pub arch: Option<ArchSpec>,
}

impl FlavorSpec {
    /// Create an empty flavor spec
    pub fn empty() -> Self {
        Self::default()
    }

    /// Create a new flavor spec
    pub fn new(items: Vec<FlavorItem>, arch: Option<ArchSpec>) -> Self {
        let mut spec = Self { items, arch };
        spec.canonicalize();
        spec
    }

    /// Check if this flavor spec is empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.arch.is_none()
    }

    /// Canonicalize for consistent storage and comparison
    /// CRITICAL: Must be called before storing in database
    pub fn canonicalize(&mut self) {
        // Sort items alphabetically by name
        self.items.sort_by(|a, b| a.name.cmp(&b.name));

        // Sort and dedupe architectures
        if let Some(arch) = &mut self.arch {
            arch.architectures.sort();
            arch.architectures.dedup();
        }
    }

    /// Parse a flavor specification string
    ///
    /// Examples:
    /// - `[ssl, !debug, is: x86_64]`
    /// - `ssl, !debug` (without brackets)
    /// - `[]` (empty)
    /// - `[is: x86 x86_64]` (arch only)
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();

        if s.is_empty() {
            return Ok(Self::empty());
        }

        // Handle bracketed form
        let inner = if s.starts_with('[') && s.ends_with(']') {
            &s[1..s.len() - 1]
        } else {
            s
        };

        if inner.trim().is_empty() {
            return Ok(Self::empty());
        }

        let mut items = Vec::new();
        let mut arch = None;

        // Split on comma, but handle "is:" specially
        let mut remaining = inner;
        while !remaining.is_empty() {
            remaining = remaining.trim();

            // Check for architecture spec
            if remaining.starts_with("is:") {
                // Consume everything until the next comma or end
                let arch_end = remaining.find(',').unwrap_or(remaining.len());
                let arch_str = &remaining[3..arch_end].trim();

                let architectures: Vec<String> = arch_str
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();

                if architectures.is_empty() {
                    return Err(Error::ParseError(
                        "Empty architecture specification after 'is:'".to_string(),
                    ));
                }

                arch = Some(ArchSpec { architectures });

                // Move past this part
                if arch_end < remaining.len() {
                    remaining = &remaining[arch_end + 1..];
                } else {
                    break;
                }
            } else {
                // Regular flavor item
                let item_end = remaining.find(',').unwrap_or(remaining.len());
                let item_str = remaining[..item_end].trim();

                if !item_str.is_empty() {
                    items.push(FlavorItem::parse(item_str)?);
                }

                // Move past this part
                if item_end < remaining.len() {
                    remaining = &remaining[item_end + 1..];
                } else {
                    break;
                }
            }
        }

        let mut spec = Self { items, arch };
        spec.canonicalize();
        Ok(spec)
    }

    /// Check if a package with this flavor matches the given system flavor
    ///
    /// Returns (matches: bool, score: i32) where score is used for
    /// preference ranking among valid candidates.
    pub fn matches(&self, system: &SystemFlavor) -> (bool, i32) {
        let mut score = 0;

        // Check architecture first (hard requirement)
        if let Some(ref arch) = self.arch {
            if !arch.contains(&system.architecture) {
                return (false, 0);
            }
            score += 10; // Bonus for matching architecture
        }

        for item in &self.items {
            let system_has = system.features.contains(&item.name);

            match item.op {
                FlavorOp::Required => {
                    // Package requires this feature; system must have it
                    if !system_has {
                        return (false, 0);
                    }
                    score += 10; // Strong positive match
                }
                FlavorOp::Not => {
                    // Package requires system NOT have this feature
                    if system_has {
                        return (false, 0);
                    }
                    score += 10; // Strong positive match for exclusion
                }
                FlavorOp::Prefers => {
                    // Soft preference - adds to score if matched
                    if system_has {
                        score += 5;
                    }
                }
                FlavorOp::PrefersNot => {
                    // Soft preference - adds to score if NOT matched
                    if !system_has {
                        score += 5;
                    }
                }
            }
        }

        (true, score)
    }

    /// Select the best matching flavor spec from candidates
    pub fn select_best<'a, T>(
        candidates: &'a [(FlavorSpec, T)],
        system: &SystemFlavor,
    ) -> Option<&'a T> {
        candidates
            .iter()
            .filter_map(|(spec, item)| {
                let (matches, score) = spec.matches(system);
                if matches {
                    Some((score, item))
                } else {
                    None
                }
            })
            .max_by_key(|(score, _)| *score)
            .map(|(_, item)| item)
    }
}

impl fmt::Display for FlavorSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return Ok(());
        }

        let mut parts: Vec<String> = self.items.iter().map(|item| item.to_string()).collect();

        // Architecture always goes last
        if let Some(ref arch) = self.arch {
            parts.push(arch.to_string());
        }

        write!(f, "[{}]", parts.join(", "))
    }
}

impl FromStr for FlavorSpec {
    type Err = Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        FlavorSpec::parse(s)
    }
}

/// System flavor represents the current system's capabilities
#[derive(Debug, Clone, Default)]
pub struct SystemFlavor {
    /// Features present on the system
    pub features: HashSet<String>,
    /// Current architecture
    pub architecture: String,
}

impl SystemFlavor {
    /// Create a new system flavor
    pub fn new(architecture: impl Into<String>) -> Self {
        Self {
            features: HashSet::new(),
            architecture: architecture.into(),
        }
    }

    /// Add a feature to the system
    pub fn with_feature(mut self, feature: impl Into<String>) -> Self {
        self.features.insert(feature.into());
        self
    }

    /// Add multiple features
    pub fn with_features(mut self, features: impl IntoIterator<Item = impl Into<String>>) -> Self {
        for f in features {
            self.features.insert(f.into());
        }
        self
    }

    /// Detect system flavor from the current environment
    pub fn detect() -> Self {
        let architecture = std::env::consts::ARCH.to_string();
        Self::new(architecture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === FlavorOp tests ===

    #[test]
    fn test_flavor_op_parse_required() {
        let (op, name) = FlavorOp::parse_with_name("ssl").unwrap();
        assert_eq!(op, FlavorOp::Required);
        assert_eq!(name, "ssl");
    }

    #[test]
    fn test_flavor_op_parse_not() {
        let (op, name) = FlavorOp::parse_with_name("!debug").unwrap();
        assert_eq!(op, FlavorOp::Not);
        assert_eq!(name, "debug");
    }

    #[test]
    fn test_flavor_op_parse_prefers() {
        let (op, name) = FlavorOp::parse_with_name("~vmware").unwrap();
        assert_eq!(op, FlavorOp::Prefers);
        assert_eq!(name, "vmware");
    }

    #[test]
    fn test_flavor_op_parse_prefers_not() {
        let (op, name) = FlavorOp::parse_with_name("~!xen").unwrap();
        assert_eq!(op, FlavorOp::PrefersNot);
        assert_eq!(name, "xen");
    }

    #[test]
    fn test_flavor_op_parse_with_spaces() {
        let (op, name) = FlavorOp::parse_with_name("  ~! xen  ").unwrap();
        assert_eq!(op, FlavorOp::PrefersNot);
        assert_eq!(name, "xen");
    }

    #[test]
    fn test_flavor_op_parse_empty_error() {
        assert!(FlavorOp::parse_with_name("").is_err());
        assert!(FlavorOp::parse_with_name("   ").is_err());
    }

    #[test]
    fn test_flavor_op_parse_missing_name_error() {
        assert!(FlavorOp::parse_with_name("!").is_err());
        assert!(FlavorOp::parse_with_name("~").is_err());
        assert!(FlavorOp::parse_with_name("~!").is_err());
    }

    // === FlavorItem tests ===

    #[test]
    fn test_flavor_item_display() {
        assert_eq!(
            FlavorItem::new(FlavorOp::Required, "ssl").to_string(),
            "ssl"
        );
        assert_eq!(
            FlavorItem::new(FlavorOp::Not, "debug").to_string(),
            "!debug"
        );
        assert_eq!(
            FlavorItem::new(FlavorOp::Prefers, "vmware").to_string(),
            "~vmware"
        );
        assert_eq!(
            FlavorItem::new(FlavorOp::PrefersNot, "xen").to_string(),
            "~!xen"
        );
    }

    // === FlavorSpec parsing tests ===

    #[test]
    fn test_flavor_spec_parse_empty_brackets() {
        let spec = FlavorSpec::parse("[]").unwrap();
        assert!(spec.items.is_empty());
        assert!(spec.arch.is_none());
        assert!(spec.is_empty());
    }

    #[test]
    fn test_flavor_spec_parse_empty_string() {
        let spec = FlavorSpec::parse("").unwrap();
        assert!(spec.is_empty());
    }

    #[test]
    fn test_flavor_spec_parse_single_item() {
        let spec = FlavorSpec::parse("[ssl]").unwrap();
        assert_eq!(spec.items.len(), 1);
        assert_eq!(spec.items[0].op, FlavorOp::Required);
        assert_eq!(spec.items[0].name, "ssl");
        assert!(spec.arch.is_none());
    }

    #[test]
    fn test_flavor_spec_parse_arch_only() {
        let spec = FlavorSpec::parse("[is: x86_64]").unwrap();
        assert!(spec.items.is_empty());
        assert_eq!(
            spec.arch.as_ref().unwrap().architectures,
            vec!["x86_64".to_string()]
        );
    }

    #[test]
    fn test_flavor_spec_parse_multi_arch() {
        let spec = FlavorSpec::parse("[is: x86 x86_64]").unwrap();
        assert!(spec.items.is_empty());
        // Canonicalized (sorted)
        assert_eq!(
            spec.arch.as_ref().unwrap().architectures,
            vec!["x86".to_string(), "x86_64".to_string()]
        );
    }

    #[test]
    fn test_flavor_spec_parse_mixed() {
        let spec = FlavorSpec::parse("[ssl, !debug, is: x86_64]").unwrap();
        assert_eq!(spec.items.len(), 2);
        // Canonicalized (sorted by name): debug comes before ssl
        assert_eq!(spec.items[0].op, FlavorOp::Not);
        assert_eq!(spec.items[0].name, "debug");
        assert_eq!(spec.items[1].op, FlavorOp::Required);
        assert_eq!(spec.items[1].name, "ssl");
        assert_eq!(
            spec.arch.as_ref().unwrap().architectures,
            vec!["x86_64".to_string()]
        );
    }

    #[test]
    fn test_flavor_spec_parse_all_operators() {
        let spec = FlavorSpec::parse("[ssl, !debug, ~vmware, ~!xen]").unwrap();
        assert_eq!(spec.items.len(), 4);
        // Sorted: debug, ssl, vmware, xen
        assert_eq!(spec.items[0].name, "debug");
        assert_eq!(spec.items[0].op, FlavorOp::Not);
        assert_eq!(spec.items[1].name, "ssl");
        assert_eq!(spec.items[1].op, FlavorOp::Required);
        assert_eq!(spec.items[2].name, "vmware");
        assert_eq!(spec.items[2].op, FlavorOp::Prefers);
        assert_eq!(spec.items[3].name, "xen");
        assert_eq!(spec.items[3].op, FlavorOp::PrefersNot);
    }

    #[test]
    fn test_flavor_spec_parse_without_brackets() {
        let spec = FlavorSpec::parse("ssl, !debug").unwrap();
        assert_eq!(spec.items.len(), 2);
    }

    #[test]
    fn test_flavor_spec_parse_original_conary_example() {
        // From conaryopedia: [!dom0, ~!domU, ~vmware, ~!xen is: x86 x86_64]
        let spec = FlavorSpec::parse("[!dom0, ~!domU, ~vmware, ~!xen, is: x86 x86_64]").unwrap();
        assert_eq!(spec.items.len(), 4);
        assert!(spec.arch.is_some());
        assert_eq!(
            spec.arch.as_ref().unwrap().architectures,
            vec!["x86".to_string(), "x86_64".to_string()]
        );
    }

    // === Canonicalization tests ===

    #[test]
    fn test_flavor_spec_canonicalization_order() {
        let spec1 = FlavorSpec::parse("[ssl, debug]").unwrap();
        let spec2 = FlavorSpec::parse("[debug, ssl]").unwrap();
        assert_eq!(spec1.to_string(), spec2.to_string());
        assert_eq!(spec1.to_string(), "[debug, ssl]");
    }

    #[test]
    fn test_flavor_spec_canonicalization_arch_order() {
        let spec = FlavorSpec::parse("[is: x86_64 x86]").unwrap();
        // Sorted: x86 before x86_64
        assert_eq!(
            spec.arch.as_ref().unwrap().architectures,
            vec!["x86".to_string(), "x86_64".to_string()]
        );
        assert_eq!(spec.to_string(), "[is: x86 x86_64]");
    }

    #[test]
    fn test_flavor_spec_canonicalization_dedup_arch() {
        let spec = FlavorSpec::parse("[is: x86_64 x86 x86_64]").unwrap();
        // Deduped
        assert_eq!(spec.arch.as_ref().unwrap().architectures.len(), 2);
    }

    // === Display/round-trip tests ===

    #[test]
    fn test_flavor_spec_display_empty() {
        let spec = FlavorSpec::empty();
        assert_eq!(spec.to_string(), "");
    }

    #[test]
    fn test_flavor_spec_display_roundtrip() {
        let original = "[!debug, ssl, ~vmware, is: x86 x86_64]";
        let spec = FlavorSpec::parse(original).unwrap();
        let displayed = spec.to_string();
        let reparsed = FlavorSpec::parse(&displayed).unwrap();
        assert_eq!(spec, reparsed);
    }

    // === Matching tests ===

    #[test]
    fn test_matching_required_present() {
        let spec = FlavorSpec::parse("[ssl]").unwrap();
        let system = SystemFlavor::new("x86_64").with_feature("ssl");
        let (matches, score) = spec.matches(&system);
        assert!(matches);
        assert!(score > 0);
    }

    #[test]
    fn test_matching_required_absent() {
        let spec = FlavorSpec::parse("[ssl]").unwrap();
        let system = SystemFlavor::new("x86_64");
        let (matches, _) = spec.matches(&system);
        assert!(!matches);
    }

    #[test]
    fn test_matching_not_present() {
        let spec = FlavorSpec::parse("[!debug]").unwrap();
        let system = SystemFlavor::new("x86_64").with_feature("debug");
        let (matches, _) = spec.matches(&system);
        assert!(!matches);
    }

    #[test]
    fn test_matching_not_absent() {
        let spec = FlavorSpec::parse("[!debug]").unwrap();
        let system = SystemFlavor::new("x86_64");
        let (matches, score) = spec.matches(&system);
        assert!(matches);
        assert!(score > 0);
    }

    #[test]
    fn test_matching_prefers_scoring() {
        let spec = FlavorSpec::parse("[~vmware]").unwrap();
        let system_with = SystemFlavor::new("x86_64").with_feature("vmware");
        let system_without = SystemFlavor::new("x86_64");

        let (matches_with, score_with) = spec.matches(&system_with);
        let (matches_without, score_without) = spec.matches(&system_without);

        // Both match, but with feature should score higher
        assert!(matches_with);
        assert!(matches_without);
        assert!(score_with > score_without);
    }

    #[test]
    fn test_matching_prefers_not_scoring() {
        let spec = FlavorSpec::parse("[~!xen]").unwrap();
        let system_with = SystemFlavor::new("x86_64").with_feature("xen");
        let system_without = SystemFlavor::new("x86_64");

        let (matches_with, score_with) = spec.matches(&system_with);
        let (matches_without, score_without) = spec.matches(&system_without);

        // Both match, but without feature should score higher
        assert!(matches_with);
        assert!(matches_without);
        assert!(score_without > score_with);
    }

    #[test]
    fn test_matching_architecture() {
        let spec = FlavorSpec::parse("[is: x86_64]").unwrap();
        let system_match = SystemFlavor::new("x86_64");
        let system_no_match = SystemFlavor::new("aarch64");

        assert!(spec.matches(&system_match).0);
        assert!(!spec.matches(&system_no_match).0);
    }

    #[test]
    fn test_matching_multi_architecture() {
        let spec = FlavorSpec::parse("[is: x86 x86_64]").unwrap();
        let system_x86 = SystemFlavor::new("x86");
        let system_x86_64 = SystemFlavor::new("x86_64");
        let system_arm = SystemFlavor::new("aarch64");

        assert!(spec.matches(&system_x86).0);
        assert!(spec.matches(&system_x86_64).0);
        assert!(!spec.matches(&system_arm).0);
    }

    #[test]
    fn test_matching_empty_spec() {
        let spec = FlavorSpec::empty();
        let system = SystemFlavor::new("x86_64").with_feature("ssl");
        let (matches, score) = spec.matches(&system);
        assert!(matches);
        assert_eq!(score, 0); // No preferences to score
    }

    // === Select best tests ===

    #[test]
    fn test_select_best() {
        let candidates = vec![
            (FlavorSpec::parse("[ssl]").unwrap(), "pkg-ssl"),
            (FlavorSpec::parse("[!ssl]").unwrap(), "pkg-no-ssl"),
            (FlavorSpec::parse("[~ssl]").unwrap(), "pkg-prefers-ssl"),
        ];

        let system_with_ssl = SystemFlavor::new("x86_64").with_feature("ssl");
        let system_without_ssl = SystemFlavor::new("x86_64");

        // System with SSL should get pkg-ssl (required match beats preference)
        let best_with = FlavorSpec::select_best(&candidates, &system_with_ssl);
        assert_eq!(best_with, Some(&"pkg-ssl"));

        // System without SSL should get pkg-no-ssl
        let best_without = FlavorSpec::select_best(&candidates, &system_without_ssl);
        assert_eq!(best_without, Some(&"pkg-no-ssl"));
    }

    #[test]
    fn test_select_best_no_match() {
        let candidates = vec![(FlavorSpec::parse("[ssl]").unwrap(), "pkg-ssl")];

        let system = SystemFlavor::new("x86_64"); // No ssl feature
        let best = FlavorSpec::select_best(&candidates, &system);
        assert!(best.is_none());
    }
}
