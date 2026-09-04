// conary-core/src/flavor/mod.rs
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

                let architectures: Vec<String> =
                    arch_str.split_whitespace().map(|s| s.to_string()).collect();

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
                if matches { Some((score, item)) } else { None }
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
mod tests;
