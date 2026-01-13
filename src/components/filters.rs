// src/components/filters.rs

//! External filter loading for custom component classification
//!
//! This module provides support for loading custom classification rules from
//! configuration files, allowing users to override or extend the built-in
//! classifier behavior.
//!
//! # Filter Configuration Format
//!
//! Filters are defined in a simple text format, one rule per line:
//!
//! ```text
//! # Comments start with #
//! # Format: pattern -> component
//!
//! /opt/myapp/**  -> runtime
//! /opt/myapp/lib/*.so -> lib
//! /opt/myapp/include/** -> devel
//! ```
//!
//! # Filter Priority
//!
//! When multiple filters match a path, the most specific match wins.
//! Custom filters are applied before built-in rules by default.

use super::ComponentType;
use std::path::Path;

/// A single filter rule mapping a pattern to a component type
#[derive(Debug, Clone)]
pub struct FilterRule {
    /// The glob pattern to match
    pub pattern: String,
    /// The component type to assign to matching files
    pub component: ComponentType,
    /// Priority (higher = checked first)
    pub priority: i32,
}

impl FilterRule {
    /// Create a new filter rule
    pub fn new(pattern: impl Into<String>, component: ComponentType) -> Self {
        Self {
            pattern: pattern.into(),
            component,
            priority: 0,
        }
    }

    /// Set the priority for this rule
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Check if this rule matches a path
    pub fn matches(&self, path: &str) -> bool {
        glob_match(&self.pattern, path)
    }
}

/// External filter set loaded from configuration
#[derive(Debug, Default)]
pub struct FilterSet {
    /// Rules sorted by priority (highest first)
    rules: Vec<FilterRule>,
}

impl FilterSet {
    /// Create a new empty filter set
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Add a rule to the filter set
    pub fn add_rule(&mut self, rule: FilterRule) {
        self.rules.push(rule);
        // Re-sort by priority (highest first)
        self.rules.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// Load filters from a configuration file
    ///
    /// File format:
    /// ```text
    /// # Comment
    /// pattern -> component
    /// pattern -> component [priority]
    /// ```
    pub fn load_from_file(path: &Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::parse(&content))
    }

    /// Load filters from a configuration directory
    ///
    /// Loads all .conf files from the directory and merges them.
    pub fn load_from_dir(dir: &Path) -> std::io::Result<Self> {
        let mut filter_set = Self::new();

        if !dir.exists() {
            return Ok(filter_set);
        }

        let entries = std::fs::read_dir(dir)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "conf")
                && let Ok(loaded) = Self::load_from_file(&path)
            {
                for rule in loaded.rules {
                    filter_set.add_rule(rule);
                }
            }
        }

        Ok(filter_set)
    }

    /// Parse filter rules from a string
    pub fn parse(content: &str) -> Self {
        let mut filter_set = Self::new();

        for line in content.lines() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some(rule) = Self::parse_rule(line) {
                filter_set.add_rule(rule);
            }
        }

        filter_set
    }

    /// Parse a single rule line
    ///
    /// Format: `pattern -> component [priority]`
    fn parse_rule(line: &str) -> Option<FilterRule> {
        // Split on ->
        let parts: Vec<&str> = line.splitn(2, "->").collect();
        if parts.len() != 2 {
            return None;
        }

        let pattern = parts[0].trim();
        let rest = parts[1].trim();

        // Check for optional priority in brackets
        let (component_str, priority) = if let Some(bracket_start) = rest.find('[') {
            if let Some(bracket_end) = rest.find(']') {
                let comp = rest[..bracket_start].trim();
                let pri_str = rest[bracket_start + 1..bracket_end].trim();
                let priority = pri_str.parse().unwrap_or(0);
                (comp, priority)
            } else {
                (rest, 0)
            }
        } else {
            (rest, 0)
        };

        // Parse component type
        let component = ComponentType::parse(component_str)?;

        Some(FilterRule {
            pattern: pattern.to_string(),
            component,
            priority,
        })
    }

    /// Classify a path using the filter rules
    ///
    /// Returns Some if a rule matches, None if no rules match.
    pub fn classify(&self, path: &str) -> Option<ComponentType> {
        // Find the first matching rule (rules are sorted by priority)
        for rule in &self.rules {
            if rule.matches(path) {
                return Some(rule.component);
            }
        }
        None
    }

    /// Check if the filter set is empty
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Get the number of rules
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Get an iterator over the rules
    pub fn rules(&self) -> impl Iterator<Item = &FilterRule> {
        self.rules.iter()
    }
}

/// A classifier that combines external filters with the built-in classifier
pub struct FilteredClassifier {
    /// External filter rules (checked first)
    filters: FilterSet,
    /// Whether to use built-in classifier as fallback
    use_builtin_fallback: bool,
}

impl FilteredClassifier {
    /// Create a new filtered classifier with the given filter set
    pub fn new(filters: FilterSet) -> Self {
        Self {
            filters,
            use_builtin_fallback: true,
        }
    }

    /// Disable the built-in classifier fallback
    pub fn without_builtin(mut self) -> Self {
        self.use_builtin_fallback = false;
        self
    }

    /// Classify a path using filters, falling back to built-in if needed
    pub fn classify(&self, path: &str) -> ComponentType {
        // Try external filters first
        if let Some(component) = self.filters.classify(path) {
            return component;
        }

        // Fall back to built-in classifier
        if self.use_builtin_fallback {
            super::ComponentClassifier::classify(Path::new(path))
        } else {
            ComponentType::Runtime
        }
    }

    /// Classify multiple paths
    pub fn classify_all(
        &self,
        paths: &[String],
    ) -> std::collections::HashMap<ComponentType, Vec<String>> {
        let mut result = std::collections::HashMap::new();

        for path in paths {
            let comp_type = self.classify(path);
            result
                .entry(comp_type)
                .or_insert_with(Vec::new)
                .push(path.clone());
        }

        result
    }
}

/// Simple glob pattern matching
///
/// Supports:
/// - `*` - matches any sequence of characters within a path segment
/// - `**` - matches any sequence of characters including path separators
/// - `?` - matches any single character
fn glob_match(pattern: &str, path: &str) -> bool {
    glob_match_impl(pattern.as_bytes(), path.as_bytes())
}

fn glob_match_impl(pattern: &[u8], path: &[u8]) -> bool {
    let mut p = 0;
    let mut s = 0;
    let mut star_p = usize::MAX;
    let mut match_s = 0;

    while s < path.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == path[s]) {
            // Match single character
            p += 1;
            s += 1;
        } else if p + 1 < pattern.len() && pattern[p] == b'*' && pattern[p + 1] == b'*' {
            // ** matches everything including /
            star_p = p;
            match_s = s;
            p += 2;
            // Skip any trailing / after **
            if p < pattern.len() && pattern[p] == b'/' {
                p += 1;
            }
        } else if p < pattern.len() && pattern[p] == b'*' {
            // * matches anything except /
            if path[s] == b'/' {
                // Can't match / with single *
                if star_p != usize::MAX {
                    p = star_p + 2;
                    if p < pattern.len() && pattern[p - 1] == b'*' {
                        match_s += 1;
                        s = match_s;
                        continue;
                    }
                }
                return false;
            }
            star_p = p;
            match_s = s;
            p += 1;
        } else if star_p != usize::MAX {
            // Backtrack to last * or **
            if pattern.get(star_p + 1) == Some(&b'*') {
                // It was **, match one more character
                match_s += 1;
                s = match_s;
                p = star_p + 2;
                if p < pattern.len() && pattern[p] == b'/' {
                    p += 1;
                }
            } else {
                // It was *, but can't match /
                if path[match_s] == b'/' {
                    return false;
                }
                match_s += 1;
                s = match_s;
                p = star_p + 1;
            }
        } else {
            return false;
        }
    }

    // Check remaining pattern - consume trailing * or ** patterns
    while p < pattern.len() && pattern[p] == b'*' {
        // Check for ** (double star)
        if p + 1 < pattern.len() && pattern[p + 1] == b'*' {
            p += 2;
        } else {
            p += 1;
        }
        // Skip trailing slash after glob
        if p < pattern.len() && pattern[p] == b'/' {
            p += 1;
        }
    }

    p >= pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ====================
    // Glob matching tests
    // ====================

    #[test]
    fn test_glob_exact() {
        assert!(glob_match("/usr/bin/foo", "/usr/bin/foo"));
        assert!(!glob_match("/usr/bin/foo", "/usr/bin/bar"));
    }

    #[test]
    fn test_glob_single_star() {
        assert!(glob_match("/usr/bin/*", "/usr/bin/foo"));
        assert!(glob_match("/usr/bin/*", "/usr/bin/bar"));
        assert!(!glob_match("/usr/bin/*", "/usr/bin/foo/bar"));
    }

    #[test]
    fn test_glob_double_star() {
        assert!(glob_match("/usr/**", "/usr/bin/foo"));
        assert!(glob_match("/usr/**", "/usr/bin/foo/bar"));
        assert!(glob_match("/usr/**", "/usr/lib/x86_64/libfoo.so"));
    }

    #[test]
    fn test_glob_double_star_middle() {
        assert!(glob_match("/usr/**/lib", "/usr/lib"));
        assert!(glob_match("/usr/**/lib", "/usr/local/lib"));
        assert!(glob_match("/usr/**/lib", "/usr/local/share/lib"));
    }

    #[test]
    fn test_glob_question_mark() {
        assert!(glob_match("/usr/bin/fo?", "/usr/bin/foo"));
        assert!(glob_match("/usr/bin/fo?", "/usr/bin/fob"));
        assert!(!glob_match("/usr/bin/fo?", "/usr/bin/fooo"));
    }

    #[test]
    fn test_glob_extension() {
        assert!(glob_match("*.so", "libfoo.so"));
        assert!(glob_match("/usr/lib/*.so", "/usr/lib/libfoo.so"));
        assert!(!glob_match("*.so", "libfoo.so.1"));
    }

    // ====================
    // FilterRule tests
    // ====================

    #[test]
    fn test_filter_rule_new() {
        let rule = FilterRule::new("/opt/myapp/**", ComponentType::Runtime);
        assert_eq!(rule.pattern, "/opt/myapp/**");
        assert_eq!(rule.component, ComponentType::Runtime);
        assert_eq!(rule.priority, 0);
    }

    #[test]
    fn test_filter_rule_with_priority() {
        let rule = FilterRule::new("/opt/myapp/**", ComponentType::Runtime).with_priority(100);
        assert_eq!(rule.priority, 100);
    }

    #[test]
    fn test_filter_rule_matches() {
        let rule = FilterRule::new("/opt/myapp/**", ComponentType::Runtime);
        assert!(rule.matches("/opt/myapp/bin/foo"));
        assert!(rule.matches("/opt/myapp/lib/bar.so"));
        assert!(!rule.matches("/usr/bin/foo"));
    }

    // ====================
    // FilterSet parsing
    // ====================

    #[test]
    fn test_filter_set_parse_simple() {
        let content = r#"
            # Comment
            /opt/myapp/** -> runtime
            /opt/myapp/lib/*.so -> lib
        "#;

        let filters = FilterSet::parse(content);
        assert_eq!(filters.len(), 2);
    }

    #[test]
    fn test_filter_set_parse_with_priority() {
        let content = r#"
            /opt/myapp/** -> runtime [10]
            /opt/myapp/lib/*.so -> lib [100]
        "#;

        let filters = FilterSet::parse(content);
        // Higher priority should come first
        let rules: Vec<_> = filters.rules().collect();
        assert_eq!(rules[0].component, ComponentType::Lib);
        assert_eq!(rules[0].priority, 100);
        assert_eq!(rules[1].component, ComponentType::Runtime);
        assert_eq!(rules[1].priority, 10);
    }

    #[test]
    fn test_filter_set_parse_skip_invalid() {
        let content = r#"
            /opt/myapp/** -> runtime
            invalid line without arrow
            /opt/myapp/lib/*.so -> unknowntype
        "#;

        let filters = FilterSet::parse(content);
        // Only the valid line should be parsed
        assert_eq!(filters.len(), 1);
    }

    #[test]
    fn test_filter_set_classify() {
        let content = r#"
            /opt/myapp/lib/*.so -> lib [100]
            /opt/myapp/** -> runtime
        "#;

        let filters = FilterSet::parse(content);

        // Lib rule has higher priority
        assert_eq!(
            filters.classify("/opt/myapp/lib/foo.so"),
            Some(ComponentType::Lib)
        );

        // Runtime catches other paths
        assert_eq!(
            filters.classify("/opt/myapp/bin/foo"),
            Some(ComponentType::Runtime)
        );

        // No match
        assert_eq!(filters.classify("/usr/bin/foo"), None);
    }

    // ====================
    // FilteredClassifier tests
    // ====================

    #[test]
    fn test_filtered_classifier_with_filters() {
        let content = "/opt/myapp/** -> runtime";
        let filters = FilterSet::parse(content);
        let classifier = FilteredClassifier::new(filters);

        assert_eq!(
            classifier.classify("/opt/myapp/bin/foo"),
            ComponentType::Runtime
        );
    }

    #[test]
    fn test_filtered_classifier_fallback_to_builtin() {
        let filters = FilterSet::new();
        let classifier = FilteredClassifier::new(filters);

        // Should fall back to built-in classifier
        assert_eq!(
            classifier.classify("/usr/include/stdio.h"),
            ComponentType::Devel
        );
        assert_eq!(
            classifier.classify("/usr/share/doc/foo/README"),
            ComponentType::Doc
        );
    }

    #[test]
    fn test_filtered_classifier_override_builtin() {
        // Custom rule that overrides the built-in behavior
        let content = "/usr/include/** -> runtime";
        let filters = FilterSet::parse(content);
        let classifier = FilteredClassifier::new(filters);

        // Custom filter overrides built-in (headers would normally be :devel)
        assert_eq!(
            classifier.classify("/usr/include/stdio.h"),
            ComponentType::Runtime
        );
    }

    #[test]
    fn test_filtered_classifier_without_fallback() {
        let filters = FilterSet::new();
        let classifier = FilteredClassifier::new(filters).without_builtin();

        // Without fallback, everything becomes runtime
        assert_eq!(
            classifier.classify("/usr/include/stdio.h"),
            ComponentType::Runtime
        );
    }

    #[test]
    fn test_filtered_classifier_classify_all() {
        let content = r#"
            /opt/myapp/lib/*.so -> lib
            /opt/myapp/** -> runtime
        "#;
        let filters = FilterSet::parse(content);
        let classifier = FilteredClassifier::new(filters);

        let paths = vec![
            "/opt/myapp/bin/foo".to_string(),
            "/opt/myapp/lib/bar.so".to_string(),
            "/usr/include/stdio.h".to_string(), // Falls back to built-in
        ];

        let classified = classifier.classify_all(&paths);

        assert_eq!(
            classified.get(&ComponentType::Runtime).map(|v| v.len()),
            Some(1)
        );
        assert_eq!(
            classified.get(&ComponentType::Lib).map(|v| v.len()),
            Some(1)
        );
        assert_eq!(
            classified.get(&ComponentType::Devel).map(|v| v.len()),
            Some(1)
        );
    }
}
